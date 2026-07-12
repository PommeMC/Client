use azalea_protocol::packets::game::ServerboundGamePacket;
use tokio::sync::mpsc;

/// An outbound game packet: either an azalea-serialized packet or bytes
/// pre-encoded by `net::wire` (varint packet id + body).
pub enum Outbound {
    Packet(Box<ServerboundGamePacket>),
    Raw(Vec<u8>),
}

pub struct PacketSender {
    tx: mpsc::UnboundedSender<Outbound>,
}

impl PacketSender {
    pub fn new(tx: mpsc::UnboundedSender<Outbound>) -> Self {
        Self { tx }
    }

    pub fn send(&self, packet: ServerboundGamePacket) {
        self.queue(Outbound::Packet(Box::new(packet)));
    }

    pub fn send_raw(&self, bytes: Vec<u8>) {
        self.queue(Outbound::Raw(bytes));
    }

    fn queue(&self, out: Outbound) {
        if let Err(e) = self.tx.send(out) {
            tracing::error!("Failed to queue outbound packet: {e}");
        }
    }
}
