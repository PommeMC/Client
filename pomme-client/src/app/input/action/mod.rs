//! Logical game actions, display metadata, default inputs and UI ordering.

use std::borrow::Cow;

#[cfg(any(test, feature = "test-harness"))]
use winit::keyboard::KeyCode;

#[cfg(any(test, feature = "test-harness"))]
use super::InputKey::Keyboard;
use super::{ActionId, DisplayText, InputKey, StringId};

mod catalog;

pub use catalog::{Action, ActionCategory};

/// Display metadata and explicit UI ordering for an action category.
#[derive(Clone, Debug)]
pub struct ActionCategoryMetadata {
    pub display: DisplayText<'static>,
    pub ui_order: i32,
}

/// Persistent identity, display metadata and default inputs of an action.
#[derive(Clone, Debug)]
pub struct ActionMetadata {
    pub id: ActionId,
    pub display: DisplayText<'static>,
    pub category: ActionCategory,
    /// Shown on the Key Binds screen. Pomme-only actions vanilla handles
    /// outside its key mappings (Escape) stay off it.
    pub listed: bool,
    /// Priority within the category; equal priorities sort by localized name.
    pub ui_order: i32,
    /// Initial editable bindings, replaced by saved settings.
    pub defaults: &'static [InputKey],
    /// Permanent assignments, independent of settings and not editable in the
    /// UI.
    pub fixed_inputs: &'static [InputKey],
}

impl Action {
    /// Stable identifier used in options.json and key mapping lookups.
    pub fn id(self) -> ActionId {
        self.metadata().id
    }

    /// Translated display label, or the catalog's English fallback.
    pub fn display_name(self) -> Cow<'static, str> {
        self.metadata().display.resolve()
    }

    pub fn category(self) -> ActionCategory {
        self.metadata().category
    }

    pub fn listed(self) -> bool {
        self.metadata().listed
    }

    pub fn defaults(self) -> &'static [InputKey] {
        self.metadata().defaults
    }

    pub fn fixed_inputs(self) -> &'static [InputKey] {
        self.metadata().fixed_inputs
    }

    /// The first default or fixed keyboard key, for harness input synthesis.
    #[cfg(any(test, feature = "test-harness"))]
    pub fn default_key(self) -> Option<KeyCode> {
        self.defaults()
            .iter()
            .chain(self.fixed_inputs())
            .find_map(|input| match input {
                Keyboard(code) => Some(*code),
                _ => None,
            })
    }

    /// `KeyMapping.compareTo`: category, action priority, then localized name.
    pub fn compare_ui(self, other: Self) -> std::cmp::Ordering {
        self.category()
            .ui_order()
            .cmp(&other.category().ui_order())
            .then_with(|| self.metadata().ui_order.cmp(&other.metadata().ui_order))
            .then_with(|| self.display_name().cmp(&other.display_name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_overlay_and_modifier_sort_first_in_that_order() {
        let mut actions: Vec<_> = Action::ALL
            .iter()
            .copied()
            .filter(|action| action.listed() && action.category() == ActionCategory::Debug)
            .collect();
        actions.reverse();
        actions.sort_by(|left, right| left.compare_ui(*right));
        assert_eq!(
            &actions[..2],
            &[Action::ToggleOverlay, Action::DebugModifier]
        );
        assert!(
            actions[2..]
                .windows(2)
                .all(|pair| pair[0].display_name() <= pair[1].display_name())
        );
        assert!(Action::Hotbar1.compare_ui(Action::ToggleOverlay).is_lt());
    }
}
