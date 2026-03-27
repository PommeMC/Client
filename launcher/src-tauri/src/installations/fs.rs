use crate::installations::{Directory, Installation, InstallationError};
use crate::storage::installations_dir;
use std::path::Path;

pub fn ensure_dirs(instance_dir: &Path) -> Result<(), InstallationError> {
    for sub in &["mods", "resourcepacks", "shaderpacks"] {
        std::fs::create_dir_all(instance_dir.join(sub))?;
    }

    let servers = instance_dir.join("server.json");
    if !servers.exists() {
        std::fs::write(servers, "[]")?;
    }

    let options = instance_dir.join("options.json");
    if !options.exists() {
        std::fs::write(options, "{}")?;
    }

    Ok(())
}

pub fn create_installation_fs(installation: &Installation) -> Result<(), InstallationError> {
    let instance_dir = installations_dir().join(&installation.directory);
    if instance_dir.exists() {
        return Err(InstallationError::DirectoryAlreadyExists);
    }

    ensure_dirs(&instance_dir)?;

    let install_json = serde_json::to_string_pretty(installation)?;
    std::fs::write(instance_dir.join("installation.json"), install_json)?;

    std::fs::write(
        instance_dir.join("servers.json"),
        serde_json::to_string_pretty(&serde_json::json!([{
          "name": "Test server",
          "address": "mc.kasane.love:29666",
          "resourcePack": "prompt"
        }]))?,
    )?;

    std::fs::write(
        instance_dir.join("options.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "video_settings": {
                "render_distance": 16
            }
        }))?,
    )?;

    Ok(())
}

pub fn remove_installation_fs(installation_dir: &Directory) -> Result<(), InstallationError> {
    let path = installations_dir().join(installation_dir);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
