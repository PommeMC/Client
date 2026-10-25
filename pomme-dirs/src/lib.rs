//! Pomme's data folder, shared by the client and the launcher so both find
//! the same files:
//!
//! ```text
//! <root>/
//!   minecraft/assets/     Mojang's asset index and objects
//!   minecraft/versions/   each version's json, jar and extracted assets
//!   pomme-assets/         Pomme's branded assets
//!   bedrock/<tag>/        Bedrock's resource pack, for the Bedrock theme
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
/// Left in a profile moved by copying when deleting the original failed: the
/// copy is complete, so the next run can switch to it and drop the leftover.
const MOVED_MARKER: &str = ".pomme-moved";
/// Added to a move's destination while it's being copied; specific enough
/// that a stale one can be deleted without touching anything of the user's.
const PART_SUFFIX: &str = ".pomme-part";

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

    pub fn bedrock_assets(&self) -> PathBuf {
        self.root.join("bedrock")
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
    /// anything unrecognised, stays in `old`. A failed step doesn't stop the
    /// rest; the next run retries it.
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

        let mut result = Ok(());
        let launcher = self.launcher();
        for (from, to) in [
            (old.join("assets"), self.minecraft_assets()),
            (old.join("versions"), self.minecraft_versions()),
            (old.join("pomme_assets"), self.pomme_assets()),
            (old.join("pomme-assets"), self.pomme_assets()),
            (old.join("clients"), launcher.join("clients")),
            (old.join("settings.json"), launcher.join("settings.json")),
        ] {
            keep_first(&mut result, merge(&from, &to));
        }
        keep_first(&mut result, move_profiles(&installations, &self.profiles()));

        // After a failed run the launcher may already have written an empty
        // list here, which would otherwise hide the real one for good.
        let registry = old.join("installations.json");
        let new_registry = launcher.join("installations.json");
        if registry.is_file()
            && fs::read_to_string(&new_registry).map_or(true, |json| json.trim() == "[]")
        {
            let _ = fs::remove_file(&new_registry);
            keep_first(
                &mut result,
                fs::create_dir_all(&launcher).and_then(|()| move_path(&registry, &new_registry)),
            );
        }
        if let Ok(json) = fs::read_to_string(&new_registry)
            && let Some(rehomed) = rehome_installations(&json, &installations, &self.profiles())
        {
            keep_first(&mut result, fs::write(&new_registry, rehomed));
        }

        // Older launchers wrote an empty account list nothing reads.
        let accounts = old.join("accounts.json");
        if fs::read_to_string(&accounts).is_ok_and(|json| json.trim() == "[]") {
            keep_first(&mut result, fs::remove_file(&accounts));
        }

        let _ = fs::remove_dir(old);
        // Windows nests it as `.pomme\data`.
        if let Some(parent) = old
            .parent()
            .filter(|p| p.file_name() == Some(".pomme".as_ref()))
        {
            let _ = fs::remove_dir(parent);
        }
        result
    }
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Replaces the per-user root, for platforms (Android) whose app storage
/// `directories` doesn't know. Call it before anything uses [`layout`].
pub fn set_root(root: PathBuf) {
    let _ = ROOT.set(root);
}

/// The per-user layout: `%LOCALAPPDATA%\Pomme`,
/// `~/Library/Application Support/Pomme` or `~/.local/share/pomme`, unless
/// [`set_root`] named another.
pub fn layout() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let root = ROOT.get().cloned().unwrap_or_else(|| {
            let name = if cfg!(target_os = "linux") {
                "pomme"
            } else {
                "Pomme"
            };
            directories::BaseDirs::new()
                .map(|dirs| dirs.data_local_dir().join(name))
                .unwrap_or_else(|| PathBuf::from(name))
        });
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
/// volumes. The copy goes through [`PART_SUFFIX`], so `to` only ever appears
/// complete and the partial copy never outlives the call; if deleting `from`
/// then fails, `from` is left alongside `to`.
pub fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    let mut part = to.as_os_str().to_owned();
    part.push(PART_SUFFIX);
    let part = PathBuf::from(part);
    // A stale one is from an earlier attempt that got cut off.
    let _ = remove(&part);
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_then_remove(from, to, &part)
}

