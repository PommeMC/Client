use azalea_protocol::packets::game::ServerboundGamePacket;
use tokio::sync::mpsc;

pub struct PacketSender {
    tx: mpsc::UnboundedSender<ServerboundGamePacket>,
}

impl PacketSender {
    pub fn new(tx: mpsc::UnboundedSender<ServerboundGamePacket>) -> Self {
        Self { tx }
    }

    pub fn send(&self, packet: ServerboundGamePacket) {
        if let Err(e) = self.tx.send(packet) {
            tracing::error!("Failed to queue outbound packet: {e}");
        }
    }
}
