//! Vanilla 26.3 ResourceManager contract, initialized by the application.
//! Application asset consumers remain on their separate filesystem stack until
//! migration.

use std::collections::{BTreeMap, HashSet};
use std::io;

use super::pack_resources::PackResources;
use super::{Resource, ResourceError, ResourceId, ResourceOrigin};

mod fallback;
mod multi_pack;
mod reloadable;

// Consumer-facing APIs remain available for the subsequent asset migration.
#[allow(unused_imports)]
pub(crate) use fallback::FallbackResourceManager;
#[allow(unused_imports)]
pub(crate) use multi_pack::MultiPackResourceManager;
pub(crate) use reloadable::ReloadableResourceManager;

/// Returns true when the resource identifier is included in an enumeration.
pub(crate) type Selector<'a> = dyn Fn(&ResourceId) -> bool + 'a;

fn io_error(
    operation: &'static str,
    origin: &ResourceOrigin,
    id: Option<&ResourceId>,
    source: io::Error,
) -> io::Error {
    io::Error::new(
        source.kind(),
        ResourceError::Io {
            operation,
            id: id.cloned(),
            origin: origin.clone(),
            source,
        },
    )
}

#[derive(Debug, thiserror::Error)]
#[error("namespace {namespace:?}, directory {directory:?}: {source}")]
struct ListingError {
    namespace: String,
    directory: String,
    #[source]
    source: io::Error,
}

fn listing_error(
    operation: &'static str,
    origin: &ResourceOrigin,
    namespace: &str,
    directory: &str,
    source: io::Error,
) -> io::Error {
    io::Error::new(
        source.kind(),
        ListingError {
            namespace: namespace.to_owned(),
            directory: directory.to_owned(),
            source: io_error(operation, origin, None, source),
        },
    )
}

/// Object-safe resource view matching Vanilla's manager operations.
/// ResourceProvider's core lookup is folded into this trait; convenience
/// opening helpers are not part of this contract.
/// I/O failures propagate rather than becoming absence or empty results.
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

    /// Borrows the configured packs without opening resource content.
    fn list_packs(&self) -> Box<dyn Iterator<Item = &dyn PackResources> + '_>;
}

#[cfg(test)]
mod tests {
    #![allow(
        dead_code,
        reason = "Generic API assertions pending user review before fixtures"
    )]

    use std::collections::{BTreeMap, HashSet};
    use std::io::Read;

    use super::{ResourceManager, Selector};
    use crate::resources::ResourceId;

    pub(super) fn test_get_resource<M: ResourceManager + ?Sized>(
        manager: &M,
        id: &ResourceId,
        expected: &[u8],
    ) {
        let resource = manager
            .get_resource(id)
            .expect("get_resource should succeed")
            .expect("get_resource should return a resource");
        let mut actual = Vec::new();
        resource
            .content()
            .open()
            .expect("get_resource content should open")
            .read_to_end(&mut actual)
            .expect("get_resource content should read");
        assert_eq!(actual, expected, "get_resource bytes for {id:?}");
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
        let resources = manager
            .get_resource_stack(id)
            .expect("get_resource_stack should succeed");
        let mut actual = Vec::new();
        for resource in resources {
            let mut bytes = Vec::new();
            resource
                .content()
                .open()
                .expect("get_resource_stack content should open")
                .read_to_end(&mut bytes)
                .expect("get_resource_stack content should read");
            actual.push(bytes);
        }
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
        let resources = manager
            .list_resources(directory, selector)
            .expect("list_resources should succeed");
        let mut actual = BTreeMap::new();
        for (id, resource) in resources {
            let mut bytes = Vec::new();
            resource
                .content()
                .open()
                .expect("list_resources content should open")
                .read_to_end(&mut bytes)
                .expect("list_resources content should read");
            actual.insert(id, bytes);
        }
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
        let resources = manager
            .list_resource_stacks(directory, selector)
            .expect("list_resource_stacks should succeed");
        let mut actual = BTreeMap::new();
        for (id, resources) in resources {
            let mut stack = Vec::new();
            for resource in resources {
                let mut bytes = Vec::new();
                resource
                    .content()
                    .open()
                    .expect("list_resource_stacks content should open")
                    .read_to_end(&mut bytes)
                    .expect("list_resource_stacks content should read");
                stack.push(bytes);
            }
            actual.insert(id, stack);
        }
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

    pub(super) fn test_list_packs<M: ResourceManager + ?Sized>(
        manager: &M,
        expected_location_ids: &[&str],
    ) {
        let actual: Vec<_> = manager
            .list_packs()
            .map(|pack| pack.location().id.as_str())
            .collect();
        assert_eq!(
            actual, expected_location_ids,
            "list_packs location IDs in order"
        );
    }
}
