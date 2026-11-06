//! Vanilla's configurable debug screen: each entry is always shown, shown
//! with the F3 overlay, or never (`DebugScreenEntryList`), set on the Debug
//! Options screen and saved to `debug-profile.json`.
//!
//! TODO: only the FPS entry is an entry so far; the overlay's other lines
//! (`hud::build_debug_overlay`) move onto entries one at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::rhi::PresentMode;
use crate::ui::common::{self, TextWidthFn};
use crate::ui::menu::MAX_FRAMERATE_UNLIMITED;
use crate::ui::text::TextSpan;

/// Vanilla `DebugScreenEntryStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DebugEntryStatus {
    AlwaysOn,
    InOverlay,
    Never,
}

/// Vanilla `DebugScreenProfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DebugProfile {
    Default,
    Performance,
}

impl DebugProfile {
    pub const ALL: [Self; 2] = [Self::Default, Self::Performance];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default profile",
            Self::Performance => "Performance profile",
        }
    }

    /// `DebugScreenEntries.PROFILES`, ids Pomme has no entry for yet included
    /// so a saved profile keeps them.
    fn statuses(self) -> HashMap<String, DebugEntryStatus> {
        use DebugEntryStatus::{AlwaysOn, InOverlay};
        let pairs: &[(&str, DebugEntryStatus)] = match self {
            Self::Default => &[
                ("minecraft:3d_crosshair", InOverlay),
                ("minecraft:game_version", InOverlay),
                ("minecraft:tps", InOverlay),
                (FPS.id, InOverlay),
                ("minecraft:memory", InOverlay),
                ("minecraft:system_specs", InOverlay),
                ("minecraft:player_position", InOverlay),
                ("minecraft:player_section_position", InOverlay),
                ("minecraft:simple_performance_impactors", InOverlay),
            ],
            Self::Performance => &[
                ("minecraft:tps", InOverlay),
                (FPS.id, AlwaysOn),
                ("minecraft:gpu_utilization", InOverlay),
                ("minecraft:memory", InOverlay),
                ("minecraft:simple_performance_impactors", InOverlay),
            ],
        };
        pairs
            .iter()
            .map(|&(id, status)| (id.to_owned(), status))
            .collect()
    }
}

/// Vanilla `DebugEntryCategory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DebugEntryCategory {
    ScreenText,
}

impl DebugEntryCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::ScreenText => "Debug Screen Text",
        }
    }
}

/// A registered entry (`DebugScreenEntries.register`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugEntry {
    pub id: &'static str,
    pub category: DebugEntryCategory,
    /// `DebugScreenEntry.isAllowed`: shown with reduced debug info too.
    allowed_when_reduced: bool,
}

impl DebugEntry {
    pub fn path(&self) -> &'static str {
        self.id.split_once(':').map_or(self.id, |(_, path)| path)
    }

    pub fn is_allowed(&self, reduced_debug_info: bool) -> bool {
        self.allowed_when_reduced || !reduced_debug_info
    }
}

/// `DebugEntryFps`.
pub const FPS: DebugEntry = DebugEntry {
    id: "minecraft:fps",
    category: DebugEntryCategory::ScreenText,
    allowed_when_reduced: true,
};

/// Pomme-only: the frame rate of the slowest 1% and 0.1% of frames.
pub const FRAME_TIME_LOWS: DebugEntry = DebugEntry {
    id: "pomme:frame_time_lows",
    category: DebugEntryCategory::ScreenText,
    allowed_when_reduced: true,
};

pub const ENTRIES: [DebugEntry; 2] = [FPS, FRAME_TIME_LOWS];

fn entry(id: &str) -> Option<DebugEntry> {
    ENTRIES.into_iter().find(|entry| entry.id == id)
}

/// `Identifier.compareTo`: by path, then namespace.
fn identifier_order(id: &str) -> (&str, &str) {
    id.split_once(':')
        .map_or((id, "minecraft"), |(namespace, path)| (path, namespace))
}

/// The Debug Options list (`OptionList.updateSearch`): the entries whose path
/// contains `search`, by category and then identifier.
pub fn options_list(search: &str) -> Vec<DebugEntry> {
    let mut entries: Vec<DebugEntry> = ENTRIES
        .into_iter()
        .filter(|entry| entry.path().contains(search))
        .collect();
    entries.sort_by_key(|entry| (entry.category, identifier_order(entry.id)));
    entries
}

