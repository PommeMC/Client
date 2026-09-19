//! Pomme-owned game-chat encoding and decoding. Inbound frames arrive after
//! version translation and before azalea's typed decode, whose 26.2 component
//! decoder drops hover events.

use std::collections::{HashSet, VecDeque};
use std::io::Cursor;

use crossbeam_channel::Sender;
use pomme_protocol::wire::{game_serverbound_id, read_varint, write_varint};
use pomme_protocol::{Direction, PacketTable, Phase};
use serde_json::Value;
use simdnbt::owned::{NbtCompound, NbtTag};
use uuid::Uuid;

use super::NetworkEvent;
use crate::chat_component::{
    Argument, Component, HoverEvent, Style, nbt_to_value, normalize_identifier,
};
use crate::net::chat_security::{LastSeenUpdate, SignedChatBody};
use crate::ui::chat::{ChatMessageSource, ChatMessageTag};
use crate::ui::text::format_component_spans;

const BAD_CHAT_INDEX: &str = "multiplayer.disconnect.bad_chat_index";
const INVALID_PACKET: &str = "multiplayer.disconnect.invalid_packet";
const SIGNATURE_CACHE_SIZE: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct ChatTypeRegistry {
    /// Chat decorations by protocol id, parsed once per registry sync.
    decorations: Vec<Result<ChatDecoration, String>>,
}

impl ChatTypeRegistry {
    pub fn from_entries(entries: Vec<NbtCompound>) -> Self {
        Self {
            decorations: entries
                .iter()
                .enumerate()
                .map(|(id, nbt)| registry_decoration(id as u32, nbt))
                .collect(),
        }
    }

    fn decoration(&self, protocol_id: u32) -> Result<ChatDecoration, String> {
        self.decorations
            .get(protocol_id as usize)
            .ok_or_else(|| format!("unknown chat_type registry id {protocol_id}"))?
            .clone()
    }
}

/// The inbound chat state vanilla `handleLogin` resets: the global message
/// index and the `MessageSignatureCache`.
pub struct InboundChat {
    signature_cache: Vec<Option<[u8; 256]>>,
    next_global_index: u32,
    /// 1.21.5 (770) added `globalIndex`; older layouts translate it as zero.
    validate_global_index: bool,
}

impl InboundChat {
    pub fn new(validate_global_index: bool) -> Self {
        Self {
            signature_cache: vec![None; SIGNATURE_CACHE_SIZE],
            next_global_index: 0,
            validate_global_index,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.validate_global_index);
    }

    fn unpack(&self, packed: PackedSignature) -> Option<[u8; 256]> {
        match packed {
            PackedSignature::Full(signature) => Some(*signature),
            PackedSignature::Id(id) => self.signature_cache.get(id).copied().flatten(),
        }
    }

    /// Vanilla `MessageSignatureCache.push`.
    fn push(&mut self, last_seen: &[[u8; 256]], signature: Option<[u8; 256]>) {
        let mut queue: VecDeque<[u8; 256]> = last_seen.iter().copied().collect();
        queue.extend(signature);
        let new_entries: HashSet<[u8; 256]> = queue.iter().copied().collect();
        for slot in &mut self.signature_cache {
            let Some(next) = queue.pop_back() else {
                break;
            };
            if let Some(previous) = slot.replace(next)
                && !new_entries.contains(&previous)
            {
                queue.push_front(previous);
            }
        }
    }
}

/// Why a chat packet was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ChatPacketError {
    /// The frame didn't decode, and is skipped like any malformed packet.
    Malformed(String),
    /// Vanilla disconnects with this translation key.
    Disconnect(&'static str),
}

impl From<String> for ChatPacketError {
    fn from(error: String) -> Self {
        Self::Malformed(error)
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

/// Vanilla `MessageSignature.Packed`: a cache id or a full signature.
#[derive(Clone, Debug)]
enum PackedSignature {
    Id(usize),
    Full(Box<[u8; 256]>),
}

/// `ServerboundChatPacket`, with or without a signature.
pub fn encode_outbound_message(
    message: &str,
    timestamp_millis: u64,
    salt: i64,
    signature: Option<&[u8; 256]>,
    update: &LastSeenUpdate,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 300);
    write_varint(&mut out, game_serverbound_id("chat"));
    write_wire_string(&mut out, message);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&salt.to_be_bytes());
    match signature {
        Some(signature) => {
            out.push(1);
            out.extend_from_slice(signature);
        }
        None => out.push(0),
    }
    write_last_seen_update(&mut out, update);
    out
}

pub fn encode_outbound_command(command: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(command.len() + 6);
    write_varint(&mut out, game_serverbound_id("chat_command"));
    write_wire_string(&mut out, command);
    out
}

pub fn encode_outbound_signed_command(
    command: &str,
    timestamp_millis: u64,
    salt: i64,
    signatures: &[(String, [u8; 256])],
    update: &LastSeenUpdate,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(command.len() + signatures.len() * 280 + 32);
    write_varint(&mut out, game_serverbound_id("chat_command_signed"));
    write_wire_string(&mut out, command);
    out.extend_from_slice(&timestamp_millis.to_be_bytes());
    out.extend_from_slice(&salt.to_be_bytes());
    write_varint(&mut out, signatures.len() as u32);
    for (name, signature) in signatures {
        write_wire_string(&mut out, name);
        out.extend_from_slice(signature);
    }
    write_last_seen_update(&mut out, update);
    out
}

