//! The login phase, as vanilla 26.2 lays it out
//! (`net/minecraft/network/protocol/login`, and the cookie packets it shares
//! with later phases in `protocol/cookie`).

use crate::codec::{DecodeResult, Reader};
use crate::packets::{PacketTable, Phase};
use crate::player_info::{Property, property};
use crate::wire::{write_utf, write_varint};

/// `ClientboundLoginDisconnectPacket`'s `lenientJson(262144)`.
const MAX_REASON_LENGTH: usize = 262_144;
/// `ByteBufCodecs.PLAYER_NAME`.
const MAX_NAME_LENGTH: usize = 16;
/// `ByteBufCodecs.GAME_PROFILE_PROPERTIES`' `readCount`.
const MAX_PROPERTIES: usize = 16;

/// What the client sends while logging in.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerboundLogin {
    /// `ServerboundHelloPacket`.
    Hello { name: String, profile_id: u128 },
    /// `ServerboundKeyPacket`: the shared secret and the server's challenge,
    /// each encrypted with its public key.
    Key {
        key: Vec<u8>,
        encrypted_challenge: Vec<u8>,
    },
    /// `ServerboundCustomQueryAnswerPacket` with no payload, the answer
    /// vanilla gives every query.
    CustomQueryAnswer { transaction_id: i32 },
    /// `ServerboundLoginAcknowledgedPacket`.
    LoginAcknowledged,
    /// `ServerboundCookieResponsePacket`.
    CookieResponse {
        key: String,
        payload: Option<Vec<u8>>,
    },
}

impl ServerboundLogin {
    /// The packet's name in the packet table.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Key { .. } => "key",
            Self::CustomQueryAnswer { .. } => "custom_query_answer",
            Self::LoginAcknowledged => "login_acknowledged",
            Self::CookieResponse { .. } => "cookie_response",
        }
    }

    /// The native frame: the packet id, then the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = PacketTable::native()
            .serverbound_frame(Phase::Login, self.name())
            .unwrap_or_else(|| panic!("{} missing from packet table", self.name()));
        match self {
            Self::Hello { name, profile_id } => {
                write_utf(&mut buf, name);
                buf.extend(profile_id.to_be_bytes());
            }
            Self::Key {
                key,
                encrypted_challenge,
            } => {
                byte_array(&mut buf, key);
                byte_array(&mut buf, encrypted_challenge);
            }
            Self::CustomQueryAnswer { transaction_id } => {
                write_varint(&mut buf, *transaction_id as u32);
                buf.push(0);
            }
            Self::LoginAcknowledged => {}
            Self::CookieResponse { key, payload } => {
                write_utf(&mut buf, key);
                buf.push(u8::from(payload.is_some()));
                if let Some(payload) = payload {
                    byte_array(&mut buf, payload);
                }
            }
        }
        buf
    }
}

/// `FriendlyByteBuf.writeByteArray`.
fn byte_array(buf: &mut Vec<u8>, bytes: &[u8]) {
    write_varint(buf, bytes.len() as u32);
    buf.extend_from_slice(bytes);
}

/// What the server sends while the client logs in.
#[derive(Clone, Debug, PartialEq)]
pub enum LoginPacket {
    /// The reason, a text component as JSON.
    // TODO: vanilla parses it with `LenientJsonParser`, which also takes JSON
    // that `serde_json` rejects.
    Disconnect(serde_json::Value),
    Hello(Hello),
    Finished(Finished),
    /// The size from which frames are compressed; negative for never.
    Compression(i32),
    /// `ClientboundCustomQueryPacket`; vanilla discards the payload too.
    CustomQuery {
        transaction_id: i32,
    },
    /// The key of the cookie asked for.
    CookieRequest(String),
}

/// `ClientboundHelloPacket`, the encryption request.
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub server_id: String,
    pub public_key: Vec<u8>,
    pub challenge: Vec<u8>,
    pub should_authenticate: bool,
}

/// `ClientboundLoginFinishedPacket`.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub profile: GameProfile,
    pub session_id: u128,
}

/// `ByteBufCodecs.GAME_PROFILE`.
#[derive(Clone, Debug, PartialEq)]
pub struct GameProfile {
    pub id: u128,
    pub name: String,
    pub properties: Vec<Property>,
}

/// Decodes login packet `name`; `None` if it isn't one.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<LoginPacket>> {
    Some(match name {
        "login_disconnect" => r.utf(MAX_REASON_LENGTH).and_then(|reason| {
            serde_json::from_str(reason)
                .map(LoginPacket::Disconnect)
                .map_err(|_| r.error("reason isn't JSON"))
        }),
        "hello" => hello(r).map(LoginPacket::Hello),
        "login_finished" => finished(r).map(LoginPacket::Finished),
        "login_compression" => r.var_i32().map(LoginPacket::Compression),
        "custom_query" => custom_query(r),
        "cookie_request" => r
            .identifier()
            .map(|key| LoginPacket::CookieRequest(key.to_owned())),
        _ => return None,
    })
}

/// Decodes a whole clientbound login frame, its id first.
pub fn read_frame(frame: &[u8]) -> DecodeResult<LoginPacket> {
    PacketTable::native().read_clientbound(Phase::Login, frame, read)
}

