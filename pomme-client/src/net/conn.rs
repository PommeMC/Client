//! A Minecraft protocol connection over either transport.
//!
//! Replaces `azalea_protocol::connect::Connection`, which is pinned to a TCP
//! stream's owned halves and cannot be rebuilt over another transport because
//! its half wrappers carry private `PhantomData`. The framing beneath it is
//! already transport-generic, so this owns only the struct layer.
//!
//! There is also no connection phase here. Azalea makes it a type parameter and
//! re-types the whole connection on every transition; pomme names the packet
//! enum at each call site anyway, and dropping it makes the mid-session
//! reconfiguration excursion ordinary calls on one object.

use std::fmt::Debug;
use std::io::{self, Cursor};

use azalea_crypto::{Aes128CfbDec, Aes128CfbEnc};
use azalea_protocol::packets::ProtocolPacket;
use azalea_protocol::read::{ReadPacketError, deserialize_packet, read_raw_packet};
use azalea_protocol::write::{serialize_packet, write_raw_packet};
use tokio::io::{ReadHalf, SimplexStream, WriteHalf};
use tokio::net::TcpStream;

use super::stream::{NetReader, NetWriter};

pub struct RawReader {
    stream: NetReader,
    /// Accumulates wire bytes until they contain a whole frame; a single read
    /// can carry part of a packet, or several.
    buffer: Cursor<Vec<u8>>,
    compression_threshold: Option<u32>,
    dec_cipher: Option<Aes128CfbDec>,
}

pub struct RawWriter {
    stream: NetWriter,
    compression_threshold: Option<u32>,
    enc_cipher: Option<Aes128CfbEnc>,
}

/// Held as two fields rather than behind accessors so the game loop can read
/// and write in one `select!` on disjoint borrows.
pub struct Conn {
    pub reader: RawReader,
    pub writer: RawWriter,
}

impl RawReader {
    /// Reads one frame, decrypted and decompressed.
    // TODO: no maximum frame length, so a hostile length prefix buffers without
    // bound. Inherited from azalea's framing; cap it if framing moves here.
    pub async fn read(&mut self) -> Result<Box<[u8]>, Box<ReadPacketError>> {
        read_raw_packet(
            &mut self.stream,
            &mut self.buffer,
            self.compression_threshold,
            &mut self.dec_cipher,
        )
        .await
    }
}

impl RawWriter {
    /// Writes one already-serialized frame, compressing and encrypting it.
    pub async fn write(&mut self, frame: &[u8]) -> io::Result<()> {
        write_raw_packet(
            frame,
            &mut self.stream,
            self.compression_threshold,
            &mut self.enc_cipher,
        )
        .await
    }
}

impl Conn {
    pub fn from_tcp(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self::new(NetReader::Tcp(read), NetWriter::Tcp(write))
    }

    #[allow(dead_code)]
    pub fn from_memory(rx: ReadHalf<SimplexStream>, tx: WriteHalf<SimplexStream>) -> Self {
        Self::new(NetReader::Memory(rx), NetWriter::Memory(tx))
    }

    fn new(stream_in: NetReader, stream_out: NetWriter) -> Self {
        Self {
            reader: RawReader {
                stream: stream_in,
                buffer: Cursor::new(Vec::new()),
                compression_threshold: None,
                dec_cipher: None,
            },
            writer: RawWriter {
                stream: stream_out,
                compression_threshold: None,
                enc_cipher: None,
            },
        }
    }

    pub async fn read_packet<P: ProtocolPacket + Debug>(
        &mut self,
    ) -> Result<P, Box<ReadPacketError>> {
        let raw = self.reader.read().await?;
        deserialize_packet(&mut Cursor::new(&raw))
    }

    pub async fn write_packet<P: ProtocolPacket + Debug>(&mut self, packet: &P) -> io::Result<()> {
        let raw = serialize_packet(packet).map_err(io::Error::other)?;
        self.writer.write(&raw).await
    }

    /// A negative threshold disables compression, per the wire format.
    pub fn set_compression_threshold(&mut self, threshold: i32) {
        let threshold = u32::try_from(threshold).ok();
        self.reader.compression_threshold = threshold;
        self.writer.compression_threshold = threshold;
    }

