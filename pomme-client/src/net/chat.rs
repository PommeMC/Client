//! Pomme-owned inbound game-chat decoding.
//!
//! Game frames reach this module after protocol-version translation but before
//! Azalea's typed packet decoder. That ordering is intentional: Azalea's 26.2
//! text-component decoder drops hover-event data, so chat components must be
//! decoded here if Pomme is going to support Vanilla interaction semantics.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::io::Cursor;

use crossbeam_channel::Sender;
use pomme_protocol::{Direction, PacketTable, Phase};
use serde_json::Value;
use simdnbt::owned::{NbtCompound, NbtTag};

use super::NetworkEvent;
use crate::chat_component::{Argument, Component, HoverEvent, Style};
use crate::ui::chat::{ChatMessageSource, ChatMessageTag};
use crate::ui::text::format_component_spans;

#[derive(Clone, Debug)]
pub struct ChatTypeRegistry {
    entries: Vec<NbtCompound>,
    signature_cache: RefCell<Vec<Option<[u8; 256]>>>,
}

impl Default for ChatTypeRegistry {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            signature_cache: RefCell::new(vec![None; 128]),
        }
    }
}

impl ChatTypeRegistry {
    pub fn from_entries(entries: Vec<NbtCompound>) -> Self {
        Self {
            entries,
            ..Self::default()
        }
    }

    fn unpack_signature(&self, id: usize) -> Option<[u8; 256]> {
        self.signature_cache.borrow().get(id).copied().flatten()
    }

