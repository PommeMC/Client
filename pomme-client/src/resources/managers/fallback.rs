//! Vanilla's distinct lookup, stack and enumeration algorithms for one
//! namespace.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::rc::Rc;

use super::{ResourceManager, Selector};
use crate::resources::pack_resources::{IoSupplier, PackResources, PackType};
use crate::resources::{ReadStream, Resource, ResourceContent, ResourceId, ResourceOrigin};

pub(super) type PathFilter = Rc<dyn Fn(&str) -> bool>;
type MetadataFinder = Rc<dyn Fn() -> Option<ResourceContent>>;

// Deferred metadata lookups own both provenance and the backend, not the
// manager.
#[derive(Clone)]
struct ContentSource {
    origin: ResourceOrigin,
    pack: Rc<dyn PackResources>,
}

impl ContentSource {
    fn content(&self, id: ResourceId, supplier: IoSupplier<ReadStream>) -> ResourceContent {
        ResourceContent::from_supplier(id, self.origin.clone(), supplier)
    }

    /// Vanilla's `PackResources.getResource` never throws: a pack that can't
    /// answer is logged and counts as not having the resource.
    fn lookup(&self, pack_type: PackType, id: &ResourceId) -> Option<IoSupplier<ReadStream>> {
        self.pack
            .get_resource(pack_type, id)
            .inspect_err(|error| {
                tracing::warn!(%id, pack = %self.origin.name, %error, "Failed to look up resource in pack");
            })
            .ok()
            .flatten()
    }

    fn metadata(&self, pack_type: PackType, id: &ResourceId) -> Option<ResourceContent> {
        self.lookup(pack_type, id)
            .map(|supplier| self.content(id.clone(), supplier))
    }

    /// As `PathPackResources.listPath`, a listing error is logged and the pack
    /// keeps whatever it emitted before it.
    fn list(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut dyn FnMut(ResourceId, IoSupplier<ReadStream>),
    ) {
        if let Err(error) = self
            .pack
            .list_resources(pack_type, namespace, directory, output)
        {
            tracing::warn!(namespace, directory, pack = %self.origin.name, %error, "Failed to list resources in pack");
        }
    }
}

#[derive(Clone)]
struct PackEntry {
    name: String,
    source: Option<ContentSource>,
    filter: Option<PathFilter>,
}

impl PackEntry {
    fn is_filtered(&self, id: &ResourceId) -> bool {
        self.filter.as_ref().is_some_and(|filter| filter(id.path()))
    }
}

// Borrow sources only during enumeration; returned handles own their suppliers.
struct PackContentReference<'a> {
    source: &'a ContentSource,
    supplier: IoSupplier<ReadStream>,
}

struct EntryStack<'a> {
    metadata_id: ResourceId,
    files: Vec<PackContentReference<'a>>,
    metadata: Vec<PackContentReference<'a>>,
}

impl<'a> EntryStack<'a> {
    fn new(id: &ResourceId) -> Self {
        Self {
            metadata_id: id.sidecar(),
            files: Vec::new(),
            metadata: Vec::new(),
        }
    }

    fn apply_filter(&mut self, base_id: &ResourceId, entry: &PackEntry) {
        if entry.is_filtered(base_id) {
            self.files.clear();
        } else if entry.is_filtered(&self.metadata_id) {
            // Deliberate Vanilla else-if: a file filter does not also erase metadata.
            self.metadata.clear();
        }
    }

    fn push_file(&mut self, source: &'a ContentSource, supplier: IoSupplier<ReadStream>) {
        self.files.push(PackContentReference { source, supplier });
    }

    fn set_metadata(&mut self, source: &'a ContentSource, supplier: IoSupplier<ReadStream>) {
        let metadata = PackContentReference { source, supplier };
        if let Some(previous) = self
            .metadata
            .iter_mut()
            .find(|entry| Rc::ptr_eq(&entry.source.pack, &source.pack))
        {
            *previous = metadata;
        } else {
            self.metadata.push(metadata);
        }
    }

    fn into_resources(self, file_id: &ResourceId) -> Option<Vec<Resource>> {
        if self.files.is_empty() {
            return None;
        }
        Some(
            self.files
                .into_iter()
                .map(|PackContentReference { source, supplier }| {
                    let content = source.content(file_id.clone(), supplier);
                    let metadata = self
                        .metadata
                        .iter()
                        .find(|entry| Rc::ptr_eq(&entry.source.pack, &source.pack))
                        .map(|entry| {
                            entry
                                .source
                                .content(self.metadata_id.clone(), entry.supplier.clone())
                        });
                    Resource::new(content, constant_metadata(metadata))
                })
                .collect(),
        )
    }
}

/// Entries are ordered low to high. Returned resources share the entries; a
/// later push copies them, so it can't change a resource's metadata lookup.
/// Direct lookup respects this namespace partition, even if a registered pack
/// supplies other namespaces as well.
pub(crate) struct FallbackResourceManager {
    pack_type: PackType,
    namespace: String,
    entries: Rc<Vec<PackEntry>>,
}

impl FallbackResourceManager {
    pub(crate) fn new(pack_type: PackType, namespace: String) -> Self {
        Self {
            pack_type,
            namespace,
            entries: Rc::default(),
        }
    }

    pub(super) fn push(
        &mut self,
        origin: ResourceOrigin,
        pack: Rc<dyn PackResources>,
        filter: Option<PathFilter>,
    ) {
        Rc::make_mut(&mut self.entries).push(PackEntry {
            name: pack.pack_id().to_owned(),
            source: Some(ContentSource { origin, pack }),
            filter,
        });
    }

    pub(super) fn push_filter_only(&mut self, name: String, filter: PathFilter) {
        Rc::make_mut(&mut self.entries).push(PackEntry {
            name,
            source: None,
            filter: Some(filter),
        });
    }