    /// Arms both directions. The caller must have flushed the key packet first:
    /// it is the last plaintext frame.
    pub fn set_encryption_key(&mut self, key: [u8; 16]) {
        let (enc_cipher, dec_cipher) = azalea_crypto::create_cipher(&key);
        self.reader.dec_cipher = Some(dec_cipher);
        self.writer.enc_cipher = Some(enc_cipher);
    }
}

#[cfg(test)]
mod tests {
    use azalea_protocol::packets::status::ServerboundStatusPacket;
    use azalea_protocol::packets::status::s_ping_request::ServerboundPingRequest;
    use azalea_protocol::packets::status::s_status_request::ServerboundStatusRequest;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    const CAP: usize = 64 * 1024;

    /// Two connections joined by a pipe in each direction.
    fn pipe_pair() -> (Conn, Conn) {
        let (a_rx, b_tx) = tokio::io::simplex(CAP);
        let (b_rx, a_tx) = tokio::io::simplex(CAP);
        (Conn::from_memory(a_rx, a_tx), Conn::from_memory(b_rx, b_tx))
    }

    /// A connection plus the peer's raw ends of its two pipes.
    fn conn_and_wire() -> (Conn, ReadHalf<SimplexStream>, WriteHalf<SimplexStream>) {
        let (peer_rx, tx) = tokio::io::simplex(CAP);
        let (rx, peer_tx) = tokio::io::simplex(CAP);
        (Conn::from_memory(rx, tx), peer_rx, peer_tx)
    }

    fn ping(time: u64) -> ServerboundStatusPacket {
        ServerboundStatusPacket::PingRequest(ServerboundPingRequest { time })
    }

    fn status_request() -> ServerboundStatusPacket {
        ServerboundStatusPacket::StatusRequest(ServerboundStatusRequest {})
    }

    async fn recv(conn: &mut Conn) -> ServerboundStatusPacket {
        conn.read_packet().await.unwrap()
    }

    #[tokio::test]
    async fn round_trips_at_every_compression_setting() {
        // A ping is 9 bytes on the wire, so 256 takes the store-uncompressed
        // branch and 1 the deflate branch. A negative threshold disables
        // compression, as does never setting one.
        for threshold in [None, Some(-1), Some(256), Some(1)] {
            let (mut a, mut b) = pipe_pair();
            if let Some(threshold) = threshold {
                a.set_compression_threshold(threshold);
                b.set_compression_threshold(threshold);
            }
            a.write_packet(&ping(7)).await.unwrap();
            assert_eq!(recv(&mut b).await, ping(7), "threshold {threshold:?}");
        }
    }

    #[tokio::test]
    async fn round_trips_encrypted() {
        let (mut a, mut b) = pipe_pair();
        let key = [7u8; 16];
        a.set_encryption_key(key);
        b.set_encryption_key(key);
        // Two packets: the stream cipher must stay in step across frames.
        a.write_packet(&ping(1)).await.unwrap();
        a.write_packet(&ping(2)).await.unwrap();
        assert_eq!(recv(&mut b).await, ping(1));
        assert_eq!(recv(&mut b).await, ping(2));
    }

    #[tokio::test]
    async fn frame_is_length_then_id() {
        let (mut conn, mut wire_rx, _peer_tx) = conn_and_wire();
        conn.write_packet(&status_request()).await.unwrap();

        let mut frame = [0u8; 2];
        wire_rx.read_exact(&mut frame).await.unwrap();
        assert_eq!(frame, [0x01, 0x00], "one byte of payload, packet id zero");
    }

    /// A read can deliver part of a frame, or several frames at once.
    #[tokio::test]
    async fn frames_split_across_reads_reassemble() {
        let (mut conn, _wire_rx, mut peer_tx) = conn_and_wire();
        for chunk in [&[0x01u8][..], &[0x00, 0x01][..], &[0x00][..]] {
            peer_tx.write_all(chunk).await.unwrap();
        }

        for _ in 0..2 {
            assert_eq!(recv(&mut conn).await, status_request());
        }
    }
}
