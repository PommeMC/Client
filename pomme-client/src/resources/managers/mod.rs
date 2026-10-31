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
