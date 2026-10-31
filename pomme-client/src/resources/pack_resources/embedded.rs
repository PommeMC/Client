//! Pomme's static client assets, separate from Minecraft's built-in pack.

use std::collections::HashSet;
use std::io::{self, Cursor};
use std::rc::Rc;

use serde_json::Value;

use super::path::{validate_namespace, validate_path, validate_segment};
use super::{IoSupplier, PackMetadataResources, PackResources, PackType, ResourceOutput};
use crate::resources::packs::{PackLocationInfo, PackMetadata, ResourcesSupplier};
use crate::resources::resource::{ReadStream, ResourceId};
use crate::resources::resource_id;

// Stable application IDs, independent of the installed Minecraft version.
// Selected packs can override them through the ordinary resource lookup.
pub(crate) const POMME_UI_FONT: ResourceId = resource_id!("pomme:font/ui.ttf");
pub(crate) const POMME_ICONS_FONT: ResourceId = resource_id!("pomme:font/icons.ttf");
pub(crate) const POMME_ICON: ResourceId = resource_id!("pomme:textures/icon.png");

const ENTRIES: &[(ResourceId, &[u8])] = &[
    (
        POMME_UI_FONT,
        include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf"),
    ),
    (
        POMME_ICONS_FONT,
        include_bytes!("../../renderer/fonts/fa-solid-900.ttf"),
    ),
    (POMME_ICON, include_bytes!("../../../assets/icon.png")),
];

fn bytes(id: &ResourceId) -> Option<&'static [u8]> {
    ENTRIES
        .iter()
        .find_map(|(name, bytes)| (name == id).then_some(*bytes))
}

pub(crate) struct EmbeddedPackResources {
    location: PackLocationInfo,
}

/// Static storage needs no setup. The source supplies logical PackMetadata
/// directly; opening metadata does not synthesize a pack.mcmeta root file.
pub(crate) struct EmbeddedResourcesSupplier;

impl EmbeddedResourcesSupplier {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl ResourcesSupplier for EmbeddedResourcesSupplier {
    fn open_metadata(
        &self,
        location: &PackLocationInfo,
    ) -> io::Result<Rc<dyn PackMetadataResources>> {
        Ok(Rc::new(EmbeddedPackResources::new(location.clone())))
    }

    fn open_resources(
        &self,
        location: &PackLocationInfo,
        _metadata: &PackMetadata,
    ) -> io::Result<Vec<Rc<dyn PackResources>>> {
        Ok(vec![Rc::new(EmbeddedPackResources::new(location.clone()))])
    }
}

impl EmbeddedPackResources {
    pub(crate) fn new(location: PackLocationInfo) -> Self {
        Self { location }
    }
}

impl PackMetadataResources for EmbeddedPackResources {
    fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    fn get_root_resource(&self, path: &[&str]) -> io::Result<Option<IoSupplier<ReadStream>>> {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty root resource path",
            ));
        }
        for segment in path {
            validate_segment(segment)?;
        }
        // The static table represents assets/<namespace>/<path>, and no
        // other root files. In particular it contains no pack.mcmeta.
        let ["assets", namespace, resource_path @ ..] = path else {
            return Ok(None);
        };
        let resource_path = resource_path.join("/");
        Ok(ENTRIES.iter().find_map(|(id, bytes)| {
            (id.namespace() == *namespace && id.path() == resource_path)
                .then(|| static_supplier(bytes))
        }))
    }

    fn metadata_section_json(&self, _name: &str) -> io::Result<Option<Value>> {
        // No fabricated metadata envelope or application policy.
        Ok(None)
    }

    /// Static bytes own no handles. Close leaves suppliers/streams usable.
    fn close(&self) -> io::Result<()> {
        Ok(())
    }
}

impl PackResources for EmbeddedPackResources {
    fn get_resource(
        &self,
        pack_type: PackType,
        id: &ResourceId,
    ) -> io::Result<Option<IoSupplier<ReadStream>>> {
        validate_namespace(id.namespace())?;
        validate_path(id.path())?;
        if pack_type != PackType::ClientResources {
            return Ok(None);
        }
        Ok(bytes(id).map(static_supplier))
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        directory: &str,
        output: &mut ResourceOutput<'_>,
    ) -> io::Result<()> {
        validate_namespace(namespace)?;
        if !directory.is_empty() {
            validate_path(directory)?;
        }
        if pack_type == PackType::ClientResources {
            for (id, bytes) in ENTRIES {
                if id.namespace() == namespace
                    && (directory.is_empty()
                        || id.path() == directory
                        || id
                            .path()
                            .strip_prefix(directory)
                            .is_some_and(|tail| tail.starts_with('/')))
                {
                    output(id.clone(), static_supplier(bytes));
                }
            }
        }
        Ok(())
    }

    fn get_namespaces(&self, pack_type: PackType) -> io::Result<HashSet<String>> {
        Ok(if pack_type == PackType::ClientResources {
            ENTRIES
                .iter()
                .map(|(id, _)| id.namespace().to_owned())
                .collect()
        } else {
            HashSet::new()
        })
    }
}

fn static_supplier(bytes: &'static [u8]) -> IoSupplier<ReadStream> {
    Rc::new(move || Ok(Box::new(Cursor::new(bytes)) as ReadStream))
}
