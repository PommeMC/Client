//! `waypoint`, decoded by pomme for the locator bar.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::waypoint;

use super::NetworkEvent;
use super::handler::send_ordered;

/// Decodes a waypoint packet; `None` for any other packet.
pub(super) fn handle_raw_waypoint_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, waypoint::read)?.map(|packet| {
            send_ordered(
                event_tx,
                NetworkEvent::Waypoint {
                    operation: packet.operation,
                    waypoint: packet.waypoint,
                },
            );
        }),
    )
}