fn hello(r: &mut Reader) -> DecodeResult<Hello> {
    Ok(Hello {
        server_id: r.utf(20)?.to_owned(),
        public_key: whole_byte_array(r)?,
        challenge: whole_byte_array(r)?,
        should_authenticate: r.bool()?,
    })
}

/// `FriendlyByteBuf.readByteArray()`, bounded only by what is left.
fn whole_byte_array(r: &mut Reader) -> DecodeResult<Vec<u8>> {
    Ok(r.byte_array(r.remaining())?.to_vec())
}

fn finished(r: &mut Reader) -> DecodeResult<Finished> {
    Ok(Finished {
        profile: GameProfile {
            id: r.uuid()?,
            name: r.utf(MAX_NAME_LENGTH)?.to_owned(),
            properties: r.list(MAX_PROPERTIES, property)?,
        },
        session_id: r.uuid()?,
    })
}

/// `ClientboundCustomQueryPacket.readUnknownPayload`: the channel, then
/// whatever is left, skipped.
fn custom_query(r: &mut Reader) -> DecodeResult<LoginPacket> {
    /// `ClientboundCustomQueryPacket.MAX_PAYLOAD_SIZE`.
    const MAX_PAYLOAD: usize = 0x10_0000;

    let transaction_id = r.var_i32()?;
    r.identifier()?;
    if r.remaining() > MAX_PAYLOAD {
        return Err(r.error("payload over its maximum"));
    }
    r.bytes(r.remaining())?;
    Ok(LoginPacket::CustomQuery { transaction_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::Direction;
    use crate::vanilla_fixtures::case;

    const UUID: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;

    /// One of every serverbound packet, carrying what `CodecOracle`'s `login`
    /// encoded.
    fn packets() -> Vec<ServerboundLogin> {
        vec![
            ServerboundLogin::Hello {
                name: "Player".into(),
                profile_id: UUID,
            },
            ServerboundLogin::CustomQueryAnswer {
                transaction_id: 300,
            },
            ServerboundLogin::LoginAcknowledged,
            ServerboundLogin::CookieResponse {
                key: "minecraft:session".into(),
                payload: Some(vec![1, 2, 3]),
            },
        ]
    }

    #[test]
    fn serverbound_packets_encode_as_vanilla_does() {
        for packet in packets() {
            let frame = packet.encode();
            assert_eq!(
                frame[1..],
                case(&format!("login/serverbound/{}", packet.name())),
                "{}",
                packet.name()
            );
        }
    }

    /// The key packet's constructor encrypts, so its layout is pinned by hand:
    /// two length-prefixed byte arrays.
    #[test]
    fn key_is_two_byte_arrays() {
        let key = ServerboundLogin::Key {
            key: vec![9; 3],
            encrypted_challenge: vec![7; 2],
        };
        assert_eq!(key.encode(), [1, 3, 9, 9, 9, 2, 7, 7]);
        let absent = ServerboundLogin::CookieResponse {
            key: "a".into(),
            payload: None,
        };
        assert_eq!(absent.encode(), [4, 1, b'a', 0]);
    }

    /// The clientbound frame of fixture `name`.
    fn frame(name: &str) -> Vec<u8> {
        let id = PacketTable::native()
            .id(Phase::Login, Direction::Clientbound, name)
            .unwrap();
        let mut frame = vec![id as u8];
        frame.extend(case(&format!("login/clientbound/{name}")));
        frame
    }

    #[test]
    fn clientbound_packets_match_vanilla_encoder() {
        assert_eq!(
            read_frame(&frame("login_disconnect")),
            Ok(LoginPacket::Disconnect("Kicked".into()))
        );
        assert_eq!(
            read_frame(&frame("hello")),
            Ok(LoginPacket::Hello(Hello {
                server_id: "server".into(),
                public_key: vec![1, 2, 3],
                challenge: vec![4, 5, 6, 7],
                should_authenticate: true,
            }))
        );
        assert_eq!(
            read_frame(&frame("login_finished")),
            Ok(LoginPacket::Finished(Finished {
                profile: GameProfile {
                    id: UUID,
                    name: "Player".into(),
                    properties: vec![Property {
                        name: "textures".into(),
                        value: "value".into(),
                        signature: Some("signature".into()),
                    }],
                },
                session_id: UUID.swap_bytes(),
            }))
        );
        assert_eq!(
            read_frame(&frame("login_compression")),
            Ok(LoginPacket::Compression(256))
        );
        assert_eq!(
            read_frame(&frame("custom_query")),
            Ok(LoginPacket::CustomQuery {
                transaction_id: 300
            })
        );
        assert_eq!(
            read_frame(&frame("cookie_request")),
            Ok(LoginPacket::CookieRequest("minecraft:session".into()))
        );
    }

    #[test]
    fn a_frame_that_isnt_a_login_packet_is_an_error() {
        assert!(read_frame(&[9]).is_err());
        // Compression with a byte left over.
        assert!(read_frame(&[3, 0, 0]).is_err());
        assert!(read_frame(&[0, 1, b'{']).is_err());
    }
}
