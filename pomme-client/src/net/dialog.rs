//! Pomme-owned decoding of the common dialog packets (vanilla
//! `ClientCommonPacketListenerImpl`), read from native-layout frames in
//! either phase before azalea's typed decode, which rejects object
//! components.

use crossbeam_channel::Sender;
use pomme_protocol::wire::read_varint;
use pomme_protocol::{Direction, PacketTable, Phase};
use simdnbt::owned::NbtTag;

use super::NetworkEvent;
use super::chat::{
    ensure_end, read_bool, read_component, read_nbt_tag, read_string, read_varint_req,
};
use crate::chat_component::Component;
use crate::ui::server_dialog::{DialogReference, ServerLink};

/// `ServerLinks.KnownLinkType` names, by id.
const KNOWN_LINK_TYPES: [&str; 10] = [
    "report_bug",
    "community_guidelines",
    "support",
    "status",
    "feedback",
    "community",
    "website",
    "forums",
    "news",
    "announcements",
];

/// Decodes the dialog packets pomme reads itself; `false` when `raw` is none
/// of them. A malformed one is consumed with a warning. Play's
/// `show_dialog`/`clear_dialog` are left to azalea.
pub fn handle_raw_dialog_packet(phase: Phase, raw: &[u8], event_tx: &Sender<NetworkEvent>) -> bool {
    let mut pos = 0;
    let Some(id) = read_varint(raw, &mut pos) else {
        return false;
    };
    let Some(name) = PacketTable::native().name_of(phase, Direction::Clientbound, id) else {
        return false;
    };
    let configuration = phase == Phase::Configuration;
    let event = match name {
        "server_links" => {
            parse_server_links(raw, &mut pos).map(|links| NetworkEvent::ServerLinks { links })
        }
        "show_dialog" if configuration => {
            parse_inline_dialog(raw, &mut pos).map(|dialog| NetworkEvent::ShowDialog { dialog })
        }
        "clear_dialog" if configuration => Ok(NetworkEvent::ClearDialog),
        _ => return false,
    };
    match event.and_then(|event| ensure_end(raw, pos, name).map(|()| event)) {
        Ok(event) => {
            let _ = event_tx.try_send(event);
        }
        Err(error) => tracing::warn!("Skipping malformed {name} packet: {error}"),
    }
    true
}

/// `ClientboundServerLinksPacket` (`ServerLinks.UNTRUSTED_LINKS_STREAM_CODEC`),
/// keeping the entries `handleServerLinks` accepts.
fn parse_server_links(raw: &[u8], pos: &mut usize) -> Result<Vec<ServerLink>, String> {
    let count = read_varint_req(raw, pos, "server link count")?;
    let mut links = Vec::new();
    for _ in 0..count {
        // `ByteBufCodecs.either`: true is the known type.
        let label = if read_bool(raw, pos)? {
            // `OutOfBoundsStrategy.ZERO`: an unknown id is a bug report link.
            let id = read_varint_req(raw, pos, "server link type")? as usize;
            let name = KNOWN_LINK_TYPES.get(id).unwrap_or(&KNOWN_LINK_TYPES[0]);
            Component::translate(format!("known_server_link.{name}"), Vec::new())
        } else {
            read_component(raw, pos)?
        };
        let url = read_string(raw, pos, 32_767, "server link")?;
        if let Some(url) = validated_server_link_url(&url) {
            links.push(ServerLink { label, url });
        }
    }
    Ok(links)
}

fn validated_server_link_url(url: &str) -> Option<String> {
    crate::chat_component::parse_untrusted_url(url.to_owned())
        .map_err(|error| tracing::warn!("Ignoring invalid server link `{url}`: {error}"))
        .ok()
}

