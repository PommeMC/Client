use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{PackId, PackStorage, ResourcePackDescriptor};
use crate::resources::{ResourceError, ResourceId, ResourceOrigin};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ThemeSpecification {
    Java,
    Pomme { root: PathBuf },
    Bedrock { root: PathBuf },
}

pub(super) struct ConfiguredTheme {
    pub(super) specification: ThemeSpecification,
    pub(super) pack_id: PackId,
}

impl ConfiguredTheme {
    pub(super) fn source(&self) -> Result<Option<ResourcePackDescriptor>, ResourceError> {
        let (root, name, directory) = match &self.specification {
            ThemeSpecification::Java => return Ok(None),
            ThemeSpecification::Pomme { root } => (root, "Pomme theme assets", "panoramas"),
            ThemeSpecification::Bedrock { root } => (root, "Bedrock theme assets", "textures/ui"),
        };
        let origin = ResourceOrigin {
            pack_id: self.pack_id.clone(),
            name: name.to_owned(),
        };
        match std::fs::read_dir(root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ResourceError::Io {
                    operation: "open source directory",
                    id: None,
                    origin,
                    source,
                });
            }
        }
        let remappings = (0..6)
            .map(|face| {
                Ok((
                    panorama_id(face)?,
                    PathBuf::from(directory).join(format!("panorama_{face}.png")),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ResourceError>>()?;
        Ok(Some(ResourcePackDescriptor {
            storage: vec![PackStorage::NamespaceDirectory {
                root: root.clone(),
                namespace: "minecraft".to_owned(),
                remappings: Some(Arc::new(remappings)),
            }],
            origin,
            filter: Default::default(),
        }))
    }
}

pub(crate) fn panorama_id(face: u32) -> Result<ResourceId, ResourceError> {
    ResourceId::new(&format!(
        "minecraft:textures/gui/title/background/panorama_{face}.png"
    ))
}
