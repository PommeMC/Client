//! `/connect/<host>/<port>`: a WebSocket from the page, bridged to a game
//! server's TCP port, since a page can't open sockets.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use hickory_resolver::config::{GOOGLE, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::{Resolver, TokioResolver};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::limits::Spend;
use crate::{AppState, Config, guard};

/// Vanilla's largest packet, plus room for its length prefix.
const MAX_MESSAGE: usize = (2 << 20) + 8;

/// Below this, ports belong to services such as mail, not game servers.
const FIRST_UNPRIVILEGED_PORT: u16 = 1024;

/// The system's DNS, or Google's where there's none, as the desktop client
/// resolves servers (`net::resolve`).
static RESOLVER: LazyLock<TokioResolver> = LazyLock::new(|| {
    Resolver::builder_tokio()
        .unwrap_or_else(|_| {
            Resolver::builder_with_config(
                ResolverConfig::udp_and_tcp(&GOOGLE),
                TokioRuntimeProvider::new(),
            )
        })
        .build()
        .expect("the DNS resolver builds")
});

/// Dials `host:port` for the page, then carries the connection's bytes over a
/// WebSocket. Only this relay's own page may, and only to servers the open
/// internet could reach anyway, other than the relay's own machine.
pub async fn connect(
    State(state): State<AppState>,
    Path((host, port)): Path<(String, u16)>,
    peer: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !guard::same_origin(&headers) {
        tracing::warn!("Refused a cross-site connection to {host}:{port}");
        return StatusCode::FORBIDDEN.into_response();
    }
    let client = guard::client(&state.config, peer, &headers);
    if !state.limits.allow(Spend::Connect, client) {
        tracing::warn!("Rate limited {client}'s connections");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Some(slot) = state.connections.open(&state.config, client) else {
        tracing::warn!("Refused {client} another connection");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let (host, port) = server_address(host, port).await;
    if port < FIRST_UNPRIVILEGED_PORT {
        return StatusCode::FORBIDDEN.into_response();
    }
    let addresses: Vec<SocketAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addresses) => addresses
            .filter(|address| reachable(&state.config, address.ip()))
            .collect(),
        Err(error) => {
            tracing::warn!("Couldn't resolve {host}: {error}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    if addresses.is_empty() {
        tracing::warn!("Refused {host}:{port}, which has no address it may reach");
        return StatusCode::FORBIDDEN.into_response();
    }
    // The checked addresses themselves, so DNS can't answer differently twice.
    let tcp = match tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(addresses.as_slice()),
    )
    .await
    {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => {
            tracing::warn!("Couldn't reach {host}:{port}: {error}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
        Err(_) => {
            tracing::warn!("Reaching {host}:{port} timed out");
            return StatusCode::GATEWAY_TIMEOUT.into_response();
        }
    };
    tracing::info!("Connected {client} to {host}:{port}");
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            bridge(socket, tcp).await;
            drop(slot);
        })
}

/// Where `host:port` is served: at the default port, the target of its
/// `_minecraft._tcp` SRV record when it has one, as the desktop client finds
/// it.
async fn server_address(host: String, port: u16) -> (String, u16) {
    if port != 25565 {
        return (host, port);
    }
    let Ok(lookup) = RESOLVER.srv_lookup(format!("_minecraft._tcp.{host}")).await else {
        return (host, port);
    };
    match lookup.answers().first().map(|record| &record.data) {
        Some(RData::SRV(srv)) => (srv.target.to_ascii(), srv.port),
        _ => (host, port),
    }
}

/// Whether the relay may dial `ip`: not one of its own `--block-address`es,
/// and public unless private servers are allowed.
fn reachable(config: &Config, ip: IpAddr) -> bool {
    let ip = unwrapped(ip);
    !config
        .block_addresses
        .iter()
        .any(|blocked| unwrapped(*blocked) == ip)
        && (config.allow_private_servers || public(ip))
}

/// Whether `ip` is on the open internet, not a network only the relay's
/// machine can see: loopback, private, link-local, shared (carrier NAT) or
/// otherwise reserved, including IPv4 carried inside IPv6.
fn public(ip: IpAddr) -> bool {
    match unwrapped(ip) {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first == 0x2001 && ip.segments()[1] == 0x0db8))
        }
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && ip.octets()[2] == 0)
        || (a == 198 && (18..20).contains(&b)))
}

/// `ip`, or the IPv4 address it carries as an IPv4-mapped or NAT64
/// (`64:ff9b::/96`) address.
fn unwrapped(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    let segments = v6.segments();
    let nat64 = segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
    v6.to_ipv4_mapped()
        .or_else(|| {
            nat64.then(|| {
                let [.., hi, lo] = segments;
                Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo))
            })
        })
        .map_or(ip, IpAddr::V4)
}

/// Copies bytes both ways until either side closes.
async fn bridge(socket: WebSocket, tcp: TcpStream) {
    let _ = tcp.set_nodelay(true);
    let (mut from_server, mut to_server) = tcp.into_split();
    let (mut to_page, mut from_page) = socket.split();
    let upstream = async {
        while let Some(Ok(message)) = from_page.next().await {
            match message {
                Message::Binary(bytes) => {
                    if to_server.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let downstream = async {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(read @ 1..) = from_server.read(&mut buffer).await {
            let bytes = Bytes::copy_from_slice(&buffer[..read]);
            if to_page.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = to_page.close().await;
    };
    tokio::select! {
        () = upstream => {}
        () = downstream => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_internet_addresses_are_public() {
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "198.18.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn blocked_addresses_stay_blocked_however_written() {
        let config = Config {
            allow_private_servers: true,
            block_addresses: vec!["203.0.113.7".parse().unwrap()],
            ..Config::default()
        };
        for ip in ["203.0.113.7", "::ffff:203.0.113.7", "64:ff9b::cb00:7107"] {
            assert!(!reachable(&config, ip.parse().unwrap()), "{ip}");
        }
        assert!(reachable(&config, "192.168.0.1".parse().unwrap()));
    }
}
