//! Pomme-owned inbound game-chat decoding.
//!
//! Game frames reach this module after protocol-version translation but before
//! Azalea's typed packet decoder. That ordering is intentional: Azalea's 26.2
//! text-component decoder drops hover-event data, so chat components must be
//! decoded here if Pomme is going to support Vanilla interaction semantics.

use std::io::Cursor;

use crossbeam_channel::Sender;
use pomme_protocol::{Direction, PacketTable, Phase};
use serde_json::Value;
use simdnbt::owned::{NbtCompound, NbtTag};

use super::NetworkEvent;
use crate::chat_component::{Argument, Component, HoverEvent, Style};
use crate::ui::text::format_component_spans;

#[derive(Clone, Debug, Default)]
pub struct ChatTypeRegistry {
    entries: Vec<NbtCompound>,
}

impl ChatTypeRegistry {
    pub fn from_entries(entries: Vec<NbtCompound>) -> Self {
        Self { entries }
    }
}

#[derive(Clone, Debug)]
struct ChatDecoration {
    translation_key: String,
    parameters: Vec<DecorationParameter>,
    style: Style,
}

#[derive(Clone, Copy, Debug)]
enum DecorationParameter {
    Sender,
    Target,
    Content,
}

#[derive(Clone, Debug)]
struct BoundChatType {
    decoration: ChatDecoration,
    name: Component,
    target_name: Option<Component>,
}

#[derive(Clone, Debug)]
enum FilterMask {
    PassThrough,
    FullyFiltered,
    Partial(Vec<u64>),
}

/// Returns `None` when this is not a chat packet. Chat packets are always
/// consumed, including malformed ones (reported through the `Err`) so a bad
/// payload never falls through to Azalea's lossy component decoder.
pub fn encode_outbound_message(message: &str, timestamp_millis: u64) -> Result<Vec<u8>, String> {
    if message.chars().count() > 256 {
        return Err("chat message exceeds 256 characters".into());
    }
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat")
        .ok_or_else(|| "latest protocol has no serverbound chat packet".to_owned())?;
    let mut out = Vec::with_capacity(message.len() + 32);
    pomme_protocol::wire::write_varint(&mut out, id);
    write_wire_string(&mut out, message);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes()); // salt
    out.push(0); // no signature
    pomme_protocol::wire::write_varint(&mut out, 0); // last-seen offset
    out.extend_from_slice(&[0; 3]); // 20 acknowledged bits
    out.push(0); // ignore last-seen checksum
    Ok(out)
}

pub fn encode_outbound_command(command: &str) -> Result<Vec<u8>, String> {
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat_command")
        .ok_or_else(|| "latest protocol has no serverbound chat_command packet".to_owned())?;
    let mut out = Vec::with_capacity(command.len() + 6);
    pomme_protocol::wire::write_varint(&mut out, id);
    write_wire_string(&mut out, command);
    Ok(out)
}

pub fn handle_raw_chat_packet(
    raw: &[u8],
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
) -> Option<Result<(), String>> {
    let mut pos = 0usize;
    let packet_id = read_varint(raw, &mut pos)?;
    let name = PacketTable::latest().name_of(Phase::Game, Direction::Clientbound, packet_id)?;

    let result = match name {
        "system_chat" => parse_system_chat(raw, &mut pos, event_tx),
        "set_action_bar_text" => parse_action_bar(raw, &mut pos, event_tx),
        "disguised_chat" => parse_disguised_chat(raw, &mut pos, event_tx, chat_types),
        "player_chat" => parse_player_chat(raw, &mut pos, event_tx, chat_types),
        _ => return None,
    };
    Some(result)
}

fn parse_system_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), String> {
    let component = read_component(raw, pos)?;
    let overlay = read_bool(raw, pos)?;
    ensure_end(raw, *pos, "system_chat")?;
    if overlay {
        send_action_bar(event_tx, &component);
    } else {
        send_chat(event_tx, &component);
    }
    Ok(())
}

fn parse_action_bar(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), String> {
    let component = read_component(raw, pos)?;
    ensure_end(raw, *pos, "set_action_bar_text")?;
    send_action_bar(event_tx, &component);
    Ok(())
}

fn parse_disguised_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
) -> Result<(), String> {
    let content = read_component(raw, pos)?;
    let bound = read_bound_chat_type(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "disguised_chat")?;
    send_chat(event_tx, &decorate(content, bound));
    Ok(())
}

