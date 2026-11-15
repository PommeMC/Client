use super::*;

/// How a wire version's `update_advancements` differs from 26.2's.
#[derive(Clone, Copy)]
pub(super) struct AdvancementLayout {
    /// Through 1.21.11 a display's icon is a non-empty `ItemStack` (count,
    /// item, patch) rather than 26.1's `ItemStackTemplate`.
    pub icon_count_first: bool,
    /// Pre-1.21.9 `profile` component layout.
    pub old_profile: bool,
    /// 26.3 moved the screen position (two floats) out of `DisplayInfo` to
    /// a pair after each entry, sent even without a display.
    pub position_after_entry: bool,
    /// Through 1.21.4 the packet lacks the trailing `showAdvancements`
    /// bool, true in every vanilla send path.
    pub append_show: bool,
}

/// Rewrites `update_advancements` into 26.2's layout: each display's icon
/// becomes an `ItemStackTemplate` (item, count, patch) in native ids, and the
/// rest follows `layout`. An icon whose item the native version lacks takes
/// its display with it, so that advancement is tracked but not shown.
/// TODO: without an item grammar (1.20.5 to 1.21.11), an icon carrying a
/// component [`translate_component_patch`] can't walk drops the packet.
pub(super) fn translate_update_advancements(
    id: u32,
    payload: &[u8],
    layout: AdvancementLayout,
    items: Option<&ItemIds>,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    let copy = |cur: &Cursor<&[u8]>, out: &mut Vec<u8>, from: usize| {
        out.extend_from_slice(&payload[from..cur.position() as usize]);
    };

    advance(&mut cur, 1)?; // reset
    let count = var_u32(&mut cur)?;
    copy(&cur, &mut out, 0);
    for _ in 0..count {
        let entry_at = cur.position() as usize;
        skip_utf(&mut cur)?; // id
        skip_optional(&mut cur, skip_utf)?; // parent
        copy(&cur, &mut out, entry_at);

        let mut display = Vec::new();
        let shown = read_u8(&mut cur)? != 0 && {
            let text_at = cur.position() as usize;
            skip_nbt(&mut cur)?; // title
            skip_nbt(&mut cur)?; // description
            copy(&cur, &mut display, text_at);
            let icon = translate_icon(&mut cur, &mut display, layout, items, remaps)?;
            let tail_at = cur.position() as usize;
            varint_span(&mut cur)?; // type
            if read_i32(&mut cur)? & 1 != 0 {
                skip_utf(&mut cur)?; // background
            }
            if !layout.position_after_entry {
                advance(&mut cur, 8)?; // x, y
            }
            copy(&cur, &mut display, tail_at);
            icon
        };

        let rest_at = cur.position() as usize;
        for _ in 0..var_u32(&mut cur)? {
            for _ in 0..var_u32(&mut cur)? {
                skip_utf(&mut cur)?; // requirement
            }
        }
        advance(&mut cur, 1)?; // sends telemetry
        let rest_end = cur.position() as usize;
        if layout.position_after_entry {
            advance(&mut cur, 8)?; // x, y
            display.extend_from_slice(&payload[rest_end..rest_end + 8]);
        }

        out.push(u8::from(shown));
        if shown {
            out.extend_from_slice(&display);
        }
        out.extend_from_slice(&payload[rest_at..rest_end]);
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    if layout.append_show {
        out.push(1);
    }
    Some(out)
}

/// Writes a display's icon as a native `ItemStackTemplate`; `Some(false)`
/// (nothing written) when the native version lacks the item.
fn translate_icon(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    layout: AdvancementLayout,
    items: Option<&ItemIds>,
    remaps: &RegistryRemaps,
) -> Option<bool> {
    let (item, count) = read_item_and_count(cur, layout.icon_count_first)?;
    let mut patch = Vec::new();
    match items {
        Some(items) => items.items(remaps).patch(cur, &mut patch)?,
        None => translate_component_patch(cur, &mut patch, remaps, layout.old_profile)?,
    }
    let Some(item) = remaps.remap(ClientRegistry::Item, item) else {
        return Some(false);
    };
    wire::write_varint(out, item);
    wire::write_varint(out, count as u32);
    out.extend_from_slice(&patch);
    Some(true)
}
