use super::*;

impl Ids765 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            // These two shadow 769's player_chat arm; the globalIndex
            // prepend is folded into the chat-type holder rewrite.
            i if i == self.player_chat_id => translate_player_chat_765,
            i if i == self.disguised_chat_id => translate_disguised_chat_765,
            i if i == self.update_mob_effect_id => translate_update_mob_effect_765,
            i if i == self.level_particles_id => translate_level_particles_765,
            i if i == self.server_data_id => translate_server_data_765,
            _ => return None,
        })
    }
}

/// The 1.20.4 `container_set_slot`: 767's sentinel handling plus the
/// old-form item translation.
pub(super) fn translate_container_set_slot_765(
    v: &Ids767,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?;
    let state = varint_span(&mut cur)?;
    let slot_at = cur.position() as usize;
    let slot = i16::from_be_bytes(payload.get(slot_at..slot_at + 2)?.try_into().ok()?);
    advance(&mut cur, 2)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    match container {
        -1 => wire::write_varint(&mut out, v.set_cursor_item_id),
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            out.extend_from_slice(&payload[state.start..slot_at + 2]);
        }
    }
    translate_item_765(&mut cur, &mut out, named_nbt)?;
    Some(out)
}

/// The 1.20.4 `container_click`: hashed stacks become bare old-form items
/// (the shared head copies; byte and varint container ids agree for
/// vanilla's small ids).
pub(super) fn translate_container_click_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        varint_span(&mut cur)?; // container id
        varint_span(&mut cur)?; // state id
        advance(&mut cur, 3)?; // slot, button
        varint_span(&mut cur)?; // click type

        let mut out = Vec::with_capacity(payload.len() + 8);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..cur.position() as usize]);

        let changed = var_u32(&mut cur)?;
        wire::write_varint(&mut out, changed);
        for _ in 0..changed {
            let slot_at = cur.position() as usize;
            advance(&mut cur, 2)?;
            out.extend_from_slice(&payload[slot_at..cur.position() as usize]);
            write_old_item(&mut out, read_hashed_stack(&mut cur)?);
        }
        write_old_item(&mut out, read_hashed_stack(&mut cur)?);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// The 1.20.4 `set_creative_mode_slot`: the component stack (whose patch
/// `remap_outbound` already cleared) becomes a bare old-form item.
pub(super) fn translate_creative_slot_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        advance(&mut cur, 2)?; // slot short
        let count = var_u32(&mut cur)?;

        let mut out = Vec::with_capacity(payload.len() + 4);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..2]);
        if count == 0 {
            out.push(0);
            return Some(out);
        }
        let item = var_u32(&mut cur)?;
        write_old_item(&mut out, Some((item, count)));
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// The 1.20.4 `chat_command`, which is always the signed form: empty
/// timestamp, salt, signatures and last-seen update are appended (offline
/// servers accept unsigned commands).
pub(super) fn translate_chat_command_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 24);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(payload);
    out.extend_from_slice(&[0; 16]); // timestamp, salt
    wire::write_varint(&mut out, 0); // no argument signatures
    wire::write_varint(&mut out, 0); // last-seen offset
    out.extend_from_slice(&[0; 3]); // last-seen acknowledged bit set
    vec![out]
}

/// Translates one 1.20.4 optional item stack into a bare 26.2 stack
/// (count + item + empty patch). Item ids stay in the wire version's
/// space for `remap_inbound`/`remap_stack`.
pub(super) fn translate_item_765(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    named_nbt: bool,
) -> Option<()> {
    let Some((item, count)) = read_old_item(cur, named_nbt)? else {
        wire::write_varint(out, 0);
        return Some(());
    };
    wire::write_varint(out, u32::from(count));
    wire::write_varint(out, item);
    wire::write_varint(out, 0);
    wire::write_varint(out, 0);
    Some(())
}

/// Rewrites `container_set_content`: the head (byte container id == varint
/// for vanilla's small ids, state id, count) copies; each item translates
/// bare.
pub(super) fn translate_container_set_content_765(
    id: u32,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?; // container id
    varint_span(&mut cur)?; // state id
    let count = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..=count {
        // The trailing iteration is the carried item.
        translate_item_765(&mut cur, &mut out, named_nbt)?;
    }
    Some(out)
}

/// Rewrites `set_equipment`'s slot/item pairs (the slot byte's high bit
/// continues the list).
pub(super) fn translate_set_equipment_765(
    id: u32,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    loop {
        let slot = read_u8(&mut cur)?;
        out.push(slot);
        translate_item_765(&mut cur, &mut out, named_nbt)?;
        if slot & 0x80 == 0 {
            return Some(out);
        }
    }
}