fn parse_player_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
) -> Result<(), String> {
    // globalIndex, sender UUID, message index
    read_varint_req(raw, pos, "player_chat.global_index")?;
    skip(raw, pos, 16, "player_chat.sender")?;
    read_varint_req(raw, pos, "player_chat.index")?;

    // Nullable 256-byte message signature.
    if read_bool(raw, pos)? {
        skip(raw, pos, 256, "player_chat.signature")?;
    }

    let signed_content = read_string(raw, pos, 256, "player_chat.body.content")?;
    // Instant epoch millis + salt.
    skip(raw, pos, 16, "player_chat.timestamp_and_salt")?;

    // LastSeenMessages.Packed: max 20 packed signatures. Packed id 0 carries
    // the full 256-byte signature; other ids refer to the client's cache.
    let last_seen = read_varint_req(raw, pos, "player_chat.last_seen.count")? as usize;
    if last_seen > 20 {
        return Err(format!(
            "player_chat has {last_seen} last-seen signatures (max 20)"
        ));
    }
    for _ in 0..last_seen {
        if read_varint_req(raw, pos, "player_chat.last_seen.id")? == 0 {
            skip(raw, pos, 256, "player_chat.last_seen.signature")?;
        }
    }

    let unsigned = if read_bool(raw, pos)? {
        Some(read_component(raw, pos)?)
    } else {
        None
    };
    let filter = read_filter_mask(raw, pos)?;
    let bound = read_bound_chat_type(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "player_chat")?;

    let content = match filter {
        FilterMask::FullyFiltered => return Ok(()),
        FilterMask::PassThrough => unsigned.unwrap_or_else(|| Component::text(signed_content)),
        FilterMask::Partial(bits) => filtered_component(&signed_content, &bits),
    };
    send_chat(event_tx, &decorate(content, bound));
    Ok(())
}

fn send_chat(event_tx: &Sender<NetworkEvent>, component: &Component) {
    let spans = format_component_spans(component, [1.0; 4]);
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    tracing::info!("Chat: {text}");
    let _ = event_tx.try_send(NetworkEvent::ChatMessage { spans });
}

fn send_action_bar(event_tx: &Sender<NetworkEvent>, component: &Component) {
    let spans = format_component_spans(component, [1.0; 4]);
    let _ = event_tx.try_send(NetworkEvent::ActionBar { spans });
}

fn decorate(content: Component, bound: BoundChatType) -> Component {
    let mut args = Vec::with_capacity(bound.decoration.parameters.len());
    for parameter in &bound.decoration.parameters {
        let selected = match parameter {
            DecorationParameter::Sender => bound.name.clone(),
            DecorationParameter::Target => bound
                .target_name
                .clone()
                .unwrap_or_else(|| Component::text("")),
            DecorationParameter::Content => content.clone(),
        };
        args.push(Argument::Component(Box::new(selected)));
    }
    let mut result = Component::translate(bound.decoration.translation_key, args);
    result.style = bound.decoration.style;
    result
}

fn read_bound_chat_type(
    raw: &[u8],
    pos: &mut usize,
    chat_types: &ChatTypeRegistry,
) -> Result<BoundChatType, String> {
    let holder = read_varint_req(raw, pos, "chat_type.holder")?;
    let decoration = if holder == 0 {
        let chat = read_direct_decoration(raw, pos)?;
        // The holder also carries narration decoration. It is not currently
        // consumed by Pomme's narrator, but must be parsed to stay aligned.
        let _narration = read_direct_decoration(raw, pos)?;
        chat
    } else {
        registry_decoration(chat_types, holder - 1)?
    };
    let name = read_component(raw, pos)?;
    let target_name = if read_bool(raw, pos)? {
        Some(read_component(raw, pos)?)
    } else {
        None
    };
    Ok(BoundChatType {
        decoration,
        name,
        target_name,
    })
}

