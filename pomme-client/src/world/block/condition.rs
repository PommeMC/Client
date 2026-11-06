//! Vanilla multipart selectors, evaluated while building render tables.

use super::PropMap;

/// Missing `when` applies unconditionally; property objects imply AND.
#[derive(Clone, Debug)]
pub enum Condition {
    Always,
    /// `KeyValueCondition.Terms`: the property matches any of its terms.
    Property {
        name: String,
        terms: Box<[Term]>,
    },
    And(Vec<Condition>),
    Or(Vec<Condition>),
}

/// `KeyValueCondition.Term`: a value, matched or (with a leading `!`) not.
#[derive(Clone, Debug)]
pub struct Term {
    value: String,
    negated: bool,
}

/// Invalid pack selectors must not silently become unconditional parts.
#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("multipart condition must be a nonempty object")]
    ExpectedObject,
    #[error("a combiner condition takes exactly one of AND and OR")]
    CombinerSize,
    #[error("{0} requires an array of conditions")]
    ExpectedArray(&'static str),
    #[error("invalid multipart property value for {0}")]
    InvalidValue(String),
}

impl Condition {
    /// Vanilla `Condition.CODEC`: a combiner (`AND` or `OR` over a list) is
    /// tried first, reading only those keys, then a property map.
    pub fn parse(when: Option<&serde_json::Value>) -> Result<Self, ConditionError> {
        let Some(when) = when else {
            return Ok(Self::Always);
        };
        let object = when
            .as_object()
            .filter(|o| !o.is_empty())
            .ok_or(ConditionError::ExpectedObject)?;
        let combiners: Vec<_> = [("AND", true), ("OR", false)]
            .into_iter()
            .filter_map(|(key, is_and)| Some((key, is_and, object.get(key)?)))
            .collect();
        match combiners[..] {
            [] => {}
            [(key, is_and, value)] => {
                let conditions = value
                    .as_array()
                    .ok_or(ConditionError::ExpectedArray(key))?
                    .iter()
                    .map(|v| Self::parse(Some(v)))
                    .collect::<Result<_, _>>()?;
                return Ok(if is_and {
                    Self::And(conditions)
                } else {
                    Self::Or(conditions)
                });
            }
            _ => return Err(ConditionError::CombinerSize),
        }
        let conditions = object
            .iter()
            .map(|(name, value)| {
                let terms =
                    parse_terms(value).ok_or_else(|| ConditionError::InvalidValue(name.clone()))?;
                Ok(Self::Property {
                    name: name.clone(),
                    terms,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self::And(conditions))
    }

    /// Evaluates against a state's immutable properties during table creation.
    pub fn matches(&self, props: &PropMap) -> bool {
        match self {
            Self::Always => true,
            Self::Property { name, terms } => props.get(name).is_some_and(|value| {
                terms
                    .iter()
                    .any(|term| (term.value == value) != term.negated)
            }),
            Self::And(conditions) => conditions.iter().all(|c| c.matches(props)),
            Self::Or(conditions) => conditions.iter().any(|c| c.matches(props)),
        }
    }
}

/// `Terms.CODEC`: a string, or the legacy int or bool, split on `|` into
/// terms that may each start with `!`; an empty term is invalid.
fn parse_terms(value: &serde_json::Value) -> Option<Box<[Term]>> {
    let value = match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => i32::try_from(value.as_i64()?).ok()?.to_string(),
        _ => return None,
    };
    value
        .split('|')
        .map(|term| {
            let (value, negated) = match term.strip_prefix('!') {
                Some(value) => (value, true),
                None => (term, false),
            };
            (!value.is_empty()).then(|| Term {
                value: value.to_owned(),
                negated,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockStateTable;

    /// Checks `when` against every native state of `block`.
    fn assert_states(block: &str, when: serde_json::Value, expected: impl Fn(&PropMap) -> bool) {
        let condition = Condition::parse(Some(&when)).unwrap();
        let mut states = BlockStateTable::native()
            .table()
            .iter()
            .filter(|data| data.id() == block)
            .map(|data| data.properties())
            .peekable();
        assert!(states.peek().is_some(), "no native states for {block}");
        for props in states {
            assert_eq!(
                condition.matches(props),
                expected(props),
                "{:?}",
                props.entries().collect::<Vec<_>>()
            );
        }
    }

    /// Vanilla 26.2's redstone dot selector draws the dot only on wires that
    /// join both axes or connect nowhere.
    #[test]
    fn redstone_dot_follows_vanilla_or_selector() {
        let when = serde_json::json!({"OR": [
            {"east": "none", "north": "none", "south": "none", "west": "none"},
            {"east": "side|up", "north": "side|up"},
            {"east": "side|up", "south": "side|up"},
            {"south": "side|up", "west": "side|up"},
            {"north": "side|up", "west": "side|up"}
        ]});
        assert_states("redstone_wire", when, |props| {
            let connected = |side| props.get(side) != Some("none");
            let [n, s, e, w] = ["north", "south", "east", "west"].map(connected);
            (n || s) == (e || w)
        });
    }

    /// Vanilla 26.2's AND selectors pick one chiseled bookshelf slot model per
    /// facing and fill state.
    #[test]
    fn bookshelf_slot_follows_vanilla_and_selector() {
        let when = serde_json::json!({"AND": [{"facing": "north"}, {"slot_0_occupied": "true"}]});
        assert_states("chiseled_bookshelf", when, |props| {
            props.get("facing") == Some("north") && props.get("slot_0_occupied") == Some("true")
        });
    }

    /// Nesting, int and bool values, per-term negation, and the selectors
    /// the codec rejects.
    #[test]
    fn nested_and_invalid_selectors() {
        let props =
            PropMap::from_pairs(vec![("north", "up"), ("powered", "false"), ("power", "1")]);
        let condition = serde_json::json!({"AND": [
            {"OR": [{"north": "side|up"}, {"powered": true}]},
            {"powered": "!true", "power": 1, "north": "!none|side"}
        ]});
        assert!(Condition::parse(Some(&condition)).unwrap().matches(&props));
        // Each term carries its own `!`: "is side, or isn't up".
        let per_term = serde_json::json!({"north": "side|!up"});
        assert!(!Condition::parse(Some(&per_term)).unwrap().matches(&props));
        // A combiner reads only `AND` and `OR`, ignoring other keys.
        let beside = serde_json::json!({"OR": [{"power": "1"}], "north": "none"});
        assert!(Condition::parse(Some(&beside)).unwrap().matches(&props));
        for value in [
            serde_json::json!({}),
            serde_json::json!({"OR": {}}),
            serde_json::json!({"AND": [], "OR": []}),
            serde_json::json!({"north": []}),
            serde_json::json!({"north": "!|"}),
            serde_json::json!({"north": "up||side"}),
            serde_json::json!({"power": 1.5}),
        ] {
            assert!(Condition::parse(Some(&value)).is_err(), "{value}");
        }
        assert!(Condition::parse(None).unwrap().matches(&props));
    }
}
