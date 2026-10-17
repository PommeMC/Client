use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::TextWidthFn;
use crate::ui::components::debug::groups::{DebugGroup, DebugGroupContents};

pub struct DebugColumn {
    side: DebugColumnSide,
    groups: Vec<DebugGroup>,
    previous_groups: Vec<DebugGroup>,
    height_so_far: f32,
}

impl DebugColumn {
    const TOP_PADDING: f32 = 2.0;

    pub fn new(side: DebugColumnSide) -> Self {
        Self {
            side,
            groups: Vec::new(),
            previous_groups: Vec::new(),
            height_so_far: 0.0,
        }
    }

    pub fn new_frame(&mut self, gui_scale: f32) {
        self.previous_groups = std::mem::take(&mut self.groups);
        self.height_so_far = Self::TOP_PADDING * gui_scale;
    }

    pub fn is_full(&self, screen_height: f32) -> bool {
        self.height_so_far > screen_height
    }

    pub fn height_so_far(&self) -> f32 {
        self.height_so_far
    }

    pub fn previous_groups(&self) -> &[DebugGroup] {
        &self.previous_groups
    }

    pub fn add(
        &mut self,
        contents: DebugGroupContents,
        elements: &mut Vec<MenuElement>,
        text_width: TextWidthFn,
        gui_scale: f32,
        screen_width: f32,
    ) {
        let rect = contents.extract(
            elements,
            text_width,
            self.side,
            gui_scale,
            screen_width,
            self.height_so_far,
        );
        self.height_so_far += rect[3] + 9.0 * gui_scale;
        self.groups.push(contents.group);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum DebugColumnSide {
    Left,
    Right,
}
