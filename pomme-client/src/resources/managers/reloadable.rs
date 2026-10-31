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
            active: MultiPackResourceManager::empty(pack_type),
        }
    }

    /// Prepare namespace/filter state fully before activation. Failure
    /// preserves the active view and skips rebuilding; success calls
    /// rebuild exactly once. This validates setup, not all future supplier
    /// I/O. Reused pack objects retain their backend metadata caches. A
    /// rebuild panic does not roll back.
    ///
    /// Unlike Vanilla's close-first reload, neither old packs nor failed
    /// candidates are explicitly closed: retained resources and candidate packs
    /// can share ZIP state, even through distinct sibling views. Resource
    /// metadata closures own snapshots rather than borrowing this mutable
    /// active view.
    pub(crate) fn reload(
        &mut self,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
        rebuild: impl FnOnce(&Self),
    ) -> io::Result<()> {
        let candidate = MultiPackResourceManager::new(self.pack_type, packs)?;
        self.active = candidate;
        rebuild(self);
        Ok(())
    }

    /// Caller-requested closure only; may invalidate old handles or shared
    /// views. Reload and Drop never call this method automatically.
    pub(crate) fn close(&self) -> io::Result<()> {
        self.active.close()
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

    fn list_packs(&self) -> Box<dyn Iterator<Item = &dyn PackResources> + '_> {
        self.active.list_packs()
    }
}
