//! Physical input state, logical action metadata, and persistent key bindings.
//!
//! Key bindings serialize action IDs to arrays of physical input IDs, for
//! example `"key.jump": ["key.keyboard.space", "key.gamepad.south"]`.
//! The Key Binds screen edits these settings using limited input capture.
use std::collections::{HashMap, HashSet};

use gilrs::ff::{BaseEffect, Effect, EffectBuilder, Repeat, Replay};
use gilrs::{Button, GamepadId, Gilrs};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, Touch};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

use self::touch::{TouchControls, TouchLayout, TouchOutput};
use crate::app::TICK_RATE_MS;
use crate::app::phases::AppPhase;
use crate::app::state_slot::StateSlot;

pub mod action;

pub mod input_key;
pub mod key_bindings;
mod metadata;
pub mod touch;

pub use action::Action;
pub use input_key::InputKey;
pub use key_bindings::KeyBindings;
pub use metadata::{ActionId, BindingMetadata, DisplayText, NOT_BOUND, StringId};

/// Left-stick deflection past which a direction counts as a digital press.
pub const STICK_MOVEMENT_THRESHOLD: f32 = 0.25;

/// Convert gilrs left-stick axes into vanilla movement axes: X is `xxa`
/// (positive left, negative right) and Y is `zza` (positive forward).
pub(crate) fn gamepad_movement_axes(analog: glam::Vec2) -> glam::Vec2 {
    glam::vec2(-analog.x, analog.y)
}

/// Value in milliseconds for how long the controller should rumble to be only
/// an "instant".
pub const SHORT_RUMBLE_TIME: u32 = 5;

pub struct InputState {
    pub key_bindings: KeyBindings,
    pressed: HashSet<KeyCode>,
    modifiers: Modifiers,
    /// Ctrl held by a harness `press`, for one frame.
    #[cfg(feature = "test-harness")]
    harness_ctrl: bool,
    /// A harness `press` waiting for the next frame.
    #[cfg(feature = "test-harness")]
    harness_press: Option<(KeyCode, bool)>,
    mouse_delta: (f64, f64),
    cursor_captured: bool,
    touch: TouchControls,
    selected_slot: u8,
    left_click: ClickState,
    right_click: ClickState,
    middle_click: ClickState,
    cursor_pos: (f32, f32),
    cursor_moved: bool,
    menu_scroll: f32,
    menu_scroll_x: f32,
    /// A focused text field (anvil rename, creative search) is capturing
    /// keyboard input this frame: letter/digit hotkeys must type, not act.
    pub text_capture: bool,
    /// A container screen (inventory, chest, creative) is open: it consumes
    /// hotbar digits and the drop/swap keys, like vanilla screens do.
    pub menu_capture: bool,
    /// The player is a spectator: digits drive the spectator menu instead of
    /// the carried slot, and middle click is `key.spectatorHotbar`.
    pub spectator: bool,
    /// Digit presses queued for the spectator menu, drained per tick like the
    /// click counts (vanilla routes them in `handleKeybinds`).
    spectator_slot_presses: Vec<u8>,
    /// Keys pressed since the last `end_frame`, including OS key repeats:
    /// vanilla dispatches GLFW repeats to screens and debug chords.
    just_pressed: HashSet<KeyCode>,
    mouse_buttons_down: HashSet<MouseButton>,
    mouse_buttons_just_pressed: HashSet<MouseButton>,
    gamepad_buttons_just_pressed: HashSet<(GamepadId, Button)>,
    /// Per-action press counters mirroring vanilla `KeyMapping.click`, so a
    /// key repeating faster than the tick rate still fires once per press.
    click_counts: HashMap<Action, u32>,
    /// Ordered key/char events for focused text fields, mirroring vanilla's
    /// `keyPressed` + `charTyped` callback pair. Drained once per frame by
    /// whichever screen owns the focused field.
    text_events: Vec<crate::ui::text_edit::TextInputEvent>,
    binding_capture: bool,
    binding_presses: Vec<InputKey>,
    // Keep captured keys/buttons suppressed until release, even if capture
    // ended on the preceding frame (notably F3's release-triggered shortcut).
    binding_keys_down: HashSet<KeyCode>,
    binding_gamepad_buttons_down: HashSet<(gilrs::GamepadId, Button)>,
    enter_pressed: bool,
    escape_pressed: bool,
    tab_pressed: bool,
    f5_pressed: bool,
    up_pressed: bool,
    down_pressed: bool,
    page_up_pressed: bool,
    page_down_pressed: bool,
    gamepad_manager: Option<Gilrs>,
    weak_rumble_effect: Option<Effect>,
    strong_rumble_effect: Option<Effect>,
    active_gamepad_id: Option<GamepadId>,
    recent_actions: HashMap<Action, bool>,
    touch_actions_down: HashSet<Action>,
    /// Touch taps' releases, held back until the presses have been read.
    tap_releases: Vec<Action>,
}

#[derive(Default)]
pub struct ClickState {
    held: bool,
    just_pressed: bool,
    just_released: bool,
}

