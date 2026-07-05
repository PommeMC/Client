use std::collections::{HashMap, HashSet};

use gilrs::ff::{BaseEffect, Effect, EffectBuilder, Repeat, Replay};
use gilrs::{Button, GamepadId, Gilrs};
use winit::event::{ElementState, Modifiers, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::app::TICK_RATE_MS;
use crate::app::phases::AppPhase;
use crate::app::state_slot::StateSlot;

/// Left-stick deflection past which a direction counts as a digital press.
pub const STICK_MOVEMENT_THRESHOLD: f32 = 0.25;

/// Value in milliseconds for how long the controller should rumble to be only
/// an "instant".
pub const SHORT_RUMBLE_TIME: u32 = 5;

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
    OpenChat,
    OpenCommands,
    Close,
}

pub struct InputState {
    pressed: HashSet<KeyCode>,
    modifiers: Modifiers,
    mouse_delta: (f64, f64),
    cursor_captured: bool,
    selected_slot: u8,
    left_click: ClickState,
    right_click: ClickState,
    middle_click: ClickState,
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
    gamepad_manager: Option<Gilrs>,
    weak_rumble_effect: Option<Effect>,
    strong_rumble_effect: Option<Effect>,
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

    fn with_controller(mut gamepad_manager: Option<Gilrs>) -> Self {
        // `Weak`/`Strong` pick the motor, not the intensity, so the weak motor
        // at a higher `magnitude` is intentional.
        let (weak_effect, strong_effect) = match gamepad_manager.as_mut() {
            Some(manager) => (
                build_rumble_effect(
                    manager,
                    gilrs::ff::BaseEffectType::Weak { magnitude: 60_000 },
                    gilrs::ff::Ticks::from_ms(SHORT_RUMBLE_TIME),
                ),
                build_rumble_effect(
                    manager,
                    gilrs::ff::BaseEffectType::Strong { magnitude: 30_000 },
                    gilrs::ff::Ticks::from_ms(TICK_RATE_MS + 50),
                ),
            ),
            None => (None, None),
        };

        Self {
            pressed: HashSet::new(),
            modifiers: Modifiers::default(),
            mouse_delta: (0.0, 0.0),
            cursor_captured: true,
            selected_slot: 0,
            left_click: ClickState::default(),
            right_click: ClickState::default(),
            middle_click: ClickState::default(),
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
            gamepad_manager,
            weak_rumble_effect: weak_effect,
            strong_rumble_effect: strong_effect,
            active_gamepad_id: None,
            recent_actions: HashMap::new(),
        }
    }

    pub fn update(&mut self, phase: &mut StateSlot<AppPhase>) -> bool {
        let events: Vec<gilrs::Event> = match self.gamepad_manager.as_mut() {
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
                        game.close_creative_inventory();
                        should_apply_cursor_grab = true;
                    } else if game.open_container.is_some() {
                        game.close_menu();
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
                    if game.chunk_load_bench.is_some() {
                        // Cancel a running benchmark instead of opening the menu;
                        // update_game restores the render distance next frame.
                        game.chunk_load_abort = true;
                        should_apply_cursor_grab = true;
                    } else if !game.dead && !game.options_from_game {
                        use crate::ui::pause::PauseScreen;
                        if game.inventory_open || game.open_container.is_some() {
                            game.close_menu();
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
                if self.action_just_pressed(Action::Close) {
                    if !game.dead && (game.inventory_open || game.open_container.is_some()) {
                        game.close_menu();
                        should_apply_cursor_grab = true;
                    }

                    if game.chat.is_open() {
                        game.chat.close();
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::Close);
                }
                if self.action_just_pressed(Action::ChangePerspective) {
                    gfx.renderer.cycle_camera_mode();

                    self.recent_actions.remove(&Action::ChangePerspective);
                }
                if self.action_just_pressed(Action::OpenChat) {
                    if !game.paused && !game.gui_open() {
                        game.chat.open();
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::OpenChat);
                }
                if self.action_just_pressed(Action::OpenCommands) {
                    if !game.paused && !game.gui_open() {
                        game.chat.open_with_slash();
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::OpenCommands);
                }
            }

            app
        });

        should_apply_cursor_grab
    }

