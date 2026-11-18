//! The Pomme client's own builds, published as GitHub releases: a `client-v*`
//! one per release and a rolling `nightly` one for master's latest build.
//! Finding the newest on a channel, and downloading this platform's zip
//! checked against the release's `sha256sums.txt`.

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Progress, get};

/// Newest first; listed rather than `/releases/latest`, which held the
/// launcher's releases before the client's.
const RELEASES_URL: &str = "https://api.github.com/repos/PommeMC/Client/releases?per_page=30";
const RELEASE_PREFIX: &str = "client-v";
const NIGHTLY_PREFIX: &str = "nightly-";
/// The tag of the one release each nightly build replaces.
const NIGHTLY_TAG: &str = "nightly";
const CHECKSUMS_ASSET: &str = "sha256sums.txt";

/// Which builds a client updates to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Releases only.
    Stable,
    /// Master's latest build too, whichever is newer.
    Nightly,
}

/// A published build, ordered oldest first: release `x.y.z` from
/// `client-vx.y.z+<minecraft>`, or the `n`th build of master after it from
/// `nightly-x.y.z.n+<minecraft>`. A nightly is newer than the release it
/// follows and older than the next one, which is cut from a later master.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Build {
    version: (u32, u32, u32),
    /// 0 for a release.
    nightly: u32,
}

impl Build {
    pub fn parse(id: &str) -> Option<Self> {
        let (core, is_nightly) = match id.strip_prefix(RELEASE_PREFIX) {
            Some(core) => (core, false),
            None => (id.strip_prefix(NIGHTLY_PREFIX)?, true),
        };
        let core = core.split_once('+').map_or(core, |(core, _)| core);
        let mut parts = core.split('.').map(str::parse);
        let version = (
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        );
        let nightly = if is_nightly {
            parts.next()?.ok().filter(|n| *n > 0)?
        } else {
            0
        };
        parts.next().is_none().then_some(Self { version, nightly })
    }
}

impl std::fmt::Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (major, minor, patch) = self.version;
        write!(f, "{major}.{minor}.{patch}")?;
        if self.nightly > 0 {
            write!(f, "-nightly.{}", self.nightly)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct Release {
    tag_name: String,
    name: Option<String>,
    assets: Vec<Asset>,
}

impl Release {
    /// What names the build: the tag, or under the rolling nightly tag the
    /// release's name.
    pub fn id(&self) -> &str {
        match (self.tag_name.as_str(), &self.name) {
            (NIGHTLY_TAG, Some(name)) => name,
            (tag, _) => tag,
        }
    }
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

/// Whether `theirs` is a later build than `ours`; never for an id that
/// doesn't parse.
pub fn is_newer(ours: &str, theirs: &str) -> bool {
    match (Build::parse(ours), Build::parse(theirs)) {
        (Some(ours), Some(theirs)) => theirs > ours,
        _ => false,
    }
}

pub async fn latest_release(http: &reqwest::Client, channel: Channel) -> Result<Release, String> {
    let releases: Vec<Release> = get(http, RELEASES_URL)
        .await?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    newest(releases, channel).ok_or_else(|| "No client release found".to_owned())
}

/// The newest build among `releases` that `channel` takes.
fn newest(releases: Vec<Release>, channel: Channel) -> Option<Release> {
    releases
        .into_iter()
        .filter_map(|release| Some((Build::parse(release.id())?, release)))
        .filter(|(build, _)| channel == Channel::Nightly || build.nightly == 0)
        .max_by_key(|(build, _)| *build)
        .map(|(_, release)| release)
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
            .ok_or_else(|| format!("Release {} has no {name}", release.id()))
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
    fn a_nightly_sits_between_the_release_it_follows_and_the_next() {
        let (release, nightly) = ("client-v0.2.12+26.2", "nightly-0.2.12.345+26.2");
        assert!(is_newer(release, nightly));
        assert!(is_newer(nightly, "nightly-0.2.12.346+26.2"));
        assert!(is_newer(nightly, "client-v0.2.13+26.2"));
        assert!(!is_newer(nightly, release));
        assert!(!is_newer(nightly, nightly));
        // A nightly names the run that built it.
        assert!(!is_newer(release, "nightly-0.2.12+26.2"));
        assert!(!is_newer(release, "nightly-0.2.12.0+26.2"));
        assert!(!is_newer(release, "nightly"));
    }

    #[test]
    fn a_build_shows_as_its_version() {
        let shown = |id| Build::parse(id).map(|build| build.to_string());
        assert_eq!(shown("client-v0.2.8+26.2").as_deref(), Some("0.2.8"));
        assert_eq!(shown("client-v0.2.8").as_deref(), Some("0.2.8"));
        assert_eq!(
            shown("nightly-0.2.8.345+26.2").as_deref(),
            Some("0.2.8-nightly.345")
        );
        assert_eq!(shown("launcher-v0.1.2"), None);
    }

    /// The id of the newest build `channel` takes from GitHub's `listed`.
    fn newest_id(listed: &str, channel: Channel) -> Option<String> {
        let releases: Vec<Release> = serde_json::from_str(listed).unwrap();
        newest(releases, channel).map(|release| release.id().to_owned())
    }

    #[test]
    fn a_channel_takes_its_newest_build() {
        // As GitHub lists them: the rolling nightly, recreated each night,
        // then the releases newest first.
        let listed = r#"[
            {"tag_name": "nightly", "name": "nightly-0.2.12.345+26.2", "assets": []},
            {"tag_name": "client-v0.2.12+26.2", "name": "client-v0.2.12+26.2", "assets": []},
            {"tag_name": "client-v0.2.11+26.2", "name": null, "assets": []},
            {"tag_name": "launcher-v0.1.2", "name": null, "assets": []}
        ]"#;
        assert_eq!(
            newest_id(listed, Channel::Stable).as_deref(),
            Some("client-v0.2.12+26.2")
        );
        assert_eq!(
            newest_id(listed, Channel::Nightly).as_deref(),
            Some("nightly-0.2.12.345+26.2")
        );
    }

    #[test]
    fn the_newest_build_is_the_highest_however_they_are_listed() {
        let listed = r#"[
            {"tag_name": "client-v0.2.11+26.2", "name": null, "assets": []},
            {"tag_name": "client-v0.2.12+26.2", "name": null, "assets": []}
        ]"#;
        assert_eq!(
            newest_id(listed, Channel::Stable).as_deref(),
            Some("client-v0.2.12+26.2")
        );
    }

    #[test]
    fn the_nightly_channel_takes_a_release_newer_than_the_nightly() {
        let listed = r#"[
            {"tag_name": "client-v0.2.13+26.2", "name": null, "assets": []},
            {"tag_name": "nightly", "name": "nightly-0.2.12.345+26.2", "assets": []}
        ]"#;
        assert_eq!(
            newest_id(listed, Channel::Nightly).as_deref(),
            Some("client-v0.2.13+26.2")
        );
    }

    #[test]
    fn no_release_is_no_build() {
        let listed = r#"[{"tag_name": "launcher-v0.1.2", "name": null, "assets": []}]"#;
        assert_eq!(newest_id(listed, Channel::Nightly), None);
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
