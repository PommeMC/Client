use std::borrow::Cow;

use winit::event::MouseButton;

use super::super::{BindingMetadata, DisplayText, StringId};

input_catalog! {
    input_type: MouseButton,
    input_variant: Mouse,
    id_builder: mouse_input_id,
    metadata_fallback: {
        // TODO: Map Other(n) onto vanilla's GLFW-numbered key.mouse.6-8.
        // winit's number is platform-specific (raw X11/evdev codes, macOS
        // button numbers), so it keeps its own id for now.
        MouseButton::Other(number) => Some(BindingMetadata {
            serialized_id: Cow::Owned(format!("{}{number}", mouse_input_id!("other."))),
            display: DisplayText {
                translation_id: None,
                fallback: Cow::Owned(format!("Button {number}")),
            },
        }),
    },
    decode_fallback: |suffix: &str| suffix
        .strip_prefix("other.")
        .filter(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
        .and_then(|number| number.parse::<u16>().ok())
        .map(MouseButton::Other),
    entries: {
        Left => {
            serialized_suffix: "left",
            translation_id: Some(StringId::new("key.mouse.left")),
            fallback: "Left Button",
        },
        Right => {
            serialized_suffix: "right",
            translation_id: Some(StringId::new("key.mouse.right")),
            fallback: "Right Button",
        },
        Middle => {
            serialized_suffix: "middle",
            translation_id: Some(StringId::new("key.mouse.middle")),
            fallback: "Middle Button",
        },
        // GLFW buttons 4 and 5; vanilla labels them with the "key.mouse"
        // ("Button %s") format.
        Back => {
            serialized_suffix: "4",
            translation_id: None,
            fallback: "Button 4",
        },
        Forward => {
            serialized_suffix: "5",
            translation_id: None,
            fallback: "Button 5",
        },
    }
}
