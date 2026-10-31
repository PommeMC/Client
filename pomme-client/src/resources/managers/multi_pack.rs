//! Namespace routing and filter preparation across low-to-high priority packs.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io;
use std::rc::Rc;

use super::fallback::{FallbackResourceManager, PathFilter};
use super::{ResourceManager, Selector, io_error};
use crate::resources::pack_resources::{PackResources, PackType};
use crate::resources::packs::{ResourceFilter, parse_filter};
use crate::resources::{Resource, ResourceId, ResourceOrigin};

/// Owns a prepared view, not immutable bytes. Supplied origins preserve opaque
/// registration identity across reloads; textual pack IDs are diagnostics only.
/// Dropping this manager never explicitly closes shared backends.
pub(crate) struct MultiPackResourceManager {
    packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
    namespaces: BTreeMap<String, FallbackResourceManager>,
}

impl MultiPackResourceManager {
    pub(crate) fn new(
        pack_type: PackType,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
    ) -> io::Result<Self> {
        let mut prepared = Vec::with_capacity(packs.len());
        let mut namespaces = BTreeSet::new();
        for (origin, pack) in &packs {
            let provided = pack
                .get_namespaces(pack_type)
                .map_err(|error| io_error("get namespaces", origin, None, error))?;
            namespaces.extend(provided.iter().cloned());
            // Storage/root errors propagate; invalid decoded sections are ignored
            // as a whole, never retaining a successfully parsed prefix.
            let section = pack.metadata_section_json("filter").map_err(|error| {
                io_error("load pack.mcmeta filter section", origin, None, error)
            })?;
            let filter = parse_filter(section.as_ref()).unwrap_or_else(|error| {
                tracing::warn!(pack = pack.pack_id(), %error,
                    "Invalid pack.mcmeta filter metadata; ignoring entire filter section (Rust regex syntax, not full Java Pattern support)");
                ResourceFilter::default()
            });
            prepared.push((provided, Rc::new(filter)));
        }
        let mut managers = BTreeMap::new();
        for ((origin, pack), (provided, filter)) in packs.iter().zip(prepared) {
            let path_filter: PathFilter = {
                let filter = filter.clone();
                Rc::new(move |path| filter.path_blocked(path))
            };
            for namespace in &namespaces {
                let has_content = provided.contains(namespace);
                let is_filtered = filter.namespace_blocked(namespace);
                if !has_content && !is_filtered {
                    continue;
                }
                let manager = managers
                    .entry(namespace.clone())
                    .or_insert_with(|| FallbackResourceManager::new(pack_type, namespace.clone()));
                if has_content {
                    manager.push(
                        origin.clone(),
                        pack.clone(),
                        is_filtered.then(|| path_filter.clone()),
                    );
                } else {
                    manager.push_filter_only(pack.pack_id().to_owned(), path_filter.clone());
                }
            }
        }
        Ok(Self {
            packs,
            namespaces: managers,
        })
    }

    pub(super) fn empty(_pack_type: PackType) -> Self {
        Self {
            packs: Vec::new(),
            namespaces: BTreeMap::new(),
        }
    }

    /// Explicitly closes every original registration once, not once per
    /// namespace. This may invalidate retained handles and sibling backend
    /// views. Attempts all entries, returning the first failure and logging
    /// later ones.
    pub(crate) fn close(&self) -> io::Result<()> {
        let mut first_error = None;
        for (origin, pack) in &self.packs {
            if let Err(error) = pack.close() {
                if first_error.is_none() {
                    first_error = Some(io_error("close pack", origin, None, error));
                } else {
                    tracing::warn!(pack = %origin.name, pack_id = ?origin.pack_id, %error,
                        "Additional pack close failure after an earlier error");
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ResourceManager for MultiPackResourceManager {
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>> {
        match self.namespaces.get(id.namespace()) {
            Some(manager) => manager.get_resource(id),
            None => Ok(None),
        }
    }

    fn get_namespaces(&self) -> io::Result<HashSet<String>> {
        Ok(self.namespaces.keys().cloned().collect())
    }

    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>> {
        match self.namespaces.get(id.namespace()) {
            Some(manager) => manager.get_resource_stack(id),
            None => Ok(Vec::new()),
        }
    }

    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        check_directory(directory)?;
        let mut result = BTreeMap::new();
        for manager in self.namespaces.values() {
            result.extend(manager.list_resources(directory, selector)?);
        }
        Ok(result)
    }

    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        check_directory(directory)?;
        let mut result = BTreeMap::new();
        for manager in self.namespaces.values() {
            result.extend(manager.list_resource_stacks(directory, selector)?);
        }
        Ok(result)
    }

    fn list_packs(&self) -> Box<dyn Iterator<Item = &dyn PackResources> + '_> {
        Box::new(self.packs.iter().map(|(_, pack)| pack.as_ref()))
    }
}

fn check_directory(directory: &str) -> io::Result<()> {
    if directory.ends_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Trailing slash in resource directory {directory:?}"),
        ));
    }
    Ok(())
}
