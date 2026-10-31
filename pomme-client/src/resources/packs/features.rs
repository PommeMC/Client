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

/// Only the four flags registered in Minecraft 26.3's main universe can enter
/// this set. Empty requested metadata does not implicitly request vanilla.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FeatureFlagSet {
    bits: u64,
}

impl FeatureFlagSet {
    pub(crate) const VANILLA: Self = Self { bits: 1 << 0 };
    pub(crate) const TRADE_REBALANCE: Self = Self { bits: 1 << 1 };
    pub(crate) const REDSTONE_EXPERIMENTS: Self = Self { bits: 1 << 2 };
    pub(crate) const MINECART_IMPROVEMENTS: Self = Self { bits: 1 << 3 };

    pub(crate) const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub(crate) const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    /// Whether every flag in `other` is present (including the empty set).
    pub(crate) const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    pub(super) fn decode(section: &Value) -> io::Result<Self> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
        let enabled = section
            .get(ENABLED)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                invalid("features.enabled must be an array of identifiers".to_owned())
            })?;
        let mut flags = Self::empty();
        for value in enabled {
            let name = value
                .as_str()
                .filter(|name| valid_identifier(name))
                .ok_or_else(|| invalid(format!("invalid features.enabled identifier: {value}")))?;
            let name = normalize_identifier(name);
            let bit = FEATURE_IDS
                .iter()
                .position(|known| *known == name)
                .ok_or_else(|| {
                    invalid(format!("unknown feature id in features.enabled: {name}"))
                })?;
            flags.bits |= 1 << bit;
        }
        Ok(flags)
    }
}
