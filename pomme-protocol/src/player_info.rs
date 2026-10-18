//! Player list packets, as vanilla 26.2 lays them out
//! (`net/minecraft/network/protocol/game/ClientboundPlayerInfo*Packet`).
//! Display names stay as NBT.

use crate::codec::{DecodeResult, MAX_STRING_LENGTH, Reader};

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerInfoPacket {
    Update {
        actions: Actions,
        entries: Vec<Entry>,
    },
    Remove(Vec<u128>),
}

/// `ClientboundPlayerInfoUpdatePacket.Action`, in ordinal order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Actions {
    pub add_player: bool,
    pub initialize_chat: bool,
    pub update_game_mode: bool,
    pub update_listed: bool,
    pub update_latency: bool,
    pub update_display_name: bool,
    pub update_list_order: bool,
    pub update_hat: bool,
}

/// One player's entry; fields its packet's actions leave out keep vanilla's
/// `EntryBuilder` defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub profile_id: u128,
    pub name: String,
    pub properties: Vec<Property>,
    pub chat_session: Option<ChatSession>,
    /// A `GameType` id.
    pub game_mode: u32,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<Box<[u8]>>,
    pub list_order: i32,
    pub show_hat: bool,
}

/// A game profile property, such as `textures`.
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// `RemoteChatSession.Data`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatSession {
    pub session_id: u128,
    /// `ProfilePublicKey.Data`: expiry in epoch milliseconds, the
    /// DER-encoded key and Mojang's signature over them.
    pub expires_at: i64,
    pub key: Box<[u8]>,
    pub key_signature: Box<[u8]>,
}

/// Decodes packet `name`, or `None` if it isn't a player list packet.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<PlayerInfoPacket>> {
    let read: fn(&mut Reader) -> DecodeResult<PlayerInfoPacket> = match name {
        // ClientboundPlayerInfoUpdatePacket.java:34-43
        "player_info_update" => |r| {
            let actions = actions(r.u8()?);
            let entries = r.list(usize::MAX, |r| entry(r, actions))?;
            Ok(PlayerInfoPacket::Update { actions, entries })
        },
        // ClientboundPlayerInfoRemovePacket.java:12
        "player_info_remove" => |r| Ok(PlayerInfoPacket::Remove(r.list(usize::MAX, Reader::uuid)?)),
        _ => return None,
    };
    Some(read(r))
}

/// `readEnumSet`: eight actions fit one fixed bitset byte, bit N for
/// ordinal N.
fn actions(mask: u8) -> Actions {
    let bit = |n: u8| mask & (1 << n) != 0;
    Actions {
        add_player: bit(0),
        initialize_chat: bit(1),
        update_game_mode: bit(2),
        update_listed: bit(3),
        update_latency: bit(4),
        update_display_name: bit(5),
        update_list_order: bit(6),
        update_hat: bit(7),
    }
}

/// The profile id, then each action's reader in ordinal order (`Action`,
/// :88-117).
fn entry(r: &mut Reader, actions: Actions) -> DecodeResult<Entry> {
    let mut entry = Entry {
        profile_id: r.uuid()?,
        ..Default::default()
    };
    if actions.add_player {
        // ByteBufCodecs.PLAYER_NAME, GAME_PROFILE_PROPERTIES (:280-306)
        entry.name = r.utf(16)?.to_owned();
        entry.properties = r.list(16, property)?;
    }
    if actions.initialize_chat {
        entry.chat_session = r.optional(chat_session)?;
    }
    if actions.update_game_mode {
        entry.game_mode = r.varint()?;
    }
    if actions.update_listed {
        entry.listed = r.bool()?;
    }
    if actions.update_latency {
        entry.latency = r.var_i32()?;
    }
    if actions.update_display_name {
        entry.display_name = r.optional(|r| Ok(r.nbt()?.into()))?;
    }
    if actions.update_list_order {
        entry.list_order = r.var_i32()?;
    }
    if actions.update_hat {
        entry.show_hat = r.bool()?;
    }
    Ok(entry)
}

fn property(r: &mut Reader) -> DecodeResult<Property> {
    Ok(Property {
        name: r.utf(64)?.to_owned(),
        value: r.utf(MAX_STRING_LENGTH)?.to_owned(),
        signature: r.optional(|r| Ok(r.utf(1024)?.to_owned()))?,
    })
}

