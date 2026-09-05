use crate::types::Color;

#[derive(Default)]
pub struct GuiRenderState {
    pub before_blur_cmds: Vec<DrawCmd>,
    pub after_blur_cmds: Vec<DrawCmd>,
}

impl GuiRenderState {
    pub fn clear(&mut self) {
        self.before_blur_cmds.clear();
        self.after_blur_cmds.clear();
    }
}

pub enum DrawCmd {
    Rect(RectCmd),
}

pub struct RectCmd {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 4],
}

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub color: Color,
}
