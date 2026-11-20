//! A Minecraft protocol connection over either transport, TCP or the
//! in-process pipe to an integrated server: the stream, the read buffer and
//! encryption around `frame`'s frames.
//!
//! There is no connection phase here. A packet is already encoded for its
//! phase, so a mid-session reconfiguration is ordinary calls on one object.

use std::io::{self, Cursor};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, ReadHalf, SimplexStream, WriteHalf};
use tokio::net::TcpStream;

use super::crypt::{self, Decryptor, Encryptor};
use super::frame::{self, FrameError};
use super::stream::{NetReader, NetWriter};

/// How much one read takes off the stream.
const READ_CHUNK: usize = 64 * 1024;

pub struct RawReader {
    stream: NetReader,
    /// Accumulates wire bytes until they contain a whole frame; a single read
    /// can carry part of a packet, or several.
    buffer: Cursor<Vec<u8>>,
    /// Where a read lands before it joins `buffer`, so one dropped halfway
    /// (the loops read inside `select!`) leaves `buffer` as it was.
    chunk: Box<[u8]>,
    compression_threshold: Option<u32>,
    dec_cipher: Option<Decryptor>,
}

pub struct RawWriter {
    stream: NetWriter,
    compression_threshold: Option<u32>,
    enc_cipher: Option<Encryptor>,
}

/// Held as two fields rather than behind accessors so the game loop can read
/// and write in one `select!` on disjoint borrows.
pub struct Conn {
    /// Translation state owned by this connection; absent for native sessions.
    pub translation: Option<std::sync::Arc<super::translate::TranslationSession>>,
    pub reader: RawReader,
    pub writer: RawWriter,
}

impl RawReader {
    /// Reads one packet, decrypted and decompressed.
    pub async fn read(&mut self) -> Result<Box<[u8]>, FrameError> {
        loop {
            let consumed = self.buffer.position() as usize;
            if let Some((frame, taken)) = frame::split(&self.buffer.get_ref()[consumed..])? {
                let packet = frame::unpack(frame, self.compression_threshold.is_some())?;
                self.buffer.set_position((consumed + taken) as u64);
                self.compact();
                return Ok(packet);
            }
            let read = self.stream.read(&mut self.chunk).await?;
            if read == 0 {
                return Err(FrameError::Closed);
            }
            let received = &mut self.chunk[..read];
            if let Some(cipher) = &mut self.dec_cipher {
                crypt::decrypt(cipher, received);
            }
            self.buffer.get_mut().extend_from_slice(received);
        }
    }

    /// Drops the packets already handed out. A burst rarely ends on a frame
    /// boundary, so a join's chunks would otherwise all stay buffered.
    fn compact(&mut self) {
        /// Below this the buffer isn't worth moving.
        const MIN: usize = 64 * 1024;

        let consumed = self.buffer.position() as usize;
        let bytes = self.buffer.get_mut();
        if consumed == bytes.len() {
            bytes.clear();
            bytes.shrink_to(MIN);
            self.buffer.set_position(0);
            return;
        }
        // Only once the consumed part outweighs the rest, so the moves stay
        // linear in the bytes read.
        if consumed >= MIN && consumed >= bytes.len() - consumed {
            bytes.drain(..consumed);
            bytes.shrink_to(2 * bytes.len().max(MIN));
            self.buffer.set_position(0);
        }
    }
}

impl RawWriter {
    /// Writes one already-encoded packet, compressing and encrypting it.
    pub async fn write(&mut self, packet: &[u8]) -> io::Result<()> {
        let mut wire = frame::pack(packet, self.compression_threshold).map_err(io::Error::other)?;
        if let Some(cipher) = &mut self.enc_cipher {
            crypt::encrypt(cipher, &mut wire);
        }
        self.stream.write_all(&wire).await
    }
}

/// One side of an in-process connection.
pub struct MemoryEnd {
    pub rx: ReadHalf<SimplexStream>,
    pub tx: WriteHalf<SimplexStream>,
}

/// Pipes an integrated server and the client to each other, returning the
/// client's end and the server's.
///
/// Two independent pipes rather than a duplex pair, so neither direction waits
/// on a lock held by the other. The client-to-server pipe is oversized because
/// the game loop writes from the task that reads (see the TODO in `game_loop`).
#[allow(dead_code, reason = "only a singleplayer build opens a world")]
pub fn memory_pipes() -> (MemoryEnd, MemoryEnd) {
    const TO_CLIENT: usize = 1024 * 1024;
    const TO_SERVER: usize = 256 * 1024;

    let (client_rx, server_tx) = tokio::io::simplex(TO_CLIENT);
    let (server_rx, client_tx) = tokio::io::simplex(TO_SERVER);
    (
        MemoryEnd {
            rx: client_rx,
            tx: client_tx,
        },
        MemoryEnd {
            rx: server_rx,
            tx: server_tx,
        },
    )
}