    pub fn get_active_gamepad(&self) -> Option<gilrs::Gamepad<'_>> {
        let manager = self.gamepad_manager.as_ref()?;
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

                Button::DPadRight => {
                    self.recent_actions.insert(Action::OpenChat, true);
                }

                Button::East => {
                    self.recent_actions.insert(Action::Close, true);
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

                Button::DPadRight => {
                    self.recent_actions.insert(Action::OpenChat, false);
                }

                Button::East => {
                    self.recent_actions.insert(Action::Close, false);
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
                self.key_pressed(KeyCode::Escape) || self.gamepad_button_down(Button::Start)
            }
            Action::ViewPlayerList => {
                self.key_pressed(KeyCode::Tab) || self.gamepad_button_down(Button::Select)
            }
            Action::ChangePerspective => {
                self.key_pressed(KeyCode::F5) || self.gamepad_button_down(Button::DPadUp)
            }
            Action::OpenChat => {
                self.key_pressed(KeyCode::KeyT) || self.gamepad_button_down(Button::DPadRight)
            }
            Action::OpenCommands => self.key_pressed(KeyCode::Slash),
            // Controller-only; keyboard Escape closes via OpenMenu and the chat path.
            Action::Close => self.gamepad_button_down(Button::East),
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
        self.middle_click.just_pressed = false;
        self.middle_click.just_released = false;
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

    pub fn weak_rumble_for_instant(&self) -> Result<(), gilrs::ff::Error> {
        self.weak_rumble_effect
            .as_ref()
            .map_or(Ok(()), Effect::play)
    }

    pub fn strong_rumble_for_tick(&self) -> Result<(), gilrs::ff::Error> {
        self.strong_rumble_effect
            .as_ref()
            .map_or(Ok(()), Effect::play)
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
                        KeyCode::KeyT => {
                            self.recent_actions.insert(Action::OpenChat, true);
                        }
                        KeyCode::Slash => {
                            self.recent_actions.insert(Action::OpenCommands, true);
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
            MouseButton::Middle => {
                self.middle_click.held = was_pressed;
                if was_pressed {
                    self.middle_click.just_pressed = true;
                } else {
                    self.middle_click.just_released = true;
                }
            }
            _ => (),
        }
    }

    pub fn left_just_pressed(&self) -> bool {
        self.left_click.just_pressed
    }

    pub fn right_just_pressed(&self) -> bool {
        self.right_click.just_pressed
    }

    pub fn left_held(&self) -> bool {
        self.left_click.held
    }

    pub fn right_held(&self) -> bool {
        self.right_click.held
    }

    pub fn middle_just_pressed(&self) -> bool {
        self.middle_click.just_pressed
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

/// Build a single-motor force-feedback effect targeting the FF-capable gamepads
/// connected right now. Rumble is best-effort: returns `None` if no controller
/// supports it or the effect can't be created.
///
/// TODO: the effect is bound to the gamepads present when this runs (startup);
/// a controller connected later won't rumble until this is rebuilt on hotplug.
fn build_rumble_effect(
    manager: &mut Gilrs,
    kind: gilrs::ff::BaseEffectType,
    duration: gilrs::ff::Ticks,
) -> Option<Effect> {
    let ff_supported = manager
        .gamepads()
        .filter_map(|(id, gp)| gp.is_ff_supported().then_some(id))
        .collect::<Vec<_>>();
    if ff_supported.is_empty() {
        return None;
    }

    EffectBuilder::new()
        .add_effect(BaseEffect {
            kind,
            scheduling: Replay {
                play_for: duration,
                ..Default::default()
            },
            envelope: Default::default(),
        })
        .repeat(Repeat::For(duration))
        .gamepads(&ff_supported)
        .finish(manager)
        .map_err(|e| tracing::warn!("Failed to create rumble effect: {e}"))
        .ok()
}
