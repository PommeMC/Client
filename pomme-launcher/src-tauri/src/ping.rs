use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL_VERSION: i32 = 769;

#[derive(serde::Serialize, serde::Deserialize, Clone, specta::Type)]
pub struct SavedServer {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub category: String,
}

#[derive(serde::Serialize, Clone, specta::Type)]
pub struct ServerStatus {
    pub online: bool,
    pub players: i32,
    pub max_players: i32,
    pub ping_ms: i64,
    pub motd: String,
    pub version: String,
}

pub async fn ping_server(address: &str) -> ServerStatus {
    match tokio::time::timeout(TIMEOUT, ping_inner(address)).await {
        Ok(Ok(status)) => status,
        _ => ServerStatus {
            online: false,
            players: 0,
            max_players: 0,
            ping_ms: -1,
            motd: String::new(),
            version: String::new(),
        },
    }
}

async fn resolve_srv(host: &str) -> Option<(String, u16)> {
    let resolver = hickory_resolver::TokioResolver::builder_tokio()
        .ok()?
        .build();
    let srv_name = format!("_minecraft._tcp.{host}.");
    let lookup = resolver.srv_lookup(&srv_name).await.ok()?;
    let record = lookup.iter().next()?;
    let target = record
        .target()
        .to_string()
        .trim_end_matches('.')
        .to_string();
    Some((target, record.port()))
}

async fn ping_inner(address: &str) -> Result<ServerStatus, Box<dyn std::error::Error>> {
    let (mut host, mut port) = parse_address(address);
    if let Some((srv_host, srv_port)) = resolve_srv(&host).await {
        host = srv_host;
        port = srv_port;
    }
    let addr = format!("{host}:{port}");
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(&addr)).await??;

    let mut handshake = Vec::new();
    write_varint(&mut handshake, 0x00);
    write_varint(&mut handshake, PROTOCOL_VERSION as u32);
    write_string(&mut handshake, &host);
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1);

    let mut packet = Vec::new();
    write_varint(&mut packet, handshake.len() as u32);
    packet.extend_from_slice(&handshake);
    stream.write_all(&packet).await?;

    let mut status_req = Vec::new();
    write_varint(&mut status_req, 1);
    write_varint(&mut status_req, 0x00);
    stream.write_all(&status_req).await?;

    let _length = read_varint(&mut stream).await?;
    let _packet_id = read_varint(&mut stream).await?;
    let json_len = read_varint(&mut stream).await? as usize;
    let mut json_buf = vec![0u8; json_len];
    stream.read_exact(&mut json_buf).await?;
    let json_str = String::from_utf8(json_buf)?;

    let ping_start = Instant::now();
    let mut ping_packet = Vec::new();
    write_varint(&mut ping_packet, 9);
    write_varint(&mut ping_packet, 0x01);
    ping_packet.extend_from_slice(&0i64.to_be_bytes());
    stream.write_all(&ping_packet).await?;

    let _pong_len = read_varint(&mut stream).await?;
    let _pong_id = read_varint(&mut stream).await?;
    let mut pong_payload = [0u8; 8];
    stream.read_exact(&mut pong_payload).await?;
    let ping_ms = ping_start.elapsed().as_millis() as i64;

    let parsed: serde_json::Value = serde_json::from_str(&json_str)?;

    let players = parsed
        .get("players")
        .and_then(|p| p.get("online"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let max_players = parsed
        .get("players")
        .and_then(|p| p.get("max"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let motd = parsed
        .get("description")
        .map(|d| {
            if let Some(text) = d.as_str() {
                text.to_string()
            } else {
                d.get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string()
            }
        })
        .unwrap_or_default();

    let version = parsed
        .get("version")
        .and_then(|v| {
            v.get("protocol")
                .and_then(|p| p.as_i64())
                .and_then(|p| i32::try_from(p).ok())
                .and_then(|p| {
                    crate::VERSION_PROTOCOL_MAP
                        .iter()
                        .find(|(_, val)| *val == p)
                })
                .map(|(n, _)| (*n).to_string())
                .or_else(|| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
        })
        .unwrap_or_default();

    Ok(ServerStatus {
        online: true,
        players,
        max_players,
        ping_ms,
        motd,
        version,
    })
}

fn parse_address(address: &str) -> (String, u16) {
    if let Some((host, port_str)) = address.rsplit_once(':')
        && let Ok(port) = port_str.parse::<u16>()
    {
        return (host.to_string(), port);
    }
    (address.to_string(), 25565)
}

fn write_varint(buf: &mut Vec<u8>, value: u32) {
    let mut val = value;
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if val == 0 {
            break;
        }
    }
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_varint(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

async fn read_varint(stream: &mut TcpStream) -> Result<i32, Box<dyn std::error::Error>> {
    let mut result = 0i32;
    let mut shift = 0;
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await?;
        result |= ((byte[0] & 0x7F) as i32) << shift;
        if byte[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 32 {
            return Err("VarInt too large".into());
        }
    }
    Ok(result)
}