    fn stack_metadata_finder(&self, id: &ResourceId, winning_index: usize) -> MetadataFinder {
        let entries = Rc::clone(&self.entries);
        let metadata_id = id.sidecar();
        let pack_type = self.pack_type;
        Rc::new(move || {
            for entry in entries[winning_index..].iter().rev() {
                if let Some(source) = &entry.source
                    && let Some(metadata) = source.metadata(pack_type, &metadata_id)
                {
                    return Some(metadata);
                }
                if entry.is_filtered(&metadata_id) {
                    break;
                }
            }
            None
        })
    }
}

impl ResourceManager for FallbackResourceManager {
    /// Scan high to low; defer metadata lookup to the owned winning-to-top
    /// snapshot.
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>> {
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if let Some(source) = &entry.source
                && let Some(supplier) = source.lookup(self.pack_type, id)
            {
                return Ok(Some(Resource::new(
                    source.content(id.clone(), supplier),
                    self.stack_metadata_finder(id, index),
                )));
            }
            // Own content wins before this pack's filter applies to lower packs.
            if entry.is_filtered(id) {
                tracing::warn!(%id, pack = %entry.name, "Resource not found, but filtered by pack");
                break;
            }
        }
        Ok(None)
    }

    fn get_namespaces(&self) -> io::Result<HashSet<String>> {
        Ok(HashSet::from([self.namespace.clone()]))
    }

    /// Scan high to low with source-local metadata; filters affect only lower
    /// entries.
    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>> {
        let metadata_id = id.sidecar();
        let mut resources = Vec::new();
        let mut metadata_blocked = false;
        for entry in self.entries.iter().rev() {
            if let Some(source) = &entry.source
                && let Some(supplier) = source.lookup(self.pack_type, id)
            {
                let metadata: MetadataFinder = if metadata_blocked {
                    constant_metadata(None)
                } else {
                    let source = source.clone();
                    let metadata_id = metadata_id.clone();
                    let pack_type = self.pack_type;
                    Rc::new(move || source.metadata(pack_type, &metadata_id))
                };
                resources.push(Resource::new(
                    source.content(id.clone(), supplier),
                    metadata,
                ));
            }
            if entry.is_filtered(id) {
                if resources.is_empty() {
                    tracing::warn!(%id, pack = %entry.name, "Resource not found, but filtered by pack");
                }
                break;
            }
            if entry.is_filtered(&metadata_id) {
                metadata_blocked = true;
            }
        }
        resources.reverse();
        Ok(resources)
    }

    /// Filter previous winners, then enumerate low to high; metadata uses
    /// priority, not identity.
    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        let mut files: BTreeMap<ResourceId, (usize, ResourceContent)> = BTreeMap::new();
        let mut metadata: BTreeMap<ResourceId, (usize, ResourceContent)> = BTreeMap::new();
        for (priority, entry) in self.entries.iter().enumerate() {
            files.retain(|id, _| !entry.is_filtered(id));
            metadata.retain(|id, _| !entry.is_filtered(id));
            if let Some(source) = &entry.source {
                source.list(
                    self.pack_type,
                    &self.namespace,
                    directory,
                    &mut |id, supplier| {
                        if let Some(base) = id.sidecar_base() {
                            if selector(&base) {
                                let content = source.content(id.clone(), supplier);
                                metadata.insert(id, (priority, content));
                            }
                        } else if selector(&id) {
                            let content = source.content(id.clone(), supplier);
                            files.insert(id, (priority, content));
                        }
                    },
                );
            }
        }
        Ok(files
            .into_iter()
            .map(|(id, (priority, content))| {
                let sidecar = metadata
                    .remove(&id.sidecar())
                    .filter(|(metadata_priority, _)| *metadata_priority >= priority)
                    .map(|(_, content)| content);
                (id, Resource::new(content, constant_metadata(sidecar)))
            })
            .collect())
    }

    /// Filter then append low to high; each stack pairs metadata by source
    /// identity, not priority.
    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        let mut stacks: BTreeMap<ResourceId, EntryStack<'_>> = BTreeMap::new();
        for entry in self.entries.iter() {
            for (id, stack) in &mut stacks {
                stack.apply_filter(id, entry);
            }
            if let Some(source) = &entry.source {
                source.list(
                    self.pack_type,
                    &self.namespace,
                    directory,
                    &mut |id, supplier| {
                        if let Some(base) = id.sidecar_base() {
                            if selector(&base) {
                                let stack = stacks
                                    .entry(base.clone())
                                    .or_insert_with(|| EntryStack::new(&base));
                                stack.set_metadata(source, supplier);
                            }
                        } else if selector(&id) {
                            let stack = stacks
                                .entry(id.clone())
                                .or_insert_with(|| EntryStack::new(&id));
                            stack.push_file(source, supplier);
                        }
                    },
                );
            }
        }
        Ok(stacks
            .into_iter()
            .filter_map(|(id, stack)| stack.into_resources(&id).map(|resources| (id, resources)))
            .collect())
    }
}

fn constant_metadata(content: Option<ResourceContent>) -> MetadataFinder {
    // Enumeration retains the exact supplier; metadata access must not relookup.
    Rc::new(move || content.clone())
}

#[cfg(test)]
mod resource_manager_tests {
    use super::FallbackResourceManager;
    use crate::resources::managers::tests;
    use crate::resources::pack_resources::PackType;

    fn fixture() -> FallbackResourceManager {
        let (origin, pack) = tests::embedded_pack();
        let mut manager =
            FallbackResourceManager::new(PackType::ClientResources, "pomme".to_owned());
        manager.push(origin, pack, None);
        manager
    }

    tests::embedded_pack_tests!(fixture);
}
