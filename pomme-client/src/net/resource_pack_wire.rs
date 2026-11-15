//! In-game `resource_pack_push` and `resource_pack_pop`, decoded by pomme.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::resource_pack::{self, ResourcePackPacket};
use pomme_protocol::serverbound::{GamePacket, ResourcePackAction};

use super::NetworkEvent;
use super::handler::send_ordered;
use super::sender::PacketSender;

/// Decodes a resource pack packet; `None` for any other packet.
pub(super) fn handle_raw_resource_pack_packet(
    packet_id: u32,
    payload: &[u8],
    sender: &PacketSender,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, resource_pack::read)?.map(|packet| {
            if let Some(reply) = handle_resource_pack_packet(packet, event_tx) {
                sender.send(reply);
            }
        }),
    )
}

/// Forwards a resource pack packet, in whichever phase it came; the reply to
/// send, if it wants one.
pub(super) fn handle_resource_pack_packet(
    packet: ResourcePackPacket,
    event_tx: &Sender<NetworkEvent>,
) -> Option<GamePacket> {
    match packet {
        ResourcePackPacket::Push {
            id,
            url,
            hash,
            required,
            prompt: _,
        } => {
            let id = uuid::Uuid::from_u128(id);
            tracing::info!("Server pushing resource pack {id} (required: {required})");
            send_ordered(
                event_tx,
                NetworkEvent::ResourcePackPush {
                    id,
                    url,
                    hash,
                    required,
                },
            );
            Some(GamePacket::ResourcePack {
                id: id.as_u128(),
                action: ResourcePackAction::Accepted,
            })
        }
        ResourcePackPacket::Pop { id } => {
            let id = id.map(uuid::Uuid::from_u128);
            tracing::info!("Server popping resource pack {id:?}");
            send_ordered(event_tx, NetworkEvent::ResourcePackPop { id });
            None
        }
    }
}
