use azalea_protocol::packets::game::ServerboundGamePacket;
use tokio::sync::mpsc;

/// An outbound game packet: either an azalea-serialized packet or bytes
/// pre-encoded by `net::wire` (varint packet id + body).
pub enum Outbound {
    Packet(Box<ServerboundGamePacket>),
    Raw(Vec<u8>),
    ChatProcessed { signature: [u8; 256], shown: bool },
    ChatDeleted { signature: [u8; 256] },
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

    pub fn mark_chat_processed(&self, signature: [u8; 256], shown: bool) {
        self.queue(Outbound::ChatProcessed { signature, shown });
    }

    pub fn ignore_chat_signature(&self, signature: [u8; 256]) {
        self.queue(Outbound::ChatDeleted { signature });
    }

    fn queue(&self, out: Outbound) {
        if let Err(e) = self.tx.send(out) {
            tracing::error!("Failed to queue outbound packet: {e}");
        }
    }
}
