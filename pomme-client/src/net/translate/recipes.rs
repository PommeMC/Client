use super::*;

impl Translation {
    pub(super) fn recipe_translator(&self, id: u32) -> Option<RecipeTranslator> {
        if id == self.recipe_book_add_id {
            Some(translate_recipe_book_add)
        } else if id == self.update_recipes_id {
            Some(translate_update_recipes)
        } else if id == self.place_ghost_recipe_id {
            Some(translate_place_ghost_recipe)
        } else {
            None
        }
    }
}

pub(super) fn translate_recipe_book_add(
    id: u32,
    payload: &[u8],
    ctx: RecipeCtx<'_>,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);

    let entries = copy_varint(&mut cur, &mut out)?;
    for _ in 0..entries {
        copy_varint(&mut cur, &mut out)?; // RecipeDisplayId
        translate_recipe_display(&mut cur, &mut out, ctx)?;
        copy_varint(&mut cur, &mut out)?; // OptionalInt wire sentinel
        copy_varint(&mut cur, &mut out)?; // RecipeBookCategory
        copy_optional(&mut cur, &mut out, |cur, out| {
            let requirements = copy_varint(cur, out)?;
            for _ in 0..requirements {
                translate_item_holder_set(cur, out, ctx.remaps)?;
            }
            Some(())
        })?;
        copy_bytes(&mut cur, &mut out, 1)?; // entry flags
    }
    copy_bytes(&mut cur, &mut out, 1)?; // replace
    (cur.position() as usize == payload.len()).then_some(out)
}

/// Normalizes the post-1.21.2 recipe property sets and stonecutter entries.
pub(super) fn translate_update_recipes(
    id: u32,
    payload: &[u8],
    ctx: RecipeCtx<'_>,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);

    let property_sets = copy_varint(&mut cur, &mut out)?;
    for _ in 0..property_sets {
        copy_utf(&mut cur, &mut out)?;
        let items = copy_varint(&mut cur, &mut out)?;
        for _ in 0..items {
            let item = var_u32(&mut cur)?;
            wire::write_varint(&mut out, ctx.remaps.remap(ClientRegistry::Item, item)?);
        }
    }

    let stonecutter = copy_varint(&mut cur, &mut out)?;
    for _ in 0..stonecutter {
        translate_item_holder_set(&mut cur, &mut out, ctx.remaps)?;
        translate_slot_display(&mut cur, &mut out, ctx, MAX_SLOT_DISPLAY_DEPTH)?;
    }
    (cur.position() as usize == payload.len()).then_some(out)
}

pub(super) fn translate_place_ghost_recipe(
    id: u32,
    payload: &[u8],
    ctx: RecipeCtx<'_>,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    copy_varint(&mut cur, &mut out)?; // container id
    translate_recipe_display(&mut cur, &mut out, ctx)?;
    (cur.position() as usize == payload.len()).then_some(out)
}

pub(super) fn translate_recipe_display(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ctx: RecipeCtx<'_>,
) -> Option<()> {
    let source = var_u32(cur)?;
    let display = ctx.remaps.remap(ClientRegistry::RecipeDisplay, source)?;
    wire::write_varint(out, display);
    let slots = |cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, n: usize| {
        (0..n).try_for_each(|_| translate_slot_display(cur, out, ctx, MAX_SLOT_DISPLAY_DEPTH))
    };
    match display {
        0 => {
            // Shapeless: ingredient displays, result, crafting station.
            translate_slot_display_vec(cur, out, ctx, MAX_SLOT_DISPLAY_DEPTH)?;
            slots(cur, out, 2)
        }
        1 => {
            // Shaped: width, height, ingredient displays, result, station.
            copy_varint(cur, out)?;
            copy_varint(cur, out)?;
            translate_slot_display_vec(cur, out, ctx, MAX_SLOT_DISPLAY_DEPTH)?;
            slots(cur, out, 2)
        }
        2 => {
            // Furnace: ingredient, fuel, result, station, duration, xp.
            slots(cur, out, 4)?;
            copy_varint(cur, out)?;
            copy_bytes(cur, out, 4)
        }
        // Stonecutter: input, result, station.
        3 => slots(cur, out, 3),
        // Smithing: template, base, addition, result, station.
        4 => slots(cur, out, 5),
        _ => None,
    }
}

pub(super) fn translate_slot_display_vec(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ctx: RecipeCtx<'_>,
    depth: usize,
) -> Option<()> {
    let len = copy_varint(cur, out)?;
    (0..len).try_for_each(|_| translate_slot_display(cur, out, ctx, depth))
}

/// One slot display, `depth` levels of nesting allowed below it.
pub(super) fn translate_slot_display(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ctx: RecipeCtx<'_>,
    depth: usize,
) -> Option<()> {
    let depth = depth.checked_sub(1)?;
    let nested =
        |cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>| translate_slot_display(cur, out, ctx, depth);
    let source = var_u32(cur)?;
    let display = ctx.remaps.remap(ClientRegistry::SlotDisplay, source)?;
    if display == SMITHING_TRIM_DISPLAY && ctx.layout.trim_pattern_display {
        // 26.2 can't name a trim pattern by its template item, so keep the
        // untrimmed base: the nearest display it can carry.
        nested(cur, out)?;
        let mut discarded = Vec::new();
        nested(cur, &mut discarded)?; // material
        return nested(cur, &mut discarded); // pattern
    }
    wire::write_varint(out, display);
    match display {
        0 | 1 => Some(()),     // empty, any_fuel
        2 => nested(cur, out), // with_any_potion
        3 => {
            // only_with_component
            nested(cur, out)?;
            let component = var_u32(cur)?;
            wire::write_varint(
                out,
                ctx.remaps
                    .remap(ClientRegistry::DataComponentType, component)?,
            );
            Some(())
        }
        4 => {
            let item = var_u32(cur)?;
            wire::write_varint(out, ctx.remaps.remap(ClientRegistry::Item, item)?);
            Some(())
        }
        5 => translate_item_stack_template(cur, out, ctx),
        6 => copy_utf(cur, out), // tag resource location
        7 | 9 => {
            // dyed (dye, target), with_remainder (input, remainder)
            nested(cur, out)?;
            nested(cur, out)
        }
        SMITHING_TRIM_DISPLAY => {
            nested(cur, out)?;
            nested(cur, out)?;
            translate_trim_pattern_holder(cur, out)
        }
        10 => translate_slot_display_vec(cur, out, ctx, depth),
        _ => None,
    }
}

/// One `Holder<TrimPattern>`: a registry id + 1, or 0 and an inline pattern
/// (asset id, description, decal).
pub(super) fn translate_trim_pattern_holder(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
) -> Option<()> {
    let holder = var_u32(cur)?;
    wire::write_varint(out, holder);
    if holder != 0 {
        // Registry references use the connection's dynamic trim-pattern
        // registry directly; there is no static client remap to apply.
        return Some(());
    }

    copy_utf(cur, out)?; // asset id
    let description_at = cur.position() as usize;
    Reader::on_cursor(cur, Reader::nbt).ok()?; // description
    out.extend_from_slice(&cur.get_ref()[description_at..cur.position() as usize]);
    copy_bytes(cur, out, 1) // decal
}
