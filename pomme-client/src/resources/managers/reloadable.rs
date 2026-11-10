//! Prepare-before-swap replacement of the application's active resource view.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::rc::Rc;

use super::multi_pack::MultiPackResourceManager;
use super::{ResourceManager, Selector};
use crate::resources::pack_resources::{PackResources, PackType};
use crate::resources::{Resource, ResourceId, ResourceOrigin};

pub(crate) struct ReloadableResourceManager {
    pack_type: PackType,
    active: MultiPackResourceManager,
}

impl ReloadableResourceManager {
    pub(crate) fn new(pack_type: PackType) -> Self {
        Self {
            pack_type,
            active: MultiPackResourceManager::empty(),
        }
    }

    /// Prepare namespace/filter state fully before activation, then call
    /// rebuild exactly once. Reused pack objects retain their backend metadata
    /// caches. A rebuild panic does not roll back.
    ///
    /// Unlike Vanilla's close-first reload, old packs are not explicitly
    /// closed: retained resources and new packs can share ZIP state, even
    /// through distinct sibling views. Resource
    /// metadata closures own snapshots rather than borrowing this mutable
    /// active view.
    #[cfg(test)]
    pub(crate) fn reload(
        &mut self,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
        rebuild: impl FnOnce(&Self),
    ) {
        self.active = MultiPackResourceManager::new(self.pack_type, packs);
        rebuild(self);
    }

    /// Prepare consumer-owned data against the candidate before publishing it.
    /// On failure neither activation nor rebuild runs.
    pub(crate) fn reload_with_preparation<T>(
        &mut self,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
        prepare: impl FnOnce(&dyn ResourceManager) -> io::Result<T>,
        rebuild: impl FnOnce(&Self),
    ) -> io::Result<T> {
        let candidate = MultiPackResourceManager::new(self.pack_type, packs);
        let prepared = prepare(&candidate)?;
        self.active = candidate;
        rebuild(self);
        Ok(prepared)
    }
}

impl ResourceManager for ReloadableResourceManager {
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>> {
        self.active.get_resource(id)
    }

    fn get_namespaces(&self) -> io::Result<HashSet<String>> {
        self.active.get_namespaces()
    }

    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>> {
        self.active.get_resource_stack(id)
    }

    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>> {
        self.active.list_resources(directory, selector)
    }

    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>> {
        self.active.list_resource_stacks(directory, selector)
    }
}

#[cfg(test)]
mod resource_manager_tests {
    use super::ReloadableResourceManager;
    use crate::resources::managers::tests;
    use crate::resources::pack_resources::PackType;

    fn fixture() -> ReloadableResourceManager {
        let mut manager = ReloadableResourceManager::new(PackType::ClientResources);
        manager.reload(vec![tests::embedded_pack()], |_| {});
        manager
    }

    tests::embedded_pack_tests!(fixture);
}
