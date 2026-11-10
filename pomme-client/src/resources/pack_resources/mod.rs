//! Storage contracts used by resource managers and repository suppliers.
//! Maps Vanilla 26.3 PackResources, PackMetadataResources, PackType and
//! IoSupplier into Rust. Fallible lookup uses Result<Option<_>>; the managers
//! turn a pack's errors into absence. Suppliers own their captured state.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use serde_json::Value;

use crate::resources::packs::PackLocationInfo;
use crate::resources::resource::{ReadStream, ResourceId};

mod embedded;
mod file;
mod mapped;
mod overlayed;
mod path;
mod vanilla;

#[cfg(test)]
pub(crate) use embedded::EmbeddedPackResources;
pub(crate) use embedded::{EmbeddedResourcesSupplier, POMME_ICON, POMME_ICONS_FONT, POMME_UI_FONT};
pub(crate) use file::FileResourcesSupplier;
pub(crate) use mapped::MappedResourcesSupplier;
pub(crate) use overlayed::OverlayedPackResources;
pub(crate) use path::{PathPackResources, PathResourcesSupplier};
pub(crate) use vanilla::VanillaResourcesSupplier;

pub(crate) const METADATA_EXTENSION: &str = ".mcmeta";
pub(crate) const PACK_META: &str = "pack.mcmeta";

/// Owned, repeatable supplier shared across threads; invocation may fail.
/// Captures must retain any needed storage leases rather than borrow a pack or
/// manager. Owned streams are memory-safe beyond those objects' lifetimes, but
/// continued usability after explicit pack closure is a backend policy.
pub(crate) type IoSupplier<T> = Arc<dyn Fn() -> io::Result<T> + Send + Sync + 'static>;

/// Enumeration transfers identifiers and unopened suppliers to the caller.
pub(crate) type ResourceOutput<'a> = dyn FnMut(ResourceId, IoSupplier<ReadStream>) + 'a;

/// Vanilla `PackType`. Only client resources exist here; server data is the
/// integrated server's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PackType {
    ClientResources,
}

impl PackType {
    pub(crate) const fn directory(self) -> &'static str {
        match self {
            Self::ClientResources => "assets",
        }
    }
}

/// Object-safe pack identity, root resources, metadata and explicit closure.
/// A backend reports its I/O failures; the managers log them and treat the
/// pack as not having the resource.
pub(crate) trait PackMetadataResources {
    fn location(&self) -> &PackLocationInfo;

    /// Vanilla's variadic root path represented as borrowed path segments.
    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>>;

    /// Rust object-safety adapter: returns the selected section, not the entire
    /// pack.mcmeta envelope. A missing file or absent key yields None;
    /// malformed JSON or a non-object root is an error. A present null is
    /// Some(Value::Null) and must reach the codec. Backend implementations
    /// own metadata caching and its lifecycle; this contract supplies no
    /// cache or parser.
    fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>>;

    /// Required backend operation; &self permits shared ownership with interior
    /// state. During migration each backend must define repeated-close behavior
    /// and the effect on retained suppliers and open streams. No idempotence or
    /// continued usability is promised here; this is not an automatic Drop
    /// hook.
    fn close(&self) -> io::Result<()>;
}

/// Object-safe resource access in a pack. Lookup and enumeration do not open
/// content streams; their suppliers perform that I/O on demand.
pub(crate) trait PackResources: PackMetadataResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>>;

    /// Enumerates under a directory prefix; an empty prefix is allowed.
    /// Delivers owned IDs and suppliers without opening their streams.
    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()>;

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>>;

    fn pack_id(&self) -> &str {
        self.location().id.as_str()
    }
}

#[cfg(test)]
mod tests {

    use std::collections::HashSet;
    use std::io::Read;

    use serde_json::Value;

    use super::{PackMetadataResources, PackResources, PackType};
    use crate::resources::ResourceId;
    use crate::resources::packs::PackLocationInfo;

