//! What keeps a public relay from being used by anyone but its own page: the
//! hostnames it answers for, the headers every response carries, and the cap
//! on each client's game connections.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{AppState, Config, relay};

/// Refuses requests for hostnames the relay doesn't serve, then marks every
/// response with the [`SECURITY_HEADERS`] its route didn't set itself.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let host = text(request.headers(), header::HOST);
    if !host.is_some_and(|host| serves(&state.config, host)) {
        tracing::warn!("Refused a request for host {host:?}");
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS.iter() {
        headers.entry(name).or_insert_with(|| value.clone());
    }
    response
}

/// Whether `host`, a `Host` header, names this relay: the public hostname it
/// was given, or localhost when it has none. A public relay doesn't answer to
/// localhost, which anything but a browser can claim to be asking for.
fn serves(config: &Config, host: &str) -> bool {
    let name = host
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
        .map_or(host, |(name, _)| name);
    match &config.public_host {
        Some(public) => name.eq_ignore_ascii_case(public),
        None => matches!(name, "localhost" | "127.0.0.1" | "[::1]"),
    }
}

/// Whether a request came from a page of this relay's own origin, going by
/// its `Origin`, which browsers set on every cross-origin and WebSocket
/// request and pages can't forge. A public relay's page is served over TLS.
pub fn same_origin(config: &Config, headers: &HeaderMap) -> bool {
    text(headers, header::ORIGIN)
        .and_then(|origin| origin.split_once("://"))
        .zip(text(headers, header::HOST))
        .is_some_and(|((scheme, origin_host), host)| {
            origin_host.eq_ignore_ascii_case(host)
                && (config.public_host.is_none() || scheme == "https")
        })
}

/// Whether a browser sent the request from a page of another site. Requests
/// that name none come from outside a browser, which no page can make.
pub fn cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin")
}

/// The page may run only its own scripts and wasm, and fetch only the relay
/// and the hosts it relays (which the service worker sends here).
static SECURITY_HEADERS: LazyLock<Vec<(header::HeaderName, HeaderValue)>> = LazyLock::new(|| {
    let relayed: String = relay::ALLOWED_HOSTS
        .iter()
        .map(|(host, _)| format!(" https://{host}"))
        .collect();
    let policy = format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
         connect-src 'self'{relayed}; worker-src 'self'; object-src 'none'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    [
        (header::CONTENT_SECURITY_POLICY, policy.as_str()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::STRICT_TRANSPORT_SECURITY, "max-age=31536000"),
        (
            header::HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name, HeaderValue::from_str(value).expect("a valid header")))
    .collect()
});

/// The client a request came from: the peer, or behind a trusted reverse
/// proxy the last address in `X-Forwarded-For`, the one the proxy added
/// (earlier ones, and earlier header lines, are the client's own claim). An
/// IPv6 client is its /56, the block a provider gives one subscriber, so it
/// can't take a fresh address for every request.
pub fn client(config: &Config, peer: ConnectInfo<SocketAddr>, headers: &HeaderMap) -> IpAddr {
    let forwarded = || {
        let lines = headers.get_all("x-forwarded-for");
        let address = lines
            .iter()
            .next_back()?
            .to_str()
            .ok()?
            .rsplit(',')
            .next()?
            .trim();
        // Some proxies add the client's port, or bracket an IPv6 address.
        let with_port = address.parse().ok().map(|socket: SocketAddr| socket.ip());
        let bare = address.trim_start_matches('[').trim_end_matches(']');
        with_port.or_else(|| bare.parse().ok())
    };
    let address = if config.trust_proxy {
        forwarded().unwrap_or_else(|| {
            tracing::warn!("The proxy sent no client address; its own stands in");
            peer.0.ip()
        })
    } else {
        peer.0.ip()
    };
    network(address)
}

fn network(ip: IpAddr) -> IpAddr {
    const SUBSCRIBER_BITS: u32 = 56;
    match ip.to_canonical() {
        IpAddr::V6(ip) => {
            let mask = u128::MAX << (u128::BITS - SUBSCRIBER_BITS);
            IpAddr::V6(Ipv6Addr::from(u128::from(ip) & mask))
        }
        ip => ip,
    }
}

