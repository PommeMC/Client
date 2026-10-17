//! Physical input state, logical action metadata, and persistent key bindings.
//!
//! Key bindings serialize action IDs to arrays of physical input IDs, for
//! example `"key.jump": ["key.keyboard.space", "key.gamepad.south"]`.
//! The Key Binds screen edits these settings using limited input capture.
use std::collections::{HashMap, HashSet};

use gilrs::ff::{BaseEffect, Effect, EffectBuilder, Repeat, Replay};
use gilrs::{Button, GamepadId, Gilrs};
use winit::event::{ElementState, Modifiers, MouseButton};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::app::TICK_RATE_MS;
use crate::app::phases::AppPhase;
use crate::app::state_slot::StateSlot;

pub mod action;

pub mod input_key;
pub mod key_bindings;
mod metadata;

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

/// Label of key mapping `key`: Pomme's binding for the actions it has, else
/// vanilla's default (`Options` key mappings). A lang key and its en_us
/// fallback.
pub fn keybind_label(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "key.forward" => ("key.keyboard.w", "W"),
        "key.left" => ("key.keyboard.a", "A"),
        "key.back" => ("key.keyboard.s", "S"),
        "key.right" => ("key.keyboard.d", "D"),
        "key.jump" => action_label(Action::Jump)?,
        "key.sneak" => action_label(Action::Sneak)?,
        "key.sprint" => action_label(Action::Sprint)?,
        "key.inventory" => action_label(Action::ToggleInventory)?,
        "key.swapOffhand" => action_label(Action::SwapOffhand)?,
        "key.drop" => action_label(Action::DropItem)?,
        "key.use" => ("key.mouse.right", "Right Button"),
        "key.attack" => ("key.mouse.left", "Left Button"),
        "key.pickItem" | "key.spectatorHotbar" => ("key.mouse.middle", "Middle Button"),
        "key.chat" => action_label(Action::OpenChat)?,
        "key.playerlist" => action_label(Action::ViewPlayerList)?,
        "key.command" => action_label(Action::OpenCommands)?,
        "key.togglePerspective" => action_label(Action::ChangePerspective)?,
        "key.friends" => ("key.keyboard.o", "O"),
        "key.socialInteractions" => ("key.keyboard.p", "P"),
        "key.screenshot" => ("key.keyboard.f2", "F2"),
        "key.smoothCamera" | "key.spectatorOutlines" => ("key.keyboard.unknown", "Not Bound"),
        "key.fullscreen" => ("key.keyboard.f11", "F11"),
        "key.advancements" => ("key.keyboard.l", "L"),
        "key.quickActions" => ("key.keyboard.g", "G"),
        "key.toggleGui" => ("key.keyboard.f1", "F1"),
        "key.toggleSpectatorShaderEffects" => ("key.keyboard.f4", "F4"),
        "key.saveToolbarActivator" => ("key.keyboard.c", "C"),
        "key.loadToolbarActivator" => ("key.keyboard.x", "X"),
        "key.hotbar.1" => ("key.keyboard.1", "1"),
        "key.hotbar.2" => ("key.keyboard.2", "2"),
        "key.hotbar.3" => ("key.keyboard.3", "3"),
        "key.hotbar.4" => ("key.keyboard.4", "4"),
        "key.hotbar.5" => ("key.keyboard.5", "5"),
        "key.hotbar.6" => ("key.keyboard.6", "6"),
        "key.hotbar.7" => ("key.keyboard.7", "7"),
        "key.hotbar.8" => ("key.keyboard.8", "8"),
        "key.hotbar.9" => ("key.keyboard.9", "9"),
        "key.debug.overlay" | "key.debug.modifier" => ("key.keyboard.f3", "F3"),
        "key.debug.crash" | "key.debug.copyLocation" => ("key.keyboard.c", "C"),
        "key.debug.reloadChunk" => ("key.keyboard.a", "A"),
        "key.debug.showHitboxes" => ("key.keyboard.b", "B"),
        "key.debug.clearChat" => ("key.keyboard.d", "D"),
        "key.debug.showChunkBorders" => ("key.keyboard.g", "G"),
        "key.debug.showAdvancedTooltips" => ("key.keyboard.h", "H"),
        "key.debug.copyRecreateCommand" => ("key.keyboard.i", "I"),
        "key.debug.spectate" => ("key.keyboard.n", "N"),
        "key.debug.switchGameMode" => ("key.keyboard.f4", "F4"),
        "key.debug.chunkOcclusion" => action_label(Action::DebugChunkOcclusion)?,
        "key.debug.debugOptions" => ("key.keyboard.f6", "F6"),
        "key.debug.focusPause" => ("key.keyboard.p", "P"),
        "key.debug.dumpDynamicTextures" => ("key.keyboard.s", "S"),
        "key.debug.reloadResourcePacks" => ("key.keyboard.t", "T"),
        "key.debug.profiling" => ("key.keyboard.l", "L"),
        "key.debug.dumpVersion" => ("key.keyboard.v", "V"),
        "key.debug.profilingChart" => ("key.keyboard.1", "1"),
        "key.debug.fpsCharts" => ("key.keyboard.2", "2"),
        "key.debug.networkCharts" => ("key.keyboard.3", "3"),
        "key.debug.lightmapTexture" => ("key.keyboard.4", "4"),
        _ => return None,
    })
}

