//! `/connect/<host>/<port>`: a WebSocket from the page, bridged to a game
//! server's TCP port, since a page can't open sockets.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Once};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::select_ok;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt as _, StreamExt as _};
use hickory_resolver::config::{GOOGLE, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::{Resolver, TokioResolver};
use pomme_protocol::status::Intention;
use pomme_protocol::wire::read_varint;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::limits::{Limits, Spend};
use crate::{AppState, Config, guard};

/// Several times what the page sends in one message, which is a piece of the
/// stream and not a whole packet.
const MAX_MESSAGE: usize = 256 * 1024;

/// How long each step of reaching a server may take: finding its SRV record,
/// resolving its name, and connecting to it.
const REACH_WAIT: Duration = Duration::from_secs(5);

/// A connection silent both ways for this long is closed. A server in play
/// sends a keep-alive every 15 seconds, and the client drops one that goes
/// quiet for 30.
const IDLE: Duration = Duration::from_secs(60);

/// How long the page has to send its handshake.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);

/// More than a handshake frame takes: its host is at most 255 characters.
const MAX_HANDSHAKE: usize = 1024;

/// The bytes a frame's length may take, past which a server drops the
/// connection.
const MAX_LENGTH_PREFIX: usize = 3;

/// What a message costs its client on top of its bytes, for its frame and
/// the handling of it, so that empty ones aren't free.
pub const MESSAGE_COST: usize = 16;

/// The most of a name's IPv4 addresses, and of its IPv6 ones, dialed at once.
const DIALED_PER_FAMILY: usize = 2;

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
    if !guard::same_origin(&state.config, &headers) {
        tracing::debug!("Refused a cross-site connection to {host:?}:{port}");
        return StatusCode::FORBIDDEN.into_response();
    }
    let client = guard::client(&state.config, peer, &headers);
    if !state.limits.allow(Spend::Connect, client) {
        tracing::debug!("Rate limited {client}'s connections");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let (host, port) = server_address(host, port).await;
    if port < FIRST_UNPRIVILEGED_PORT {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Not the system's lookup, which holds a thread until it gives up however
    // long ago the wait for it ended.
    let lookup = RESOLVER.lookup_ip(host.as_str());
    // A name that doesn't resolve is refused as one with no address the relay
    // may reach is, so the answer doesn't say which names exist behind it.
    let found = in_time("resolve", &host, lookup).await;
    let resolved = found.iter().flat_map(|found| found.iter());
    let addresses: Vec<SocketAddr> = dialed(resolved, |ip| reachable(&state.config, ip))
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    if addresses.is_empty() {
        if found.is_ok() {
            tracing::warn!("Refused {host:?}:{port}, which has no address it may reach");
        }
        return StatusCode::FORBIDDEN.into_response();
    }
    // Only now, so a name that's slow to resolve doesn't hold a slot.
    let Some(slot) = state.connections.open(&state.config, client) else {
        tracing::warn!("Refused {client} another connection");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    // The checked addresses themselves, so DNS can't answer differently
    // twice, and together, so one that never answers doesn't use up the wait.
    let dials = addresses.into_iter().map(TcpStream::connect).map(Box::pin);
    let tcp = match in_time("reach", &host, select_ok(dials)).await {
        Ok((tcp, _)) => tcp,
        Err(status) => return status.into_response(),
    };
    tracing::info!("Connected {client} to {host:?}:{port}");
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            bridge(socket, tcp, &state.limits, client).await;
            drop(slot);
        })
}

/// What `step` gives within [`REACH_WAIT`], or the status for it failing or
/// taking longer, having logged which.
async fn in_time<T, E: std::fmt::Display>(
    verb: &str,
    host: &str,
    step: impl Future<Output = Result<T, E>>,
) -> Result<T, StatusCode> {
    match tokio::time::timeout(REACH_WAIT, step).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::warn!("Couldn't {verb} {host:?}: {error}");
            Err(StatusCode::BAD_GATEWAY)
        }
        Err(_) => {
            tracing::warn!("Couldn't {verb} {host:?} in time");
            Err(StatusCode::GATEWAY_TIMEOUT)
        }
    }
}

/// Where `host:port` is served: at the default port, the target of its
/// `_minecraft._tcp` SRV record when it has one, as the desktop client finds
/// it.
async fn server_address(host: String, port: u16) -> (String, u16) {
    if port != 25565 {
        return (host, port);
    }
    let lookup = RESOLVER.srv_lookup(format!("_minecraft._tcp.{host}"));
    let Ok(Ok(lookup)) = tokio::time::timeout(REACH_WAIT, lookup).await else {
        return (host, port);
    };
    let served = lookup
        .answers()
        .iter()
        .find_map(|record| match &record.data {
            RData::SRV(srv) => Some((srv.target.to_ascii(), srv.port)),
            _ => None,
        });
    served.unwrap_or((host, port))
}