/// `RemoteChatSession.Data.read` (RemoteChatSession.java:36-37) and
/// `ProfilePublicKey.Data` (ProfilePublicKey.java:52).
fn chat_session(r: &mut Reader) -> DecodeResult<ChatSession> {
    Ok(ChatSession {
        session_id: r.uuid()?,
        expires_at: r.i64()?,
        key: r.byte_array(512)?.into(),
        key_signature: r.byte_array(4096)?.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{write_utf, write_varint};

    fn decode(name: &str, bytes: &[u8]) -> PlayerInfoPacket {
        let mut r = Reader::new(bytes);
        let packet = read(name, &mut r).expect("a player list packet").unwrap();
        r.ensure_eof().unwrap();
        packet
    }

    /// An update with `mask` and one entry for profile 9 whose action
    /// payloads are `body`.
    fn update(mask: u8, body: &[u8]) -> Vec<u8> {
        [&[mask, 1][..], &9u128.to_be_bytes(), body].concat()
    }

    fn entry_of(actions: u8, body: &[u8]) -> (Actions, Entry) {
        let PlayerInfoPacket::Update { actions, entries } =
            decode("player_info_update", &update(actions, body))
        else {
            panic!("not an update");
        };
        let [entry] = &entries[..] else {
            panic!("{entries:?}");
        };
        (actions, entry.clone())
    }

    fn utf(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        write_utf(&mut out, s);
        out
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("block_update", &mut Reader::new(&[])).is_none());
    }

    #[test]
    fn each_action_alone() {
        let base = Entry {
            profile_id: 9,
            ..Default::default()
        };
        let mut latency = Vec::new();
        write_varint(&mut latency, -1i32 as u32);
        let text = [8, 0, 1, b'x'];
        let cases: [(u8, Vec<u8>, Entry); 7] = [
            (
                1 << 0,
                [utf("bob"), vec![0]].concat(),
                Entry {
                    name: "bob".into(),
                    ..base.clone()
                },
            ),
            (1 << 1, vec![0], base.clone()),
            (
                1 << 2,
                vec![3],
                Entry {
                    game_mode: 3,
                    ..base.clone()
                },
            ),
            (
                1 << 3,
                vec![1],
                Entry {
                    listed: true,
                    ..base.clone()
                },
            ),
            (
                1 << 4,
                latency,
                Entry {
                    latency: -1,
                    ..base.clone()
                },
            ),
            (
                1 << 5,
                [&[1][..], &text].concat(),
                Entry {
                    display_name: Some(text.into()),
                    ..base.clone()
                },
            ),
            (
                1 << 7,
                vec![1],
                Entry {
                    show_hat: true,
                    ..base.clone()
                },
            ),
        ];
        for (mask, body, expected) in cases {
            assert_eq!(entry_of(mask, &body).1, expected, "mask {mask:#x}");
        }
        let (actions, entry) = entry_of(1 << 6, &[5]);
        assert_eq!(
            actions,
            Actions {
                update_list_order: true,
                ..Default::default()
            }
        );
        assert_eq!(entry.list_order, 5);
    }

    #[test]
    fn every_action_in_ordinal_order() {
        let body = [
            &utf("bob")[..],
            // one signed property
            &[1],
            &utf("textures"),
            &utf("abc"),
            &[1],
            &utf("sig"),
            // a chat session
            &[1],
            &7u128.to_be_bytes(),
            &1000i64.to_be_bytes(),
            &[2, 0xAA, 0xBB, 1, 0xCC],
            &[1, 1, 0, 0, 3, 1],
        ]
        .concat();
        let (actions, entry) = entry_of(0xFF, &body);
        assert!(actions.add_player && actions.update_hat);
        assert_eq!(
            entry,
            Entry {
                profile_id: 9,
                name: "bob".into(),
                properties: vec![Property {
                    name: "textures".into(),
                    value: "abc".into(),
                    signature: Some("sig".into()),
                }],
                chat_session: Some(ChatSession {
                    session_id: 7,
                    expires_at: 1000,
                    key: [0xAA, 0xBB].into(),
                    key_signature: [0xCC].into(),
                }),
                game_mode: 1,
                listed: true,
                latency: 0,
                display_name: None,
                list_order: 3,
                show_hat: true,
            }
        );
    }

    #[test]
    fn at_most_sixteen_properties() {
        let property = [utf("a"), utf("b"), vec![0]].concat();
        let body = |n: u8| {
            let mut body = [utf("bob"), vec![n]].concat();
            for _ in 0..n {
                body.extend(&property);
            }
            body
        };
        assert_eq!(entry_of(1, &body(16)).1.properties.len(), 16);
        let bytes = update(1, &body(17));
        assert!(
            read("player_info_update", &mut Reader::new(&bytes))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn remove_lists_profile_ids() {
        let bytes = [&[2][..], &1u128.to_be_bytes(), &2u128.to_be_bytes()].concat();
        assert_eq!(
            decode("player_info_remove", &bytes),
            PlayerInfoPacket::Remove(vec![1, 2])
        );
    }
}
