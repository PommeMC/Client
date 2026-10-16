// concat! requires literals rather than constants. Generate the shared constant
// and compile-time ID builder from one prefix literal to keep them in sync.
macro_rules! input_id_prefix {
    ($constant:ident, $builder:ident, $prefix:literal) => {
        pub const $constant: &str = $prefix;

        macro_rules! $builder {
            ($suffix:literal) => {
                concat!($prefix, $suffix)
            };
        }
    };
}

input_id_prefix!(KEYBOARD_INPUT_ID_PREFIX, keyboard_input_id, "key.keyboard.");
input_id_prefix!(MOUSE_INPUT_ID_PREFIX, mouse_input_id, "key.mouse.");
input_id_prefix!(GAMEPAD_INPUT_ID_PREFIX, gamepad_input_id, "key.gamepad.");

// Device-specific fallback arms preserve exhaustive matching where possible,
// while supporting non-exhaustive keyboard codes and numbered mouse buttons.
macro_rules! input_catalog {
    (
        input_type: $input_type:ident,
        input_variant: $input_variant:ident,
        id_builder: $id_builder:ident,
        metadata_fallback: { $($metadata_fallback:tt)* },
        decode_fallback: $decode_fallback:expr,
        entries: {
            $($input:ident => {
                serialized_suffix: $suffix:literal,
                translation_id: $translation_id:expr,
                fallback: $fallback:literal $(,)?
            }),* $(,)?
        }
    ) => {
        pub(super) fn input_id_from_input(input: $input_type) -> Option<std::borrow::Cow<'static, str>> {
            metadata_from_input(input).map(|metadata| metadata.serialized_id)
        }

        pub(super) fn metadata_from_input(input: $input_type) -> Option<super::BindingMetadata<'static>> {
            match input {
                $($input_type::$input => Some(super::BindingMetadata {
                    serialized_id: std::borrow::Cow::Borrowed($id_builder!($suffix)),
                    display: super::DisplayText {
                        translation_id: $translation_id,
                        fallback: std::borrow::Cow::Borrowed($fallback),
                    },
                }),)*
                $($metadata_fallback)*
            }
        }

        pub(super) fn input_from_input_id(input_id: &str) -> Option<super::InputKey> {
            let suffix = input_id.strip_prefix($id_builder!(""))?;
            let input = match suffix {
                $($suffix => Some($input_type::$input),)*
                _ => ($decode_fallback)(suffix),
            };
            input.map(super::InputKey::$input_variant)
        }
    };
}