fn read_direct_decoration(raw: &[u8], pos: &mut usize) -> Result<ChatDecoration, String> {
    let translation_key = read_string(raw, pos, 32767, "chat_type.translation_key")?;
    let count = read_varint_req(raw, pos, "chat_type.parameters.count")? as usize;
    if count > 3 {
        return Err(format!(
            "chat type has {count} decoration parameters (max 3)"
        ));
    }
    let mut parameters = Vec::with_capacity(count);
    for _ in 0..count {
        parameters.push(match read_varint_req(raw, pos, "chat_type.parameter")? {
            0 => DecorationParameter::Sender,
            1 => DecorationParameter::Target,
            2 => DecorationParameter::Content,
            value => return Err(format!("unknown chat decoration parameter {value}")),
        });
    }
    let style_value = read_nbt_value(raw, pos)?;
    let style = if style_value.is_null() {
        Style::default()
    } else {
        Style::from_value(&style_value)
            .map_err(|e| format!("invalid chat decoration style: {e}"))?
    };
    Ok(ChatDecoration {
        translation_key,
        parameters,
        style,
    })
}

fn registry_decoration(
    chat_types: &ChatTypeRegistry,
    protocol_id: u32,
) -> Result<ChatDecoration, String> {
    let nbt = chat_types
        .entries
        .get(protocol_id as usize)
        .ok_or_else(|| format!("unknown chat_type registry id {protocol_id}"))?;
    let root = serde_json::to_value(NbtTag::Compound(nbt.clone()))
        .map_err(|e| format!("could not inspect chat_type registry value: {e}"))?;
    let chat = root
        .get("chat")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("chat_type registry id {protocol_id} has no chat decoration"))?;
    let translation_key = chat
        .get("translation_key")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("chat_type registry id {protocol_id} has no translation_key"))?
        .to_owned();
    let parameter_values = chat
        .get("parameters")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("chat_type registry id {protocol_id} has no parameters"))?;
    let mut parameters = Vec::with_capacity(parameter_values.len());
    for value in parameter_values {
        parameters.push(match value.as_str() {
            Some("sender") => DecorationParameter::Sender,
            Some("target") => DecorationParameter::Target,
            Some("content") => DecorationParameter::Content,
            other => return Err(format!("unknown chat decoration parameter {other:?}")),
        });
    }
    let style = match chat.get("style") {
        Some(value) => Style::from_value(value)
            .map_err(|e| format!("invalid chat_type registry style: {e}"))?,
        None => Style::default(),
    };
    Ok(ChatDecoration {
        translation_key,
        parameters,
        style,
    })
}

fn read_filter_mask(raw: &[u8], pos: &mut usize) -> Result<FilterMask, String> {
    match read_varint_req(raw, pos, "player_chat.filter_mask.type")? {
        0 => Ok(FilterMask::PassThrough),
        1 => Ok(FilterMask::FullyFiltered),
        2 => {
            let count = read_varint_req(raw, pos, "player_chat.filter_mask.longs")? as usize;
            if count > 64 {
                return Err(format!("player_chat filter mask contains {count} longs"));
            }
            let mut longs = Vec::with_capacity(count);
            for _ in 0..count {
                let bytes = take(raw, pos, 8, "player_chat.filter_mask.long")?;
                longs.push(u64::from_be_bytes(bytes.try_into().unwrap()));
            }
            Ok(FilterMask::Partial(longs))
        }
        value => Err(format!("unknown player_chat filter mask type {value}")),
    }
}

fn filtered_component(text: &str, bits: &[u64]) -> Component {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut root = Component::text("");
    let mut start = 0usize;
    while start < units.len() {
        let filtered = bit_is_set(bits, start);
        let mut end = start + 1;
        while end < units.len() && bit_is_set(bits, end) == filtered {
            end += 1;
        }
        if filtered {
            let mut part = Component::text("#".repeat(end - start));
            part.style.color = Some(0x555555);
            part.style.hover_event = Some(HoverEvent::Text(Box::new(Component::translate(
                "chat.filtered",
                Vec::new(),
            ))));
            root.siblings.push(part);
        } else {
            root.siblings.push(Component::text(String::from_utf16_lossy(
                &units[start..end],
            )));
        }
        start = end;
    }
    root
}

fn bit_is_set(bits: &[u64], index: usize) -> bool {
    bits.get(index / 64)
        .is_some_and(|word| word & (1u64 << (index % 64)) != 0)
}

fn read_component(raw: &[u8], pos: &mut usize) -> Result<Component, String> {
    let value = read_nbt_value(raw, pos)?;
    Component::from_value(&value).map_err(|e| format!("invalid text component: {e}"))
}

fn read_nbt_value(raw: &[u8], pos: &mut usize) -> Result<Value, String> {
    let slice = raw
        .get(*pos..)
        .ok_or_else(|| "NBT begins past end of packet".to_owned())?;
    let mut cursor = Cursor::new(slice);
    let tag =
        simdnbt::owned::read_tag(&mut cursor).map_err(|e| format!("invalid network NBT: {e:?}"))?;
    *pos += cursor.position() as usize;
    serde_json::to_value(tag).map_err(|e| format!("could not inspect network NBT: {e}"))
}

