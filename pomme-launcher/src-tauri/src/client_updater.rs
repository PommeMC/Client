//! Downloads the correct per-platform `pomme-client` binary from the project's
//! GitHub releases (vanilla-style), caches it under `.pomme/clients/<tag>/`,
//! verifies its sha256, and returns the path. The launcher prefers a local dev
//! build (`commands::find_client_binary`); this is the production path used
//! when no local build is present.

use std::path::PathBuf;

use pomme_install::client::{self, Release};
use tauri::AppHandle;
use tauri_specta::Event;

use crate::downloader::DownloadProgressEvent;
use crate::storage;

/// Ensure the latest released client is installed and return its binary path.
/// Returns a cached copy if it's already current; on network failure, falls
/// back to the newest cached client so the game still launches offline.
pub async fn ensure_client(app: &AppHandle) -> Result<PathBuf, String> {
    let http = reqwest::Client::new();

    let release = match client::latest_release(&http).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Couldn't reach GitHub releases ({e}); falling back to cached client");
            return newest_cached_client()
                .ok_or_else(|| format!("Couldn't download the client and none is installed: {e}"));
        }
    };

    if let Some(binary) = installed_binary(&release.tag_name) {
        return Ok(binary);
    }

    install(app, &http, &release).await?;
    Ok(storage::client_binary(&release.tag_name))
}

async fn install(app: &AppHandle, http: &reqwest::Client, release: &Release) -> Result<(), String> {
    let tag = &release.tag_name;
    let dir = storage::client_version_dir(tag);

    emit(app, 0, 1, "Downloading client...");
    let bytes = client::download(http, release, client::platform_zip()?, &|_| {}).await?;

    emit(app, 1, 1, "Installing client...");
    client::extract_flat(&bytes, &dir)?;
    let binary = storage::client_binary(tag);
    if !binary.exists() {
        return Err("client binary not found in downloaded archive".to_string());
    }

    std::fs::write(storage::client_marker(tag), tag).map_err(|e| e.to_string())?;
    prune_old_clients(tag);
    Ok(())
}

/// Newest verified client already on disk (by marker mtime), for offline
/// launch.
fn newest_cached_client() -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(storage::clients_dir()).ok()?.flatten() {
        let name = entry.file_name();
        let Some(tag) = name.to_str() else { continue };
        let Some(binary) = installed_binary(tag) else {
            continue;
        };
        if let Ok(mtime) = std::fs::metadata(storage::client_marker(tag)).and_then(|m| m.modified())
            && best.as_ref().is_none_or(|(t, _)| mtime > *t)
        {
            best = Some((mtime, binary));
        }
    }
    best.map(|(_, p)| p)
}

/// Drop every cached client except the one we just installed.
fn prune_old_clients(keep: &str) {
    if let Ok(entries) = std::fs::read_dir(storage::clients_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && entry.file_name().to_str() != Some(keep) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}

/// The cached binary path for `tag` if it's fully installed (binary + marker).
fn installed_binary(tag: &str) -> Option<PathBuf> {
    let binary = storage::client_binary(tag);
    (binary.exists() && storage::client_marker(tag).exists()).then_some(binary)
}

fn emit(app: &AppHandle, downloaded: u32, total: u32, status: &str) {
    let _ = DownloadProgressEvent {
        downloaded,
        total,
        status: status.to_string(),
    }
    .emit(app);
}
