//! The browser build's server. It serves the page, and relays the page's
//! requests to the Mojang and Microsoft services it needs: those don't allow
//! cross-origin requests, so a browser can't call them itself. The page's
//! service worker (`pomme-web/sw.js`) sends every cross-origin request here
//! as `/relay/<host>/<path>`, and only the hosts in [`relay::ALLOWED_HOSTS`]
//! are fetched. A page can't open sockets either, so it reaches game servers
//! through a WebSocket to `/connect/<host>/<port>`, which this bridges to the
//! server's TCP port.
//!
//! Public hosting puts it behind a TLS reverse proxy, run as
//! `--public-host play.pomme.rs --trust-proxy`. It then answers only for that
//! hostname, refuses cross-site requests, dials only public addresses at
//! unprivileged ports (never the machine's own, given as `--block-address`),
//! and rate limits each client.

mod connect;
mod guard;
mod limits;
mod relay;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::{any, get};
use clap::Parser;
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
    /// naming any other host but localhost are refused, so no site can point
    /// its own name at the relay and use it.
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
    let state = AppState {
        http: relay::client(),
        config: Arc::new(args.config),
        connections: Arc::default(),
        limits: Arc::default(),
    };
    let app = Router::new()
        .route("/relay/{host}/{*path}", any(relay::relay))
        .route("/connect/{host}/{port}", get(connect::connect))
        .fallback_service(ServeDir::new(&args.dir))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::guard,
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .unwrap_or_else(|e| panic!("failed to listen on {}: {e}", args.listen));
    tracing::info!("Serving {} on http://{}", args.dir.display(), args.listen);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("the server stops");
}
