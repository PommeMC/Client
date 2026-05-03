use crate::storage;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tokio::sync::{RwLock, RwLockReadGuard};

static LAUNCHER_SETTINGS: LazyLock<RwLock<LauncherSettings>> =
    LazyLock::new(|| RwLock::new(LauncherSettings::load()));

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSettings {
    pub language: String,
    pub keep_launcher_open: bool,
    pub launch_with_console: bool,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        LauncherSettings {
            language: "English".into(),
            keep_launcher_open: true,
            launch_with_console: false,
        }
    }
}

impl LauncherSettings {
    async fn save(&self) -> Result<(), String> {
        let path = storage::settings_file();
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        tokio::fs::write(path, json)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn load() -> Self {
        let path = storage::settings_file();

        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<LauncherSettings>(&content) {
                Ok(cfg) => return cfg,
                Err(err) => {
                    log::warn!("Settings file invalid ({}), using defaults", err);
                }
            },
            Err(_) => {
                log::info!("Settings file not found, creating default settings");
            }
        }

        let default = LauncherSettings::default();
        if let Ok(json) = serde_json::to_string_pretty(&default) {
            let _ = std::fs::write(&path, json);
        }
        default
    }

    pub async fn get() -> RwLockReadGuard<'static, LauncherSettings> {
        LAUNCHER_SETTINGS.read().await
    }

    pub async fn update<F>(f: F) -> Result<(), String>
    where
        F: FnOnce(&mut LauncherSettings),
    {
        let cloned = {
            let mut settings = LAUNCHER_SETTINGS.write().await;
            f(&mut settings);
            settings.clone()
        };

        cloned.save().await
    }
}
