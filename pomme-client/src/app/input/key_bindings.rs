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
/// empty array explicitly unbinds an action. Vector order is preserved. Fixed
/// catalog inputs always apply, are never saved, and can't be bound here.
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
    /// Replace editable inputs; an empty vector leaves only fixed assignments.
    pub fn set(&mut self, action: Action, mut inputs: Vec<InputKey>) {
        inputs.retain(|input| {
            !Action::ALL
                .iter()
                .any(|owner| owner.fixed_inputs().contains(input))
        });
        self.0.insert(action, inputs);
    }

    pub fn reset(&mut self, action: Action) {
        self.set(action, action.defaults().to_vec());
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

    /// Effective bindings: configured and fixed assignments, once per action.
    #[cfg(test)]
    pub fn actions_for_input<'a>(
        &'a self,
        input: &'a InputKey,
    ) -> impl Iterator<Item = &'a Action> {
        self.0.iter().filter_map(move |(action, inputs)| {
            (inputs.contains(input) || action.fixed_inputs().contains(input)).then_some(action)
        })
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
    use std::collections::HashSet;

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

    /// Escape stays on Close and OpenMenu whatever is saved.
    fn assert_escape_fixed(bindings: &KeyBindings) {
        let actions: HashSet<_> = bindings
            .actions_for_input(&InputKey::Keyboard(KeyCode::Escape))
            .copied()
            .collect();
        assert_eq!(actions, HashSet::from([Action::Close, Action::OpenMenu]));
    }

    #[test]
    fn fixed_catalog_inputs_survive_editing_and_reset_without_duplicates() {
        let escape = InputKey::Keyboard(KeyCode::Escape);
        let replacement = InputKey::Keyboard(KeyCode::KeyC);
        assert_eq!(Action::Close.fixed_inputs(), &[escape]);
        assert_eq!(Action::OpenMenu.fixed_inputs(), &[escape]);
        assert_eq!(Action::OpenMenu.id().as_str(), "key.menu");
        assert_eq!(Action::OpenMenu.display_name(), "Open Pause Menu");
        assert!(!Action::OpenMenu.listed());
        assert_eq!(Action::Close.id().as_str(), "key.close");
        assert_eq!(Action::Close.display_name(), "Cancel/Close");
        let mut bindings = KeyBindings::default();
        for inputs in [vec![replacement], vec![], vec![escape, escape]] {
            bindings.set(Action::Close, inputs.clone());
            bindings.set(Action::OpenMenu, inputs);
            assert_escape_fixed(&bindings);
            assert!(!bindings.0[&Action::Close].contains(&escape));
            assert!(!bindings.0[&Action::OpenMenu].contains(&escape));
        }
        bindings.set(Action::Close, vec![replacement]);
        assert!(
            bindings
                .actions_for_input(&replacement)
                .any(|action| *action == Action::Close)
        );
        bindings.reset_all();
        assert!(bindings.all_default());
        assert_eq!(inputs(&bindings, Action::OpenMenu), ["key.gamepad.start"]);
        assert_eq!(inputs(&bindings, Action::Close), ["key.gamepad.east"]);
        assert_escape_fixed(&bindings);
        for action in Action::ALL {
            let fixed = action.fixed_inputs();
            assert!(fixed.iter().enumerate().all(|(index, input)| {
                !fixed[..index].contains(input) && !action.defaults().contains(input)
            }));
        }
    }

    #[test]
    fn fixed_assignments_are_reserved_and_not_serialized_as_editable_bindings() {
        let bindings = load(
            r#"{
                "key.close": ["key.keyboard.escape", "key.keyboard.c"],
                "key.menu": ["key.keyboard.escape", "key.gamepad.start"],
                "key.jump": ["key.keyboard.escape", "key.keyboard.j"]
            }"#,
        );
        assert_eq!(inputs(&bindings, Action::Close), ["key.keyboard.c"]);
        assert_eq!(inputs(&bindings, Action::Jump), ["key.keyboard.j"]);
        assert_eq!(inputs(&bindings, Action::OpenMenu), ["key.gamepad.start"]);
        assert_escape_fixed(&bindings);
        let json = serde_json::to_string(&bindings).unwrap();
        assert!(!json.contains("key.keyboard.escape"));
        let loaded = load(&json);
        assert_eq!(inputs(&loaded, Action::Close), ["key.keyboard.c"]);
        assert_eq!(inputs(&loaded, Action::OpenMenu), ["key.gamepad.start"]);
        assert_escape_fixed(&loaded);
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
    fn reset_restores_defaults() {
        let mut bindings = KeyBindings::default();
        bindings.set(Action::Jump, Vec::new());
        bindings.set(Action::DropItem, Vec::new());
        assert!(!bindings.all_default());
        bindings.reset(Action::Jump);
        assert!(bindings.is_default(Action::Jump));
        bindings.reset_all();
        assert!(bindings.all_default());
    }
}
