use super::*;

/// Remaps one entity-data item stack (count, item id, component patch) into
/// the native registry space; an item the native version lacks becomes an
/// empty stack. `Some(false)` means a patch [`translate_component_patch`]
/// can't walk: it and the rest of the list copy verbatim after the remapped
/// head, or the list ends at the emptied stack. `None` means a malformed
/// head; nothing is written.
/// TODO: the verbatim patch keeps wire-space component-type ids.
pub(super) fn translate_item_stack(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
    old_profile: bool,
) -> Option<bool> {
    let count_at = cur.position() as usize;
    let count = var_i32(cur)?;
    let count_end = cur.position() as usize;
    if count <= 0 {
        out.extend_from_slice(&cur.get_ref()[count_at..count_end]);
        return Some(true);
    }
    let item = var_u32(cur)?;
    let patch_at = cur.position() as usize;
    let mut patch = Vec::new();
    let walked = translate_component_patch(cur, &mut patch, remaps, old_profile).is_some();
    let Some(native) = remaps.remap(ClientRegistry::Item, item) else {
        wire::write_varint(out, 0);
        if !walked {
            out.push(0xFF);
        }
        return Some(walked);
    };
    out.extend_from_slice(&cur.get_ref()[count_at..count_end]);
    wire::write_varint(out, native);
    if walked {
        out.extend_from_slice(&patch);
    } else {
        out.extend_from_slice(&cur.get_ref()[patch_at..]);
    }
    Some(walked)
}

/// An `item_stack` display's stack, written as 26.2's `ItemStackTemplate`
/// (item id, count, patch). Through 1.21.11 the display wrapped a non-empty
/// `ItemStack`, which leads with the count instead.
pub(super) fn translate_item_stack_template(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ctx: RecipeCtx<'_>,
) -> Option<()> {
    // A non-empty `ItemStack` has a positive count; a template only rejects 0.
    let (item, count) = if ctx.layout.item_stack_count_first {
        let count = var_i32(cur).filter(|&count| count > 0)?;
        (var_u32(cur)?, count)
    } else {
        let item = var_u32(cur)?;
        (item, var_i32(cur).filter(|&count| count != 0)?)
    };
    wire::write_varint(out, ctx.remaps.remap(ClientRegistry::Item, item)?);
    wire::write_varint(out, count as u32);
    translate_component_patch(cur, out, ctx.remaps, ctx.layout.old_profile)
}

/// Remaps an item stack's component patch into the native registry space.
/// `old_profile` marks the pre-1.21.9 `profile` component layout (see
/// [`translate_old_profile`]). `None` means a component payload the walker
/// doesn't know (or a malformed patch).
pub(super) fn translate_component_patch(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
    old_profile: bool,
) -> Option<()> {
    let added_at = cur.position() as usize;
    let added = var_u32(cur)?;
    out.extend_from_slice(&cur.get_ref()[added_at..cur.position() as usize]);
    let removed_at = cur.position() as usize;
    let removed = var_u32(cur)?;
    out.extend_from_slice(&cur.get_ref()[removed_at..cur.position() as usize]);
    for _ in 0..added {
        let component = var_u32(cur)?;
        let native = remaps.remap(ClientRegistry::DataComponentType, component)?;
        wire::write_varint(out, native);
        let value_at = cur.position() as usize;
        match native {
            COMPONENT_MAP_ID => {
                varint_span(cur)?;
            }
            COMPONENT_PROFILE if old_profile => {
                translate_old_profile(cur, out)?;
                continue;
            }
            COMPONENT_PROFILE => {
                Profile::azalea_read(cur).ok()?;
            }
            _ => return None,
        }
        out.extend_from_slice(&cur.get_ref()[value_at..cur.position() as usize]);
    }
    for _ in 0..removed {
        let component = var_u32(cur)?;
        wire::write_varint(
            out,
            remaps.remap(ClientRegistry::DataComponentType, component)?,
        );
    }
    Some(())
}

impl ItemIds {
    pub(super) fn items<'a>(&self, remaps: &'a RegistryRemaps) -> Items<'a> {
        Items {
            codec: self.codec,
            remaps,
        }
    }

    pub(super) fn rewrite(&self, id: u32) -> Option<ItemRewrite> {
        Some(match id {
            i if i == self.container_set_content_id => translate_container_set_content,
            i if i == self.container_set_slot_id => translate_container_set_slot,
            i if i == self.set_cursor_item_id => translate_set_cursor_item,
            i if i == self.set_player_inventory_id => translate_set_player_inventory,
            i if i == self.set_equipment_id => translate_set_equipment,
            i if i == self.merchant_offers_id => translate_merchant_offers,
            _ => return None,
        })
    }
}

