use super::*;

impl Ids763 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.forget_level_chunk_id => translate_forget_level_chunk_763,
            i if i == self.block_entity_data_id => translate_block_entity_data_763,
            i if i == self.tag_query_id => translate_tag_query_763,
            _ => return None,
        })
    }
}

/// Rewrites `respawn`: 1.20.2 moved `dataToKeep` from the middle of the spawn
/// info to the end of the packet. Chains into the 765 rewrite.
pub(super) fn translate_respawn_763(
    id: u32,
    payload: &[u8],
    state: &SessionState,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_utf(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 12)?; // seed, game types, isDebug, isFlat
    let at = cur.position() as usize;
    let data_to_keep = *payload.get(at)?;

    let mut moved = Vec::with_capacity(payload.len());
    moved.extend_from_slice(&payload[..at]);
    moved.extend_from_slice(payload.get(at + 1..)?);
    moved.push(data_to_keep);
    translate_respawn_765(id, &moved, state)
}

/// Rewrites `forget_level_chunk`: 1.20.2 packed the two coordinate ints into a
/// `ChunkPos` long, whose big-endian bytes put z ahead of x.
pub(super) fn translate_forget_level_chunk_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload.get(4..8)?);
    out.extend_from_slice(payload.get(..4)?);
    Some(out)
}

/// Rewrites `block_entity_data`, whose tag is written straight through
/// `writeNbt` and so carries the pre-1.20.2 root name. The chunk's inline
/// block entities go through [`copy_block_entities`] instead.
pub(super) fn translate_block_entity_data_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    copy_bytes(&mut cur, &mut out, 8)?; // block pos
    copy_varint(&mut cur, &mut out)?; // block entity type
    copy_unnamed_nbt(&mut cur, &mut out, true)?;
    Some(out)
}

/// Rewrites `tag_query`, the other packet writing a bare `writeNbt` tag.
/// Pomme never sends the queries that prompt one, but the layout is covered
/// rather than left as a trap.
pub(super) fn translate_tag_query_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    copy_varint(&mut cur, &mut out)?; // transaction id
    copy_unnamed_nbt(&mut cur, &mut out, true)?;
    Some(out)
}

/// Rewrites 1.20.1's `add_player` onto `add_entity`, which 1.20.2 started
/// spawning players with. The head yaw repeats the body yaw, as
/// `ClientPacketListener.handleAddPlayer` did; the data field and velocity are
/// zero, neither being read for a player. Emits the wire version's own
/// `add_entity`, which the caller feeds back through the chain.
pub(super) fn translate_add_player_763(ids: &Ids763, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    advance(&mut cur, 16)?; // uuid
    let head_end = cur.position() as usize;
    advance(&mut cur, 24)?; // position doubles
    let pos_end = cur.position() as usize;
    let y_rot = read_u8(&mut cur)?;
    let x_rot = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, ids.add_entity_old_id);
    out.extend_from_slice(&payload[..head_end]);
    wire::write_varint(&mut out, ids.player_entity_type);
    out.extend_from_slice(&payload[head_end..pos_end]);
    out.extend_from_slice(&[x_rot, y_rot, y_rot]);
    wire::write_varint(&mut out, 0); // data
    out.extend_from_slice(&[0; 6]); // velocity shorts
    Some(out)
}

/// Advances past a 1.20.1 `login` payload's prefix and level list, leaving the
/// cursor on the registry codec.
pub(super) fn seek_login_codec_763(cur: &mut Cursor<&[u8]>) -> Option<()> {
    advance(cur, LOGIN_PREFIX_763)?;
    let levels = var_u32(cur)?;
    for _ in 0..levels {
        skip_utf(cur)?;
    }
    Some(())
}

/// Rewrites the 1.20.1 game `login` payload into the 765 form: the registry
/// codec rides inline here (read separately, before the game phase), and the
/// spawn-info fields hadn't been gathered into `CommonPlayerSpawnInfo` yet.
pub(super) fn translate_game_login_763(payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    seek_login_codec_763(&mut cur)?;
    let levels_end = cur.position() as usize;
    skip_nbt_root(&mut cur, true)?; // the registry codec, read before the join
    let spawn_at = cur.position() as usize;
    skip_utf(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 8)?; // seed
    // Also where the counts begin.
    let spawn_end = cur.position() as usize;
    varint_span(&mut cur)?; // max players
    varint_span(&mut cur)?; // chunk radius
    varint_span(&mut cur)?; // simulation distance
    advance(&mut cur, 2)?; // reducedDebugInfo, showDeathScreen
    let counts_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len());
    out.extend_from_slice(&payload[..LOGIN_HEAD_763]);
    out.extend_from_slice(&payload[LOGIN_PREFIX_763..levels_end]);
    out.extend_from_slice(&payload[spawn_end..counts_end]);
    out.push(0); // doLimitedCrafting, absent at 763
    out.extend_from_slice(&payload[spawn_at..spawn_end]);
    out.extend_from_slice(&payload[LOGIN_HEAD_763..LOGIN_PREFIX_763]);
    out.extend_from_slice(payload.get(counts_end..)?); // isDebug .. portalCooldown
    Some(out)
}
