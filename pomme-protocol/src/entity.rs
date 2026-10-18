//! The entity lifecycle, movement and event packets, as vanilla 26.2 lays
//! them out (`net/minecraft/network/protocol/game/Clientbound*Packet`).

use glam::DVec3;

use crate::codec::{DecodeResult, Reader};

/// `PositionMoveRotation.STREAM_CODEC`: absolute position and motion, then
/// the rotation in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionMoveRotation {
    pub position: DVec3,
    pub delta: DVec3,
    pub y_rot: f32,
    pub x_rot: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntityPacket {
    Add {
        id: i32,
        uuid: u128,
        entity_type: u32,
        position: DVec3,
        movement: DVec3,
        x_rot: f32,
        y_rot: f32,
        y_head_rot: f32,
        data: i32,
    },
    Remove {
        ids: Vec<i32>,
    },
    /// The move packets; a `move_entity_rot` has no `delta` and a
    /// `move_entity_pos` no rotation (`(y_rot, x_rot)`).
    Move {
        id: i32,
        delta: Option<DVec3>,
        rotation: Option<(f32, f32)>,
        on_ground: bool,
    },
    PositionSync {
        id: i32,
        values: PositionMoveRotation,
        on_ground: bool,
    },
    Teleport {
        id: i32,
        change: PositionMoveRotation,
        /// `Relative.SET_STREAM_CODEC` bits (X = 0 .. ROTATE_DELTA = 8).
        relative: u32,
        on_ground: bool,
    },
    Motion {
        id: i32,
        velocity: DVec3,
    },
    RotateHead {
        id: i32,
        y_head_rot: f32,
    },
    Animate {
        id: i32,
        action: u8,
    },
    Event {
        id: i32,
        event: i8,
    },
    HurtAnimation {
        id: i32,
        yaw: f32,
    },
    Damage {
        id: i32,
        damage_type: u32,
        cause: Option<i32>,
        direct: Option<i32>,
        position: Option<DVec3>,
    },
    TakeItem {
        item: i32,
        collector: i32,
        amount: i32,
    },
    Passengers {
        vehicle: i32,
        passengers: Vec<i32>,
    },
}

/// Decodes packet `name`, or `None` if it isn't an entity packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<EntityPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<EntityPacket> = match name {
        // ClientboundAddEntityPacket.java:63-75: rotations as x, y, then head.
        "add_entity" => |r| {
            Ok(EntityPacket::Add {
                id: r.var_i32()?,
                uuid: r.uuid()?,
                entity_type: r.varint()?,
                position: r.vec3()?,
                movement: r.lp_vec3()?,
                x_rot: angle(r)?,
                y_rot: angle(r)?,
                y_head_rot: angle(r)?,
                data: r.var_i32()?,
            })
        },
        // ClientboundRemoveEntitiesPacket.java:32-33
        "remove_entities" => |r| Ok(EntityPacket::Remove { ids: ids(r)? }),
        "move_entity_pos" => |r| read_move(r, true, false),
        "move_entity_pos_rot" => |r| read_move(r, true, true),
        "move_entity_rot" => |r| read_move(r, false, true),
        // ClientboundEntityPositionSyncPacket.java:18
        "entity_position_sync" => |r| {
            Ok(EntityPacket::PositionSync {
                id: r.var_i32()?,
                values: position_move_rotation(r)?,
                on_ground: r.bool()?,
            })
        },
        // ClientboundTeleportEntityPacket.java:19
        "teleport_entity" => |r| {
            Ok(EntityPacket::Teleport {
                id: r.var_i32()?,
                change: position_move_rotation(r)?,
                relative: r.i32()? as u32,
                on_ground: r.bool()?,
            })
        },
        // ClientboundSetEntityMotionPacket.java:21
        "set_entity_motion" => |r| {
            Ok(EntityPacket::Motion {
                id: r.var_i32()?,
                velocity: r.lp_vec3()?,
            })
        },
        // ClientboundRotateHeadPacket.java:31-33
        "rotate_head" => |r| {
            Ok(EntityPacket::RotateHead {
                id: r.var_i32()?,
                y_head_rot: angle(r)?,
            })
        },
        // ClientboundAnimatePacket.java:30-32
        "animate" => |r| {
            Ok(EntityPacket::Animate {
                id: r.var_i32()?,
                action: r.u8()?,
            })
        },
        // ClientboundEntityEventPacket.java:30-32: a plain int id.
        "entity_event" => |r| {
            Ok(EntityPacket::Event {
                id: r.i32()?,
                event: r.i8()?,
            })
        },
        // ClientboundHurtAnimationPacket.java:22-23
        "hurt_animation" => |r| {
            Ok(EntityPacket::HurtAnimation {
                id: r.var_i32()?,
                yaw: r.f32()?,
            })
        },
        // ClientboundDamageEventPacket.java:29-30
        "damage_event" => |r| {
            Ok(EntityPacket::Damage {
                id: r.var_i32()?,
                damage_type: r.varint()?,
                cause: optional_entity_id(r)?,
                direct: optional_entity_id(r)?,
                position: if r.bool()? { Some(r.vec3()?) } else { None },
            })
        },
        // ClientboundTakeItemEntityPacket.java:26-29
        "take_item_entity" => |r| {
            Ok(EntityPacket::TakeItem {
                item: r.var_i32()?,
                collector: r.var_i32()?,
                amount: r.var_i32()?,
            })
        },
        // ClientboundSetPassengersPacket.java:30-32
        "set_passengers" => |r| {
            Ok(EntityPacket::Passengers {
                vehicle: r.var_i32()?,
                passengers: ids(r)?,
            })
        },
        _ => return None,
    };
    Some(read(r))
}

