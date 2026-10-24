//! The Pomme client's own builds, published as `client-v*` GitHub releases:
//! finding the newest, and downloading this platform's zip checked against the
//! release's `sha256sums.txt`.

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Progress;

/// Newest first; listed rather than `/releases/latest`, which held the
/// launcher's releases before the client's.
const RELEASES_URL: &str = "https://api.github.com/repos/PommeMC/Client/releases?per_page=30";
const TAG_PREFIX: &str = "client-v";
const CHECKSUMS_ASSET: &str = "sha256sums.txt";
/// GitHub's API refuses requests without one.
const USER_AGENT: &str = "pomme";

#[derive(Deserialize)]
pub struct Release {
    pub tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The Linux AppImage, which updates by replacing itself: its inside is a
/// read-only mount.
pub const APPIMAGE: &str = "pomme-client-linux-x64-gnu.AppImage";

/// This platform's release zip, the files that sit beside the binary.
pub fn platform_zip() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("pomme-client-windows-x64.zip"),
        ("linux", "x86_64") => Ok("pomme-client-linux-x64-gnu.zip"),
        ("macos", "aarch64") => Ok("pomme-client-macos-arm64.zip"),
        (os, arch) => Err(format!("No client build for {os}/{arch}")),
    }
}

/// Whether release `theirs` is a later build than `ours`, by the `x.y.z` of
/// `client-vx.y.z+<minecraft>`; never for a tag that doesn't parse.
pub fn is_newer(ours: &str, theirs: &str) -> bool {
    match (release_core(ours), release_core(theirs)) {
        (Some(ours), Some(theirs)) => theirs > ours,
        _ => false,
    }
}

/// The `x.y.z` of a `client-vx.y.z+<minecraft>` tag.
pub fn release_version(tag: &str) -> Option<&str> {
    let core = tag.strip_prefix(TAG_PREFIX)?;
    Some(core.split_once('+').map_or(core, |(core, _)| core))
}

fn release_core(tag: &str) -> Option<(u32, u32, u32)> {
    let mut parts = release_version(tag)?.split('.').map(str::parse);
    let version = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(version)
}

pub async fn latest_release(http: &reqwest::Client) -> Result<Release, String> {
    let releases: Vec<Release> = get(http, RELEASES_URL)
        .await?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    releases
        .into_iter()
        .find(|r| r.tag_name.starts_with(TAG_PREFIX))
        .ok_or_else(|| "No client release found".to_owned())
}

/// File `name` of `release`, refused unless it matches the release's
/// published sha256.
pub async fn download(
    http: &reqwest::Client,
    release: &Release,
    name: &str,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<Vec<u8>, String> {
    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("Release {} has no {name}", release.tag_name))
    };
    let sums = get(http, &asset(CHECKSUMS_ASSET)?.browser_download_url)
        .await?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let expected = checksum_for(&sums, name).ok_or_else(|| format!("No checksum for {name}"))?;

    let mut response = get(http, &asset(name)?.browser_download_url).await?;
    let total = response.content_length().unwrap_or(0) as u32;
    let mut bytes = Vec::with_capacity(total as usize);
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        on_progress(Progress {
            done: bytes.len() as u32,
            total,
            status: "Downloading".to_owned(),
        });
    }

    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(bytes)
    } else {
        Err(format!("{name} didn't match its sha256"))
    }
}

/// Extracts every file of a release zip straight into `dir`: the zips are
/// flat, the binary beside the libraries it loads from its own folder.
pub fn extract_flat(zip: &[u8], dir: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(ToOwned::to_owned))
        else {
            continue;
        };
        let path = dir.join(name);
        let failed = |e: std::io::Error| format!("{}: {e}", path.display());
        let mut out = std::fs::File::create(&path).map_err(failed)?;
        std::io::copy(&mut entry, &mut out).map_err(failed)?;
        // The binary and the libraries beside it stay executable.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .map_err(failed)?;
        }
    }
    Ok(())
}

/// The hash for `asset` in `sha256sum` output (`<hash>  <file>`).
fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let file = fields.next()?.trim_start_matches('*');
        (file.rsplit('/').next() == Some(asset)).then(|| hash.to_owned())
    })
}

async fn get(http: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    http.get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("{url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_the_release_core() {
        assert!(is_newer("client-v0.1.9+26.2", "client-v0.1.10+26.2"));
        assert!(is_newer("client-v0.1.0+26.2", "client-v1.0.0+26.3"));
        assert!(!is_newer("client-v0.2.0+26.2", "client-v0.2.0+26.3"));
        assert!(!is_newer("client-v0.2.0+26.2", "client-v0.1.5+26.2"));
        assert!(!is_newer("client-v0.2.0+26.2", "launcher-v0.3.0"));
        assert!(!is_newer("dev", "client-v0.3.0+26.2"));
    }

    #[test]
    fn the_version_drops_the_minecraft_part() {
        assert_eq!(release_version("client-v0.2.8+26.2"), Some("0.2.8"));
        assert_eq!(release_version("client-v0.2.8"), Some("0.2.8"));
        assert_eq!(release_version("launcher-v0.1.2"), None);
    }

    #[test]
    fn checksum_matches_the_file_name() {
        let sums = "aa11  pomme-client-macos-arm64.zip\nbb22 *dist/pomme-client-windows-x64.zip\n";
        assert_eq!(
            checksum_for(sums, "pomme-client-windows-x64.zip").as_deref(),
            Some("bb22")
        );
        assert_eq!(
            checksum_for(sums, "pomme-client-macos-arm64.zip").as_deref(),
            Some("aa11")
        );
        assert_eq!(checksum_for(sums, "pomme-client-linux-x64-gnu.zip"), None);
    }

    #[test]
    fn extract_flattens_keeps_modes_and_stays_inside_the_folder() {
        let dir = crate::tests::temp_dir("extract-flat");
        let zip = crate::tests::zip_of(&[
            ("bundle/pomme-client", "new", 0o755),
            ("../escape", "no", 0o644),
        ]);
        extract_flat(&zip, &dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("pomme-client")).unwrap(),
            "new"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("pomme-client"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        assert!(!dir.join("escape").exists());
        assert!(!dir.parent().unwrap().join("escape").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
