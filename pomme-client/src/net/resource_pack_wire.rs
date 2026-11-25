//! In-game `resource_pack_push` and `resource_pack_pop`, decoded by pomme.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::resource_pack::{self, ResourcePackPacket};

use super::NetworkEvent;
use super::handler::send_ordered;

/// Decodes a resource pack packet; `None` for any other packet.
pub(super) fn handle_raw_resource_pack_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, resource_pack::read)?
            .map(|packet| handle_resource_pack_packet(packet, event_tx)),
    )
}

/// Forwards a resource pack packet, in whichever phase it came; the main
/// thread answers it.
pub(super) fn handle_resource_pack_packet(
    packet: ResourcePackPacket,
    event_tx: &Sender<NetworkEvent>,
) {
    match packet {
        ResourcePackPacket::Push {
            id,
            url,
            hash,
            required,
            prompt,
        } => {
            let id = uuid::Uuid::from_u128(id);
            tracing::info!("Server pushing resource pack {id} (required: {required})");
            let prompt = prompt.and_then(|nbt| {
                super::chat::read_component(&nbt, &mut 0)
                    .inspect_err(|error| tracing::warn!("Resource pack prompt: {error}"))
                    .ok()
            });
            send_ordered(
                event_tx,
                NetworkEvent::ResourcePackPush {
                    id,
                    url,
                    hash,
                    required,
                    prompt,
                },
            );
        }
        ResourcePackPacket::Pop { id } => {
            let id = id.map(uuid::Uuid::from_u128);
            tracing::info!("Server popping resource pack {id:?}");
            send_ordered(event_tx, NetworkEvent::ResourcePackPop { id });
        }
    }
}
