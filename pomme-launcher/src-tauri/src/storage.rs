//! The launcher's paths in `pomme_dirs`' layout, which the client shares.

use std::path::{Path, PathBuf};

use pomme_dirs::layout;

fn ensure_file(path: &Path, default: &str) {
    if !path.exists() {
        let _ = std::fs::write(path, default);
    }
}

pub fn ensure_dirs() {
    if let Err(e) = pomme_dirs::migrate() {
        log::warn!("Couldn't move the old .pomme folder: {e}");
    }

    let _ = std::fs::create_dir_all(pomme_assets_dir());
    let _ = std::fs::create_dir_all(clients_dir());
    let _ = std::fs::create_dir_all(profiles_dir());
    let _ = std::fs::create_dir_all(indexes_dir());
    let _ = std::fs::create_dir_all(objects_dir());
    let _ = std::fs::create_dir_all(versions_dir());

    ensure_file(&settings_file(), "{}");
}

/// `minecraft/assets/`
pub fn assets_dir() -> PathBuf {
    layout().minecraft_assets()
}
/// `minecraft/assets/indexes/`
pub fn indexes_dir() -> PathBuf {
    assets_dir().join("indexes")
}
/// `minecraft/assets/objects/`
pub fn objects_dir() -> PathBuf {
    assets_dir().join("objects")
}

/// `pomme-assets/`
pub fn pomme_assets_dir() -> PathBuf {
    layout().pomme_assets()
}

/// `minecraft/versions/`
pub fn versions_dir() -> PathBuf {
    layout().minecraft_versions()
}
/// `minecraft/versions/{version}/`
pub fn version_dir(version: &str) -> PathBuf {
    versions_dir().join(version)
}
/// `minecraft/versions/{version}/{version}.jar`
pub fn version_jar(version: &str) -> PathBuf {
    version_dir(version).join(format!("{version}.jar"))
}
/// `minecraft/versions/{version}/extracted/`
pub fn version_extracted_dir(version: &str) -> PathBuf {
    version_dir(version).join("extracted")
}
/// `minecraft/versions/{version}/extracted/.extracted`
pub fn version_extracted_marker(version: &str) -> PathBuf {
    version_extracted_dir(version).join(".extracted")
}

/// `launcher/clients/`
pub fn clients_dir() -> PathBuf {
    layout().launcher().join("clients")
}
/// `launcher/clients/{tag}/`
pub fn client_version_dir(tag: &str) -> PathBuf {
    clients_dir().join(tag)
}
/// `launcher/clients/{tag}/pomme-client[.exe]`
pub fn client_binary(tag: &str) -> PathBuf {
    #[cfg(target_family = "windows")]
    let name = "pomme-client.exe";
    #[cfg(not(target_family = "windows"))]
    let name = "pomme-client";
    client_version_dir(tag).join(name)
}
/// `launcher/clients/{tag}/.verified`
pub fn client_marker(tag: &str) -> PathBuf {
    client_version_dir(tag).join(".verified")
}

/// `profiles/`
pub fn profiles_dir() -> PathBuf {
    layout().profiles()
}
/// `profiles/default/`
pub fn default_profile() -> PathBuf {
    layout().default_profile()
}

/// `launcher/settings.json`
pub fn settings_file() -> PathBuf {
    layout().launcher().join("settings.json")
}
/// `launcher/installations.json`
pub fn installations_file() -> PathBuf {
    layout().launcher().join("installations.json")
}
