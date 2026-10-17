use gilrs::Button;

// Directional face names avoid assuming Xbox, PlayStation, or Nintendo labels.
// TODO: We likely would ultimately want to detect the style of controller and
// allow device-specific labels/images
input_catalog! {
    input_type: Button,
    input_variant: Controller,
    id_builder: gamepad_input_id,
    // Unknown can't tell unmapped buttons apart, so it isn't bindable.
    metadata_fallback: { Button::Unknown => None, },
    decode_fallback: |_| None,
    entries: {
        South => {
            serialized_suffix: "south",
            translation_id: None,
            fallback: "South",
        },
        East => {
            serialized_suffix: "east",
            translation_id: None,
            fallback: "East",
        },
        North => {
            serialized_suffix: "north",
            translation_id: None,
            fallback: "North",
        },
        West => {
            serialized_suffix: "west",
            translation_id: None,
            fallback: "West",
        },
        C => {
            serialized_suffix: "c",
            translation_id: None,
            fallback: "Gamepad C",
        },
        Z => {
            serialized_suffix: "z",
            translation_id: None,
            fallback: "Gamepad Z",
        },
        LeftTrigger => {
            serialized_suffix: "left.trigger",
            translation_id: None,
            fallback: "Left Trigger",
        },
        LeftTrigger2 => {
            serialized_suffix: "left.trigger.2",
            translation_id: None,
            fallback: "Left Trigger 2",
        },
        RightTrigger => {
            serialized_suffix: "right.trigger",
            translation_id: None,
            fallback: "Right Trigger",
        },
        RightTrigger2 => {
            serialized_suffix: "right.trigger.2",
            translation_id: None,
            fallback: "Right Trigger 2",
        },
        Select => {
            serialized_suffix: "select",
            translation_id: None,
            fallback: "Select",
        },
        Start => {
            serialized_suffix: "start",
            translation_id: None,
            fallback: "Start",
        },
        Mode => {
            serialized_suffix: "mode",
            translation_id: None,
            fallback: "Mode",
        },
        LeftThumb => {
            serialized_suffix: "left.thumb",
            translation_id: None,
            fallback: "Left Thumb",
        },
        RightThumb => {
            serialized_suffix: "right.thumb",
            translation_id: None,
            fallback: "Right Thumb",
        },
        DPadUp => {
            serialized_suffix: "dpad.up",
            translation_id: None,
            fallback: "D-Pad Up",
        },
        DPadDown => {
            serialized_suffix: "dpad.down",
            translation_id: None,
            fallback: "D-Pad Down",
        },
        DPadLeft => {
            serialized_suffix: "dpad.left",
            translation_id: None,
            fallback: "D-Pad Left",
        },
        DPadRight => {
            serialized_suffix: "dpad.right",
            translation_id: None,
            fallback: "D-Pad Right",
        },
    }
}
