use crate::storage;

use serde::Deserialize;
use std::path::Path;
use tauri::AppHandle;
use tauri_specta::Event;

const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RESOURCES_BASE_URL: &str = "https://resources.download.minecraft.net";

#[derive(Deserialize)]
struct VersionManifest {
    versions: Vec<VersionEntry>,
}

#[derive(Deserialize)]
struct VersionEntry {
    id: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionJson {
    asset_index: AssetIndexRef,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct AssetIndexRef {
    url: String,
}

#[derive(Deserialize)]
struct Downloads {
    client: DownloadEntry,
}

#[derive(Deserialize)]
struct DownloadEntry {
    url: String,
    sha1: String,
    size: u64,
}

#[derive(Deserialize)]
struct AssetIndexJson {
    objects: std::collections::HashMap<String, AssetObject>,
}

#[derive(Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

#[derive(serde::Serialize, Clone, specta::Type, tauri_specta::Event)]
pub struct DownloadProgressEvent {
    pub downloaded: u32,
    pub total: u32,
    pub status: String,
}

pub fn needs_download(version: &str) -> bool {
    let no_index = !storage::indexes_dir()
        .join(format!("{version}.json"))
        .exists();
    let not_extracted = !storage::version_extracted_marker(version).exists();
    no_index || not_extracted
}

pub async fn download(app: &AppHandle, version: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let index_path = storage::indexes_dir().join(format!("{version}.json"));

    let (asset_index, version_json) = if index_path.exists() {
        let content = std::fs::read_to_string(&index_path).map_err(|e| e.to_string())?;
        let idx: AssetIndexJson = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        (idx, None)
    } else {
        emit_progress(app, 0, 1, "Fetching version manifest...");
        let manifest: VersionManifest = client
            .get(VERSION_MANIFEST_URL)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        let entry = manifest
            .versions
            .iter()
            .find(|v| v.id == version)
            .ok_or_else(|| format!("Version {version} not found"))?;

        let vj: VersionJson = client
            .get(&entry.url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        let index_content = client
            .get(&vj.asset_index.url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;

        std::fs::write(&index_path, &index_content).map_err(|e| e.to_string())?;
        let idx: AssetIndexJson =
            serde_json::from_str(&index_content).map_err(|e| e.to_string())?;
        (idx, Some(vj))
    };

    download_objects(app, &client, &asset_index).await?;
    download_jar(app, &client, version, version_json.as_ref()).await?;

    Ok(())
}

const CONCURRENT_DOWNLOADS: usize = 32;

async fn download_objects(
    app: &AppHandle,
    client: &reqwest::Client,
    index: &AssetIndexJson,
) -> Result<(), String> {
    let objects = storage::objects_dir();

    let mut to_download: Vec<(String, String, u64)> = Vec::new();
    let mut total_bytes: u64 = 0;
    for obj in index.objects.values() {
        let prefix = &obj.hash[..2];
        let path = objects.join(prefix).join(&obj.hash);
        if path.exists() && std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) == obj.size {
            continue;
        }
        total_bytes += obj.size;
        to_download.push((
            obj.hash.clone(),
            format!("{RESOURCES_BASE_URL}/{prefix}/{}", obj.hash),
            obj.size,
        ));
    }

    let total = to_download.len() as u32;
    if total == 0 {
        emit_progress(app, total, total, "Assets downloaded");
        return Ok(());
    }

    let mut completed = 0u32;
    let mut downloaded_bytes: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = to_download.into_iter();

    for _ in 0..CONCURRENT_DOWNLOADS {
        if let Some((hash, url, size)) = pending.next() {
            let c = client.clone();
            tasks.spawn(async move {
                let bytes = c.get(&url).send().await?.bytes().await?;
                Ok::<_, reqwest::Error>((hash, bytes, size))
            });
        }
    }

    while let Some(result) = tasks.join_next().await {
        let (hash, bytes, size) = result
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;

        let prefix = &hash[..2];
        let dir = objects.join(prefix);
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(&hash), &bytes).map_err(|e| e.to_string())?;

        completed += 1;
        downloaded_bytes += size;

        if let Some((hash, url, size)) = pending.next() {
            let c = client.clone();
            tasks.spawn(async move {
                let bytes = c.get(&url).send().await?.bytes().await?;
                Ok::<_, reqwest::Error>((hash, bytes, size))
            });
        }

        if last_emit.elapsed().as_millis() >= 100 {
            let dl_mb = downloaded_bytes as f64 / (1024.0 * 1024.0);
            let total_mb = total_bytes as f64 / (1024.0 * 1024.0);
            emit_progress(
                app,
                completed,
                total,
                &format!("Downloading assets ({dl_mb:.1}/{total_mb:.1} MB)"),
            );
            last_emit = std::time::Instant::now();
        }
    }

    emit_progress(app, total, total, "Assets downloaded");
    Ok(())
}

async fn download_jar(
    app: &AppHandle,
    client: &reqwest::Client,
    version: &str,
    cached_vj: Option<&VersionJson>,
) -> Result<(), String> {
    let jar_assets = storage::version_extracted_dir(version);
    let marker = storage::version_extracted_marker(version);
    if marker.exists() {
        return Ok(());
    }

    let ver_dir = storage::version_dir(version);
    let _ = std::fs::create_dir_all(&ver_dir);

    let fetched;
    let dl = if let Some(vj) = cached_vj {
        &vj.downloads.client
    } else {
        emit_progress(app, 0, 1, "Fetching version info...");
        let manifest: VersionManifest = client
            .get(VERSION_MANIFEST_URL)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let entry = manifest
            .versions
            .iter()
            .find(|v| v.id == version)
            .ok_or_else(|| format!("Version {version} not found"))?;
        let vj: VersionJson = client
            .get(&entry.url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        fetched = vj.downloads.client;
        &fetched
    };

    let jar_path = storage::version_jar(version);
    if !jar_path.exists() || std::fs::metadata(&jar_path).map(|m| m.len()).unwrap_or(0) != dl.size {
        emit_progress(app, 0, 1, "Downloading client JAR...");
        let bytes = client
            .get(&dl.url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;

        let actual_hash = format!("{}", sha1_smol::Sha1::from(&bytes).digest());
        if actual_hash != dl.sha1 {
            log::warn!("JAR hash mismatch: expected {}, got {actual_hash}", dl.sha1);
        }

        std::fs::write(&jar_path, &bytes).map_err(|e| e.to_string())?;
    }

    extract_jar(app, &jar_path, &jar_assets)?;
    std::fs::write(&marker, version).map_err(|e| e.to_string())?;

    Ok(())
}

fn extract_jar(app: &AppHandle, jar_path: &Path, output_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(jar_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let total = archive.len() as u32;
    let mut extracted = 0u32;
    let mut last_emit = std::time::Instant::now();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();

        if !name.starts_with("assets/") || entry.is_dir() {
            continue;
        }

        let out_path = output_dir.join(&name);
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut out_file = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;

        extracted += 1;
        if last_emit.elapsed().as_millis() >= 100 {
            emit_progress(
                app,
                extracted,
                total,
                &format!("Extracting client JAR ({extracted}/{total})"),
            );
            last_emit = std::time::Instant::now();
        }
    }

    Ok(())
}

fn emit_progress(app: &AppHandle, downloaded: u32, total: u32, status: &str) {
    let _ = DownloadProgressEvent {
        downloaded,
        total,
        status: status.to_string(),
    }
    .emit(app);
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