fn write_wire_string(out: &mut Vec<u8>, value: &str) {
    pomme_protocol::wire::write_varint(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

fn read_string(
    raw: &[u8],
    pos: &mut usize,
    max_chars: usize,
    field: &str,
) -> Result<String, String> {
    let len = read_varint_req(raw, pos, field)? as usize;
    // Vanilla's UTF-8 byte limit is at most max_chars * 3.
    if len > max_chars.saturating_mul(3) {
        return Err(format!("{field} is {len} bytes (max {})", max_chars * 3));
    }
    let bytes = take(raw, pos, len, field)?;
    let value = std::str::from_utf8(bytes).map_err(|e| format!("{field} is not UTF-8: {e}"))?;
    if value.chars().count() > max_chars {
        return Err(format!("{field} exceeds {max_chars} characters"));
    }
    Ok(value.to_owned())
}

fn read_bool(raw: &[u8], pos: &mut usize) -> Result<bool, String> {
    match *take(raw, pos, 1, "boolean")?.first().unwrap() {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(format!("invalid boolean byte {value}")),
    }
}

fn read_varint_req(raw: &[u8], pos: &mut usize, field: &str) -> Result<u32, String> {
    read_varint(raw, pos).ok_or_else(|| format!("truncated/invalid varint for {field}"))
}

fn read_varint(raw: &[u8], pos: &mut usize) -> Option<u32> {
    pomme_protocol::wire::read_varint(raw, pos)
}

fn skip(raw: &[u8], pos: &mut usize, len: usize, field: &str) -> Result<(), String> {
    take(raw, pos, len, field).map(|_| ())
}

fn take<'a>(raw: &'a [u8], pos: &mut usize, len: usize, field: &str) -> Result<&'a [u8], String> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| format!("{field} length overflow"))?;
    let value = raw
        .get(*pos..end)
        .ok_or_else(|| format!("truncated {field}"))?;
    *pos = end;
    Ok(value)
}