impl ClickState {
    fn set(&mut self, pressed: bool) {
        self.held = pressed;
        if pressed {
            self.just_pressed = true;
        } else {
            self.just_released = true;
        }
    }
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
            key_bindings: KeyBindings::default(),
            pressed: HashSet::new(),
            modifiers: Modifiers::default(),
            #[cfg(feature = "test-harness")]
            harness_ctrl: false,
            #[cfg(feature = "test-harness")]
            harness_press: None,
            mouse_delta: (0.0, 0.0),
            cursor_captured: true,
            touch: TouchControls::default(),
            selected_slot: 0,
            left_click: ClickState::default(),
            right_click: ClickState::default(),
            middle_click: ClickState::default(),
            cursor_pos: (0.0, 0.0),
            cursor_moved: false,
            menu_scroll: 0.0,
            menu_scroll_x: 0.0,
            text_capture: false,
            menu_capture: false,
            spectator: false,
            spectator_slot_presses: Vec::new(),
            just_pressed: HashSet::new(),
            mouse_buttons_down: HashSet::new(),
            mouse_buttons_just_pressed: HashSet::new(),
            gamepad_buttons_just_pressed: HashSet::new(),
            click_counts: HashMap::new(),
            text_events: Vec::new(),
            binding_capture: false,
            binding_presses: Vec::new(),
            binding_keys_down: HashSet::new(),
            binding_gamepad_buttons_down: HashSet::new(),
            enter_pressed: false,
            escape_pressed: false,
            tab_pressed: false,
            f5_pressed: false,
            up_pressed: false,
            down_pressed: false,
            page_up_pressed: false,
            page_down_pressed: false,
            gamepad_manager,
            weak_rumble_effect: weak_effect,
            strong_rumble_effect: strong_effect,
            active_gamepad_id: None,
            recent_actions: HashMap::new(),
            touch_actions_down: HashSet::new(),
            tap_releases: Vec::new(),
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
        let held = self
            .touch
            .tick(std::time::Instant::now(), phase.get().world_has_pointer());
        self.apply_touch(held);

        if self.binding_capture {
            return false;
        }

        let mut should_apply_cursor_grab = false;