/// Rewrites `merchant_offers`: 1.20.5 turned the cost stacks into
/// `ItemCost` (item + count + component predicate, no NBT) with an explicit
/// optional second cost; the result stays a plain (bare) stack and the
/// per-offer numeric tail copies verbatim.
pub(super) fn translate_merchant_offers_765(
    id: u32,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // container id
    let offers = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..offers {
        translate_item_cost_765(&mut cur, &mut out, false, named_nbt)?;
        // Result: non-optional stack, bare.
        translate_item_765(&mut cur, &mut out, named_nbt)?;
        translate_item_cost_765(&mut cur, &mut out, true, named_nbt)?;
        let tail_at = cur.position() as usize;
        advance(&mut cur, 25)?; // outOfStock, 4 ints, multiplier, demand
        out.extend_from_slice(&payload[tail_at..cur.position() as usize]);
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// One 765 cost stack to a 26.2 `ItemCost` (`optional` wraps it in the
/// presence bool the second cost gained); an empty stack becomes an absent
/// cost, which only vanilla's costB ever is.
pub(super) fn translate_item_cost_765(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    optional: bool,
    named_nbt: bool,
) -> Option<()> {
    let Some((item, count)) = read_old_item(cur, named_nbt)? else {
        // Only the optional second cost can be absent; a missing base cost
        // has no `ItemCost` form, so the frame is unrepresentable.
        if !optional {
            return None;
        }
        out.push(0);
        return Some(());
    };
    if optional {
        out.push(1);
    }
    wire::write_varint(out, item);
    wire::write_varint(out, u32::from(count));
    wire::write_varint(out, 0); // empty component predicate
    Some(())
}

/// Rewrites `update_mob_effect`: 1.20.5 widened the amplifier from a byte
/// to a varint and dropped the trailing factor-data NBT. Vanilla reads that
/// byte signed, so an amplifier past 127 wraps negative on both sides; the
/// varint carries the sign through rather than clamping it.
pub(super) fn translate_update_mob_effect_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let head = varint_span(&mut cur)?; // entity id
    let effect = varint_span(&mut cur)?;
    let amplifier = read_u8(&mut cur)? as i8;
    let duration = varint_span(&mut cur)?;
    let flags = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[head.start..effect.end]);
    wire::write_varint(&mut out, amplifier as i32 as u32);
    out.extend_from_slice(&payload[duration]);
    out.push(flags);
    Some(out)
}

/// Rewrites `level_particles` for 1.20.4, where the particle type id led
/// the packet; it moves to just before the payload, and the `alwaysShow`
/// bool later versions gained is synthesized.
pub(super) fn translate_level_particles_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let particle = varint_span(&mut cur)?;
    let limiter_at = cur.position() as usize;
    advance(&mut cur, 1 + 24 + 16 + 4)?; // limiter, pos, dists, speed, count

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.push(payload[limiter_at]);
    out.push(0); // alwaysShow
    out.extend_from_slice(&payload[limiter_at + 1..cur.position() as usize]);
    out.extend_from_slice(&payload[particle]);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Rewrites `player_chat` for 1.20.4, whose trailing `ChatType.Bound` needs
/// the holder bump; the 1.21.5 globalIndex prepend folds in, since this arm
/// shadows 769's.
pub(super) fn translate_player_chat_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, 0); // globalIndex, 1.21.5+
    player_chat_head(&mut cur, &mut out, copy_nbt)?;
    chat_type_holder(&mut cur, &mut out, payload).map(|()| out)
}

/// Rewrites `disguised_chat` for 1.20.4: the same chat-type holder bump
/// after the message component.
pub(super) fn translate_disguised_chat_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_nbt(&mut cur)?; // message

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    chat_type_holder(&mut cur, &mut out, payload).map(|()| out)
}

/// Rewrites `server_data` for 1.20.4: the motd and optional icon, without
/// the trailing `enforcesSecureChat` bool.
pub(super) fn translate_server_data_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (end, _) = read_server_data_765(payload, false)?;
    let mut out = Vec::with_capacity(end + 5);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..end]);
    Some(out)
}

/// A pre-1.20.5 `server_data` payload: where its trailing `enforcesSecureChat`
/// bool starts, and the bool. The motd is JSON text before 1.20.3
/// (`json_motd`), NBT after; the optional icon bytes follow it.
pub(super) fn read_server_data_765(payload: &[u8], json_motd: bool) -> Option<(usize, bool)> {
    let mut cur = Cursor::new(payload);
    if json_motd {
        skip_utf(&mut cur)?;
    } else {
        skip_nbt(&mut cur)?;
    }
    skip_optional(&mut cur, skip_utf)?;
    let end = cur.position() as usize;
    match payload[end..] {
        [flag @ (0 | 1)] => Some((end, flag == 1)),
        _ => None,
    }
}

/// Rewrites `respawn` for 1.20.4: the spawn info's dimension type is a
/// resource key string, turned into the synced-registry index captured
/// from registry data; the seaLevel insert then applies like 767's.
pub(super) fn translate_respawn_765(
    id: u32,
    payload: &[u8],
    state: &SessionState,
) -> Option<Vec<u8>> {
    let converted = convert_spawn_info_dimension(payload, 0, state)?;
    let mut out = Vec::with_capacity(converted.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&insert_sea_level(&converted)?);
    Some(out)
}

/// Rewrites the 1.20.4 game `login` payload up to the 766 form (dimension
/// key string -> registry index, trailing enforcesSecureChat synthesized);
/// the shared seaLevel/onlineMode chain runs after.
pub(super) fn translate_game_login_765(payload: &[u8], state: &SessionState) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 5)?; // player id, hardcore
    let levels = var_u32(&mut cur)?;
    for _ in 0..levels {
        skip_utf(&mut cur)?;
    }
    varint_span(&mut cur)?; // max players
    varint_span(&mut cur)?; // chunk radius
    varint_span(&mut cur)?; // simulation distance
    advance(&mut cur, 3)?; // reducedDebug, showDeathScreen, doLimitedCrafting

    let mut converted = convert_spawn_info_dimension(payload, cur.position() as usize, state)?;
    converted.push(0); // enforcesSecureChat, absent at 765
    Some(converted)
}
