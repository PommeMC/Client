//! Downloads the correct per-platform `pomme-client` binary from the project's
//! GitHub releases (vanilla-style), caches it under `.pomme/clients/<tag>/`,
//! verifies its sha256, and returns the path. The launcher prefers a local dev
//! build (`commands::find_client_binary`); this is the production path used
//! when no local build is present.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_specta::Event;

use crate::downloader::DownloadProgressEvent;
use crate::storage;

const RELEASES_URL: &str = "https://api.github.com/repos/PommeMC/Client/releases?per_page=30";
const USER_AGENT: &str = "pomme-launcher";
const CHECKSUMS_ASSET: &str = "sha256sums.txt";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The release asset name for the current platform, or `None` if unsupported.
fn platform_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("pomme-client-windows-x64.zip"),
        ("linux", "x86_64") => Some("pomme-client-linux-x64-gnu.zip"),
        ("macos", "aarch64") => Some("pomme-client-macos-arm64.zip"),
        _ => None,
    }
}

/// Ensure the latest released client is installed and return its binary path.
/// Returns a cached copy if it's already current; on network failure, falls
/// back to the newest cached client so the game still launches offline.
pub async fn ensure_client(app: &AppHandle) -> Result<PathBuf, String> {
    let asset_name = platform_asset().ok_or_else(|| {
        format!(
            "No client build for your platform ({}/{})",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    let client = reqwest::Client::new();

    let release = match latest_client_release(&client).await {
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

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("Release {} has no asset {asset_name}", release.tag_name))?;
    let checksums = release.assets.iter().find(|a| a.name == CHECKSUMS_ASSET);

    install(app, &client, &release.tag_name, asset, checksums).await?;
    Ok(storage::client_binary(&release.tag_name))
}

/// Newest `client-v*` release (the GitHub API returns releases newest-first).
async fn latest_client_release(client: &reqwest::Client) -> Result<Release, String> {
    let releases: Vec<Release> = get(client, RELEASES_URL)
        .await?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    releases
        .into_iter()
        .find(|r| r.tag_name.starts_with("client-v"))
        .ok_or_else(|| "No client-v* release found".to_string())
}

async fn install(
    app: &AppHandle,
    client: &reqwest::Client,
    tag: &str,
    asset: &Asset,
    checksums: Option<&Asset>,
) -> Result<(), String> {
    let dir = storage::client_version_dir(tag);
    let _ = std::fs::create_dir_all(&dir);

    emit(app, 0, 1, "Downloading client...");
    let bytes = get(client, &asset.browser_download_url)
        .await?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(cs) = checksums {
        let sums = get(client, &cs.browser_download_url)
            .await?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let expected = checksum_for(&sums, &asset.name)
            .ok_or_else(|| format!("No checksum for {} in {CHECKSUMS_ASSET}", asset.name))?;
        let actual = hex(&Sha256::digest(&bytes));
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(format!(
                "Client checksum mismatch for {}: expected {expected}, got {actual}",
                asset.name
            ));
        }
    } else {
        log::warn!("Release {tag} has no {CHECKSUMS_ASSET}; skipping client integrity check");
    }

    emit(app, 1, 1, "Installing client...");
    extract_all(&bytes, &dir)?;
    let binary = storage::client_binary(tag);
    if !binary.exists() {
        return Err("client binary not found in downloaded archive".to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }

    std::fs::write(storage::client_marker(tag), tag).map_err(|e| e.to_string())?;
    prune_old_clients(tag);
    Ok(())
}

/// Extract every file in the zip (flat) into `dir`. Releases bundle the binary
/// plus, on macOS, the Vulkan loader + MoltenVK + ICD next to it.
fn extract_all(zip_bytes: &[u8], dir: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let base = entry.name().rsplit('/').next().unwrap_or_default();
        if base.is_empty() {
            continue;
        }
        let mut out = std::fs::File::create(dir.join(base)).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Find the sha256 for `asset_name` in `sha256sum`-format text (`<hash>
/// <file>`).
fn checksum_for(sums: &str, asset_name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        let base = file.rsplit('/').next().unwrap_or(file);
        (base == asset_name).then(|| hash.to_string())
    })
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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The cached binary path for `tag` if it's fully installed (binary + marker).
fn installed_binary(tag: &str) -> Option<PathBuf> {
    let binary = storage::client_binary(tag);
    (binary.exists() && storage::client_marker(tag).exists()).then_some(binary)
}

/// GET `url` with the GitHub-required User-Agent, erroring on a non-2xx status.
async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())
}

fn emit(app: &AppHandle, downloaded: u32, total: u32, status: &str) {
    let _ = DownloadProgressEvent {
        downloaded,
        total,
        status: status.to_string(),
    }
    .emit(app);
}
