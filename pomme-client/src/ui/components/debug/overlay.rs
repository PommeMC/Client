use std::path::{Path, PathBuf};

use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::TextWidthFn;
use crate::ui::components::debug::column::{DebugColumn, DebugColumnSide};
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entries::DebugEntryId;
use crate::ui::components::debug::entry::DebugEntryStatus;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;
use crate::ui::components::debug::profile::DebugScreenProfile;
use crate::ui::components::debug::registry::DebugEntries;

pub struct DebugScreenOverlay {
    is_overlay_visible: bool,
    profile: DebugScreenProfile,
    debug_profile_file: PathBuf,

    entries: DebugEntries,
    currently_enabled: Vec<DebugEntryId>,

    left_column: DebugColumn,
    right_column: DebugColumn,
}

impl DebugScreenOverlay {
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
            entries: DebugEntries::new(),
            currently_enabled: Vec::new(),
            left_column: DebugColumn::new(DebugColumnSide::Left),
            right_column: DebugColumn::new(DebugColumnSide::Right),
        };

        slf.save_profile();
        slf.rebuild_current_list();
        slf
    }

    pub fn toggle_overlay(&mut self) {
        self.is_overlay_visible = !self.is_overlay_visible;
        self.rebuild_current_list();
    }

    pub fn save_profile(&mut self) {
        match serde_json::to_string_pretty(&self.profile) {
            Ok(string) => {
                if let Err(error) = std::fs::write(&self.debug_profile_file, string) {
                    tracing::error!("Failed to write debug profile file: {error}");
                }
            }
            Err(error) => tracing::error!("Failed to serialize debug profile: {error}"),
        }
    }

    pub fn rebuild_current_list(&mut self) {
        let reduced_debug_info = false; // TODO(debug-overlay)

        self.currently_enabled = DebugEntryId::ALL
            .iter()
            .copied()
            .filter(|entry| {
                let status = self.profile.status(entry);

                let is_enabled = status == DebugEntryStatus::AlwaysOn
                    || (self.is_overlay_visible && status == DebugEntryStatus::InOverlay);

                is_enabled && self.entries.is_allowed(*entry, reduced_debug_info)
            })
            .collect();

        self.currently_enabled.sort_by_key(|id| id.name());
        // TODO(debug-overlay): DebugColumn caching
        // self.currently_enabled_version += 1;
    }

    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        gui_scale: f32,
        text_width: TextWidthFn,
        screen_width: f32,
        screen_height: f32,
        info: DebugInfo,
    ) {
        self.left_column.new_frame(gui_scale);
        self.right_column.new_frame(gui_scale);

        let mut displayer = DebugScreenDisplayer::new();
        for &id in &self.currently_enabled {
            self.entries.display(id, &mut displayer, &info);
        }

        // Vanilla moves MISC to the end.
        if let Some(misc) = displayer.groups.shift_remove(&DebugGroup::Misc) {
            displayer.groups.insert(DebugGroup::Misc, misc);
        }

        if !displayer.left_priority.lines.is_empty() {
            self.left_column.add(
                displayer.left_priority,
                elements,
                text_width,
                gui_scale,
                screen_width,
            );
        }
        if !displayer.right_priority.lines.is_empty() {
            self.right_column.add(
                displayer.right_priority,
                elements,
                text_width,
                gui_scale,
                screen_width,
            );
        }

        let mut groups = displayer.groups;
        // TODO(debug-overlay): add custom renderers
        groups.retain(|_, c| !c.lines.is_empty() || !c.facts.is_empty());

        // Groups stay in the column they were in last frame.
        let prev_left = self.left_column.previous_groups().to_vec();
        for group in prev_left {
            if !self.left_column.is_full(screen_height)
                && let Some(contents) = groups.shift_remove(&group)
            {
                self.left_column
                    .add(contents, elements, text_width, gui_scale, screen_width);
            }
        }
        let prev_right = self.right_column.previous_groups().to_vec();
        for group in prev_right {
            if !self.right_column.is_full(screen_height)
                && let Some(contents) = groups.shift_remove(&group)
            {
                self.right_column
                    .add(contents, elements, text_width, gui_scale, screen_width);
            }
        }

        // Groups with a preferred side.
        let keys: Vec<DebugGroup> = groups.keys().cloned().collect();
        for key in keys {
            match key.preferred_column() {
                Some(DebugColumnSide::Left) if !self.left_column.is_full(screen_height) => {
                    let contents = groups.shift_remove(&key).unwrap();
                    self.left_column
                        .add(contents, elements, text_width, gui_scale, screen_width);
                }
                Some(DebugColumnSide::Right) if !self.right_column.is_full(screen_height) => {
                    let contents = groups.shift_remove(&key).unwrap();
                    self.right_column
                        .add(contents, elements, text_width, gui_scale, screen_width);
                }
                _ => {}
            }
        }

        // Everything else goes to the shorter column and groups that don't fit are
        // dropped.
        for (_, contents) in groups {
            if self.left_column.height_so_far() < self.right_column.height_so_far()
                && !self.left_column.is_full(screen_height)
            {
                self.left_column
                    .add(contents, elements, text_width, gui_scale, screen_width);
            } else if !self.right_column.is_full(screen_height) {
                self.right_column
                    .add(contents, elements, text_width, gui_scale, screen_width);
            }
        }
    }
}
