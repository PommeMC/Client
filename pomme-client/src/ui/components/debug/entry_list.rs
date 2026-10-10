use std::path::{Path, PathBuf};

use crate::ui::components::debug::entries::{DebugEntryId, DebugEntryStatus};
use crate::ui::components::debug::profile::DebugScreenProfile;

pub struct DebugEntryList {
    is_overlay_visible: bool,
    profile: DebugScreenProfile,
    debug_profile_file: PathBuf,
    currently_enabled: Vec<DebugEntryId>,
}

impl DebugEntryList {
    pub fn new(game_dir: &Path) -> Self {
        let debug_profile_file = game_dir.join("debug_profile.json");

        let profile = match std::fs::read_to_string(&debug_profile_file) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(profile) => profile,
                Err(error) => {
                    tracing::error!("Failed to parse debug profile: {error}, resetting to default");
                    DebugScreenProfile::Default
                }
            },
            Err(error) => {
                tracing::error!("Failed to read debug profile: {error}, resetting to default");
                DebugScreenProfile::Default
            }
        };

        let mut slf = Self {
            is_overlay_visible: false,
            profile,
            debug_profile_file,
            currently_enabled: Vec::new(),
        };

        slf.save();
        slf.rebuild_current_list();
        slf
    }

    pub fn save(&mut self) {
        match serde_json::to_string_pretty(&self.profile) {
            Ok(string) => {
                if let Err(error) = std::fs::write(&self.debug_profile_file, string) {
                    tracing::error!("Failed to save debug profile file: {error}");
                }
            }
            Err(error) => tracing::error!("Failed to save debug profile file: {error}"),
        }
    }

    pub fn get_currently_enabled(&self) -> &[DebugEntryId] {
        &self.currently_enabled
    }

    pub fn rebuild_current_list(&mut self) {
        self.currently_enabled = DebugEntryId::ALL
            .iter()
            .copied()
            .filter(|entry| {
                let status = self.profile.status(entry);
                status == DebugEntryStatus::AlwaysOn
                    || (self.is_overlay_visible && status == DebugEntryStatus::InOverlay)
            })
            .collect();
        self.currently_enabled.sort_by_key(|id| id.name());
    }

    pub fn toggle_overlay(&mut self) {
        self.is_overlay_visible = !self.is_overlay_visible;
        self.rebuild_current_list();
    }
}
