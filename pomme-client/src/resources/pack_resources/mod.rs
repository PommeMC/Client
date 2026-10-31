//! Storage contracts used by resource managers and repository suppliers.
//! Maps Vanilla 26.3 PackResources, PackMetadataResources, PackType,
//! MetadataSectionType and IoSupplier into Rust.
//! Fallible lookup uses Result<Option<_>>; typed metadata lives on an extension
//! trait to keep the base traits object-safe. Suppliers own their captured
//! state.

use std::collections::HashSet;
use std::io;
use std::rc::Rc;

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

/// Owned, repeatable, application-thread supplier; invocation may fail.
/// Captures must retain any needed storage leases rather than borrow a pack or
/// manager. Owned streams are memory-safe beyond those objects' lifetimes, but
/// continued usability after explicit pack closure is a backend policy.
pub(crate) type IoSupplier<T> = Rc<dyn Fn() -> io::Result<T> + 'static>;

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
