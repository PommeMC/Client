//! Resolve a server address to connectable socket addresses, IPv4 first.
//!
//! azalea uses the single "first" resolved IP, which on a dual-stack host can
//! be an unreachable IPv6, such as a NAT64-synthesized AAAA whose gateway
//! doesn't route while the plain IPv4 is fine. We gather every address we can,
//! prefer IPv4, and let the caller try them in order so a dead IPv6 doesn't
//! blackhole the connection.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use azalea_protocol::address::ServerAddr;
use azalea_protocol::connect::{Connection, ConnectionError};
use azalea_protocol::packets::handshake::s_intention::ServerboundIntention;
use azalea_protocol::packets::handshake::{ClientboundHandshakePacket, ServerboundHandshakePacket};
use azalea_protocol::packets::{ClientIntention, PROTOCOL_VERSION};
use azalea_protocol::resolve::{ResolveError, resolve_address};
use thiserror::Error;
use tokio::net::TcpStream;

/// Socket addresses to try in order: IPv4 first, the SRV-correct ones (derived
/// from azalea's resolution) ahead of the system-resolver fallbacks.
pub async fn resolve_candidates(server: &ServerAddr) -> Result<Vec<SocketAddr>, ResolveError> {
    let primary = resolve_address(server).await?;
    let mut candidates: Vec<SocketAddr> = Vec::new();

    // A NAT64-mapped IPv6 (well-known 64:ff9b::/96 prefix) embeds the real IPv4
    // in its low 32 bits; that IPv4 is reachable even when the NAT64 gateway
    // isn't. Prefer it, keeping azalea's (SRV-resolved) port.
    if let IpAddr::V6(v6) = primary.ip()
        && let Some(v4) = nat64_embedded_ipv4(v6)
    {
        candidates.push(SocketAddr::new(IpAddr::V4(v4), primary.port()));
    }
    candidates.push(primary);

    // System resolver as a general fallback (covers normal dual-stack hosts).
    if let Ok(extra) = tokio::net::lookup_host((server.host.as_str(), server.port)).await {
        candidates.extend(extra);
    }

    // IPv4 before IPv6 (stable, so the SRV-correct entries stay ahead), deduped.
    candidates.sort_by_key(SocketAddr::is_ipv6);
    let mut seen = HashSet::new();
    candidates.retain(|a| seen.insert(*a));
    Ok(candidates)
}

/// The IPv4 embedded in a well-known-prefix (`64:ff9b::/96`) NAT64 address.
fn nat64_embedded_ipv4(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let o = addr.octets();
    (o[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0])
        .then(|| Ipv4Addr::new(o[12], o[13], o[14], o[15]))
}

pub type HandshakeConnection = Connection<ClientboundHandshakePacket, ServerboundHandshakePacket>;

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("{0}")]
    Resolve(ResolveError),
    #[error("{0}")]
    Unreachable(std::io::Error),
    #[error("{0}")]
    Handshake(ConnectionError),
}

/// Resolve `server` and open a handshake-stage connection with the given intent
/// to the first reachable candidate (IPv4 first), bounding each attempt by 5s.
/// Shared by the join and ping paths.
pub async fn connect(
    server: &ServerAddr,
    intention: ClientIntention,
) -> Result<HandshakeConnection, ConnectError> {
    let candidates = resolve_candidates(server)
        .await
        .map_err(ConnectError::Resolve)?;

    let mut last_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses resolved");
    for addr in &candidates {
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                tracing::info!("Connecting to {} (resolved: {addr})...", server.host);
                let mut conn = Connection::new_from_stream(stream)
                    .await
                    .map_err(ConnectError::Handshake)?;
                conn.write(ServerboundIntention {
                    protocol_version: PROTOCOL_VERSION,
                    hostname: server.host.clone(),
                    port: server.port,
                    intention,
                })
                .await
                .map_err(|e| ConnectError::Handshake(e.into()))?;
                return Ok(conn);
            }
            Ok(Err(e)) => {
                tracing::warn!("Connect to {addr} failed: {e}");
                last_err = e;
            }
            Err(_) => {
                tracing::warn!("Connect to {addr} timed out");
                last_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out");
            }
        }
    }
    Err(ConnectError::Unreachable(last_err))
}
