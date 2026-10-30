use std::collections::BTreeSet;
use std::rc::Rc;

use super::ResourceSources;
use super::packs::providers::validate_query;
use super::packs::{ResourceConfiguration, ResourcePack, ResourcePackDescriptor};
use super::resource::{Resource, ResourceError, ResourceId};

/// App-lifetime owner of the active source stack, ordered low-to-high priority.
/// Pack discovery and selection belong to pack management.
pub(crate) struct ResourceManager {
    packs: Vec<ResourcePack>,
}

impl ResourceManager {
    pub(crate) fn from_configuration(
        configuration: &ResourceConfiguration,
    ) -> Result<Self, ResourceError> {
        Self::open_sources(configuration.sources()?)
    }

    #[cfg(test)]
    pub(crate) fn from_sources(sources: ResourceSources) -> Result<Self, ResourceError> {
        Self::open_sources(sources)
    }

    fn open_sources(sources: ResourceSources) -> Result<Self, ResourceError> {
        let packs = sources
            .0
            .into_iter()
            .map(ResourcePackDescriptor::open)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { packs })
    }

    /// Prepare and open every candidate before activation; failure preserves
    /// the active sources and skips the callback. Existing handles retain old
    /// providers, not immutable filesystem bytes. Consumer failures do not roll
    /// back.
    pub(crate) fn reload(
        &mut self,
        configuration: &ResourceConfiguration,
        rebuild: impl FnOnce(&Self),
    ) -> Result<(), ResourceError> {
        let candidate = Self::open_sources(configuration.sources()?)?;
        self.packs = candidate.packs;
        // Vanilla stores PreparableReloadListener objects and coordinates async
        // prepare/apply. Pomme supplies a synchronous callback so consumers stay
        // app-owned without ownership cycles.
        rebuild(self);
        Ok(())
    }

    pub(crate) fn namespaces(&self) -> Result<BTreeSet<String>, ResourceError> {
        let mut namespaces = BTreeSet::new();
        for source in self.packs.iter().flat_map(|pack| &pack.sources) {
            namespaces.extend(source.namespaces()?);
        }
        Ok(namespaces)
    }

    /// Highest-priority match for a checked ID; access errors never fall back.
    /// Lazy metadata comes from the winning source or higher, never lower.
    /// Extracted images may use external sidecars, but not the reverse.
    pub(crate) fn get_resource(&self, id: &ResourceId) -> Result<Option<Resource>, ResourceError> {
        for (pack_index, pack) in self.packs.iter().enumerate().rev() {
            for (source_index, source) in pack.sources.iter().enumerate().rev() {
                let Some(content) = source.clone().get_resource(id)? else {
                    continue;
                };
                let metadata_packs = self.packs[pack_index..].to_vec();
                let id = id.clone();
                let metadata = Rc::new(move || {
                    let metadata_id = ResourceId::new(&format!("{id}.mcmeta"))?;
                    for (index, candidate) in metadata_packs.iter().enumerate().rev() {
                        // Keep the physical-layer boundary even within one pack:
                        // external content cannot inherit a lower extracted sidecar.
                        let start = if index == 0 { source_index } else { 0 };
                        for source in candidate.sources[start..].iter().rev() {
                            if let Some(sidecar) = source.clone().get_resource(&metadata_id)? {
                                return Ok(Some(sidecar));
                            }
                        }
                        if candidate.filter.blocks(&metadata_id) {
                            break;
                        }
                    }
                    Ok(None)
                });
                return Ok(Some(Resource::new(content, metadata)));
            }
            if pack.filter.blocks(id) {
                break;
            }
        }
        Ok(None)
    }

    /// All matches low-to-high, empty on absence; access errors never fall
    /// back. Each entry's lazy, uncached metadata comes only from its own
    /// source, unless a higher pack filters that sidecar ID.
    pub(crate) fn get_resource_stack(
        &self,
        id: &ResourceId,
    ) -> Result<Vec<Resource>, ResourceError> {
        let mut resources = Vec::new();
        let metadata_id = ResourceId::new(&format!("{id}.mcmeta"))?;
        let mut metadata_blocked = false;
        // Walk high-to-low so blocked lower storage is never accessed.
        for pack in self.packs.iter().rev() {
            for source in pack.sources.iter().rev() {
                let Some(content) = source.clone().get_resource(id)? else {
                    continue;
                };
                let source = source.clone();
                let metadata_id = metadata_id.clone();
                let metadata = Rc::new(move || {
                    if metadata_blocked {
                        Ok(None)
                    } else {
                        source.clone().get_resource(&metadata_id)
                    }
                });
                resources.push(Resource::new(content, metadata));
            }
            if pack.filter.blocks(id) {
                break;
            }
            metadata_blocked |= pack.filter.blocks(&metadata_id);
        }
        resources.reverse();
        Ok(resources)
    }

    /// All-namespace query without sidecars, in namespace-then-path order.
    /// Enumerates IDs without resolving resources; enumeration errors
    /// propagate.
    pub(crate) fn list_resource_ids(
        &self,
        prefix: &str,
        selector: impl Fn(&ResourceId) -> bool,
    ) -> Result<BTreeSet<ResourceId>, ResourceError> {
        validate_query(None, prefix)?;
        let mut ids = BTreeSet::new();
        for pack in &self.packs {
            ids.retain(|id| !pack.filter.blocks(id));
            for source in &pack.sources {
                for namespace in source.namespaces()? {
                    ids.extend(source.list_resources(&namespace, prefix)?);
                }
            }
        }
        ids.retain(|id| !id.path().ends_with(".mcmeta") && selector(id));
        Ok(ids)
    }
}
