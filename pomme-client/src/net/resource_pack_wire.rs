//! In-game `resource_pack_push` and `resource_pack_pop`, decoded by pomme.

use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_protocol::packets::game::s_resource_pack::{Action, ServerboundResourcePack};
use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::resource_pack::{self, ResourcePackPacket};

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
        super::read_raw_packet(packet_id, payload, resource_pack::read)?.map(
            |packet| match packet {
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
                    sender.send(ServerboundGamePacket::ResourcePack(
                        ServerboundResourcePack {
                            id,
                            action: Action::Accepted,
                        },
                    ));
                }
                ResourcePackPacket::Pop { id } => {
                    let id = id.map(uuid::Uuid::from_u128);
                    tracing::info!("Server popping resource pack {id:?}");
                    send_ordered(event_tx, NetworkEvent::ResourcePackPop { id });
                }
            },
        ),
    )
}
