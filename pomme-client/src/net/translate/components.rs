use super::*;

/// Writes the direct chat-type registry id as the holder 1.20.5 replaced it
/// with (id + 1, 0 being reserved for an inline value), then the rest of the
/// frame. The id is synced-registry order either way, so it needs no remap.
pub(super) fn chat_type_holder(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    payload: &[u8],
) -> Option<()> {
    let chat_type = var_u32(cur)?;
    wire::write_varint(out, chat_type + 1);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(())
}

/// Reads one pre-1.20.3 length-prefixed JSON component and writes it as the
/// network-NBT form 1.20.3 introduced.
pub(super) fn transcode_component(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = var_u32(cur)? as usize;
    let at = cur.position() as usize;
    advance(cur, len)?;
    let json = std::str::from_utf8(&cur.get_ref()[at..at + len]).ok()?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    json_to_nbt(&value).write(out);
    Some(())
}

/// A JSON chat-component value as an owned NBT tag: the codec shape is
/// identical (vanilla runs the same component codec through JsonOps and
/// NbtOps), except NBT lists are homogeneous — mixed arrays normalize to
/// compound lists with primitives wrapped as `{text}`.
pub(super) fn json_to_nbt(value: &serde_json::Value) -> simdnbt::owned::NbtTag {
    use simdnbt::owned::{NbtList, NbtTag};
    match value {
        serde_json::Value::Null => NbtTag::String("".into()),
        serde_json::Value::Bool(b) => NbtTag::Byte(*b as i8),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => match i32::try_from(i) {
                Ok(i) => NbtTag::Int(i),
                Err(_) => NbtTag::Long(i),
            },
            None => NbtTag::Double(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => NbtTag::String(s.as_str().into()),
        serde_json::Value::Array(items) => NbtTag::List(NbtList::Compound(
            items.iter().map(json_to_compound).collect(),
        )),
        serde_json::Value::Object(_) => NbtTag::Compound(json_to_compound(value)),
    }
}

pub(super) fn json_to_compound(value: &serde_json::Value) -> simdnbt::owned::NbtCompound {
    let mut compound = simdnbt::owned::NbtCompound::new();
    match value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map {
                compound.insert(key.as_str(), json_to_nbt(entry));
            }
        }
        // A primitive inside a component list renders as its text form.
        other => {
            let text = match other {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            compound.insert("text", simdnbt::owned::NbtTag::String(text.as_str().into()));
        }
    }
    compound
}

/// Copies `varints` leading varints, then transcodes one trailing component.
pub(super) fn copy_then_transcode(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    varints: u32,
) -> Option<()> {
    for _ in 0..varints {
        copy_varint(cur, out)?;
    }
    transcode_component(cur, out)
}

/// `boss_event`'s name sits in the Add (0) and UpdateName (3) op bodies
/// (`ClientboundBossEventPacket`, order unchanged since 1.20.2).
pub(super) fn transcode_boss_event(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_bytes(cur, out, 16)?; // boss bar uuid
    let op = copy_varint(cur, out)?;
    if matches!(op, 0 | 3) {
        transcode_component(cur, out)?;
    }
    Some(())
}

/// The 764 team `Parameters` (display, options, visibility, collision,
/// color, prefix, suffix) precede the player list; the shared
/// `translate_team` reorder runs on the converted payload afterwards.
pub(super) fn transcode_team(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_utf(cur, out)?; // team name
    let method = read_u8(cur)?;
    out.push(method);
    if matches!(method, 0 | 2) {
        transcode_component(cur, out)?; // display name
        copy_bytes(cur, out, 1)?; // options
        copy_utf(cur, out)?; // nametag visibility
        copy_utf(cur, out)?; // collision rule
        copy_varint(cur, out)?; // color
        transcode_component(cur, out)?; // prefix
        transcode_component(cur, out)?; // suffix
    }
    Some(())
}

/// The shared `player_chat` walk up to the trailing chat type; `component`
/// handles the nullable unsignedContent (a verbatim copy at 765, the
/// JSON -> NBT transcode at 764).
pub(super) fn player_chat_head(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    component: impl FnOnce(&mut Cursor<&[u8]>, &mut Vec<u8>) -> Option<()>,
) -> Option<()> {
    copy_bytes(cur, out, 16)?; // sender uuid
    copy_varint(cur, out)?; // index
    copy_optional(cur, out, |c, o| copy_bytes(c, o, 256))?; // signature
    copy_utf(cur, out)?; // content
    copy_bytes(cur, out, 16)?; // timestamp, salt
    let last_seen = copy_varint(cur, out)?;
    for _ in 0..last_seen {
        // A zero id carries a full signature instead of a cache reference.
        if copy_varint(cur, out)? == 0 {
            copy_bytes(cur, out, 256)?;
        }
    }
    copy_optional(cur, out, component)?; // unsigned content
    let filter = copy_varint(cur, out)?;
    if filter == 2 {
        let longs = copy_varint(cur, out)?; // partially-filtered bit set
        copy_bytes(cur, out, longs as usize * 8)?;
    }
    Some(())
}

/// The `player_chat` walk through its three component sites (nullable
/// unsignedContent, then the chat-type name/target pair).
pub(super) fn transcode_player_chat(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    player_chat_head(cur, out, transcode_component)?;
    transcode_chat_type(cur, out)
}

/// `ChatType.BoundNetwork`: chat-type id, name, nullable target name.
pub(super) fn transcode_chat_type(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_varint(cur, out)?;
    transcode_component(cur, out)?;
    copy_optional(cur, out, transcode_component)
}

/// `player_info_update`: per entry, per action bit, the display name
/// (bit 5, last) is the one component; every earlier action's payload
/// copies (order per the 1.20.2 `ClientboundPlayerInfoUpdatePacket`, whose
/// first six actions match azalea's).
pub(super) fn transcode_player_info(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let actions = read_u8(cur)?;
    out.push(actions);
    let entries = copy_varint(cur, out)?;
    for _ in 0..entries {
        copy_bytes(cur, out, 16)?; // uuid
        if actions & 0x01 != 0 {
            copy_utf(cur, out)?; // name
            let properties = copy_varint(cur, out)?;
            for _ in 0..properties {
                copy_utf(cur, out)?;
                copy_utf(cur, out)?;
                copy_optional(cur, out, copy_utf)?; // signature
            }
        }
        if actions & 0x02 != 0 {
            // initialize chat: session uuid, expiry, public key, signature
            copy_optional(cur, out, |c, o| {
                copy_bytes(c, o, 24)?;
                let key = copy_varint(c, o)?;
                copy_bytes(c, o, key as usize)?;
                let sig = copy_varint(c, o)?;
                copy_bytes(c, o, sig as usize)
            })?;
        }
        if actions & 0x04 != 0 {
            copy_varint(cur, out)?; // game mode
        }
        if actions & 0x08 != 0 {
            copy_bytes(cur, out, 1)?; // listed
        }
        if actions & 0x10 != 0 {
            copy_varint(cur, out)?; // latency
        }
        if actions & 0x20 != 0 {
            copy_optional(cur, out, transcode_component)?; // display name
        }
    }
    Some(())
}

/// `command_suggestions`: nullable tooltip component per suggestion.
pub(super) fn transcode_suggestions(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    for _ in 0..3 {
        copy_varint(cur, out)?; // id, range start, range length
    }
    let suggestions = copy_varint(cur, out)?;
    for _ in 0..suggestions {
        copy_utf(cur, out)?;
        copy_optional(cur, out, transcode_component)?;
    }
    Some(())
}
