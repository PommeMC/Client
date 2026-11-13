//! Vanilla `AbstractSignEditScreen`, with `SignEditScreen`'s and
//! `HangingSignEditScreen`'s backgrounds.

use std::time::Instant;

use winit::keyboard::KeyCode;

use crate::position::BlockPos;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::common::{
    FIELD_SELECTION, FONT_SIZE, WHITE, gui_centered, gui_float_half_width, gui_half_width,
    gui_quarter_height, push_button, rgb,
};
use crate::ui::container::push_transparent_background;
use crate::ui::text_edit::{SystemClipboard, TextFieldHelper, TextInputEvent, caret_visible};
use crate::world::block_entity::{SignBlock, SignText};

pub struct SignEditState {
    pub pos: BlockPos,
    pub is_front_text: bool,
    block: SignBlock,
    pub messages: [String; 4],
    line: usize,
    field: TextFieldHelper,
    text_color: [f32; 4],
    cursor_blink_start: Instant,
}

impl SignEditState {
    pub fn new(pos: BlockPos, is_front_text: bool, block: SignBlock, text: &SignText) -> Self {
        let messages = std::array::from_fn(|i| text.messages[i].plain_text());
        let mut field = TextFieldHelper::default();
        field.set_cursor_to_end(&messages[0]);
        Self {
            pos,
            is_front_text,
            block,
            messages,
            line: 0,
            field,
            text_color: rgb(text.ink_color()),
            cursor_blink_start: Instant::now(),
        }
    }

    /// `keyPressed` and `charTyped`; `width` is `Font.width` in GUI units.
    /// True when a line changed.
    pub fn handle_text_input(
        &mut self,
        events: &[TextInputEvent],
        width: &dyn Fn(&str) -> f32,
    ) -> bool {
        let max_width = self.block.max_text_line_width();
        let valid = |s: &str| width(s) <= max_width;
        let mut changed = false;
        for event in events {
            let line = self.line;
            let message = &mut self.messages[line];
            let original = message.clone();
            match event {
                TextInputEvent::Key {
                    code: KeyCode::ArrowUp,
                    ..
                } => self.set_line((self.line + 3) & 3),
                TextInputEvent::Key {
                    code: KeyCode::ArrowDown | KeyCode::Enter | KeyCode::NumpadEnter,
                    ..
                } => self.set_line((self.line + 1) & 3),
                TextInputEvent::Key { code, mods } => {
                    self.field
                        .key_pressed(message, *code, mods, &mut SystemClipboard, &valid);
                }
                TextInputEvent::Char(c) => self.field.char_typed(message, *c, &valid),
            }
            changed |= self.messages[line] != original;
        }
        changed
    }

    fn set_line(&mut self, line: usize) {
        self.line = line;
        self.field.set_cursor_to_end(&self.messages[line]);
    }

