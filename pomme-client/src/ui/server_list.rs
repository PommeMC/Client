use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::ui::text::{TextSpan, component_from_json, format_component_spans, legacy_format_spans};

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub address: String,
    /// The protocol from this server's last successful ping, so a join
    /// before the current ping completes still skips the wire-version probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<i32>,
    /// Vanilla `ServerData.packStatus`, stored as its `acceptTextures`.
    #[serde(
        rename = "acceptTextures",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub accept_textures: Option<bool>,
    /// Fields other tools own (the launcher's category, ...), passed through
    /// untouched so a client save doesn't strip them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Vanilla `ServerData.ServerPackStatus`, in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerPackStatus {
    Enabled,
    Disabled,
    #[default]
    Prompt,
}

impl ServerPackStatus {
    /// `ServerPackStatus.FIELD_CODEC`: `acceptTextures`, absent to prompt.
    pub fn from_accept_textures(accept: Option<bool>) -> Self {
        match accept {
            Some(true) => Self::Enabled,
            Some(false) => Self::Disabled,
            None => Self::Prompt,
        }
    }

    pub fn accept_textures(self) -> Option<bool> {
        match self {
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
            Self::Prompt => None,
        }
    }

    /// `CycleButton`'s next value.
    pub fn cycle(self) -> Self {
        match self {
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Prompt,
            Self::Prompt => Self::Enabled,
        }
    }

    pub fn lang_key(self) -> &'static str {
        match self {
            Self::Enabled => "manageServer.resourcePack.enabled",
            Self::Disabled => "manageServer.resourcePack.disabled",
            Self::Prompt => "manageServer.resourcePack.prompt",
        }
    }
}

/// The server a join targets, as vanilla's `ServerData`: its pack status,
/// and the entry the pack prompt saves its answer to.
#[derive(Clone, Debug)]
pub struct JoinedServer {
    pub name: String,
    pub address: String,
    pub pack_status: ServerPackStatus,
}

impl JoinedServer {
    pub fn from_entry(entry: &ServerEntry) -> Self {
        Self {
            name: entry.name.clone(),
            address: entry.address.clone(),
            pack_status: ServerPackStatus::from_accept_textures(entry.accept_textures),
        }
    }

    /// A direct connection's fresh `ServerData`.
    pub fn unlisted(address: String) -> Self {
        Self {
            name: default_server_name().to_owned(),
            address,
            pack_status: ServerPackStatus::Prompt,
        }
    }
}

/// `I18n.get("selectServer.defaultName")`.
pub fn default_server_name() -> &'static str {
    crate::lang::translate("selectServer.defaultName").unwrap_or("Minecraft Server")
}

/// How the client can speak to a pinged server.
#[derive(Clone, Copy, PartialEq)]
pub enum Compat {
    /// The version the client speaks natively: joined without translation.
    Native,
    /// Another protocol with embedded translation data: joinable, with the
    /// wire translated on the fly.
    Translated,
    /// A protocol without translation data: a join would be refused.
    Incompatible,
}

