use crate::position::BlockPos;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::common::{FONT_SIZE, WHITE, push_button, push_field_text, push_menu_background};
use crate::ui::text::TextSpan;
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

const MAX_SIGN_LINE_LENGTH: usize = 384;

pub fn line_inner_w(is_hanging: bool, gs: f32) -> f32 {
    if is_hanging { 60.0 * gs } else { 90.0 * gs }
}

pub struct SignEditState {
    pub pos: BlockPos,
    pub is_front_text: bool,
    pub is_hanging: bool,
    pub is_wall: bool,
    pub wood_index: u8,
    pub lines: [TextFieldState; 4],
    pub current_line: usize,
    inner_w: f32,
    pub text_color: [f32; 4],
}

impl SignEditState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pos: BlockPos,
        is_front_text: bool,
        is_hanging: bool,
        is_wall: bool,
        wood_index: u8,
        initial_lines: [String; 4],
        text_color: [f32; 4],
        gs: f32,
    ) -> Self {
        let inner_w = line_inner_w(is_hanging, gs);
        let mut lines = [
            TextFieldState::new(MAX_SIGN_LINE_LENGTH),
            TextFieldState::new(MAX_SIGN_LINE_LENGTH),
            TextFieldState::new(MAX_SIGN_LINE_LENGTH),
            TextFieldState::new(MAX_SIGN_LINE_LENGTH),
        ];
        for (i, text) in initial_lines.into_iter().enumerate() {
            lines[i].set_value(&text, inner_w, &|_| 0.0);
        }
        lines[0].set_focused(true);
        Self {
            pos,
            is_front_text,
            is_hanging,
            is_wall,
            wood_index,
            lines,
            current_line: 0,
            inner_w,
            text_color,
        }
    }

    pub fn set_current_line(&mut self, next: usize) {
        self.lines[self.current_line].set_focused(false);
        self.current_line = next % 4;
        self.lines[self.current_line].set_focused(true);
    }

    pub fn handle_text_input(
        &mut self,
        events: &[TextInputEvent],
        width_fn: &dyn Fn(&str) -> f32,
    ) -> bool {
        let inner_w = self.inner_w;
        let mut clipboard = SystemClipboard;
        let mut finish = false;
        for ev in events {
            match ev {
                TextInputEvent::Key { code, mods } => match code {
                    winit::keyboard::KeyCode::ArrowUp => {
                        self.set_current_line((self.current_line + 3) % 4);
                    }
                    winit::keyboard::KeyCode::ArrowDown => {
                        self.set_current_line((self.current_line + 1) % 4);
                    }
                    winit::keyboard::KeyCode::Tab => {
                        if mods.shift {
                            self.set_current_line((self.current_line + 3) % 4);
                        } else {
                            self.set_current_line((self.current_line + 1) % 4);
                        }
                    }
                    winit::keyboard::KeyCode::Enter | winit::keyboard::KeyCode::NumpadEnter => {
                        if self.current_line < 3 {
                            self.set_current_line(self.current_line + 1);
                        } else {
                            finish = true;
                        }
                    }
                    winit::keyboard::KeyCode::Backspace
                        if self.lines[self.current_line].value().is_empty()
                            && self.current_line > 0 =>
                    {
                        self.set_current_line(self.current_line - 1);
                    }
                    _ => {
                        self.lines[self.current_line].key_pressed(
                            *code,
                            mods,
                            &mut clipboard,
                            inner_w,
                            width_fn,
                        );
                    }
                },
                TextInputEvent::Char(c) => {
                    self.lines[self.current_line].char_typed(*c, inner_w, width_fn);
                }
            }
        }
        finish
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> bool {
        push_menu_background(elements, screen_w, screen_h);

        let title_text = if self.is_hanging {
            "Edit Hanging Sign Message"
        } else {
            "Edit Sign Message"
        };
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: 40.0 * gs,
            text: title_text.into(),
            scale: FONT_SIZE * gs,
            color: WHITE,
            centered: true,
        });

        let (
            sign_y_offset,
            sprite,
            sprite_w,
            sprite_h,
            sprite_x,
            sprite_y,
            src_rect,
            text_scale,
            inner_w,
        ) = if self.is_hanging {
            let w = 72.0 * gs;
            let h = 72.0 * gs;
            (
                125.0 * gs,
                SpriteId::HangingSign(self.wood_index),
                w,
                h,
                (screen_w - w) / 2.0,
                76.0 * gs,
                [0.0, 0.0, 1.0, 1.0],
                1.0,
                60.0 * gs,
            )
        } else {
            let w = 93.6 * gs;
            let (h, src_rect) = if self.is_wall {
                (46.8 * gs, [0.0, 0.0, 1.0, 12.0 / 26.0])
            } else {
                (101.4 * gs, [0.0, 0.0, 1.0, 1.0])
            };
            (
                90.0 * gs,
                SpriteId::Sign(self.wood_index),
                w,
                h,
                (screen_w - w) / 2.0,
                66.3 * gs,
                src_rect,
                0.9765628,
                90.0 * gs,
            )
        };

        elements.push(MenuElement::SubImage {
            x: sprite_x,
            y: sprite_y,
            w: sprite_w,
            h: sprite_h,
            sprite,
            src_rect,
            tint: [1.0, 1.0, 1.0, 1.0],
        });

        let fs = FONT_SIZE * text_scale * gs;
        let line_h = 10.0 * text_scale * gs;
        let line_hit_x = screen_w / 2.0 - inner_w / 2.0;
        let line_hit_w = inner_w;

        for i in 0..4 {
            let line_y = sign_y_offset + (i as f32 * 10.0 - 20.0) * text_scale * gs;
            let text_w = width_fn(self.lines[i].value());
            let text_x = screen_w / 2.0 - text_w / 2.0;

            if clicked
                && cursor.0 >= line_hit_x
                && cursor.0 <= line_hit_x + line_hit_w
                && cursor.1 >= line_y
                && cursor.1 < line_y + line_h
            {
                self.set_current_line(i);
                let rel_x = cursor.0 - text_x;
                let click_pos = self.lines[i].pos_from_click(rel_x, inner_w, width_fn);
                self.lines[i].move_cursor_to(click_pos, false, inner_w, width_fn);
            }

            let val = self.lines[i].value().to_string();
            if i == self.current_line {
                let info = self.lines[i].render_info(inner_w, true, width_fn);
                let shown = &self.lines[i].value()[info.display_start..info.display_end];
                let spans = [TextSpan::new(shown.to_string(), self.text_color)];

                elements.push(MenuElement::ScissorPush {
                    x: line_hit_x,
                    y: line_y,
                    w: line_hit_w,
                    h: line_h,
                });
                push_field_text(
                    elements,
                    &info,
                    shown,
                    Some(&spans),
                    text_x,
                    line_y,
                    fs,
                    gs,
                    gs,
                    self.text_color,
                    None,
                    width_fn,
                );
                elements.push(MenuElement::ScissorPop);
            } else if !val.is_empty() {
                elements.push(MenuElement::ScissorPush {
                    x: line_hit_x,
                    y: line_y,
                    w: line_hit_w,
                    h: line_h,
                });
                elements.push(MenuElement::TextFlat {
                    x: text_x,
                    y: line_y,
                    text: val,
                    scale: fs,
                    color: self.text_color,
                });
                elements.push(MenuElement::ScissorPop);
            }
        }

        let btn_w = 200.0 * gs;
        let btn_h = 20.0 * gs;
        let btn_x = (screen_w - btn_w) / 2.0;
        let btn_y = screen_h / 4.0 + 144.0 * gs;
        let done_hovered = push_button(
            elements,
            cursor,
            btn_x,
            btn_y,
            btn_w,
            btn_h,
            gs,
            FONT_SIZE * gs,
            "Done",
            true,
        );
        if clicked && done_hovered {
            return true;
        }
        false
    }

    pub fn lines_as_strings(&self) -> [String; 4] {
        [
            self.lines[0].value().to_string(),
            self.lines[1].value().to_string(),
            self.lines[2].value().to_string(),
            self.lines[3].value().to_string(),
        ]
    }
}
