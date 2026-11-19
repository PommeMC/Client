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
//! unprivileged ports (never the machine's own, given as `--block-address`),
//! and rate limits each client. The proxy must pass the request's `Host` on
//! unchanged and add the client's address to `X-Forwarded-For`.

mod connect;
mod guard;
mod limits;
mod relay;

use std::net::{IpAddr, SocketAddr};
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

#[derive(clap::Args)]
#[cfg_attr(test, derive(Default))]
struct Config {
    /// The hostname the page is served at, such as play.pomme.rs. Requests
    /// naming any other host are refused, so no site can point its own name
    /// at the relay and use it. Without one, the relay answers to localhost.
    #[arg(long)]
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
    /// An address game connections may never reach, such as the relay
    /// machine's own public one, whose services would otherwise be dialed
    /// from inside. Repeatable.
    #[arg(long = "block-address")]
    block_addresses: Vec<IpAddr>,
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
    serve(listener, app(args.config, &args.dir)).await;
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
fn app(config: Config, dir: &Path) -> Router {
    let state = AppState {
        http: relay::client(),
        config: Arc::new(config),
        connections: Arc::default(),
        limits: Arc::default(),
    };
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
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode, header};
    use axum::response::Response;
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
    use tower::ServiceExt as _;

    use super::*;

    /// A relay on a free local port, which may reach this machine's own.
    async fn relay() -> SocketAddr {
        let config = Config {
            allow_private_servers: true,
            connections_per_client: 8,
            max_connections: 8,
            ..Config::default()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, app(config, Path::new("no-such-dir"))));
        address
    }

    /// The page's WebSocket to `server` through `relay`.
    async fn page(
        relay: SocketAddr,
        server: SocketAddr,
    ) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        let url = format!("ws://{relay}/connect/127.0.0.1/{}", server.port());
        let mut request = url.into_client_request().unwrap();
        let origin = format!("http://{relay}").parse().unwrap();
        request.headers_mut().insert(header::ORIGIN, origin);
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }

    #[tokio::test]
    async fn a_handshake_is_bridged_and_anything_else_reaches_nothing() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let relay = relay().await;

        let mut other = page(relay, address).await;
        let (mut tcp, _) = server.accept().await.unwrap();
        let request = b"GET / HTTP/1.1\r\nHost: example.org\r\n\r\n".repeat(32);
        other.send(Message::binary(request)).await.unwrap();
        let mut received = Vec::new();
        tcp.read_to_end(&mut received).await.unwrap();
        assert!(received.is_empty(), "{received:?}");

        let mut page = page(relay, address).await;
        let (mut tcp, _) = server.accept().await.unwrap();
        let mut opening = connect::tests::handshake();
        opening.extend([1, 0]);
        page.send(Message::binary(opening.clone())).await.unwrap();
        let mut received = vec![0; opening.len()];
        tcp.read_exact(&mut received).await.unwrap();
        assert_eq!(received, opening);
        tcp.write_all(b"status").await.unwrap();
        let reply = page.next().await.unwrap().unwrap();
        assert_eq!(reply.into_data().as_ref(), b"status");
    }

    async fn get(uri: &str, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::get(uri);
        if !headers.iter().any(|(name, _)| *name == "host") {
            request = request.header(header::HOST, "localhost:8080");
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        app(Config::default(), Path::new("no-such-dir"))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))))
            .oneshot(request.body(Body::empty()).unwrap())
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
