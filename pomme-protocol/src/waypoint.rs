//! `waypoint` (`ClientboundTrackedWaypointPacket`) as vanilla 26.2 lays it
//! out: an operation and a `TrackedWaypoint`.

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};

#[derive(Clone, Debug, PartialEq)]
pub struct WaypointPacket {
    pub operation: WaypointOperation,
    pub waypoint: TrackedWaypoint,
}

/// `ClientboundTrackedWaypointPacket.Operation`, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaypointOperation {
    Track,
    Untrack,
    Update,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackedWaypoint {
    pub id: WaypointId,
    /// `Waypoint.Icon.style`, a `waypoint_style_asset` key.
    pub style: String,
    /// `Waypoint.Icon.color`, as red, green, blue.
    pub color: Option<[u8; 3]>,
    pub data: WaypointData,
}

/// A waypoint's `Either<UUID, String>` identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WaypointId {
    Uuid(u128),
    Name(String),
}

/// `TrackedWaypoint.Type`'s contents, in ordinal order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaypointData {
    Empty,
    Position {
        x: i32,
        y: i32,
        z: i32,
    },
    Chunk {
        x: i32,
        z: i32,
    },
    /// Radians.
    Azimuth {
        angle: f32,
    },
}

/// Decodes packet `name`, or `None` if it isn't `waypoint`.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<WaypointPacket>> {
    (name == "waypoint").then(|| {
        // ClientboundTrackedWaypointPacket.java:30
        Ok(WaypointPacket {
            operation: match r.varint()? {
                0 => WaypointOperation::Track,
                1 => WaypointOperation::Untrack,
                2 => WaypointOperation::Update,
                _ => return Err(r.error("unknown waypoint operation")),
            },
            waypoint: tracked_waypoint(r)?,
        })
    })
}

/// `TrackedWaypoint.read`.
fn tracked_waypoint(r: &mut Reader) -> DecodeResult<TrackedWaypoint> {
    // `FriendlyByteBuf.readEither`: `true` for the left (UUID) side.
    let id = if r.bool()? {
        WaypointId::Uuid(r.uuid()?)
    } else {
        WaypointId::Name(r.utf(MAX_STRING_LENGTH)?.to_owned())
    };
    // `Waypoint.Icon.STREAM_CODEC`: the style key, then an optional
    // `ByteBufCodecs.RGB_COLOR`.
    let style = r.identifier()?.to_owned();
    let color = if r.bool()? {
        Some([r.u8()?, r.u8()?, r.u8()?])
    } else {
        None
    };
    let data = match r.varint()? {
        0 => WaypointData::Empty,
        1 => WaypointData::Position {
            x: r.var_i32()?,
            y: r.var_i32()?,
            z: r.var_i32()?,
        },
        2 => WaypointData::Chunk {
            x: r.var_i32()?,
            z: r.var_i32()?,
        },
        3 => WaypointData::Azimuth { angle: r.f32()? },
        _ => return Err(r.error("unknown waypoint type")),
    };
    Ok(TrackedWaypoint {
        id,
        style,
        color,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    const ID: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;

    fn decode(bytes: &[u8]) -> WaypointPacket {
        let mut r = Reader::new(bytes);
        let packet = read("waypoint", &mut r).expect("a waypoint").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn waypoint(style: &str, color: Option<[u8; 3]>, data: WaypointData) -> TrackedWaypoint {
        TrackedWaypoint {
            id: WaypointId::Uuid(ID),
            style: style.into(),
            color,
            data,
        }
    }

    #[test]
    fn each_operation_and_type_reads_as_vanilla_writes_it() {
        assert_eq!(
            decode(&case("waypoint/track_position")),
            WaypointPacket {
                operation: WaypointOperation::Track,
                waypoint: waypoint(
                    "minecraft:bowtie",
                    Some([0x12, 0xab, 0x34]),
                    WaypointData::Position {
                        x: 1,
                        y: -64,
                        z: 300
                    },
                ),
            }
        );
        assert_eq!(
            decode(&case("waypoint/update_chunk")),
            WaypointPacket {
                operation: WaypointOperation::Update,
                waypoint: waypoint(
                    "minecraft:default",
                    None,
                    WaypointData::Chunk { x: -3, z: 7 }
                ),
            }
        );
        assert_eq!(
            decode(&case("waypoint/track_azimuth")).waypoint.data,
            WaypointData::Azimuth { angle: 1.25 }
        );
        assert_eq!(
            decode(&case("waypoint/untrack")),
            WaypointPacket {
                operation: WaypointOperation::Untrack,
                waypoint: waypoint("minecraft:default", None, WaypointData::Empty),
            }
        );
    }

    #[test]
    fn a_named_waypoint_reads_its_string_identifier() {
        // Untrack, `Either.right("spawn")`, the default style, no color, empty.
        let mut bytes = vec![1, 0, 5];
        bytes.extend_from_slice(b"spawn");
        bytes.push(17);
        bytes.extend_from_slice(b"minecraft:default");
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(decode(&bytes).waypoint.id, WaypointId::Name("spawn".into()));
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("set_camera", &mut Reader::new(&[])).is_none());
    }
}