    pub(super) fn test_get_resource<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        id: &ResourceId,
        expected: &[u8],
    ) {
        let supplier = pack
            .get_resource(pack_type, id)
            .expect("get_resource should succeed")
            .expect("get_resource should return a supplier");
        let mut actual = Vec::new();
        supplier()
            .expect("get_resource supplier should open")
            .read_to_end(&mut actual)
            .expect("get_resource content should read");
        assert_eq!(
            actual, expected,
            "get_resource bytes for {pack_type:?}, {id:?}"
        );
    }

    pub(super) fn test_get_resource_missing<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        id: &ResourceId,
    ) {
        assert!(
            pack.get_resource(pack_type, id)
                .expect("get_resource should succeed")
                .is_none(),
            "get_resource should return None for {pack_type:?}, {id:?}"
        );
    }

    pub(super) fn test_list_resources<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        expected: &[(ResourceId, Vec<u8>)],
    ) {
        let mut actual = Vec::new();
        pack.list_resources(pack_type, namespace, directory, &mut |id, supplier| {
            let mut bytes = Vec::new();
            supplier()
                .expect("list_resources supplier should open")
                .read_to_end(&mut bytes)
                .expect("list_resources content should read");
            actual.push((id, bytes));
        })
        .expect("list_resources should succeed");
        actual.sort();
        let mut expected = expected.to_vec();
        expected.sort();
        assert_eq!(
            actual, expected,
            "list_resources IDs and bytes for {pack_type:?}, {namespace:?}, {directory:?}"
        );
    }

    pub(super) fn test_get_namespaces<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        expected: &HashSet<String>,
    ) {
        let actual = pack
            .get_namespaces(pack_type)
            .expect("get_namespaces should succeed");
        assert_eq!(&actual, expected, "get_namespaces result for {pack_type:?}");
    }

    pub(super) fn test_pack_id<P: PackResources + ?Sized>(pack: &P, expected: &str) {
        assert_eq!(pack.pack_id(), expected, "pack_id result");
    }

    pub(super) fn test_location<P: PackMetadataResources + ?Sized>(
        pack: &P,
        expected: &PackLocationInfo,
    ) {
        let actual = pack.location();
        assert_eq!(actual.id, expected.id, "location ID");
        assert_eq!(actual.title, expected.title, "location title");
        assert_eq!(
            actual.known_pack_info, expected.known_pack_info,
            "location known pack"
        );
    }

    pub(super) fn test_get_root_resource<P: PackMetadataResources + ?Sized>(
        pack: &P,
        path: &[&str],
        expected: &[u8],
    ) {
        let supplier = pack
            .get_root_resource(path)
            .expect("get_root_resource should succeed")
            .expect("get_root_resource should return a supplier");
        let mut actual = Vec::new();
        supplier()
            .expect("get_root_resource supplier should open")
            .read_to_end(&mut actual)
            .expect("get_root_resource content should read");
        assert_eq!(actual, expected, "get_root_resource bytes for {path:?}");
    }

    pub(super) fn test_get_root_resource_missing<P: PackMetadataResources + ?Sized>(
        pack: &P,
        path: &[&str],
    ) {
        assert!(
            pack.get_root_resource(path)
                .expect("get_root_resource should succeed")
                .is_none(),
            "get_root_resource should return None for {path:?}"
        );
    }

    pub(super) fn test_metadata_section_json<P: PackMetadataResources + ?Sized>(
        pack: &P,
        name: &str,
        expected: Option<Value>,
    ) {
        let actual = pack
            .metadata_section_json(name)
            .expect("metadata_section_json should succeed");
        assert_eq!(
            actual, expected,
            "metadata_section_json result for {name:?}"
        );
    }

    pub(super) fn test_close<P: PackMetadataResources + ?Sized>(pack: &P) {
        pack.close().expect("close should succeed");
    }
}

#[cfg(test)]
mod path_validation {
    use std::io::ErrorKind;

    use super::{PackMetadataResources, PackResources, PackType, tests};
    use crate::resources::resource::ResourceId;

