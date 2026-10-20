//! Installs a Minecraft version's assets the way the client reads them: the
//! asset index and objects under `assets/`, and the client jar's `assets/`
//! extracted under `versions/<version>/extracted/`.
//!
//! Every download is checked against the sha1 Mojang publishes, retried, and
//! written beside its final path then renamed, so an interrupted install
//! leaves nothing half-written under a real name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

pub mod client;

const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RESOURCES_BASE_URL: &str = "https://resources.download.minecraft.net";
const CONCURRENT_DOWNLOADS: usize = 32;
const ATTEMPTS: u32 = 3;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// How far an install has got: `done` of `total` steps, with what's happening.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    pub status: String,
}

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
    asset_index: Download,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    client: Download,
}

#[derive(Deserialize)]
struct Download {
    url: String,
    sha1: String,
}

#[derive(Deserialize)]
struct AssetIndexJson {
    objects: HashMap<String, AssetObject>,
}

#[derive(Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

fn index_path(assets_dir: &Path, version: &str) -> PathBuf {
    assets_dir.join("indexes").join(format!("{version}.json"))
}

fn extracted_dir(versions_dir: &Path, version: &str) -> PathBuf {
    versions_dir.join(version).join("extracted")
}

/// Written into a version's extraction once it's complete.
const EXTRACTED_MARKER: &str = ".extracted";

fn extracted_marker(versions_dir: &Path, version: &str) -> PathBuf {
    extracted_dir(versions_dir, version).join(EXTRACTED_MARKER)
}

/// The version's index and its jar's extraction are in place. Each is
/// written only once it's complete, the extraction with its `.extracted`
/// marker; older launchers extracted in place and wrote the marker last, so
/// one without it may be half done.
pub fn is_installed(assets_dir: &Path, versions_dir: &Path, version: &str) -> bool {
    index_path(assets_dir, version).exists() && extracted_marker(versions_dir, version).exists()
}

/// Downloads whatever of `version` is missing or damaged, reporting through
/// `on_progress`. Runs on a multi-threaded tokio runtime: the jar extraction
/// blocks in place.
pub async fn install(
    http: &reqwest::Client,
    assets_dir: &Path,
    versions_dir: &Path,
    version: &str,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<(), String> {
    let report = |done, total, status: &str| {
        on_progress(Progress {
            done,
            total,
            status: status.to_owned(),
        })
    };

    report(0, 1, "Fetching version info...");
    let manifest: VersionManifest = get_json(http, VERSION_MANIFEST_URL).await?;
    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == version)
        .ok_or_else(|| format!("Version {version} not found"))?;
    let version_json: VersionJson = get_json(http, &entry.url).await?;

    let index_bytes = fetch_verified(
        http,
        &version_json.asset_index.url,
        &version_json.asset_index.sha1,
    )
    .await?;
    let index: AssetIndexJson =
        serde_json::from_slice(&index_bytes).map_err(|e| format!("Asset index: {e}"))?;
    download_objects(http, &assets_dir.join("objects"), &index, &report).await?;
    // After the objects, so an index on disk means they're all there.
    write_atomic(&index_path(assets_dir, version), &index_bytes).await?;

    let out = extracted_dir(versions_dir, version);
    if !extracted_marker(versions_dir, version).exists() {
        report(0, 1, "Downloading the client jar...");
        let client = &version_json.downloads.client;
        let jar = fetch_verified(http, &client.url, &client.sha1).await?;
        let mut part = out.as_os_str().to_owned();
        part.push(".part");
        let part = PathBuf::from(part);
        let extract_report = |done, total| {
            report(
                done,
                total,
                &format!("Extracting the client jar ({done}/{total})"),
            )
        };
        tokio::task::block_in_place(|| {
            let _ = std::fs::remove_dir_all(&part);
            extract_assets(&jar, &part, &extract_report)?;
            // Inside the `.part`, so it lands with the files in one rename.
            std::fs::write(part.join(EXTRACTED_MARKER), version)
                .map_err(|e| format!("{}: {e}", part.display()))?;
            let _ = std::fs::remove_dir_all(&out);
            std::fs::rename(&part, &out).map_err(|e| format!("{}: {e}", out.display()))
        })?;
    }
    Ok(())
}

