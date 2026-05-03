use std::collections::VecDeque;
use std::time::Instant;

use super::common::{self, WHITE};
use crate::renderer::pipelines::menu_overlay::MenuElement;

const MAX_MESSAGES: usize = 100;
const VISIBLE_MESSAGES: usize = 10;
const MESSAGE_LIFETIME_SECS: f32 = 10.0;
const CHAT_X: f32 = 4.0;
const CHAT_BOTTOM_OFFSET: f32 = 52.0;
const LINE_HEIGHT: f32 = 12.0;
const CHAT_WIDTH: f32 = 320.0;
const INPUT_HEIGHT: f32 = 16.0;
const TEXT_PAD: f32 = 4.0;

const MSG_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.39];
const INPUT_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

struct ChatLine {
    text: String,
    received: Instant,
}

pub struct ChatState {
    messages: VecDeque<ChatLine>,
    input: String,
    open: bool,
    cursor_blink: Instant,
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            input: String::new(),
            open: false,
            cursor_blink: Instant::now(),
        }
    }

    pub fn push_message(&mut self, text: String) {
        self.messages.push_back(ChatLine {
            text,
            received: Instant::now(),
        });
        if self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.input.clear();
        self.cursor_blink = Instant::now();
    }

    pub fn open_with_slash(&mut self) {
        self.open = true;
        self.input = "/".into();
        self.cursor_blink = Instant::now();
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn handle_key_input(
        &mut self,
        typed_chars: &[char],
        backspace: bool,
        enter: bool,
    ) -> Option<String> {
        if !self.open {
            return None;
        }

        for ch in typed_chars {
            self.input.push(*ch);
            self.cursor_blink = Instant::now();
        }
        if backspace {
            self.input.pop();
            self.cursor_blink = Instant::now();
        }
        if enter {
            let msg = if self.input.is_empty() {
                None
            } else {
                Some(self.input.clone())
            };
            self.input.clear();
            self.open = false;
            return msg;
        }

        None
    }

    pub fn build(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_h: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) {
        let now = Instant::now();
        let fs = common::FONT_SIZE * gs;
        let lh = LINE_HEIGHT * gs;
        let chat_w = CHAT_WIDTH * gs;
        let chat_x = CHAT_X * gs;
        let pad = TEXT_PAD * gs;
        let chat_bottom = screen_h - CHAT_BOTTOM_OFFSET * gs;

        let visible: Vec<&ChatLine> = if self.open {
            self.messages.iter().rev().take(VISIBLE_MESSAGES).collect()
        } else {
            self.messages
                .iter()
                .rev()
                .filter(|m| now.duration_since(m.received).as_secs_f32() < MESSAGE_LIFETIME_SECS)
                .take(VISIBLE_MESSAGES)
                .collect()
        };

        for (i, line) in visible.iter().enumerate() {
            let y = chat_bottom - (i as f32 + 1.0) * lh;
            elements.push(MenuElement::Rect {
                x: chat_x,
                y,
                w: chat_w,
                h: lh,
                corner_radius: 0.0,
                color: MSG_BG,
            });
            elements.push(MenuElement::Text {
                x: chat_x + pad,
                y: y + (lh - fs) / 2.0,
                text: line.text.clone(),
                scale: fs,
                color: WHITE,
                centered: false,
            });
        }

        if self.open {
            let input_h = INPUT_HEIGHT * gs;
            let text_y = chat_bottom + (input_h - fs) / 2.0;

            elements.push(MenuElement::Rect {
                x: chat_x,
                y: chat_bottom,
                w: chat_w,
                h: input_h,
                corner_radius: 0.0,
                color: INPUT_BG,
            });
            elements.push(MenuElement::Text {
                x: chat_x + pad,
                y: text_y,
                text: self.input.clone(),
                scale: fs,
                color: WHITE,
                centered: false,
            });

            let tw = text_width_fn(&self.input, fs);
            common::push_cursor_blink(
                elements,
                &self.cursor_blink,
                chat_x + pad,
                text_y,
                gs,
                fs,
                tw,
            );
        }
    }
}