fn action_label(action: Action) -> Option<(&'static str, &'static str)> {
    keycode_translation(action.default_key()?)
}

fn keycode_translation(key: KeyCode) -> Option<(&'static str, &'static str)> {
    match key {
        KeyCode::KeyA => Some(("key.keyboard.a", "A")),
        KeyCode::KeyC => Some(("key.keyboard.c", "C")),
        KeyCode::KeyD => Some(("key.keyboard.d", "D")),
        KeyCode::KeyG => Some(("key.keyboard.g", "G")),
        KeyCode::KeyH => Some(("key.keyboard.h", "H")),
        KeyCode::KeyI => Some(("key.keyboard.i", "I")),
        KeyCode::KeyN => Some(("key.keyboard.n", "N")),
        KeyCode::KeyO => Some(("key.keyboard.o", "O")),
        KeyCode::KeyV => Some(("key.keyboard.v", "V")),
        KeyCode::F4 => Some(("key.keyboard.f4", "F4")),
        KeyCode::Space => Some(("key.keyboard.space", "Space")),
        KeyCode::ShiftLeft => Some(("key.keyboard.left.shift", "Left Shift")),
        KeyCode::ControlLeft => Some(("key.keyboard.left.control", "Left Control")),
        KeyCode::KeyE => Some(("key.keyboard.e", "E")),
        KeyCode::KeyF => Some(("key.keyboard.f", "F")),
        KeyCode::KeyQ => Some(("key.keyboard.q", "Q")),
        KeyCode::KeyT => Some(("key.keyboard.t", "T")),
        KeyCode::Tab => Some(("key.keyboard.tab", "Tab")),
        KeyCode::Slash => Some(("key.keyboard.slash", "/")),
        KeyCode::F5 => Some(("key.keyboard.f5", "F5")),
        _ => None,
    }
}

pub struct InputState {
    pub key_bindings: KeyBindings,
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
            key_bindings: KeyBindings::default(),
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

        if self.binding_capture {
            return false;
        }

        let mut should_apply_cursor_grab = false;