#[derive(Clone)]
pub enum PingState {
    Pinging,
    Success {
        motd: Vec<TextSpan>,
        /// Online and maximum players; `None` when the server reports neither.
        players: Option<(i32, i32)>,
        latency_ms: u64,
        version: String,
        /// The server's protocol number, reused at join time to skip the
        /// wire-version probe.
        protocol: i32,
        compat: Compat,
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

/// Monotonic refresh counter. A ping discards its result if this advanced while
/// it was in flight (mirrors vanilla's `pinger.removeAll()` on refresh).
pub type PingGeneration = Arc<AtomicU64>;

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
        // TODO: use crate::util::write_atomic (vanilla ServerList.save goes
        // through a temp file + Util.safeReplaceFile).
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

    /// Vanilla `ServerList.saveSingleServer`: the entry with the same name and
    /// address takes the server's pack status.
    pub fn save_pack_status(&mut self, server: &JoinedServer) {
        if let Some(entry) = self
            .servers
            .iter_mut()
            .find(|entry| entry.name == server.name && entry.address == server.address)
        {
            entry.accept_textures = server.pack_status.accept_textures();
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
    generation: &PingGeneration,
) {
    let spawned_gen = generation.load(Ordering::Acquire);
    for server in servers {
        let address = server.address.clone();
        results.write().insert(address.clone(), PingState::Pinging);
        rt.spawn(ping_server(
            address,
            Arc::clone(results),
            Arc::clone(generation),
            spawned_gen,
        ));
    }
}

async fn ping_server(
    address: String,
    results: PingResults,
    generation: PingGeneration,
    spawned_gen: u64,
) {
    let result = async {
        let server_addr = crate::net::address::ServerAddr::parse(&address)
            .ok_or_else(|| format!("Invalid address: {address}"))?;
        let (status, mut conn) = crate::net::resolve::request_status(&server_addr)
            .await
            .map_err(|e| format!("{address}: {e}"))?;

        let ping_start = Instant::now();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        conn.writer
            .write(&pomme_protocol::status::encode_ping_request(time as i64))
            .await
            .map_err(|e| format!("Ping request failed: {e}"))?;

        crate::net::resolve::read_status(&mut conn)
            .await
            .map_err(|e| format!("Pong failed: {e}"))?;
        let latency_ms = ping_start.elapsed().as_millis() as u64;

        let motd = motd_spans(&status.description);
        // Vanilla `ServerStatusPinger`: a status without a version is "Old",
        // protocol 0.
        let (version, protocol) = status
            .version
            .map_or_else(|| ("Old".to_owned(), 0), |v| (v.name, v.protocol));
        // Keyed to the native version, not the launched one.
        let compat = if protocol == pomme_protocol::version::NATIVE.protocol {
            Compat::Native
        } else if crate::net::translate::joinable(protocol) {
            tokio::task::spawn_blocking(move || crate::net::translate::prewarm(protocol));
            Compat::Translated
        } else {
            Compat::Incompatible
        };

        let favicon_rgba = status.favicon.as_deref().and_then(decode_favicon);
        let players = status.players.as_ref().map(|p| (p.online, p.max));
        let player_names: Vec<String> = status
            .players
            .into_iter()
            .flat_map(|p| p.sample)
            .map(|p| p.name)
            .collect();

        Ok(PingState::Success {
            motd,
            players,
            latency_ms,
            version,
            protocol,
            compat,
            favicon_rgba,
            player_names,
        })
    }
    .await;

    let state = match result {
        Ok(s) => s,
        Err(e) => PingState::Failed(e),
    };
    if generation.load(Ordering::Acquire) == spawned_gen {
        results.write().insert(address, state);
    }
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

/// Vanilla's MOTD base color, 0x808080.
const MOTD_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// A status description as the list draws it. Legacy codes are applied here,
/// over each component's whole text, so one carries across the lines the
/// entry later wraps it into (`StringSplitter.splitLines` keeps the style at
/// a break).
fn motd_spans(description: &serde_json::Value) -> Vec<TextSpan> {
    let spans = format_component_spans(&component_from_json(description), MOTD_COLOR);
    legacy_format_spans(&spans, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `acceptTextures` is absent to prompt, as vanilla's codec writes it.
    #[test]
    fn pack_status_round_trips_through_accept_textures() {
        for status in [
            ServerPackStatus::Enabled,
            ServerPackStatus::Disabled,
            ServerPackStatus::Prompt,
        ] {
            let entry = ServerEntry {
                name: "Server".into(),
                address: "example.com".into(),
                protocol: None,
                accept_textures: status.accept_textures(),
                extra: Default::default(),
            };
            let json = serde_json::to_value(&entry).unwrap();
            assert_eq!(
                json.get("acceptTextures").is_some(),
                status != ServerPackStatus::Prompt
            );
            let read: ServerEntry = serde_json::from_value(json).unwrap();
            assert_eq!(JoinedServer::from_entry(&read).pack_status, status);
        }
    }

    /// `ServerList.set` matches the name as well as the address.
    #[test]
    fn a_pack_answer_saves_to_the_entry_with_its_name_and_address() {
        let dir = std::env::temp_dir().join(format!("pomme-servers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut list = ServerList::load(&dir);
        for name in ["Other", "Server"] {
            list.add(ServerEntry {
                name: name.into(),
                address: "example.com".into(),
                protocol: None,
                accept_textures: None,
                extra: Default::default(),
            });
        }
        let mut joined = JoinedServer::from_entry(&list.servers[1]);
        joined.pack_status = ServerPackStatus::Enabled;
        list.save_pack_status(&joined);
        let saved = ServerList::load(&dir);
        assert_eq!(saved.servers[0].accept_textures, None);
        assert_eq!(saved.servers[1].accept_textures, Some(true));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_motds_legacy_code_carries_past_its_line_break() {
        const GREEN: [f32; 4] = [85.0 / 255.0, 1.0, 85.0 / 255.0, 1.0];
        let spans = motd_spans(&serde_json::json!("\u{a7}aWelcome\nto the server"));
        let [span] = spans.as_slice() else {
            panic!("one span, got {spans:?}");
        };
        assert_eq!(span.text, "Welcome\nto the server");
        assert_eq!(span.color, GREEN);

        // Each component's text resets to its own style, as in vanilla.
        let spans = motd_spans(&serde_json::json!([
            {"text": "\u{a7}aone "},
            {"text": "two", "color": "red"},
            " three"
        ]));
        let colors: Vec<_> = spans.iter().map(|s| (s.text.as_str(), s.color)).collect();
        assert_eq!(colors[0], ("one ", GREEN));
        assert_eq!(colors[1].0, "two");
        assert_eq!(colors[2], (" three", MOTD_COLOR));
    }

    /// The launcher writes fields the client doesn't model (category); a
    /// client save must pass them through, not strip them.
    #[test]
    fn entry_round_trip_keeps_unknown_fields() {
        let json = r#"{"name":"a","address":"b:25565","protocol":775,"category":"Modded"}"#;
        let entry: ServerEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.protocol, Some(775));
        assert_eq!(
            entry.extra.get("category").and_then(|v| v.as_str()),
            Some("Modded")
        );
        let back: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_value(&entry).unwrap(), back);
    }

    /// Entries without the optional fields load and save without gaining any.
    #[test]
    fn entry_round_trip_minimal() {
        let json = r#"{"name":"a","address":"b"}"#;
        let entry: ServerEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.protocol, None);
        assert_eq!(serde_json::to_string(&entry).unwrap(), json);
    }
}