/// The addresses to dial of those a name has: the first `reachable` ones of
/// each family, so its IPv6 ones don't crowd out the rest on a relay with no
/// IPv6 route. A family with enough isn't checked further, however many
/// addresses the name has.
fn dialed(
    addresses: impl Iterator<Item = IpAddr>,
    reachable: impl Fn(IpAddr) -> bool,
) -> impl Iterator<Item = IpAddr> {
    let mut found = [0; 2];
    addresses.filter(move |ip| {
        let found = &mut found[usize::from(ip.is_ipv6())];
        let dial = *found < DIALED_PER_FAMILY && reachable(*ip);
        *found += usize::from(dial);
        dial
    })
}

/// Whether the relay may dial `ip`: not a `--block-address`, and unless
/// private servers are allowed, public and not the relay's own machine. Each
/// holds for `ip` as written and for the IPv4 address it may carry.
fn reachable(config: &Config, ip: IpAddr) -> bool {
    let carried = Some(unwrapped(ip)).filter(|carried| *carried != ip);
    let mut forms = [Some(ip), carried].into_iter().flatten();
    let blocked = |ip| {
        config
            .block_addresses
            .iter()
            .any(|network| network.contains(ip))
    };
    !forms.clone().any(blocked) && (config.allow_private_servers || (public(ip) && !forms.any(own)))
}

/// A `--block-address`: one address, or every address under a prefix.
#[derive(Clone, Copy)]
pub struct Network {
    prefix: u128,
    bits: u32,
}

impl Network {
    /// Whether it holds `ip`.
    fn contains(self, ip: IpAddr) -> bool {
        let differing = (as_v6(ip) ^ self.prefix).checked_shr(u128::BITS - self.bits);
        differing.unwrap_or(0) == 0
    }
}

impl FromStr for Network {
    type Err = &'static str;

    fn from_str(written: &str) -> Result<Self, Self::Err> {
        let (address, bits) = match written.split_once('/') {
            Some((address, bits)) => (address, Some(bits)),
            None => (written, None),
        };
        let address: IpAddr = address.parse().map_err(|_| "expected an IP address")?;
        let Some(bits) = bits else {
            return Ok(Self {
                prefix: as_v6(unwrapped(address)),
                bits: u128::BITS,
            });
        };
        let (width, mapped) = match address {
            IpAddr::V4(_) => (Ipv4Addr::BITS, u128::BITS - Ipv4Addr::BITS),
            IpAddr::V6(_) => (u128::BITS, 0),
        };
        let bits = bits.parse::<u32>().ok().filter(|bits| *bits <= width);
        Ok(Self {
            prefix: as_v6(address),
            bits: mapped + bits.ok_or("expected a prefix length the address has")?,
        })
    }
}

/// `ip` as IPv6 bits, an IPv4 address as its IPv4-mapped one.
fn as_v6(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(ip) => ip.to_ipv6_mapped().into(),
        IpAddr::V6(ip) => ip.into(),
    }
}

/// Whether `ip` is one of this machine's addresses, the only ones it can
/// bind. A bind that fails any other way doesn't say, so `ip` counts as one.
fn own(ip: IpAddr) -> bool {
    match UdpSocket::bind((ip, 0)) {
        Err(error) if error.kind() == ErrorKind::AddrNotAvailable => false,
        Err(error) => {
            static WARNED: Once = Once::new();
            WARNED.call_once(|| {
                tracing::warn!("Can't tell this machine's addresses from others: {error}");
            });
            true
        }
        Ok(_) => true,
    }
}

/// Whether `ip` is on the open internet, not a network only the relay's
/// machine can see: loopback, private, link-local, shared (carrier NAT) or
/// otherwise reserved, including IPv4 carried inside IPv6.
fn public(ip: IpAddr) -> bool {
    match unwrapped(ip) {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            let [first, second, ..] = ip.segments();
            // Global unicast (`2000::/3`), less what it sets aside: protocol
            // assignments (`2001::/23`) and documentation (`2001:db8::/32`,
            // `3fff::/20`).
            (first & 0xe000) == 0x2000
                && !(first == 0x2001 && (second < 0x200 || second == 0x0db8))
                && !(first == 0x3fff && second < 0x1000)
        }
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (18..20).contains(&b)))
}

/// `ip`, or the IPv4 address it carries as an IPv4-mapped, NAT64
/// (`64:ff9b::/96`) or 6to4 (`2002::/16`) address.
fn unwrapped(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    let segments = v6.segments();
    let carried = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    let nat64 = segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
    v6.to_ipv4_mapped()
        .or_else(|| nat64.then(|| carried(segments[6], segments[7])))
        .or_else(|| (segments[0] == 0x2002).then(|| carried(segments[1], segments[2])))
        .map_or(ip, IpAddr::V4)
}

