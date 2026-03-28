use crate::{
    installations::{Directory, Installation, InstallationError},
    storage::data_dir,
};

use std::path::{Path, PathBuf};

fn copy_dir(src: &Path, dst: &Path) -> Result<(), InstallationError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

pub fn registry_file() -> PathBuf {
    let path = data_dir().join("installations.json");
    if !path.exists() {
        std::fs::write(&path, "[]").ok();
    }
    path
}

pub fn ensure_install_fs(install: &Installation) -> Result<(), InstallationError> {
    let dir_path: &Path = install.directory.as_ref();

    for sub_dir in ["mods", "resourcepacks", "shaderpacks"] {
        std::fs::create_dir_all(dir_path.join(sub_dir))?;
    }

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
    copy_dir(src_path, dst_path)
}

pub fn move_install_fs(src: &Directory, dst: &Directory) -> Result<(), InstallationError> {
    let src_path: &Path = src.as_ref();
    let dst_path: &Path = dst.as_ref();
    copy_dir(src_path, dst_path)?;
    std::fs::remove_dir_all(src_path)?;
    Ok(())
}
