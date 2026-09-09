//! Transport halves for a Minecraft connection: a TCP socket, or an in-memory
//! pipe for an integrated server (vanilla uses a Netty local channel for the
//! same job).
//!
//! An enum rather than a boxed trait object: framing does many small reads and
//! a vtable hop per poll is measurable during the chunk-load burst.
//!
//! Nothing constructs the memory variants outside tests yet; the connect path
//! starts offering them when singleplayer lands.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, SimplexStream, WriteHalf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub enum NetReader {
    Tcp(OwnedReadHalf),
    #[allow(dead_code)]
    Memory(ReadHalf<SimplexStream>),
}

pub enum NetWriter {
    Tcp(OwnedWriteHalf),
    #[allow(dead_code)]
    Memory(WriteHalf<SimplexStream>),
}

impl AsyncRead for NetReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Self::Memory(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for NetWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            Self::Memory(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Memory(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Memory(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