/// `ClientboundMoveEntityPacket.Pos/PosRot/Rot`: shorts of 1/4096 block
/// (`VecDeltaCodec`), then y and x rotation.
fn read_move(r: &mut Reader, pos: bool, rot: bool) -> DecodeResult<EntityPacket> {
    let id = r.var_i32()?;
    let delta = if pos {
        Some(DVec3::new(delta(r)?, delta(r)?, delta(r)?))
    } else {
        None
    };
    let rotation = if rot {
        Some((angle(r)?, angle(r)?))
    } else {
        None
    };
    Ok(EntityPacket::Move {
        id,
        delta,
        rotation,
        on_ground: r.bool()?,
    })
}

fn delta(r: &mut Reader) -> DecodeResult<f64> {
    Ok(f64::from(r.i16()?) / 4096.0)
}

/// `ClientboundDamageEventPacket.readOptionalEntityId`: id + 1, 0 for none.
fn optional_entity_id(r: &mut Reader) -> DecodeResult<Option<i32>> {
    Ok(r.var_i32()?.checked_sub(1).filter(|id| *id >= 0))
}

fn position_move_rotation(r: &mut Reader) -> DecodeResult<PositionMoveRotation> {
    Ok(PositionMoveRotation {
        position: r.vec3()?,
        delta: r.vec3()?,
        y_rot: r.f32()?,
        x_rot: r.f32()?,
    })
}

/// `FriendlyByteBuf.readIntIdList`: a count, then varint ids.
fn ids(r: &mut Reader) -> DecodeResult<Vec<i32>> {
    r.list(usize::MAX, Reader::var_i32)
}

fn angle(r: &mut Reader) -> DecodeResult<f32> {
    Ok(unpack_degrees(r.i8()?))
}