/// `DebugScreenEntryList.SerializedOptions`: a profile, or custom statuses.
#[derive(Serialize, Deserialize)]
struct SerializedOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<DebugProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom: Option<HashMap<String, DebugEntryStatus>>,
    /// `DataFixTypes.wrapCodec`'s stamp.
    #[serde(rename = "DataVersion", skip_deserializing)]
    data_version: i32,
}

/// 26.3's data version (`world_version` in its `version.json`).
const DATA_VERSION: i32 = 5023;

/// Vanilla `DebugScreenEntryList`, without the overlay's visibility, which
/// Pomme keeps as `GameState::show_debug` and passes in.
pub struct DebugEntries {
    statuses: HashMap<String, DebugEntryStatus>,
    profile: Option<DebugProfile>,
    path: PathBuf,
}

impl DebugEntries {
    /// `load`: the saved profile or statuses, or the default profile, saved
    /// over a file it can't read.
    pub fn load(game_dir: &Path) -> Self {
        let mut entries = Self {
            statuses: HashMap::new(),
            profile: None,
            path: game_dir.join("debug-profile.json"),
        };
        if !entries.path.is_file() {
            entries.reset_to_profile(DebugProfile::Default);
            return entries;
        }
        let saved = std::fs::read_to_string(&entries.path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                serde_json::from_str::<SerializedOptions>(&text).map_err(|e| e.to_string())
            });
        match saved {
            Ok(SerializedOptions {
                profile: Some(profile),
                ..
            }) => entries.reset_to_profile(profile),
            Ok(SerializedOptions { custom, .. }) => entries.statuses = custom.unwrap_or_default(),
            Err(e) => {
                tracing::error!(
                    "Couldn't read debug profile file {}, resetting to default: {e}",
                    entries.path.display()
                );
                entries.reset_to_profile(DebugProfile::Default);
                entries.save();
            }
        }
        entries
    }

    fn reset_to_profile(&mut self, profile: DebugProfile) {
        self.profile = Some(profile);
        self.statuses = profile.statuses();
    }

    pub fn load_profile(&mut self, profile: DebugProfile) {
        self.reset_to_profile(profile);
        self.save();
    }

    pub fn is_using_profile(&self, profile: DebugProfile) -> bool {
        self.profile == Some(profile)
    }

    pub fn status(&self, id: &str) -> DebugEntryStatus {
        self.statuses
            .get(id)
            .copied()
            .unwrap_or(DebugEntryStatus::Never)
    }

    pub fn set_status(&mut self, id: &str, status: DebugEntryStatus) {
        self.profile = None;
        self.statuses.insert(id.to_owned(), status);
        self.save();
    }

    /// `rebuildCurrentList`: the entries showing now, in identifier order.
    pub fn currently_enabled(
        &self,
        overlay_visible: bool,
        reduced_debug_info: bool,
    ) -> Vec<DebugEntry> {
        let mut enabled: Vec<DebugEntry> = self
            .statuses
            .iter()
            .filter(|&(_, &status)| {
                status == DebugEntryStatus::AlwaysOn
                    || overlay_visible && status == DebugEntryStatus::InOverlay
            })
            .filter_map(|(id, _)| entry(id))
            .filter(|entry| entry.is_allowed(reduced_debug_info))
            .collect();
        enabled.sort_by_key(|entry| identifier_order(entry.id));
        enabled
    }

    fn save(&self) {
        let serialized = SerializedOptions {
            profile: self.profile,
            custom: self.profile.is_none().then(|| self.statuses.clone()),
            data_version: DATA_VERSION,
        };
        let written = serde_json::to_vec(&serialized)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::util::write_atomic(&self.path, &bytes));
        if let Err(e) = written {
            tracing::error!(
                "Failed to save debug profile file {}: {e}",
                self.path.display()
            );
        }
    }
}

/// What the entries read.
pub struct DebugEntryInput {
    pub fps: u32,
    /// The Max Framerate option.
    pub framerate_limit: u32,
    pub present_mode: Option<PresentMode>,
    pub refresh_rate_hz: Option<f32>,
    /// The frame rate of the slowest 1% and 0.1% of recent frames.
    pub lows: (u32, u32),
}