    fn push_signatures(&self, last_seen: Vec<[u8; 256]>, signature: Option<[u8; 256]>) {
        let mut queue: VecDeque<[u8; 256]> = last_seen.into_iter().collect();
        if let Some(signature) = signature {
            queue.push_back(signature);
        }
        let new_entries: HashSet<[u8; 256]> = queue.iter().copied().collect();
        let mut cache = self.signature_cache.borrow_mut();
        for slot in cache.iter_mut() {
            let Some(next) = queue.pop_back() else {
                break;
            };
            let previous = slot.replace(next);
            if let Some(previous) = previous
                && !new_entries.contains(&previous)
            {
                queue.push_front(previous);
            }
        }
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
#[cfg(test)]
pub fn encode_outbound_message(message: &str, timestamp_millis: u64) -> Result<Vec<u8>, String> {
    if java_utf16_len(message) > 256 {
        return Err("chat message exceeds 256 UTF-16 code units".into());
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

pub fn encode_outbound_unsigned_message(
    message: &str,
    timestamp_millis: u64,
    salt: i64,
    update: &crate::net::chat_security::LastSeenUpdate,
) -> Result<Vec<u8>, String> {
    if java_utf16_len(message) > 256 {
        return Err("chat message exceeds 256 UTF-16 code units".into());
    }
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat")
        .ok_or_else(|| "latest protocol has no serverbound chat packet".to_owned())?;
    let mut out = Vec::with_capacity(message.len() + 32);
    pomme_protocol::wire::write_varint(&mut out, id);
    write_wire_string(&mut out, message);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&salt.to_be_bytes());
    out.push(0); // no signature
    write_last_seen_update(&mut out, update);
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

pub fn encode_outbound_signed_message(
    message: &str,
    timestamp_millis: u64,
    salt: i64,
    signature: [u8; 256],
    update: &crate::net::chat_security::LastSeenUpdate,
) -> Result<Vec<u8>, String> {
    if java_utf16_len(message) > 256 {
        return Err("chat message exceeds 256 UTF-16 code units".into());
    }
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat")
        .ok_or_else(|| "latest protocol has no serverbound chat packet".to_owned())?;
    let mut out = Vec::with_capacity(message.len() + 300);
    pomme_protocol::wire::write_varint(&mut out, id);
    write_wire_string(&mut out, message);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&salt.to_be_bytes());
    out.push(1);
    out.extend_from_slice(&signature);
    write_last_seen_update(&mut out, update);
    Ok(out)
}

pub fn encode_outbound_signed_command(
    command: &str,
    timestamp_millis: u64,
    salt: i64,
    signatures: &[(String, [u8; 256])],
    update: &crate::net::chat_security::LastSeenUpdate,
) -> Result<Vec<u8>, String> {
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat_command_signed")
        .ok_or_else(|| {
            "latest protocol has no serverbound chat_command_signed packet".to_owned()
        })?;
    let mut out = Vec::with_capacity(command.len() + signatures.len() * 280 + 32);
    pomme_protocol::wire::write_varint(&mut out, id);
    write_wire_string(&mut out, command);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&salt.to_be_bytes());
    pomme_protocol::wire::write_varint(&mut out, signatures.len() as u32);
    for (name, signature) in signatures {
        write_wire_string(&mut out, name);
        out.extend_from_slice(signature);
    }
    write_last_seen_update(&mut out, update);
    Ok(out)
}

pub fn encode_chat_session_update(
    session: &crate::net::chat_security::LocalChatSession,
) -> Result<Vec<u8>, String> {
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat_session_update")
        .ok_or_else(|| "latest protocol has no chat_session_update packet".to_owned())?;
    let mut out =
        Vec::with_capacity(session.public_key_der.len() + session.key_signature.len() + 40);
    pomme_protocol::wire::write_varint(&mut out, id);
    out.extend_from_slice(session.session_id.as_bytes());
    out.extend_from_slice(&session.expires_at_ms.to_be_bytes());
    pomme_protocol::wire::write_varint(&mut out, session.public_key_der.len() as u32);
    out.extend_from_slice(&session.public_key_der);
    pomme_protocol::wire::write_varint(&mut out, session.key_signature.len() as u32);
    out.extend_from_slice(&session.key_signature);
    Ok(out)
}

pub fn encode_chat_ack(offset: u32) -> Result<Vec<u8>, String> {
    let id = PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, "chat_ack")
        .ok_or_else(|| "latest protocol has no chat_ack packet".to_owned())?;
    let mut out = Vec::with_capacity(6);
    pomme_protocol::wire::write_varint(&mut out, id);
    pomme_protocol::wire::write_varint(&mut out, offset);
    Ok(out)
}

fn write_last_seen_update(out: &mut Vec<u8>, update: &crate::net::chat_security::LastSeenUpdate) {
    pomme_protocol::wire::write_varint(out, update.offset);
    out.extend_from_slice(&update.acknowledged);
    out.push(update.checksum);
}

pub fn handle_raw_chat_packet(
    raw: &[u8],
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
    expected_global_index: Option<&mut u32>,
) -> Option<Result<(), String>> {
    let mut pos = 0usize;
    let packet_id = read_varint(raw, &mut pos)?;
    let name = PacketTable::latest().name_of(Phase::Game, Direction::Clientbound, packet_id)?;

    let result = match name {
        "system_chat" => parse_system_chat(raw, &mut pos, event_tx),
        "set_action_bar_text" => parse_action_bar(raw, &mut pos, event_tx),
        "disguised_chat" => parse_disguised_chat(raw, &mut pos, event_tx, chat_types),
        "player_chat" => {
            parse_player_chat(raw, &mut pos, event_tx, chat_types, expected_global_index)
        }
        "delete_chat" => parse_delete_chat(raw, &mut pos, event_tx, chat_types),
        "command_suggestions" => parse_command_suggestions(raw, &mut pos, event_tx),
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
        send_chat(
            event_tx,
            ChatDelivery {
                component: &component,
                secure_component: None,
                missing_profile_component: None,
                signature: None,
                sender_uuid: None,
                signed_body: None,
                source: ChatMessageSource::SystemServer,
                tag: Some(ChatMessageTag::SystemSinglePlayer),
            },
        );
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
    let decorated = decorate(content, bound);
    send_chat(
        event_tx,
        ChatDelivery {
            component: &decorated,
            secure_component: None,
            missing_profile_component: None,
            signature: None,
            sender_uuid: None,
            signed_body: None,
            source: ChatMessageSource::Player,
            tag: Some(ChatMessageTag::System),
        },
    );
    Ok(())
}

fn parse_player_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
    expected_global_index: Option<&mut u32>,
) -> Result<(), String> {
    // globalIndex, sender UUID, message index
    let global_index = read_varint_req(raw, pos, "player_chat.global_index")?;
    let sender_uuid = uuid::Uuid::from_bytes(
        take(raw, pos, 16, "player_chat.sender")?
            .try_into()
            .unwrap(),
    );
    let message_index = read_varint_req(raw, pos, "player_chat.index")? as i32;

    // Nullable 256-byte message signature.
    let signature = if read_bool(raw, pos)? {
        Some(read_full_signature(raw, pos, "player_chat.signature")?)
    } else {
        None
    };

    let signed_content = read_string(raw, pos, 256, "player_chat.body.content")?;
    let timestamp_bytes = take(raw, pos, 8, "player_chat.timestamp")?;
    let timestamp_ms = i64::from_be_bytes(timestamp_bytes.try_into().unwrap());
    let salt = i64::from_be_bytes(take(raw, pos, 8, "player_chat.salt")?.try_into().unwrap());

    // LastSeenMessages.Packed: max 20 packed signatures. Packed id 0 carries
    // the full 256-byte signature; positive values are cache index + 1.
    let last_seen_count = read_varint_req(raw, pos, "player_chat.last_seen.count")? as usize;
    if last_seen_count > 20 {
        return Err(format!(
            "player_chat has {last_seen_count} last-seen signatures (max 20)"
        ));
    }
    let mut last_seen = Vec::with_capacity(last_seen_count);
    for _ in 0..last_seen_count {
        last_seen.push(read_packed_signature(raw, pos, chat_types)?);
    }

    let unsigned = if read_bool(raw, pos)? {
        Some(read_component(raw, pos)?)
    } else {
        None
    };
    let filter = read_filter_mask(raw, pos)?;
    let bound = read_bound_chat_type(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "player_chat")?;

    if let Some(expected) = expected_global_index {
        if global_index != *expected {
            return Err(format!(
                "out-of-order player chat: expected global index {}, got {global_index}",
                *expected
            ));
        }
        *expected = expected.wrapping_add(1);
    }

    let mut validation_error = Component::translate("chat.validation_error", Vec::new());
    validation_error.style.color = Some(0xff5555);
    validation_error.style.italic = Some(true);
    let missing_profile = decorate(validation_error, bound.clone());

    let trust_content = unsigned
        .clone()
        .unwrap_or_else(|| Component::text(signed_content.clone()));
    let decorated_for_trust = decorate(trust_content.clone(), bound.clone());
    let unsigned_modified_style = unsigned
        .as_ref()
        .is_some_and(component_has_non_default_font);
    let modified =
        !decorated_for_trust.plain_text().contains(&signed_content) || unsigned_modified_style;
    let fully_filtered = matches!(filter, FilterMask::FullyFiltered);
    let signed_body = crate::net::chat_security::SignedChatBody {
        content: signed_content.clone(),
        timestamp_ms,
        salt,
        last_seen: last_seen.clone(),
        message_index,
        modified,
        modified_when_unsigned_hidden: unsigned_modified_style,
        fully_filtered,
    };

    let content = match &filter {
        FilterMask::FullyFiltered | FilterMask::PassThrough => trust_content,
        FilterMask::Partial(bits) => filtered_component(&signed_content, bits),
    };
    let secure_content = match &filter {
        FilterMask::FullyFiltered | FilterMask::PassThrough => {
            Component::text(signed_content.clone())
        }
        FilterMask::Partial(bits) => filtered_component(&signed_content, bits),
    };
    let secure_decorated = decorate(secure_content, bound.clone());
    let decorated = decorate(content, bound);
    chat_types.push_signatures(last_seen, signature);
    send_chat(
        event_tx,
        ChatDelivery {
            component: &decorated,
            secure_component: Some(&secure_decorated),
            missing_profile_component: Some(&missing_profile),
            signature,
            sender_uuid: Some(sender_uuid),
            signed_body: Some(signed_body),
            source: ChatMessageSource::Player,
            tag: None,
        },
    );
    Ok(())
}

fn parse_delete_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
) -> Result<(), String> {
    let signature = read_packed_signature(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "delete_chat")?;
    let _ = event_tx.try_send(NetworkEvent::DeleteChatMessage { signature });
    Ok(())
}

