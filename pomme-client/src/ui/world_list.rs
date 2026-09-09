//! Singleplayer saves on disk.
//!
//! Each save is a directory under `<game_dir>/saves` holding a sidecar file
//! that pomme owns. The embedded server keeps the seed, difficulty, game rules
//! and generation settings once a world exists; everything the world list
//! shows and the server has no field for lives here.
//!
//! Display name and directory name are separate, as in vanilla: renaming a
//! world rewrites the name and leaves its folder alone.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const SIDECAR: &str = "pomme_level.json";
const MAX_FILE_NAME: usize = 255;

/// Vanilla offers Hardcore as a third choice, but it is Survival plus a flag
/// rather than a mode of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameMode {
    Survival,
    Creative,
}

impl GameMode {
    /// Vanilla `gameMode.<name>`, as the world list's info line shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Survival => "Survival Mode",
            Self::Creative => "Creative Mode",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn cycle(self) -> Self {
        match self {
            Self::Peaceful => Self::Easy,
            Self::Easy => Self::Normal,
            Self::Normal => Self::Hard,
            Self::Hard => Self::Peaceful,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Peaceful => "Peaceful",
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Hard => "Hard",
        }
    }
}

/// Vanilla `WorldOptions.parseSeed`: a decimal long when the text parses as
/// one, otherwise Java's string hash. `None` means the caller should pick a
/// random seed.
#[allow(dead_code, reason = "the launch path consumes this")]
pub fn parse_seed(seed: &str) -> Option<i64> {
    let seed = seed.trim();
    if seed.is_empty() {
        return None;
    }
    Some(
        seed.parse::<i64>()
            .unwrap_or_else(|_| i64::from(java_string_hash(seed))),
    )
}

/// Java's `String.hashCode`, which runs over UTF-16 code units, so a character
/// outside the basic plane contributes two rounds rather than one.
#[allow(dead_code, reason = "the launch path consumes this")]
fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, unit| {
        h.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WorldSummary {
    pub name: String,
    /// The save directory's name, filled from the path on load rather than
    /// stored, so a moved or renamed directory stays self-describing.
    #[serde(skip)]
    pub folder: String,
    /// Epoch millis, zero until the world has been opened.
    pub last_played: u64,
    pub game_mode: GameMode,
    pub hardcore: bool,
    pub allow_commands: bool,
    pub difficulty: Difficulty,
    /// Kept exactly as typed; parsed when the world is created.
    pub seed: String,
    pub version: String,
    /// Fields other tools own, passed through untouched so a save from here
    /// doesn't strip them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct WorldList {
    pub worlds: Vec<WorldSummary>,
    saves_dir: PathBuf,
}

impl WorldList {
    /// Reads every save under `saves_dir`, most recently played first.
    /// Directories without a readable sidecar are skipped.
    pub fn scan(saves_dir: &Path) -> Self {
        let mut worlds: Vec<WorldSummary> = std::fs::read_dir(saves_dir)
            .into_iter()
            .flatten() // a missing saves directory is simply no worlds
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let folder = entry.file_name().to_str()?.to_owned();
                let json = std::fs::read_to_string(entry.path().join(SIDECAR)).ok()?;
                let mut summary: WorldSummary = serde_json::from_str(&json)
                    .inspect_err(|e| tracing::warn!("Skipping save {folder}: {e}"))
                    .ok()?;
                summary.folder = folder;
                Some(summary)
            })
            .collect();
        // Vanilla `LevelSummary::compareTo`.
        worlds.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.folder.cmp(&b.folder))
        });
        Self {
            worlds,
            saves_dir: saves_dir.to_path_buf(),
        }
    }

    pub fn get(&self, folder: &str) -> Option<&WorldSummary> {
        self.worlds.iter().find(|w| w.folder == folder)
    }

    /// A free directory name for `name`, deduplicated with a counter suffix.
    /// Vanilla `WorldCreationUiState::findResultFolder`.
    pub fn available_folder_name(&self, name: &str) -> String {
        let name = name.trim();
        let base = sanitize_name(if name.is_empty() { "New World" } else { name });
        let mut base = if is_reserved_name(&base) {
            format!("_{base}_")
        } else {
            base
        };

        // An existing " (N)" suffix continues from N rather than nesting.
        let mut count = 0u32;
        if let Some((stem, n)) = split_counter(&base) {
            base = stem;
            count = n;
        }
        truncate_to(&mut base, MAX_FILE_NAME);

        loop {
            let candidate = if count == 0 {
                base.clone()
            } else {
                let suffix = format!(" ({count})");
                let mut stem = base.clone();
                truncate_to(&mut stem, MAX_FILE_NAME - suffix.len());
                stem + &suffix
            };
            if !self.saves_dir.join(&candidate).exists() {
                return candidate;
            }
            count += 1;
        }
    }

    /// Writes a new save directory and its sidecar. `summary.folder` must
    /// already come from [`Self::available_folder_name`].
    pub fn create(&mut self, summary: WorldSummary) -> std::io::Result<PathBuf> {
        let dir = self.saves_dir.join(&summary.folder);
        std::fs::create_dir_all(&dir)?;
        self.save(&summary)?;
        self.worlds.insert(0, summary);
        Ok(dir)
    }

    /// Changes the display name only, leaving the directory alone.
    pub fn rename(&mut self, folder: &str, new_name: &str) -> std::io::Result<()> {
        self.update(folder, |w| w.name = new_name.trim().to_owned())
    }

    #[allow(dead_code, reason = "the launch path consumes this")]
    pub fn touch_last_played(&mut self, folder: &str) -> std::io::Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.update(folder, |w| w.last_played = now)
    }

    pub fn delete(&mut self, folder: &str) -> std::io::Result<()> {
        std::fs::remove_dir_all(self.saves_dir.join(folder))?;
        self.worlds.retain(|w| w.folder != folder);
        Ok(())
    }

    fn update(
        &mut self,
        folder: &str,
        edit: impl FnOnce(&mut WorldSummary),
    ) -> std::io::Result<()> {
        let Some(i) = self.worlds.iter().position(|w| w.folder == folder) else {
            return Ok(());
        };
        edit(&mut self.worlds[i]);
        self.save(&self.worlds[i])
    }

    fn save(&self, summary: &WorldSummary) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(summary)?;
        std::fs::write(self.saves_dir.join(&summary.folder).join(SIDECAR), json)
    }
}