/// Copies bytes both ways until either side closes, both fall silent or
/// `client` has had all it may, once the page has opened with a handshake:
/// nothing reaches the server, or comes back from it, for a client that isn't
/// speaking to a game server.
async fn bridge(socket: WebSocket, tcp: TcpStream, limits: &Limits, client: IpAddr) {
    let _ = tcp.set_nodelay(true);
    let (mut from_server, mut to_server) = tcp.into_split();
    let (mut to_page, mut from_page) = socket.split();
    let charged = |bytes: usize| limits.relayed(client, bytes + MESSAGE_COST);
    let sent = tokio::time::timeout(HANDSHAKE_WAIT, opening(&mut from_page, charged));
    let Ok(Some(sent)) = sent.await else {
        return;
    };
    let written = tokio::time::timeout(IDLE, to_server.write_all(&sent));
    if !matches!(written.await, Ok(Ok(()))) {
        return;
    }
    let activity = Activity::start();
    let carried = |bytes: usize| {
        activity.mark();
        let allowed = charged(bytes);
        if !allowed {
            tracing::warn!("Closed {client}'s connection at what it may have relayed");
        }
        allowed
    };
    let upstream = async {
        while let Some(Ok(message)) = from_page.next().await {
            match message {
                Message::Binary(bytes) if !bytes.is_empty() => {
                    if !carried(bytes.len()) || to_server.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                // Not carried on, but the relay still had to receive it.
                other => {
                    if !charged(other.into_data().len()) {
                        break;
                    }
                }
            }
        }
    };
    let downstream = async {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(read @ 1..) = from_server.read(&mut buffer).await {
            let bytes = Bytes::copy_from_slice(&buffer[..read]);
            if !carried(read) || to_page.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = to_page.close().await;
    };
    tokio::select! {
        () = upstream => {}
        () = downstream => {}
        () = activity.silent() => {}
    }
}

/// The page's first bytes, each message `charged` to it, if they open with a
/// handshake and it sent nothing else first.
async fn opening(
    from_page: &mut SplitStream<WebSocket>,
    charged: impl Fn(usize) -> bool,
) -> Option<Vec<u8>> {
    let mut sent = Vec::new();
    while let Some(Ok(message)) = from_page.next().await {
        let binary = matches!(message, Message::Binary(_));
        let bytes = message.into_data();
        if !charged(bytes.len()) || !binary {
            break;
        }
        sent.extend_from_slice(&bytes);
        if let Some(handshake) = opens_with_handshake(&sent) {
            return handshake.then_some(sent);
        }
    }
    None
}

/// Whether `sent` opens with a handshake frame, or `None` until enough of it
/// is here to tell.
fn opens_with_handshake(sent: &[u8]) -> Option<bool> {
    let mut at = 0;
    let Some(length) = read_varint(sent, &mut at) else {
        return (sent.len() >= MAX_LENGTH_PREFIX).then_some(false);
    };
    if at > MAX_LENGTH_PREFIX || length as usize > MAX_HANDSHAKE {
        return Some(false);
    }
    let frame = sent.get(at..at + length as usize)?;
    Some(Intention::decode(frame).is_ok())
}

/// When a connection last carried bytes either way.
struct Activity {
    start: Instant,
    last_millis: AtomicU64,
}

impl Activity {
    fn start() -> Self {
        Self {
            start: Instant::now(),
            last_millis: AtomicU64::new(0),
        }
    }

    fn mark(&self) {
        let millis = self.start.elapsed().as_millis() as u64;
        self.last_millis.store(millis, Ordering::Relaxed);
    }

    /// Returns once [`IDLE`] has passed since the last [`Self::mark`].
    async fn silent(&self) {
        loop {
            let last = Duration::from_millis(self.last_millis.load(Ordering::Relaxed));
            let deadline = self.start + last + IDLE;
            if Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep_until(deadline).await;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn only_open_internet_addresses_are_public() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700::1111",
            "2a00:1450::1",
            "2001:200::1",
            "2001:db9::1",
            "3ffe::1",
            "3fff:1000::1",
        ] {
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
            "64:ff9b:1::a00:1",
            "2002:a00:1::1",
            "::a00:1",
            "::808:808",
            "::ffff:0:7f00:1",
            "2001::1",
            "2001:1ff::1",
            "2001:db8::1",
            "100::1",
            "3fff::1",
            "3fff:fff::1",
            "4000::1",
            "5f00::1",
            "ff02::1",
            "fec0::1",
            "192.88.99.1",
        ] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
    }

    /// A relay that reaches anything but `networks`.
    fn blocking(networks: &[&str]) -> Config {
        Config {
            allow_private_servers: true,
            block_addresses: networks
                .iter()
                .map(|network| network.parse().unwrap())
                .collect(),
            ..Config::default()
        }
    }

    #[test]
    fn a_blocked_ipv6_network_holds_the_ipv4_carriers_in_it() {
        let config = blocking(&["2000::/3", "64:ff9b::/96"]);
        for ip in ["2606:4700::1111", "2002:808:808::1", "64:ff9b::808:808"] {
            assert!(!reachable(&config, ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "::ffff:8.8.8.8"] {
            assert!(reachable(&config, ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn some_of_each_family_are_dialed() {
        let addresses =
            |family: &'static str| (1..=5).map(move |n| format!("{family}{n}").parse().unwrap());
        let v6_first = addresses("2001:db8::").chain(addresses("203.0.113."));
        let checked = std::cell::Cell::new(0);
        let reachable = |ip: IpAddr| {
            checked.set(checked.get() + 1);
            ip.to_string() != "2001:db8::1"
        };
        let dialed: Vec<String> = dialed(v6_first, reachable)
            .map(|ip| ip.to_string())
            .collect();
        let expected = ["2001:db8::2", "2001:db8::3", "203.0.113.1", "203.0.113.2"];
        assert_eq!(dialed, expected);
        assert_eq!(checked.get(), expected.len() + 1);
    }

    #[test]
    fn blocked_addresses_stay_blocked_however_written() {
        let config = blocking(&[
            "203.0.113.7",
            "198.51.100.0/24",
            "2001:db8:5::/64",
            "64:ff9b::cb00:7109",
        ]);
        for ip in [
            "203.0.113.7",
            "::ffff:203.0.113.7",
            "64:ff9b::cb00:7107",
            "2002:cb00:7107::1",
            "203.0.113.9",
            "198.51.100.0",
            "198.51.100.255",
            "::ffff:198.51.100.9",
            "2001:db8:5::20",
            "2001:db8:5:0:ffff::1",
        ] {
            assert!(!reachable(&config, ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "192.168.0.1",
            "203.0.113.8",
            "198.51.99.255",
            "198.51.101.0",
            "2001:db8:4:ffff::1",
            "2001:db8:5:1::1",
        ] {
            assert!(reachable(&config, ip.parse().unwrap()), "{ip}");
        }
        for written in [
            "",
            "play.pomme.rs",
            "203.0.113.7/33",
            "2001:db8::/129",
            "203.0.113.7/",
        ] {
            assert!(written.parse::<Network>().is_err(), "{written}");
        }
    }

    #[test]
    fn the_machines_own_addresses_are_told_from_others() {
        assert!(own("127.0.0.1".parse().unwrap()));
        assert!(!own("203.0.113.7".parse().unwrap()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_connection_ends_and_traffic_keeps_it_open() {
        let activity = Activity::start();
        let silent = activity.silent();
        tokio::pin!(silent);
        let step = IDLE - Duration::from_secs(1);
        for _ in 0..3 {
            assert!(tokio::time::timeout(step, &mut silent).await.is_err());
            activity.mark();
        }
        let late = IDLE + Duration::from_secs(1);
        assert!(tokio::time::timeout(late, &mut silent).await.is_ok());
    }

    /// A status handshake as the page sends it, behind its length.
    pub fn handshake() -> Vec<u8> {
        let packet = Intention {
            protocol_version: 776,
            host: "play.example.org".into(),
            port: 25565,
            intent: pomme_protocol::status::ClientIntent::Status,
        }
        .encode();
        let mut sent = vec![packet.len() as u8];
        sent.extend(packet);
        sent
    }

    #[test]
    fn only_a_handshake_opens_a_connection() {
        let mut sent = handshake();
        for partial in 0..sent.len() {
            assert_eq!(opens_with_handshake(&sent[..partial]), None, "{partial}");
        }
        assert_eq!(opens_with_handshake(&sent), Some(true));
        sent.extend([1, 0]);
        assert_eq!(opens_with_handshake(&sent), Some(true), "with what follows");

        for other in [
            b"GET / HTTP/1.1\r\nHost: example.org\r\n\r\n".as_slice(),
            b"SSH-2.0-OpenSSH_9.6\r\n",
            b"*1\r\n$4\r\nPING\r\n",
            &[0xff; 5],
            &[0xff, 0x7f, 0],
            &[0x80, 0x80, 0x80, 0],
        ] {
            assert_ne!(opens_with_handshake(other), Some(true), "{other:?}");
            let mut whole = other.to_vec();
            whole.resize(MAX_HANDSHAKE, 0);
            assert_eq!(opens_with_handshake(&whole), Some(false), "{other:?}");
        }
    }
}
