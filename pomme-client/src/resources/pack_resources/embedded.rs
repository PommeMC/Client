//! Pomme's static client assets, separate from Minecraft's built-in pack.

use std::collections::HashSet;
use std::io::{self, Cursor};
use std::rc::Rc;
use std::sync::Arc;

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
pub(crate) const POMME_EDITION: ResourceId = resource_id!("pomme:gui/title/edition.png");

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
    (
        POMME_EDITION,
        include_bytes!("../../../assets/gui/title/edition.png"),
    ),
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
    Arc::new(move || Ok(Box::new(Cursor::new(bytes)) as ReadStream))
}

#[cfg(test)]
mod test_fixture {
    use super::EmbeddedPackResources;
    use crate::chat_component::Component;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    pub(super) fn default_location() -> PackLocationInfo {
        PackLocationInfo {
            id: "embedded-api".into(),
            title: Component::text("Embedded API fixture"),
            source: PackSource::built_in(),
            known_pack_info: None,
        }
    }

    pub(super) fn fixture(location: PackLocationInfo) -> EmbeddedPackResources {
        EmbeddedPackResources::new(location)
    }
}

#[cfg(test)]
mod pack_metadata_resources_tests {
    use pomme_protocol::KnownPack;

    use super::test_fixture::{default_location, fixture};
    use crate::chat_component::Component;
    use crate::resources::pack_resources::tests;
    use crate::resources::packs::{PackLocationInfo, PackSource};

    #[test]
    fn test_get_root_resource() {
        let pack = fixture(default_location());
        tests::test_get_root_resource(
            &pack,
            &["assets", "pomme", "textures", "icon.png"],
            include_bytes!("../../../assets/icon.png"),
        );
    }

    #[test]
    fn test_get_root_resource_missing() {
        let pack = fixture(default_location());
        tests::test_get_root_resource_missing(&pack, &["missing.txt"]);
    }

    #[test]
    fn test_metadata_section_missing() {
        let pack = fixture(default_location());
        tests::test_metadata_section_json(&pack, "missing", None);
    }

    #[test]
    fn test_close() {
        let pack = fixture(default_location());
        tests::test_close(&pack);
    }

    #[test]
    fn test_location() {
        let expected = PackLocationInfo {
            id: "embedded-location".into(),
            title: Component::translate("location.embedded.title", Vec::new()),
            source: PackSource::feature(),
            known_pack_info: Some(KnownPack {
                namespace: "location_test".into(),
                id: "embedded_known".into(),
                version: "test-1".into(),
            }),
        };
        let pack = fixture(expected.clone());
        tests::test_location(&pack, &expected);
    }
}

#[cfg(test)]
mod pack_metadata_resources_ext_tests {
    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::tests;

    #[test]
    fn test_get_metadata_section_missing() {
        let pack = fixture(default_location());
        tests::test_get_metadata_section_missing(&pack, "missing");
    }
}

#[cfg(test)]
mod pack_resources_tests {
    use std::collections::HashSet;

    use pomme_protocol::KnownPack;

    use super::test_fixture::{default_location, fixture};
    use crate::resources::pack_resources::{PackType, tests};
    use crate::resources::packs::PackLocationInfo;
    use crate::resources::resource_id;

    #[test]
    fn test_get_resource_client() {
        let pack = fixture(default_location());
        tests::test_get_resource(
            &pack,
            PackType::ClientResources,
            &resource_id!("pomme:textures/icon.png"),
            include_bytes!("../../../assets/icon.png"),
        );
    }

    #[test]
    fn test_get_resource_missing() {
        let pack = fixture(default_location());
        tests::test_get_resource_missing(
            &pack,
            PackType::ClientResources,
            &resource_id!("pomme:textures/missing.png"),
        );
    }

    #[test]
    fn test_get_resource_wrong_domain() {
        let pack = fixture(default_location());
        tests::test_get_resource_missing(
            &pack,
            PackType::ServerData,
            &resource_id!("pomme:textures/icon.png"),
        );
    }

