//! The Controls screen's options and the state of the keys they make toggle.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{Action, InputKey, KeyBindings};

/// `Options.sprintWindow`'s range ends here.
pub const MAX_SPRINT_WINDOW: u8 = 10;

/// `Options`' controls: whether the four `ToggleKeyMapping`s toggle, plus
/// `autoJump`, `sprintWindow` and `operatorItemsTab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlOptions {
    pub toggle_crouch: bool,
    pub toggle_sprint: bool,
    pub toggle_attack: bool,
    pub toggle_use: bool,
    pub auto_jump: bool,
    /// Ticks after letting go of forward in which pressing it again sprints;
    /// 0 turns the double tap off.
    pub sprint_window: u8,
    pub operator_items_tab: bool,
}

impl Default for ControlOptions {
    fn default() -> Self {
        Self {
            toggle_crouch: false,
            toggle_sprint: false,
            toggle_attack: false,
            toggle_use: false,
            auto_jump: false,
            sprint_window: 7,
            operator_items_tab: false,
        }
    }
}

impl ControlOptions {
    /// Whether `action`'s key toggles, which only a `ToggleKeyMapping` can.
    pub fn toggles(&self, action: Action) -> bool {
        match action {
            Action::Sneak => self.toggle_crouch,
            Action::Sprint => self.toggle_sprint,
            Action::Destroy => self.toggle_attack,
            Action::Use => self.toggle_use,
            _ => false,
        }
    }

    /// The saved options with `sprint_window` brought into its range.
    pub fn sanitized(mut self) -> Self {
        self.sprint_window = self.sprint_window.min(MAX_SPRINT_WINDOW);
        self
    }
}

/// `ToggleKeyMapping.shouldRestore`: use is the one left up after a screen.
fn restores_after_screen(action: Action) -> bool {
    action != Action::Use
}

/// Which toggling keys are down, as `ToggleKeyMapping` tracks them.
#[derive(Default)]
pub struct ToggleKeys {
    down: HashSet<Action>,
    released_by_screen: HashSet<Action>,
    screen_open: bool,
}

impl ToggleKeys {
    /// `ToggleKeyMapping.setDown(true)`: each press flips the key.
    pub fn press(&mut self, action: Action) {
        if !self.down.remove(&action) {
            self.down.insert(action);
        }
    }

    pub fn is_down(&self, action: Action) -> bool {
        self.down.contains(&action)
    }

    /// `KeyMapping.resetToggleKeys`.
    pub fn reset(&mut self) {
        self.down.clear();
    }

    /// `Gui.setScreen`: a screen opening lets go of every key
    /// (`KeyMapping.releaseAll`), and the last one closing puts back the
    /// toggled keys that restore (`restoreToggleStatesOnScreenClosed`), which
    /// are those still toggling and bound to the keyboard.
    pub fn set_screen_open(
        &mut self,
        open: bool,
        options: &ControlOptions,
        bindings: &KeyBindings,
    ) {
        if open == self.screen_open {
            return;
        }
        self.screen_open = open;
        if open {
            self.released_by_screen.extend(self.down.drain());
            return;
        }
        for action in self.released_by_screen.drain() {
            let on_keyboard = matches!(
                bindings.inputs_for_action(action).next(),
                Some(InputKey::Keyboard(_))
            );
            if restores_after_screen(action) && options.toggles(action) && on_keyboard {
                self.down.insert(action);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::event::MouseButton;
    use winit::keyboard::KeyCode;

    use super::*;

    fn toggling() -> ControlOptions {
        ControlOptions {
            toggle_crouch: true,
            toggle_sprint: true,
            toggle_attack: true,
            toggle_use: true,
            ..ControlOptions::default()
        }
    }

    #[test]
    fn only_the_four_toggle_keys_have_the_option() {
        let options = toggling();
        for action in [Action::Sneak, Action::Sprint, Action::Destroy, Action::Use] {
            assert!(options.toggles(action));
            assert!(!ControlOptions::default().toggles(action));
        }
        assert!(!options.toggles(Action::Jump));
    }

    #[test]
    fn a_saved_sprint_window_is_kept_in_range() {
        let options: ControlOptions = serde_json::from_str(r#"{"sprint_window": 200}"#).unwrap();
        assert_eq!(options.sanitized().sprint_window, MAX_SPRINT_WINDOW);
        assert!(!options.auto_jump);
    }

    #[test]
    fn each_press_flips_the_key() {
        let mut keys = ToggleKeys::default();
        keys.press(Action::Sneak);
        assert!(keys.is_down(Action::Sneak));
        keys.press(Action::Sneak);
        assert!(!keys.is_down(Action::Sneak));
    }

    #[test]
    fn a_screen_releases_the_keys_and_restores_the_keyboard_ones_but_use() {
        let mut bindings = KeyBindings::default();
        bindings.set(Action::Use, vec![InputKey::Keyboard(KeyCode::KeyU)]);
        let options = toggling();
        let mut keys = ToggleKeys::default();
        for action in [Action::Sneak, Action::Sprint, Action::Destroy, Action::Use] {
            keys.press(action);
        }
        assert_eq!(
            bindings.inputs_for_action(Action::Destroy).next(),
            Some(&InputKey::Mouse(MouseButton::Left))
        );

        keys.set_screen_open(true, &options, &bindings);
        assert!(!keys.is_down(Action::Sneak));

        // A second screen replacing the first keeps what the first released.
        keys.set_screen_open(true, &options, &bindings);
        keys.set_screen_open(false, &options, &bindings);
        assert!(keys.is_down(Action::Sneak));
        assert!(keys.is_down(Action::Sprint));
        assert!(!keys.is_down(Action::Destroy));
        assert!(!keys.is_down(Action::Use));
    }

    #[test]
    fn a_key_set_back_to_hold_in_the_screen_stays_up() {
        let bindings = KeyBindings::default();
        let mut keys = ToggleKeys::default();
        keys.press(Action::Sneak);
        keys.set_screen_open(true, &toggling(), &bindings);
        keys.set_screen_open(false, &ControlOptions::default(), &bindings);
        assert!(!keys.is_down(Action::Sneak));

        // The release it remembered is spent.
        keys.set_screen_open(true, &toggling(), &bindings);
        keys.set_screen_open(false, &toggling(), &bindings);
        assert!(!keys.is_down(Action::Sneak));
    }

    #[test]
    fn resetting_lets_go_of_every_key() {
        let mut keys = ToggleKeys::default();
        keys.press(Action::Sprint);
        keys.reset();
        assert!(!keys.is_down(Action::Sprint));
    }
}