/// Vanilla `SharedConstants.ILLEGAL_FILE_CHARACTERS`, plus the second pass
/// `FileUtil::sanitizeName` makes over `[./"]`.
const ILLEGAL: &[char] = &[
    '/', '\n', '\r', '\t', '\0', '\x0c', '`', '?', '*', '\\', '<', '>', '|', '"', ':', '.',
];

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if ILLEGAL.contains(&c) { '_' } else { c })
        .collect()
}

/// Names Windows refuses. Vanilla also rejects anything ending in a dot, but
/// sanitising has already turned every dot into an underscore by this point.
fn is_reserved_name(name: &str) -> bool {
    const RESERVED: &[&str] = &["COM", "CLOCK$", "CON", "PRN", "AUX", "NUL"];
    if RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)) {
        return true;
    }
    let Some((stem, digit)) = name.split_at_checked(3) else {
        return false;
    };
    matches!(digit.as_bytes(), [b'1'..=b'9'])
        && (stem.eq_ignore_ascii_case("COM") || stem.eq_ignore_ascii_case("LPT"))
}

fn split_counter(name: &str) -> Option<(String, u32)> {
    let stem = name.strip_suffix(')')?;
    let (stem, digits) = stem.rsplit_once(" (")?;
    Some((stem.to_owned(), digits.parse().ok()?))
}