/// Configuration's `ClientboundShowDialogPacket.CONTEXT_FREE_STREAM_CODEC`:
/// always an inline dialog.
fn parse_inline_dialog(raw: &[u8], pos: &mut usize) -> Result<DialogReference, String> {
    match read_nbt_tag(raw, pos)? {
        NbtTag::Compound(nbt) => Ok(DialogReference::inline(&nbt)),
        _ => Err("dialog is not a compound".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::wire::write_varint;
    use simdnbt::owned::NbtCompound;

    use super::*;
    use crate::chat_component::Content;

    fn packet_id(phase: Phase, name: &str) -> u32 {
        PacketTable::native()
            .id(phase, Direction::Clientbound, name)
            .unwrap()
    }

    fn write_string(out: &mut Vec<u8>, value: &str) {
        write_varint(out, value.len() as u32);
        out.extend_from_slice(value.as_bytes());
    }

    fn component_entry(out: &mut Vec<u8>, component: NbtCompound, url: &str) {
        out.push(0);
        NbtTag::Compound(component).write(out);
        write_string(out, url);
    }

    fn decode(phase: Phase, raw: &[u8]) -> Vec<NetworkEvent> {
        let (tx, rx) = crossbeam_channel::unbounded();
        assert!(handle_raw_dialog_packet(phase, raw, &tx));
        rx.try_iter().collect()
    }

    fn links_packet(phase: Phase) -> Vec<u8> {
        let mut raw = Vec::new();
        write_varint(&mut raw, packet_id(phase, "server_links"));
        write_varint(&mut raw, 4);
        // Known type 6 (website).
        raw.push(1);
        write_varint(&mut raw, 6);
        write_string(&mut raw, "https://example.com");

        let mut text = NbtCompound::new();
        text.insert("text", "Wiki");
        component_entry(&mut raw, text, "https://example.com/wiki");

        let mut object = NbtCompound::new();
        object.insert("sprite", "minecraft:item/diamond");
        component_entry(&mut raw, object, "https://example.com/store");

        // Dropped by `parseAndValidateUntrustedUri`.
        raw.push(1);
        write_varint(&mut raw, 0);
        write_string(&mut raw, "javascript:alert(1)");
        raw
    }

    #[test]
    fn server_links_decode_in_both_phases() {
        for phase in [Phase::Configuration, Phase::Game] {
            let events = decode(phase, &links_packet(phase));
            let [NetworkEvent::ServerLinks { links }] = events.as_slice() else {
                panic!("expected one server_links event");
            };
            let urls: Vec<&str> = links.iter().map(|link| link.url.as_str()).collect();
            assert_eq!(
                urls,
                [
                    "https://example.com",
                    "https://example.com/wiki",
                    "https://example.com/store"
                ]
            );
            assert!(matches!(
                &links[0].label.content,
                Content::Translate { key, .. } if key == "known_server_link.website"
            ));
            assert_eq!(links[1].label.plain_text(), "Wiki");
            assert!(matches!(links[2].label.content, Content::Object { .. }));
        }
    }

    #[test]
    fn out_of_range_link_type_is_a_bug_report() {
        let mut raw = Vec::new();
        write_varint(&mut raw, packet_id(Phase::Game, "server_links"));
        write_varint(&mut raw, 1);
        raw.push(1);
        write_varint(&mut raw, 42);
        write_string(&mut raw, "https://example.com/bugs");
        let events = decode(Phase::Game, &raw);
        let [NetworkEvent::ServerLinks { links }] = events.as_slice() else {
            panic!("expected one server_links event");
        };
        assert!(matches!(
            &links[0].label.content,
            Content::Translate { key, .. } if key == "known_server_link.report_bug"
        ));
    }

    #[test]
    fn server_links_accept_only_untrusted_http_and_https_urls() {
        assert_eq!(
            validated_server_link_url("https://example.com/path").as_deref(),
            Some("https://example.com/path")
        );
        assert_eq!(
            validated_server_link_url("http://example.com").as_deref(),
            Some("http://example.com")
        );
        assert!(validated_server_link_url("file:///tmp/pomme").is_none());
        assert!(validated_server_link_url("mailto:test@example.com").is_none());
        assert!(validated_server_link_url("not a uri").is_none());
    }

    #[test]
    fn configuration_dialogs_are_inline() {
        let mut expected = NbtCompound::new();
        expected.insert("type", "minecraft:notice");
        expected.insert("title", "Rules");
        let mut raw = Vec::new();
        write_varint(&mut raw, packet_id(Phase::Configuration, "show_dialog"));
        NbtTag::Compound(expected.clone()).write(&mut raw);
        let events = decode(Phase::Configuration, &raw);
        let [NetworkEvent::ShowDialog { dialog }] = events.as_slice() else {
            panic!("expected one show_dialog event");
        };
        let DialogReference::Holder(crate::chat_component::DialogHolder::Nbt(tag)) = dialog else {
            panic!("expected an inline dialog");
        };
        assert_eq!(tag, &NbtTag::Compound(expected));

        let mut raw = Vec::new();
        write_varint(&mut raw, packet_id(Phase::Configuration, "clear_dialog"));
        assert!(matches!(
            decode(Phase::Configuration, &raw).as_slice(),
            [NetworkEvent::ClearDialog]
        ));

        // Play's holder form stays with azalea.
        let mut raw = Vec::new();
        write_varint(&mut raw, packet_id(Phase::Game, "clear_dialog"));
        let (tx, _rx) = crossbeam_channel::unbounded();
        assert!(!handle_raw_dialog_packet(Phase::Game, &raw, &tx));
    }
}