impl Items<'_> {
    /// Runs `read` on the rest of `cur`, advancing it past what was read.
    pub(super) fn read<T>(
        cur: &mut Cursor<&[u8]>,
        read: impl FnOnce(&mut Reader) -> DecodeResult<T>,
    ) -> Option<T> {
        Reader::on_cursor(cur, read).ok()
    }

    /// `ItemStack.OPTIONAL_STREAM_CODEC`.
    pub(super) fn stack(self, cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
        Self::read(cur, |r| {
            self.codec
                .translate_stack(r, out, ItemCodec::native(), self.remaps)
        })
    }

    /// `ItemStack.STREAM_CODEC`, which has no empty form: `Some(false)` when
    /// the native version lacks the item.
    pub(super) fn non_empty_stack(
        self,
        cur: &mut Cursor<&[u8]>,
        out: &mut Vec<u8>,
    ) -> Option<bool> {
        let at = out.len();
        self.stack(cur, out)?;
        Some(out[at..] != [0])
    }

    /// `DataComponentPatch.STREAM_CODEC`.
    pub(super) fn patch(self, cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
        Self::read(cur, |r| {
            self.codec
                .translate_patch(r, out, ItemCodec::native(), self.remaps)
        })
    }

    /// `ItemCost.STREAM_CODEC`: an item, a count and exact-component
    /// predicates, read in full; `Some(false)` (nothing written) when the
    /// native version lacks the item.
    pub(super) fn cost(self, cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<bool> {
        let item = var_u32(cur)?;
        let mut cost = Vec::new();
        copy_varint(cur, &mut cost)?; // count
        let predicates = var_u32(cur)?;
        let mut kept = (0, Vec::new());
        for _ in 0..predicates {
            let carried = Self::read(cur, |r| {
                self.codec.translate_typed_component(
                    r,
                    &mut kept.1,
                    ItemCodec::native(),
                    self.remaps,
                )
            })?;
            kept.0 += u32::from(carried);
        }
        let Some(item) = self.remaps.remap(ClientRegistry::Item, item) else {
            return Some(false);
        };
        wire::write_varint(out, item);
        out.extend_from_slice(&cost);
        wire::write_varint(out, kept.0);
        out.extend_from_slice(&kept.1);
        Some(true)
    }
}

/// Serverbound `set_creative_mode_slot` from pomme's native encoding (a
/// short slot, then an untrusted stack) into the wire version's ids.
pub(super) fn translate_creative_slot(
    old_id: u32,
    payload: &[u8],
    wire_codec: &'static ItemCodec,
    from_native: &RegistryRemaps,
) -> Vec<Vec<u8>> {
    let mut r = Reader::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, old_id);
    let translated = r.bytes(2).and_then(|slot| {
        out.extend_from_slice(slot);
        ItemCodec::native().translate_untrusted_stack(&mut r, &mut out, wire_codec, from_native)
    });
    match translated {
        Ok(()) => vec![out],
        Err(e) => {
            tracing::warn!("Suppressing an untranslatable creative slot: {e}");
            Vec::new()
        }
    }
}

/// The frame id, then `head` (bytes copied verbatim), then `body`.
pub(super) fn item_frame(
    id: u32,
    payload: &[u8],
    head: usize,
    body: impl FnOnce(&mut Cursor<&[u8]>, &mut Vec<u8>) -> Option<()>,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(head as u64);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload.get(..head)?);
    body(&mut cur, &mut out)?;
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// The length of a leading run of `varints` varints.
pub(super) fn varint_run_len(payload: &[u8], varints: usize) -> Option<usize> {
    let mut cur = Cursor::new(payload);
    for _ in 0..varints {
        varint_span(&mut cur)?;
    }
    Some(cur.position() as usize)
}

/// `container_set_content`: container id, state id, the slots, the carried
/// stack.
pub(super) fn translate_container_set_content(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, varint_run_len(payload, 2)?, |cur, out| {
        let slots = var_u32(cur)?;
        wire::write_varint(out, slots);
        for _ in 0..=slots {
            items.stack(cur, out)?;
        }
        Some(())
    })
}

