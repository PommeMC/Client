use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{Action, InputKey};

/// Borrowed view of an action and all of its configured physical inputs.
#[derive(Clone, Copy)]
pub struct ActionBindingsRef<'a> {
    pub action: Action,
    pub inputs: &'a [InputKey],
}

/// Persistent action-to-input mappings. The settings field `key_bindings`
/// serializes as `"key_bindings": { "key.jump": ["key.keyboard.space", ...] }`.
/// Missing actions use defaults; saved arrays replace those defaults, and an
/// empty array explicitly unbinds an action. Vector order is preserved.
#[derive(Clone)]
pub struct KeyBindings(HashMap<Action, Vec<InputKey>>);

impl Default for KeyBindings {
    // TODO: Migrate the live input paths to these bindings; until then the
    // defaults mirror InputState's hardcoded mouse/controller handling.
    fn default() -> Self {
        Self(
            Action::ALL
                .iter()
                .map(|&action| (action, action.defaults().to_vec()))
                .collect(),
        )
    }
}

impl Serialize for KeyBindings {
    /// Sorted by action id so options.json stays stable across saves.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut entries: Vec<_> = self
            .0
            .iter()
            .map(|(action, inputs)| {
                let ids: Vec<_> = inputs.iter().filter_map(InputKey::input_id).collect();
                (action.id().as_str(), ids)
            })
            .collect();
        entries.sort_unstable_by_key(|(id, _)| *id);
        serializer.collect_map(entries)
    }
}

impl<'de> Deserialize<'de> for KeyBindings {
    /// Lenient like vanilla `Options.load`: an unknown action or input is
    /// skipped (logged) rather than failing, so one stale entry can't reset
    /// the rest of options.json. An array whose inputs are all unknown
    /// leaves the action unbound, as vanilla's unparsable key does.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut bindings = Self::default();
        let serde_json::Value::Object(saved) = serde_json::Value::deserialize(deserializer)? else {
            tracing::warn!("Ignoring key_bindings that isn't an object");
            return Ok(bindings);
        };
        for (id, value) in saved {
            let Some(action) = Action::from_id(&id) else {
                tracing::warn!("Ignoring key binding for unknown action {id}");
                continue;
            };
            let serde_json::Value::Array(values) = value else {
                tracing::warn!("Ignoring key binding for {id} that isn't an array");
                continue;
            };
            let inputs = values
                .iter()
                .filter_map(|value| match value.as_str().map(InputKey::from_input_id) {
                    Some(Ok(input)) => Some(input),
                    Some(Err(error)) => {
                        tracing::warn!("Ignoring {error} bound to {id}");
                        None
                    }
                    None => {
                        tracing::warn!("Ignoring non-string input {value} bound to {id}");
                        None
                    }
                })
                .collect();
            bindings.set(action, inputs);
        }
        Ok(bindings)
    }
}

impl KeyBindings {
    /// Replace all inputs for an action; an empty vector explicitly unbinds it.
    pub fn set(&mut self, action: Action, inputs: Vec<InputKey>) {
        self.0.insert(action, inputs);
    }

    pub fn reset_all(&mut self) {
        *self = Self::default();
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.0
            .get(&action)
            .is_none_or(|inputs| inputs.as_slice() == action.defaults())
    }

    pub fn all_default(&self) -> bool {
        Action::ALL.iter().all(|&action| self.is_default(action))
    }

    /// Configured bindings; callers choose the UI's category and display order.
    pub fn iter(&self) -> impl Iterator<Item = ActionBindingsRef<'_>> {
        self.0.iter().map(|(&action, inputs)| ActionBindingsRef {
            action,
            inputs: inputs.as_slice(),
        })
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

    use super::*;

    fn load(json: &str) -> KeyBindings {
        serde_json::from_str(json).unwrap()
    }

    fn inputs(bindings: &KeyBindings, action: Action) -> Vec<String> {
        bindings.0[&action]
            .iter()
            .filter_map(|input| input.input_id().map(|id| id.into_owned()))
            .collect()
    }

    #[test]
    fn unknown_entries_skip_without_losing_others() {
        let bindings = load(
            r#"{
                "key.unknownAction": ["key.keyboard.a"],
                "key.jump": ["key.keyboard.unknown.thing", "key.keyboard.j", null],
                "key.sneak": null,
                "key.drop": []
            }"#,
        );
        assert_eq!(inputs(&bindings, Action::Jump), ["key.keyboard.j"]);
        assert!(bindings.is_default(Action::Sneak));
        assert!(inputs(&bindings, Action::DropItem).is_empty());
    }

    #[test]
    fn non_object_keeps_defaults() {
        assert!(load("[1, 2]").all_default());
    }

    #[test]
    fn save_is_sorted_and_round_trips() {
        let mut bindings = KeyBindings::default();
        bindings.set(Action::Jump, vec![InputKey::Keyboard(KeyCode::KeyJ)]);
        let json = serde_json::to_string(&bindings).unwrap();
        let mut ids: Vec<_> = Action::ALL
            .iter()
            .map(|action| action.id().as_str())
            .collect();
        ids.sort_unstable();
        let positions: Vec<_> = ids
            .iter()
            .map(|id| json.find(&format!("\"{id}\"")).unwrap())
            .collect();
        assert!(positions.is_sorted());

        let loaded = load(&json);
        assert_eq!(inputs(&loaded, Action::Jump), ["key.keyboard.j"]);
        assert!(
            Action::ALL
                .iter()
                .filter(|&&action| action != Action::Jump)
                .all(|&action| loaded.is_default(action))
        );
    }

    #[test]
    fn reset_all_restores_defaults() {
        let mut bindings = KeyBindings::default();
        bindings.set(Action::Jump, Vec::new());
        bindings.set(Action::DropItem, Vec::new());
        assert!(!bindings.all_default());

        bindings.reset_all();
        assert!(bindings.all_default());
    }
}
