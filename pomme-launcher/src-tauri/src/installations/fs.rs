use std::path::{Path, PathBuf};

use crate::installations::{Directory, Installation, InstallationError};
use crate::storage::installations_file;

pub fn registry_file() -> PathBuf {
    let path = installations_file();
    if !path.exists() {
        std::fs::write(&path, "[]").ok();
    }
    path
}

pub fn ensure_install_fs(install: &Installation) -> Result<(), InstallationError> {
    let dir_path: &Path = install.directory.as_ref();

    pomme_dirs::create_profile(dir_path)?;

    let servers_path = dir_path.join("servers.json");
    if !servers_path.exists() {
        std::fs::write(servers_path, "[]")?;
    }

    let options_path = dir_path.join("options.json");
    if !options_path.exists() {
        std::fs::write(options_path, "{}")?;
    }

    Ok(())
}

pub fn remove_install_fs(dir: &Directory) -> Result<(), InstallationError> {
    let dir_path: &Path = dir.as_ref();

    std::fs::remove_dir_all(dir_path)?;

    Ok(())
}

pub fn duplicate_install_fs(src: &Directory, dst: &Directory) -> Result<(), InstallationError> {
    let src_path: &Path = src.as_ref();
    let dst_path: &Path = dst.as_ref();
    Ok(pomme_dirs::copy_dir(src_path, dst_path)?)
}

pub fn move_install_fs(src: &Directory, dst: &Directory) -> Result<(), InstallationError> {
    let src_path: &Path = src.as_ref();
    let dst_path: &Path = dst.as_ref();
    Ok(pomme_dirs::move_path(src_path, dst_path)?)
}