fn ensure_end(raw: &[u8], pos: usize, packet: &str) -> Result<(), String> {
    if pos == raw.len() {
        Ok(())
    } else {
        Err(format!("{packet} has {} trailing bytes", raw.len() - pos))
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::{NbtCompound, NbtList};

    use super::*;

    fn write_varint(out: &mut Vec<u8>, value: u32) {
        pomme_protocol::wire::write_varint(out, value);
    }

    fn write_component(out: &mut Vec<u8>, compound: NbtCompound) {
        NbtTag::Compound(compound).write(out);
    }

    fn write_string(out: &mut Vec<u8>, value: &str) {
        write_varint(out, value.len() as u32);
        out.extend_from_slice(value.as_bytes());
    }

    fn text_component(text: &str) -> NbtCompound {
        let mut component = NbtCompound::new();
        component.insert("text", text);
        component
    }

    fn test_chat_registries(template: &str) -> ChatTypeRegistry {
        let mut chat = NbtCompound::new();
        chat.insert("translation_key", template);
        chat.insert(
            "parameters",
            NbtTag::List(NbtList::from(vec![
                "sender".to_owned(),
                "content".to_owned(),
            ])),
        );
        let mut style = NbtCompound::new();
        style.insert("color", "gray");
        chat.insert("style", NbtTag::Compound(style));

        let mut entry = NbtCompound::new();
        entry.insert("chat", NbtTag::Compound(chat));
        ChatTypeRegistry::from_entries(vec![entry])
    }

    fn write_bound_chat_type(out: &mut Vec<u8>, name: &str) {
        // Holder reference id 0 is encoded as 1.
        write_varint(out, 1);
        write_component(out, text_component(name));
        out.push(0); // no target name
    }

    #[test]
    fn outbound_message_matches_latest_vanilla_layout() {
        let raw = encode_outbound_message("hello", 1234).unwrap();
        let mut pos = 0;
        let id = read_varint(&raw, &mut pos).unwrap();
        assert_eq!(
            id,
            PacketTable::latest()
                .id(Phase::Game, Direction::Serverbound, "chat")
                .unwrap()
        );
        assert_eq!(
            read_string(&raw, &mut pos, 256, "message").unwrap(),
            "hello"
        );
        assert_eq!(
            u64::from_be_bytes(
                take(&raw, &mut pos, 8, "timestamp")
                    .unwrap()
                    .try_into()
                    .unwrap()
            ),
            1234
        );
        assert_eq!(take(&raw, &mut pos, 8, "salt").unwrap(), &[0; 8]);
        assert!(!read_bool(&raw, &mut pos).unwrap());
        assert_eq!(read_varint(&raw, &mut pos), Some(0));
        assert_eq!(take(&raw, &mut pos, 3, "acknowledged").unwrap(), &[0; 3]);
        assert_eq!(take(&raw, &mut pos, 1, "checksum").unwrap(), &[0]);
        assert_eq!(pos, raw.len());
    }

    #[test]
    fn outbound_message_translates_cleanly_to_1_20_1_layout() {
        let raw = encode_outbound_message("hello 1.20.1", 1234).unwrap();
        let translated = super::super::translate::Translation::for_protocol(763)
            .unwrap()
            .translate_outbound_game_frame(raw);
        assert_eq!(translated.len(), 1);

        let old = &translated[0];
        let mut pos = 0;
        assert_eq!(
            read_varint(old, &mut pos),
            PacketTable::for_protocol(763)
                .unwrap()
                .id(Phase::Game, Direction::Serverbound, "chat")
        );
        assert_eq!(
            read_string(old, &mut pos, 256, "message").unwrap(),
            "hello 1.20.1"
        );
        assert_eq!(
            u64::from_be_bytes(
                take(old, &mut pos, 8, "timestamp")
                    .unwrap()
                    .try_into()
                    .unwrap()
            ),
            1234
        );
        assert_eq!(take(old, &mut pos, 8, "salt").unwrap(), &[0; 8]);
        assert!(!read_bool(old, &mut pos).unwrap());
        assert_eq!(read_varint(old, &mut pos), Some(0));
        assert_eq!(take(old, &mut pos, 3, "acknowledged").unwrap(), &[0; 3]);
        // 1.21.4 and older have no trailing last-seen checksum byte.
        assert_eq!(pos, old.len());
    }

    #[test]
    fn outbound_command_uses_raw_latest_frame_and_old_version_translator() {
        let raw = encode_outbound_command("say hi").unwrap();
        let mut pos = 0;
        assert_eq!(
            read_varint(&raw, &mut pos),
            PacketTable::latest().id(Phase::Game, Direction::Serverbound, "chat_command")
        );
        assert_eq!(
            read_string(&raw, &mut pos, 32767, "command").unwrap(),
            "say hi"
        );
        assert_eq!(pos, raw.len());

        let translated = super::super::translate::Translation::for_protocol(765)
            .unwrap()
            .translate_outbound_game_frame(raw);
        assert_eq!(translated.len(), 1);
        assert!(translated[0].len() > pos + 20);
    }

    #[test]
    fn system_chat_round_trips_every_supported_protocol() {
        let mut protocols = pomme_protocol::version::VERSIONS
            .iter()
            .map(|version| version.protocol)
            .collect::<Vec<_>>();
        protocols.sort_unstable();
        protocols.dedup();

        for protocol in protocols {
            let table = PacketTable::for_protocol(protocol).unwrap_or_else(PacketTable::latest);
            let id = table
                .id(Phase::Game, Direction::Clientbound, "system_chat")
                .unwrap();
            let mut wire = Vec::new();
            write_varint(&mut wire, id);
            if protocol <= 764 {
                write_string(
                    &mut wire,
                    &serde_json::json!({
                        "text": format!("hello-{protocol}"),
                        "color": "aqua"
                    })
                    .to_string(),
                );
            } else {
                let mut component = text_component(&format!("hello-{protocol}"));
                component.insert("color", "aqua");
                write_component(&mut wire, component);
            }
            wire.push(0);

            let latest = if protocol == pomme_protocol::version::LATEST.protocol {
                wire.into_boxed_slice()
            } else {
                super::super::translate::Translation::for_protocol(protocol)
                    .unwrap()
                    .translate_game_frame(wire.into_boxed_slice())
                    .unwrap_or_else(|| {
                        panic!("system chat did not translate from protocol {protocol}")
                    })
            };
            let (tx, rx) = crossbeam_channel::bounded(1);
            handle_raw_chat_packet(&latest, &tx, &ChatTypeRegistry::default())
                .unwrap()
                .unwrap_or_else(|e| {
                    panic!("native chat decode failed for protocol {protocol}: {e}")
                });
            let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
                panic!("expected chat event for protocol {protocol}");
            };
            assert_eq!(
                spans.iter().map(|s| s.text.as_str()).collect::<String>(),
                format!("hello-{protocol}")
            );
            assert_eq!(
                spans[0].component_style.as_ref().unwrap().color,
                Some(0x55ffff)
            );
        }
    }

    #[test]
    fn outbound_message_translates_every_supported_protocol() {
        let mut protocols = pomme_protocol::version::VERSIONS
            .iter()
            .map(|version| version.protocol)
            .collect::<Vec<_>>();
        protocols.sort_unstable();
        protocols.dedup();

        for protocol in protocols {
            let latest = encode_outbound_message("cross-version", 99).unwrap();
            let frames = if protocol == pomme_protocol::version::LATEST.protocol {
                vec![latest]
            } else {
                super::super::translate::Translation::for_protocol(protocol)
                    .unwrap()
                    .translate_outbound_game_frame(latest)
            };
            assert_eq!(frames.len(), 1, "protocol {protocol}");
            let frame = &frames[0];
            let table = PacketTable::for_protocol(protocol).unwrap_or_else(PacketTable::latest);
            let mut pos = 0;
            assert_eq!(
                read_varint(frame, &mut pos),
                table.id(Phase::Game, Direction::Serverbound, "chat"),
                "protocol {protocol}"
            );
            assert_eq!(
                read_string(frame, &mut pos, 256, "message").unwrap(),
                "cross-version",
                "protocol {protocol}"
            );
            skip(frame, &mut pos, 16, "timestamp/salt").unwrap();
            assert!(!read_bool(frame, &mut pos).unwrap(), "protocol {protocol}");
            assert_eq!(read_varint(frame, &mut pos), Some(0), "protocol {protocol}");
            skip(frame, &mut pos, 3, "acknowledged").unwrap();
            if protocol >= 770 {
                skip(frame, &mut pos, 1, "checksum").unwrap();
            }
            assert_eq!(pos, frame.len(), "protocol {protocol}");
        }
    }

    #[test]
    fn outbound_message_rejects_vanilla_length_overflow() {
        assert!(encode_outbound_message(&"x".repeat(257), 0).is_err());
    }

    #[test]
    fn system_chat_from_1_20_1_translates_into_native_component_path() {
        let old_id = PacketTable::for_protocol(763)
            .unwrap()
            .id(Phase::Game, Direction::Clientbound, "system_chat")
            .unwrap();
        let json = serde_json::json!({
            "text": "Old server",
            "color": "gold",
            "hoverEvent": {
                "action": "show_text",
                "contents": {"text": "1.20.1 tooltip"}
            }
        })
        .to_string();
        let mut old = Vec::new();
        write_varint(&mut old, old_id);
        write_string(&mut old, &json);
        old.push(0); // not overlay

        let latest = super::super::translate::Translation::for_protocol(763)
            .unwrap()
            .translate_game_frame(old.into_boxed_slice())
            .expect("1.20.1 system chat should translate");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&latest, &tx, &ChatTypeRegistry::default())
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
            panic!("expected chat event");
        };
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "Old server"
        );
        let style = spans[0].component_style.as_ref().unwrap();
        assert_eq!(style.color, Some(0xffaa00));
        assert!(matches!(style.hover_event, Some(HoverEvent::Text(_))));
    }

    #[test]
    fn disguised_chat_from_1_20_1_translates_chat_type_and_components() {
        let old_id = PacketTable::for_protocol(763)
            .unwrap()
            .id(Phase::Game, Direction::Clientbound, "disguised_chat")
            .unwrap();
        let content = serde_json::json!({
            "text": "Legacy hello",
            "clickEvent": {"action": "copy_to_clipboard", "value": "legacy"}
        })
        .to_string();
        let sender = serde_json::json!({"text": "Alice"}).to_string();

        let mut old = Vec::new();
        write_varint(&mut old, old_id);
        write_string(&mut old, &content);
        write_varint(&mut old, 0); // direct chat_type registry id before holders
        write_string(&mut old, &sender);
        old.push(0); // no target name

        let latest = super::super::translate::Translation::for_protocol(763)
            .unwrap()
            .translate_game_frame(old.into_boxed_slice())
            .expect("1.20.1 disguised chat should translate");
        let registries = test_chat_registries("<%s> %s");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&latest, &tx, &registries)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
            panic!("expected chat event");
        };
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "<Alice> Legacy hello"
        );
        let content_span = spans.iter().find(|s| s.text == "Legacy hello").unwrap();
        assert!(matches!(
            content_span.component_style.as_ref().unwrap().click_event,
            Some(crate::chat_component::ClickEvent::CopyToClipboard(ref value)) if value == "legacy"
        ));
    }

    #[test]
    fn system_chat_decodes_hover_before_azalea() {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "system_chat")
            .unwrap();
        let mut root = NbtCompound::new();
        root.insert("text", "Click me");
        let mut hover_text = NbtCompound::new();
        hover_text.insert("text", "Tooltip");
        let mut hover = NbtCompound::new();
        hover.insert("action", "show_text");
        hover.insert("value", NbtTag::Compound(hover_text));
        root.insert("hover_event", NbtTag::Compound(hover));

        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_component(&mut raw, root);
        raw.push(0);

        let (tx, rx) = crossbeam_channel::bounded(1);
        let result = handle_raw_chat_packet(&raw, &tx, &ChatTypeRegistry::default()).unwrap();
        result.unwrap();
        let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
            panic!("expected chat event");
        };
        let style = spans[0].component_style.as_ref().unwrap();
        assert!(matches!(style.hover_event, Some(HoverEvent::Text(_))));
    }

    #[test]
    fn disguised_chat_uses_registry_decoration_without_azalea_component_decode() {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "disguised_chat")
            .unwrap();
        let mut content = text_component("Hello");
        let mut click = NbtCompound::new();
        click.insert("action", "copy_to_clipboard");
        click.insert("value", "hello");
        content.insert("click_event", NbtTag::Compound(click));

        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_component(&mut raw, content);
        write_bound_chat_type(&mut raw, "Alice");

        let registries = test_chat_registries("<%s> %s");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&raw, &tx, &registries)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
            panic!("expected chat event");
        };
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "<Alice> Hello"
        );
        let content_span = spans.iter().find(|s| s.text == "Hello").unwrap();
        assert!(matches!(
            content_span.component_style.as_ref().unwrap().click_event,
            Some(crate::chat_component::ClickEvent::CopyToClipboard(ref value)) if value == "hello"
        ));
        assert_eq!(
            content_span.component_style.as_ref().unwrap().color,
            Some(0xaaaaaa)
        );
    }

    #[test]
    fn player_chat_packet_layout_preserves_unsigned_component_interactions() {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "player_chat")
            .unwrap();
        let mut unsigned = text_component("Decorated");
        let mut hover_text = NbtCompound::new();
        hover_text.insert("text", "Unsigned tooltip");
        let mut hover = NbtCompound::new();
        hover.insert("action", "show_text");
        hover.insert("value", NbtTag::Compound(hover_text));
        unsigned.insert("hover_event", NbtTag::Compound(hover));

        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, 0); // global index
        raw.extend_from_slice(&[0; 16]); // sender UUID
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_string(&mut raw, "signed body");
        raw.extend_from_slice(&0u64.to_be_bytes()); // timestamp
        raw.extend_from_slice(&0u64.to_be_bytes()); // salt
        write_varint(&mut raw, 0); // last-seen signatures
        raw.push(1); // unsigned content present
        write_component(&mut raw, unsigned);
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");

        let registries = test_chat_registries("<%s> %s");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&raw, &tx, &registries)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans } = rx.recv().unwrap() else {
            panic!("expected chat event");
        };
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "<Alice> Decorated"
        );
        let content_span = spans.iter().find(|s| s.text == "Decorated").unwrap();
        assert!(matches!(
            content_span.component_style.as_ref().unwrap().hover_event,
            Some(HoverEvent::Text(_))
        ));
    }

    #[test]
    fn partial_filter_builds_dark_gray_hoverable_hashes() {
        let component = filtered_component("abcdef", &[0b001100]);
        let spans = format_component_spans(&component, [1.0; 4]);
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "ab##ef"
        );
        let filtered = spans.iter().find(|s| s.text == "##").unwrap();
        assert_eq!(
            filtered.component_style.as_ref().unwrap().color,
            Some(0x555555)
        );
        assert!(matches!(
            filtered.component_style.as_ref().unwrap().hover_event,
            Some(HoverEvent::Text(_))
        ));
    }
}