        phase.transition(|mut app| {
            if let AppPhase::InGame {
                gfx,
                connection,
                game,
                ..
            } = &mut app
            {
                // A dialog (or the confirm screen over it) is a screen, and
                // vanilla runs no key mapping while one is up.
                if self.action_just_pressed(Action::ToggleInventory) && !game.dialog_open() {
                    if game.creative_inventory_open
                        || game.open_container.is_some()
                        || game.inventory_open
                    {
                        game.close_inventory();
                        should_apply_cursor_grab = true;
                    } else if !game.paused
                        && !game.dead
                        && !game.death_screen_open
                        && game.player.game_mode != 3
                        && !game.chat.is_open()
                        && game.game_mode_switcher.is_none()
                    {
                        game.open_inventory();
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::ToggleInventory);
                }
                if self.action_just_pressed(Action::OpenMenu) && !game.dialog_open() {
                    if game.chat.is_open() {
                        // ChatScreen consumes Escape before the game-level
                        // pause action: link confirmation, then suggestions,
                        // then the chat screen itself.
                        should_apply_cursor_grab = game.escape_chat();
                    } else if game.game_mode_switcher.is_some() {
                        // Esc cancels the F3+F4 switcher without applying.
                        game.game_mode_switcher = None;
                        should_apply_cursor_grab = true;
                    } else if game.chunk_load_bench.is_some() {
                        // Cancel a running benchmark instead of opening the menu;
                        // update_game restores the render distance next frame.
                        game.chunk_load_abort = true;
                        should_apply_cursor_grab = true;
                    } else if !game.dead && !game.death_screen_open && !game.options_from_game {
                        use crate::ui::pause::PauseScreen;
                        if game.inventory_open || game.open_container.is_some() {
                            if !game
                                .recipe_book_ui
                                .close_for_escape(&mut game.recipe_book, &connection.packet_tx)
                            {
                                game.close_menu();
                            }
                        } else if game.paused {
                            // Step back through the benchmark sub-screens; close
                            // the menu from the main screen.
                            match game.pause_screen {
                                PauseScreen::ChunkLoader => {
                                    game.pause_screen = PauseScreen::Benchmark
                                }
                                PauseScreen::Benchmark => game.pause_screen = PauseScreen::Main,
                                PauseScreen::Main | PauseScreen::Hidden => game.paused = false,
                            }
                        } else {
                            game.paused = true;
                            game.pause_screen = PauseScreen::Main;
                        }

                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::OpenMenu);
                }
                if self.action_just_pressed(Action::Close) && !game.dialog_open() {
                    if !game.dead
                        && !game.death_screen_open
                        && (game.inventory_open || game.open_container.is_some())
                    {
                        game.close_menu();
                        should_apply_cursor_grab = true;
                    }

                    // Same order as Escape: modal, then suggestions, then chat.
                    if game.chat.is_open() {
                        should_apply_cursor_grab |= game.escape_chat();
                    }

                    self.recent_actions.remove(&Action::Close);
                }
                if self.action_just_pressed(Action::ChangePerspective) {
                    if !game.death_screen_open {
                        gfx.renderer.cycle_camera_mode();
                    }

                    self.recent_actions.remove(&Action::ChangePerspective);
                }
                if self.action_just_pressed(Action::OpenChat) {
                    if game.recipe_book_ui.focus_search_from_chat_key(
                        &game.recipe_book,
                        game.recipe_book_screen_active(),
                    ) {
                        // Vanilla `keyChat` focuses an open recipe book's search.
                        self.text_capture = true;
                    } else if !game.paused
                        && !game.death_screen_open
                        && !game.gui_open()
                        && !game.chat.is_open()
                    {
                        game.chat.open(
                            crate::ui::chat::ChatMethod::Message,
                            game.command_tree.as_deref(),
                        );
                        // The frame flag is written at end of update; set it now
                        // so keys later in this same event batch already type.
                        self.text_capture = true;
                        should_apply_cursor_grab = true;
                    }

                    self.recent_actions.remove(&Action::OpenChat);
                }
                if self.action_just_pressed(Action::OpenCommands) {
                    if !game.paused
                        && !game.death_screen_open
                        && !game.gui_open()
                        && !game.chat.is_open()
                    {
                        game.chat.open(
                            crate::ui::chat::ChatMethod::Command,
                            game.command_tree.as_deref(),
                        );
                        self.text_capture = true;
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
        if self
            .active_gamepad_id
            .is_some_and(|id| self.binding_gamepad_buttons_down.contains(&(id, button)))
        {
            return false;
        }
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

        if let gilrs::EventType::ButtonReleased(button, _) = event.event {
            self.binding_gamepad_buttons_down
                .remove(&(event.id, button));
        } else if self.binding_capture {
            // DualSense triggers can emit an unmapped digital ButtonPressed(Unknown)
            // before the mapped analog trigger crosses gilrs's press threshold and
            // emits ButtonPressed(RightTrigger2). Capture accepts the first press,
            // so ignore Unknown rather than letting the duplicate signal win.
            // Unknown also cannot distinguish different unmapped physical buttons.
            if let gilrs::EventType::ButtonPressed(button, _) = event.event
                && button != Button::Unknown
                && self.binding_gamepad_buttons_down.insert((event.id, button))
            {
                let input = InputKey::Controller(button);
                if input.metadata().is_some() {
                    self.binding_presses.push(input);
                }
            }
            return;
        }

        if let gilrs::EventType::ButtonPressed(button, _) = event.event {
            self.gamepad_buttons_just_pressed.insert((event.id, button));
        }

        let (button, pressed) = match event.event {
            gilrs::EventType::ButtonPressed(button, _) => (button, true),
            gilrs::EventType::ButtonReleased(button, _) => (button, false),
            _ => return,
        };
        for action in self.bound_actions(InputKey::Controller(button)) {
            match action {
                // TODO: gamepad spectator-menu support (vanilla has none).
                Action::NextHotbarSlot if pressed && !self.spectator && !self.menu_capture => {
                    self.selected_slot = (self.selected_slot + 1) % 9;
                }
                Action::PreviousHotbarSlot if pressed && !self.spectator && !self.menu_capture => {
                    self.selected_slot = (self.selected_slot + 8) % 9;
                }
                Action::Destroy
                | Action::Use
                | Action::ToggleInventory
                | Action::OpenMenu
                | Action::ChangePerspective
                | Action::OpenChat
                | Action::Close => {
                    self.recent_actions.insert(action, pressed);
                }
                Action::PickBlock => self.middle_click.set(pressed),
                Action::SpectatorHotbar if pressed => self.click_spectator_hotbar(),
                _ => {}
            }
        }
    }

    /// Every action bound to `input`: vanilla `KeyMapping.click` and `set`
    /// reach all the mappings on a key, not just the first.
    fn bound_actions(&self, input: InputKey) -> Vec<Action> {
        self.key_bindings
            .actions_for_input(&input)
            .copied()
            .collect()
    }

    /// Queues one press of `action`, vanilla `KeyMapping.click`.
    fn click(&mut self, action: Action) {
        *self.click_counts.entry(action).or_insert(0) += 1;
    }

    fn click_spectator_hotbar(&mut self) {
        if self.spectator && !self.menu_capture {
            self.click(Action::SpectatorHotbar);
        }
    }

    /// Whether any input bound to `action` is held.
    pub fn action_held(&self, action: Action) -> bool {
        self.key_bindings
            .inputs_for_action(action)
            .any(|input| self.input_pressed(input))
    }

    /// Whether any input bound to `action` was pressed since `end_frame`.
    pub fn action_input_just_pressed(&self, action: Action) -> bool {
        self.key_bindings
            .inputs_for_action(action)
            .any(|input| self.input_just_pressed(input))
    }

    pub fn performing_action(&self, action: Action) -> bool {
        if self.touch_actions_down.contains(&action) {
            return true;
        }
        match action {
            Action::Jump => self.action_held(action) || self.touch.jump_held(),
            Action::ToggleInventory => {
                self.action_just_pressed(action) || self.bound_gamepad_button_down(action)
            }
            // Controller-only; keyboard Escape closes via OpenMenu and the chat path.
            Action::Close => self.bound_gamepad_button_down(action),
            // DropItem and SwapOffhand are click-count driven (`consume_click`);
            // this is their held state only.
            Action::Sneak
            | Action::Sprint
            | Action::Destroy
            | Action::Use
            | Action::OpenMenu
            | Action::ViewPlayerList
            | Action::ChangePerspective
            | Action::OpenChat
            | Action::OpenCommands
            | Action::DropItem
            | Action::SwapOffhand
            | Action::SpectatorHotbar => self.action_held(action),
            // Other actions do not have a held-state query here.
            _ => false,
        }
    }

    fn bound_gamepad_button_down(&self, action: Action) -> bool {
        self.key_bindings
            .inputs_for_action(action)
            .any(|input| match input {
                InputKey::Controller(button) => self.gamepad_button_down(*button),
                _ => false,
            })
    }

    /// Whether the physical key or button is currently held.
    pub fn input_pressed(&self, input: &InputKey) -> bool {
        match input {
            InputKey::Keyboard(key) => self.key_pressed(*key),
            InputKey::Mouse(button) => self.mouse_buttons_down.contains(button),
            InputKey::Controller(button) => self.gamepad_button_down(*button),
        }
    }

    /// Physical presses since `end_frame`, including keyboard repeats.
    pub fn input_just_pressed(&self, input: &InputKey) -> bool {
        match input {
            InputKey::Keyboard(key) => self.key_just_pressed(*key),
            InputKey::Mouse(button) => self.mouse_buttons_just_pressed.contains(button),
            InputKey::Controller(button) => self
                .active_gamepad_id
                .is_some_and(|id| self.gamepad_buttons_just_pressed.contains(&(id, *button))),
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

        // A release in the same pass as its press would overwrite it.
        for release in std::mem::take(&mut self.tap_releases) {
            self.touch_actions_down.remove(&release);
        }
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

    /// The touch stick's or gamepad left stick's movement, touch first.
    pub fn analog_movement_axes(&self) -> Option<glam::Vec2> {
        self.touch
            .stick()
            .or_else(|| self.get_gamepad_left_analog())
            .map(gamepad_movement_axes)
    }

    pub fn get_gamepad_right_analog(&self) -> Option<glam::Vec2> {
        self.gamepad_stick(gilrs::Axis::RightStickX, gilrs::Axis::RightStickY)
    }

    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    #[cfg(test)]
    pub(crate) fn set_key_pressed_for_test(&mut self, key: KeyCode, pressed: bool) {
        if pressed {
            self.pressed.insert(key);
        } else {
            self.pressed.remove(&key);
        }
    }

    /// Pressed since the last `end_frame`, OS key repeats included (vanilla
    /// screens and debug chords receive GLFW repeat events).
    /// The drop key this frame, for screens that throw items themselves
    /// (vanilla `AbstractContainerScreen.keyPressed`'s `keyDrop`).
    pub fn drop_key_just_pressed(&self) -> bool {
        self.action_input_just_pressed(Action::DropItem)
    }

    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        self.just_pressed.contains(&key)
    }

    /// Hotbar digit pressed since the last `end_frame`, for container swaps.
    pub fn hotbar_key_just_pressed(&self) -> Option<u8> {
        // `AbstractContainerScreen.checkHotbarKeyPressed` takes the first.
        (0..9).find(|&slot| self.action_input_just_pressed(HOTBAR_SLOTS[slot as usize]))
    }

    /// Consume one queued press of `action`, vanilla `KeyMapping.consumeClick`.
    pub fn consume_click(&mut self, action: Action) -> bool {
        match self.click_counts.get_mut(&action) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => false,
        }
    }

    pub fn clear_click_counts(&mut self) {
        self.click_counts.clear();
        self.spectator_slot_presses.clear();
    }

    /// Digit presses queued for the spectator menu since the last tick.
    pub fn take_spectator_slot_presses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.spectator_slot_presses)
    }

    /// Frame-scoped input state; called once at the end of each redraw so a
    /// latch set outside its consumer's screen can't fire later.
    pub fn end_frame(&mut self) {
        self.just_pressed.clear();
        self.mouse_buttons_just_pressed.clear();
        self.gamepad_buttons_just_pressed.clear();
        #[cfg(feature = "test-harness")]
        {
            self.harness_ctrl = false;
            if let Some((code, ctrl)) = self.harness_press.take() {
                self.harness_ctrl = ctrl;
                self.on_key(code, ElementState::Pressed, false);
                self.on_key(code, ElementState::Released, false);
            }
        }
        self.up_pressed = false;
        self.down_pressed = false;
        self.page_up_pressed = false;
        self.page_down_pressed = false;
        self.text_events.clear();
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

    pub fn on_key_event(&mut self, event: &KeyEvent) {
        if let Some(code) = key_code(event) {
            self.on_key(code, event.state, event.repeat);
        }
    }

    /// Presses and releases `code` for the next frame, as a real key event
    /// arriving between frames would, with Ctrl held for that frame when
    /// `ctrl` is set. The harness runs after the frame's update, so pressing
    /// now would be cleared before anything read it.
    #[cfg(feature = "test-harness")]
    pub fn press_key(&mut self, code: KeyCode, ctrl: bool) {
        self.harness_press = Some((code, ctrl));
    }

    fn on_key(&mut self, code: KeyCode, state: ElementState, repeat: bool) {
        if self.captures_key(code) {
            self.capture_keyboard_input(code, state, repeat);
            return;
        }
        match state {
            ElementState::Pressed => {
                self.pressed.insert(code);
                self.just_pressed.insert(code);
                let actions = self.bound_actions(InputKey::Keyboard(code));
                // Vanilla `handleKeybinds` visits slots in order: the last
                // selects, and in spectator each one reaches the menu.
                if !self.text_capture && !self.menu_capture {
                    for slot in hotbar_slots(&actions) {
                        self.select_hotbar(slot);
                    }
                }
                let gameplay = !self.text_capture && !self.menu_capture;
                for action in actions {
                    match action {
                        Action::ToggleInventory if !self.text_capture => {
                            self.recent_actions.insert(action, true);
                        }
                        Action::DropItem | Action::SwapOffhand if gameplay => {
                            self.click(action);
                        }
                        // Screen cancellation follows Escape's route, including
                        // recipe-book interception, without opening pause in play.
                        Action::OpenMenu => {
                            self.recent_actions.insert(Action::OpenMenu, true);
                        }
                        Action::Close if self.menu_capture => {
                            self.recent_actions.insert(Action::OpenMenu, true);
                        }
                        Action::ChangePerspective | Action::OpenChat | Action::OpenCommands => {
                            self.recent_actions.insert(action, true);
                        }
                        Action::PickBlock if gameplay => self.middle_click.set(true),
                        Action::SpectatorHotbar if !self.text_capture => {
                            self.click_spectator_hotbar();
                        }
                        _ => {}
                    }
                }
            }
            ElementState::Released => {
                if self.key_bindings.matches_key(Action::PickBlock, code) {
                    self.middle_click.set(false);
                }
                self.pressed.remove(&code);
            }
        }
    }

    pub fn binding_capture_active(&self) -> bool {
        self.binding_capture
    }

    /// Binding editing only: do not turn the press that opened the editor into
    /// a binding, or replay pending menu input after capture ends.
    pub fn set_binding_capture(&mut self, active: bool) {
        if self.binding_capture == active {
            return;
        }
        self.binding_capture = active;
        self.binding_presses.clear();
        if active {
            self.clear_just_pressed_actions();
            self.clear_click_counts();
            self.just_pressed.clear();
            self.mouse_buttons_just_pressed.clear();
            self.gamepad_buttons_just_pressed.clear();
            self.text_events.clear();
            self.enter_pressed = false;
            self.escape_pressed = false;
            self.tab_pressed = false;
            self.f5_pressed = false;
            self.up_pressed = false;
            self.down_pressed = false;
            self.page_up_pressed = false;
            self.page_down_pressed = false;
        }
    }

    pub fn captures_key_event(&self, event: &KeyEvent) -> bool {
        key_code(event).is_some_and(|code| self.captures_key(code)) || self.binding_capture
    }

    fn captures_key(&self, code: KeyCode) -> bool {
        self.binding_capture || self.binding_keys_down.contains(&code)
    }

    fn capture_keyboard_input(&mut self, code: KeyCode, state: ElementState, repeat: bool) {
        if state.is_pressed() {
            let new_press = self.binding_keys_down.insert(code);
            if self.binding_capture && new_press && !repeat {
                let input = InputKey::Keyboard(code);
                if input.metadata().is_some() {
                    self.binding_presses.push(input);
                }
            }
        } else {
            self.binding_keys_down.remove(&code);
            self.pressed.remove(&code);
        }
    }

    pub fn take_binding_presses(&mut self) -> Vec<InputKey> {
        std::mem::take(&mut self.binding_presses)
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    pub fn on_menu_key_event(&mut self, event: &winit::event::KeyEvent) {
        if self.captures_key_event(event) || !event.state.is_pressed() {
            return;
        }

        self.on_menu_key_press(key_code(event), event.text.as_deref());
    }

    // Normalized press data keeps the screen route independent of winit's
    // platform-private KeyEvent fields.
    fn on_menu_key_press(&mut self, code: Option<KeyCode>, text: Option<&str>) {
        // The ordered event stream for caret-editing text fields. Chars are
        // suppressed while the edit modifier is held, like GLFW's char
        // callback for Ctrl (Cmd) chords.
        if let Some(code) = code {
            self.text_events
                .push(crate::ui::text_edit::TextInputEvent::Key {
                    code,
                    mods: self.key_mods(),
                });
        }
        let state = self.modifiers.state();
        if let Some(text) = text
            && !state.control_key()
            && !state.super_key()
        {
            for ch in text.chars() {
                if !ch.is_control() {
                    self.text_events
                        .push(crate::ui::text_edit::TextInputEvent::Char(ch));
                }
            }
        }

        if let Some(code) = code {
            match code {
                _ if self.key_bindings.matches_key(Action::Close, code) => {
                    self.escape_pressed = true
                }
                KeyCode::Enter | KeyCode::NumpadEnter => self.enter_pressed = true,
                KeyCode::Tab => self.tab_pressed = true,
                KeyCode::F5 => self.f5_pressed = true,
                // Chat history/scroll keys; latched here so OS key repeat
                // works like vanilla.
                KeyCode::ArrowUp => self.up_pressed = true,
                KeyCode::ArrowDown => self.down_pressed = true,
                KeyCode::PageUp => self.page_up_pressed = true,
                KeyCode::PageDown => self.page_down_pressed = true,
                _ => {}
            }
        }
    }

    pub fn drain_text_events(&mut self) -> Vec<crate::ui::text_edit::TextInputEvent> {
        std::mem::take(&mut self.text_events)
    }

    pub fn key_mods(&self) -> crate::ui::text_edit::KeyMods {
        let state = self.modifiers.state();
        crate::ui::text_edit::KeyMods {
            shift: state.shift_key(),
            ctrl: state.control_key(),
            alt: state.alt_key(),
            super_key: state.super_key(),
        }
    }

    pub fn consume_menu_scroll(&mut self) -> f32 {
        self.consume_menu_scroll_xy().1
    }

    pub fn consume_menu_scroll_xy(&mut self) -> (f32, f32) {
        let scroll = (self.menu_scroll_x, self.menu_scroll);
        self.menu_scroll_x = 0.0;
        self.menu_scroll = 0.0;
        scroll
    }

    pub fn on_menu_scroll_xy(&mut self, x: f32, y: f32) {
        self.menu_scroll_x += x;
        self.menu_scroll += y;
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

    pub fn ctrl_held(&self) -> bool {
        #[cfg(feature = "test-harness")]
        if self.harness_ctrl {
            return true;
        }
        self.modifiers.state().control_key()
    }

    pub fn f5_pressed(&mut self) -> bool {
        std::mem::take(&mut self.f5_pressed)
    }

    pub fn up_pressed(&mut self) -> bool {
        std::mem::take(&mut self.up_pressed)
    }

    pub fn down_pressed(&mut self) -> bool {
        std::mem::take(&mut self.down_pressed)
    }

    pub fn page_up_pressed(&mut self) -> bool {
        std::mem::take(&mut self.page_up_pressed)
    }

    pub fn page_down_pressed(&mut self) -> bool {
        std::mem::take(&mut self.page_down_pressed)
    }

    pub fn selected_slot(&self) -> u8 {
        self.selected_slot
    }

    /// The server's held slot; the handler has already checked it is a hotbar
    /// index.
    pub fn set_selected_slot(&mut self, slot: u8) {
        self.selected_slot = slot;
    }

    pub fn on_scroll(&mut self, delta: f32) {
        if self.menu_capture {
            return;
        }
        if delta > 0.0 {
            self.selected_slot = (self.selected_slot + 8) % 9;
        } else if delta < 0.0 {
            self.selected_slot = (self.selected_slot + 1) % 9;
        }
    }

    /// `gameplay`: the world has the input (no screen, chat or pause menu).
    pub fn on_touch(&mut self, touch: &Touch, gameplay: bool, layout: &TouchLayout) {
        let p = glam::vec2(touch.location.x as f32, touch.location.y as f32);
        let out = self.touch.on_touch(
            touch.id,
            touch.phase,
            p,
            gameplay,
            layout,
            std::time::Instant::now(),
        );
        self.apply_touch(out);
    }

    /// Vanilla `handleKeybinds`: in spectator a hotbar slot drives the
    /// spectator menu and never touches the carried slot.
    fn select_hotbar(&mut self, slot: u8) {
        if self.spectator {
            self.spectator_slot_presses.push(slot);
        } else {
            self.selected_slot = slot;
        }
    }

    pub fn touch(&self) -> &TouchControls {
        &self.touch
    }

    /// Gameplay gestures target actions independently of physical bindings;
    /// screen touches remain pointer events.
    fn apply_touch(&mut self, out: Vec<TouchOutput>) {
        for output in out {
            match output {
                TouchOutput::Look(delta) => self.on_mouse_motion((delta.x.into(), delta.y.into())),
                TouchOutput::PointerDown => {
                    self.on_mouse_button(MouseButton::Left, ElementState::Pressed);
                }
                TouchOutput::PointerUp => {
                    self.on_mouse_button(MouseButton::Left, ElementState::Released);
                }
                TouchOutput::DestroyStart => {
                    self.touch_actions_down.insert(Action::Destroy);
                    self.recent_actions.insert(Action::Destroy, true);
                }
                TouchOutput::DestroyEnd | TouchOutput::DestroyCancel => {
                    self.touch_actions_down.remove(&Action::Destroy);
                    self.recent_actions.remove(&Action::Destroy);
                }
                TouchOutput::Use => {
                    self.touch_actions_down.insert(Action::Use);
                    self.recent_actions.insert(Action::Use, true);
                    self.tap_releases.push(Action::Use);
                }
                TouchOutput::Tap(action) => {
                    self.touch_actions_down.insert(action);
                    self.recent_actions.insert(action, true);
                    self.tap_releases.push(action);
                }
                TouchOutput::Hotbar(slot) => self.select_hotbar(slot),
                TouchOutput::PointerMove(p) => self.on_cursor_moved(p.x, p.y),
            }
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
        if self.binding_capture && state.is_pressed() {
            let input = InputKey::Mouse(button);
            if input.metadata().is_some() {
                self.binding_presses.push(input);
            }
            return;
        }
        let was_pressed = match state {
            ElementState::Pressed => true,
            ElementState::Released => false,
        };

        if was_pressed {
            self.mouse_buttons_down.insert(button);
            self.mouse_buttons_just_pressed.insert(button);
        } else {
            self.mouse_buttons_down.remove(&button);
        }
        match button {
            MouseButton::Left => self.left_click.set(was_pressed),
            MouseButton::Right => self.right_click.set(was_pressed),
            _ => (),
        }

        for action in self.bound_actions(InputKey::Mouse(button)) {
            match action {
                Action::Destroy | Action::Use => {
                    self.recent_actions.insert(action, was_pressed);
                }
                Action::PickBlock => self.middle_click.set(was_pressed),
                Action::SpectatorHotbar if was_pressed => self.click_spectator_hotbar(),
                _ => {}
            }
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

/// The key's code, with Android's Back as Escape: it arrives with no physical
/// code, and closes screens and chat as Escape does.
pub fn key_code(event: &KeyEvent) -> Option<KeyCode> {
    key_code_of(&event.physical_key, &event.logical_key)
}

fn key_code_of(physical: &PhysicalKey, logical: &Key) -> Option<KeyCode> {
    match (physical, logical) {
        (PhysicalKey::Code(code), _) => Some(*code),
        (_, Key::Named(NamedKey::BrowserBack)) => Some(KeyCode::Escape),
        _ => None,
    }
}

/// The hotbar actions, slot 0 first.
const HOTBAR_SLOTS: [Action; 9] = [
    Action::Hotbar1,
    Action::Hotbar2,
    Action::Hotbar3,
    Action::Hotbar4,
    Action::Hotbar5,
    Action::Hotbar6,
    Action::Hotbar7,
    Action::Hotbar8,
    Action::Hotbar9,
];

/// The hotbar slots among `actions`, in slot order.
fn hotbar_slots(actions: &[Action]) -> impl Iterator<Item = u8> + '_ {
    (0..9u8).filter(|&slot| actions.contains(&HOTBAR_SLOTS[slot as usize]))
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

#[cfg(test)]
mod binding_capture_tests {
    use super::*;

    #[test]
    fn fixed_catalog_escape_preserves_legacy_pause_routing() {
        assert!(
            Action::OpenMenu
                .fixed_inputs()
                .contains(&InputKey::Keyboard(KeyCode::Escape))
        );
        assert!(
            Action::OpenMenu
                .defaults()
                .iter()
                .all(|input| !matches!(input, InputKey::Keyboard(_)))
        );
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::Escape, true);
        assert!(input.performing_action(Action::OpenMenu));
        assert!(!input.performing_action(Action::Close));
        input.set_key_pressed_for_test(KeyCode::Escape, false);
        assert!(!input.performing_action(Action::OpenMenu));
    }

    #[test]
    fn a_use_tap_is_seen_before_it_releases() {
        let mut input = InputState::released();
        input.apply_touch(vec![TouchOutput::Use]);
        assert!(input.action_just_pressed(Action::Use));
        assert!(input.performing_action(Action::Use));

        input.clear_just_pressed_actions();
        assert!(!input.performing_action(Action::Use));
    }

    #[test]
    fn keyboard_capture_suppresses_repeats_until_release() {
        let mut input = InputState::released();
        input.set_binding_capture(true);
        input.capture_keyboard_input(KeyCode::F3, ElementState::Pressed, false);
        input.capture_keyboard_input(KeyCode::F3, ElementState::Pressed, true);
        let presses = input.take_binding_presses();
        assert_eq!(presses.len(), 1);
        assert!(matches!(presses[0], InputKey::Keyboard(KeyCode::F3)));
        assert!(!input.key_pressed(KeyCode::F3));
        assert!(!input.key_just_pressed(KeyCode::F3));

        input.set_binding_capture(false);
        assert!(input.binding_keys_down.contains(&KeyCode::F3));
        input.capture_keyboard_input(KeyCode::F3, ElementState::Pressed, true);
        assert!(input.take_binding_presses().is_empty());
        input.capture_keyboard_input(KeyCode::F3, ElementState::Released, false);
        assert!(!input.binding_keys_down.contains(&KeyCode::F3));
    }

    #[test]
    fn mouse_capture_has_no_normal_clicks_and_cancellation_discards_presses() {
        let mut input = InputState::released();
        input.set_binding_capture(true);
        for button in [
            MouseButton::Left,
            MouseButton::Middle,
            MouseButton::Other(7),
        ] {
            input.on_mouse_button(button, ElementState::Pressed);
        }
        let presses = input.take_binding_presses();
        assert_eq!(presses.len(), 3);
        assert!(matches!(presses[2], InputKey::Mouse(MouseButton::Other(7))));
        assert!(!input.left_just_pressed());
        assert!(!input.left_held());
        assert!(!input.middle_just_pressed());
        assert!(input.recent_actions.is_empty());

        input.on_mouse_button(MouseButton::Right, ElementState::Pressed);
        input.set_binding_capture(false);
        assert!(input.take_binding_presses().is_empty());
        input.on_mouse_button(MouseButton::Left, ElementState::Pressed);
        assert!(input.left_just_pressed());
    }
}

#[cfg(test)]
mod key_code_tests {
    use winit::keyboard::{Key, KeyCode, NamedKey, NativeKey, NativeKeyCode, PhysicalKey};

    use super::key_code_of;

    #[test]
    fn android_back_is_escape() {
        let unidentified = PhysicalKey::Unidentified(NativeKeyCode::Android(4));
        assert_eq!(
            key_code_of(
                &PhysicalKey::Code(KeyCode::KeyT),
                &Key::Character("t".into())
            ),
            Some(KeyCode::KeyT)
        );
        assert_eq!(
            key_code_of(&unidentified, &Key::Named(NamedKey::BrowserBack)),
            Some(KeyCode::Escape)
        );
        assert_eq!(
            key_code_of(&unidentified, &Key::Unidentified(NativeKey::Android(4))),
            None
        );
    }
}

#[cfg(test)]
mod binding_lookup_tests {
    use super::*;

    #[test]
    fn configured_close_routes_captured_keys_like_escape() {
        // Captured Close uses Escape's OpenMenu action, not controller Close.
        for text_capture in [false, true] {
            for code in [KeyCode::Escape, KeyCode::KeyJ] {
                let mut input = InputState::released();
                input.menu_capture = true;
                input.text_capture = text_capture;
                input
                    .key_bindings
                    .set(Action::Close, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
                input.on_key(code, ElementState::Pressed, false);
                input.on_key(code, ElementState::Released, false);
                assert!(input.action_just_pressed(Action::OpenMenu), "{code:?}");
                assert!(!input.action_just_pressed(Action::Close));
            }
        }

        // Cancel must not become a second pause binding during gameplay.
        let mut input = InputState::released();
        input
            .key_bindings
            .set(Action::Close, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert!(!input.action_just_pressed(Action::OpenMenu));
    }

    #[test]
    fn configured_enter_close_precedes_menu_activation() {
        use crate::ui::menu::MenuInput;

        for code in [KeyCode::Enter, KeyCode::NumpadEnter] {
            for bound in [false, true] {
                let mut input = InputState::released();
                if bound {
                    input
                        .key_bindings
                        .set(Action::Close, vec![InputKey::Keyboard(code)]);
                }
                input.on_key(code, ElementState::Pressed, false);
                input.on_menu_key_press(Some(code), Some("\r"));
                let menu_input = MenuInput {
                    enter: input.enter_pressed(),
                    escape: input.escape_pressed(),
                    events: input.drain_text_events(),
                    ..MenuInput::backdrop()
                };
                assert_eq!(menu_input.activate(), !bound, "{code:?}, bound={bound}");
                assert_eq!(menu_input.escape, bound);
                assert_eq!(menu_input.events.len(), 1, "retain the text-edit key event");

                // Escape remains fixed even after Close is rebound to Enter.
                input.on_menu_key_press(Some(KeyCode::Escape), None);
                assert!(input.escape_pressed());
                assert!(!input.enter_pressed());
            }
        }
    }

    #[test]
    fn rebound_hotbar_routes_selection_spectator_and_captured_presses() {
        let mut input = InputState::released();
        input
            .key_bindings
            .set(Action::Hotbar3, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        input.on_key(KeyCode::Digit3, ElementState::Pressed, false);
        assert_eq!(input.selected_slot(), 0);
        input.on_key(KeyCode::Digit3, ElementState::Released, false);
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert_eq!(input.selected_slot(), 2);
        input.on_key(KeyCode::KeyJ, ElementState::Released, false);
        input.end_frame();

        input.set_selected_slot(0);
        input.spectator = true;
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert_eq!(input.selected_slot(), 0);
        assert_eq!(input.take_spectator_slot_presses(), [2]);
        input.on_key(KeyCode::KeyJ, ElementState::Released, false);
        input.end_frame();

        input.menu_capture = true;
        assert_eq!(input.hotbar_key_just_pressed(), None);
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert!(input.take_spectator_slot_presses().is_empty());
        assert_eq!(input.selected_slot(), 0);
        assert_eq!(input.hotbar_key_just_pressed(), Some(2));
    }

    #[test]
    fn touch_actions_survive_rebinding_without_leaking_pointer_input() {
        let mut input = InputState::released();
        input.key_bindings.set(Action::Destroy, Vec::new());
        input.key_bindings.set(Action::ToggleInventory, Vec::new());
        input.apply_touch(vec![TouchOutput::DestroyStart]);
        assert!(input.performing_action(Action::Destroy));
        assert!(input.action_just_pressed(Action::Destroy));
        assert!(!input.left_just_pressed());
        assert!(!input.left_held());
        input.clear_just_pressed_actions();
        assert!(input.performing_action(Action::Destroy));
        input.apply_touch(vec![TouchOutput::DestroyCancel]);
        assert!(!input.performing_action(Action::Destroy));
        assert!(!input.left_click.just_released);

        input.apply_touch(vec![TouchOutput::Tap(Action::ToggleInventory)]);
        assert!(input.performing_action(Action::ToggleInventory));
        assert!(input.action_just_pressed(Action::ToggleInventory));
        input.clear_just_pressed_actions();
        assert!(!input.performing_action(Action::ToggleInventory));
        assert!(!input.action_just_pressed(Action::ToggleInventory));
        input.apply_touch(vec![TouchOutput::PointerDown]);
        assert!(input.left_just_pressed());
        input.apply_touch(vec![TouchOutput::PointerUp]);
        assert!(input.left_click.just_released);
    }

    #[test]
    fn rebound_drop_routes_gameplay_clicks_and_captured_key_presses() {
        let mut input = InputState::released();
        input
            .key_bindings
            .set(Action::DropItem, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        input.on_key(KeyCode::KeyQ, ElementState::Pressed, false);
        assert!(!input.drop_key_just_pressed());
        assert!(!input.consume_click(Action::DropItem));
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert!(input.drop_key_just_pressed());
        assert!(input.consume_click(Action::DropItem));
        input.end_frame();
        input.menu_capture = true;
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, true);
        assert!(input.drop_key_just_pressed());
        assert!(!input.consume_click(Action::DropItem));
    }

    #[test]
    fn rebound_mouse_actions_separate_ui_clicks_and_action_lifetimes() {
        let mut input = InputState::released();
        let button = MouseButton::Other(7);
        let mouse = InputKey::Mouse(button);
        input.key_bindings.set(Action::Destroy, vec![mouse]);
        input.key_bindings.set(Action::PickBlock, vec![mouse]);
        input.on_mouse_button(MouseButton::Left, ElementState::Pressed);
        assert!(input.left_just_pressed());
        assert!(!input.action_just_pressed(Action::Destroy));
        assert!(!input.middle_just_pressed());
        input.on_mouse_button(MouseButton::Left, ElementState::Released);
        input.clear_just_pressed_actions();

        input.on_mouse_button(button, ElementState::Pressed);
        assert!(input.action_just_pressed(Action::Destroy));
        assert!(!input.left_just_pressed());
        input.on_mouse_button(button, ElementState::Released);
        assert!(!input.action_just_pressed(Action::Destroy));
        assert!(!input.input_pressed(&mouse));
        assert!(input.input_just_pressed(&mouse));
        input.end_frame();
        assert!(!input.input_just_pressed(&mouse));
        // A short Pick click survives the frame, but not action/tick clearing.
        assert!(input.middle_just_pressed());
        input.clear_just_pressed_actions();
        assert!(!input.middle_just_pressed());
    }

    #[test]
    fn a_key_bound_to_two_actions_fires_both() {
        let mut input = InputState::released();
        let f = InputKey::Keyboard(KeyCode::KeyF);
        input.key_bindings.set(Action::DropItem, vec![f]);
        input.key_bindings.set(Action::SwapOffhand, vec![f]);
        input.on_key(KeyCode::KeyF, ElementState::Pressed, false);
        assert!(input.consume_click(Action::DropItem));
        assert!(input.consume_click(Action::SwapOffhand));
    }

    #[test]
    fn a_key_bound_to_two_hotbar_slots_selects_the_later_one() {
        let mut input = InputState::released();
        let j = InputKey::Keyboard(KeyCode::KeyJ);
        input.key_bindings.set(Action::Hotbar3, vec![j]);
        input.key_bindings.set(Action::Hotbar5, vec![j]);
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert_eq!(input.selected_slot(), 4);
        input.on_key(KeyCode::KeyJ, ElementState::Released, false);

        input.spectator = true;
        input.on_key(KeyCode::KeyJ, ElementState::Pressed, false);
        assert_eq!(input.take_spectator_slot_presses(), [2, 4]);
    }

    #[test]
    fn pick_block_and_spectator_hotbar_follow_a_keyboard_binding() {
        let mut input = InputState::released();
        let k = InputKey::Keyboard(KeyCode::KeyK);
        input.key_bindings.set(Action::PickBlock, vec![k]);
        input.on_key(KeyCode::KeyK, ElementState::Pressed, false);
        assert!(input.middle_just_pressed());
        input.on_key(KeyCode::KeyK, ElementState::Released, false);
        input.clear_just_pressed_actions();

        input.on_mouse_button(MouseButton::Middle, ElementState::Pressed);
        assert!(!input.middle_just_pressed(), "middle no longer picks");

        input.key_bindings.set(Action::SpectatorHotbar, vec![k]);
        input.spectator = true;
        input.on_key(KeyCode::KeyK, ElementState::Pressed, false);
        assert!(input.consume_click(Action::SpectatorHotbar));
    }

    #[test]
    fn held_actions_follow_a_binding_on_another_device() {
        let mut input = InputState::released();
        let side = MouseButton::Back;
        input
            .key_bindings
            .set(Action::Jump, vec![InputKey::Mouse(side)]);
        input.on_mouse_button(side, ElementState::Pressed);
        assert!(input.performing_action(Action::Jump));
        input.on_mouse_button(side, ElementState::Released);
        assert!(!input.performing_action(Action::Jump));
    }
}
