//! The layouts 26.3 changed, for wire versions below it. Every older gate's
//! rewrites leave a packet in its 26.2 layout; the upgrades here run after
//! them and lift that layout to 26.3's.

use super::*;

impl Ids776 {
    /// The 26.3 frame for a 26.2-layout `payload` of native packet `id`:
    /// `None` when 26.3 left the packet alone, `Some(None)` when the payload
    /// is malformed.
    pub(super) fn upgrade(&self, id: u32, payload: &[u8]) -> Option<Option<Vec<u8>>> {
        Some(match id {
            i if i == self.move_entity_pos_id => upgrade_move_entity(id, payload, false),
            i if i == self.move_entity_pos_rot_id => upgrade_move_entity(id, payload, true),
            i if i == self.move_entity_rot_id => upgrade_move_entity_rot(id, payload),
            i if i == self.entity_position_sync_id => upgrade_entity_position_sync(id, payload),
            i if i == self.login_id => upgrade_login(id, payload),
            i if i == self.respawn_id => upgrade_previous_game_type(id, payload, 0),
            i if i == self.animate_id => upgrade_animate(id, self.swing_animation_id, payload),
            i if i == self.level_chunk_id => upgrade_chunk(id, payload),
            i if i == self.light_update_id => upgrade_light_update(id, payload),
            i if i == self.player_chat_id => upgrade_player_chat(id, payload),
            i if i == self.explode_id => Some(upgrade_explode(id, payload)),
            _ => return None,
        })
    }
}

/// `move_entity_pos` / `move_entity_pos_rot`: 26.2 sends three shorts, the
/// rotation bytes if any, and a trailing onGround bool; 26.3 leads with a
/// `properties` varint, onGround in bit 0 and a step count above it, which is
/// zero for the one short triple here.
fn upgrade_move_entity(id: u32, payload: &[u8], rotation: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let body_at = cur.position() as usize;
    advance(&mut cur, if rotation { 8 } else { 6 })?;
    let on_ground = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.push(on_ground & 1);
    out.extend_from_slice(&payload[body_at..cur.position() as usize - 1]);
    Some(out)
}

/// `move_entity_rot`: 26.3 moved onGround ahead of the rotation bytes.
fn upgrade_move_entity_rot(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let rot_at = cur.position() as usize;
    advance(&mut cur, 2)?;
    let on_ground = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.push(on_ground);
    out.extend_from_slice(&payload[rot_at..rot_at + 2]);
    Some(out)
}

/// `entity_position_sync`: 26.2's position and delta doubles become a linear
/// `PositionPath` (type 0, then the position). The delta has no place in
/// 26.3's packet.
fn upgrade_entity_position_sync(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let position_at = cur.position() as usize;
    advance(&mut cur, 48)?; // position, delta
    let tail_at = cur.position() as usize;
    advance(&mut cur, 9)?; // yRot, xRot, onGround

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.push(0);
    out.extend_from_slice(&payload[position_at..position_at + 24]);
    out.extend_from_slice(&payload[tail_at..]);
    Some(out)
}

/// `login` carries its spawn info after the player id, hardcore flag, level
/// list, three varints and three bools.
fn upgrade_login(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 5)?; // player id, hardcore
    let levels = var_u32(&mut cur)?;
    for _ in 0..levels {
        skip_utf(&mut cur)?;
    }
    for _ in 0..3 {
        varint_span(&mut cur)?; // max players, chunk radius, simulation distance
    }
    advance(&mut cur, 3)?; // reduced debug info, death screen, limited crafting
    upgrade_previous_game_type(id, payload, cur.position() as usize)
}

/// The `previousGameType` of a `CommonPlayerSpawnInfo` starting at
/// `spawn_info_at`: 26.2 sends a signed byte (-1 = none) where 26.3 reads an
/// optional varint (0 = none, else id + 1). `gameType` before it went from a
/// byte to a varint, byte-identical for its four values.
fn upgrade_previous_game_type(id: u32, payload: &[u8], spawn_info_at: usize) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(spawn_info_at as u64);
    varint_span(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 9)?; // seed, game type
    let previous_at = cur.position() as usize;
    let previous = read_u8(&mut cur)? as i8;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..previous_at]);
    wire::write_varint(
        &mut out,
        u32::try_from(i32::from(previous) + 1).unwrap_or(0),
    );
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// `animate`: 26.3 moved the swings to `swing_animation` and renumbered the
/// rest (wake up 2 -> 0, critical hit 4 -> 1, magic critical hit 5 -> 2).
/// A 26.2 swing names only the hand (0 main, 3 off).
// TODO: a 26.2 swing takes `SwingAnimation.DEFAULT`; the 26.2 client swung
// with the entity's held item's own animation.
fn upgrade_animate(id: u32, swing_animation_id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (&action, entity) = payload.split_last()?;
    let mut out = Vec::with_capacity(payload.len() + 4);
    let hand = match action {
        0 => 0,
        3 => 1,
        _ => {
            wire::write_varint(&mut out, id);
            out.extend_from_slice(entity);
            out.push(match action {
                2 => 0,
                4 => 1,
                5 => 2,
                _ => return None,
            });
            return Some(out);
        }
    };
    wire::write_varint(&mut out, swing_animation_id);
    out.extend_from_slice(entity);
    // The hand, then a whack of six ticks.
    out.extend([hand, 1, 6]);
    Some(out)
}