fn parse_command_suggestions(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), String> {
    let id = read_varint_req(raw, pos, "command_suggestions.id")?;
    let start = read_varint_req(raw, pos, "command_suggestions.start")? as usize;
    let _length = read_varint_req(raw, pos, "command_suggestions.length")?;
    let count = read_varint_req(raw, pos, "command_suggestions.count")? as usize;
    if count > 10_000 {
        return Err(format!(
            "command_suggestions has unreasonable count {count}"
        ));
    }
    let mut options = Vec::with_capacity(count);
    for _ in 0..count {
        let text = read_string(raw, pos, 32_767, "command_suggestions.text")?;
        let tooltip = if read_bool(raw, pos)? {
            Some(read_component(raw, pos)?)
        } else {
            None
        };
        options.push(crate::ui::chat::ChatSuggestion { text, tooltip });
    }
    ensure_end(raw, *pos, "command_suggestions")?;
    let _ = event_tx.try_send(NetworkEvent::CommandSuggestions { id, start, options });
    Ok(())
}

struct ChatDelivery<'a> {
    component: &'a Component,
    secure_component: Option<&'a Component>,
    missing_profile_component: Option<&'a Component>,
    signature: Option<[u8; 256]>,
    sender_uuid: Option<uuid::Uuid>,
    signed_body: Option<crate::net::chat_security::SignedChatBody>,
    source: ChatMessageSource,
    tag: Option<ChatMessageTag>,
}

