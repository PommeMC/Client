//! Helpers shared by unit tests.

use std::path::PathBuf;
use std::rc::Rc;

use crate::chat_component::Component;
use crate::resources::managers::MultiPackResourceManager;
use crate::resources::pack_resources::{
    PackResources, PackType, PathPackResources, PathResourcesSupplier,
};
use crate::resources::packs::{
    FeatureFlagSet, Pack, PackCompat, PackEnablementConfig, PackLocationInfo, PackMetadata,
    PackPosition, PackSource,
};

/// Opens fixture pack roots in low-to-high priority order.
pub fn test_resources(roots: &[PathBuf]) -> MultiPackResourceManager {
    let entries = roots
        .iter()
        .enumerate()
        .map(|(index, root)| {
            let location = PackLocationInfo {
                id: format!("fixture-{index}"),
                title: Component::text("Resource fixture"),
                source: PackSource::built_in(),
                known_pack_info: None,
            };
            let pack = Pack::new(
                location.clone(),
                PackMetadata {
                    description: Component::text("Resource fixture"),
                    compatibility: PackCompat::Compatible,
                    requested_features: FeatureFlagSet::empty(),
                    overlays: Vec::new(),
                },
                PackEnablementConfig {
                    required: false,
                    default_position: PackPosition::Top,
                    fixed_position: false,
                },
                Rc::new(PathResourcesSupplier::new(root.clone())),
            );
            let backend: Rc<dyn PackResources> =
                Rc::new(PathPackResources::new(location, root.clone()));
            (pack.origin(), backend)
        })
        .collect();
    MultiPackResourceManager::new(PackType::ClientResources, entries).unwrap()
}

/// A fresh directory name under the system temp dir; the test creates and
/// removes it.
pub fn test_temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pomme_{label}_{}_{}", std::process::id(), nonce))
}
