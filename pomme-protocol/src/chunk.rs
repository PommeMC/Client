//! Chunk and light packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/Clientbound*Packet`). The section
//! buffer stays opaque here; `section` reads its paletted containers.

use crate::codec::{DecodeResult, Reader};

/// `ClientboundLevelChunkPacketData`'s cap on the section buffer.
const MAX_SECTION_BYTES: usize = 0x200000;
/// `ClientboundLightUpdatePacketData.DATA_LAYER_STREAM_CODEC`'s layer size.
const LIGHT_LAYER_BYTES: usize = 2048;

/// `ClientboundLevelChunkPacketData.BlockEntityInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEntityInfo {
    /// Section-relative x in the high nibble, z in the low one.
    pub packed_xz: u8,
    pub y: i16,
    /// A `block_entity_type` registry id.
    pub kind: u32,
    /// The update tag as network NBT, `None` for an end tag.
    pub tag: Option<Box<[u8]>>,
}

/// `ClientboundLightUpdatePacketData`: the masks are BitSets as longs, and
/// each update is one 2048-byte layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightData {
    pub sky_y_mask: Vec<u64>,
    pub block_y_mask: Vec<u64>,
    pub empty_sky_y_mask: Vec<u64>,
    pub empty_block_y_mask: Vec<u64>,
    pub sky_updates: Vec<Box<[u8]>>,
    pub block_updates: Vec<Box<[u8]>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChunkPacket {
    Level {
        x: i32,
        z: i32,
        /// `Heightmap.Types` ids and their packed data.
        heightmaps: Vec<(u32, Box<[u64]>)>,
        sections: Box<[u8]>,
        block_entities: Vec<BlockEntityInfo>,
        light: LightData,
    },
    Light {
        x: i32,
        z: i32,
        light: LightData,
    },
    Forget {
        x: i32,
        z: i32,
    },
    BatchStart,
    BatchFinished {
        size: u32,
    },
    CacheCenter {
        x: i32,
        z: i32,
    },
    CacheRadius {
        radius: i32,
    },
}

/// Decodes packet `name`, or `None` if it isn't a chunk packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<ChunkPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<ChunkPacket> = match name {
        // ClientboundLevelChunkWithLightPacket.java:39-44, then
        // ClientboundLevelChunkPacketData.java:52-61.
        "level_chunk_with_light" => |r| {
            let x = r.i32()?;
            let z = r.i32()?;
            let heightmaps = r.list(usize::MAX, |r| {
                Ok((r.varint()?, r.long_array()?.into_boxed_slice()))
            })?;
            let size = r.varint()? as usize;
            if size > MAX_SECTION_BYTES {
                return Err(r.error("chunk sections over their maximum"));
            }
            let sections = r.bytes(size)?.into();
            let block_entities = r.list(usize::MAX, block_entity)?;
            Ok(ChunkPacket::Level {
                x,
                z,
                heightmaps,
                sections,
                block_entities,
                light: light_data(r)?,
            })
        },
        // ClientboundLightUpdatePacket.java:34-38
        "light_update" => |r| {
            Ok(ChunkPacket::Light {
                x: r.var_i32()?,
                z: r.var_i32()?,
                light: light_data(r)?,
            })
        },
        // ClientboundForgetLevelChunkPacket.java:18-19: `ChunkPos.unpack`,
        // x in the low half.
        "forget_level_chunk" => |r| {
            let packed = r.i64()?;
            Ok(ChunkPacket::Forget {
                x: packed as i32,
                z: (packed >> 32) as i32,
            })
        },
        // ClientboundChunkBatchStartPacket.java:19
        "chunk_batch_start" => |_| Ok(ChunkPacket::BatchStart),
        // ClientboundChunkBatchFinishedPacket.java:17-18
        "chunk_batch_finished" => |r| Ok(ChunkPacket::BatchFinished { size: r.varint()? }),
        // ClientboundSetChunkCacheCenterPacket.java:24-26
        "set_chunk_cache_center" => |r| {
            Ok(ChunkPacket::CacheCenter {
                x: r.var_i32()?,
                z: r.var_i32()?,
            })
        },
        // ClientboundSetChunkCacheRadiusPacket.java:22-23
        "set_chunk_cache_radius" => |r| {
            Ok(ChunkPacket::CacheRadius {
                radius: r.var_i32()?,
            })
        },
        _ => return None,
    };
    Some(read(r))
}

/// ClientboundLevelChunkPacketData.java:132-137
fn block_entity(r: &mut Reader) -> DecodeResult<BlockEntityInfo> {
    let packed_xz = r.u8()?;
    let y = r.i16()?;
    let kind = r.varint()?;
    let tag = r.nbt()?;
    Ok(BlockEntityInfo {
        packed_xz,
        y,
        kind,
        tag: (tag[0] != 0).then(|| tag.into()),
    })
}