        phase.transition(|mut app| {
            if let AppPhase::InGame {
                gfx,
                connection: _connection,
                game,
                ..
            } = &mut app
            {
                // A dialog (or the confirm screen over it) is a screen, and
                // vanilla runs no key mapping while one is up.
                if self.action_just_pressed(Action::ToggleInventory) && !game.dialog_open() {
                    if game.creative_inventory_open {
                        game.close_creative_inventory();
                        should_apply_cursor_grab = true;
                    } else if game.open_container.is_some() {
                        game.close_menu();
                        should_apply_cursor_grab = true;
                    } else if !game.paused
                        && !game.dead
                        && !game.death_screen_open
                        && game.player.game_mode != 3
                        && !game.chat.is_open()
                        && game.game_mode_switcher.is_none()
                    {
                        if crate::player::is_creative(game.player.game_mode) {
                            game.creative_inventory_open = true;
                        } else {
                            game.inventory_open = !game.inventory_open;
                        }
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
                            game.close_menu();
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
                    if !game.paused
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

        match event.event {
            gilrs::EventType::ButtonPressed(button, _) => match button {
                _ if self
                    .key_bindings
                    .matches(Action::Destroy, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::Destroy, true);
                }
                // TODO: gamepad spectator-menu support (vanilla has none).
                _ if self
                    .key_bindings
                    .matches(Action::NextHotbarSlot, &InputKey::Controller(button))
                    && !self.spectator
                    && !self.menu_capture =>
                {
                    self.selected_slot = (self.selected_slot + 1) % 9;
                }
                _ if self
                    .key_bindings
                    .matches(Action::Use, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::Use, true);
                }
                _ if self
                    .key_bindings
                    .matches(Action::PreviousHotbarSlot, &InputKey::Controller(button))
                    && !self.spectator
                    && !self.menu_capture =>
                {
                    self.selected_slot = (self.selected_slot + 8) % 9;
                }
                _ if self
                    .key_bindings
                    .matches(Action::ToggleInventory, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::ToggleInventory, true);
                }

                _ if self
                    .key_bindings
                    .matches(Action::OpenMenu, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::OpenMenu, true);
                }

                _ if self
                    .key_bindings
                    .matches(Action::ChangePerspective, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::ChangePerspective, true);
                }

                _ if self
                    .key_bindings
                    .matches(Action::OpenChat, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::OpenChat, true);
                }

                _ if self
                    .key_bindings
                    .matches(Action::Close, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::Close, true);
                }

