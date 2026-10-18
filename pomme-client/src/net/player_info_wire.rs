//! Player list packets, decoded by pomme for every session: translated frames
//! already carry the native layout.

use crossbeam_channel::Sender;
use pomme_protocol::codec::DecodeResult;
use pomme_protocol::player_info::{self, Entry, PlayerInfoPacket};
use uuid::Uuid;

use super::NetworkEvent;
use super::chat::component_spans;
use super::chat_security::{ProfileKeyServices, ValidatedChatSession};
use crate::player::tab_list::PlayerInfoEntry;

/// Decodes a player list packet; `None` for any other packet.
pub(super) fn handle_raw_player_info_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    Some(
        super::read_raw_packet(packet_id, payload, player_info::read)?.map(|packet| {
            if let Some(event) = event(packet) {
                let _ = event_tx.try_send(event);
            }
        }),
    )
}

/// The client event for a packet; `None` when a display name is unreadable.
fn event(packet: PlayerInfoPacket) -> Option<NetworkEvent> {
    Some(match packet {
        PlayerInfoPacket::Update { actions, entries } => NetworkEvent::PlayerInfoUpdate {
            actions,
            entries: entries.into_iter().map(entry).collect::<Option<_>>()?,
        },
        PlayerInfoPacket::Remove(ids) => NetworkEvent::PlayerInfoRemove {
            uuids: ids.into_iter().map(Uuid::from_u128).collect(),
        },
    })
}

fn entry(entry: Entry) -> Option<PlayerInfoEntry> {
    let uuid = Uuid::from_u128(entry.profile_id);
    let chat_session = entry
        .chat_session
        .as_ref()
        .and_then(|session| validate(uuid, &entry.name, session));
    Some(PlayerInfoEntry {
        uuid,
        textures: entry
            .properties
            .into_iter()
            .find(|property| property.name == "textures")
            .map(|property| property.value),
        name: entry.name,
        game_mode: super::game_type(entry.game_mode),
        listed: entry.listed,
        latency: entry.latency,
        display_name: super::present(entry.display_name.as_deref(), component_spans)?,
        list_order: entry.list_order,
        // TODO: carry `show_hat` once the tab overlay draws player faces
        // (vanilla `PlayerTabOverlay` draws the hat layer when it's set).
        chat_session,
    })
}

/// A player's chat session, checked against Mojang's services keys; dropped
/// when it doesn't validate or the keys aren't available.
fn validate(
    uuid: Uuid,
    name: &str,
    session: &player_info::ChatSession,
) -> Option<ValidatedChatSession> {
    let Some(services) = ProfileKeyServices::get() else {
        tracing::warn!(
            player = %name,
            "Ignoring chat session due to missing Mojang Services public key"
        );
        return None;
    };
    services
        .validate_session(uuid, session)
        .inspect_err(|error| {
            tracing::error!(player = %name, "Failed to validate profile key: {error}");
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use pomme_protocol::player_info::Property;

    use super::*;

    #[test]
    fn entries_take_the_skin_and_a_known_game_mode() {
        let property = |name: &str| Property {
            name: name.into(),
            value: format!("{name} value"),
            signature: None,
        };
        let mapped = |game_mode| {
            entry(Entry {
                profile_id: 9,
                name: "bob".into(),
                properties: vec![property("other"), property("textures")],
                game_mode,
                ..Default::default()
            })
            .unwrap()
        };
        let bob = mapped(3);
        assert_eq!(bob.uuid, Uuid::from_u128(9));
        assert_eq!(bob.textures.as_deref(), Some("textures value"));
        assert_eq!(bob.game_mode, 3);
        assert_eq!(mapped(4).game_mode, 0);
    }
}