async fn download_objects(
    http: &reqwest::Client,
    objects_dir: &Path,
    index: &AssetIndexJson,
    report: &(dyn Fn(u32, u32, &str) + Send + Sync),
) -> Result<(), String> {
    let missing: Vec<&AssetObject> = index
        .objects
        .values()
        .filter(|obj| {
            !std::fs::metadata(object_path(objects_dir, &obj.hash))
                .is_ok_and(|m| m.len() == obj.size)
        })
        .collect();
    let total = missing.len() as u32;
    let total_mb = missing.iter().map(|o| o.size).sum::<u64>() as f64 / 1048576.0;

    let mut done = 0;
    let mut done_bytes = 0;
    let mut last_report = Instant::now();
    let mut pending = missing.into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    let spawn = |tasks: &mut tokio::task::JoinSet<_>, obj: &AssetObject| {
        let (http, hash, size) = (http.clone(), obj.hash.clone(), obj.size);
        let path = object_path(objects_dir, &hash);
        tasks.spawn(async move {
            let url = format!("{RESOURCES_BASE_URL}/{}/{hash}", &hash[..2]);
            let bytes = fetch_verified(&http, &url, &hash).await?;
            write_atomic(&path, &bytes).await?;
            Ok::<u64, String>(size)
        });
    };
    for obj in pending.by_ref().take(CONCURRENT_DOWNLOADS) {
        spawn(&mut tasks, obj);
    }
    while let Some(result) = tasks.join_next().await {
        done_bytes += result.map_err(|e| e.to_string())??;
        done += 1;
        if let Some(obj) = pending.next() {
            spawn(&mut tasks, obj);
        }
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            let done_mb = done_bytes as f64 / 1048576.0;
            report(
                done,
                total,
                &format!("Downloading assets ({done_mb:.1}/{total_mb:.1} MB)"),
            );
            last_report = Instant::now();
        }
    }
    Ok(())
}

fn object_path(objects_dir: &Path, hash: &str) -> PathBuf {
    objects_dir.join(&hash[..2]).join(hash)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let bytes = with_retries(url, || fetch(http, url)).await?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// The body of `url`, which must hash to `sha1`.
async fn fetch_verified(http: &reqwest::Client, url: &str, sha1: &str) -> Result<Vec<u8>, String> {
    with_retries(url, || async {
        let bytes = fetch(http, url).await?;
        if sha1_hex(&bytes) == sha1 {
            Ok(bytes)
        } else {
            Err("the download didn't match its sha1".into())
        }
    })
    .await
}

async fn with_retries<T, F, Fut>(url: &str, mut attempt: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut last_error = String::new();
    for n in 0..ATTEMPTS {
        if n > 0 {
            tokio::time::sleep(Duration::from_millis(500 * u64::from(n))).await;
        }
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(e) => last_error = e,
        }
    }
    Err(format!("{url}: {last_error}"))
}

fn sha1_hex(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

/// Writes `bytes` beside `path`, then renames it into place.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    tokio::fs::write(&part, bytes)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    tokio::fs::rename(&part, path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Extracts the jar's `assets/` entries into `out`, refusing any entry whose
/// path would land outside it.
fn extract_assets(jar: &[u8], out: &Path, report: &dyn Fn(u32, u32)) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(jar)).map_err(|e| format!("Client jar: {e}"))?;
    let total = archive.len() as u32;
    let mut last_report = Instant::now();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Client jar: {e}"))?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() || !name.starts_with("assets") {
            continue;
        }
        let path = out.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut file =
            std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| format!("{}: {e}", path.display()))?;
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            report(i as u32 + 1, total);
            last_report = Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{extract_assets, is_installed, sha1_hex, write_atomic};

    pub(crate) fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pomme-install-{label}-{}", std::process::id()))
    }

    fn jar(entries: &[(&str, &str)]) -> Vec<u8> {
        let entries: Vec<_> = entries
            .iter()
            .map(|&(name, body)| (name, body, 0o644))
            .collect();
        zip_of(&entries)
    }

    /// A zip of `(name, body, unix mode)` entries.
    pub(crate) fn zip_of(entries: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for &(name, body, mode) in entries {
            let options = zip::write::SimpleFileOptions::default().unix_permissions(mode);
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn sha1_is_lower_hex() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn only_assets_extract_and_escapes_are_refused() {
        let dir = temp_dir("extract");
        let jar = jar(&[
            ("assets/minecraft/lang/en_us.json", "{}"),
            ("net/minecraft/Main.class", "code"),
            ("assets/../../escaped.txt", "nope"),
        ]);
        extract_assets(&jar, &dir, &|_, _| {}).unwrap();
        assert!(dir.join("assets/minecraft/lang/en_us.json").exists());
        assert!(!dir.join("net").exists());
        assert!(!dir.parent().unwrap().join("escaped.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn writes_land_whole_and_mark_the_install() {
        let dir = temp_dir("install");
        let (assets, versions) = (dir.join("assets"), dir.join("versions"));
        assert!(!is_installed(&assets, &versions, "26.2"));
        write_atomic(&assets.join("indexes/26.2.json"), b"{}")
            .await
            .unwrap();
        assert!(!is_installed(&assets, &versions, "26.2"));
        // What an older launcher left when it was closed mid-extraction.
        std::fs::create_dir_all(versions.join("26.2/extracted/assets")).unwrap();
        assert!(!is_installed(&assets, &versions, "26.2"));
        std::fs::write(versions.join("26.2/extracted/.extracted"), "26.2").unwrap();
        assert!(is_installed(&assets, &versions, "26.2"));
        assert!(!assets.join("indexes/26.2.json.part").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