                _ => {}
            },
            gilrs::EventType::ButtonReleased(button, _) => match button {
                _ if self
                    .key_bindings
                    .matches(Action::Destroy, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::Destroy, false);
                }
                _ if self
                    .key_bindings
                    .matches(Action::Use, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::Use, false);
                }
                _ if self
                    .key_bindings
                    .matches(Action::ToggleInventory, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::ToggleInventory, false);
                }

                _ if self
                    .key_bindings
                    .matches(Action::OpenMenu, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::OpenMenu, false);
                }

                _ if self
                    .key_bindings
                    .matches(Action::ChangePerspective, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::ChangePerspective, false);
                }

                _ if self
                    .key_bindings
                    .matches(Action::OpenChat, &InputKey::Controller(button)) =>
                {
                    self.recent_actions.insert(Action::OpenChat, false);
                }

                _ if self
                    .key_bindings
                    .matches(Action::Close, &InputKey::Controller(button)) =>
                {
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
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::Sneak => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::Sprint => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::Destroy => {
                self.bound_mouse_button_down(action) || self.bound_gamepad_button_down(action)
            }
            Action::Use => {
                self.bound_mouse_button_down(action) || self.bound_gamepad_button_down(action)
            }
            Action::ToggleInventory => {
                self.action_just_pressed(Action::ToggleInventory)
                    || self.bound_gamepad_button_down(action)
            }
            Action::OpenMenu => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::ViewPlayerList => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::ChangePerspective => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::OpenChat => {
                self.bound_key_pressed(action) || self.bound_gamepad_button_down(action)
            }
            Action::OpenCommands => self.bound_key_pressed(action),
            // Controller-only; keyboard Escape closes via OpenMenu and the chat path.
            Action::Close => self.bound_gamepad_button_down(action),
            // Click-count driven (`consume_click`); held state only.
            Action::DropItem | Action::SwapOffhand => self.bound_key_pressed(action),
            // Vanilla `key.spectatorHotbar` default: middle mouse.
            Action::SpectatorHotbar => self.bound_mouse_button_down(action),
            // Other actions do not have a held-state query here.
            _ => false,
        }
    }

    pub(crate) fn bound_key_pressed(&self, action: Action) -> bool {
        self.key_bindings
            .inputs_for_action(action)
            .any(|input| match input {
                InputKey::Keyboard(key) => self.key_pressed(*key),
                _ => false,
            })
    }

    fn bound_mouse_button_down(&self, action: Action) -> bool {
        self.key_bindings
            .inputs_for_action(action)
            .any(|input| match input {
                InputKey::Mouse(button) => self.mouse_buttons_down.contains(button),
                _ => false,
            })
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

    pub fn get_gamepad_movement_axes(&self) -> Option<glam::Vec2> {
        self.get_gamepad_left_analog().map(gamepad_movement_axes)
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
    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        self.just_pressed.contains(&key)
    }

    /// Hotbar digit pressed since the last `end_frame`, for container swaps.
    pub fn hotbar_key_just_pressed(&self) -> Option<u8> {
        fn hotbar_actions() -> [Action; 9] {
            [
                Action::Hotbar1,
                Action::Hotbar2,
                Action::Hotbar3,
                Action::Hotbar4,
                Action::Hotbar5,
                Action::Hotbar6,
                Action::Hotbar7,
                Action::Hotbar8,
                Action::Hotbar9,
            ]
        }

        hotbar_actions()
            .iter()
            .position(|&action| {
                self.key_bindings
                    .inputs_for_action(action)
                    .any(|input| self.input_just_pressed(input))
            })
            .map(|slot| slot as u8)
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

    pub fn on_key_event(&mut self, event: &winit::event::KeyEvent) {
        if let PhysicalKey::Code(code) = event.physical_key {
            if self.captures_key_event(event) {
                self.capture_keyboard_input(code, event.state, event.repeat);
                return;
            }
            match event.state {
                ElementState::Pressed => {
                    self.pressed.insert(code);
                    self.just_pressed.insert(code);
                    if !self.text_capture
                        && !self.menu_capture
                        && let Some(slot) =
                            hotbar_slot(&InputKey::Keyboard(code), &self.key_bindings)
                    {
                        // Vanilla handleKeybinds: spectator digits drive the
                        // spectator menu and never touch the carried slot.
                        if self.spectator {
                            self.spectator_slot_presses.push(slot);
                        } else {
                            self.selected_slot = slot;
                        }
                    }
                    match code {
                        _ if self
                            .key_bindings
                            .matches(Action::ToggleInventory, &InputKey::Keyboard(code))
                            && !self.text_capture =>
                        {
                            self.recent_actions.insert(Action::ToggleInventory, true);
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::DropItem, &InputKey::Keyboard(code))
                            && !self.text_capture
                            && !self.menu_capture =>
                        {
                            *self.click_counts.entry(Action::DropItem).or_insert(0) += 1;
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::SwapOffhand, &InputKey::Keyboard(code))
                            && !self.text_capture
                            && !self.menu_capture =>
                        {
                            *self.click_counts.entry(Action::SwapOffhand).or_insert(0) += 1;
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::OpenMenu, &InputKey::Keyboard(code)) =>
                        {
                            self.recent_actions.insert(Action::OpenMenu, true);
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::ChangePerspective, &InputKey::Keyboard(code)) =>
                        {
                            self.recent_actions.insert(Action::ChangePerspective, true);
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::OpenChat, &InputKey::Keyboard(code)) =>
                        {
                            self.recent_actions.insert(Action::OpenChat, true);
                        }
                        _ if self
                            .key_bindings
                            .matches(Action::OpenCommands, &InputKey::Keyboard(code)) =>
                        {
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

    pub fn captures_key_event(&self, event: &winit::event::KeyEvent) -> bool {
        self.binding_capture
            || matches!(event.physical_key, PhysicalKey::Code(code) if self.binding_keys_down.contains(&code))
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

        // The ordered event stream for caret-editing text fields. Chars are
        // suppressed while the edit modifier is held, like GLFW's char
        // callback for Ctrl (Cmd) chords.
        if let PhysicalKey::Code(code) = event.physical_key {
            self.text_events
                .push(crate::ui::text_edit::TextInputEvent::Key {
                    code,
                    mods: self.key_mods(),
                });
        }
        let state = self.modifiers.state();
        if let Some(text) = &event.text
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

        if let PhysicalKey::Code(code) = event.physical_key {
            match code {
                KeyCode::Enter | KeyCode::NumpadEnter => self.enter_pressed = true,
                _ if self
                    .key_bindings
                    .matches(Action::Close, &InputKey::Keyboard(code)) =>
                {
                    self.escape_pressed = true
                }
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
            MouseButton::Left => {
                self.left_click.held = was_pressed;
                if was_pressed {
                    self.left_click.just_pressed = true;
                } else {
                    self.left_click.just_released = true;
                }
            }
            MouseButton::Right => {
                self.right_click.held = was_pressed;
                if was_pressed {
                    self.right_click.just_pressed = true;
                } else {
                    self.right_click.just_released = true;
                }
            }
            _ => (),
        }

        let input = InputKey::Mouse(button);
        if self.key_bindings.matches(Action::Destroy, &input) {
            self.recent_actions.insert(Action::Destroy, was_pressed);
        }
        if self.key_bindings.matches(Action::Use, &input) {
            self.recent_actions.insert(Action::Use, was_pressed);
        }
        if self.key_bindings.matches(Action::PickBlock, &input) {
            self.middle_click.held = was_pressed;
            if was_pressed {
                self.middle_click.just_pressed = true;
            } else {
                self.middle_click.just_released = true;
            }
        }
        if was_pressed
            && self.spectator
            && !self.menu_capture
            && self.key_bindings.matches(Action::SpectatorHotbar, &input)
        {
            *self
                .click_counts
                .entry(Action::SpectatorHotbar)
                .or_insert(0) += 1;
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

fn hotbar_slot(input: &InputKey, bindings: &KeyBindings) -> Option<u8> {
    match input {
        _ if bindings.matches(Action::Hotbar1, input) => Some(0),
        _ if bindings.matches(Action::Hotbar2, input) => Some(1),
        _ if bindings.matches(Action::Hotbar3, input) => Some(2),
        _ if bindings.matches(Action::Hotbar4, input) => Some(3),
        _ if bindings.matches(Action::Hotbar5, input) => Some(4),
        _ if bindings.matches(Action::Hotbar6, input) => Some(5),
        _ if bindings.matches(Action::Hotbar7, input) => Some(6),
        _ if bindings.matches(Action::Hotbar8, input) => Some(7),
        _ if bindings.matches(Action::Hotbar9, input) => Some(8),
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

#[cfg(test)]
mod legacy_input_tests {
    use super::*;

    #[test]
    fn fixed_catalog_escape_preserves_legacy_pause_routing() {
        assert_eq!(Action::OpenMenu.default_key(), None);
        let mut input = InputState::released();
        input.set_key_pressed_for_test(KeyCode::Escape, true);
        assert!(input.performing_action(Action::OpenMenu));
        assert!(!input.performing_action(Action::Close));
        input.set_key_pressed_for_test(KeyCode::Escape, false);
        assert!(!input.performing_action(Action::OpenMenu));
    }
}

#[cfg(test)]
mod binding_capture_tests {
    use super::*;

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
mod binding_lookup_tests {
    use super::*;

    #[test]
    fn rebound_inputs_preserve_device_checks_and_physical_frame_state() {
        let mut input = InputState::released();
        let mouse = InputKey::Mouse(MouseButton::Other(7));
        let keyboard = InputKey::Keyboard(KeyCode::KeyJ);
        input.key_bindings.set(Action::Jump, vec![mouse, keyboard]);
        input.set_key_pressed_for_test(KeyCode::Space, true);
        assert!(!input.performing_action(Action::Jump));
        input.set_key_pressed_for_test(KeyCode::KeyJ, true);
        assert!(input.performing_action(Action::Jump));
        input.set_key_pressed_for_test(KeyCode::KeyJ, false);
        input.on_mouse_button(MouseButton::Other(7), ElementState::Pressed);
        assert!(!input.performing_action(Action::Jump));
        assert!(input.input_pressed(&mouse));
        assert!(input.input_just_pressed(&mouse));
        input.end_frame();
        assert!(!input.input_just_pressed(&mouse));
        assert!(input.input_pressed(&mouse));
        assert!(!input.performing_action(Action::Jump));
        input.on_mouse_button(MouseButton::Other(7), ElementState::Released);
        assert!(!input.performing_action(Action::Jump));
        input.key_bindings.set(Action::Jump, vec![]);
        input.set_key_pressed_for_test(KeyCode::KeyJ, true);
        assert!(!input.performing_action(Action::Jump));
    }

    #[test]
    fn inventory_query_keeps_the_keyboard_press_latch_not_the_hold() {
        let key = InputKey::Keyboard(KeyCode::KeyJ);
        let mut input = InputState::released();
        input.key_bindings.set(Action::ToggleInventory, vec![key]);
        input.set_key_pressed_for_test(KeyCode::KeyJ, true);
        assert!(!input.performing_action(Action::ToggleInventory));
        input.recent_actions.insert(Action::ToggleInventory, true);
        assert!(input.performing_action(Action::ToggleInventory));
        input.clear_action(Action::ToggleInventory);
        assert!(!input.performing_action(Action::ToggleInventory));
        assert!(input.key_pressed(KeyCode::KeyJ));
    }

    #[test]
    fn rebound_mouse_action_keeps_ui_clicks_physical() {
        let mut input = InputState::released();
        input.key_bindings.set(
            Action::Destroy,
            vec![InputKey::Mouse(MouseButton::Other(7))],
        );
        input.on_mouse_button(MouseButton::Left, ElementState::Pressed);
        assert!(input.left_just_pressed());
        assert!(!input.action_just_pressed(Action::Destroy));
        input.clear_just_pressed_actions();
        input.on_mouse_button(MouseButton::Other(7), ElementState::Pressed);
        assert!(input.action_just_pressed(Action::Destroy));
        assert!(!input.left_just_pressed());
        input.on_mouse_button(MouseButton::Other(7), ElementState::Released);
        assert!(!input.action_just_pressed(Action::Destroy));
    }

    #[test]
    fn shared_mouse_binding_updates_each_matching_action() {
        let mut input = InputState::released();
        let button = MouseButton::Other(7);
        for action in [
            Action::Destroy,
            Action::Use,
            Action::PickBlock,
            Action::SpectatorHotbar,
        ] {
            input
                .key_bindings
                .set(action, vec![InputKey::Mouse(button)]);
        }
        input.spectator = true;
        input.on_mouse_button(button, ElementState::Pressed);
        assert!(input.action_just_pressed(Action::Destroy));
        assert!(input.action_just_pressed(Action::Use));
        assert!(input.middle_just_pressed());
        assert!(input.consume_click(Action::SpectatorHotbar));
        assert!(!input.consume_click(Action::SpectatorHotbar));
        assert!(!input.left_just_pressed());
        assert!(!input.right_just_pressed());

        input.on_mouse_button(button, ElementState::Released);
        assert!(!input.action_just_pressed(Action::Destroy));
        assert!(!input.action_just_pressed(Action::Use));
        assert!(!input.middle_click.held);
        assert!(input.middle_just_pressed());
        assert!(!input.consume_click(Action::SpectatorHotbar));
    }

    #[test]
    fn rebound_pick_keeps_its_press_until_the_tick_clears_it() {
        let mut input = InputState::released();
        input.key_bindings.set(
            Action::PickBlock,
            vec![InputKey::Mouse(MouseButton::Other(7))],
        );
        input.on_mouse_button(MouseButton::Middle, ElementState::Pressed);
        assert!(!input.middle_just_pressed());
        input.on_mouse_button(MouseButton::Other(7), ElementState::Pressed);
        input.on_mouse_button(MouseButton::Other(7), ElementState::Released);
        input.end_frame();
        assert!(input.middle_just_pressed());
        input.clear_just_pressed_actions();
        assert!(!input.middle_just_pressed());
    }
}