fn copy_then_remove(from: &Path, to: &Path, part: &Path) -> io::Result<()> {
    let copied = if from.is_dir() {
        copy_dir(from, part)
    } else {
        fs::copy(from, part).map(drop)
    };
    if let Err(e) = copied.and_then(|()| fs::rename(part, to)) {
        let _ = remove(part);
        return Err(e);
    }
    remove(from)
}

fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
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
    let mut result = Ok(());
    if from.is_dir() && to.is_dir() {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            keep_first(
                &mut result,
                merge(&entry.path(), &to.join(entry.file_name())),
            );
        }
        let _ = fs::remove_dir(from);
    }
    result
}

/// Moves each profile in `from` to `to` whole, never mixing it into a folder
/// that's already there: a taken name is left where it is.
fn move_profiles(from: &Path, to: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(());
    };
    let mut result = fs::create_dir_all(to);
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        if target.exists() {
            continue;
        }
        let moved = move_path(&entry.path(), &target);
        if moved.is_err() && target.is_dir() {
            let _ = fs::write(target.join(MOVED_MARKER), "");
        }
        keep_first(&mut result, moved);
    }
    let _ = fs::remove_dir(from);
    result
}

fn keep_first(result: &mut io::Result<()>, step: io::Result<()>) {
    if result.is_ok() {
        *result = step;
    }
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
/// place under `to` once its folder has moved, so one still in `from` keeps
/// working there. A copy carrying [`MOVED_MARKER`] counts as moved, and the
/// original left beside it is deleted. `None` when nothing changed,
/// including for custom directories and a registry that doesn't parse.
fn rehome_installations(json: &str, from: &Path, to: &Path) -> Option<String> {
    let mut registry = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let mut changed = false;
    for install in registry.as_array_mut().into_iter().flatten() {
        if let Some(dir) = install.get_mut("directory")
            && let Some(old_dir) = dir.as_str().map(PathBuf::from)
            && let Ok(rest) = old_dir.strip_prefix(from)
        {
            let new_dir = to.join(rest);
            let marker = new_dir.join(MOVED_MARKER);
            let left_over = old_dir.exists();
            if new_dir.is_dir() && (!left_over || marker.is_file()) {
                if left_over && fs::remove_dir_all(&old_dir).is_ok() {
                    let _ = fs::remove_file(&marker);
                }
                *dir = new_dir.to_string_lossy().into_owned().into();
                changed = true;
            }
        }
    }
    changed.then(|| serde_json::to_string_pretty(&registry).ok())?
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

    /// A scratch base holding an old `.pomme` and the layout it moves into.
    fn old_and_new(name: &str) -> (PathBuf, PathBuf, Layout) {
        let base = scratch(name);
        let old = base.join(".pomme");
        let layout = Layout::new(base.join("Pomme"));
        (base, old, layout)
    }

    /// A scratch base with a `from` to move and the `to` it goes to.
    fn from_and_to(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = scratch(name);
        (base.clone(), base.join("from"), base.join("to"))
    }

    /// An old `installations.json` listing one installation at `dir`.
    fn write_registry(old: &Path, dir: &Path) {
        let registry = serde_json::json!([{ "directory": dir }]);
        write(&old.join("installations.json"), &registry.to_string());
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
        let (base, old, layout) = old_and_new("migrate");
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
        let (base, old, layout) = old_and_new("emptied");
        old_tree(&old);

        layout.migrate_from(&old).unwrap();

        assert!(!old.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_taken_destination_keeps_the_old_entry() {
        let (base, old, layout) = old_and_new("merge");
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

    fn registry_dirs(layout: &Layout) -> Vec<PathBuf> {
        let json = fs::read_to_string(layout.launcher().join("installations.json")).unwrap();
        let registry: serde_json::Value = serde_json::from_str(&json).unwrap();
        registry
            .as_array()
            .unwrap()
            .iter()
            .map(|install| PathBuf::from(install["directory"].as_str().unwrap()))
            .collect()
    }

    #[test]
    fn an_empty_launcher_registry_gives_way_to_the_real_one() {
        let (base, old, layout) = old_and_new("empty-registry");
        write(&old.join("installations/default/options.json"), "{}");
        write_registry(&old, &old.join("installations/default"));
        write(&layout.launcher().join("installations.json"), "[]");

        layout.migrate_from(&old).unwrap();

        assert_eq!(registry_dirs(&layout), [layout.default_profile()]);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_profile_that_cant_move_keeps_its_path_until_it_does() {
        let (base, old, layout) = old_and_new("stuck-profile");
        let stuck = old.join("installations/stuck");
        write(&stuck.join("options.json"), "{}");
        write_registry(&old, &stuck);
        // A file where the profile's folder would go blocks the move.
        let blocker = layout.profiles().join("stuck");
        write(&blocker, "");

        layout.migrate_from(&old).unwrap();
        assert_eq!(registry_dirs(&layout), std::slice::from_ref(&stuck));

        fs::remove_file(&blocker).unwrap();
        layout.migrate_from(&old).unwrap();
        assert_eq!(registry_dirs(&layout), [blocker]);
        assert!(!stuck.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_half_moved_profile_points_at_its_copy() {
        let (base, old, layout) = old_and_new("half-moved");
        // The copy finished, then deleting the original stopped partway.
        let left_over = old.join("installations/default");
        write(&left_over.join("options.json"), "{}");
        let copy = layout.default_profile();
        write(&copy.join("options.json"), "{}");
        write(&copy.join("saves/world/level.dat"), "world");
        write(&copy.join(MOVED_MARKER), "");
        write_registry(&old, &left_over);

        layout.migrate_from(&old).unwrap();

        assert_eq!(registry_dirs(&layout), std::slice::from_ref(&copy));
        assert!(!left_over.exists());
        assert!(!copy.join(MOVED_MARKER).exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_profile_written_separately_never_replaces_the_original() {
        let (base, old, layout) = old_and_new("separate-profile");
        let original = old.join("installations/default");
        write(&original.join("servers.json"), "[\"mine\"]");
        write(&original.join("saves/world/level.dat"), "world");
        // Same names, other contents, and no marker: not a copy of it.
        write(&layout.default_profile().join("servers.json"), "[]");
        write(&layout.default_profile().join("saves/world/level.dat"), "");
        write_registry(&old, &original);

        layout.migrate_from(&old).unwrap();

        assert_eq!(registry_dirs(&layout), std::slice::from_ref(&original));
        assert_eq!(
            fs::read_to_string(original.join("saves/world/level.dat")).unwrap(),
            "world"
        );
        assert_eq!(
            fs::read_to_string(original.join("servers.json")).unwrap(),
            "[\"mine\"]"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_copied_move_leaves_a_complete_copy() {
        let (base, from, to) = from_and_to("copied-move");
        let part = base.join(format!("to{PART_SUFFIX}"));
        write(&from.join("saves/world/level.dat"), "world");

        copy_then_remove(&from, &to, &part).unwrap();

        assert_eq!(
            fs::read_to_string(to.join("saves/world/level.dat")).unwrap(),
            "world"
        );
        assert!(!from.exists());
        assert!(!part.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_move_clears_a_cut_off_copy() {
        let (base, from, to) = from_and_to("cut-off-copy");
        write(&from.join("level.dat"), "world");
        // What a copy that ran out of space left behind.
        write(&base.join(format!("to{PART_SUFFIX}/level.dat")), "");

        move_path(&from, &to).unwrap();

        assert_eq!(fs::read_to_string(to.join("level.dat")).unwrap(), "world");
        assert!(!base.join(format!("to{PART_SUFFIX}")).exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_move_onto_a_taken_folder_fails_without_a_part() {
        let (base, from, to) = from_and_to("taken-folder");
        write(&from.join("level.dat"), "world");
        write(&to.join("level.dat"), "other");

        assert!(move_path(&from, &to).is_err());

        assert_eq!(fs::read_to_string(from.join("level.dat")).unwrap(), "world");
        assert_eq!(fs::read_to_string(to.join("level.dat")).unwrap(), "other");
        assert!(!base.join(format!("to{PART_SUFFIX}")).exists());
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
