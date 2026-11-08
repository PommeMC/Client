//! Helpers shared by unit tests.

use std::path::PathBuf;
use std::rc::Rc;

use crate::chat_component::Component;
use crate::resources::ResourceOrigin;
use crate::resources::managers::MultiPackResourceManager;
use crate::resources::pack_resources::{PackResources, PackType, PathPackResources};
use crate::resources::packs::PackLocationInfo;

/// A fresh directory name under the system temp dir; the test creates and
/// removes it.
pub fn test_temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pomme_{label}_{}_{}", std::process::id(), nonce))
}

/// Opens fixture pack roots in low-to-high priority order.
pub fn test_resources(roots: &[PathBuf]) -> MultiPackResourceManager {
    let entries = roots
        .iter()
        .enumerate()
        .map(|(index, root)| {
            let location = PackLocationInfo {
                id: format!("fixture-{index}"),
                title: Component::text("Resource fixture"),
                known_pack_info: None,
            };
            let origin = ResourceOrigin::test(&location.id);
            let backend: Rc<dyn PackResources> =
                Rc::new(PathPackResources::new(location, root.clone()));
            (origin, backend)
        })
        .collect();
    MultiPackResourceManager::new(PackType::ClientResources, entries)
}