    /// Draws the screen; true when Done was clicked. `width` is `Font.width`
    /// in GUI units.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
        width: &dyn Fn(&str) -> f32,
    ) -> bool {
        push_transparent_background(elements, screen_w, screen_h);
        let title = if self.block.hanging {
            "hanging_sign.edit"
        } else {
            "sign.edit"
        };
        elements.push(MenuElement::Text {
            x: gui_half_width(screen_w, gs),
            y: 40.0 * gs,
            text: crate::lang::translate(title).unwrap_or(title).into(),
            scale: FONT_SIZE * gs,
            color: WHITE,
            centered: true,
        });

        // `extractSign`: everything is placed from the middle of the screen at
        // the sign's y offset.
        let (y_offset, text_scale) = if self.block.hanging {
            (125.0, 1.0)
        } else {
            (90.0, 0.9765628)
        };
        let origin = (gui_float_half_width(screen_w, gs), y_offset * gs);
        self.push_background(elements, origin, gs);

        // `extractSignText`, in text units scaled by `getSignTextScale`.
        let unit = text_scale * gs;
        let at = |x: f32, y: f32| (origin.0 + x * unit, origin.1 + y * unit);
        let line_height = self.block.text_line_height();
        let midpoint = 4.0 * line_height / 2.0;
        let cursor_y = self.line as f32 * line_height - midpoint;
        let cursor_bottom = cursor_y + line_height;
        let show_cursor = caret_visible(self.cursor_blink_start);
        let text = |elements: &mut Vec<MenuElement>, s: &str, x: f32, y: f32| {
            let (x, y) = at(x, y);
            elements.push(MenuElement::TextFlat {
                x,
                y,
                text: s.into(),
                scale: FONT_SIZE * unit,
                color: self.text_color,
            });
        };
        let rect = |elements: &mut Vec<MenuElement>, x0: f32, y0: f32, x1: f32, y1: f32, color| {
            let (x, y) = at(x0, y0);
            elements.push(MenuElement::Rect {
                x,
                y,
                w: (x1 - x0) * unit,
                h: (y1 - y0) * unit,
                corner_radius: 0.0,
                color,
            });
        };
        for (i, line) in self.messages.iter().enumerate() {
            let half = (width(line) as i32 / 2) as f32;
            let x_at = |pos: usize| width(&line[..pos]) - half;
            if i == self.line {
                let cursor_pos = self.field.cursor().min(line.len());
                let selection_pos = self.field.selection().min(line.len());
                // Drawn under the text: Pomme's text highlight is a solid
                // block, where vanilla inverts the text it covers.
                if selection_pos != cursor_pos {
                    let (from, to) = (x_at(cursor_pos), x_at(selection_pos));
                    let (x0, x1) = (from.min(to), from.max(to));
                    rect(elements, x0, cursor_y, x1, cursor_bottom, FIELD_SELECTION);
                }
                if show_cursor {
                    let cursor_x = x_at(cursor_pos);
                    if cursor_pos < line.len() {
                        let (x1, y0) = (cursor_x + 1.0, cursor_y - 1.0);
                        rect(elements, cursor_x, y0, x1, cursor_bottom, self.text_color);
                    } else {
                        text(elements, "_", cursor_x, cursor_y);
                    }
                }
            }
            text(elements, line, -half, i as f32 * line_height - midpoint);
        }

        let (btn_w, btn_h) = (200.0 * gs, 20.0 * gs);
        let done_hovered = push_button(
            elements,
            cursor,
            gui_centered(screen_w, btn_w / gs, gs),
            gui_quarter_height(screen_h, gs) + 144.0 * gs,
            btn_w,
            btn_h,
            gs,
            FONT_SIZE * gs,
            crate::lang::translate("gui.done").unwrap_or("Done"),
            true,
        );
        clicked && done_hovered
    }

    /// `SignEditScreen.extractSignBackground` (a wall sign shows the board
    /// without its post) and `HangingSignEditScreen.extractSignBackground`.
    fn push_background(&self, elements: &mut Vec<MenuElement>, origin: (f32, f32), gs: f32) {
        let wood = self.block.wood as u8;
        // The pose's offset and scale, then the blit's corner and size.
        let (sprite, offset_y, scale, [x, y, w, h], v1) = if self.block.hanging {
            let sprite = SpriteId::HangingSign(wood);
            (sprite, -13.0, 4.5, [-8.0, -8.0, 16.0, 16.0], 1.0)
        } else {
            let shown = if self.block.wall { 12.0 } else { 26.0 };
            let sprite = SpriteId::Sign(wood);
            (sprite, 27.0, 3.9, [-12.0, -13.0, 24.0, shown], shown / 26.0)
        };
        let unit = scale * gs;
        elements.push(MenuElement::SubImage {
            x: origin.0 + x * unit,
            y: origin.1 + offset_y * gs + y * unit,
            w: w * unit,
            h: h * unit,
            sprite,
            src_rect: [0.0, 0.0, 1.0, v1],
            tint: WHITE,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_edit::KeyMods;

    fn key(code: KeyCode) -> TextInputEvent {
        TextInputEvent::Key {
            code,
            mods: KeyMods {
                shift: false,
                ctrl: false,
                alt: false,
                super_key: false,
            },
        }
    }

    fn editor(hanging: bool) -> SignEditState {
        let block = SignBlock {
            hanging,
            wall: false,
            wood: 0,
            angle: 0.0,
        };
        SignEditState::new(BlockPos::new(0, 0, 0), true, block, &SignText::default())
    }

    /// Six units a character, as the font's default advance.
    fn width(s: &str) -> f32 {
        s.chars().count() as f32 * 6.0
    }

    #[test]
    fn up_down_and_enter_wrap_through_the_four_lines() {
        let mut sign = editor(false);
        sign.handle_text_input(&[key(KeyCode::ArrowUp)], &width);
        assert_eq!(sign.line, 3);
        sign.handle_text_input(&[key(KeyCode::Enter), key(KeyCode::ArrowDown)], &width);
        assert_eq!(sign.line, 1);
        let typed = sign.handle_text_input(&[TextInputEvent::Char('a')], &width);
        assert!(typed);
        assert_eq!(sign.messages[1], "a");
    }

    #[test]
    fn a_line_stops_at_the_signs_text_width() {
        // 90 wide for a sign, 60 for a hanging sign.
        for (hanging, fits) in [(false, 15), (true, 10)] {
            let mut sign = editor(hanging);
            let chars: Vec<_> = (0..20).map(|_| TextInputEvent::Char('x')).collect();
            sign.handle_text_input(&chars, &width);
            assert_eq!(sign.messages[0].len(), fits);
        }
    }
}
