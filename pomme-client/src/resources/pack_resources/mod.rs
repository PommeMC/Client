//! Storage contracts used by resource managers and repository suppliers.
//! Maps Vanilla 26.3 PackResources, PackMetadataResources, PackType,
//! MetadataSectionType and IoSupplier into Rust.
//! Fallible lookup uses Result<Option<_>>; typed metadata lives on an extension
//! trait to keep the base traits object-safe. Suppliers own their captured
//! state.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use pomme_protocol::KnownPack;
use serde_json::Value;

use crate::resources::packs::PackLocationInfo;
use crate::resources::resource::{ReadStream, ResourceId};

mod embedded;
mod file;
mod mapped;
mod overlayed;
mod path;
mod vanilla;

// Backends are exposed for subsequent application consumer adoption.
#[allow(unused_imports)]
pub(crate) use embedded::{EmbeddedPackResources, EmbeddedResourcesSupplier};
#[allow(
    unused_imports,
    reason = "Stable embedded resource IDs, pending asset-consumer adoption"
)]
pub(crate) use embedded::{POMME_ICON, POMME_ICONS_FONT, POMME_UI_FONT};
#[allow(unused_imports)]
pub(crate) use file::{FilePackResources, FileResourcesSupplier};
#[allow(unused_imports)]
pub(crate) use mapped::{MappedPackResources, MappedResourcesSupplier};
#[allow(unused_imports)]
pub(crate) use overlayed::OverlayedPackResources;
#[allow(unused_imports)]
pub(crate) use path::{PathPackResources, PathResourcesSupplier};
#[allow(unused_imports)]
pub(crate) use vanilla::{VanillaPackResources, VanillaResourcesSupplier};

pub(crate) const METADATA_EXTENSION: &str = ".mcmeta";
pub(crate) const PACK_META: &str = "pack.mcmeta";

/// Owned, repeatable supplier shared across threads; invocation may fail.
/// Captures must retain any needed storage leases rather than borrow a pack or
/// manager. Owned streams are memory-safe beyond those objects' lifetimes, but
/// continued usability after explicit pack closure is a backend policy.
pub(crate) type IoSupplier<T> = Arc<dyn Fn() -> io::Result<T> + Send + Sync + 'static>;

/// Enumeration transfers identifiers and unopened suppliers to the caller.
pub(crate) type ResourceOutput<'a> = dyn FnMut(ResourceId, IoSupplier<ReadStream>) + 'a;

/// Returns true when the identifier is blocked.
pub(crate) type Filter<'a> = dyn Fn(&ResourceId) -> bool + 'a;

/// Domain shape only; this does not add server-data support to the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PackType {
    ClientResources,
    ServerData,
}

impl PackType {
    pub(crate) const fn directory(self) -> &'static str {
        match self {
            Self::ClientResources => "assets",
            Self::ServerData => "data",
        }
    }
}

/// Named section with both directions of Vanilla's Codec, expressed as JSON
/// function pointers rather than a new codec framework. Codec errors propagate.
pub(crate) struct MetadataSectionType<T> {
    pub(crate) name: String,
    pub(crate) decode: fn(&Value) -> io::Result<T>,
    pub(crate) encode: fn(&T) -> io::Result<Value>,
}

/// Object-safe pack identity, root resources, metadata and explicit closure.
/// I/O failures are errors, never resource or section absence.
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

/// Typed metadata lookup also available on unsized base-trait objects.
pub(crate) trait PackMetadataResourcesExt: PackMetadataResources {
    fn get_metadata_section<T>(&self, section: &MetadataSectionType<T>) -> io::Result<Option<T>> {
        self.metadata_section_json(&section.name)?
            .map(|value| (section.decode)(&value))
            .transpose()
    }
}

impl<P: PackMetadataResources + ?Sized> PackMetadataResourcesExt for P {}

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

    fn known_pack_info(&self) -> Option<&KnownPack> {
        self.location().known_pack_info.as_ref()
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
mod tests {
    #![allow(
        dead_code,
        reason = "Generic API assertions pending user review before fixtures"
    )]

    use std::collections::HashSet;
    use std::fmt::Debug;
    use std::io::{self, Read};

    use pomme_protocol::KnownPack;
    use serde_json::Value;

    use super::{
        MetadataSectionType, PackMetadataResources, PackMetadataResourcesExt, PackResources,
        PackType,
    };
    use crate::chat_component::Component;
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

    pub(super) fn test_known_pack_info<P: PackResources + ?Sized>(
        pack: &P,
        expected: Option<&KnownPack>,
    ) {
        assert_eq!(pack.known_pack_info(), expected, "known_pack_info result");
    }

    pub(super) fn test_location<P: PackMetadataResources + ?Sized>(
        pack: &P,
        expected: &PackLocationInfo,
    ) {
        let actual = pack.location();
        assert_eq!(actual.id, expected.id, "location ID");
        assert_eq!(actual.title, expected.title, "location title");
        assert_eq!(
            actual.source.should_add_automatically(),
            expected.source.should_add_automatically(),
            "location source automatic selection"
        );
        let probe = Component::text("location source probe");
        assert_eq!(
            actual.source.decorate(probe.clone()),
            expected.source.decorate(probe),
            "location source decoration"
        );
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

    pub(super) fn test_get_metadata_section<
        P: PackMetadataResourcesExt + ?Sized,
        T: PartialEq + Debug,
    >(
        pack: &P,
        section: &MetadataSectionType<T>,
        expected: Option<T>,
    ) {
        let actual = pack
            .get_metadata_section(section)
            .expect("get_metadata_section should succeed");
        assert_eq!(
            actual, expected,
            "get_metadata_section result for {:?}",
            section.name
        );
    }

    pub(super) fn test_get_metadata_section_missing<P: PackMetadataResourcesExt + ?Sized>(
        pack: &P,
        name: &str,
    ) {
        let section = MetadataSectionType::<()> {
            name: name.to_owned(),
            decode: |_| panic!("missing metadata section must not invoke the decoder"),
            encode: |_| Ok(Value::Null),
        };
        assert!(
            pack.get_metadata_section(&section)
                .expect("get_metadata_section should succeed for a missing section")
                .is_none(),
            "get_metadata_section should return None for {name:?}"
        );
    }

    pub(super) fn test_get_metadata_section_null<P: PackMetadataResourcesExt + ?Sized>(
        pack: &P,
        name: &str,
    ) {
        let section = MetadataSectionType {
            name: name.to_owned(),
            decode: |value| {
                assert_eq!(value, &Value::Null, "decoder should receive JSON null");
                Ok("decoded null")
            },
            encode: |_| Ok(Value::Null),
        };
        test_get_metadata_section(pack, &section, Some("decoded null"));
    }

    pub(super) fn test_get_metadata_section_decode_error<
        P: PackMetadataResourcesExt + ?Sized,
        T,
    >(
        pack: &P,
        section: &MetadataSectionType<T>,
        expected_kind: io::ErrorKind,
    ) {
        match pack.get_metadata_section(section) {
            Err(error) => assert_eq!(error.kind(), expected_kind, "metadata decoder error kind"),
            Ok(_) => panic!("get_metadata_section should propagate the decoder error"),
        }
    }

    pub(super) fn test_close<P: PackMetadataResources + ?Sized>(pack: &P) {
        pack.close().expect("close should succeed");
    }
}