/// `container_set_slot`: container id, state id, a short slot, the stack.
pub(super) fn translate_container_set_slot(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, varint_run_len(payload, 2)? + 2, |cur, out| {
        items.stack(cur, out)
    })
}

pub(super) fn translate_set_cursor_item(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, 0, |cur, out| items.stack(cur, out))
}

/// `set_player_inventory`: a slot, the stack.
pub(super) fn translate_set_player_inventory(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, varint_run_len(payload, 1)?, |cur, out| {
        items.stack(cur, out)
    })
}

/// `set_equipment`: the entity, then slot/stack pairs whose slot byte's high
/// bit continues the list.
pub(super) fn translate_set_equipment(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, varint_run_len(payload, 1)?, |cur, out| {
        loop {
            let slot = read_u8(cur)?;
            out.push(slot);
            items.stack(cur, out)?;
            if slot & 0x80 == 0 {
                return Some(());
            }
        }
    })
}

/// `merchant_offers`: per offer a base cost, the result, an optional second
/// cost and a numeric tail. An offer with an item the native version lacks
/// is left out, rather than shown cheaper or as a different trade.
/// TODO: leaving an offer out renumbers the ones after it; map the indices
/// once pomme sends trade selections (`select_trade`).
pub(super) fn translate_merchant_offers(
    id: u32,
    payload: &[u8],
    items: Items<'_>,
) -> Option<Vec<u8>> {
    item_frame(id, payload, varint_run_len(payload, 1)?, |cur, out| {
        let offers = var_u32(cur)?;
        let (mut kept, mut body) = (0, Vec::new());
        for _ in 0..offers {
            let mut offer = Vec::new();
            let base = items.cost(cur, &mut offer)?;
            let result = items.non_empty_stack(cur, &mut offer)?;
            let has_second = read_u8(cur)? != 0;
            offer.push(u8::from(has_second));
            let second = !has_second || items.cost(cur, &mut offer)?;
            let tail_at = cur.position() as usize;
            advance(cur, 25)?; // outOfStock, 4 ints, multiplier, demand
            offer.extend_from_slice(&cur.get_ref()[tail_at..cur.position() as usize]);
            if base && result && second {
                kept += 1;
                body.extend_from_slice(&offer);
            }
        }
        wire::write_varint(out, kept);
        out.extend_from_slice(&body);
        Some(())
    })
}

/// Copies a 26.3 component patch into native ids through 26.3's item
/// grammar, dropping components the native version lacks or lays out
/// differently.
pub(super) fn copy_component_patch(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let items = Items {
        codec: ItemCodec::for_protocol(777)?,
        remaps,
    };
    items.patch(cur, out)
}

pub(super) fn remap_with<T: Registry>(
    remaps: &RegistryRemaps,
    reg: ClientRegistry,
    value: &mut T,
) -> bool {
    match remaps.remap(reg, value.to_u32()).and_then(T::from_u32) {
        Some(v) => {
            *value = v;
            true
        }
        None => false,
    }
}

/// azalea's typed encoder always writes native-version component-type ids,
/// and `DataComponentPatch` is opaque (single entries can't be rewritten or
/// removed), so a patch touching any component the target version numbers
/// differently is cleared wholesale rather than sent misencoded.
pub(super) fn strip_untranslatable_components(remaps: &RegistryRemaps, data: &mut ItemStackData) {
    let translates = |kind: DataComponentKind| {
        remaps.remap(ClientRegistry::DataComponentType, kind.to_u32()) == Some(kind.to_u32())
    };
    if !data
        .component_patch
        .iter()
        .all(|(kind, _)| translates(kind))
    {
        tracing::warn!("Dropping creative item components the wire version numbers differently");
        data.component_patch = Default::default();
    }
}

/// Remaps a stack's item kind, clearing the stack when the target version
/// has no such item.
pub(super) fn remap_stack(remaps: &RegistryRemaps, stack: &mut ItemStack) {
    let cleared = match stack {
        ItemStack::Present(data) => !remap_with(remaps, ClientRegistry::Item, &mut data.kind),
        ItemStack::Empty => false,
    };
    if cleared {
        *stack = ItemStack::Empty;
    }
}