    #[test]
    fn test_list_resources_fonts() {
        let pack = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "pomme",
            "font",
            &[
                (
                    resource_id!("pomme:font/ui.ttf"),
                    include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf").to_vec(),
                ),
                (
                    resource_id!("pomme:font/icons.ttf"),
                    include_bytes!("../../renderer/fonts/fa-solid-900.ttf").to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn test_list_resources_textures() {
        let pack = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "pomme",
            "textures",
            &[(
                resource_id!("pomme:textures/icon.png"),
                include_bytes!("../../../assets/icon.png").to_vec(),
            )],
        );
    }

    #[test]
    fn test_list_resources_empty_directory() {
        let pack = fixture(default_location());
        tests::test_list_resources(
            &pack,
            PackType::ClientResources,
            "pomme",
            "",
            &[
                (
                    resource_id!("pomme:font/ui.ttf"),
                    include_bytes!("../../renderer/fonts/Montserrat-Medium.ttf").to_vec(),
                ),
                (
                    resource_id!("pomme:font/icons.ttf"),
                    include_bytes!("../../renderer/fonts/fa-solid-900.ttf").to_vec(),
                ),
                (
                    resource_id!("pomme:textures/icon.png"),
                    include_bytes!("../../../assets/icon.png").to_vec(),
                ),
                (
                    resource_id!("pomme:gui/title/edition.png"),
                    include_bytes!("../../../assets/gui/title/edition.png").to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn test_list_resources_missing_directory() {
        let pack = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ClientResources, "pomme", "missing", &[]);
    }

    #[test]
    fn test_list_resources_missing_namespace() {
        let pack = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ClientResources, "missing", "font", &[]);
    }

    #[test]
    fn test_list_resources_wrong_domain() {
        let pack = fixture(default_location());
        tests::test_list_resources(&pack, PackType::ServerData, "pomme", "font", &[]);
    }

    #[test]
    fn test_get_namespaces_client() {
        let pack = fixture(default_location());
        tests::test_get_namespaces(
            &pack,
            PackType::ClientResources,
            &HashSet::from(["pomme".into()]),
        );
    }

    #[test]
    fn test_get_namespaces_server() {
        let pack = fixture(default_location());
        tests::test_get_namespaces(&pack, PackType::ServerData, &HashSet::new());
    }

    #[test]
    fn test_pack_id() {
        let pack = fixture(default_location());
        tests::test_pack_id(&pack, "embedded-api");
    }

    #[test]
    fn test_known_pack_info_none() {
        let pack = fixture(default_location());
        tests::test_known_pack_info(&pack, None);
    }

    #[test]
    fn test_known_pack_info_some() {
        let expected = KnownPack {
            namespace: "api_identity".into(),
            id: "embedded_known".into(),
            version: "api-2".into(),
        };
        let location = PackLocationInfo {
            known_pack_info: Some(expected.clone()),
            ..default_location()
        };
        let pack = fixture(location);
        tests::test_known_pack_info(&pack, Some(&expected));
    }
}

#[cfg(test)]
mod resources_supplier_tests {
    use super::EmbeddedResourcesSupplier;
    use crate::chat_component::Component;
    use crate::resources::packs::{
        FeatureFlagSet, PackCompat, PackLocationInfo, PackMetadata, PackSource, ResourcesSupplier,
    };

    fn location() -> PackLocationInfo {
        PackLocationInfo {
            id: "embedded-supplier-api".into(),
            title: Component::text("Supplier API test"),
            source: PackSource::default(),
            known_pack_info: None,
        }
    }

    #[test]
    fn test_open_metadata() {
        let supplier = EmbeddedResourcesSupplier::new();
        let location = location();
        <dyn ResourcesSupplier>::test_open_metadata(&supplier, &location, "embedded-supplier-api");
    }

    #[test]
    fn test_open_resources() {
        let supplier = EmbeddedResourcesSupplier::new();
        let location = location();
        let metadata = PackMetadata {
            description: Component::text("Supplier API test"),
            compatibility: PackCompat::Compatible,
            requested_features: FeatureFlagSet::empty(),
            overlays: Vec::new(),
        };
        <dyn ResourcesSupplier>::test_open_resources(
            &supplier,
            &location,
            &metadata,
            &["embedded-supplier-api"],
        );
    }
}
