//! Pomme's data folder, shared by the client and the launcher so both find
//! the same files:
//!
//! ```text
//! <root>/
//!   minecraft/assets/     Mojang's asset index and objects
//!   minecraft/versions/   each version's json, jar and extracted assets
//!   pomme-assets/         Pomme's branded assets
//!   profiles/<name>/      options, servers, saves, packs and mods
//!   cache/<version>/      rebuildable, safe to delete
//!   launcher/             launcher-only state
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fs, io};

/// Made with every profile, so mods and packs have a home before anything
/// reads them.
const PROFILE_DIRS: [&str; 4] = ["mods", "resourcepacks", "shaderpacks", "config"];
const MAX_COPY_DEPTH: u32 = 16;

pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn minecraft_assets(&self) -> PathBuf {
        self.root.join("minecraft").join("assets")
    }

    pub fn minecraft_versions(&self) -> PathBuf {
        self.root.join("minecraft").join("versions")
    }

    pub fn pomme_assets(&self) -> PathBuf {
        self.root.join("pomme-assets")
    }

    pub fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn default_profile(&self) -> PathBuf {
        self.profiles().join("default")
    }

    pub fn cache(&self, version: &str) -> PathBuf {
        self.root.join("cache").join(version)
    }

    pub fn launcher(&self) -> PathBuf {
        self.root.join("launcher")
    }

    /// Moves the launcher-era `.pomme` folder at `old` into this layout. Safe
    /// to run on every start: anything whose new place is already taken, and
    /// anything unrecognised, stays in `old`.
    pub fn migrate_from(&self, old: &Path) -> io::Result<()> {
        if !old.is_dir() || old == self.root {
            return Ok(());
        }
        let installations = old.join("installations");
        if let Ok(entries) = fs::read_dir(&installations) {
            for profile in entries.flatten() {
                drop_stale(&profile.path());
            }
        }

        let launcher = self.launcher();
        for (from, to) in [
            (old.join("assets"), self.minecraft_assets()),
            (old.join("versions"), self.minecraft_versions()),
            (old.join("pomme_assets"), self.pomme_assets()),
            (old.join("pomme-assets"), self.pomme_assets()),
            (installations.clone(), self.profiles()),
            (old.join("clients"), launcher.join("clients")),
            (old.join("settings.json"), launcher.join("settings.json")),
        ] {
            merge(&from, &to)?;
        }

        let registry = old.join("installations.json");
        let new_registry = launcher.join("installations.json");
        if registry.is_file() && !new_registry.exists() {
            let json = fs::read_to_string(&registry)?;
            fs::create_dir_all(&launcher)?;
            fs::write(
                &new_registry,
                rehome_installations(&json, &installations, &self.profiles()),
            )?;
            fs::remove_file(&registry)?;
        }

        // Older launchers wrote an empty account list nothing reads.
        let accounts = old.join("accounts.json");
        if fs::read_to_string(&accounts).is_ok_and(|json| json.trim() == "[]") {
            fs::remove_file(&accounts)?;
        }

        let _ = fs::remove_dir(old);
        // Windows nests it as `.pomme\data`.
        if let Some(parent) = old
            .parent()
            .filter(|p| p.file_name() == Some(".pomme".as_ref()))
        {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

/// The per-user layout: `%LOCALAPPDATA%\Pomme`,
/// `~/Library/Application Support/Pomme` or `~/.local/share/pomme`.
pub fn layout() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let name = if cfg!(target_os = "linux") {
            "pomme"
        } else {
            "Pomme"
        };
        let root = directories::BaseDirs::new()
            .map(|dirs| dirs.data_local_dir().join(name))
            .unwrap_or_else(|| PathBuf::from(name));
        Layout::new(root)
    })
}

/// [`Layout::migrate_from`] for this user's old `.pomme` folder.
pub fn migrate() -> io::Result<()> {
    match directories::ProjectDirs::from("", "", ".pomme") {
        Some(old) => layout().migrate_from(old.data_dir()),
        None => Ok(()),
    }
}

pub fn create_profile(dir: &Path) -> io::Result<()> {
    for sub in PROFILE_DIRS {
        fs::create_dir_all(dir.join(sub))?;
    }
    Ok(())
}

/// Renames `from` to `to`, copying instead when they're on different
/// volumes.
pub fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        copy_dir(from, to)?;
        fs::remove_dir_all(from)
    } else {
        fs::copy(from, to)?;
        fs::remove_file(from)
    }
}

pub fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    copy_dir_at(from, to, 0)
}

fn copy_dir_at(from: &Path, to: &Path, depth: u32) -> io::Result<()> {
    if depth > MAX_COPY_DEPTH {
        return Err(io::Error::other("directory nesting too deep"));
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_at(&entry.path(), &target, depth + 1)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Moves `from` to `to`, folding a folder into one that's already there.
/// Anything whose destination is taken stays behind.
fn merge(from: &Path, to: &Path) -> io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if !to.exists() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        return move_path(from, to);
    }
    if from.is_dir() && to.is_dir() {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            merge(&entry.path(), &to.join(entry.file_name()))?;
        }
        let _ = fs::remove_dir(from);
    }
    Ok(())
}

