//! The handshake and the status ping, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/{handshake,status,ping}`). Their ids and
//! layouts are the same on every version, which is how a client asks a server
//! which one it speaks.

use serde::Deserialize;

use crate::codec::{DecodeResult, Reader};
use crate::packets::{Direction, PacketTable, Phase};
use crate::wire::{write_utf, write_varint};

/// `ClientboundStatusResponsePacket`'s `lenientJson(Short.MAX_VALUE)`.
const MAX_STATUS_LENGTH: usize = i16::MAX as usize;

/// `ClientIntent`, by its wire id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientIntent {
    Status = 1,
    Login = 2,
    Transfer = 3,
}

/// `ClientIntentionPacket`, the one handshake packet.
#[derive(Clone, Debug, PartialEq)]
pub struct Intention {
    pub protocol_version: i32,
    pub host: String,
    pub port: u16,
    pub intent: ClientIntent,
}

impl Intention {
    /// The frame: the packet id, then the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = frame(Phase::Handshake, "intention");
        write_varint(&mut buf, self.protocol_version as u32);
        write_utf(&mut buf, &self.host);
        buf.extend(self.port.to_be_bytes());
        write_varint(&mut buf, self.intent as u32);
        buf
    }
}

/// `ServerboundStatusRequestPacket`, which has no body.
pub fn encode_status_request() -> Vec<u8> {
    frame(Phase::Status, "status_request")
}

/// `ServerboundPingRequestPacket`.
pub fn encode_ping_request(time: i64) -> Vec<u8> {
    let mut buf = frame(Phase::Status, "ping_request");
    buf.extend(time.to_be_bytes());
    buf
}

fn frame(phase: Phase, name: &str) -> Vec<u8> {
    let id = PacketTable::native()
        .id(phase, Direction::Serverbound, name)
        .unwrap_or_else(|| panic!("{name} missing from packet table"));
    let mut buf = Vec::new();
    write_varint(&mut buf, id);
    buf
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatusPacket {
    Response(ServerStatus),
    /// The `time` of the ping it answers.
    Pong(i64),
}

/// `ServerStatus`. Every field is lenient, as its codec's
/// `lenientOptionalFieldOf`: one that is missing or malformed reads as absent.
// TODO: vanilla parses the payload with `LenientJsonParser`, which also takes
// JSON that `serde_json` rejects (unquoted names, comments).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerStatus {
    /// The MOTD, a text component as JSON; null when absent.
    pub description: serde_json::Value,
    pub players: Option<Players>,
    pub version: Option<Version>,
    /// The icon as sent, a `data:image/png;base64,` URL.
    pub favicon: Option<String>,
    pub enforces_secure_chat: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Players {
    pub max: i32,
    pub online: i32,
    #[serde(default, deserialize_with = "lenient")]
    pub sample: Vec<NameAndId>,
}

/// A listed player; `id` is its UUID as a string.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct NameAndId {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Version {
    pub name: String,
    pub protocol: i32,
}

/// A field's value, or its default when it doesn't parse.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

impl ServerStatus {
    fn from_json(mut json: serde_json::Value) -> Self {
        let mut field = |name: &str| json.get_mut(name).map(serde_json::Value::take);
        Self {
            description: field("description").unwrap_or_default(),
            players: field("players").and_then(|v| serde_json::from_value(v).ok()),
            version: field("version").and_then(|v| serde_json::from_value(v).ok()),
            favicon: field("favicon").and_then(|v| serde_json::from_value(v).ok()),
            enforces_secure_chat: field("enforcesSecureChat")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }
}

/// Decodes status packet `name`; `None` if it isn't one.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<StatusPacket>> {
    Some(match name {
        "status_response" => r.utf(MAX_STATUS_LENGTH).and_then(|payload| {
            serde_json::from_str(payload)
                .map(|json| StatusPacket::Response(ServerStatus::from_json(json)))
                .map_err(|_| r.error("status isn't JSON"))
        }),
        "pong_response" => r.i64().map(StatusPacket::Pong),
        _ => return None,
    })
}

/// Decodes a whole clientbound status frame, its id first.
pub fn read_frame(frame: &[u8]) -> DecodeResult<StatusPacket> {
    let mut r = Reader::new(frame);
    let id = r.varint()?;
    let packet = PacketTable::native()
        .name_of(Phase::Status, Direction::Clientbound, id)
        .and_then(|name| read(name, &mut r))
        .unwrap_or_else(|| Err(r.error("not a status packet")))?;
    r.ensure_eof()?;
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    /// A frame's body, past its one-byte packet id.
    fn body(frame: &[u8]) -> &[u8] {
        &frame[1..]
    }

    #[test]
    fn serverbound_packets_encode_as_vanilla_does() {
        let intention = Intention {
            protocol_version: 776,
            host: "play.example.org".into(),
            port: 25565,
            intent: ClientIntent::Login,
        };
        assert_eq!(body(&intention.encode()), case("handshake/intention"));
        assert_eq!(
            body(&encode_ping_request(0x0123_4567_89ab_cdef)),
            case("status/ping_request")
        );
    }

    #[test]
    fn frames_carry_their_table_ids() {
        assert_eq!(encode_status_request(), [0]);
        assert_eq!(encode_ping_request(0)[0], 1);
        let status = Intention {
            protocol_version: 0,
            host: String::new(),
            port: 0,
            intent: ClientIntent::Status,
        };
        assert_eq!(status.encode(), [0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn status_response_matches_vanilla_encoder() {
        let mut frame = vec![0];
        frame.extend(case("status/status_response"));
        let StatusPacket::Response(status) = read_frame(&frame).unwrap() else {
            panic!("not a response");
        };
        assert_eq!(status.description, "A Minecraft Server");
        assert_eq!(
            status.players,
            Some(Players {
                max: 20,
                online: 1,
                sample: vec![NameAndId {
                    id: "01234567-89ab-cdef-fedc-ba9876543210".into(),
                    name: "Player".into(),
                }],
            })
        );
        assert_eq!(
            status.version,
            Some(Version {
                name: "26.2".into(),
                protocol: 776,
            })
        );
        assert_eq!(
            status.favicon.as_deref(),
            Some("data:image/png;base64,AQID")
        );
        assert!(status.enforces_secure_chat);
    }

    #[test]
    fn pong_matches_vanilla_encoder() {
        let mut frame = vec![1];
        frame.extend(case("status/pong_response"));
        assert_eq!(
            read_frame(&frame),
            Ok(StatusPacket::Pong(0x0123_4567_89ab_cdef))
        );
    }

    #[test]
    fn missing_and_malformed_fields_read_as_absent() {
        let read = |json: &str| ServerStatus::from_json(serde_json::from_str(json).unwrap());
        assert_eq!(read("{}"), ServerStatus::default());

        let status = read(
            r#"{"version": {"name": 5}, "players": {"max": 9, "online": 2, "sample": "none"},
                "favicon": 1, "enforcesSecureChat": "yes"}"#,
        );
        assert_eq!(status.version, None);
        assert_eq!(
            status.players,
            Some(Players {
                max: 9,
                online: 2,
                sample: Vec::new(),
            })
        );
        assert_eq!(status.favicon, None);
        assert!(!status.enforces_secure_chat);
    }

    #[test]
    fn a_frame_that_isnt_a_status_packet_is_an_error() {
        assert!(read_frame(&[2]).is_err());
        assert!(read_frame(&[1, 0]).is_err());
        assert!(read_frame(&[0, 1, b'{']).is_err());
    }
}
