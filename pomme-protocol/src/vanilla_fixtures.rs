//! Packet payloads encoded by vanilla's own `STREAM_CODEC`s
//! (`just codec-fixtures`, `tools/codec-oracle/CodecOracle.java`), so decoders
//! are tested against bytes vanilla produced rather than a reading of its
//! source.

use std::sync::OnceLock;

use glam::DVec3;
use serde::Deserialize;

use crate::codec::{DecodeResult, Reader};
use crate::entity::{PositionMoveRotation, Relative};

#[derive(Deserialize)]
struct Fixtures {
    version: String,
    protocol: i32,
    /// By phase, then direction, as the packet table files spell them.
    packets: std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>>,
    entity_dimensions: std::collections::HashMap<String, [f32; 3]>,
    registry_ids: std::collections::HashMap<String, u32>,
    registry_sizes: std::collections::HashMap<String, usize>,
    cases: Cases,
}

/// Hex payloads by case name.
type Cases = std::collections::HashMap<String, String>;

fn fixtures() -> &'static Fixtures {
    static FIXTURES: OnceLock<Fixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("data/codec-fixtures-26.3.json"))
                .expect("codec fixtures parse");
        assert_eq!(
            fixtures.version,
            crate::version::NATIVE.name,
            "fixtures for another version"
        );
        fixtures
    })
}

/// The protocol number the game reports.
pub fn protocol() -> i32 {
    fixtures().protocol
}

/// The game's packet names for a phase and direction, in id order.
pub fn packet_names(phase: &str, direction: &str) -> &'static [String] {
    &fixtures().packets[phase][direction]
}

/// How many entity types the game has.
pub fn entity_type_count() -> usize {
    fixtures().entity_dimensions.len()
}

/// The width, height and eye height the game gives entity type `name`.
pub fn entity_dimensions(name: &str) -> [f32; 3] {
    fixtures().entity_dimensions[name]
}

/// The payload of case `name` (`<packet>/<case>`), without its packet id.
pub fn case(name: &str) -> Vec<u8> {
    bytes(&fixtures().cases, name)
}

/// Case `name` as 26.2 encoded it, for the rewrites that lift its layouts.
pub fn case_26_2(name: &str) -> Vec<u8> {
    static CASES: OnceLock<Cases> = OnceLock::new();
    let cases = CASES.get_or_init(|| {
        #[derive(Deserialize)]
        struct Older {
            cases: Cases,
        }
        let older: Older = serde_json::from_str(include_str!("data/codec-fixtures-26.2.json"))
            .expect("codec fixtures parse");
        older.cases
    });
    bytes(cases, name)
}

fn bytes(cases: &Cases, name: &str) -> Vec<u8> {
    let hex = cases
        .get(name)
        .unwrap_or_else(|| panic!("no codec fixture {name}; run `just codec-fixtures`"));
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex fixture"))
        .collect()
}

/// Case `name` decoded whole by `read`, the reader of the module that owns
/// its packet.
pub fn decoded<P>(name: &str, read: fn(&str, &mut Reader) -> Option<DecodeResult<P>>) -> P {
    let packet = name.split('/').next().expect("a packet name");
    let payload = case(name);
    let mut r = Reader::new(&payload);
    let decoded = read(packet, &mut r)
        .unwrap_or_else(|| panic!("{packet} is not this module's packet"))
        .unwrap_or_else(|e| panic!("{name}: {}", e.what));
    r.ensure_eof().expect("the whole payload is read");
    decoded
}

/// Checks that each case decodes, through `read`, to the packet beside it.
pub fn assert_decoded<'a, P: PartialEq + std::fmt::Debug>(
    read: fn(&str, &mut Reader) -> Option<DecodeResult<P>>,
    cases: impl IntoIterator<Item = (&'a str, P)>,
) {
    for (name, expected) in cases {
        assert_eq!(decoded(name, read), expected, "{name}");
    }
}

/// The id `CodecOracle` gives its resource pack and its waypoints.
pub const ORACLE_ID: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;

/// `text` as the NBT string a plain text component is sent as.
pub fn nbt_string(text: &str) -> Box<[u8]> {
    let length = u16::try_from(text.len()).expect("a short string");
    [&[8][..], &length.to_be_bytes(), text.as_bytes()]
        .concat()
        .into()
}

/// The compound `{d: 5}` as network NBT, which `CodecOracle.d5` gives the
/// block entities it sends.
pub const D5_TAG: [u8; 10] = [10, 3, 0, 1, b'd', 0, 0, 0, 5, 0];

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