/// The block caches (now kept under `cache/`) and the empty `plugins/` older
/// launchers made.
fn drop_stale(profile: &Path) {
    if let Ok(entries) = fs::read_dir(profile) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("block_cache") && name.ends_with(".json") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let _ = fs::remove_dir(profile.join("plugins"));
}

/// Points each installation whose `directory` was under `from` at the same
/// place under `to`. Custom directories elsewhere, and a registry that
/// doesn't parse, are left as they are.
fn rehome_installations(json: &str, from: &Path, to: &Path) -> String {
    let Ok(mut registry) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_owned();
    };
    for install in registry.as_array_mut().into_iter().flatten() {
        if let Some(dir) = install.get_mut("directory")
            && let Some(rest) = dir
                .as_str()
                .and_then(|d| Path::new(d).strip_prefix(from).ok())
        {
            *dir = to.join(rest).to_string_lossy().into_owned().into();
        }
    }
    serde_json::to_string_pretty(&registry).unwrap_or_else(|_| json.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pomme-dirs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// A launcher-era `.pomme` with every kind of entry the migration handles.
    fn old_tree(old: &Path) {
        write(&old.join("assets/indexes/26.2.json"), "{}");
        write(&old.join("assets/objects/ab/abcd"), "object");
        write(&old.join("versions/26.2/extracted/.extracted"), "26.2");
        fs::create_dir_all(old.join("pomme_assets")).unwrap();
        write(&old.join("pomme-assets/panoramas/panorama_0.png"), "png");
        write(&old.join("installations/default/options.json"), "{}");
        write(
            &old.join("installations/default/saves/world/level.dat"),
            "world",
        );
        write(&old.join("installations/default/block_cache_v4.json"), "{}");
        write(&old.join("installations/default/block_cache.json"), "{}");
        fs::create_dir_all(old.join("installations/default/plugins")).unwrap();
        write(&old.join("installations/26.1/options.json"), "{}");
        write(&old.join("clients/client-v0.1.0/pomme-client"), "binary");
        write(&old.join("settings.json"), "{}");
        write(&old.join("accounts.json"), "[]");
        let registry = serde_json::json!([
            { "id": "a", "directory": old.join("installations/default") },
            { "id": "b", "directory": "/somewhere/else" },
        ]);
        write(&old.join("installations.json"), &registry.to_string());
    }

    #[test]
    fn an_old_folder_moves_into_the_layout() {
        let base = scratch("migrate");
        let old = base.join(".pomme");
        let layout = Layout::new(base.join("Pomme"));
        old_tree(&old);
        write(&old.join("notes.txt"), "mine");

        layout.migrate_from(&old).unwrap();

        let root = base.join("Pomme");
        for file in [
            "minecraft/assets/indexes/26.2.json",
            "minecraft/assets/objects/ab/abcd",
            "minecraft/versions/26.2/extracted/.extracted",
            "pomme-assets/panoramas/panorama_0.png",
            "profiles/default/options.json",
            "profiles/default/saves/world/level.dat",
            "profiles/26.1/options.json",
            "launcher/clients/client-v0.1.0/pomme-client",
            "launcher/settings.json",
            "launcher/installations.json",
        ] {
            assert!(root.join(file).is_file(), "missing {file}");
        }
        for gone in [
            "profiles/default/block_cache_v4.json",
            "profiles/default/block_cache.json",
            "profiles/default/plugins",
        ] {
            assert!(!root.join(gone).exists(), "kept {gone}");
        }
        assert!(!old.join("accounts.json").exists());
        assert_eq!(fs::read_to_string(old.join("notes.txt")).unwrap(), "mine");

        let registry: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(root.join("launcher/installations.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            registry[0]["directory"].as_str().map(PathBuf::from),
            Some(layout.default_profile())
        );
        assert_eq!(registry[1]["directory"], "/somewhere/else");

        layout.migrate_from(&old).unwrap();
        assert_eq!(fs::read_to_string(old.join("notes.txt")).unwrap(), "mine");
        assert!(root.join("profiles/default/options.json").is_file());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_emptied_old_folder_is_removed() {
        let base = scratch("emptied");
        let old = base.join(".pomme");
        let layout = Layout::new(base.join("Pomme"));
        old_tree(&old);

        layout.migrate_from(&old).unwrap();

        assert!(!old.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_taken_destination_keeps_the_old_entry() {
        let base = scratch("merge");
        let old = base.join(".pomme");
        let layout = Layout::new(base.join("Pomme"));
        write(&old.join("pomme-assets/panoramas/panorama_0.png"), "old");
        write(&old.join("pomme-assets/panoramas/panorama_1.png"), "moved");
        write(
            &layout.pomme_assets().join("panoramas/panorama_0.png"),
            "new",
        );

        layout.migrate_from(&old).unwrap();

        let panoramas = layout.pomme_assets().join("panoramas");
        assert_eq!(
            fs::read_to_string(panoramas.join("panorama_0.png")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(panoramas.join("panorama_1.png")).unwrap(),
            "moved"
        );
        assert_eq!(
            fs::read_to_string(old.join("pomme-assets/panoramas/panorama_0.png")).unwrap(),
            "old"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_profile_gets_its_folders() {
        let dir = scratch("profile");
        create_profile(&dir).unwrap();
        for sub in PROFILE_DIRS {
            assert!(dir.join(sub).is_dir());
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
