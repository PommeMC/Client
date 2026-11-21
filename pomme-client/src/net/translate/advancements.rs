use super::*;

/// How a wire version's `update_advancements` differs from 26.2's, the
/// layout every version this rewrites shares its screen positions with.
#[derive(Clone, Copy)]
pub(super) struct AdvancementLayout {
    /// Through 1.21.11 a display's icon is a non-empty `ItemStack` (count,
    /// item, patch) rather than 26.1's `ItemStackTemplate`.
    pub icon_count_first: bool,
    /// Through 1.21.4 the packet lacks the trailing `showAdvancements`
    /// bool, true in every vanilla send path.
    pub append_show: bool,
}

/// Rewrites `update_advancements` into the native layout: each display's
/// icon becomes an `ItemStackTemplate` (item, count, patch) in native ids,
/// its screen position (two floats) moves out of `DisplayInfo` to after the
/// entry, where 26.3 sends one even without a display, and the rest follows
/// `layout`. An icon whose item the native version lacks takes its display
/// with it, so that advancement is tracked but not shown.
pub(super) fn translate_update_advancements(
    id: u32,
    payload: &[u8],
    layout: AdvancementLayout,
    items: Items<'_>,
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
        let mut position = [0; 8];
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
            copy(&cur, &mut display, tail_at);
            let position_at = cur.position() as usize;
            advance(&mut cur, 8)?; // x, y
            position.copy_from_slice(&payload[position_at..position_at + 8]);
            icon
        };

        let rest_at = cur.position() as usize;
        for _ in 0..var_u32(&mut cur)? {
            for _ in 0..var_u32(&mut cur)? {
                skip_utf(&mut cur)?; // requirement
            }
        }
        advance(&mut cur, 1)?; // sends telemetry
        out.push(u8::from(shown));
        if shown {
            out.extend_from_slice(&display);
        }
        out.extend_from_slice(&payload[rest_at..cur.position() as usize]);
        out.extend_from_slice(&position);
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
    items: Items<'_>,
    remaps: &RegistryRemaps,
) -> Option<bool> {
    let (item, count) = read_item_and_count(cur, layout.icon_count_first)?;
    let mut patch = Vec::new();
    items.patch(cur, &mut patch)?;
    let Some(item) = remaps.remap(ClientRegistry::Item, item) else {
        return Some(false);
    };
    wire::write_varint(out, item);
    wire::write_varint(out, count as u32);
    out.extend_from_slice(&patch);
    Some(true)
}
