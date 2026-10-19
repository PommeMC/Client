//! Where accounts and refresh tokens are kept: string values by key.

use std::collections::HashMap;
use std::path::PathBuf;

pub trait SecretStore: Send + Sync {
    fn read(&self, key: &str) -> Option<String>;
    fn write(&self, key: &str, value: &str);
}

/// The OS credential store, under the launcher's service name so the
/// launcher and the desktop client see the same accounts.
#[cfg(feature = "keyring")]
pub struct KeyringStore;

#[cfg(feature = "keyring")]
const KEYRING_SERVICE: &str = "pomme-launcher";

#[cfg(feature = "keyring")]
impl SecretStore for KeyringStore {
    fn read(&self, key: &str) -> Option<String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .ok()?
            .get_password()
            .ok()
    }

    fn write(&self, key: &str, value: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
            let _ = entry.set_password(value);
        }
    }
}

/// A JSON file of every key, for platforms without an OS credential store
/// (Android, inside the app's private storage).
pub struct FileStore {
    pub path: PathBuf,
}

impl FileStore {
    fn entries(&self) -> HashMap<String, String> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

impl SecretStore for FileStore {
    fn read(&self, key: &str) -> Option<String> {
        self.entries().remove(key)
    }

    fn write(&self, key: &str, value: &str) {
        let mut entries = self.entries();
        entries.insert(key.to_owned(), value.to_owned());
        if let Ok(json) = serde_json::to_vec(&entries) {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // Through a temp file, so a crash mid-write can't lose every
            // account.
            let mut tmp = self.path.as_os_str().to_owned();
            tmp.push(".part");
            let tmp = PathBuf::from(tmp);
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileStore, SecretStore};

    #[test]
    fn file_store_keeps_each_key() {
        let dir = std::env::temp_dir().join(format!("pomme-auth-{}", std::process::id()));
        let store = FileStore {
            path: dir.join("accounts.json"),
        };
        assert_eq!(store.read("a"), None);
        store.write("a", "1");
        store.write("b", "2");
        store.write("a", "3");
        assert_eq!(store.read("a").as_deref(), Some("3"));
        assert_eq!(store.read("b").as_deref(), Some("2"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