/// Vanilla `DebugScreenDisplayer`'s left and right columns.
#[derive(Default)]
pub struct DebugColumns {
    pub left: Vec<String>,
    pub right: Vec<String>,
    regular: Vec<String>,
}

impl DebugColumns {
    /// `addPriorityLine`: to the shorter column, the left on a tie.
    fn add_priority_line(&mut self, line: String) {
        if self.left.len() > self.right.len() {
            self.right.push(line);
        } else {
            self.left.push(line);
        }
    }

    fn add_line(&mut self, line: String) {
        self.regular.push(line);
    }

    /// `DebugScreenOverlay.extractRenderState`'s column assembly: priority
    /// lines, a gap, then the regular lines split down the middle.
    fn finish(mut self) -> Self {
        if !self.left.is_empty() {
            self.left.push(String::new());
        }
        if !self.right.is_empty() {
            self.right.push(String::new());
        }
        if !self.regular.is_empty() {
            let mid = self.regular.len().div_ceil(2);
            let right = self.regular.split_off(mid);
            let split = !right.is_empty();
            self.left.append(&mut self.regular);
            self.right.extend(right);
            self.left.push(String::new());
            if split {
                self.right.push(String::new());
            }
        }
        self
    }
}

/// The lines `entries` show, laid out in columns.
pub fn entry_columns(entries: &[DebugEntry], input: &DebugEntryInput) -> DebugColumns {
    let mut columns = DebugColumns::default();
    for entry in entries {
        match entry.id {
            id if id == FPS.id => columns.add_priority_line(fps_line(input)),
            id if id == FRAME_TIME_LOWS.id => columns.add_line(format!(
                "Lows: 1% {} fps, 0.1% {} fps",
                input.lows.0, input.lows.1
            )),
            _ => {}
        }
    }
    columns.finish()
}

/// `DebugEntryFps.display`.
fn fps_line(input: &DebugEntryInput) -> String {
    let limit = if input.framerate_limit == MAX_FRAMERATE_UNLIMITED {
        "inf".to_owned()
    } else {
        input.framerate_limit.to_string()
    };
    let present = match input.present_mode {
        Some(PresentMode::Immediate) => " (immediate)",
        Some(PresentMode::Mailbox) => " (mailbox)",
        Some(PresentMode::Fifo) => " (fifo)",
        Some(PresentMode::FifoRelaxed) => " (fifo relaxed)",
        None => "",
    };
    // `VideoMode.refreshRateLabel`.
    let hz = match input.refresh_rate_hz {
        None => "0".to_owned(),
        Some(rate) if rate == rate.round() => (rate as i32).to_string(),
        Some(rate) => format!("{rate:.2}"),
    };
    format!("{} fps T: {limit}{present} @{hz}Hz", input.fps)
}