impl Conn {
    pub fn is_encrypted(&self) -> bool {
        self.writer.enc_cipher.is_some()
    }

    pub fn from_tcp(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self::new(NetReader::Tcp(read), NetWriter::Tcp(write))
    }

    pub fn from_memory(end: MemoryEnd) -> Self {
        Self::new(NetReader::Memory(end.rx), NetWriter::Memory(end.tx))
    }

    fn new(stream_in: NetReader, stream_out: NetWriter) -> Self {
        Self {
            translation: None,
            reader: RawReader {
                stream: stream_in,
                buffer: Cursor::new(Vec::new()),
                chunk: vec![0; READ_CHUNK].into(),
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

    /// A negative threshold disables compression, per the wire format.
    pub fn set_compression_threshold(&mut self, threshold: i32) {
        let threshold = u32::try_from(threshold).ok();
        self.reader.compression_threshold = threshold;
        self.writer.compression_threshold = threshold;
    }

    /// Arms both directions. The caller must have flushed the key packet first:
    /// it is the last plaintext frame.
    pub fn set_encryption_key(&mut self, key: [u8; 16]) {
        let (enc_cipher, dec_cipher) = crypt::ciphers(&key);
        self.reader.dec_cipher = Some(dec_cipher);
        self.writer.enc_cipher = Some(enc_cipher);
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::status::{encode_ping_request as ping, encode_status_request};

    use super::*;

    /// Two connections joined by a pipe in each direction.
    fn pipe_pair() -> (Conn, Conn) {
        let (client, server) = memory_pipes();
        (Conn::from_memory(client), Conn::from_memory(server))
    }

    /// A connection plus the peer's raw ends of its two pipes.
    fn conn_and_wire() -> (Conn, ReadHalf<SimplexStream>, WriteHalf<SimplexStream>) {
        let (client, server) = memory_pipes();
        (Conn::from_memory(client), server.rx, server.tx)
    }

    #[test]
    fn compacting_drops_the_frames_already_read() {
        let (mut conn, ..) = conn_and_wire();
        let reader = &mut conn.reader;
        let unread = [7u8; 10];
        let read = 1 << 20;

        reader.buffer = Cursor::new([vec![0; read], unread.to_vec()].concat());
        reader.buffer.set_position(read as u64);
        reader.compact();
        assert_eq!(reader.buffer.position(), 0);
        assert_eq!(reader.buffer.get_ref()[..], unread);
        assert!(reader.buffer.get_ref().capacity() < read);

        // Too little read to be worth moving the rest.
        reader.buffer = Cursor::new(vec![0; read]);
        reader.buffer.set_position(1000);
        reader.compact();
        assert_eq!(reader.buffer.position(), 1000);
    }

    async fn recv(conn: &mut Conn) -> Vec<u8> {
        conn.reader.read().await.unwrap().into_vec()
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
            a.writer.write(&ping(7)).await.unwrap();
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
        a.writer.write(&ping(1)).await.unwrap();
        a.writer.write(&ping(2)).await.unwrap();
        assert_eq!(recv(&mut b).await, ping(1));
        assert_eq!(recv(&mut b).await, ping(2));
    }

    #[tokio::test]
    async fn frame_is_length_then_id() {
        let (mut conn, mut wire_rx, _peer_tx) = conn_and_wire();
        conn.writer.write(&encode_status_request()).await.unwrap();

        let mut frame = [0u8; 2];
        wire_rx.read_exact(&mut frame).await.unwrap();
        assert_eq!(frame, [0x01, 0x00], "one byte of payload, packet id zero");
    }

    /// The loops read inside `select!`, which drops a read that another
    /// branch beats; what it had taken off the stream must still be there.
    #[tokio::test]
    async fn a_dropped_read_loses_nothing() {
        let (mut conn, _wire_rx, mut peer_tx) = conn_and_wire();
        async fn dropped(conn: &mut Conn) {
            let read = conn.reader.read();
            let timed_out = tokio::time::timeout(std::time::Duration::from_millis(20), read).await;
            assert!(timed_out.is_err(), "a whole frame arrived");
        }

        dropped(&mut conn).await;
        peer_tx.write_all(&[0x09, 0x01, 0, 0]).await.unwrap();
        dropped(&mut conn).await;
        peer_tx.write_all(&[0, 0, 0, 0, 0, 7]).await.unwrap();
        assert_eq!(recv(&mut conn).await, ping(7));
    }

    /// A read can deliver part of a frame, or several frames at once.
    #[tokio::test]
    async fn frames_split_across_reads_reassemble() {
        let (mut conn, _wire_rx, mut peer_tx) = conn_and_wire();
        for chunk in [&[0x01u8][..], &[0x00, 0x01][..], &[0x00][..]] {
            peer_tx.write_all(chunk).await.unwrap();
        }

        for _ in 0..2 {
            assert_eq!(recv(&mut conn).await, encode_status_request());
        }
    }
}
