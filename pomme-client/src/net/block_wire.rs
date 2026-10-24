//! Block packets, decoded by pomme for every session: block state ids stay
//! in the wire version's id space (the connection's selected table reads them),
//! and only `block_entity_data`'s type needs a registry remap.

use azalea_block::BlockState;
use azalea_registry::Registry;
use azalea_registry::builtin::BlockEntityKind;
use crossbeam_channel::Sender;
use pomme_protocol::block::{self, BlockPacket};
use pomme_protocol::codec::DecodeResult;
use simdnbt::owned::NbtCompound;

use super::NetworkEvent;
use super::handler::send_ordered;
use crate::position::BlockPos;
use crate::world::block::BlockStateTable;

/// Decodes a block packet; `None` for any other packet.
pub(super) fn handle_raw_block_packet(
    packet_id: u32,
    payload: &[u8],
    block_state_table: BlockStateTable,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, block::read)?
            .map(|packet| dispatch(packet, block_state_table, event_tx)),
    )
}

fn dispatch(
    packet: BlockPacket,
    block_state_table: BlockStateTable,
    event_tx: &Sender<NetworkEvent>,
) {
    match packet {
        BlockPacket::Update { pos, state } => {
            if let Some(state) = state_of(block_state_table, state) {
                send_ordered(
                    event_tx,
                    NetworkEvent::BlockUpdate {
                        pos: block_pos(pos),
                        state,
                    },
                );
            }
        }
        BlockPacket::SectionUpdate { updates } => {
            let updates = updates
                .into_iter()
                .filter_map(|(pos, state)| {
                    Some((block_pos(pos), state_of(block_state_table, state)?))
                })
                .collect();
            send_ordered(event_tx, NetworkEvent::SectionBlocksUpdate { updates });
        }
        BlockPacket::ChangedAck { seq } => {
            send_ordered(event_tx, NetworkEvent::BlockChangedAck { seq });
        }
        BlockPacket::Event {
            pos, action, param, ..
        } => {
            let _ = event_tx.try_send(NetworkEvent::BlockEvent {
                pos: block_pos(pos),
                action_id: action,
                action_parameter: param,
            });
        }
        BlockPacket::EntityData { pos, kind, tag } => {
            let Some(kind) = native_block_entity_type(kind) else {
                tracing::debug!("Dropping block entity data for unknown type {kind}");
                return;
            };
            let Some(nbt) = block_entity_nbt(&tag) else {
                return;
            };
            let _ = event_tx.try_send(NetworkEvent::BlockEntityUpdate {
                pos: block_pos(pos),
                kind,
                nbt,
            });
        }
        BlockPacket::LevelEvent {
            event, pos, data, ..
        } => {
            let _ = event_tx.try_send(NetworkEvent::LevelEvent {
                event_type: event as u32,
                pos: block_pos(pos),
                data: data as u32,
            });
        }
    }
}

fn block_pos((x, y, z): block::BlockPos) -> BlockPos {
    BlockPos { x, y, z }
}

fn state_of(block_state_table: BlockStateTable, id: u32) -> Option<BlockState> {
    let state = block_state_table.try_state(id);
    if state.is_none() {
        tracing::debug!("Skipping unknown block state {id}");
    }
    state
}

/// A block entity's network-NBT tag as a compound; `None` if unreadable.
pub(super) fn block_entity_nbt(tag: &[u8]) -> Option<NbtCompound> {
    match super::chat::read_nbt_tag(tag, &mut 0) {
        Ok(tag) => Some(tag.into_compound().unwrap_or_default()),
        Err(e) => {
            tracing::debug!("Unreadable block entity data: {e}");
            None
        }
    }
}

/// The wire block entity type in the native registry; translated sessions
/// remap it from their version's ids.
pub(super) fn native_block_entity_type(id: u32) -> Option<BlockEntityKind> {
    BlockEntityKind::from_u32(super::translate::native_id(
        pomme_protocol::ClientRegistry::BlockEntityType,
        id,
    )?)
}