/// Truncates on a character boundary, since a name can hold any UTF-8 the
/// sanitiser left alone.
fn truncate_to(name: &mut String, max: usize) {
    if name.len() <= max {
        return;
    }
    let mut end = max;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, folder: &str, last_played: u64) -> WorldSummary {
        WorldSummary {
            name: name.to_owned(),
            folder: folder.to_owned(),
            last_played,
            game_mode: GameMode::Survival,
            hardcore: false,
            allow_commands: false,
            difficulty: Difficulty::Normal,
            seed: String::new(),
            version: "26.2".to_owned(),
            extra: serde_json::Map::new(),
        }
    }

    /// A saves directory that cleans itself up, so a failing assert doesn't
    /// leave one behind.
    struct TempSaves(PathBuf);

    impl TempSaves {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pomme-worlds-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl std::ops::Deref for TempSaves {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempSaves {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_seeds_like_vanilla() {
        assert_eq!(parse_seed(""), None);
        assert_eq!(parse_seed("   "), None);
        assert_eq!(parse_seed(" 42 "), Some(42));
        assert_eq!(parse_seed("-5"), Some(-5));
        // Not a decimal long, so Java's string hash: 'a' * 31 + 'b'.
        assert_eq!(parse_seed("ab"), Some(3105));
        // A character outside the basic plane hashes as its two UTF-16 units.
        assert_eq!(parse_seed("\u{1D11E}"), Some(1_772_394));
        // Too large for an i64, so it hashes rather than saturating.
        assert_eq!(parse_seed("99999999999999999999"), Some(1_260_560_192));
    }

    #[test]
    fn sanitizes_illegal_characters() {
        for (input, expected) in [
            ("a/b", "a_b"),
            ("a.b", "a_b"),
            ("a:b*c?d", "a_b_c_d"),
            ("a\"b", "a_b"),
            ("plain name", "plain name"),
            ("café", "café"),
        ] {
            assert_eq!(sanitize_name(input), expected, "{input}");
        }
    }

    #[test]
    fn detects_reserved_windows_names() {
        for name in ["CON", "con", "NUL", "COM", "COM1", "lpt9", "CLOCK$", "aux"] {
            assert!(is_reserved_name(name), "{name}");
        }
        for name in ["CONS", "LPT", "COM0", "LPT10", "World"] {
            assert!(!is_reserved_name(name), "{name}");
        }
    }

    #[test]
    fn deduplicates_folder_names() {
        let dir = TempSaves::new();
        let list = WorldList::scan(&dir);

        assert_eq!(list.available_folder_name("New World"), "New World");
        std::fs::create_dir_all(dir.join("New World")).unwrap();
        assert_eq!(list.available_folder_name("New World"), "New World (1)");
        std::fs::create_dir_all(dir.join("New World (1)")).unwrap();
        assert_eq!(list.available_folder_name("New World"), "New World (2)");
        // An existing counter resumes rather than nesting.
        assert_eq!(list.available_folder_name("New World (1)"), "New World (2)");

        assert_eq!(list.available_folder_name("   "), "New World (2)");
        assert_eq!(list.available_folder_name("CON"), "_CON_");
        assert_eq!(list.available_folder_name("a/b"), "a_b");
    }

    #[test]
    fn round_trips_through_disk_preserving_unknown_fields() {
        let dir = TempSaves::new();
        let mut list = WorldList::scan(&dir);

        let mut world = summary("My World", "My World", 0);
        world
            .extra
            .insert("launcher_note".into(), serde_json::json!("keep me"));
        list.create(world).unwrap();

        let reloaded = WorldList::scan(&dir);
        let world = reloaded.get("My World").expect("world should load");
        assert_eq!(world.name, "My World");
        assert_eq!(world.folder, "My World");
        assert_eq!(world.extra["launcher_note"], serde_json::json!("keep me"));
    }

    #[test]
    fn rename_leaves_the_folder_alone() {
        let dir = TempSaves::new();
        let mut list = WorldList::scan(&dir);
        list.create(summary("Before", "Before", 0)).unwrap();

        list.rename("Before", "  After  ").unwrap();

        let reloaded = WorldList::scan(&dir);
        let world = reloaded.get("Before").expect("folder should be unchanged");
        assert_eq!(world.name, "After", "name is trimmed and rewritten");
    }

    #[test]
    fn sorts_by_last_played_then_folder() {
        let dir = TempSaves::new();
        let mut list = WorldList::scan(&dir);
        for (name, played) in [("b", 10), ("a", 10), ("recent", 99)] {
            list.create(summary(name, name, played)).unwrap();
        }

        let folders: Vec<_> = WorldList::scan(&dir)
            .worlds
            .iter()
            .map(|w| w.folder.clone())
            .collect();
        assert_eq!(folders, ["recent", "a", "b"]);
    }

    #[test]
    fn delete_removes_the_directory() {
        let dir = TempSaves::new();
        let mut list = WorldList::scan(&dir);
        list.create(summary("Doomed", "Doomed", 0)).unwrap();

        list.delete("Doomed").unwrap();

        assert!(!dir.join("Doomed").exists());
        assert!(WorldList::scan(&dir).worlds.is_empty());
    }
}