/// Vanilla `Mth.unpackDegrees`: a signed byte of 1/256 turns, in degrees.
pub fn unpack_degrees(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{write_lp_vec3, write_varint};

    fn decode(name: &str, bytes: &[u8]) -> EntityPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("an entity packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn var(v: i32) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(&mut out, v as u32);
        out
    }

    fn vec3(v: DVec3) -> Vec<u8> {
        [v.x, v.y, v.z]
            .iter()
            .flat_map(|c| c.to_be_bytes())
            .collect()
    }

    #[test]
    fn add_entity_reads_x_then_y_rotation() {
        let position = DVec3::new(1.5, -60.0, 3.25);
        let mut bytes = var(7);
        bytes.extend(42u128.to_be_bytes());
        bytes.extend(var(140));
        bytes.extend(vec3(position));
        write_lp_vec3(&mut bytes, DVec3::ZERO);
        // xRot -64 (-90°), yRot 64 (90°), head yRot -128 (-180°).
        bytes.extend([0xC0, 0x40, 0x80]);
        bytes.extend(var(5));
        assert_eq!(
            decode("add_entity", &bytes),
            EntityPacket::Add {
                id: 7,
                uuid: 42,
                entity_type: 140,
                position,
                movement: DVec3::ZERO,
                x_rot: -90.0,
                y_rot: 90.0,
                y_head_rot: -180.0,
                data: 5,
            }
        );
    }

    #[test]
    fn moves_carry_their_parts() {
        let shorts = [4096i16, -2048, 1].iter().flat_map(|s| s.to_be_bytes());
        let mut pos = var(3);
        pos.extend(shorts.clone());
        pos.push(1);
        let mut pos_rot = var(3);
        pos_rot.extend(shorts);
        pos_rot.extend([0x40, 0xC0, 0]);
        let rot = [var(3), vec![0x40, 0xC0, 1]].concat();
        let delta = Some(DVec3::new(1.0, -0.5, 1.0 / 4096.0));
        assert_eq!(
            decode("move_entity_pos", &pos),
            EntityPacket::Move {
                id: 3,
                delta,
                rotation: None,
                on_ground: true
            }
        );
        assert_eq!(
            decode("move_entity_pos_rot", &pos_rot),
            EntityPacket::Move {
                id: 3,
                delta,
                rotation: Some((90.0, -90.0)),
                on_ground: false
            }
        );
        assert_eq!(
            decode("move_entity_rot", &rot),
            EntityPacket::Move {
                id: 3,
                delta: None,
                rotation: Some((90.0, -90.0)),
                on_ground: true
            }
        );
    }

    #[test]
    fn teleport_and_sync_share_position_move_rotation() {
        let values = PositionMoveRotation {
            position: DVec3::new(10.0, 64.0, -5.0),
            delta: DVec3::new(0.0, 0.25, 0.0),
            y_rot: 45.0,
            x_rot: -10.0,
        };
        let mut pmr = vec3(values.position);
        pmr.extend(vec3(values.delta));
        pmr.extend(45.0f32.to_be_bytes());
        pmr.extend((-10.0f32).to_be_bytes());
        let sync = [var(9), pmr.clone(), vec![1]].concat();
        // Relative X and Y_ROT, as a plain int.
        let teleport = [var(9), pmr, 0b1001i32.to_be_bytes().to_vec(), vec![0]].concat();
        assert_eq!(
            decode("entity_position_sync", &sync),
            EntityPacket::PositionSync {
                id: 9,
                values,
                on_ground: true
            }
        );
        assert_eq!(
            decode("teleport_entity", &teleport),
            EntityPacket::Teleport {
                id: 9,
                change: values,
                relative: 0b1001,
                on_ground: false
            }
        );
    }

    #[test]
    fn motion_reads_an_lp_vec3_with_its_scale_extension() {
        let velocity = DVec3::new(6.0, -1.0, 0.0);
        let mut bytes = var(2);
        write_lp_vec3(&mut bytes, velocity);
        let EntityPacket::Motion {
            id: 2,
            velocity: read,
        } = decode("set_entity_motion", &bytes)
        else {
            panic!("a motion packet");
        };
        assert!((read - velocity).abs().max_element() < 1e-3, "{read}");
    }

    #[test]
    fn damage_sources_are_offset_by_one() {
        let mut none = var(4);
        none.extend(var(12));
        none.extend([0, 0, 0]);
        let mut sourced = var(4);
        sourced.extend(var(12));
        sourced.extend(var(8));
        sourced.extend(var(1));
        sourced.push(1);
        sourced.extend(vec3(DVec3::ONE));
        assert_eq!(
            decode("damage_event", &none),
            EntityPacket::Damage {
                id: 4,
                damage_type: 12,
                cause: None,
                direct: None,
                position: None
            }
        );
        assert_eq!(
            decode("damage_event", &sourced),
            EntityPacket::Damage {
                id: 4,
                damage_type: 12,
                cause: Some(7),
                direct: Some(0),
                position: Some(DVec3::ONE)
            }
        );
    }

    #[test]
    fn small_packets() {
        // Entity events use a plain int id.
        assert_eq!(
            decode("entity_event", &[0, 0, 1, 0, 3]),
            EntityPacket::Event { id: 256, event: 3 }
        );
        assert_eq!(
            decode("remove_entities", &[2, 5, 6]),
            EntityPacket::Remove { ids: vec![5, 6] }
        );
        assert_eq!(
            decode("set_passengers", &[1, 1, 2]),
            EntityPacket::Passengers {
                vehicle: 1,
                passengers: vec![2]
            }
        );
        assert_eq!(
            decode("take_item_entity", &[9, 1, 3]),
            EntityPacket::TakeItem {
                item: 9,
                collector: 1,
                amount: 3
            }
        );
        assert_eq!(
            decode("animate", &[1, 2]),
            EntityPacket::Animate { id: 1, action: 2 }
        );
        assert_eq!(
            decode("rotate_head", &[1, 0x20]),
            EntityPacket::RotateHead {
                id: 1,
                y_head_rot: 45.0
            }
        );
        let mut hurt = vec![1];
        hurt.extend(90.0f32.to_be_bytes());
        assert_eq!(
            decode("hurt_animation", &hurt),
            EntityPacket::HurtAnimation { id: 1, yaw: 90.0 }
        );
    }

    #[test]
    fn other_packets_and_truncation() {
        assert!(read("set_entity_data", &mut Reader::new(&[])).is_none());
        let err = read("entity_event", &mut Reader::new(&[0, 0])).unwrap();
        assert_eq!(err.map_err(|e| e.what), Err("truncated"));
    }
}
