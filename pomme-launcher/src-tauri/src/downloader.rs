//! The launcher's side of `pomme-install`: installs into the launcher's data
//! dir and reports progress to the UI.

use tauri::AppHandle;
use tauri_specta::Event;

use crate::storage;

#[derive(serde::Serialize, Clone, specta::Type, tauri_specta::Event)]
pub struct DownloadProgressEvent {
    pub downloaded: u32,
    pub total: u32,
    pub status: String,
}

pub fn needs_download(version: &str) -> bool {
    !pomme_install::is_installed(&storage::assets_dir(), &storage::versions_dir(), version)
}

pub async fn download(app: &AppHandle, version: &str) -> Result<(), String> {
    pomme_install::install(
        &reqwest::Client::new(),
        &storage::assets_dir(),
        &storage::versions_dir(),
        version,
        &|progress| {
            let _ = DownloadProgressEvent {
                downloaded: progress.done,
                total: progress.total,
                status: progress.status,
            }
            .emit(app);
        },
    )
    .await
}

pub async fn get_downloaded_versions() -> Vec<String> {
    let mut matches = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&storage::versions_dir()).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir()
                && let Some(id) = path.file_name().and_then(|n| n.to_str())
                && !needs_download(id)
            {
                matches.push(id.to_string());
            }
        }
    }
    matches
}