/// ClientboundLightUpdatePacketData.java:51-56
fn light_data(r: &mut Reader) -> DecodeResult<LightData> {
    let layers =
        |r: &mut Reader| r.list(usize::MAX, |r| Ok(r.byte_array(LIGHT_LAYER_BYTES)?.into()));
    Ok(LightData {
        sky_y_mask: r.long_array()?,
        block_y_mask: r.long_array()?,
        empty_sky_y_mask: r.long_array()?,
        empty_block_y_mask: r.long_array()?,
        sky_updates: layers(r)?,
        block_updates: layers(r)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::write_varint;

    fn decode(name: &str, bytes: &[u8]) -> ChunkPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a chunk packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    fn long_array(out: &mut Vec<u8>, longs: &[u64]) {
        write_varint(out, longs.len() as u32);
        for l in longs {
            out.extend(l.to_be_bytes());
        }
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn level_chunk_with_light() {
        let mut bytes = Vec::new();
        bytes.extend(3i32.to_be_bytes());
        bytes.extend((-2i32).to_be_bytes());
        // Two heightmaps: WORLD_SURFACE (1) and MOTION_BLOCKING (4).
        write_varint(&mut bytes, 2);
        write_varint(&mut bytes, 1);
        long_array(&mut bytes, &[7]);
        write_varint(&mut bytes, 4);
        long_array(&mut bytes, &[8, 9]);
        write_varint(&mut bytes, 3);
        bytes.extend([1, 2, 3]);
        // One at section-relative (5, 9) with an empty compound, one with an
        // end tag.
        write_varint(&mut bytes, 2);
        bytes.extend([0x59, 0, 64, 2, 10, 0]);
        bytes.extend([0x00, 0xFF, 0xC0, 2, 0]);
        // Sky mask over two longs, empty block mask, one sky layer.
        long_array(&mut bytes, &[1, 1 << 63]);
        long_array(&mut bytes, &[]);
        long_array(&mut bytes, &[]);
        long_array(&mut bytes, &[2]);
        write_varint(&mut bytes, 1);
        write_varint(&mut bytes, 2048);
        bytes.extend([0xAB; 2048]);
        write_varint(&mut bytes, 0);
        assert_eq!(
            decode("level_chunk_with_light", &bytes),
            ChunkPacket::Level {
                x: 3,
                z: -2,
                heightmaps: vec![(1, vec![7].into()), (4, vec![8, 9].into())],
                sections: vec![1, 2, 3].into(),
                block_entities: vec![
                    BlockEntityInfo {
                        packed_xz: 0x59,
                        y: 64,
                        kind: 2,
                        tag: Some(vec![10, 0].into()),
                    },
                    BlockEntityInfo {
                        packed_xz: 0,
                        y: -64,
                        kind: 2,
                        tag: None,
                    },
                ],
                light: LightData {
                    sky_y_mask: vec![1, 1 << 63],
                    empty_block_y_mask: vec![2],
                    sky_updates: vec![vec![0xAB; 2048].into()],
                    ..LightData::default()
                },
            }
        );
    }

    #[test]
    fn oversized_sections_and_layers_are_rejected() {
        let mut bytes = Vec::new();
        bytes.extend([0; 8]);
        write_varint(&mut bytes, 0);
        write_varint(&mut bytes, MAX_SECTION_BYTES as u32 + 1);
        let r = read("level_chunk_with_light", &mut Reader::new(&bytes));
        assert!(r.unwrap().is_err());

        let mut bytes = vec![0, 0];
        for _ in 0..4 {
            long_array(&mut bytes, &[]);
        }
        write_varint(&mut bytes, 1);
        write_varint(&mut bytes, 2049);
        assert!(
            read("light_update", &mut Reader::new(&bytes))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn small_packets() {
        // `ChunkPos.pack(-3, -7)`.
        let packed = (-3i64 & 0xFFFF_FFFF) | (-7i64 << 32);
        assert_eq!(
            decode("forget_level_chunk", &packed.to_be_bytes()),
            ChunkPacket::Forget { x: -3, z: -7 }
        );
        assert_eq!(decode("chunk_batch_start", &[]), ChunkPacket::BatchStart);
        assert_eq!(
            decode("chunk_batch_finished", &[0x40]),
            ChunkPacket::BatchFinished { size: 64 }
        );
        let mut center = Vec::new();
        write_varint(&mut center, -1i32 as u32);
        write_varint(&mut center, 1);
        assert_eq!(
            decode("set_chunk_cache_center", &center),
            ChunkPacket::CacheCenter { x: -1, z: 1 }
        );
        assert_eq!(
            decode("set_chunk_cache_radius", &[12]),
            ChunkPacket::CacheRadius { radius: 12 }
        );
        assert_eq!(
            decode("light_update", &[5, 6, 0, 0, 0, 0, 0, 0]),
            ChunkPacket::Light {
                x: 5,
                z: 6,
                light: LightData::default()
            }
        );
    }
}
