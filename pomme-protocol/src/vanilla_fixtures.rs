//! Packet payloads encoded by vanilla's own `STREAM_CODEC`s
//! (`just codec-fixtures`, `tools/codec-oracle/CodecOracle.java`), so decoders
//! are tested against bytes vanilla produced rather than a reading of its
//! source.

use std::sync::OnceLock;

use glam::DVec3;
use serde::Deserialize;

use crate::entity::{PositionMoveRotation, Relative};

#[derive(Deserialize)]
struct Fixtures {
    version: String,
    registry_ids: std::collections::HashMap<String, u32>,
    registry_sizes: std::collections::HashMap<String, usize>,
    cases: std::collections::HashMap<String, String>,
}

fn fixtures() -> &'static Fixtures {
    static FIXTURES: OnceLock<Fixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("data/codec-fixtures-26.2.json"))
                .expect("codec fixtures parse");
        assert_eq!(
            fixtures.version,
            crate::version::NATIVE.name,
            "fixtures for another version"
        );
        fixtures
    })
}

/// The payload of case `name` (`<packet>/<case>`), without its packet id.
pub fn case(name: &str) -> Vec<u8> {
    let hex = fixtures()
        .cases
        .get(name)
        .unwrap_or_else(|| panic!("no codec fixture {name}; run `just codec-fixtures`"));
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex fixture"))
        .collect()
}

/// The id vanilla encoded `entry` of `registry` as, e.g.
/// `registry_id("dimension_type", "minecraft:overworld")`.
pub fn registry_id(registry: &str, entry: &str) -> u32 {
    fixtures().registry_ids[&format!("{registry}/{entry}")]
}

/// How many entries `registry` had when vanilla encoded the cases, e.g.
/// `registry_size("block_state")`.
pub fn registry_size(registry: &str) -> usize {
    fixtures().registry_sizes[registry]
}

/// The pose and relative set `CodecOracle.lifecycle` teleports with, in
/// `player_position` and `teleport_entity` alike.
pub fn teleport() -> (PositionMoveRotation, Relative) {
    let change = PositionMoveRotation {
        position: DVec3::new(1.5, 64.0, -2.0),
        delta: DVec3::new(0.0, 0.5, 0.0),
        y_rot: 90.0,
        x_rot: 10.0,
    };
    let relative = Relative {
        y: true,
        x_rot: true,
        rotate_delta: true,
        ..Default::default()
    };
    (change, relative)
}
