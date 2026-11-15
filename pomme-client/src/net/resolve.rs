//! Resolve a server address to connectable socket addresses, IPv4 first.
//!
//! The "first" resolved IP can be an unreachable IPv6 on a dual-stack host,
//! such as a NAT64-synthesized AAAA whose gateway doesn't route while the
//! plain IPv4 is fine. We gather every address we can, prefer IPv4, and let
//! the caller try them in order so a dead IPv6 doesn't blackhole the
//! connection.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;

use hickory_resolver::config::{GOOGLE, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::{Resolver, TokioResolver};
use pomme_protocol::status::{self, ClientIntent, Intention, ServerStatus, StatusPacket};
use thiserror::Error;
use tokio::net::TcpStream;

use super::address::{DEFAULT_PORT, ServerAddr};
use super::conn::Conn;

/// Why an address didn't resolve.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct ResolveError(String);

static RESOLVER: LazyLock<TokioResolver> = LazyLock::new(|| {
    Resolver::builder_tokio()
        .unwrap_or_else(|_| {
            tracing::warn!("System DNS resolver unavailable; falling back to Google DNS.");
            Resolver::builder_with_config(
                ResolverConfig::udp_and_tcp(&GOOGLE),
                TokioRuntimeProvider::new(),
            )
        })
        .build()
        .expect("DNS resolver")
});

/// Vanilla `ServerNameResolver.resolveAddress`: `server`'s address, or its SRV
/// redirect's when it has one.
async fn resolve_address(server: &ServerAddr) -> Result<SocketAddr, ResolveError> {
    let redirect = srv_redirect(server).await;
    let server = redirect.as_ref().unwrap_or(server);
    if let Ok(ip) = server.host.parse() {
        return Ok(SocketAddr::new(ip, server.port));
    }
    let lookup = RESOLVER
        .lookup_ip(server.host.as_str())
        .await
        .map_err(|e| ResolveError(e.to_string()))?;
    let ip = lookup
        .iter()
        .next()
        .ok_or_else(|| ResolveError("No A/AAAA record found".into()))?;
    Ok(SocketAddr::new(ip, server.port))
}

/// Vanilla `ServerRedirectHandler`: the target of `_minecraft._tcp.<host>`'s
/// first SRV record, asked for only on the default port.
async fn srv_redirect(server: &ServerAddr) -> Option<ServerAddr> {
    if server.port != DEFAULT_PORT {
        return None;
    }
    let lookup = RESOLVER
        .srv_lookup(format!("_minecraft._tcp.{}", server.host))
        .await
        .ok()?;
    let RData::SRV(srv) = &lookup.answers().first()?.data else {
        return None;
    };
    Some(ServerAddr {
        host: srv.target.to_ascii(),
        port: srv.port,
    })
}

/// Socket addresses to try in order: IPv4 first, the SRV-correct ones ahead
/// of the system-resolver fallbacks.
pub async fn resolve_candidates(server: &ServerAddr) -> Result<Vec<SocketAddr>, ResolveError> {
    let mut candidates: Vec<SocketAddr> = Vec::new();

    // An IP literal needs no DNS; an SRV lookup for one stalls badly on
    // resolvers that drop the query.
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
    let primary = primary.unwrap_or_else(|_| Err(ResolveError("resolution timed out".into())));
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

/// Fetch `server`'s status response, returning the still-open connection for
/// a follow-up latency ping. Shared by the server-list ping and join-time
/// wire-version negotiation.
pub async fn request_status(server: &ServerAddr) -> Result<(ServerStatus, Conn), String> {
    let mut conn = connect(server, ClientIntent::Status)
        .await
        .map_err(|e| e.to_string())?;
    conn.writer
        .write(&status::encode_status_request())
        .await
        .map_err(|e| format!("Status request failed: {e}"))?;
    match read_status(&mut conn).await {
        Ok(StatusPacket::Response(status)) => Ok((status, conn)),
        Ok(StatusPacket::Pong(_)) => Err("Unexpected packet".into()),
        Err(e) => Err(format!("Read failed: {e}")),
    }
}

/// The next status packet from the server.
pub async fn read_status(conn: &mut Conn) -> Result<StatusPacket, String> {
    let frame = conn.reader.read().await.map_err(|e| e.to_string())?;
    status::read_frame(&frame).map_err(|e| e.to_string())
}

/// Opens the handshake, naming the protocol this session speaks.
pub async fn send_intention(
    conn: &mut Conn,
    host: &str,
    port: u16,
    intent: ClientIntent,
) -> std::io::Result<()> {
    let intention = Intention {
        protocol_version: crate::version::session_protocol(),
        host: host.to_owned(),
        port,
        intent,
    };
    conn.writer.write(&intention.encode()).await
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("{0}")]
    Resolve(ResolveError),
    /// No candidate accepted a socket, or the handshake could not be sent.
    #[error("{0}")]
    Io(std::io::Error),
}

/// Resolve `server` and open a handshake-stage connection with the given intent
/// to the first reachable candidate (IPv4 first), bounding each attempt by 5s.
/// Shared by the join and ping paths.
pub async fn connect(server: &ServerAddr, intent: ClientIntent) -> Result<Conn, ConnectError> {
    let candidates = resolve_candidates(server)
        .await
        .map_err(ConnectError::Resolve)?;

    let mut last_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses resolved");
    for addr in &candidates {
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                let _ = stream.set_nodelay(true);
                tracing::info!("Connecting to {} (resolved: {addr})...", server.host);
                let mut conn = Conn::from_tcp(stream);
                send_intention(&mut conn, &server.host, server.port, intent)
                    .await
                    .map_err(ConnectError::Io)?;
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
    Err(ConnectError::Io(last_err))
}
