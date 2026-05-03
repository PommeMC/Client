use crate::installations::{Id, Installation, InstallationDraft, InstallationError, fs};

pub fn load() -> Result<Vec<Installation>, InstallationError> {
    let contents = std::fs::read_to_string(fs::registry_file())?;
    let list: Vec<Installation> = serde_json::from_str(&contents)?;

    Ok(list)
}

pub fn save(list: &[Installation]) -> Result<(), InstallationError> {
    let contents = serde_json::to_string_pretty(list)?;
    std::fs::write(fs::registry_file(), contents)?;
    Ok(())
}

pub fn find_by_id(id: &Id) -> Result<Installation, InstallationError> {
    let list = load()?;

    list.into_iter()
        .find(|i| i.id == *id)
        .ok_or(InstallationError::InstallNotFound(id.clone()))
}

pub fn register(install: Installation) -> Result<(), InstallationError> {
    let mut list = load()?;

    if list.iter().any(|i| i.directory == install.directory) {
        return Err(InstallationError::DirectoryAlreadyExists);
    }

    list.push(install);
    save(&list)?;
    Ok(())
}

pub fn unregister(install_id: &Id) -> Result<(), InstallationError> {
    let mut list = load()?;
    list.retain(|i| i.id != *install_id);
    save(&list)?;
    Ok(())
}

pub fn update(install_id: &Id, data: InstallationDraft) -> Result<Installation, InstallationError> {
    let mut list = load()?;

    let install = list
        .iter_mut()
        .find(|i| i.id == *install_id)
        .ok_or(InstallationError::InstallNotFound(install_id.clone()))?;

    install.name = data.name.try_into()?;
    install.version = data.version.into();
    install.directory = data.directory.try_into()?;
    install.width = data.width;
    install.height = data.height;

    let updated = install.clone();
    save(&list)?;
    Ok(updated)
}