pub fn encode_chat_session_update(
    session_id: Uuid,
    expires_at_ms: u64,
    public_key: &[u8],
    key_signature: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(public_key.len() + key_signature.len() + 40);
    write_varint(&mut out, game_serverbound_id("chat_session_update"));
    out.extend_from_slice(session_id.as_bytes());
    out.extend_from_slice(&expires_at_ms.to_be_bytes());
    write_varint(&mut out, public_key.len() as u32);
    out.extend_from_slice(public_key);
    write_varint(&mut out, key_signature.len() as u32);
    out.extend_from_slice(key_signature);
    out
}

pub fn encode_chat_ack(offset: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(6);
    write_varint(&mut out, game_serverbound_id("chat_ack"));
    write_varint(&mut out, offset);
    out
}

fn write_last_seen_update(out: &mut Vec<u8>, update: &LastSeenUpdate) {
    write_varint(out, update.offset);
    out.extend_from_slice(&update.acknowledged);
    out.push(update.checksum);
}

/// `ServerboundCustomClickActionPacket`, a common packet sent in whichever
/// phase the connection is in.
pub fn encode_outbound_custom_click_action(
    phase: Phase,
    identifier: &str,
    payload: Option<&NbtTag>,
) -> Result<Vec<u8>, String> {
    // The id is an `Identifier`, written as its `toString`.
    let identifier = normalize_identifier(identifier);
    if identifier.len() > 32_767 {
        return Err("custom click identifier is too long".into());
    }
    let packet_id = PacketTable::native()
        .id(phase, Direction::Serverbound, "custom_click_action")
        .expect("custom_click_action in packet table");
    let mut out = Vec::new();
    write_varint(&mut out, packet_id);
    write_wire_string(&mut out, &identifier);

    // Vanilla wraps Optional<Tag> in a length-prefixed sub-buffer. None is the
    // normal network-NBT end tag (single 0 byte); Some carries an unnamed tag.
    let mut tag_bytes = Vec::new();
    match payload {
        Some(tag) => tag.write(&mut tag_bytes),
        None => tag_bytes.push(0),
    }
    if tag_bytes.len() > 65_536 {
        return Err("custom click payload exceeds Vanilla's 65536-byte limit".into());
    }
    write_varint(&mut out, tag_bytes.len() as u32);
    out.extend_from_slice(&tag_bytes);
    Ok(out)
}

/// Returns `None` when this is not a chat packet. Chat packets are always
/// consumed, malformed ones included, so a bad payload never falls through to
/// azalea's lossy component decoder.
pub fn handle_raw_chat_packet(
    raw: &[u8],
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
    inbound: &mut InboundChat,
) -> Option<Result<(), ChatPacketError>> {
    let mut pos = 0usize;
    let packet_id = read_varint(raw, &mut pos)?;
    let name = PacketTable::native().name_of(Phase::Game, Direction::Clientbound, packet_id)?;

    let result = match name {
        "system_chat" => parse_system_chat(raw, &mut pos, event_tx),
        "set_action_bar_text" => parse_action_bar(raw, &mut pos, event_tx),
        "disguised_chat" => parse_disguised_chat(raw, &mut pos, event_tx, chat_types),
        "player_chat" => parse_player_chat(raw, &mut pos, event_tx, chat_types, inbound),
        "delete_chat" => parse_delete_chat(raw, &mut pos, event_tx, inbound),
        "command_suggestions" => parse_command_suggestions(raw, &mut pos, event_tx),
        _ => return None,
    };
    Some(result)
}

fn parse_system_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), ChatPacketError> {
    let component = read_component(raw, pos)?;
    let overlay = read_bool(raw, pos)?;
    ensure_end(raw, *pos, "system_chat")?;
    if overlay {
        send_action_bar(event_tx, &component);
    } else {
        send_chat(
            event_tx,
            ChatDelivery::unsigned(
                &component,
                ChatMessageSource::SystemServer,
                ChatMessageTag::SystemSinglePlayer,
            ),
        );
    }
    Ok(())
}

fn parse_action_bar(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), ChatPacketError> {
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
) -> Result<(), ChatPacketError> {
    let content = read_component(raw, pos)?;
    let bound = read_bound_chat_type(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "disguised_chat")?;
    let decorated = decorate(content, &bound);
    send_chat(
        event_tx,
        ChatDelivery::unsigned(
            &decorated,
            ChatMessageSource::Player,
            ChatMessageTag::System,
        ),
    );
    Ok(())
}

