use crate::graphics::state::{DrawCmd, GuiRenderState, Rect, RectCmd};
use crate::types::{Position, Size};

pub struct GuiGraphicsExtractor<'a> {
    render_state: &'a mut GuiRenderState,
    blur_applied: bool,
    gui_scale: i32,

    pub mouse_pos: Position,
    pub mouse_x: i32,
    pub mouse_y: i32,
    pub screen_size: Size,
    pub screen_width: i32,
    pub screen_height: i32,
}

impl<'a> GuiGraphicsExtractor<'a> {
    pub fn new(
        render_state: &'a mut GuiRenderState,
        gui_scale: i32,
        mouse_pos: Position,
        screen_size: Size,
    ) -> Self {
        Self {
            render_state,
            blur_applied: false,
            gui_scale,

            mouse_pos,
            mouse_x: mouse_pos.x,
            mouse_y: mouse_pos.y,
            screen_size,
            screen_width: screen_size.width,
            screen_height: screen_size.height,
        }
    }

    /// Blur may only be applied once
    pub const fn push_blur(&mut self) {
        assert!(!self.blur_applied);
        self.blur_applied = true;
    }

    pub fn push_cmd(&mut self, cmd: DrawCmd) {
        if !self.blur_applied {
            self.render_state.before_blur_cmds.push(cmd);
        } else {
            self.render_state.after_blur_cmds.push(cmd);
        }
    }

    pub fn push_rect(&mut self, rect: Rect) {
        self.push_cmd(DrawCmd::Rect(RectCmd {
            x: (rect.x * self.gui_scale) as f32,
            y: (rect.y * self.gui_scale) as f32,
            width: (rect.width * self.gui_scale) as f32,
            height: (rect.height * self.gui_scale) as f32,
            color: rect.color.into(),
        }));
    }
}