fn text(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The game connections open, per client and in all.
#[derive(Default)]
pub struct Connections(Mutex<Counts>);

type Counts = (HashMap<IpAddr, usize>, usize);

impl Connections {
    /// A slot for one more of `client`'s connections, if it and the relay are
    /// under their caps. It's given back when dropped.
    pub fn open(self: &Arc<Self>, config: &Config, client: IpAddr) -> Option<Slot> {
        let mut counts = self.counts();
        let (by_client, total) = &mut *counts;
        let open = by_client.get(&client).copied().unwrap_or(0);
        if open >= config.connections_per_client || *total >= config.max_connections {
            return None;
        }
        by_client.insert(client, open + 1);
        *total += 1;
        Some(Slot {
            connections: Arc::clone(self),
            client,
        })
    }

    fn counts(&self) -> MutexGuard<'_, Counts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct Slot {
    connections: Arc<Connections>,
    client: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self.connections.counts();
        let (by_client, total) = &mut *counts;
        *total -= 1;
        if let Some(open) = by_client.get_mut(&self.client) {
            *open -= 1;
            if *open == 0 {
                by_client.remove(&self.client);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(public_host: Option<&str>) -> Config {
        Config {
            public_host: public_host.map(str::to_owned),
            connections_per_client: 2,
            max_connections: 3,
            ..Config::default()
        }
    }

    #[test]
    fn serves_its_public_host_or_else_localhost() {
        let local = ["localhost:8090", "127.0.0.1:8090", "[::1]:8090"];
        let public = ["play.pomme.rs", "Play.Pomme.rs:443"];
        let other = [
            "evil.example",
            "evil.example:8090",
            "play.pomme.rs.evil.example",
        ];
        let serves = |public_host, host| serves(&config(public_host), host);
        for host in local {
            assert!(serves(None, host), "{host}");
            assert!(!serves(Some("play.pomme.rs"), host), "{host}");
        }
        for host in public {
            assert!(serves(Some("play.pomme.rs"), host), "{host}");
            assert!(!serves(None, host), "{host}");
        }
        for host in other {
            assert!(!serves(None, host), "{host}");
            assert!(!serves(Some("play.pomme.rs"), host), "{host}");
        }
    }

    #[test]
    fn a_trusted_proxys_own_entry_names_the_client() {
        let peer = || ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 9)));
        let client = |trust_proxy, lines: &[&'static str]| {
            let mut headers = HeaderMap::new();
            for line in lines {
                headers.append("x-forwarded-for", HeaderValue::from_static(line));
            }
            let config = Config {
                trust_proxy,
                ..Config::default()
            };
            client(&config, peer(), &headers).to_string()
        };
        assert_eq!(client(true, &["203.0.113.7"]), "203.0.113.7");
        assert_eq!(client(true, &["1.2.3.4, 203.0.113.7"]), "203.0.113.7");
        assert_eq!(client(true, &["1.2.3.4", "203.0.113.7"]), "203.0.113.7");
        assert_eq!(client(true, &["203.0.113.7:5678"]), "203.0.113.7");
        assert_eq!(client(true, &["[2001:db8::1]:5678"]), "2001:db8::");
        assert_eq!(client(true, &["[2001:db8::1]"]), "2001:db8::");
        assert_eq!(client(true, &["nonsense"]), "10.0.0.1");
        assert_eq!(client(true, &[]), "10.0.0.1");
        assert_eq!(client(false, &["203.0.113.7"]), "10.0.0.1");
    }

    #[test]
    fn same_origin_needs_an_origin_naming_the_host() {
        let headers = |origin: Option<&str>| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_static("play.pomme.rs"));
            if let Some(origin) = origin {
                headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            }
            headers
        };
        let same_origin = |public_host, origin| same_origin(&config(public_host), &headers(origin));
        for public_host in [None, Some("play.pomme.rs")] {
            assert!(same_origin(public_host, Some("https://play.pomme.rs")));
            assert!(same_origin(public_host, Some("https://Play.Pomme.rs")));
            assert!(!same_origin(public_host, Some("https://evil.example")));
            assert!(!same_origin(public_host, None));
        }
        assert!(same_origin(None, Some("http://play.pomme.rs")));
        assert!(!same_origin(
            Some("play.pomme.rs"),
            Some("http://play.pomme.rs")
        ));
    }

    #[test]
    fn an_ipv6_client_is_its_whole_block() {
        let network = |ip: &str| network(ip.parse().unwrap()).to_string();
        assert_eq!(network("2001:db8:1:1234:5:6:7:8"), "2001:db8:1:1200::");
        assert_eq!(network("2001:db8:1:12ff::9"), "2001:db8:1:1200::");
        assert_eq!(network("2001:db8:1:1300::9"), "2001:db8:1:1300::");
        assert_eq!(network("::ffff:203.0.113.7"), "203.0.113.7");
        assert_eq!(network("203.0.113.7"), "203.0.113.7");
    }

    #[test]
    fn connections_stop_at_each_cap_and_come_back() {
        let config = config(None);
        let connections = Arc::new(Connections::default());
        let [a, b]: [IpAddr; 2] = ["1.1.1.1".parse().unwrap(), "2.2.2.2".parse().unwrap()];
        let a1 = connections.open(&config, a).unwrap();
        let _a2 = connections.open(&config, a).unwrap();
        assert!(connections.open(&config, a).is_none(), "per client");
        let _b1 = connections.open(&config, b).unwrap();
        assert!(connections.open(&config, b).is_none(), "in all");
        let c = "3.3.3.3".parse().unwrap();
        assert!(connections.open(&config, c).is_none(), "in all");
        assert!(
            !connections.counts().0.contains_key(&c),
            "refused, so untracked"
        );
        drop(a1);
        assert!(connections.open(&config, b).is_some());
    }
}
