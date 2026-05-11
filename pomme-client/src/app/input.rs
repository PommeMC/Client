use std::collections::HashSet;
use winit::event::{ElementState, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};

pub struct InputState {
    pressed: HashSet<KeyCode>,
    mouse_delta: (f64, f64),
    cursor_captured: bool,
    selected_slot: u8,
    left_click: ClickState,
    right_click: ClickState,
    cursor_pos: (f32, f32),
    cursor_moved: bool,
    typed_chars: Vec<char>,
    menu_scroll: f32,
    backspace_pressed: bool,
    enter_pressed: bool,
    escape_pressed: bool,
    tab_pressed: bool,
    f5_pressed: bool,
}

#[derive(Default)]
pub struct ClickState {
    held: bool,
    just_pressed: bool,
    just_released: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            pressed: HashSet::new(),
            mouse_delta: (0.0, 0.0),
            cursor_captured: true,
            selected_slot: 0,
            left_click: ClickState::default(),
            right_click: ClickState::default(),
            cursor_pos: (0.0, 0.0),
            cursor_moved: false,
            typed_chars: Vec::new(),
            menu_scroll: 0.0,
            backspace_pressed: false,
            enter_pressed: false,
            escape_pressed: false,
            tab_pressed: false,
            f5_pressed: false,
        }
    }

    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    pub fn on_key_event(&mut self, event: &winit::event::KeyEvent) {
        if let PhysicalKey::Code(code) = event.physical_key {
            match event.state {
                ElementState::Pressed => {
                    self.pressed.insert(code);
                    if let Some(slot) = hotbar_slot(code) {
                        self.selected_slot = slot;
                    }
                }
                ElementState::Released => {
                    self.pressed.remove(&code);
                }
            }
        }
    }

    pub fn on_menu_key_event(&mut self, event: &winit::event::KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }

        if let PhysicalKey::Code(code) = event.physical_key {
            match code {
                KeyCode::Backspace => self.backspace_pressed = true,
                KeyCode::Enter | KeyCode::NumpadEnter => self.enter_pressed = true,
                KeyCode::Escape => self.escape_pressed = true,
                KeyCode::Tab => self.tab_pressed = true,
                KeyCode::F5 => self.f5_pressed = true,
                _ => {}
            }
        }

        if let Some(text) = &event.text {
            for ch in text.chars() {
                if !ch.is_control() {
                    self.typed_chars.push(ch);
                }
            }
        }
    }

    pub fn drain_typed_chars(&mut self) -> Vec<char> {
        std::mem::take(&mut self.typed_chars)
    }

    pub fn consume_menu_scroll(&mut self) -> f32 {
        let s = self.menu_scroll;
        self.menu_scroll = 0.0;
        s
    }

    pub fn on_menu_scroll(&mut self, delta: f32) {
        self.menu_scroll += delta;
    }

    pub fn backspace_pressed(&mut self) -> bool {
        std::mem::take(&mut self.backspace_pressed)
    }

    pub fn enter_pressed(&mut self) -> bool {
        std::mem::take(&mut self.enter_pressed)
    }

    pub fn escape_pressed(&mut self) -> bool {
        std::mem::take(&mut self.escape_pressed)
    }

    pub fn tab_pressed(&mut self) -> bool {
        std::mem::take(&mut self.tab_pressed)
    }

    pub fn tab_held(&self) -> bool {
        self.pressed.contains(&KeyCode::Tab)
    }

    pub fn f5_pressed(&mut self) -> bool {
        std::mem::take(&mut self.f5_pressed)
    }

    pub fn selected_slot(&self) -> u8 {
        self.selected_slot
    }

    pub fn on_scroll(&mut self, delta: f32) {
        if delta > 0.0 {
            self.selected_slot = (self.selected_slot + 8) % 9;
        } else if delta < 0.0 {
            self.selected_slot = (self.selected_slot + 1) % 9;
        }
    }

    pub fn on_mouse_motion(&mut self, delta: (f64, f64)) {
        self.mouse_delta.0 += delta.0;
        self.mouse_delta.1 += delta.1;
    }

    pub fn consume_mouse_delta(&mut self) -> (f64, f64) {
        let delta = self.mouse_delta;
        self.mouse_delta = (0.0, 0.0);
        delta
    }

    pub fn on_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let click = match button {
            MouseButton::Left => &mut self.left_click,
            MouseButton::Right => &mut self.right_click,
            _ => return,
        };
        match state {
            ElementState::Pressed => {
                click.held = true;
                click.just_pressed = true;
            }
            ElementState::Released => {
                click.held = false;
                click.just_released = true;
            }
        }
    }

    pub fn left_just_pressed(&self) -> bool {
        self.left_click.just_pressed
    }

    pub fn left_held(&self) -> bool {
        self.left_click.held
    }

    pub fn right_just_pressed(&self) -> bool {
        self.right_click.just_pressed
    }

    pub fn right_held(&self) -> bool {
        self.right_click.held
    }

    pub fn clear_click_events(&mut self) {
        self.left_click.just_pressed = false;
        self.left_click.just_released = false;
        self.right_click.just_pressed = false;
        self.right_click.just_released = false;
        self.cursor_moved = false;
    }

    pub fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.cursor_pos = (x, y);
        self.cursor_moved = true;
    }

    pub fn cursor_moved_this_frame(&self) -> bool {
        self.cursor_moved
    }

    pub fn cursor_pos(&self) -> (f32, f32) {
        self.cursor_pos
    }

    pub fn is_cursor_captured(&self) -> bool {
        self.cursor_captured
    }
}

fn hotbar_slot(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        _ => None,
    }
}