/// Decodes the whole packet, then runs vanilla `handlePlayerChat`'s index
/// check before unpacking the last-seen signatures from the cache.
fn parse_player_chat(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
    chat_types: &ChatTypeRegistry,
    inbound: &mut InboundChat,
) -> Result<(), ChatPacketError> {
    let global_index = read_varint_req(raw, pos, "player_chat.global_index")?;
    let sender_uuid = Uuid::from_bytes(
        take(raw, pos, 16, "player_chat.sender")?
            .try_into()
            .unwrap(),
    );
    let message_index = read_varint_req(raw, pos, "player_chat.index")? as i32;
    let signature = read_optional(raw, pos, |raw, pos| {
        read_full_signature(raw, pos, "player_chat.signature")
    })?;

    let signed_content = read_string(raw, pos, 256, "player_chat.body.content")?;
    let timestamp_ms = read_i64(raw, pos, "player_chat.timestamp")?;
    let salt = read_i64(raw, pos, "player_chat.salt")?;
    let last_seen_count = read_varint_req(raw, pos, "player_chat.last_seen.count")? as usize;
    if last_seen_count > 20 {
        return Err(ChatPacketError::Malformed(format!(
            "player_chat has {last_seen_count} last-seen signatures (max 20)"
        )));
    }
    let mut packed_last_seen = Vec::with_capacity(last_seen_count);
    for _ in 0..last_seen_count {
        packed_last_seen.push(read_packed_signature(raw, pos)?);
    }

    let unsigned = read_optional(raw, pos, read_component)?;
    let filter = read_filter_mask(raw, pos)?;
    let bound = read_bound_chat_type(raw, pos, chat_types)?;
    ensure_end(raw, *pos, "player_chat")?;

    if inbound.validate_global_index {
        let expected = inbound.next_global_index;
        inbound.next_global_index = expected.wrapping_add(1);
        if global_index != expected {
            tracing::error!(
                "Missing or out-of-order chat message from server, expected index {expected} but got {global_index}"
            );
            return Err(ChatPacketError::Disconnect(BAD_CHAT_INDEX));
        }
    }
    let last_seen = packed_last_seen
        .into_iter()
        .map(|packed| inbound.unpack(packed))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            tracing::error!(
                "Message from player with ID {sender_uuid} referenced unrecognized signature id"
            );
            ChatPacketError::Disconnect(INVALID_PACKET)
        })?;
    inbound.push(&last_seen, signature);

    let mut validation_error = Component::translate("chat.validation_error", Vec::new());
    validation_error.style.color = Some(0xff5555);
    validation_error.style.italic = Some(true);
    let missing_profile = decorate(validation_error, &bound);

    // Vanilla `ChatTrustLevel.isModified`, against the message as displayed
    // with and without its unsigned content.
    let decorated = decorate(
        unsigned
            .clone()
            .unwrap_or_else(|| Component::text(signed_content.clone())),
        &bound,
    );
    let decorated_signed = decorate(Component::text(signed_content.clone()), &bound);
    let modified_style = unsigned
        .as_ref()
        .is_some_and(component_has_non_default_font);
    let modified =
        |decorated: &Component| !decorated.plain_text().contains(&signed_content) || modified_style;
    let signed_body = SignedChatBody {
        modified: modified(&decorated),
        modified_when_unsigned_hidden: modified(&decorated_signed),
        fully_filtered: matches!(filter, FilterMask::FullyFiltered),
        content: signed_content.clone(),
        timestamp_ms,
        salt,
        last_seen,
        message_index,
    };

    let (decorated, secure_decorated) = match &filter {
        FilterMask::FullyFiltered | FilterMask::PassThrough => (decorated, decorated_signed),
        FilterMask::Partial(bits) => {
            let filtered = decorate(filtered_component(&signed_content, bits), &bound);
            (filtered.clone(), filtered)
        }
    };
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
    inbound: &InboundChat,
) -> Result<(), ChatPacketError> {
    let packed = read_packed_signature(raw, pos)?;
    ensure_end(raw, *pos, "delete_chat")?;
    let signature = inbound
        .unpack(packed)
        .ok_or(ChatPacketError::Disconnect(INVALID_PACKET))?;
    let _ = event_tx.try_send(NetworkEvent::DeleteChatMessage { signature });
    Ok(())
}

