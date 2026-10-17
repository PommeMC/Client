use std::borrow::Cow;

use gilrs::Button;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::{BindingMetadata, DisplayText};

#[macro_use]
mod catalog;
mod gamepad;
mod keyboard;
mod mouse;

pub use catalog::{GAMEPAD_INPUT_ID_PREFIX, KEYBOARD_INPUT_ID_PREFIX, MOUSE_INPUT_ID_PREFIX};

/// Represent the binding name for a physical input.
/// Bindings serialize as stable `key.keyboard.*`, `key.mouse.*`, and
/// `key.gamepad.*` strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    Keyboard(KeyCode),
    Mouse(MouseButton),
    Controller(Button),
}

/// Distinguishes an invalid input namespace from an unknown ID in a known one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputIdError {
    InvalidInputType {
        input_id: String,
    },
    UnknownBinding {
        input_type: &'static str,
        input_id: String,
    },
}

impl std::fmt::Display for InputIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInputType { input_id } => {
                write!(formatter, "invalid input type: {input_id}")
            }
            Self::UnknownBinding {
                input_type,
                input_id,
            } => {
                write!(formatter, "unknown {input_type} binding: {input_id}")
            }
        }
    }
}

impl std::error::Error for InputIdError {}

impl InputKey {
    /// Catalog metadata shared by persistence and display. Unsupported future
    /// keyboard codes have no catalog entry.
    pub fn metadata(&self) -> Option<BindingMetadata<'static>> {
        match self {
            Self::Keyboard(key) => keyboard::metadata_from_input(*key),
            Self::Mouse(button) => mouse::metadata_from_input(*button),
            Self::Controller(button) => gamepad::metadata_from_input(*button),
        }
    }

    /// Stable serialized input ID, drawn from the same metadata as display
    /// text.
    pub fn input_id(&self) -> Option<Cow<'static, str>> {
        match self {
            Self::Keyboard(key) => keyboard::input_id_from_input(*key),
            Self::Mouse(button) => mouse::input_id_from_input(*button),
            Self::Controller(button) => gamepad::input_id_from_input(*button),
        }
    }

    /// Decode a stable input ID, distinguishing unsupported namespaces from
    /// unknown bindings within a supported namespace.
    pub fn from_input_id(input_id: &str) -> Result<Self, InputIdError> {
        let unknown_binding = |input_type| InputIdError::UnknownBinding {
            input_type,
            input_id: input_id.to_owned(),
        };
        if input_id.starts_with(MOUSE_INPUT_ID_PREFIX) {
            return mouse::input_from_input_id(input_id).ok_or_else(|| unknown_binding("mouse"));
        }
        if input_id.starts_with(GAMEPAD_INPUT_ID_PREFIX) {
            return gamepad::input_from_input_id(input_id)
                .ok_or_else(|| unknown_binding("gamepad"));
        }
        if input_id.starts_with(KEYBOARD_INPUT_ID_PREFIX) {
            return keyboard::input_from_input_id(input_id)
                .ok_or_else(|| unknown_binding("keyboard"));
        }
        Err(InputIdError::InvalidInputType {
            input_id: input_id.to_owned(),
        })
    }

    /// Display metadata independent of the serialized binding ID.
    /// Keyboard names do not reflect the active OS keyboard layout.
    pub fn display_text(&self) -> DisplayText<'static> {
        self.metadata()
            .map_or(super::NOT_BOUND, |metadata| metadata.display)
    }

    /// Translated physical-input label, or its English fallback.
    pub fn display_name(&self) -> Cow<'static, str> {
        self.display_text().resolve()
    }
}
