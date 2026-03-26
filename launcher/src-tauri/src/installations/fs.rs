use crate::installations::{Installation, InstallationError};
use crate::storage::installations_dir;
use std::path::Path;

pub fn write_icon(instance_dir: &Path, icon: Option<&str>) -> Result<(), InstallationError> {
    let dest = instance_dir.join("icon.png");

    match icon {
        Some(data) if data.starts_with("data:image/png;base64,") => {
            use base64::Engine;
            let b64 = &data["data:image/png;base64,".len()..];
            let bytes = &base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| InstallationError::Io(e.to_string()))?;
            std::fs::write(dest, bytes)?;
        }
        Some(path) => {
            std::fs::copy(path, dest)?;
        }
        None => {}
    }

    Ok(())
}

pub fn create_installation_fs(installation: &Installation) -> Result<(), InstallationError> {
    let instance_dir = installations_dir().join(&installation.directory);
    if instance_dir.exists() {
        return Err(InstallationError::DirectoryAlreadyExists);
    }

    for sub in &["mods", "resourcepacks", "shaderpacks"] {
        std::fs::create_dir_all(instance_dir.join(sub))?;
    }

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

    write_icon(&instance_dir, installation.icon.as_deref())?;

    Ok(())
}

pub fn remove_installation_fs(installation_dir: &str) -> Result<(), InstallationError> {
    let path = installations_dir().join(installation_dir);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
