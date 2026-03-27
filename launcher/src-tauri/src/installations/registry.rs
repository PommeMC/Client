use crate::installations::{
    Directory, Id, Installation, InstallationError, Name, TimeStamp, Version,
};
use crate::storage::data_dir;

use fslock::LockFile;

async fn defaults() -> Result<Vec<Installation>, InstallationError> {
    Ok(vec![
        Installation {
            id: Id::latest_release(),
            name: Name::latest_release(),
            version: Version::try_latest_release().await?,
            last_played: None,
            created_at: TimeStamp::now(),
            directory: Directory::latest(),
            width: 854,
            height: 480,
            is_latest: true,
        },
        Installation {
            id: Id::latest_snapshot(),
            name: Name::latest_snapshot(),
            version: Version::try_latest_snapshot().await?,
            last_played: None,
            created_at: TimeStamp::now(),
            directory: Directory::latest(),
            width: 854,
            height: 480,
            is_latest: true,
        },
    ])
}

fn lock() -> Result<LockFile, InstallationError> {
    let path = data_dir().join("installations.lock");
    let mut lock = LockFile::open(&path)?;
    lock.lock()?;
    Ok(lock)
}

fn load() -> Result<Vec<Installation>, InstallationError> {
    let path = data_dir().join("installations.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn save(list: &[Installation]) -> Result<(), InstallationError> {
    let json = serde_json::to_string_pretty(list)?;
    std::fs::write(data_dir().join("installations.json"), json)?;
    Ok(())
}

pub async fn get_all() -> Result<Vec<Installation>, InstallationError> {
    let _lock = lock()?;
    let mut list = load()?;

    let mut dirty = false;
    for (i, default) in defaults().await?.into_iter().enumerate() {
        if !list.iter().any(|i| i.id == default.id) {
            list.insert(i, default);
            dirty = true;
        }
    }

    if dirty {
        save(&list)?;
    }

    Ok(list)
}

pub fn register(installation: Installation) -> Result<(), InstallationError> {
    let _lock = lock()?;
    let mut list = load()?;

    if !installation.is_latest && list.iter().any(|i| i.directory == installation.directory) {
        return Err(InstallationError::DirectoryAlreadyExists);
    }

    list.push(installation);
    save(&list)
}

pub fn unregister(id: &Id) -> Result<(), InstallationError> {
    let _lock = lock()?;
    let mut list = load()?;

    list.retain(|i| i.id != *id);

    save(&list)
}

pub fn get(id: &Id) -> Result<Option<Installation>, InstallationError> {
    let _lock = lock()?;
    let list = load()?;

    Ok(list.into_iter().find(|i| i.id == *id))
}