/// `level_chunk_with_light`: the chunk data is byte-identical, and the light
/// masks after it changed codec (see [`upgrade_light_data`]).
fn upgrade_chunk(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints
    skip_heightmaps(&mut cur)?;
    let buffer_len = var_u32(&mut cur)?;
    advance(&mut cur, buffer_len as usize)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    copy_block_entities(&mut cur, &mut out, false)?;
    upgrade_light_data(&mut cur, &mut out)?;
    Some(out)
}

/// `light_update`: chunk x/z varints, then light data.
fn upgrade_light_update(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?;
    varint_span(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    upgrade_light_data(&mut cur, &mut out)?;
    Some(out)
}

/// `ClientboundLightUpdatePacketData`: 26.2 wrote its four section masks as
/// long arrays where 26.3 uses `ByteBufCodecs.BIT_SET`; the two light-layer
/// lists after them copy verbatim.
fn upgrade_light_data(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    for _ in 0..4 {
        pack_bit_set(cur, out)?;
    }
    out.extend_from_slice(&cur.get_ref()[cur.position() as usize..]);
    Some(())
}

/// Rewrites one 26.2 bit set (`toLongArray`: a varint long count, big-endian
/// longs) as 26.3's `ByteBufCodecs.BIT_SET` (`BitSet.toByteArray`: a varint
/// byte count, little-endian bytes without trailing zeros).
fn pack_bit_set(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let longs = var_u32(cur)? as usize;
    let at = cur.position() as usize;
    advance(cur, longs.checked_mul(8)?)?;
    let mut bytes: Vec<u8> = cur.get_ref()[at..at + longs * 8]
        .chunks(8)
        .flat_map(|word| word.iter().rev().copied())
        .collect();
    bytes.truncate(
        bytes
            .iter()
            .rposition(|b| *b != 0)
            .map_or(0, |last| last + 1),
    );
    wire::write_varint(out, bytes.len() as u32);
    out.extend_from_slice(&bytes);
    Some(())
}

/// `player_chat`: its `PARTIALLY_FILTERED` mask moved to
/// `ByteBufCodecs.BIT_SET` (`FilterMask`); everything around it copies
/// verbatim.
fn upgrade_player_chat(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // global index
    advance(&mut cur, 16)?; // sender
    varint_span(&mut cur)?; // index
    skip_optional(&mut cur, |c| advance(c, 256))?; // signature
    skip_utf(&mut cur)?; // content
    advance(&mut cur, 16)?; // timestamp, salt
    for _ in 0..var_u32(&mut cur)? {
        // A last-seen entry: cache id + 1, or 0 and the full signature.
        if var_u32(&mut cur)? == 0 {
            advance(&mut cur, 256)?;
        }
    }
    skip_optional(&mut cur, skip_nbt)?; // unsigned content
    let mask = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    if mask == 2 {
        pack_bit_set(&mut cur, &mut out)?;
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// `explode`: 26.3 appended a `playSound` bool, true for every explosion a
/// 26.2 server sends.
fn upgrade_explode(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload);
    out.push(1);
    out
}

/// Rewrites `explode` in 26.2's layout, which 1.21.9 to 26.2 share: the
/// particle and sound ids (the weighted block-particle list's too) move into
/// native space. [`upgrade_explode`] then adds what 26.3 appended.
pub(super) fn translate_explode_ids(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 32)?; // center, radius, block count
    skip_optional(&mut cur, |c| advance(c, 24))?; // player knockback
    let head_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..head_end]);
    translate_particles(&mut cur, &mut out, ids, remaps, false)?;
    translate_sound_holder(&mut cur, &mut out, remaps)?;
    let entries = var_u32(&mut cur)?;
    wire::write_varint(&mut out, entries);
    for _ in 0..entries {
        translate_particles(&mut cur, &mut out, ids, remaps, false)?;
        let scaling_at = cur.position() as usize;
        advance(&mut cur, 8)?; // scaling, speed
        let weight = varint_span(&mut cur)?;
        out.extend_from_slice(&payload[scaling_at..weight.end]);
    }
    Some(out)
}
