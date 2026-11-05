//! Vanilla multipart selectors, evaluated while building render tables.

use super::PropMap;

/// Missing `when` applies unconditionally; property objects imply AND.
#[derive(Clone, Debug)]
pub enum Condition {
    Always,
    Property {
        name: String,
        values: Box<[String]>,
        negated: bool,
    },
    And(Vec<Condition>),
    Or(Vec<Condition>),
}

/// Invalid pack selectors must not silently become unconditional parts.
#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("multipart condition must be a nonempty object")]
    ExpectedObject,
    #[error("{0} requires an array of conditions")]
    ExpectedArray(&'static str),
    #[error("invalid multipart property value for {0}")]
    InvalidValue(String),
}

impl Condition {
    /// Parses absent `when`, nested AND/OR and pipe-separated alternatives.
    pub fn parse(when: Option<&serde_json::Value>) -> Result<Self, ConditionError> {
        let Some(when) = when else {
            return Ok(Self::Always);
        };
        let object = when
            .as_object()
            .filter(|o| !o.is_empty())
            .ok_or(ConditionError::ExpectedObject)?;
        if object.len() == 1 {
            for (operator, is_and) in [("AND", true), ("OR", false)] {
                if let Some(value) = object.get(operator) {
                    let conditions = value
                        .as_array()
                        .ok_or(ConditionError::ExpectedArray(operator))?
                        .iter()
                        .map(|v| Self::parse(Some(v)))
                        .collect::<Result<_, _>>()?;
                    return Ok(if is_and {
                        Self::And(conditions)
                    } else {
                        Self::Or(conditions)
                    });
                }
            }
        }
        let conditions = object
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    serde_json::Value::String(value) => value.clone(),
                    serde_json::Value::Bool(value) => value.to_string(),
                    serde_json::Value::Number(value) => value.to_string(),
                    _ => return Err(ConditionError::InvalidValue(name.clone())),
                };
                let negated = value.starts_with('!');
                let values: Box<[String]> = value
                    .strip_prefix('!')
                    .unwrap_or(&value)
                    .split('|')
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect();
                if values.is_empty() {
                    return Err(ConditionError::InvalidValue(name.clone()));
                }
                Ok(Self::Property {
                    name: name.clone(),
                    values,
                    negated,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::And(conditions))
    }

    /// Evaluates against a state's immutable properties during table creation.
    pub fn matches(&self, props: &PropMap) -> bool {
        match self {
            Self::Always => true,
            Self::Property {
                name,
                values,
                negated,
            } => props.get(name).is_some_and(|value| {
                values.iter().any(|alternative| alternative == value) != *negated
            }),
            Self::And(conditions) => conditions.iter().all(|c| c.matches(props)),
            Self::Or(conditions) => conditions.iter().any(|c| c.matches(props)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockStateTable;

    /// Every native state of `block`, with its properties.
    fn native_states(block: &str) -> Vec<&'static PropMap> {
        let states: Vec<_> = BlockStateTable::native()
            .table()
            .iter()
            .filter(|data| data.id() == block)
            .map(|data| data.properties())
            .collect();
        assert!(!states.is_empty(), "no native states for {block}");
        states
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
        let dot = Condition::parse(Some(&when)).unwrap();
        for props in native_states("redstone_wire") {
            let connected = |side| props.get(side) != Some("none");
            let [n, s, e, w] = ["north", "south", "east", "west"].map(connected);
            assert_eq!(
                dot.matches(props),
                (n || s) == (e || w),
                "{:?}",
                props.entries().collect::<Vec<_>>()
            );
        }
    }

    /// Vanilla 26.2's AND selectors pick one chiseled bookshelf slot model per
    /// facing and fill state.
    #[test]
    fn bookshelf_slot_follows_vanilla_and_selector() {
        let when = serde_json::json!({"AND": [{"facing": "north"}, {"slot_0_occupied": "true"}]});
        let slot = Condition::parse(Some(&when)).unwrap();
        for props in native_states("chiseled_bookshelf") {
            let expected = props.get("facing") == Some("north")
                && props.get("slot_0_occupied") == Some("true");
            assert_eq!(
                slot.matches(props),
                expected,
                "{:?}",
                props.entries().collect::<Vec<_>>()
            );
        }
    }

    /// Nested logic, scalar codecs, negation and invalid data keep semantics.
    #[test]
    fn nested_and_invalid_selectors() {
        let props =
            PropMap::from_pairs(vec![("north", "up"), ("powered", "false"), ("power", "1")]);
        let condition = serde_json::json!({"AND": [
            {"OR": [{"north": "side|up"}, {"powered": true}]},
            {"powered": "!true", "power": 1, "north": "!none|side"}
        ]});
        assert!(Condition::parse(Some(&condition)).unwrap().matches(&props));
        for value in [
            serde_json::json!({}),
            serde_json::json!({"OR": {}}),
            serde_json::json!({"north": []}),
            serde_json::json!({"north": "!|"}),
        ] {
            assert!(Condition::parse(Some(&value)).is_err(), "{value}");
        }
        assert!(Condition::parse(None).unwrap().matches(&props));
    }
}
