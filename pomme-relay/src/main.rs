//! The browser build's server. It serves the page, and relays the page's
//! requests to the Mojang and Microsoft services it needs: those don't allow
//! cross-origin requests, so a browser can't call them itself. The page's
//! service worker (`pomme-web/sw.js`) sends every cross-origin request here
//! as `/relay/<host>/<path>`, and only the hosts in [`relay::ALLOWED_HOSTS`]
//! are fetched. A page can't open sockets either, so it reaches game servers
//! through a WebSocket to `/connect/<host>/<port>`, which this bridges to the
//! server's TCP port once the page opens with a handshake.
//!
//! Public hosting puts it behind a TLS reverse proxy, run as
//! `--public-host play.pomme.rs --trust-proxy`. It then answers only for that
//! hostname, refuses cross-site requests, dials only public addresses at
//! unprivileged ports, and rate limits each client. It never dials its own
//! machine's addresses, but can't know one that a NAT or a container's
//! network maps to it, nor which public addresses are its neighbours behind
//! the same firewall: those are given as `--block-address`. The proxy must
//! pass the request's `Host` on unchanged and add the client's address to
//! `X-Forwarded-For`.

mod connect;
mod guard;
mod limits;
mod relay;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::routing::{any, get};
use clap::Parser;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

#[derive(Parser)]
struct Args {
    /// The built page (`just web-build` writes it to pomme-web/dist).
    #[arg(long, default_value = "pomme-web/dist")]
    dir: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    #[command(flatten)]
    config: Config,
}

/// What every route shares.
#[derive(Clone)]
struct AppState {
    http: reqwest::Client,
    config: Arc<Config>,
    connections: Arc<guard::Connections>,
    limits: Arc<limits::Limits>,
}

impl AppState {
    fn new(config: Config) -> Self {
        Self {
            http: relay::client(),
            config: Arc::new(config),
            connections: Arc::default(),
            limits: Arc::default(),
        }
    }
}

#[derive(clap::Args)]
#[cfg_attr(test, derive(Default))]
struct Config {
    /// The hostname the page is served at, such as play.pomme.rs. Requests
    /// naming any other host are refused, so no site can point its own name
    /// at the relay and use it. Without one, the relay answers to localhost.
    #[arg(long, value_parser = hostname)]
    public_host: Option<String>,
    /// Lets the page reach servers on private networks, for a relay run at
    /// home. A public relay must not, or anyone could reach what's behind it.
    #[arg(long)]
    allow_private_servers: bool,
    /// Takes each client's address from `X-Forwarded-For`, which only a
    /// reverse proxy in front of the relay may set.
    #[arg(long)]
    trust_proxy: bool,
    /// Game connections one client address may have open at once.
    #[arg(long, default_value_t = 8)]
    connections_per_client: usize,
    /// Game connections open at once across every client.
    #[arg(long, default_value_t = 512)]
    max_connections: usize,
    /// An address, or a network such as 203.0.113.0/24, game connections
    /// may never reach. The relay refuses its machine's own by itself; this
    /// is for a public one a NAT in front maps to it, and for neighbours on
    /// its network, whose services would otherwise be dialed from inside.
    /// Repeatable.
    #[arg(long = "block-address")]
    block_addresses: Vec<connect::Network>,
}

/// A hostname as a `Host` header names it, less the port.
fn hostname(value: &str) -> Result<String, &'static str> {
    let named = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    if value.is_empty() || value.ends_with('.') || !value.chars().all(named) {
        return Err("expected a bare hostname, such as play.pomme.rs");
    }
    Ok(value.to_owned())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pomme_relay=info".into()),
        )
        .init();
    let args = Args::parse();
    let listener = TcpListener::bind(args.listen)
        .await
        .unwrap_or_else(|e| panic!("failed to listen on {}: {e}", args.listen));
    tracing::info!("Serving {} on http://{}", args.dir.display(), args.listen);
    if args.config.public_host.is_none() && !args.listen.ip().is_loopback() {
        tracing::warn!(
            "Listening beyond this machine without --public-host: anything that names localhost is answered"
        );
    }
    serve(listener, app(AppState::new(args.config), &args.dir)).await;
}

async fn serve(listener: TcpListener, app: Router) {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("the server stops");
}