fn send_chat(event_tx: &Sender<NetworkEvent>, delivery: ChatDelivery<'_>) {
    let spans = format_component_spans(delivery.component, [1.0; 4]);
    let secure_spans = delivery
        .secure_component
        .map(|component| format_component_spans(component, [1.0; 4]));
    let missing_profile_spans = delivery
        .missing_profile_component
        .map(|component| format_component_spans(component, [1.0; 4]));
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    tracing::info!("Chat: {text}");
    let _ = event_tx.try_send(NetworkEvent::ChatMessage {
        spans,
        secure_spans,
        missing_profile_spans,
        signature: delivery.signature,
        sender_uuid: delivery.sender_uuid,
        signed_body: delivery.signed_body,
        source: delivery.source,
        tag: delivery.tag,
    });
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

fn read_full_signature(raw: &[u8], pos: &mut usize, field: &str) -> Result<[u8; 256], String> {
    let bytes = take(raw, pos, 256, field)?;
    Ok(bytes.try_into().unwrap())
}

fn read_packed_signature(
    raw: &[u8],
    pos: &mut usize,
    chat_types: &ChatTypeRegistry,
) -> Result<[u8; 256], String> {
    let encoded = read_varint_req(raw, pos, "packed_message_signature.id")?;
    if encoded == 0 {
        read_full_signature(raw, pos, "packed_message_signature.full")
    } else {
        let id = encoded as usize - 1;
        chat_types
            .unpack_signature(id)
            .ok_or_else(|| format!("unknown packed message signature cache id {id}"))
    }
}

fn component_has_non_default_font(component: &Component) -> bool {
    let mut modified = false;
    component.visit_text(
        &crate::chat_component::ResolvedStyle::default(),
        &mut |_, style| {
            let non_default = style.font.as_ref().is_some_and(|font| match font {
                Value::String(id) => id != "minecraft:default" && id != "default",
                Value::Object(map) => map
                    .get("id")
                    .or_else(|| map.get("font"))
                    .and_then(Value::as_str)
                    .is_none_or(|id| id != "minecraft:default" && id != "default"),
                _ => true,
            });
            modified |= non_default;
        },
    );
    modified
}

fn read_component(raw: &[u8], pos: &mut usize) -> Result<Component, String> {
    let tag = read_nbt_tag(raw, pos)?;
    Component::from_nbt_tag(&tag).map_err(|e| format!("invalid text component: {e}"))
}

fn read_nbt_tag(raw: &[u8], pos: &mut usize) -> Result<NbtTag, String> {
    let slice = raw
        .get(*pos..)
        .ok_or_else(|| "NBT begins past end of packet".to_owned())?;
    let mut cursor = Cursor::new(slice);
    let tag =
        simdnbt::owned::read_tag(&mut cursor).map_err(|e| format!("invalid network NBT: {e:?}"))?;
    *pos += cursor.position() as usize;
    Ok(tag)
}

fn read_nbt_value(raw: &[u8], pos: &mut usize) -> Result<Value, String> {
    let tag = read_nbt_tag(raw, pos)?;
    serde_json::to_value(tag).map_err(|e| format!("could not inspect network NBT: {e}"))
}

fn write_wire_string(out: &mut Vec<u8>, value: &str) {
    pomme_protocol::wire::write_varint(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

fn java_utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
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
    if java_utf16_len(value) > max_chars {
        return Err(format!("{field} exceeds {max_chars} UTF-16 code units"));
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

#[cfg(test)]
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

    fn player_chat_packet(global_index: u32) -> Vec<u8> {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "player_chat")
            .unwrap();
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, global_index);
        raw.extend_from_slice(&[0; 16]);
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_string(&mut raw, "hello");
        raw.extend_from_slice(&0u64.to_be_bytes());
        raw.extend_from_slice(&0u64.to_be_bytes());
        write_varint(&mut raw, 0); // last seen
        raw.push(0); // no unsigned content
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");
        raw
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
    fn signed_chat_encodes_signature_and_vanilla_last_seen_update() {
        let update = crate::net::chat_security::LastSeenUpdate {
            offset: 5,
            acknowledged: [0x01, 0x80, 0x04],
            checksum: 0x7f,
            last_seen: vec![[9; 256]],
        };
        let signature = [0x5a; 256];
        let raw = encode_outbound_signed_message("signed", 1234, -55, signature, &update).unwrap();
        let mut pos = 0;
        assert_eq!(
            read_varint(&raw, &mut pos),
            PacketTable::latest().id(Phase::Game, Direction::Serverbound, "chat")
        );
        assert_eq!(
            read_string(&raw, &mut pos, 256, "message").unwrap(),
            "signed"
        );
        assert_eq!(
            take(&raw, &mut pos, 8, "timestamp").unwrap(),
            &1234u64.to_be_bytes()
        );
        assert_eq!(
            take(&raw, &mut pos, 8, "salt").unwrap(),
            &(-55i64).to_be_bytes()
        );
        assert!(read_bool(&raw, &mut pos).unwrap());
        assert_eq!(take(&raw, &mut pos, 256, "signature").unwrap(), &signature);
        assert_eq!(read_varint(&raw, &mut pos), Some(5));
        assert_eq!(
            take(&raw, &mut pos, 3, "acknowledged").unwrap(),
            &[0x01, 0x80, 0x04]
        );
        assert_eq!(take(&raw, &mut pos, 1, "checksum").unwrap(), &[0x7f]);
        assert_eq!(pos, raw.len());
    }

    #[test]
    fn signed_chat_strips_checksum_before_1_21_5() {
        let update = crate::net::chat_security::LastSeenUpdate {
            offset: 2,
            acknowledged: [1, 0, 0],
            checksum: 77,
            last_seen: vec![[1; 256]],
        };
        let latest = encode_outbound_signed_message("old signed", 9, 3, [4; 256], &update).unwrap();
        for protocol in [763, 765, 766, 769] {
            let frame = super::super::translate::Translation::for_protocol(protocol)
                .unwrap()
                .translate_outbound_game_frame(latest.clone())
                .into_iter()
                .next()
                .unwrap();
            let mut pos = 0;
            assert_eq!(
                read_varint(&frame, &mut pos),
                PacketTable::for_protocol(protocol).unwrap().id(
                    Phase::Game,
                    Direction::Serverbound,
                    "chat"
                ),
                "protocol {protocol}"
            );
            assert_eq!(
                read_string(&frame, &mut pos, 256, "message").unwrap(),
                "old signed"
            );
            skip(&frame, &mut pos, 16, "timestamp/salt").unwrap();
            assert!(read_bool(&frame, &mut pos).unwrap());
            skip(&frame, &mut pos, 256, "signature").unwrap();
            assert_eq!(read_varint(&frame, &mut pos), Some(2));
            skip(&frame, &mut pos, 3, "acknowledged").unwrap();
            assert_eq!(
                pos,
                frame.len(),
                "protocol {protocol} must not retain checksum"
            );
        }
    }

    #[test]
    fn signed_command_translates_across_packet_split_and_checksum_change() {
        let update = crate::net::chat_security::LastSeenUpdate {
            offset: 3,
            acknowledged: [0x11, 0x22, 0x03],
            checksum: 0x5c,
            last_seen: vec![[7; 256]],
        };
        let latest = encode_outbound_signed_command(
            "msg Steve hello there",
            55,
            -8,
            &[("message".into(), [0xa5; 256])],
            &update,
        )
        .unwrap();

        for protocol in [763, 765, 766, 769, 770, 776] {
            let frames = if protocol == pomme_protocol::version::LATEST.protocol {
                vec![latest.clone()]
            } else {
                super::super::translate::Translation::for_protocol(protocol)
                    .unwrap()
                    .translate_outbound_game_frame(latest.clone())
            };
            assert_eq!(frames.len(), 1, "protocol {protocol}");
            let frame = &frames[0];
            let table = PacketTable::for_protocol(protocol).unwrap();
            let expected_name = if protocol <= 765 {
                "chat_command"
            } else {
                "chat_command_signed"
            };
            let mut pos = 0;
            assert_eq!(
                read_varint(frame, &mut pos),
                table.id(Phase::Game, Direction::Serverbound, expected_name),
                "protocol {protocol}"
            );
            assert_eq!(
                read_string(frame, &mut pos, 32767, "command").unwrap(),
                "msg Steve hello there"
            );
            assert_eq!(
                take(frame, &mut pos, 8, "timestamp").unwrap(),
                &55u64.to_be_bytes()
            );
            assert_eq!(
                take(frame, &mut pos, 8, "salt").unwrap(),
                &(-8i64).to_be_bytes()
            );
            assert_eq!(read_varint(frame, &mut pos), Some(1));
            assert_eq!(
                read_string(frame, &mut pos, 16, "argument name").unwrap(),
                "message"
            );
            assert_eq!(
                take(frame, &mut pos, 256, "argument signature").unwrap(),
                &[0xa5; 256]
            );
            assert_eq!(read_varint(frame, &mut pos), Some(3));
            assert_eq!(
                take(frame, &mut pos, 3, "acknowledged").unwrap(),
                &[0x11, 0x22, 0x03]
            );
            if protocol >= 770 {
                assert_eq!(take(frame, &mut pos, 1, "checksum").unwrap(), &[0x5c]);
            }
            assert_eq!(pos, frame.len(), "protocol {protocol}");
        }
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
            handle_raw_chat_packet(&latest, &tx, &ChatTypeRegistry::default(), None)
                .unwrap()
                .unwrap_or_else(|e| {
                    panic!("native chat decode failed for protocol {protocol}: {e}")
                });
            let NetworkEvent::ChatMessage { spans, .. } = rx.recv().unwrap() else {
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
    fn outbound_message_rejects_vanilla_utf16_length_overflow() {
        assert!(encode_outbound_message(&"x".repeat(256), 0).is_ok());
        assert!(encode_outbound_message(&"x".repeat(257), 0).is_err());
        assert!(encode_outbound_message(&"😀".repeat(128), 0).is_ok());
        assert!(encode_outbound_message(&"😀".repeat(129), 0).is_err());
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
        handle_raw_chat_packet(&latest, &tx, &ChatTypeRegistry::default(), None)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans, .. } = rx.recv().unwrap() else {
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
        handle_raw_chat_packet(&latest, &tx, &registries, None)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans, .. } = rx.recv().unwrap() else {
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
        let result = handle_raw_chat_packet(&raw, &tx, &ChatTypeRegistry::default(), None).unwrap();
        result.unwrap();
        let NetworkEvent::ChatMessage { spans, .. } = rx.recv().unwrap() else {
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
        handle_raw_chat_packet(&raw, &tx, &registries, None)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage { spans, .. } = rx.recv().unwrap() else {
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
    fn player_chat_sequence_advances_only_after_complete_decode() {
        let registries = test_chat_registries("<%s> %s");
        let (tx, _rx) = crossbeam_channel::bounded(8);
        let mut expected = 0u32;

        handle_raw_chat_packet(
            &player_chat_packet(0),
            &tx,
            &registries,
            Some(&mut expected),
        )
        .unwrap()
        .unwrap();
        assert_eq!(expected, 1);

        let mut malformed = player_chat_packet(1);
        malformed.pop();
        assert!(
            handle_raw_chat_packet(&malformed, &tx, &registries, Some(&mut expected))
                .unwrap()
                .is_err()
        );
        assert_eq!(expected, 1, "malformed chat must not consume the index");

        handle_raw_chat_packet(
            &player_chat_packet(1),
            &tx,
            &registries,
            Some(&mut expected),
        )
        .unwrap()
        .unwrap();
        assert_eq!(expected, 2);

        assert!(
            handle_raw_chat_packet(
                &player_chat_packet(3),
                &tx,
                &registries,
                Some(&mut expected),
            )
            .unwrap()
            .is_err()
        );
        assert_eq!(expected, 2, "out-of-order chat must not advance state");

        expected = u32::MAX;
        handle_raw_chat_packet(
            &player_chat_packet(u32::MAX),
            &tx,
            &registries,
            Some(&mut expected),
        )
        .unwrap()
        .unwrap();
        assert_eq!(expected, 0, "Java int sequence wraps at 32 bits");
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
        handle_raw_chat_packet(&raw, &tx, &registries, None)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage {
            spans,
            secure_spans,
            signed_body,
            ..
        } = rx.recv().unwrap()
        else {
            panic!("expected chat event");
        };
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "<Alice> Decorated"
        );
        let secure_spans = secure_spans.expect("secure-only alternate rendering");
        assert_eq!(
            secure_spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "<Alice> signed body"
        );
        let signed_body = signed_body.expect("signed body metadata");
        assert!(signed_body.modified);
        assert!(!signed_body.modified_when_unsigned_hidden);
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

    #[test]
    fn command_suggestions_preserve_native_component_tooltips() {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "command_suggestions")
            .unwrap();
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, 42); // request id
        write_varint(&mut raw, 5); // replacement start
        write_varint(&mut raw, 1); // replacement length
        write_varint(&mut raw, 1); // one suggestion
        write_string(&mut raw, "value");
        raw.push(1); // tooltip present
        let mut tooltip = text_component("Native tooltip");
        tooltip.insert("color", "gold");
        write_component(&mut raw, tooltip);

        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&raw, &tx, &ChatTypeRegistry::default(), None)
            .unwrap()
            .unwrap();
        let NetworkEvent::CommandSuggestions { id, start, options } = rx.recv().unwrap() else {
            panic!("expected command suggestions event");
        };
        assert_eq!(id, 42);
        assert_eq!(start, 5);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].text, "value");
        let tooltip = options[0].tooltip.as_ref().expect("tooltip preserved");
        assert_eq!(tooltip.plain_text(), "Native tooltip");
        assert_eq!(tooltip.style.color, Some(0xffaa00));
    }

    #[test]
    fn player_chat_carries_sender_and_missing_profile_fallback() {
        let id = PacketTable::latest()
            .id(Phase::Game, Direction::Clientbound, "player_chat")
            .unwrap();
        let sender = uuid::Uuid::from_u128(0x12345678_90ab_cdef_1122_334455667788);
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, 0); // global index
        raw.extend_from_slice(sender.as_bytes());
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_string(&mut raw, "hello");
        raw.extend_from_slice(&0u64.to_be_bytes()); // timestamp
        raw.extend_from_slice(&0u64.to_be_bytes()); // salt
        write_varint(&mut raw, 0); // last-seen signatures
        raw.push(0); // no unsigned content
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");

        let registries = test_chat_registries("<%s> %s");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&raw, &tx, &registries, None)
            .unwrap()
            .unwrap();
        let NetworkEvent::ChatMessage {
            sender_uuid,
            missing_profile_spans,
            ..
        } = rx.recv().unwrap()
        else {
            panic!("expected chat event");
        };
        assert_eq!(sender_uuid, Some(sender));
        let fallback = missing_profile_spans.expect("missing-profile fallback");
        assert_eq!(
            fallback.iter().map(|s| s.text.as_str()).collect::<String>(),
            "<Alice> chat.validation_error"
        );
        let error = fallback
            .iter()
            .find(|span| span.text.contains("chat.validation_error"))
            .unwrap();
        let style = error.component_style.as_ref().unwrap();
        assert_eq!(style.color, Some(0xff5555));
        assert!(style.italic);
    }
}
