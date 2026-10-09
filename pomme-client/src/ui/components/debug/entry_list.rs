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
        let debug_profile_file = game_dir.join("debug-profile.json");

        let profile = match std::fs::read_to_string(&debug_profile_file) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(profile) => profile,
                Err(error) => {
                    tracing::warn!("Failed to parse debug profile: {error}");
                    DebugScreenProfile::Default
                }
            },
            Err(error) => {
                tracing::warn!("Failed to read debug profile: {error}");
                DebugScreenProfile::Default
            }
        };

        let mut list = Self {
            is_overlay_visible: false,
            profile,
            debug_profile_file,
            currently_enabled: Vec::new(),
        };

        list.rebuild_current_list();
        list
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
