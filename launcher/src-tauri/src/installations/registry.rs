use crate::installations::{Installation, InstallationError};
use crate::storage::data_dir;

pub fn load() -> Result<Vec<Installation>, InstallationError> {
    let path = data_dir().join("installations.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn save(list: &[Installation]) -> Result<(), InstallationError> {
    let json = serde_json::to_string_pretty(list)?;
    std::fs::write(data_dir().join("installations.json"), json)?;
    Ok(())
}

pub fn register(installation: Installation) -> Result<(), InstallationError> {
    let mut list = load()?;

    if list.iter().any(|i| i.directory == installation.directory) {
        return Err(InstallationError::DirectoryAlreadyExists);
    }

    list.push(installation);
    save(&list)
}

pub fn unregister(id: &str) -> Result<(), InstallationError> {
    let mut list = load()?;

    list.retain(|i| i.id != id);

    save(&list)
}

pub fn get_install(id: &str) -> Result<Option<Installation>, InstallationError> {
    let list = load()?;

    Ok(list.into_iter().find(|i| i.id == id))
}