/// `DebugScreenOverlay.extractLines` for both columns: each line on a
/// translucent backdrop, from the top-left or the top-right corner.
pub fn push_debug_columns(
    elements: &mut Vec<MenuElement>,
    columns: &DebugColumns,
    screen_w: f32,
    gs: f32,
    text_width_fn: TextWidthFn,
) {
    const BACKDROP: [f32; 4] = [80.0 / 255.0, 80.0 / 255.0, 80.0 / 255.0, 144.0 / 255.0];
    let text = common::rgb(0xE0E0E0);
    let fs = common::FONT_SIZE * gs;
    let line_h = common::LINE_HEIGHT as f32 * gs;
    for (lines, align_left) in [(&columns.left, true), (&columns.right, false)] {
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let width = text_width_fn(line, fs);
            let left = if align_left {
                2.0 * gs
            } else {
                screen_w - 2.0 * gs - width
            };
            let top = 2.0 * gs + line_h * i as f32;
            elements.push(MenuElement::Rect {
                x: left - gs,
                y: top - gs,
                w: width + 2.0 * gs,
                h: line_h,
                corner_radius: 0.0,
                color: BACKDROP,
            });
            elements.push(MenuElement::McText {
                x: left,
                y: top,
                spans: vec![TextSpan::new(line.clone(), text)],
                scale: fs,
                centered: false,
                shadow: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DebugEntryInput {
        DebugEntryInput {
            fps: 144,
            framerate_limit: MAX_FRAMERATE_UNLIMITED,
            present_mode: Some(PresentMode::Fifo),
            refresh_rate_hz: Some(59.94),
            lows: (90, 60),
        }
    }

    #[test]
    fn the_performance_profile_shows_fps_without_the_overlay() {
        let mut list = DebugEntries {
            statuses: HashMap::new(),
            profile: None,
            path: PathBuf::new(),
        };
        list.reset_to_profile(DebugProfile::Performance);
        assert_eq!(list.currently_enabled(false, false), [FPS]);
        list.reset_to_profile(DebugProfile::Default);
        assert!(list.currently_enabled(false, false).is_empty());
        assert_eq!(list.currently_enabled(true, false), [FPS]);
    }

    #[test]
    fn setting_a_status_leaves_the_profile_and_saves() {
        let dir = crate::test_util::test_temp_dir("debug_entries");
        std::fs::create_dir_all(&dir).unwrap();
        let mut list = DebugEntries::load(&dir);
        assert!(list.is_using_profile(DebugProfile::Default));
        list.set_status(FRAME_TIME_LOWS.id, DebugEntryStatus::AlwaysOn);
        assert!(!list.is_using_profile(DebugProfile::Default));
        assert_eq!(list.currently_enabled(false, false), [FRAME_TIME_LOWS]);
        assert_eq!(list.currently_enabled(true, false), [FPS, FRAME_TIME_LOWS]);

        let reloaded = DebugEntries::load(&dir);
        assert_eq!(
            reloaded.status(FRAME_TIME_LOWS.id),
            DebugEntryStatus::AlwaysOn
        );
        assert_eq!(reloaded.status(FPS.id), DebugEntryStatus::InOverlay);
        list.load_profile(DebugProfile::Performance);
        assert!(DebugEntries::load(&dir).is_using_profile(DebugProfile::Performance));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_file_resets_to_the_default_profile() {
        let dir = crate::test_util::test_temp_dir("debug_entries_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("debug-profile.json");
        std::fs::write(&path, "{").unwrap();
        assert!(DebugEntries::load(&dir).is_using_profile(DebugProfile::Default));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"profile":"default","DataVersion":5023}"#
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_profile_saves_by_name_and_custom_statuses_by_entry() {
        let profile = SerializedOptions {
            profile: Some(DebugProfile::Performance),
            custom: None,
            data_version: DATA_VERSION,
        };
        assert_eq!(
            serde_json::to_string(&profile).unwrap(),
            r#"{"profile":"performance","DataVersion":5023}"#
        );
        let custom: SerializedOptions =
            serde_json::from_str(r#"{"custom":{"minecraft:fps":"alwaysOn"}}"#).unwrap();
        assert_eq!(
            custom.custom.unwrap()["minecraft:fps"],
            DebugEntryStatus::AlwaysOn
        );
    }

    #[test]
    fn the_fps_line_reads_like_the_reference() {
        let columns = entry_columns(&[FPS], &input());
        assert_eq!(columns.left, ["144 fps T: inf (fifo) @59.94Hz", ""]);
        let capped = DebugEntryInput {
            framerate_limit: 120,
            present_mode: None,
            refresh_rate_hz: Some(60.0),
            ..input()
        };
        assert_eq!(
            entry_columns(&[FPS], &capped).left[0],
            "144 fps T: 120 @60Hz"
        );
    }

    #[test]
    fn the_options_list_filters_by_path() {
        assert_eq!(options_list(""), [FPS, FRAME_TIME_LOWS]);
        assert_eq!(options_list("low"), [FRAME_TIME_LOWS]);
        assert!(options_list("minecraft").is_empty());
    }

    #[test]
    fn regular_lines_follow_the_priority_lines() {
        let columns = entry_columns(&[FPS, FRAME_TIME_LOWS], &input());
        assert_eq!(
            columns.left,
            [
                "144 fps T: inf (fifo) @59.94Hz",
                "",
                "Lows: 1% 90 fps, 0.1% 60 fps",
                ""
            ]
        );
        assert!(columns.right.is_empty());
    }
}
