//! Vanilla 26.3 ResourceManager contract, initialized by the application.

use std::collections::{BTreeMap, HashSet};
use std::io;

use super::{Resource, ResourceId};

mod fallback;
mod multi_pack;
mod reloadable;

pub(crate) use multi_pack::MultiPackResourceManager;
pub(crate) use reloadable::ReloadableResourceManager;

/// Returns true when the resource identifier is included in an enumeration.
pub(crate) type Selector<'a> = dyn Fn(&ResourceId) -> bool + 'a;

/// Object-safe resource view matching Vanilla's manager operations.
/// ResourceProvider's core lookup is folded into this trait; convenience
/// opening helpers are not part of this contract.
/// A pack that fails a lookup or listing is logged and skipped, as vanilla's
/// packs never throw from these paths; only a malformed query is an error.
/// Returned resources reuse the existing handle and metadata ownership model.
pub(crate) trait ResourceManager {
    /// Resolves the highest-priority resource, or None when absent.
    fn get_resource(&self, id: &ResourceId) -> io::Result<Option<Resource>>;

    fn get_namespaces(&self) -> io::Result<HashSet<String>>;

    fn get_resource_stack(&self, id: &ResourceId) -> io::Result<Vec<Resource>>;

    /// Lists winning resources in identifier order, retaining selected IDs.
    fn list_resources(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Resource>>;

    /// Lists resource stacks in identifier order, retaining selected IDs.
    fn list_resource_stacks(
        &self,
        directory: &str,
        selector: &Selector<'_>,
    ) -> io::Result<BTreeMap<ResourceId, Vec<Resource>>>;
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::io::Read;
    use std::rc::Rc;

    use super::{ResourceManager, Selector};
    use crate::resources::pack_resources::{EmbeddedPackResources, PackResources};
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::{Resource, ResourceId, ResourceOrigin};

    pub(super) fn embedded_pack() -> (ResourceOrigin, Rc<dyn PackResources>) {
        let location = PackLocationInfo::test("resource-manager-api");
        let origin = ResourceOrigin {
            name: location.id.clone(),
        };
        (origin, Rc::new(EmbeddedPackResources::new(location)))
    }

    /// The `ResourceManager` contract over `embedded_pack`, for the manager
    /// that `$fixture()` builds around it.
    macro_rules! embedded_pack_tests {
        ($fixture:ident) => {
            use std::collections::{BTreeMap, HashSet};

            use crate::resources::pack_resources::{POMME_ICON, POMME_ICONS_FONT, POMME_UI_FONT};
            use crate::resources::resource_id;

            const ICON: &[u8] = include_bytes!("../../../assets/icon.png");
            const UI_FONT: &[u8] = include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf");
            const ICONS_FONT: &[u8] = include_bytes!("../../renderer/fonts/fa-solid-900.ttf");

            #[test]
            fn test_get_resource() {
                tests::test_get_resource(&$fixture(), &POMME_ICON, ICON);
            }

            #[test]
            fn test_get_resource_missing() {
                tests::test_get_resource_missing(
                    &$fixture(),
                    &resource_id!("pomme:textures/missing.png"),
                );
            }

            #[test]
            fn test_get_resource_stack() {
                tests::test_get_resource_stack(&$fixture(), &POMME_ICON, &[ICON.to_vec()]);
            }

            #[test]
            fn test_get_resource_stack_missing() {
                tests::test_get_resource_stack(
                    &$fixture(),
                    &resource_id!("pomme:textures/missing.png"),
                    &[],
                );
            }

            #[test]
            fn test_list_resources() {
                tests::test_list_resources(
                    &$fixture(),
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
                tests::test_list_resources(&$fixture(), "missing", &|_| true, &BTreeMap::new());
            }

            #[test]
            fn test_list_resource_stacks() {
                tests::test_list_resource_stacks(
                    &$fixture(),
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
                tests::test_list_resource_stacks(
                    &$fixture(),
                    "missing",
                    &|_| true,
                    &BTreeMap::new(),
                );
            }

            #[test]
            fn test_get_namespaces() {
                tests::test_get_namespaces(&$fixture(), &HashSet::from(["pomme".to_owned()]));
            }
        };
    }
    pub(super) use embedded_pack_tests;

    fn read(resource: &Resource) -> Vec<u8> {
        let mut bytes = Vec::new();
        resource
            .content()
            .open()
            .expect("resource content should open")
            .read_to_end(&mut bytes)
            .expect("resource content should read");
        bytes
    }

    pub(super) fn test_get_resource<M: ResourceManager + ?Sized>(
        manager: &M,
        id: &ResourceId,
        expected: &[u8],
    ) {
        let resource = manager
            .get_resource(id)
            .expect("get_resource should succeed")
            .expect("get_resource should return a resource");
        assert_eq!(read(&resource), expected, "get_resource bytes for {id:?}");
    }

    pub(super) fn test_get_resource_missing<M: ResourceManager + ?Sized>(
        manager: &M,
        id: &ResourceId,
    ) {
        assert!(
            manager
                .get_resource(id)
                .expect("get_resource should succeed")
                .is_none(),
            "get_resource should return None for {id:?}"
        );
    }

    pub(super) fn test_get_resource_stack<M: ResourceManager + ?Sized>(
        manager: &M,
        id: &ResourceId,
        expected: &[Vec<u8>],
    ) {
        let actual: Vec<_> = manager
            .get_resource_stack(id)
            .expect("get_resource_stack should succeed")
            .iter()
            .map(read)
            .collect();
        assert_eq!(
            actual, expected,
            "get_resource_stack bytes in order for {id:?}"
        );
    }

    pub(super) fn test_list_resources<M: ResourceManager + ?Sized>(
        manager: &M,
        directory: &str,
        selector: &Selector<'_>,
        expected: &BTreeMap<ResourceId, Vec<u8>>,
    ) {
        let actual: BTreeMap<_, _> = manager
            .list_resources(directory, selector)
            .expect("list_resources should succeed")
            .into_iter()
            .map(|(id, resource)| (id, read(&resource)))
            .collect();
        assert_eq!(
            &actual, expected,
            "list_resources IDs and bytes under {directory:?}"
        );
    }

    pub(super) fn test_list_resource_stacks<M: ResourceManager + ?Sized>(
        manager: &M,
        directory: &str,
        selector: &Selector<'_>,
        expected: &BTreeMap<ResourceId, Vec<Vec<u8>>>,
    ) {
        let actual: BTreeMap<_, _> = manager
            .list_resource_stacks(directory, selector)
            .expect("list_resource_stacks should succeed")
            .into_iter()
            .map(|(id, stack)| (id, stack.iter().map(read).collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            &actual, expected,
            "list_resource_stacks IDs and ordered bytes under {directory:?}"
        );
    }

    pub(super) fn test_get_namespaces<M: ResourceManager + ?Sized>(
        manager: &M,
        expected: &HashSet<String>,
    ) {
        let actual = manager
            .get_namespaces()
            .expect("get_namespaces should succeed");
        assert_eq!(&actual, expected, "get_namespaces result");
    }
}
