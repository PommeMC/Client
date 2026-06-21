use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::ui::text::{TextSpan, format_text_spans};

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
}

#[derive(Clone)]
pub enum PingState {
    Pinging,
    Success {
        motd: Vec<TextSpan>,
        online: i32,
        max: i32,
        latency_ms: u64,
        version: String,
        protocol_match: bool,
        favicon_rgba: Option<Vec<u8>>,
        player_names: Vec<String>,
    },
    Failed(String),
}

pub struct ServerList {
    pub servers: Vec<ServerEntry>,
    path: PathBuf,
}

pub type PingResults = Arc<RwLock<HashMap<String, PingState>>>;

impl ServerList {
    pub fn load(game_dir: &Path) -> Self {
        let path = game_dir.join("servers.json");
        let servers = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { servers, path }
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.servers)
            && let Err(e) = std::fs::write(&self.path, json)
        {
            tracing::warn!("Failed to save server list: {e}");
        }
    }

    pub fn add(&mut self, entry: ServerEntry) {
        self.servers.push(entry);
        self.save();
    }

    pub fn update(&mut self, index: usize, entry: ServerEntry) {
        if index < self.servers.len() {
            self.servers[index] = entry;
            self.save();
        }
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        if a < self.servers.len() && b < self.servers.len() {
            self.servers.swap(a, b);
            self.save();
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.servers.len() {
            self.servers.remove(index);
            self.save();
        }
    }
}

pub fn ping_all_servers(
    rt: &tokio::runtime::Runtime,
    servers: &[ServerEntry],
    results: &PingResults,
) {
    for server in servers {
        let address = server.address.clone();
        results.write().insert(address.clone(), PingState::Pinging);
        let results = Arc::clone(results);
        rt.spawn(ping_server(address, results));
    }
}

async fn ping_server(address: String, results: PingResults) {
    use azalea_protocol::packets::ClientIntention;
    use azalea_protocol::packets::status::ClientboundStatusPacket;
    use azalea_protocol::packets::status::s_ping_request::ServerboundPingRequest;
    use azalea_protocol::packets::status::s_status_request::ServerboundStatusRequest;

    let result = async {
        use azalea_protocol::address::ServerAddr;

        let server_addr: ServerAddr = address
            .as_str()
            .try_into()
            .map_err(|_| format!("Invalid address: {address}"))?;
        let conn = crate::net::resolve::connect(&server_addr, ClientIntention::Status)
            .await
            .map_err(|e| format!("{address}: {e}"))?;

        let mut conn = conn.status();

        conn.write(ServerboundStatusRequest {})
            .await
            .map_err(|e| format!("Status request failed: {e}"))?;

        let packet = conn.read().await.map_err(|e| format!("Read failed: {e}"))?;
        let status = match packet {
            ClientboundStatusPacket::StatusResponse(s) => s,
            _ => return Err("Unexpected packet".to_string()),
        };

        let ping_start = Instant::now();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        conn.write(ServerboundPingRequest { time })
            .await
            .map_err(|e| format!("Ping request failed: {e}"))?;

        let _ = conn.read().await.map_err(|e| format!("Pong failed: {e}"))?;
        let latency_ms = ping_start.elapsed().as_millis() as u64;

        let motd = format_text_spans(&status.description);
        let version = status.version.name.clone();
        let protocol_match = status.version.protocol == azalea_protocol::packets::PROTOCOL_VERSION;
        let (online, max) = (status.players.online, status.players.max);

        let favicon_rgba = status.favicon.as_deref().and_then(decode_favicon);
        let player_names: Vec<String> = status
            .players
            .sample
            .iter()
            .map(|p| p.name.clone())
            .collect();

        Ok(PingState::Success {
            motd,
            online,
            max,
            latency_ms,
            version,
            protocol_match,
            favicon_rgba,
            player_names,
        })
    }
    .await;

    let state = match result {
        Ok(s) => s,
        Err(e) => PingState::Failed(e),
    };
    results.write().insert(address, state);
}

fn decode_favicon(data: &str) -> Option<Vec<u8>> {
    let b64 = data.strip_prefix("data:image/png;base64,").unwrap_or(data);
    let png_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?;
    let img = image::load_from_memory(&png_bytes).ok()?.to_rgba8();
    Some(img.into_raw())
}

fn with_default_port(address: &str) -> String {
    if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:25565")
    }
}

pub fn is_valid_address(address: &str) -> bool {
    if address.is_empty() {
        return false;
    }
    let with_port = with_default_port(address);
    with_port.parse::<std::net::SocketAddr>().is_ok()
        || with_port
            .split(':')
            .next()
            .is_some_and(|host| !host.is_empty())
}
