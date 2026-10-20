//! Keeps a release build current: before the menu, a newer `client-v*`
//! release replaces this build's files where they are, and the client
//! relaunches into it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pomme_install::{Progress, client};

/// The release this binary was built as (`release-client.yml` sets it); dev
/// and local builds have none and never update.
const RELEASE: Option<&str> = option_env!("POMME_RELEASE");
/// Where a new release unpacks before its files move into place.
const STAGING: &str = ".pomme-update";
/// Added to a replaced file's name; it's removed on a later start, since a
/// running Windows exe can be renamed but not deleted.
const REPLACED: &str = "old";
/// Offline, the check gives up this soon rather than hold the menu back.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// This build's `x.y.z` for display, or "dev" for one that isn't a release.
pub fn version() -> &'static str {
    RELEASE.and_then(client::release_version).unwrap_or("dev")
}

/// Whether this launch checks for a newer release. Not when the old launcher
/// started it: that keeps its own copies under `launcher/clients`.
pub fn enabled(args: &crate::LaunchArgs) -> bool {
    #[cfg(feature = "test-harness")]
    if args.harness_script.is_some() {
        return false;
    }
    RELEASE.is_some() && cfg!(not(mobile)) && args.launch_token.is_none()
}

/// Installs a newer release over this one, returning the binary to relaunch,
/// or `None` when this build is current.
pub async fn run(
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<Option<PathBuf>, String> {
    let Some(ours) = RELEASE else {
        return Ok(None);
    };
    let http = crate::net::http::client();
    let release = tokio::time::timeout(CHECK_TIMEOUT, client::latest_release(http))
        .await
        .map_err(|_| "timed out".to_owned())??;
    if !client::is_newer(ours, &release.tag_name) {
        return Ok(None);
    }
    tracing::info!("Updating from {ours} to {}", release.tag_name);
    if let Some(appimage) = appimage() {
        let file = client::download(http, &release, client::APPIMAGE, on_progress).await?;
        replace_appimage(&file, &appimage).await?;
        return Ok(Some(appimage));
    }
    let zip = client::download(http, &release, client::platform_zip()?, on_progress).await?;
    // Before the swap: Linux reports a renamed running binary by its new name.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the binary has no folder")?.to_owned();
    tokio::task::spawn_blocking(move || apply(&zip, &dir))
        .await
        .map_err(|e| e.to_string())??;
    Ok(Some(exe))
}

/// The `.AppImage` this runs from, which its runtime names in `$APPIMAGE`;
/// `current_exe()` is inside its read-only mount.
fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from)
}

async fn replace_appimage(bytes: &[u8], path: &Path) -> Result<(), String> {
    pomme_install::write_atomic(path, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

pub fn relaunch(exe: &Path) -> std::io::Result<()> {
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(drop)
}

/// Removes what the last update left behind.
pub fn clean_up() {
    if RELEASE.is_none() || cfg!(mobile) || appimage().is_some() {
        return;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        remove_leftovers(dir);
    }
}

fn remove_leftovers(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join(STAGING));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == REPLACED) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Moves each file of `zip` into `dir`, the one it replaces renamed aside;
/// on any failure the replaced files come back, so a half-applied release
/// never runs.
fn apply(zip: &[u8], dir: &Path) -> Result<(), String> {
    let staging = dir.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    client::extract_flat(zip, &staging)?;
    let mut moved = Vec::new();
    if let Err(e) = move_into_place(&staging, dir, &mut moved) {
        for target in moved.iter().rev() {
            let _ = std::fs::remove_file(target);
            let _ = std::fs::rename(replaced(target), target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

fn move_into_place(staging: &Path, dir: &Path, moved: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    for entry in entries {
        let source = entry.map_err(|e| e.to_string())?.path();
        let target = dir.join(source.file_name().ok_or("unnamed file")?);
        let failed = |e: std::io::Error| format!("{}: {e}", target.display());
        if target.exists() {
            // One the last update couldn't remove, its process still running.
            let replaced = replaced(&target);
            let _ = std::fs::remove_file(&replaced);
            std::fs::rename(&target, &replaced).map_err(failed)?;
        }
        moved.push(target.clone());
        std::fs::rename(&source, &target).map_err(failed)?;
    }
    Ok(())
}

fn replaced(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(REPLACED);
    name.into()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn release_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn install_dir(label: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = crate::test_util::test_temp_dir(label);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn apply_swaps_every_file_and_leaves_the_old_ones_aside() {
        let dir = install_dir("apply", &[("pomme-client", "old"), ("libopenal.so", "old")]);
        let zip = release_zip(&[
            ("pomme-client", "new"),
            ("libopenal.so", "new"),
            ("OpenAL-Soft-NOTICE.txt", "new"),
        ]);
        apply(&zip, &dir).unwrap();
        for name in ["pomme-client", "libopenal.so", "OpenAL-Soft-NOTICE.txt"] {
            assert_eq!(read(&dir, name), "new");
        }
        assert_eq!(read(&dir, "pomme-client.old"), "old");
        assert!(!dir.join(STAGING).exists());

        remove_leftovers(&dir);
        assert!(!dir.join("pomme-client.old").exists());
        assert!(!dir.join("libopenal.so.old").exists());
        assert_eq!(read(&dir, "pomme-client"), "new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn the_appimage_is_replaced_whole_and_executable() {
        let dir = install_dir("appimage", &[("Pomme.AppImage", "old")]);
        let appimage = dir.join("Pomme.AppImage");
        replace_appimage(b"new", &appimage).await.unwrap();
        assert_eq!(read(&dir, "Pomme.AppImage"), "new");
        assert!(!dir.join("Pomme.AppImage.part").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&appimage).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_apply_puts_the_old_files_back() {
        let dir = install_dir(
            "rollback",
            &[("pomme-client", "old"), ("libopenal.so", "old")],
        );
        // Nothing can be renamed onto a folder that isn't empty.
        std::fs::create_dir_all(dir.join("libopenal.so.old/busy")).unwrap();
        let zip = release_zip(&[("pomme-client", "new"), ("libopenal.so", "new")]);
        assert!(apply(&zip, &dir).is_err());
        assert_eq!(read(&dir, "pomme-client"), "old");
        assert_eq!(read(&dir, "libopenal.so"), "old");
        assert!(!dir.join("pomme-client.old").exists());
        assert!(!dir.join(STAGING).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
