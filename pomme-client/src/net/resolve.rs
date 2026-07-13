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
use azalea_protocol::packets::ClientIntention;
use azalea_protocol::packets::handshake::s_intention::ServerboundIntention;
use azalea_protocol::packets::handshake::{ClientboundHandshakePacket, ServerboundHandshakePacket};
use azalea_protocol::packets::status::c_status_response::ClientboundStatusResponse;
use azalea_protocol::packets::status::s_status_request::ServerboundStatusRequest;
use azalea_protocol::packets::status::{ClientboundStatusPacket, ServerboundStatusPacket};
use azalea_protocol::resolve::{ResolveError, resolve_address};
use thiserror::Error;
use tokio::net::TcpStream;

/// Socket addresses to try in order: IPv4 first, the SRV-correct ones (derived
/// from azalea's resolution) ahead of the system-resolver fallbacks.
pub async fn resolve_candidates(server: &ServerAddr) -> Result<Vec<SocketAddr>, ResolveError> {
    let mut candidates: Vec<SocketAddr> = Vec::new();

    // An IP literal needs no DNS; skip azalea's resolution, whose SRV lookup
    // runs even for literals and stalls badly on resolvers that drop the query.
    if let Ok(ip) = server.host.parse::<IpAddr>() {
        push_with_nat64(&mut candidates, SocketAddr::new(ip, server.port));
        return Ok(candidates);
    }

    // Bound both lookups so a stalled resolver can't hold the connection
    // hostage. The system resolver runs concurrently as a fallback; it also
    // covers `localhost`, which hickory can end up sending to DNS on Windows.
    let (primary, extra) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), resolve_address(server)),
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((server.host.as_str(), server.port)),
        ),
    );
    let primary = primary.unwrap_or_else(|_| Err(ResolveError::from("resolution timed out")));
    let extra: Vec<SocketAddr> = extra
        .ok()
        .and_then(Result::ok)
        .map(Iterator::collect)
        .unwrap_or_default();

    match primary {
        Ok(addr) => push_with_nat64(&mut candidates, addr),
        Err(e) if extra.is_empty() => return Err(e),
        Err(e) => tracing::warn!(
            "Resolving {} failed ({e}); using system resolver",
            server.host
        ),
    }
    candidates.extend(extra);

    // IPv4 before IPv6 (stable, so the SRV-correct entries stay ahead), deduped.
    candidates.sort_by_key(SocketAddr::is_ipv6);
    let mut seen = HashSet::new();
    candidates.retain(|a| seen.insert(*a));
    Ok(candidates)
}

/// Push `addr`, preceded by its embedded IPv4 if it's a NAT64-mapped IPv6
/// (well-known 64:ff9b::/96 prefix): that IPv4 is reachable even when the
/// NAT64 gateway isn't. Keeps the original (possibly SRV-resolved) port.
fn push_with_nat64(candidates: &mut Vec<SocketAddr>, addr: SocketAddr) {
    if let IpAddr::V6(v6) = addr.ip()
        && let Some(v4) = nat64_embedded_ipv4(v6)
    {
        candidates.push(SocketAddr::new(IpAddr::V4(v4), addr.port()));
    }
    candidates.push(addr);
}

/// The IPv4 embedded in a well-known-prefix (`64:ff9b::/96`) NAT64 address.
fn nat64_embedded_ipv4(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let o = addr.octets();
    (o[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0])
        .then(|| Ipv4Addr::new(o[12], o[13], o[14], o[15]))
}

pub type HandshakeConnection = Connection<ClientboundHandshakePacket, ServerboundHandshakePacket>;
pub type StatusConnection = Connection<ClientboundStatusPacket, ServerboundStatusPacket>;

/// Fetch `server`'s status response, returning the still-open connection for
/// a follow-up latency ping. Shared by the server-list ping and join-time
/// wire-version negotiation.
pub async fn request_status(
    server: &ServerAddr,
) -> Result<(ClientboundStatusResponse, StatusConnection), String> {
    let mut conn = connect(server, ClientIntention::Status)
        .await
        .map_err(|e| e.to_string())?
        .status();
    conn.write(ServerboundStatusRequest {})
        .await
        .map_err(|e| format!("Status request failed: {e}"))?;
    match conn.read().await.map_err(|e| format!("Read failed: {e}"))? {
        ClientboundStatusPacket::StatusResponse(s) => Ok((s, conn)),
        _ => Err("Unexpected packet".into()),
    }
}

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
                    protocol_version: crate::version::session_protocol(),
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