/// The routes, with the page in `dir` served for everything else.
fn app(state: AppState, dir: &Path) -> Router {
    Router::new()
        .nest("/relay", Router::new().fallback(any(relay::relay)))
        .route("/connect/{host}/{port}", get(connect::connect))
        .fallback_service(ServeDir::new(dir))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::guard,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode, header, request};
    use axum::response::Response;
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::{self, Message};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
    use tower::ServiceExt as _;

    use super::*;

    fn state(allow_private_servers: bool) -> AppState {
        AppState::new(Config {
            allow_private_servers,
            connections_per_client: 8,
            max_connections: 8,
            ..Config::default()
        })
    }

    /// A relay on a free local port.
    async fn relay(state: AppState) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, app(state, Path::new("no-such-dir"))));
        address
    }

    /// Fails a step that takes longer than a refusal or a relayed byte
    /// should, well inside the wait for a handshake.
    async fn soon<T>(step: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), step)
            .await
            .expect("the step finishes")
    }

    type Page = WebSocketStream<MaybeTlsStream<TcpStream>>;

    /// Opens the page's WebSocket to `server`, a `<host>/<port>`, through
    /// `relay`, as its own page or as one that sends no `Origin`.
    async fn dial(relay: SocketAddr, server: &str, own_page: bool) -> Result<Page, StatusCode> {
        let mut request = format!("ws://{relay}/connect/{server}")
            .into_client_request()
            .unwrap();
        if own_page {
            let origin = format!("http://{relay}").parse().unwrap();
            request.headers_mut().insert(header::ORIGIN, origin);
        }
        match soon(tokio_tungstenite::connect_async(request)).await {
            Ok((page, _)) => Ok(page),
            Err(tungstenite::Error::Http(response)) => Err(response.status()),
            Err(error) => panic!("{error}"),
        }
    }

    /// A local server, and the `<host>/<port>` that names it.
    async fn server() -> (TcpListener, String) {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("127.0.0.1/{}", server.local_addr().unwrap().port());
        (server, address)
    }

    /// The page's WebSocket through a relay to a server, and the server's end
    /// of the connection, on which it has already sent `BANNER`.
    async fn connected(state: AppState) -> (Page, TcpStream) {
        let (server, address) = server().await;
        let page = dial(relay(state).await, &address, true).await.unwrap();
        let (mut tcp, _) = soon(server.accept()).await.unwrap();
        tcp.write_all(BANNER).await.unwrap();
        (page, tcp)
    }

    const BANNER: &[u8] = b"banner";

    /// What the server is sent before the relay closes its connection. With
    /// the banner unread, that close is a reset on some systems.
    async fn sent_to(mut tcp: TcpStream) -> Vec<u8> {
        let mut received = Vec::new();
        match soon(tcp.read_to_end(&mut received)).await {
            Err(error) if error.kind() != std::io::ErrorKind::ConnectionReset => panic!("{error}"),
            _ => received,
        }
    }

    #[tokio::test]
    async fn connections_the_page_may_not_make_are_refused() {
        let (_listener, server) = server().await;
        for (allow_private_servers, server, own_page) in [
            (true, server.as_str(), false),
            (true, "127.0.0.1/80", true),
            (false, server.as_str(), true),
        ] {
            let relay = relay(state(allow_private_servers)).await;
            let refused = dial(relay, server, own_page).await;
            assert_eq!(refused.err(), Some(StatusCode::FORBIDDEN), "{server}");
        }
    }

    #[tokio::test]
    async fn anything_but_a_handshake_reaches_nothing() {
        let (mut page, tcp) = connected(state(true)).await;
        let request = b"GET / HTTP/1.1\r\nHost: example.org\r\n\r\n".repeat(32);
        page.send(Message::binary(request)).await.unwrap();

        let received = sent_to(tcp).await;
        assert!(received.is_empty(), "{received:?}");
        while let Some(Ok(message)) = soon(page.next()).await {
            assert!(message.is_close(), "{message:?}");
        }
    }

    #[tokio::test]
    async fn a_connection_ends_once_its_bytes_are_spent() {
        const PIECE: usize = 200 * 1024;
        const PIECES: usize = 32;
        let state = state(true);
        let client = IpAddr::from([127, 0, 0, 1]);
        state.limits.leave(client, PIECE * PIECES / 4);
        let (mut page, tcp) = connected(state).await;
        let received = tokio::spawn(sent_to(tcp));

        page.send(Message::binary(connect::tests::handshake()))
            .await
            .unwrap();
        for _ in 0..PIECES {
            if page.send(Message::binary(vec![0; PIECE])).await.is_err() {
                break;
            }
        }
        let received = received.await.unwrap().len();
        assert!((PIECE..PIECE * PIECES).contains(&received), "{received}");
    }

    #[tokio::test]
    async fn a_handshake_is_bridged() {
        let (mut page, mut tcp) = connected(state(true)).await;
        let mut opening = connect::tests::handshake();
        opening.extend([1, 0]);
        for piece in opening.chunks(7) {
            page.send(Message::binary(piece.to_vec())).await.unwrap();
        }

        let mut received = vec![0; opening.len()];
        soon(tcp.read_exact(&mut received)).await.unwrap();
        assert_eq!(received, opening);
        let reply = soon(page.next()).await.unwrap().unwrap();
        assert_eq!(reply.into_data().as_ref(), BANNER);
    }

    #[test]
    fn a_public_host_is_a_bare_hostname() {
        assert_eq!(hostname("play.pomme.rs"), Ok("play.pomme.rs".to_owned()));
        for value in [
            "",
            "play.pomme.rs.",
            "play.pomme.rs:443",
            "https://play.pomme.rs",
        ] {
            assert!(hostname(value).is_err(), "{value}");
        }
    }

    async fn get(uri: &str, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::get(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        send(request, Body::empty()).await
    }

    /// The answer to `request`, sent to localhost unless it names a host.
    async fn send(mut request: request::Builder, body: Body) -> Response {
        if !request.headers_ref().unwrap().contains_key(header::HOST) {
            request = request.header(header::HOST, "localhost:8080");
        }
        app(AppState::new(Config::default()), Path::new("no-such-dir"))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))))
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn only_listed_hosts_are_relayed() {
        for uri in ["/relay/example.com/x", "/relay/example.com/"] {
            assert_eq!(get(uri, &[]).await.status(), StatusCode::FORBIDDEN, "{uri}");
        }
    }

    #[tokio::test]
    async fn uploads_go_only_to_the_services_and_only_so_large() {
        let files = Request::post("/relay/piston-data.mojang.com/x");
        let response = send(files, Body::empty()).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let service = Request::post("/relay/api.mojang.com/x");
        let response = send(service, Body::from(vec![0; relay::MAX_BODY + 1])).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn another_sites_page_is_refused() {
        let cross_site = [("sec-fetch-site", "cross-site")];
        let response = get("/relay/api.mojang.com/x", &cross_site).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn another_hostname_is_refused() {
        let response = get("/", &[("host", "evil.example")]).await;
        assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        let response = get("/missing", &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        for name in ["content-security-policy", "x-content-type-options"] {
            assert!(response.headers().contains_key(name), "{name}");
        }
    }
}
