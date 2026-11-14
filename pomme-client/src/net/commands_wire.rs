//! `commands`, decoded by pomme.

use std::sync::Arc;

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;

use super::NetworkEvent;
use super::commands::{CommandTree, SharedCommandTree};
use super::handler::send_ordered;

/// Decodes the server's command tree; `None` for any other packet.
pub(super) fn handle_raw_commands_packet(
    packet_id: u32,
    payload: &[u8],
    shared_tree: &SharedCommandTree,
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, pomme_protocol::commands::read)?.map(|packet| {
            let nodes = packet.nodes.len();
            let tree = Arc::new(CommandTree::from_packet(packet));
            tracing::info!(
                "Command tree received: {nodes} nodes, root commands = {:?}",
                tree.root_child_names()
            );
            *shared_tree.lock() = Some(tree.clone());
            send_ordered(event_tx, NetworkEvent::CommandTree { tree });
        }),
    )
}
