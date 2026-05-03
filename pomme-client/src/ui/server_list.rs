use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
}

#[derive(Clone)]
pub struct MotdSpan {
    pub text: String,
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

#[derive(Clone)]
pub enum PingState {
    Pinging,
    Success {
        motd: Vec<MotdSpan>,
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
    use azalea_protocol::connect::Connection;
    use azalea_protocol::packets::handshake::s_intention::ServerboundIntention;
    use azalea_protocol::packets::status::ClientboundStatusPacket;
    use azalea_protocol::packets::status::s_ping_request::ServerboundPingRequest;
    use azalea_protocol::packets::status::s_status_request::ServerboundStatusRequest;
    use azalea_protocol::packets::{ClientIntention, PROTOCOL_VERSION};

    let result = async {
        use azalea_protocol::address::ServerAddr;

        let server_addr: ServerAddr = address
            .as_str()
            .try_into()
            .map_err(|_| format!("Invalid address: {address}"))?;
        let addr = azalea_protocol::resolve::resolve_address(&server_addr)
            .await
            .map_err(|e| format!("{address}: {e}"))?;
        let mut conn: Connection<_, _> = Connection::new(&addr)
            .await
            .map_err(|e| format!("Connection failed: {e}"))?;

        conn.write(ServerboundIntention {
            protocol_version: PROTOCOL_VERSION,
            hostname: server_addr.host.clone(),
            port: server_addr.port,
            intention: ClientIntention::Status,
        })
        .await
        .map_err(|e| format!("Handshake failed: {e}"))?;

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

        let motd = format_motd_spans(&status.description);
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

fn format_motd_spans(text: &azalea_chat::FormattedText) -> Vec<MotdSpan> {
    use azalea_chat::style::Style;
    use std::cell::RefCell;

    let white: azalea_chat::style::TextColor = azalea_chat::style::ChatFormatting::White
        .try_into()
        .unwrap();
    let white_style = Style::default().color(white);

    let spans: RefCell<Vec<MotdSpan>> = RefCell::new(Vec::new());
    let current_style: RefCell<Option<Style>> = RefCell::new(None);

    text.to_custom_format(
        |_running, new| {
            *current_style.borrow_mut() = Some(new.clone());
            (String::new(), String::new())
        },
        |t| {
            if !t.is_empty() {
                let style = current_style.borrow();
                let s = style.as_ref();
                let color = s.map(style_to_rgba).unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let bold = s.and_then(|s| s.bold).unwrap_or(false);
                let italic = s.and_then(|s| s.italic).unwrap_or(false);
                let strikethrough = s.and_then(|s| s.strikethrough).unwrap_or(false);
                let underline = s.and_then(|s| s.underlined).unwrap_or(false);

                spans.borrow_mut().push(MotdSpan {
                    text: t.to_string(),
                    color,
                    bold,
                    italic,
                    strikethrough,
                    underline,
                });
            }
            String::new()
        },
        |_| String::new(),
        &white_style,
    );

    let result = spans.into_inner();
    if result.is_empty() {
        let plain = format!("{text}");
        if !plain.is_empty() {
            return vec![MotdSpan {
                text: plain,
                color: [0.63, 0.63, 0.63, 1.0],
                bold: false,
                italic: false,
                strikethrough: false,
                underline: false,
            }];
        }
    }

    result
}

fn style_to_rgba(style: &azalea_chat::style::Style) -> [f32; 4] {
    if let Some(color) = &style.color {
        let v = color.value;
        [
            ((v >> 16) & 0xFF) as f32 / 255.0,
            ((v >> 8) & 0xFF) as f32 / 255.0,
            (v & 0xFF) as f32 / 255.0,
            1.0,
        ]
    } else {
        [1.0, 1.0, 1.0, 1.0]
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
