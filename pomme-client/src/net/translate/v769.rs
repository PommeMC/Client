use super::*;

impl Ids769 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.player_chat_id => translate_player_chat,
            _ => return None,
        })
    }
}

/// Rewrites `add_experience_orb` (entity id, position, short value) onto the
/// wire version's `add_entity`: a UUID made from the id, no rotation or
/// velocity, and the value in the data field, which the orb reads at spawn.
/// The caller feeds it back through the chain.
pub(super) fn translate_add_experience_orb(ids: &Ids769, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = var_u32(&mut cur)?;
    let pos_at = cur.position() as usize;
    advance(&mut cur, 24)?; // position doubles
    let pos_end = cur.position() as usize;
    let value = i16::from_be_bytes([read_u8(&mut cur)?, read_u8(&mut cur)?]);

    let mut out = Vec::with_capacity(payload.len() + 32);
    wire::write_varint(&mut out, ids.add_entity_old_id);
    wire::write_varint(&mut out, entity);
    out.extend_from_slice(uuid::Uuid::from_u128(u128::from(entity) + 1).as_bytes());
    wire::write_varint(&mut out, ids.experience_orb_entity_type);
    out.extend_from_slice(&payload[pos_at..pos_end]);
    out.extend_from_slice(&[0; 3]); // x/y/head rotation
    wire::write_varint(&mut out, i32::from(value) as u32);
    out.extend_from_slice(&[0; 6]); // velocity shorts
    Some(out)
}

/// Rewrites a native `set_creative_mode_slot` payload for 1.20.5 to 1.21.4,
/// whose stack codec doesn't length-prefix each component value as 1.21.5's
/// `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC` does. Ids are already in the
/// wire version's space (`remap_outbound` runs before encoding).
pub(super) fn undelimit_creative_slot(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        let mut out = Vec::with_capacity(payload.len() + 4);
        wire::write_varint(&mut out, old_id);
        advance(&mut cur, 2)?; // slot short
        out.extend_from_slice(&payload[..2]);
        if copy_varint(&mut cur, &mut out)? == 0 {
            return Some(out);
        }
        copy_varint(&mut cur, &mut out)?; // item
        let added = copy_varint(&mut cur, &mut out)?;
        let removed = copy_varint(&mut cur, &mut out)?;
        for _ in 0..added {
            copy_varint(&mut cur, &mut out)?; // component type
            let len = var_u32(&mut cur)? as usize;
            let at = cur.position() as usize;
            advance(&mut cur, len)?;
            out.extend_from_slice(&payload[at..at + len]);
        }
        for _ in 0..removed {
            copy_varint(&mut cur, &mut out)?;
        }
        Some(out)
    };
    rewrite().into_iter().collect()
}

/// Rewrites a native `container_click` payload for 1.21.4, which carries
/// full item stacks where 1.21.5 hashes them (`ServerboundContainerClick-
/// Packet` in both references). A hash can't be reversed, so each stack is
/// reconstructed bare (item + count, no components); the server reconciles
/// any component mismatch by resyncing the slot. Item ids are already in
/// the wire version's space (`remap_hashed` runs before encoding).
pub(super) fn translate_container_click(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        // containerId, stateId varints; slot short, button byte, clickType
        // varint: identical on both sides.
        varint_span(&mut cur)?;
        varint_span(&mut cur)?;
        advance(&mut cur, 3)?;
        varint_span(&mut cur)?;

        let mut out = Vec::with_capacity(payload.len() + 8);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..cur.position() as usize]);

        let changed = var_u32(&mut cur)?;
        wire::write_varint(&mut out, changed);
        for _ in 0..changed {
            let slot_at = cur.position() as usize;
            advance(&mut cur, 2)?; // slot short
            out.extend_from_slice(&payload[slot_at..cur.position() as usize]);
            write_bare_stack(&mut out, read_hashed_stack(&mut cur)?);
        }
        write_bare_stack(&mut out, read_hashed_stack(&mut cur)?);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// Reads a `HashedStack` (present bool, item id, count, hashed added map,
/// removed set), returning `Some((item, count))` for a present stack.
pub(super) fn read_hashed_stack(cur: &mut Cursor<&[u8]>) -> Option<Option<(u32, u32)>> {
    if read_u8(cur)? == 0 {
        return Some(None);
    }
    let item = var_u32(cur)?;
    let count = var_u32(cur)?;
    let added = var_u32(cur)?;
    for _ in 0..added {
        varint_span(cur)?; // component id
        advance(cur, 4)?; // hash
    }
    let removed = var_u32(cur)?;
    for _ in 0..removed {
        varint_span(cur)?;
    }
    Some(Some((item, count)))
}

/// Writes a pre-1.21.5 optional item stack with no components: count, item,
/// empty added/removed patch (or the zero count marking empty).
pub(super) fn write_bare_stack(out: &mut Vec<u8>, stack: Option<(u32, u32)>) {
    match stack {
        Some((item, count)) => {
            wire::write_varint(out, count);
            wire::write_varint(out, item);
            wire::write_varint(out, 0);
            wire::write_varint(out, 0);
        }
        None => wire::write_varint(out, 0),
    }
}

/// Rewrites `player_chat`: 1.21.5 prepended a `globalIndex` varint, which an
/// older layout gets as zero (the chat reader does not check the index then).
pub(super) fn translate_player_chat(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, 0);
    out.extend_from_slice(payload);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `add_entity` bytes an old orb becomes: its id, a UUID from the id,
    /// the orb's type, the position, zero rotations, the value as the data
    /// varint and zero velocity shorts.
    #[test]
    fn an_old_orb_becomes_these_add_entity_bytes() {
        let ids = cached(769)
            .and_then(|t| t.game_ids.as_ref())
            .and_then(|g| g.v769.as_ref())
            .unwrap();
        let position: Vec<u8> = [1.5f64, 64.0, -2.5]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let mut payload = vec![42];
        payload.extend(&position);
        payload.extend(300i16.to_be_bytes());

        let mut expected = Vec::new();
        wire::write_varint(&mut expected, ids.add_entity_old_id);
        expected.push(42);
        expected.extend(43u128.to_be_bytes());
        wire::write_varint(&mut expected, ids.experience_orb_entity_type);
        expected.extend(&position);
        expected.extend([0, 0, 0]);
        expected.extend([0xAC, 0x02]); // 300
        expected.extend([0; 6]);
        assert_eq!(translate_add_experience_orb(ids, &payload), Some(expected));
    }
}
