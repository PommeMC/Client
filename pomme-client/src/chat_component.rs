//! Pomme's text components (`pomme-text`), resolved against the client's
//! language and key bindings.

use std::borrow::Cow;

pub use pomme_text::*;

use crate::app::input::{Action, KeyBindings};

/// The client's language and the given key bindings.
struct ClientText<'a>(&'a KeyBindings);

impl Resolver for ClientText<'_> {
    fn translate(&self, key: &str) -> Option<&str> {
        crate::lang::translate(key)
    }

    /// The bound key's name, or the translated id for an unknown mapping.
    fn keybind<'a>(&self, key: &'a str) -> Cow<'a, str> {
        match Action::from_id(key) {
            // Chat presents a single key, so use the first effective assignment.
            Some(action) => self.0.key_name(action),
            None => Cow::Borrowed(crate::lang::translate(key).unwrap_or(key)),
        }
    }
}

/// A component's text in the client's language.
pub trait ComponentText {
    /// [`Component::visit_text_with`] under the default bindings.
    fn visit_text(&self, parent: &ResolvedStyle, visitor: &mut impl FnMut(&str, &ResolvedStyle));

    /// [`Component::visit_text_with`] under the bindings in effect.
    fn visit_text_with_bindings(
        &self,
        parent: &ResolvedStyle,
        bindings: &KeyBindings,
        visitor: &mut impl FnMut(&str, &ResolvedStyle),
    );

    /// [`Component::plain_text_with`] under the default bindings.
    fn plain_text(&self) -> String;

    /// [`Component::plain_text_with`] under the bindings in effect.
    fn plain_text_with_bindings(&self, bindings: &KeyBindings) -> String;
}

impl ComponentText for Component {
    fn visit_text(&self, parent: &ResolvedStyle, visitor: &mut impl FnMut(&str, &ResolvedStyle)) {
        self.visit_text_with_bindings(parent, &KeyBindings::default(), visitor);
    }

    fn visit_text_with_bindings(
        &self,
        parent: &ResolvedStyle,
        bindings: &KeyBindings,
        visitor: &mut impl FnMut(&str, &ResolvedStyle),
    ) {
        self.visit_text_with(parent, &ClientText(bindings), visitor);
    }

    fn plain_text(&self) -> String {
        self.plain_text_with_bindings(&KeyBindings::default())
    }

    fn plain_text_with_bindings(&self, bindings: &KeyBindings) -> String {
        self.plain_text_with(&ClientText(bindings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybind_component_uses_actual_pomme_binding_label() {
        let component = Component::from_value(&serde_json::json!({"keybind":"key.jump"})).unwrap();
        assert_eq!(component.plain_text(), "Space");
        let mut rendered = String::new();
        component.visit_text(&ResolvedStyle::default(), &mut |text, _| {
            rendered.push_str(text)
        });
        assert_eq!(rendered, "Space");
    }

    #[test]
    fn keybind_component_uses_configured_inputs_in_nested_runs() {
        use winit::event::MouseButton;
        use winit::keyboard::KeyCode;

        use crate::app::input::{Action, InputKey, KeyBindings};

        let component = Component::from_value(&serde_json::json!({
            "translate": "missing.keybind.test", "fallback": "Press %s", "bold": true,
            "with": [{"keybind": "key.jump"}],
            "extra": [{"text": " or ", "extra": [{"keybind": "key.attack"}]}]
        }))
        .unwrap();
        let mut bindings = KeyBindings::default();
        bindings.set(Action::Jump, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        bindings.set(
            Action::Destroy,
            vec![
                InputKey::Mouse(MouseButton::Right),
                InputKey::Keyboard(KeyCode::KeyK),
            ],
        );
        let spans =
            crate::ui::text::format_component_spans_with_bindings(&component, [1.0; 4], &bindings);
        assert_eq!(
            crate::ui::text::span_text(&spans),
            "Press J or Right Button"
        );
        assert!(spans.iter().all(|span| span.bold));
        assert_eq!(
            component.plain_text_with_bindings(&bindings),
            "Press J or Right Button"
        );
    }

    #[test]
    fn keybind_component_unbound_and_fixed_inputs() {
        use crate::app::input::{Action, KeyBindings, NOT_BOUND};

        let mut bindings = KeyBindings::default();
        bindings.set(Action::Jump, Vec::new());
        bindings.set(Action::OpenMenu, Vec::new());
        let jump = Component::new(Content::Keybind("key.jump".into()));
        assert_eq!(
            jump.plain_text_with_bindings(&bindings),
            NOT_BOUND.resolve()
        );
        let menu = Component::new(Content::Keybind("key.menu".into()));
        assert_eq!(menu.plain_text_with_bindings(&bindings), "Escape");
    }

    #[test]
    fn keybind_component_unknown_ids_use_translation_or_raw_id() {
        for key in ["unknown.keybind.test", "key.friends", "key.debug.crash"] {
            let component = Component::new(Content::Keybind(key.into()));
            assert_eq!(
                component.plain_text(),
                crate::lang::translate(key).unwrap_or(key)
            );
        }
    }
}
