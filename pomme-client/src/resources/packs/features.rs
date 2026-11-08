//! The fixed Minecraft 26.3 `main` feature universe, not a generic registry.

use std::io;

use serde_json::Value;

use crate::chat_component::{normalize_identifier, valid_identifier};

pub(super) const FEATURES_SECTION: &str = "features";
const ENABLED: &str = "enabled";
const FEATURE_IDS: [&str; 4] = [
    "minecraft:vanilla",
    "minecraft:trade_rebalance",
    "minecraft:redstone_experiments",
    "minecraft:minecart_improvements",
];

/// Vanilla `FeatureFlagRegistry.codec`: a pack requesting a flag outside the
/// main universe fails to load. The requested set itself has no consumer yet.
pub(super) fn validate(section: &Value) -> io::Result<()> {
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
    let enabled = section
        .get(ENABLED)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("features.enabled must be an array of identifiers".to_owned()))?;
    for value in enabled {
        let name = value
            .as_str()
            .filter(|name| valid_identifier(name))
            .ok_or_else(|| invalid(format!("invalid features.enabled identifier: {value}")))?;
        let name = normalize_identifier(name);
        if !FEATURE_IDS.contains(&name.as_str()) {
            return Err(invalid(format!(
                "unknown feature id in features.enabled: {name}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::validate;

    #[test]
    fn known_flags_pass_and_unknown_fail() {
        assert!(validate(&json!({"enabled": ["vanilla", "minecraft:trade_rebalance"]})).is_ok());
        assert!(validate(&json!({"enabled": []})).is_ok());
        assert!(validate(&json!({"enabled": ["minecraft:bundle"]})).is_err());
        assert!(validate(&json!({"enabled": "vanilla"})).is_err());
        assert!(validate(&json!({})).is_err());
    }
}
