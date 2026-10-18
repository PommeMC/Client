//! Block packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*Packet`).

use crate::codec::{DecodeResult, Reader};

/// A block position: x, y, z.
pub type BlockPos = (i32, i32, i32);

/// NBT tag type of a compound.
const TAG_COMPOUND: u8 = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum BlockPacket {
    Update {
        pos: BlockPos,
        /// A `Block.BLOCK_STATE_REGISTRY` id.
        state: u32,
    },
    /// Absolute positions and their new state ids.
    SectionUpdate {
        updates: Vec<(BlockPos, u32)>,
    },
    ChangedAck {
        seq: u32,
    },
    Event {
        pos: BlockPos,
        action: u8,
        param: u8,
        /// A `block` registry id.
        block: u32,
    },
    EntityData {
        pos: BlockPos,
        /// A `block_entity_type` registry id.
        kind: u32,
        /// A compound tag as network NBT.
        tag: Box<[u8]>,
    },
    LevelEvent {
        event: i32,
        pos: BlockPos,
        data: i32,
        global: bool,
    },
}

/// Decodes packet `name`, or `None` if it isn't a block packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<BlockPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<BlockPacket> = match name {
        // ClientboundBlockUpdatePacket.java:20
        "block_update" => |r| {
            Ok(BlockPacket::Update {
                pos: r.block_pos()?,
                state: r.varint()?,
            })
        },
        // ClientboundSectionBlocksUpdatePacket.java:48-58
        "section_blocks_update" => |r| {
            let (sx, sy, sz) = r.section_pos()?;
            let updates = r.list(usize::MAX, |r| {
                let packed = r.varlong()?;
                let pos = (
                    sx * 16 + (packed >> 8 & 0xF) as i32,
                    sy * 16 + (packed & 0xF) as i32,
                    sz * 16 + (packed >> 4 & 0xF) as i32,
                );
                Ok((pos, (packed >> 12) as u32))
            })?;
            Ok(BlockPacket::SectionUpdate { updates })
        },
        // ClientboundBlockChangedAckPacket.java:17-18
        "block_changed_ack" => |r| Ok(BlockPacket::ChangedAck { seq: r.varint()? }),
        // ClientboundBlockEventPacket.java:32-36
        "block_event" => |r| {
            Ok(BlockPacket::Event {
                pos: r.block_pos()?,
                action: r.u8()?,
                param: r.u8()?,
                block: r.varint()?,
            })
        },
        // ClientboundBlockEntityDataPacket.java:23; `TRUSTED_COMPOUND_TAG`
        // rejects anything but a compound (ByteBufCodecs.java:376-400).
        "block_entity_data" => |r| {
            let pos = r.block_pos()?;
            let kind = r.varint()?;
            let tag = r.nbt()?;
            if tag[0] != TAG_COMPOUND {
                return Err(r.error("expected a compound tag"));
            }
            Ok(BlockPacket::EntityData {
                pos,
                kind,
                tag: tag.into(),
            })
        },
        // ClientboundLevelEventPacket.java:29-33
        "level_event" => |r| {
            Ok(BlockPacket::LevelEvent {
                event: r.i32()?,
                pos: r.block_pos()?,
                data: r.i32()?,
                global: r.bool()?,
            })
        },
        _ => return None,
    };
    Some(read(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::write_varint;

    fn decode(name: &str, bytes: &[u8]) -> BlockPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a block packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    /// `BlockPos.asLong`.
    fn block_pos(x: i32, y: i32, z: i32) -> [u8; 8] {
        let packed = (i64::from(x) & 0x3FF_FFFF) << 38
            | (i64::from(z) & 0x3FF_FFFF) << 12
            | (i64::from(y) & 0xFFF);
        packed.to_be_bytes()
    }

    /// `SectionPos.asLong`.
    fn section_pos(x: i32, y: i32, z: i32) -> [u8; 8] {
        let packed = (i64::from(x) & 0x3F_FFFF) << 42
            | (i64::from(z) & 0x3F_FFFF) << 20
            | (i64::from(y) & 0xF_FFFF);
        packed.to_be_bytes()
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("set_health", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn block_update_with_negative_coordinates() {
        let bytes = [&block_pos(-3, -60, 4)[..], &[0x81, 0x01]].concat();
        assert_eq!(
            decode("block_update", &bytes),
            BlockPacket::Update {
                pos: (-3, -60, 4),
                state: 129
            }
        );
    }

    #[test]
    fn section_update_is_absolute() {
        let mut bytes = section_pos(-1, -4, 2).to_vec();
        write_varint(&mut bytes, 2);
        // State 1 at relative (15, 3, 0), then state 0 at (0, 0, 9).
        bytes.extend([0x83, 0x3E]);
        bytes.extend([0x90, 0x01]);
        assert_eq!(
            decode("section_blocks_update", &bytes),
            BlockPacket::SectionUpdate {
                updates: vec![((-1, -61, 32), 1), ((-16, -64, 41), 0)]
            }
        );
    }

    #[test]
    fn ack_event_and_level_event() {
        assert_eq!(
            decode("block_changed_ack", &[0x11]),
            BlockPacket::ChangedAck { seq: 17 }
        );
        let event = [&block_pos(1, 2, 3)[..], &[1, 2, 0xB4, 0x01]].concat();
        assert_eq!(
            decode("block_event", &event),
            BlockPacket::Event {
                pos: (1, 2, 3),
                action: 1,
                param: 2,
                block: 180
            }
        );
        let level = [
            &2001i32.to_be_bytes()[..],
            &block_pos(0, -58, 4),
            &1i32.to_be_bytes(),
            &[0],
        ]
        .concat();
        assert_eq!(
            decode("level_event", &level),
            BlockPacket::LevelEvent {
                event: 2001,
                pos: (0, -58, 4),
                data: 1,
                global: false
            }
        );
    }

    #[test]
    fn block_entity_data_needs_a_compound() {
        let tag = [TAG_COMPOUND, 0];
        let bytes = [&block_pos(0, 0, 0)[..], &[7], &tag].concat();
        assert_eq!(
            decode("block_entity_data", &bytes),
            BlockPacket::EntityData {
                pos: (0, 0, 0),
                kind: 7,
                tag: tag.into()
            }
        );
        let end = [&block_pos(0, 0, 0)[..], &[7, 0]].concat();
        assert!(
            read("block_entity_data", &mut Reader::new(&end))
                .unwrap()
                .is_err()
        );
    }
}