    // Fixtures own trusted roots/ancestors and keep the filesystem unchanged
    // through supplier reads. These check lexical rejection, not a sandbox.
    pub(super) fn assert_get_resource_invalid_input<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        controls: &[(&ResourceId, &[u8])],
        invalid_ids: &[ResourceId],
    ) {
        assert!(
            !controls.is_empty(),
            "get_resource needs a successful control"
        );
        assert!(!invalid_ids.is_empty(), "get_resource needs invalid IDs");
        for (id, expected) in controls {
            tests::test_get_resource(pack, pack_type, id, expected);
        }
        for id in invalid_ids {
            match pack.get_resource(pack_type, id) {
                Err(error) => assert_eq!(
                    error.kind(),
                    ErrorKind::InvalidInput,
                    "get_resource error for {pack_type:?}, {id:?}"
                ),
                Ok(_) => panic!(
                    "get_resource must reject {pack_type:?}, {id:?} before returning a supplier or absence"
                ),
            }
        }
    }

    pub(super) fn assert_get_root_resource_invalid_input<P: PackMetadataResources + ?Sized>(
        pack: &P,
        controls: &[(&[&str], &[u8])],
        invalid_paths: &[&[&str]],
    ) {
        assert!(
            !controls.is_empty(),
            "get_root_resource needs a successful control"
        );
        assert!(
            !invalid_paths.is_empty(),
            "get_root_resource needs invalid paths"
        );
        for (path, expected) in controls {
            tests::test_get_root_resource(pack, path, expected);
        }
        for path in invalid_paths {
            match pack.get_root_resource(path) {
                Err(error) => assert_eq!(
                    error.kind(),
                    ErrorKind::InvalidInput,
                    "get_root_resource error for {path:?}"
                ),
                Ok(_) => panic!(
                    "get_root_resource must reject {path:?} before returning a supplier or absence"
                ),
            }
        }
    }

    pub(super) fn assert_list_resources_invalid_input<P: PackResources + ?Sized>(
        pack: &P,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        expected: &[(ResourceId, Vec<u8>)],
        invalid_queries: &[(&str, &str)],
    ) {
        assert!(
            !expected.is_empty(),
            "list_resources needs a nonempty successful control"
        );
        assert!(
            !invalid_queries.is_empty(),
            "list_resources needs invalid queries"
        );
        tests::test_list_resources(pack, pack_type, namespace, directory, expected);
        for (namespace, directory) in invalid_queries {
            let mut callbacks = 0;
            let result = pack.list_resources(pack_type, namespace, directory, &mut |_, _| {
                callbacks += 1;
            });
            assert_eq!(
                callbacks, 0,
                "list_resources must not emit for {pack_type:?}, {namespace:?}, {directory:?}"
            );
            let error = result.expect_err("list_resources must reject an invalid query");
            assert_eq!(
                error.kind(),
                ErrorKind::InvalidInput,
                "list_resources error for {pack_type:?}, {namespace:?}, {directory:?}"
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod test_pack {
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::sync::Arc;

    use serde_json::Value;

    use super::{IoSupplier, PackMetadataResources, PackResources, PackType, ResourceOutput};
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::{ReadStream, ResourceId};

    /// An in-memory pack: `assets/<namespace>/<path>` keys, an optional root
    /// metadata object, and a switch that makes every lookup fail.
    pub(crate) struct TestPack {
        pub(crate) location: PackLocationInfo,
        files: HashMap<String, &'static str>,
        metadata: Option<Value>,
        broken: bool,
    }

    impl TestPack {
        pub(crate) fn new(id: &str, files: &[(&str, &'static str)]) -> Self {
            Self {
                location: PackLocationInfo::test(id),
                files: files.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
                metadata: None,
                broken: false,
            }
        }

        pub(crate) fn with_metadata(mut self, metadata: Value) -> Self {
            self.metadata = Some(metadata);
            self
        }

        pub(crate) fn broken(mut self) -> Self {
            self.broken = true;
            self
        }

        fn fail<T>(&self) -> io::Result<T> {
            Err(io::Error::other(format!("{} is broken", self.location.id)))
        }
    }

    fn supplier(bytes: &'static str) -> IoSupplier<ReadStream> {
        Arc::new(move || Ok(Box::new(bytes.as_bytes()) as ReadStream))
    }

    impl PackMetadataResources for TestPack {
        fn location(&self) -> &PackLocationInfo {
            &self.location
        }

        fn get_root_resource(&self, _path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
            Ok(None)
        }

        fn metadata_section_json(&self, name: &str) -> io::Result<Option<Value>> {
            if self.broken {
                return self.fail();
            }
            Ok(self.metadata.as_ref().and_then(|m| m.get(name)).cloned())
        }

        fn close(&self) -> io::Result<()> {
            Ok(())
        }
    }

    impl PackResources for TestPack {
        fn get_resource(
            &self,
            _pack_type: PackType,
            id: &ResourceId,
        ) -> io::Result<Option<IoSupplier<ReadStream>>> {
            if self.broken {
                return self.fail();
            }
            Ok(self
                .files
                .get(&format!("{}/{}", id.namespace(), id.path()))
                .map(|bytes| supplier(bytes)))
        }

        fn list_resources(
            &self,
            _pack_type: PackType,
            namespace: &str,
            directory: &str,
            output: &mut ResourceOutput<'_>,
        ) -> io::Result<()> {
            if self.broken {
                return self.fail();
            }
            let prefix = format!("{namespace}/{directory}/");
            for (key, bytes) in &self.files {
                if let Some(path) = key.strip_prefix(&format!("{namespace}/"))
                    && key.starts_with(&prefix)
                {
                    output(
                        ResourceId::new(&format!("{namespace}:{path}")).unwrap(),
                        supplier(bytes),
                    );
                }
            }
            Ok(())
        }

        fn get_namespaces(&self, _pack_type: PackType) -> io::Result<HashSet<String>> {
            Ok(self
                .files
                .keys()
                .filter_map(|key| key.split_once('/').map(|(ns, _)| ns.to_owned()))
                .collect())
        }
    }
}
