//! Chunk, light and chunk-batch packets, decoded by pomme for every session.

use std::sync::Arc;

use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_chunk_batch_received::ServerboundChunkBatchReceived;
use azalea_registry::builtin::BlockEntityKind;
use crossbeam_channel::Sender;
use pomme_protocol::chunk::{self, BlockEntityInfo, ChunkPacket};
use pomme_protocol::codec::DecodeResult;
use simdnbt::owned::NbtCompound;

use super::block_wire::{block_entity_nbt, native_block_entity_type};
use super::chunk_batch::ChunkBatchSizeCalculator;
use super::handler::send_ordered;
use super::sender::PacketSender;
use super::{NetworkEvent, PacketLightData};
use crate::position::{BlockPos, ChunkPos};
use crate::world::chunk::HeightmapKind;

/// Decodes a chunk packet; `None` for any other packet.
pub(super) fn handle_raw_chunk_packet(
    packet_id: u32,
    payload: &[u8],
    sender: &PacketSender,
    batch_size_calculator: &mut ChunkBatchSizeCalculator,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, chunk::read)?.map(|packet| match packet {
            ChunkPacket::Level {
                x,
                z,
                heightmaps,
                sections,
                block_entities,
                light,
            } => {
                let pos = ChunkPos::new(x, z);
                tracing::trace!("Chunk [{x}, {z}] ({} block entities)", block_entities.len());
                send_ordered(
                    event_tx,
                    NetworkEvent::ChunkLoaded {
                        pos,
                        data: Arc::new(sections),
                        heightmaps: heightmaps
                            .into_iter()
                            .filter_map(|(id, data)| Some((HeightmapKind::from_id(id)?, data)))
                            .collect(),
                        light: light.into(),
                    },
                );
                let entries = block_entities
                    .iter()
                    .filter_map(|be| block_entity(pos, be))
                    .collect();
                send_ordered(
                    event_tx,
                    NetworkEvent::BlockEntitySync {
                        chunk_pos: pos,
                        entries,
                    },
                );
            }
            ChunkPacket::Light { x, z, light } => {
                send_ordered(
                    event_tx,
                    NetworkEvent::LightUpdate {
                        pos: ChunkPos::new(x, z),
                        light: light.into(),
                    },
                );
            }
            ChunkPacket::Forget { x, z } => {
                send_ordered(
                    event_tx,
                    NetworkEvent::ChunkUnloaded {
                        pos: ChunkPos::new(x, z),
                    },
                );
            }
            ChunkPacket::Biomes(chunks) => {
                let chunks = chunks
                    .into_iter()
                    .map(|chunk| (ChunkPos::new(chunk.x, chunk.z), chunk.data))
                    .collect();
                send_ordered(event_tx, NetworkEvent::ChunkBiomes { chunks });
            }
            ChunkPacket::BatchStart => batch_size_calculator.on_batch_start(),
            ChunkPacket::BatchFinished { size } => {
                // Answered on the network thread: vanilla's
                // `handleChunkBatchFinished` is one of the few handlers it
                // doesn't defer to the main thread.
                batch_size_calculator.on_batch_finished(size);
                sender.send(ServerboundGamePacket::ChunkBatchReceived(
                    ServerboundChunkBatchReceived {
                        desired_chunks_per_tick: batch_size_calculator.desired_chunks_per_tick(),
                    },
                ));
            }
            ChunkPacket::CacheCenter { x, z } => {
                send_ordered(event_tx, NetworkEvent::ChunkCacheCenter { x, z });
            }
            ChunkPacket::CacheRadius { radius } => {
                send_ordered(
                    event_tx,
                    NetworkEvent::ServerViewDistance {
                        distance: radius as u32,
                    },
                );
            }
        }),
    )
}

/// A chunk's block entity at its world position; `None` drops it.
fn block_entity(
    chunk: ChunkPos,
    be: &BlockEntityInfo,
) -> Option<(BlockPos, BlockEntityKind, NbtCompound)> {
    let pos = BlockPos {
        x: chunk.x * 16 + i32::from(be.packed_xz >> 4),
        y: i32::from(be.y),
        z: chunk.z * 16 + i32::from(be.packed_xz & 0x0F),
    };
    let nbt = match &be.tag {
        Some(tag) => block_entity_nbt(tag)?,
        None => Default::default(),
    };
    Some((pos, native_block_entity_type(be.kind)?, nbt))
}

impl From<chunk::LightData> for PacketLightData {
    fn from(light: chunk::LightData) -> Self {
        Self {
            sky_updates: Arc::new(light.sky_updates.into()),
            block_updates: Arc::new(light.block_updates.into()),
            sky_y_mask: light.sky_y_mask.into(),
            block_y_mask: light.block_y_mask.into(),
            empty_sky_y_mask: light.empty_sky_y_mask.into(),
            empty_block_y_mask: light.empty_block_y_mask.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_entities_land_at_their_world_position() {
        let be = BlockEntityInfo {
            packed_xz: 0x59,
            y: -64,
            kind: 7,
            tag: None,
        };
        let (pos, kind, nbt) = block_entity(ChunkPos::new(-2, 3), &be).unwrap();
        assert_eq!(pos, BlockPos::new(-27, -64, 57));
        assert_eq!(kind, native_block_entity_type(7).unwrap());
        assert!(nbt.is_empty());
    }
}
