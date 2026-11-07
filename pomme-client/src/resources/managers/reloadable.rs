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
        self.reload_with_preparation(packs, |_| Ok(()), rebuild)
    }

    /// Prepare consumer-owned data against the candidate before publishing it.
    /// On failure neither activation nor rebuild runs; dropping the candidate
    /// does not explicitly close backends shared with retained views.
    pub(crate) fn reload_with_preparation<T>(
        &mut self,
        packs: Vec<(ResourceOrigin, Rc<dyn PackResources>)>,
        prepare: impl FnOnce(&dyn ResourceManager) -> io::Result<T>,
        rebuild: impl FnOnce(&Self),
    ) -> io::Result<T> {
        let candidate = MultiPackResourceManager::new(self.pack_type, packs)?;
        let prepared = prepare(&candidate)?;
        self.active = candidate;
        rebuild(self);
        Ok(prepared)
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

#[cfg(test)]
mod resource_manager_tests {
    use std::collections::{BTreeMap, HashSet};
    use std::rc::Rc;

    use super::ReloadableResourceManager;
    use crate::chat_component::Component;
    use crate::resources::managers::tests;
    use crate::resources::pack_resources::{
        EmbeddedPackResources, EmbeddedResourcesSupplier, POMME_ICON, POMME_ICONS_FONT,
        POMME_UI_FONT, PackResources, PackType,
    };
    use crate::resources::packs::{
        FeatureFlagSet, Pack, PackCompat, PackEnablementConfig, PackLocationInfo, PackMetadata,
        PackPosition, PackSource,
    };
    use crate::resources::{ResourceId, resource_id};

    const ICON: &[u8] = include_bytes!("../../../assets/icon.png");
    const UI_FONT: &[u8] = include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf");
    const ICONS_FONT: &[u8] = include_bytes!("../../renderer/fonts/fa-solid-900.ttf");

    fn fixture() -> ReloadableResourceManager {
        let location = PackLocationInfo {
            id: "resource-manager-api".to_owned(),
            title: Component::text("Resource manager API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        };
        let metadata = PackMetadata {
            description: Component::text("Resource manager API fixture"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::empty(),
            overlays: Vec::new(),
        };
        let enablement = PackEnablementConfig {
            required: false,
            default_position: PackPosition::Top,
            fixed_position: false,
        };
        let pack = Pack::new(
            location.clone(),
            metadata,
            enablement,
            Rc::new(EmbeddedResourcesSupplier::new()),
        );
        let backend: Rc<dyn PackResources> = Rc::new(EmbeddedPackResources::new(location));
        let mut manager = ReloadableResourceManager::new(PackType::ClientResources);
        manager
            .reload(vec![(pack.origin(), backend)], |_| {})
            .expect("single embedded pack fixture should reload");
        manager
    }

    #[test]
    fn test_get_resource() {
        tests::test_get_resource(&fixture(), &POMME_ICON, ICON);
    }

    #[test]
    fn test_get_resource_missing() {
        tests::test_get_resource_missing(&fixture(), &resource_id!("pomme:textures/missing.png"));
    }

    #[test]
    fn test_get_resource_stack() {
        tests::test_get_resource_stack(&fixture(), &POMME_ICON, &[ICON.to_vec()]);
    }

    #[test]
    fn test_get_resource_stack_missing() {
        tests::test_get_resource_stack(
            &fixture(),
            &resource_id!("pomme:textures/missing.png"),
            &[],
        );
    }

    #[test]
    fn test_list_resources() {
        tests::test_list_resources(
            &fixture(),
            "font",
            &|_| true,
            &BTreeMap::from([
                (POMME_UI_FONT, UI_FONT.to_vec()),
                (POMME_ICONS_FONT, ICONS_FONT.to_vec()),
            ]),
        );
    }

    #[test]
    fn test_list_resources_missing_directory() {
        tests::test_list_resources(&fixture(), "missing", &|_| true, &BTreeMap::new());
    }

    #[test]
    fn test_list_resource_stacks() {
        tests::test_list_resource_stacks(
            &fixture(),
            "font",
            &|_| true,
            &BTreeMap::from([
                (POMME_UI_FONT, vec![UI_FONT.to_vec()]),
                (POMME_ICONS_FONT, vec![ICONS_FONT.to_vec()]),
            ]),
        );
    }

    #[test]
    fn test_list_resource_stacks_missing_directory() {
        tests::test_list_resource_stacks(&fixture(), "missing", &|_| true, &BTreeMap::new());
    }

    #[test]
    fn test_get_namespaces() {
        tests::test_get_namespaces(&fixture(), &HashSet::from(["pomme".to_owned()]));
    }

    #[test]
    fn test_list_packs() {
        tests::test_list_packs(&fixture(), &["resource-manager-api"]);
    }
}
