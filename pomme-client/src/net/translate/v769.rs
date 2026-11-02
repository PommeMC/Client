use super::*;

impl Ids769 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.player_chat_id => translate_player_chat,
            i if i == self.update_advancements_id => translate_update_advancements,
            _ => return None,
        })
    }
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

/// Rewrites `player_chat`: 1.21.5 prepended a `globalIndex` varint; zero
/// keeps azalea's ordering checks happy.
pub(super) fn translate_player_chat(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, 0);
    out.extend_from_slice(payload);
    Some(out)
}

/// Rewrites `update_advancements`: 1.21.5 appended a `showAdvancements`
/// bool, true in every vanilla send path.
pub(super) fn translate_update_advancements(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload);
    out.push(1);
    Some(out)
}
