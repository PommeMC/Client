use std::collections::{HashMap, HashSet};

use gilrs::{Button, GamepadId, Gilrs};
use winit::event::{ElementState, Modifiers, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::app::phases::AppPhase;
use crate::app::state_slot::StateSlot;

/// Left-stick deflection past which a direction counts as a digital press.
pub const STICK_MOVEMENT_THRESHOLD: f32 = 0.25;

#[derive(Hash, PartialEq, Eq, Clone)]
pub enum Action {
    Jump,
    Sneak,
    Sprint,
    Destroy,
    Use,
    ToggleInventory,
    OpenMenu,
    ViewPlayerList,
    ChangePerspective,
}

pub struct InputState {
    pressed: HashSet<KeyCode>,
    modifiers: Modifiers,
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
    select_all_pressed: bool,
    copy_pressed: bool,
    cut_pressed: bool,
    undo_pressed: bool,
    controller_manager: Option<Gilrs>,
    active_gamepad_id: Option<GamepadId>,
    recent_actions: HashMap<Action, bool>,
}

#[derive(Default)]
pub struct ClickState {
    held: bool,
    just_pressed: bool,
    just_released: bool,
}

impl InputState {
    pub fn new() -> Self {
        let controller_manager = match Gilrs::new() {
            Ok(gilrs) => Some(gilrs),
            Err(err) => {
                tracing::warn!("Controller support disabled: failed to initialize gilrs: {err}");
                None
            }
        };
        Self::with_controller(controller_manager)
    }

    /// Neutral input (no keys, cursor released) for ticking while a menu is
    /// open. Never reads the controller, so it skips gilrs initialization.
    pub fn released() -> Self {
        Self {
            cursor_captured: false,
            ..Self::with_controller(None)
        }
    }

    fn with_controller(controller_manager: Option<Gilrs>) -> Self {
        Self {
            pressed: HashSet::new(),
            modifiers: Modifiers::default(),
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
            select_all_pressed: false,
            copy_pressed: false,
            cut_pressed: false,
            undo_pressed: false,
            controller_manager,
            active_gamepad_id: None,
            recent_actions: HashMap::new(),
        }
    }

    pub fn update(&mut self, phase: &mut StateSlot<AppPhase>) -> bool {
        let events: Vec<gilrs::Event> = match self.controller_manager.as_mut() {
            Some(manager) => std::iter::from_fn(|| manager.next_event()).collect(),
            None => Vec::new(),
        };
        for event in &events {
            self.on_gamepad_event(event);
        }

        let mut should_apply_cursor_grab = false;

        phase.transition(|mut app| {
            if let AppPhase::InGame {
                gfx,
                connection: _connection,
                game,
            } = &mut app
            {
                if self.action_just_pressed(Action::ToggleInventory) {
                    if game.creative_inventory_open {
                        game.creative_inventory_open = false;
                        should_apply_cursor_grab = true;
                    } else if !game.paused
                        && !game.dead
                        && game.player.game_mode != 3
                        && !game.chat.is_open()
                    {
                        if game.player.game_mode == 1 {
                            game.creative_inventory_open = true;
                        } else {
                            game.inventory_open = !game.inventory_open;
                        }
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::ToggleInventory);
                }
                if self.action_just_pressed(Action::OpenMenu) {
                    if !game.dead && !game.options_from_game {
                        use crate::ui::pause::PauseScreen;
                        if game.inventory_open {
                            game.inventory_open = false;
                        } else if game.paused {
                            // Step back through the benchmark sub-screens; close
                            // the menu from the main screen.
                            match game.pause_screen {
                                PauseScreen::ChunkLoader => {
                                    game.pause_screen = PauseScreen::Benchmark
                                }
                                PauseScreen::Benchmark => game.pause_screen = PauseScreen::Main,
                                PauseScreen::Main => game.paused = false,
                            }
                        } else {
                            game.paused = true;
                            game.pause_screen = PauseScreen::Main;
                        }

                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::OpenMenu);
                }
                if self.action_just_pressed(Action::ChangePerspective) {
                    gfx.renderer.cycle_camera_mode();

                    self.recent_actions.remove(&Action::ChangePerspective);
                }
            }

            app
        });

        should_apply_cursor_grab
    }

    pub fn get_active_gamepad(&self) -> Option<gilrs::Gamepad<'_>> {
        let manager = self.controller_manager.as_ref()?;
        self.active_gamepad_id.map(|id| manager.gamepad(id))
    }

    pub fn gamepad_button_down(&self, button: Button) -> bool {
        if let Some(gamepad) = self.get_active_gamepad() {
            return gamepad
                .button_data(button)
                .map(|button| button.is_pressed())
                .unwrap_or(false);
        }

        false
    }

    pub fn on_gamepad_event(&mut self, event: &gilrs::Event) {
        self.active_gamepad_id = Some(event.id);

        match event.event {
            gilrs::EventType::ButtonPressed(button, _) => match button {
                Button::RightTrigger2 => {
                    self.recent_actions.insert(Action::Destroy, true);
                }
                Button::RightTrigger => {
                    self.selected_slot = (self.selected_slot + 1) % 9;
                }
                Button::LeftTrigger2 => {
                    self.recent_actions.insert(Action::Use, true);
                }
                Button::LeftTrigger => {
                    self.selected_slot = (self.selected_slot + 8) % 9;
                }
                Button::North => {
                    self.recent_actions.insert(Action::ToggleInventory, true);
                }

                Button::Start => {
                    self.recent_actions.insert(Action::OpenMenu, true);
                }

                Button::DPadUp => {
                    self.recent_actions.insert(Action::ChangePerspective, true);
                }

                _ => {}
            },
            gilrs::EventType::ButtonReleased(button, _) => match button {
                Button::RightTrigger2 => {
                    self.recent_actions.insert(Action::Destroy, false);
                }
                Button::LeftTrigger2 => {
                    self.recent_actions.insert(Action::Use, false);
                }
                Button::North => {
                    self.recent_actions.insert(Action::ToggleInventory, false);
                }

                Button::Start => {
                    self.recent_actions.insert(Action::OpenMenu, false);
                }

                Button::DPadUp => {
                    self.recent_actions.insert(Action::ChangePerspective, false);
                }

                _ => {}
            },

            _ => {}
        }
    }

    pub fn performing_action(&self, action: Action) -> bool {
        match action {
            Action::Jump => {
                self.key_pressed(KeyCode::Space) || self.gamepad_button_down(Button::South)
            }
            Action::Sneak => {
                self.key_pressed(KeyCode::ShiftLeft) || self.gamepad_button_down(Button::LeftThumb)
            }
            Action::Sprint => {
                self.key_pressed(KeyCode::ControlLeft) || self.gamepad_button_down(Button::West)
            }
            Action::Destroy => self.left_held() || self.gamepad_button_down(Button::RightTrigger2),
            Action::Use => self.right_held() || self.gamepad_button_down(Button::LeftTrigger2),
            Action::ToggleInventory => {
                self.action_just_pressed(Action::ToggleInventory)
                    || self.gamepad_button_down(Button::North)
            }
            Action::OpenMenu => {
                self.key_pressed(KeyCode::Escape) || self.gamepad_button_down(Button::East)
            }
            Action::ViewPlayerList => {
                self.key_pressed(KeyCode::Tab) || self.gamepad_button_down(Button::Select)
            }
            Action::ChangePerspective => {
                self.key_pressed(KeyCode::F5) || self.gamepad_button_down(Button::DPadUp)
            }
        }
    }

    pub fn action_just_pressed(&self, action: Action) -> bool {
        self.recent_actions.get(&action).copied().unwrap_or(false)
    }

    /// Drops a pending action so a handler that already consumed the
    /// originating key press doesn't trigger it again.
    pub fn clear_action(&mut self, action: Action) {
        self.recent_actions.remove(&action);
    }

    pub fn clear_just_pressed_actions(&mut self) {
        self.recent_actions.clear();

        self.left_click.just_pressed = false;
        self.left_click.just_released = false;
        self.right_click.just_pressed = false;
        self.right_click.just_released = false;
        self.cursor_moved = false;
    }

    fn gamepad_stick(&self, x_axis: gilrs::Axis, y_axis: gilrs::Axis) -> Option<glam::Vec2> {
        let gamepad = self.get_active_gamepad()?;
        let value = |axis| {
            gamepad
                .axis_data(axis)
                .map(|data| data.value())
                .unwrap_or(0f32)
        };
        let desired = glam::vec2(value(x_axis), value(y_axis)).clamp_length_max(1.0);

        (desired.length() >= 1E-1).then_some(desired)
    }

    pub fn get_gamepad_left_analog(&self) -> Option<glam::Vec2> {
        self.gamepad_stick(gilrs::Axis::LeftStickX, gilrs::Axis::LeftStickY)
    }

    pub fn get_gamepad_right_analog(&self) -> Option<glam::Vec2> {
        self.gamepad_stick(gilrs::Axis::RightStickX, gilrs::Axis::RightStickY)
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
                    match code {
                        KeyCode::KeyE => {
                            self.recent_actions.insert(Action::ToggleInventory, true);
                        }
                        KeyCode::Escape => {
                            self.recent_actions.insert(Action::OpenMenu, true);
                        }
                        KeyCode::F5 => {
                            self.recent_actions.insert(Action::ChangePerspective, true);
                        }
                        _ => {}
                    }
                }
                ElementState::Released => {
                    self.pressed.remove(&code);
                }
            }
        }
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
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
                KeyCode::KeyV if self.modifiers.state().control_key() => {
                    if let Ok(mut cb) = arboard::Clipboard::new()
                        && let Ok(text) = cb.get_text()
                    {
                        for ch in text.chars() {
                            if !ch.is_control() {
                                self.typed_chars.push(ch);
                            }
                        }
                    }
                    return;
                }
                KeyCode::KeyA if self.modifiers.state().control_key() => {
                    self.select_all_pressed = true;
                    return;
                }
                KeyCode::KeyC if self.modifiers.state().control_key() => {
                    self.copy_pressed = true;
                    return;
                }
                KeyCode::KeyX if self.modifiers.state().control_key() => {
                    self.cut_pressed = true;
                    return;
                }
                KeyCode::KeyZ if self.modifiers.state().control_key() => {
                    self.undo_pressed = true;
                    return;
                }
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

    pub fn shift_held(&self) -> bool {
        self.modifiers.state().shift_key()
    }

    pub fn f5_pressed(&mut self) -> bool {
        std::mem::take(&mut self.f5_pressed)
    }

    pub fn select_all_pressed(&mut self) -> bool {
        std::mem::take(&mut self.select_all_pressed)
    }

    pub fn copy_pressed(&mut self) -> bool {
        std::mem::take(&mut self.copy_pressed)
    }

    pub fn cut_pressed(&mut self) -> bool {
        std::mem::take(&mut self.cut_pressed)
    }

    pub fn undo_pressed(&mut self) -> bool {
        std::mem::take(&mut self.undo_pressed)
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
        let was_pressed = match state {
            ElementState::Pressed => true,
            ElementState::Released => false,
        };

        match button {
            MouseButton::Left => {
                self.left_click.held = was_pressed;
                if was_pressed {
                    self.left_click.just_pressed = true;
                    self.recent_actions.insert(Action::Destroy, true);
                } else {
                    self.left_click.just_released = true;
                    self.recent_actions.insert(Action::Destroy, false);
                }
            }
            MouseButton::Right => {
                self.right_click.held = was_pressed;
                if was_pressed {
                    self.right_click.just_pressed = true;
                    self.recent_actions.insert(Action::Use, true);
                } else {
                    self.right_click.just_released = true;
                    self.recent_actions.insert(Action::Use, false);
                }
            }
            _ => (),
        }
    }

    pub fn left_just_pressed(&self) -> bool {
        self.left_click.just_pressed
    }

    pub fn left_held(&self) -> bool {
        self.left_click.held
    }

    pub fn right_held(&self) -> bool {
        self.right_click.held
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
