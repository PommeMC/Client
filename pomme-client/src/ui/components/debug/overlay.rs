use std::path::Path;

use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::TextWidthFn;
use crate::ui::components::debug::column::{DebugColumn, DebugColumnSide};
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entries::DebugInfo;
use crate::ui::components::debug::entry_list::DebugEntryList;
use crate::ui::components::debug::groups::DebugGroup;

pub struct DebugScreenOverlay {
    debug_entry_list: DebugEntryList,
    left_column: DebugColumn,
    right_column: DebugColumn,
}

impl DebugScreenOverlay {
    pub fn new(game_dir: &Path) -> Self {
        Self {
            debug_entry_list: DebugEntryList::new(game_dir),
            left_column: DebugColumn::new(DebugColumnSide::Left),
            right_column: DebugColumn::new(DebugColumnSide::Right),
        }
    }

    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        gui_scale: f32,
        text_width: TextWidthFn,
        screen_width: f32,
        screen_height: f32,
        info: &DebugInfo,
    ) {
        self.left_column.new_frame(gui_scale);
        self.right_column.new_frame(gui_scale);

        let mut displayer = DebugScreenDisplayer::new();
        for entry in self.debug_entry_list.get_currently_enabled() {
            entry.display(&mut displayer, info);
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