fn parse_command_suggestions(
    raw: &[u8],
    pos: &mut usize,
    event_tx: &Sender<NetworkEvent>,
) -> Result<(), ChatPacketError> {
    let id = read_varint_req(raw, pos, "command_suggestions.id")?;
    let start = read_varint_req(raw, pos, "command_suggestions.start")? as usize;
    // Requests carry the whole input, so the range always runs to its end.
    let _length = read_varint_req(raw, pos, "command_suggestions.length")?;
    let count = read_varint_req(raw, pos, "command_suggestions.count")? as usize;
    let mut options = Vec::new();
    for _ in 0..count {
        let text = read_string(raw, pos, 32_767, "command_suggestions.text")?;
        let tooltip = read_optional(raw, pos, read_component)?;
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
    sender_uuid: Option<Uuid>,
    signed_body: Option<SignedChatBody>,
    source: ChatMessageSource,
    tag: Option<ChatMessageTag>,
}

impl<'a> ChatDelivery<'a> {
    fn unsigned(component: &'a Component, source: ChatMessageSource, tag: ChatMessageTag) -> Self {
        Self {
            component,
            secure_component: None,
            missing_profile_component: None,
            signature: None,
            sender_uuid: None,
            signed_body: None,
            source,
            tag: Some(tag),
        }
    }
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

fn decorate(content: Component, bound: &BoundChatType) -> Component {
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
    let mut result = Component::translate(bound.decoration.translation_key.clone(), args);
    result.style = bound.decoration.style.clone();
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
        read_direct_decoration(raw, pos)?; // narration, unused
        chat
    } else {
        chat_types.decoration(holder - 1)?
    };
    let name = read_component(raw, pos)?;
    let target_name = read_optional(raw, pos, read_component)?;
    Ok(BoundChatType {
        decoration,
        name,
        target_name,
    })
}

fn read_direct_decoration(raw: &[u8], pos: &mut usize) -> Result<ChatDecoration, String> {
    let translation_key = read_string(raw, pos, 32767, "chat_type.translation_key")?;
    let count = read_varint_req(raw, pos, "chat_type.parameters.count")?;
    let mut parameters = Vec::new();
    for _ in 0..count {
        // `Parameter.BY_ID` maps out-of-range ids to SENDER (`ZERO`).
        parameters.push(match read_varint_req(raw, pos, "chat_type.parameter")? {
            1 => DecorationParameter::Target,
            2 => DecorationParameter::Content,
            _ => DecorationParameter::Sender,
        });
    }
    let style = Style::from_nbt_tag(&read_nbt_tag(raw, pos)?)
        .map_err(|e| format!("invalid chat decoration style: {e}"))?;
    Ok(ChatDecoration {
        translation_key,
        parameters,
        style,
    })
}

fn registry_decoration(protocol_id: u32, nbt: &NbtCompound) -> Result<ChatDecoration, String> {
    let missing = |field: &str| format!("chat_type registry id {protocol_id} has no {field}");
    let chat_tag = nbt
        .compound("chat")
        .ok_or_else(|| missing("chat decoration"))?;
    let chat = nbt_to_value(&NbtTag::Compound(chat_tag.clone()));
    let translation_key = chat
        .get("translation_key")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("translation_key"))?
        .to_owned();
    let parameter_values = chat
        .get("parameters")
        .and_then(Value::as_array)
        .ok_or_else(|| missing("parameters"))?;
    let mut parameters = Vec::with_capacity(parameter_values.len());
    for value in parameter_values {
        parameters.push(match value.as_str() {
            Some("sender") => DecorationParameter::Sender,
            Some("target") => DecorationParameter::Target,
            Some("content") => DecorationParameter::Content,
            other => return Err(format!("unknown chat decoration parameter {other:?}")),
        });
    }
    let style = match chat_tag.get("style") {
        Some(tag) => Style::from_nbt_tag(tag)
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
            let count = read_varint_req(raw, pos, "player_chat.filter_mask.longs")?;
            let mut longs = Vec::new();
            for _ in 0..count {
                longs.push(read_i64(raw, pos, "player_chat.filter_mask.long")? as u64);
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

fn read_packed_signature(raw: &[u8], pos: &mut usize) -> Result<PackedSignature, String> {
    match read_varint_req(raw, pos, "packed_message_signature.id")? {
        0 => read_full_signature(raw, pos, "packed_message_signature.full")
            .map(|signature| PackedSignature::Full(Box::new(signature))),
        id => Ok(PackedSignature::Id(id as usize - 1)),
    }
}

/// Vanilla `ChatTrustLevel.isModifiedStyle`: any font but the default,
/// object runs included (they render through their own font description).
fn component_has_non_default_font(component: &Component) -> bool {
    let mut modified = false;
    component.visit_text(
        &crate::chat_component::ResolvedStyle::default(),
        &mut |_, style| {
            modified |= style.inline_object.is_some()
                || style.font.as_ref().is_some_and(|font| {
                    !matches!(font, Value::String(id) if id == "minecraft:default" || id == "default")
                });
        },
    );
    modified
}

pub(super) fn read_component(raw: &[u8], pos: &mut usize) -> Result<Component, String> {
    let tag = read_nbt_tag(raw, pos)?;
    Component::from_nbt_tag(&tag).map_err(|e| format!("invalid text component: {e}"))
}

pub(super) fn read_nbt_tag(raw: &[u8], pos: &mut usize) -> Result<NbtTag, String> {
    let slice = raw
        .get(*pos..)
        .ok_or_else(|| "NBT begins past end of packet".to_owned())?;
    let mut cursor = Cursor::new(slice);
    let tag =
        simdnbt::owned::read_tag(&mut cursor).map_err(|e| format!("invalid network NBT: {e:?}"))?;
    *pos += cursor.position() as usize;
    Ok(tag)
}

fn write_wire_string(out: &mut Vec<u8>, value: &str) {
    write_varint(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

pub(super) fn read_string(
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
    if value.encode_utf16().count() > max_chars {
        return Err(format!("{field} exceeds {max_chars} UTF-16 code units"));
    }
    Ok(value.to_owned())
}

pub(super) fn read_bool(raw: &[u8], pos: &mut usize) -> Result<bool, String> {
    Ok(take(raw, pos, 1, "boolean")?[0] != 0)
}

/// A boolean-prefixed optional field.
fn read_optional<T>(
    raw: &[u8],
    pos: &mut usize,
    read: impl FnOnce(&[u8], &mut usize) -> Result<T, String>,
) -> Result<Option<T>, String> {
    if read_bool(raw, pos)? {
        read(raw, pos).map(Some)
    } else {
        Ok(None)
    }
}

fn read_i64(raw: &[u8], pos: &mut usize, field: &str) -> Result<i64, String> {
    Ok(i64::from_be_bytes(
        take(raw, pos, 8, field)?.try_into().unwrap(),
    ))
}

pub(super) fn read_varint_req(raw: &[u8], pos: &mut usize, field: &str) -> Result<u32, String> {
    read_varint(raw, pos).ok_or_else(|| format!("truncated/invalid varint for {field}"))
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

pub(super) fn ensure_end(raw: &[u8], pos: usize, packet: &str) -> Result<(), String> {
    if pos == raw.len() {
        Ok(())
    } else {
        Err(format!("{packet} has {} trailing bytes", raw.len() - pos))
    }
}

#[cfg(test)]
mod tests {
    use pomme_protocol::version::{NATIVE, VERSIONS};
    use simdnbt::owned::{NbtCompound, NbtList};

    use super::super::translate::{Translation, joinable};
    use super::*;
    use crate::chat_component::ClickEvent;
    use crate::net::chat_security::LastSeenUpdate;
    use crate::ui::text::TextSpan;

    fn native_id(name: &str) -> u32 {
        PacketTable::native()
            .id(Phase::Game, Direction::Clientbound, name)
            .unwrap()
    }

    fn write_component(out: &mut Vec<u8>, compound: NbtCompound) {
        NbtTag::Compound(compound).write(out);
    }

    fn text_component(text: &str) -> NbtCompound {
        let mut component = NbtCompound::new();
        component.insert("text", text);
        component
    }

    fn show_text_hover(text: &str) -> NbtTag {
        let mut hover = NbtCompound::new();
        hover.insert("action", "show_text");
        hover.insert("value", NbtTag::Compound(text_component(text)));
        NbtTag::Compound(hover)
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

    fn joinable_protocols() -> Vec<i32> {
        let mut protocols: Vec<i32> = VERSIONS
            .iter()
            .map(|version| version.protocol)
            .filter(|&protocol| joinable(protocol))
            .collect();
        protocols.sort_unstable();
        protocols.dedup();
        protocols
    }

    fn translate_inbound(protocol: i32, frame: Vec<u8>) -> Box<[u8]> {
        if protocol == NATIVE.protocol {
            return frame.into_boxed_slice();
        }
        Translation::for_protocol(protocol)
            .unwrap()
            .translate_game_frame(frame.into_boxed_slice())
            .unwrap_or_else(|| panic!("frame did not translate from protocol {protocol}"))
    }

    fn decode(raw: &[u8], chat_types: &ChatTypeRegistry) -> NetworkEvent {
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(raw, &tx, chat_types, &mut InboundChat::new(false))
            .expect("not a chat packet")
            .unwrap_or_else(|e| panic!("chat decode failed: {e:?}"));
        rx.recv().unwrap()
    }

    fn decode_chat(raw: &[u8], chat_types: &ChatTypeRegistry) -> Vec<TextSpan> {
        let NetworkEvent::ChatMessage { spans, .. } = decode(raw, chat_types) else {
            panic!("expected chat event");
        };
        spans
    }

    /// An unsigned chat message with an empty last-seen update.
    fn unsigned(message: &str, timestamp: u64) -> Vec<u8> {
        let update = LastSeenUpdate {
            offset: 0,
            acknowledged: [0; 3],
            checksum: 0,
            last_seen: Vec::new(),
        };
        encode_outbound_message(message, timestamp, 0, None, &update)
    }

    fn plain(spans: &[TextSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    fn style_of<'a>(spans: &'a [TextSpan], text: &str) -> &'a crate::chat_component::ResolvedStyle {
        spans
            .iter()
            .find(|s| s.text == text)
            .and_then(|s| s.component_style.as_deref())
            .unwrap_or_else(|| panic!("no styled span {text:?}"))
    }

    #[test]
    fn outbound_message_layout() {
        let mut expected = vec![game_serverbound_id("chat") as u8, 5];
        expected.extend_from_slice(b"hello");
        expected.extend_from_slice(&1234u64.to_be_bytes());
        // Salt, no signature, last-seen offset, 20 acknowledged bits, checksum.
        expected.extend_from_slice(&[0; 8 + 1 + 1 + 3 + 1]);
        assert_eq!(unsigned("hello", 1234), expected);
    }

    #[test]
    fn outbound_command_layout() {
        let mut expected = vec![game_serverbound_id("chat_command") as u8, 6];
        expected.extend_from_slice(b"say hi");
        assert_eq!(encode_outbound_command("say hi"), expected);
    }

    #[test]
    fn outbound_message_translates_every_supported_protocol() {
        for protocol in joinable_protocols() {
            let native = unsigned("cross-version", 99);
            let frames = if protocol == NATIVE.protocol {
                vec![native]
            } else {
                Translation::for_protocol(protocol)
                    .unwrap()
                    .translate_outbound_game_frame(native)
            };
            assert_eq!(frames.len(), 1, "protocol {protocol}");
            let frame = &frames[0];
            let table = PacketTable::for_protocol(protocol).unwrap_or_else(PacketTable::native);
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
            // 1.21.4 and older have no trailing last-seen checksum byte.
            if protocol >= 770 {
                skip(frame, &mut pos, 1, "checksum").unwrap();
            }
            assert_eq!(pos, frame.len(), "protocol {protocol}");
        }
    }

    #[test]
    fn system_chat_round_trips_every_supported_protocol() {
        for protocol in joinable_protocols() {
            let table = PacketTable::for_protocol(protocol).unwrap_or_else(PacketTable::native);
            let mut wire = Vec::new();
            write_varint(
                &mut wire,
                table
                    .id(Phase::Game, Direction::Clientbound, "system_chat")
                    .unwrap(),
            );
            let text = format!("hello-{protocol}");
            if protocol <= 764 {
                let json = serde_json::json!({"text": text, "color": "aqua"});
                write_wire_string(&mut wire, &json.to_string());
            } else {
                let mut component = text_component(&text);
                component.insert("color", "aqua");
                write_component(&mut wire, component);
            }
            wire.push(0); // not overlay

            let spans = decode_chat(
                &translate_inbound(protocol, wire),
                &ChatTypeRegistry::default(),
            );
            assert_eq!(plain(&spans), text);
            assert_eq!(style_of(&spans, &text).color, Some(0x55ffff));
        }
    }

    #[test]
    fn system_chat_from_1_20_1_keeps_legacy_hover() {
        let mut old = Vec::new();
        write_varint(
            &mut old,
            PacketTable::for_protocol(763)
                .unwrap()
                .id(Phase::Game, Direction::Clientbound, "system_chat")
                .unwrap(),
        );
        let json = serde_json::json!({
            "text": "Old server",
            "color": "gold",
            "hoverEvent": {"action": "show_text", "contents": {"text": "1.20.1 tooltip"}}
        });
        write_wire_string(&mut old, &json.to_string());
        old.push(0); // not overlay

        let spans = decode_chat(&translate_inbound(763, old), &ChatTypeRegistry::default());
        let style = style_of(&spans, "Old server");
        assert_eq!(style.color, Some(0xffaa00));
        assert!(matches!(style.hover_event, Some(HoverEvent::Text(_))));
    }

    #[test]
    fn disguised_chat_from_1_20_1_translates_chat_type_and_components() {
        let mut old = Vec::new();
        write_varint(
            &mut old,
            PacketTable::for_protocol(763)
                .unwrap()
                .id(Phase::Game, Direction::Clientbound, "disguised_chat")
                .unwrap(),
        );
        let content = serde_json::json!({
            "text": "Legacy hello",
            "clickEvent": {"action": "copy_to_clipboard", "value": "legacy"}
        });
        write_wire_string(&mut old, &content.to_string());
        write_varint(&mut old, 0); // direct chat_type registry id before holders
        write_wire_string(&mut old, &serde_json::json!({"text": "Alice"}).to_string());
        old.push(0); // no target name

        let spans = decode_chat(
            &translate_inbound(763, old),
            &test_chat_registries("<%s> %s"),
        );
        assert_eq!(plain(&spans), "<Alice> Legacy hello");
        assert_eq!(
            style_of(&spans, "Legacy hello").click_event,
            Some(ClickEvent::CopyToClipboard("legacy".into()))
        );
    }

    #[test]
    fn system_chat_decodes_hover_before_azalea() {
        let mut root = text_component("Click me");
        root.insert("hover_event", show_text_hover("Tooltip"));
        let mut raw = Vec::new();
        write_varint(&mut raw, native_id("system_chat"));
        write_component(&mut raw, root);
        raw.push(0); // not overlay

        let spans = decode_chat(&raw, &ChatTypeRegistry::default());
        assert!(matches!(
            style_of(&spans, "Click me").hover_event,
            Some(HoverEvent::Text(_))
        ));
    }

    #[test]
    fn disguised_chat_uses_registry_decoration() {
        let mut content = text_component("Hello");
        let mut click = NbtCompound::new();
        click.insert("action", "copy_to_clipboard");
        click.insert("value", "hello");
        content.insert("click_event", NbtTag::Compound(click));
        let mut raw = Vec::new();
        write_varint(&mut raw, native_id("disguised_chat"));
        write_component(&mut raw, content);
        write_bound_chat_type(&mut raw, "Alice");

        let spans = decode_chat(&raw, &test_chat_registries("<%s> %s"));
        assert_eq!(plain(&spans), "<Alice> Hello");
        let style = style_of(&spans, "Hello");
        assert_eq!(
            style.click_event,
            Some(ClickEvent::CopyToClipboard("hello".into()))
        );
        assert_eq!(style.color, Some(0xaaaaaa));
    }

    #[test]
    fn player_chat_layout_preserves_unsigned_component_interactions() {
        let mut unsigned = text_component("Decorated");
        unsigned.insert("hover_event", show_text_hover("Unsigned tooltip"));
        let mut raw = Vec::new();
        write_varint(&mut raw, native_id("player_chat"));
        write_varint(&mut raw, 0); // global index
        raw.extend_from_slice(&[0; 16]); // sender UUID
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_wire_string(&mut raw, "signed body");
        raw.extend_from_slice(&[0; 16]); // timestamp, salt
        write_varint(&mut raw, 0); // last-seen signatures
        raw.push(1); // unsigned content present
        write_component(&mut raw, unsigned);
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");

        let spans = decode_chat(&raw, &test_chat_registries("<%s> %s"));
        assert_eq!(plain(&spans), "<Alice> Decorated");
        assert!(matches!(
            style_of(&spans, "Decorated").hover_event,
            Some(HoverEvent::Text(_))
        ));
    }

    #[test]
    fn partial_filter_builds_dark_gray_hoverable_hashes() {
        let component = filtered_component("abcdef", &[0b001100]);
        let spans = format_component_spans(&component, [1.0; 4]);
        assert_eq!(plain(&spans), "ab##ef");
        let filtered = style_of(&spans, "##");
        assert_eq!(filtered.color, Some(0x555555));
        assert!(matches!(filtered.hover_event, Some(HoverEvent::Text(_))));
    }

    #[test]
    fn action_bar_and_overlay_system_chat_reach_the_action_bar() {
        let mut action_bar = Vec::new();
        write_varint(&mut action_bar, native_id("set_action_bar_text"));
        write_component(&mut action_bar, text_component("bar"));
        let mut overlay = Vec::new();
        write_varint(&mut overlay, native_id("system_chat"));
        write_component(&mut overlay, text_component("bar"));
        overlay.push(1); // overlay

        for raw in [action_bar, overlay] {
            let NetworkEvent::ActionBar { spans } = decode(&raw, &ChatTypeRegistry::default())
            else {
                panic!("expected action bar event");
            };
            assert_eq!(plain(&spans), "bar");
        }
    }

    #[test]
    fn direct_chat_type_parameters_are_unbounded_and_default_to_sender() {
        let mut raw = Vec::new();
        write_varint(&mut raw, native_id("disguised_chat"));
        write_component(&mut raw, text_component("hi"));
        write_varint(&mut raw, 0); // direct holder
        for _ in 0..2 {
            // Chat decoration, then narration decoration.
            write_wire_string(&mut raw, "%s %s %s %s");
            write_varint(&mut raw, 4);
            for parameter in [0, 2, 1, 9] {
                write_varint(&mut raw, parameter);
            }
            write_component(&mut raw, NbtCompound::new()); // empty style
        }
        write_component(&mut raw, text_component("Alice"));
        raw.push(0); // no target name

        let spans = decode_chat(&raw, &ChatTypeRegistry::default());
        assert_eq!(plain(&spans), "Alice hi  Alice");
    }

    fn player_chat_packet(global_index: u32) -> Vec<u8> {
        player_chat_packet_seen(global_index, &[])
    }

    /// A player chat whose last-seen list holds these packed cache ids.
    fn player_chat_packet_seen(global_index: u32, cache_ids: &[u32]) -> Vec<u8> {
        let id = native_id("player_chat");
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, global_index);
        raw.extend_from_slice(&[0; 16]);
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_wire_string(&mut raw, "hello");
        raw.extend_from_slice(&0u64.to_be_bytes());
        raw.extend_from_slice(&0u64.to_be_bytes());
        write_varint(&mut raw, cache_ids.len() as u32);
        for &id in cache_ids {
            write_varint(&mut raw, id + 1);
        }
        raw.push(0); // no unsigned content
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");
        raw
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
        let raw = encode_outbound_message("signed", 1234, -55, Some(&signature), &update);
        let mut pos = 0;
        assert_eq!(
            read_varint(&raw, &mut pos),
            PacketTable::native().id(Phase::Game, Direction::Serverbound, "chat")
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
        let native = encode_outbound_message("old signed", 9, 3, Some(&[4; 256]), &update);
        for protocol in [763, 765, 766, 769] {
            let frame = super::super::translate::Translation::for_protocol(protocol)
                .unwrap()
                .translate_outbound_game_frame(native.clone())
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
        let native = encode_outbound_signed_command(
            "msg Steve hello there",
            55,
            -8,
            &[("message".into(), [0xa5; 256])],
            &update,
        );

        for protocol in [763, 765, 766, 769, 770, 776] {
            let frames = if protocol == pomme_protocol::version::NATIVE.protocol {
                vec![native.clone()]
            } else {
                super::super::translate::Translation::for_protocol(protocol)
                    .unwrap()
                    .translate_outbound_game_frame(native.clone())
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

    fn handle(raw: &[u8], inbound: &mut InboundChat) -> Result<(), ChatPacketError> {
        let (tx, _rx) = crossbeam_channel::bounded(8);
        handle_raw_chat_packet(raw, &tx, &test_chat_registries("<%s> %s"), inbound).unwrap()
    }

    #[test]
    fn player_chat_index_matches_vanilla_handle_player_chat() {
        let mut inbound = InboundChat::new(true);
        handle(&player_chat_packet(0), &mut inbound).unwrap();

        let mut malformed = player_chat_packet(1);
        malformed.pop();
        assert!(matches!(
            handle(&malformed, &mut inbound),
            Err(ChatPacketError::Malformed(_))
        ));
        handle(&player_chat_packet(1), &mut inbound).unwrap();

        assert_eq!(
            handle(&player_chat_packet(3), &mut inbound),
            Err(ChatPacketError::Disconnect(BAD_CHAT_INDEX))
        );

        inbound.reset();
        handle(&player_chat_packet(0), &mut inbound).unwrap();
        inbound.next_global_index = u32::MAX;
        handle(&player_chat_packet(u32::MAX), &mut inbound).unwrap();
        assert_eq!(inbound.next_global_index, 0, "Java int sequence wraps");
    }

    #[test]
    fn unknown_signature_cache_ids_disconnect_after_the_index_check() {
        let unknown_id = player_chat_packet_seen(0, &[4]);

        let mut inbound = InboundChat::new(true);
        inbound.next_global_index = 1;
        assert_eq!(
            handle(&unknown_id, &mut inbound),
            Err(ChatPacketError::Disconnect(BAD_CHAT_INDEX))
        );
        let mut inbound = InboundChat::new(true);
        assert_eq!(
            handle(&unknown_id, &mut inbound),
            Err(ChatPacketError::Disconnect(INVALID_PACKET))
        );

        let mut delete = Vec::new();
        write_varint(&mut delete, native_id("delete_chat"));
        write_varint(&mut delete, 1);
        assert_eq!(
            handle(&delete, &mut inbound),
            Err(ChatPacketError::Disconnect(INVALID_PACKET))
        );
    }

    #[test]
    fn session_update_and_ack_layouts() {
        let session = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let mut expected = vec![game_serverbound_id("chat_session_update") as u8];
        expected.extend_from_slice(session.as_bytes());
        expected.extend_from_slice(&0x1234u64.to_be_bytes());
        expected.extend_from_slice(&[2, 0xaa, 0xbb, 1, 0xcc]);
        assert_eq!(
            encode_chat_session_update(session, 0x1234, &[0xaa, 0xbb], &[0xcc]),
            expected
        );
        assert_eq!(
            encode_chat_ack(300),
            [game_serverbound_id("chat_ack") as u8, 0xac, 0x02]
        );
    }

    #[test]
    fn command_suggestions_preserve_native_component_tooltips() {
        let id = native_id("command_suggestions");
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, 42); // request id
        write_varint(&mut raw, 5); // replacement start
        write_varint(&mut raw, 1); // replacement length
        write_varint(&mut raw, 1); // one suggestion
        write_wire_string(&mut raw, "value");
        raw.push(1); // tooltip present
        let mut tooltip = text_component("Native tooltip");
        tooltip.insert("color", "gold");
        write_component(&mut raw, tooltip);

        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(
            &raw,
            &tx,
            &ChatTypeRegistry::default(),
            &mut InboundChat::new(false),
        )
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
        let id = native_id("player_chat");
        let sender = Uuid::from_u128(0x12345678_90ab_cdef_1122_334455667788);
        let mut raw = Vec::new();
        write_varint(&mut raw, id);
        write_varint(&mut raw, 0); // global index
        raw.extend_from_slice(sender.as_bytes());
        write_varint(&mut raw, 0); // message index
        raw.push(0); // no signature
        write_wire_string(&mut raw, "hello");
        raw.extend_from_slice(&0u64.to_be_bytes()); // timestamp
        raw.extend_from_slice(&0u64.to_be_bytes()); // salt
        write_varint(&mut raw, 0); // last-seen signatures
        raw.push(0); // no unsigned content
        write_varint(&mut raw, 0); // pass-through filter
        write_bound_chat_type(&mut raw, "Alice");

        let registries = test_chat_registries("<%s> %s");
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_raw_chat_packet(&raw, &tx, &registries, &mut InboundChat::new(false))
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

    #[test]
    fn custom_click_packet_writes_the_normalized_identifier() {
        // Optional<Tag> sub-buffer: length 1, end tag.
        let mut expected = vec![68, 13];
        expected.extend_from_slice(b"minecraft:foo");
        expected.extend_from_slice(&[1, 0]);
        assert_eq!(
            encode_outbound_custom_click_action(Phase::Game, "foo", None).unwrap(),
            expected
        );

        let frame = encode_outbound_custom_click_action(Phase::Game, "a:b", None).unwrap();
        assert_eq!(frame, [68, 3, b'a', b':', b'b', 1, 0]);
    }

    /// The configuration phase registers the packet under its own id.
    #[test]
    fn custom_click_packet_uses_the_configuration_id() {
        let frame = encode_outbound_custom_click_action(Phase::Configuration, "a:b", None).unwrap();
        assert_eq!(frame, [8, 3, b'a', b':', b'b', 1, 0]);
    }

    #[test]
    fn custom_click_packet_preserves_exact_nbt_tag_types() {
        let mut compound = NbtCompound::new();
        compound.insert("byte", NbtTag::Byte(-5));
        compound.insert("short", NbtTag::Short(300));
        compound.insert("long", NbtTag::Long(9_000_000_000));
        compound.insert("float", NbtTag::Float(1.5));
        compound.insert("bytes", NbtTag::ByteArray(vec![0, 128, 255]));
        compound.insert("ints", NbtTag::IntArray(vec![-1, 2, 3]));
        compound.insert("longs", NbtTag::LongArray(vec![-4, 5, 6]));
        let payload = NbtTag::Compound(compound);

        let frame =
            encode_outbound_custom_click_action(Phase::Game, "minecraft:test", Some(&payload))
                .unwrap();
        let mut pos = 0usize;
        let packet_id = read_varint(&frame, &mut pos).unwrap();
        assert_eq!(
            PacketTable::native().name_of(Phase::Game, Direction::Serverbound, packet_id),
            Some("custom_click_action")
        );
        assert_eq!(
            read_string(&frame, &mut pos, 32_767, "id").unwrap(),
            "minecraft:test"
        );
        let payload_len = read_varint_req(&frame, &mut pos, "payload length").unwrap() as usize;
        let bytes = take(&frame, &mut pos, payload_len, "payload").unwrap();
        let mut cursor = Cursor::new(bytes);
        let decoded = simdnbt::owned::read_tag(&mut cursor).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(pos, frame.len());
    }
}
