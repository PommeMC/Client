use super::*;

impl Ids777 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.move_entity_pos_id => translate_move_entity_pos_777,
            i if i == self.move_entity_pos_rot_id => translate_move_entity_pos_rot_777,
            i if i == self.move_entity_rot_id => translate_move_entity_rot_777,
            i if i == self.entity_position_sync_id => translate_entity_position_sync_777,
            i if i == self.login_id => translate_login_777,
            i if i == self.respawn_id => translate_respawn_777,
            i if i == self.animate_id => translate_animate_777,
            i if i == self.level_chunk_id => translate_chunk_777,
            i if i == self.light_update_id => translate_light_update_777,
            i if i == self.player_chat_id => translate_player_chat_777,
            _ => return None,
        })
    }
}

/// Rewrites `level_chunk_with_light` from 26.3, whose chunk data is
/// byte-identical and whose light masks changed codec (see
/// [`copy_light_data_777`]).
pub(super) fn translate_chunk_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints
    skip_heightmaps(&mut cur)?;
    let buffer_len = var_u32(&mut cur)?;
    advance(&mut cur, buffer_len as usize)?;

    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    copy_block_entities(&mut cur, &mut out, false)?;
    copy_light_data_777(&mut cur, &mut out)?;
    Some(out)
}

/// Rewrites `light_update` from 26.3: chunk x/z varints, then light data.
pub(super) fn translate_light_update_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?;
    varint_span(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    copy_light_data_777(&mut cur, &mut out)?;
    Some(out)
}

/// Copies `ClientboundLightUpdatePacketData`: 26.3 sends its four section
/// masks with `ByteBufCodecs.BIT_SET` where 26.2 wrote long arrays; the two
/// light-layer lists after them copy verbatim.
pub(super) fn copy_light_data_777(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    for _ in 0..4 {
        repack_bit_set(cur, out)?;
    }
    out.extend_from_slice(&cur.get_ref()[cur.position() as usize..]);
    Some(())
}

/// Rewrites `move_entity_pos` / `move_entity_pos_rot`: 26.3 packs onGround
/// (bit 0) and the step count (the rest) into a `properties` varint after
/// the entity id, followed by a `VecDelta`; 26.2 reads three shorts, the
/// rotation bytes if any, and a trailing onGround bool.
pub(super) fn translate_move_entity_777(
    id: u32,
    payload: &[u8],
    rotation: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let properties = var_u32(&mut cur)?;
    let delta = collapse_vec_delta(&mut cur, properties >> 1)?;
    let rot_at = cur.position() as usize;
    if rotation {
        advance(&mut cur, 2)?;
    }

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    for c in delta {
        out.extend_from_slice(&c.to_be_bytes());
    }
    out.extend_from_slice(&payload[rot_at..cur.position() as usize]);
    out.push((properties & 1) as u8);
    Some(out)
}

pub(super) fn translate_move_entity_pos_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    translate_move_entity_777(id, payload, false)
}

pub(super) fn translate_move_entity_pos_rot_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    translate_move_entity_777(id, payload, true)
}

/// Rewrites `move_entity_rot`: 26.3 moved onGround ahead of the rotation
/// bytes.
pub(super) fn translate_move_entity_rot_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let on_ground = read_u8(&mut cur)?;
    let rot_at = cur.position() as usize;
    advance(&mut cur, 2)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(&payload[rot_at..rot_at + 2]);
    out.push(on_ground);
    Some(out)
}

/// Rewrites `entity_position_sync`: 26.3 replaced the position and delta
/// doubles with a `PositionPath`. The delta is synthesized as zero; the
/// handler leaves velocity alone for this packet.
pub(super) fn translate_entity_position_sync_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let end = read_position_path_end(&mut cur)?;
    let tail_at = cur.position() as usize;
    advance(&mut cur, 9)?; // yRot, xRot, onGround

    let mut out = Vec::with_capacity(payload.len() + 24);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(end);
    out.extend_from_slice(&[0; 24]); // zero delta movement
    out.extend_from_slice(&payload[tail_at..]);
    Some(out)
}

/// Rewrites `explode` from 26.3: the particle and sound ids (the weighted
/// block-particle list's too) move into native space, and the trailing
/// `playSound` bool 26.3 appended is dropped.
/// TODO: honour a false `playSound` once explosions render.
pub(super) fn translate_explode_777(
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

/// `login` carries its spawn info after the player id, hardcore flag, level
/// list, three varints and three bools.
pub(super) fn translate_login_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
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
    rewrite_previous_game_type(id, payload, cur.position() as usize)
}

pub(super) fn translate_respawn_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    rewrite_previous_game_type(id, payload, 0)
}

/// Rewrites `animate`: 26.3 moved the swings to `swing_animation` (see
/// [`translate_swing_animation_777`]) and renumbered the rest — wake up 2 ->
/// 0, critical hit 4 -> 1, magic critical hit 5 -> 2.
pub(super) fn translate_animate_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (&action, entity) = payload.split_last()?;
    let action = match action {
        0 => 2,
        1 => 4,
        2 => 5,
        _ => return None,
    };
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(entity);
    out.push(action);
    Some(out)
}

/// Rewrites 26.3's `swing_animation` (`entityId, hand, {type, duration}`)
/// onto `animate` with the swing action 26.2 keyed by hand (0 main, 3 off).
/// A `none` type animates nothing and drops.
pub(super) fn translate_swing_animation_777(animate_id: u32, payload: &[u8]) -> Option<Box<[u8]>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let hand = var_u32(&mut cur)?;
    if var_u32(&mut cur)? == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(entity.end + 2);
    wire::write_varint(&mut out, animate_id);
    out.extend_from_slice(&payload[entity]);
    out.push(if hand == 0 { 0 } else { 3 });
    Some(out.into_boxed_slice())
}

/// Rewrites `level_particles`: 26.3 leads with the particle, splits
/// `maxSpeed` per axis, sends `count` as a varint and appends a
/// randomization type; 26.2 ends with the particle and reads one speed (the
/// x axis here — vanilla servers send one value on all three) and an int
/// count. The particle keeps its wire-space id, remapped by the raw handler
/// like every version's, so its payload is only sized, never rewritten.
/// TODO: per-axis speed and the randomization type once particles use them.
pub(super) fn translate_level_particles_777(
    id: u32,
    payload: &[u8],
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let particle = var_u32(&mut cur)?;
    let name = wire_registries.name_of(ClientRegistry::ParticleType, particle)?;
    if PAYLOAD_PARTICLES.contains(&name) {
        copy_particle_payload(&mut cur, &mut Vec::new(), name, remaps)?;
    }
    let particle_end = cur.position() as usize;
    advance(&mut cur, 38)?; // flags, position, spread
    let speed_at = cur.position() as usize;
    advance(&mut cur, 12)?;
    let count = var_u32(&mut cur)?;
    varint_span(&mut cur)?; // randomization type

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[particle_end..speed_at + 4]);
    out.extend_from_slice(&(count as i32).to_be_bytes());
    out.extend_from_slice(&payload[..particle_end]);
    Some(out)
}

/// Rewrites `update_advancements`: 26.3 moved an advancement's screen
/// position (two floats) out of `DisplayInfo` to a pair after each entry,
/// sent even without a display. 26.2 reads the pair at the end of the
/// display, so each entry is walked to put it back or drop it; the rest of
/// the packet copies verbatim. Item ids stay in wire space like every
/// version's.
/// TODO: remap advancement icon items in `remap_inbound`.
pub(super) fn translate_update_advancements_777(
    id: u32,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?; // reset
    let count = var_u32(&mut cur)?;
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..count {
        let entry_at = cur.position() as usize;
        skip_utf(&mut cur)?; // id
        skip_optional(&mut cur, skip_utf)?; // parent
        let display_end = if read_u8(&mut cur)? != 0 {
            skip_display_info(&mut cur, remaps)?;
            Some(cur.position() as usize)
        } else {
            None
        };
        for _ in 0..var_u32(&mut cur)? {
            for _ in 0..var_u32(&mut cur)? {
                skip_utf(&mut cur)?; // requirement
            }
        }
        advance(&mut cur, 1)?; // sends telemetry
        let entry_end = cur.position() as usize;
        advance(&mut cur, 8)?; // x, y
        match display_end {
            Some(end) => {
                out.extend_from_slice(&payload[entry_at..end]);
                out.extend_from_slice(&payload[entry_end..entry_end + 8]);
                out.extend_from_slice(&payload[end..entry_end]);
            }
            None => out.extend_from_slice(&payload[entry_at..entry_end]),
        }
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Holds a 26.3 `accept_teleportation` back: 26.3 appends the accepted pose
/// and no longer follows it with a `move_player_pos_rot`
/// (`ClientPacketListener.handleMovePlayer`), whose pose pomme sends right
/// after the accept.
pub(super) fn translate_accept_teleportation_777(
    state: &mut SessionState,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    let mut p = 0;
    state.pending_accept = wire::read_varint(payload, &mut p);
    Vec::new()
}

/// Folds the `move_player_pos_rot` following a held-back accept into it
/// (`id, x, y, z, yRot, xRot`); `None` for an ordinary move.
pub(super) fn translate_move_player_pos_rot_777(
    v: &Ids777,
    payload: &[u8],
    state: &mut SessionState,
) -> Option<Vec<Vec<u8>>> {
    let teleport = state.pending_accept.take()?;
    let mut out = Vec::with_capacity(38);
    wire::write_varint(&mut out, v.accept_teleportation_old_id);
    wire::write_varint(&mut out, teleport);
    out.extend_from_slice(payload.get(..32)?); // x, y, z, yRot, xRot
    Some(vec![out])
}

/// Rewrites `swing` onto 26.3's payload-less `punch`, which is main-hand
/// only; an off-hand swing has no equivalent and is dropped.
pub(super) fn translate_swing_777(punch_old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut p = 0;
    if wire::read_varint(payload, &mut p) != Some(0) {
        tracing::debug!("Suppressing an off-hand swing 26.3 can't express");
        return Vec::new();
    }
    let mut out = Vec::with_capacity(1);
    wire::write_varint(&mut out, punch_old_id);
    vec![out]
}

/// Rewrites `player_action` for 26.3, which inserted `CHANGE_DESTROY_DIRECTION`
/// at action 1, shifting every later action up by one.
pub(super) fn translate_player_action_777(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut p = 0;
    let Some(action) = wire::read_varint(payload, &mut p) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, old_id);
    wire::write_varint(&mut out, action + u32::from(action >= 1));
    out.extend_from_slice(&payload[p..]);
    vec![out]
}

/// Rewrites `player_chat` from 26.3, whose `PARTIALLY_FILTERED` mask moved
/// to `ByteBufCodecs.BIT_SET` (`FilterMask`); everything around it copies
/// verbatim.
pub(super) fn translate_player_chat_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
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

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    if mask == 2 {
        repack_bit_set(&mut cur, &mut out)?;
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Copies one 26.3 `SlotDisplay`, recursively. Only `tag` changed: 26.3
/// sends a `HolderSet` where 26.2 sent the tag id, and 26.3's
/// `Ingredient.display()` routes direct item lists through it too, which
/// 26.2 sent as a `composite` of `item` displays. Concrete item ids are
/// normalized into native 26.2 registry space while named tags keep their id.
pub(super) fn copy_slot_display_777(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
    depth: usize,
) -> Option<()> {
    use ClientRegistry::SlotDisplay;
    let depth = depth.checked_sub(1)?;
    let kind = var_u32(cur)?;
    let name = wire_registries.name_of(SlotDisplay, kind)?;
    if name == "tag" {
        let set = var_u32(cur)?;
        if set > 0 {
            let native = RegistryTable::native();
            let item = native.id_of(SlotDisplay, "item")?;
            wire::write_varint(out, native.id_of(SlotDisplay, "composite")?);
            wire::write_varint(out, set - 1);
            for _ in 1..set {
                wire::write_varint(out, item);
                let source_item = var_u32(cur)?;
                wire::write_varint(out, remaps.remap(ClientRegistry::Item, source_item)?);
            }
            return Some(());
        }
    }
    wire::write_varint(out, remaps.remap(SlotDisplay, kind)?);
    // Nested displays lead every type that has any.
    let nested = match name {
        "with_any_potion" | "only_with_component" => 1,
        "dyed" | "with_remainder" | "smithing_trim" => 2,
        "composite" => copy_varint(cur, out)?,
        _ => 0,
    };
    for _ in 0..nested {
        copy_slot_display_777(cur, out, wire_registries, remaps, depth)?;
    }
    match name {
        "empty" | "any_fuel" | "with_any_potion" | "dyed" | "with_remainder" | "composite" => {}
        "tag" => copy_utf(cur, out)?,
        "item" => {
            let source_item = var_u32(cur)?;
            wire::write_varint(out, remaps.remap(ClientRegistry::Item, source_item)?);
        }
        "item_stack" => {
            let source_item = var_u32(cur)?;
            wire::write_varint(out, remaps.remap(ClientRegistry::Item, source_item)?);
            copy_varint(cur, out)?; // count
            copy_component_patch(cur, out, remaps)?;
        }
        "only_with_component" => {
            let component = var_u32(cur)?;
            let native = remaps.remap(ClientRegistry::DataComponentType, component)?;
            wire::write_varint(out, native);
        }
        "smithing_trim" => {
            // The pattern holder: id + 1, or 0 and an inline pattern.
            if copy_varint(cur, out)? == 0 {
                copy_utf(cur, out)?; // asset id
                copy_nbt(cur, out)?; // description
                copy_bytes(cur, out, 1)?; // decal
            }
        }
        _ => return None,
    }
    Some(())
}

/// Copies one 26.3 `RecipeDisplay`, rewriting its slot displays
/// (`*RecipeDisplay` stream codecs, unchanged from 26.2).
pub(super) fn copy_recipe_display_777(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let kind = var_u32(cur)?;
    let name = wire_registries.name_of(ClientRegistry::RecipeDisplay, kind)?;
    wire::write_varint(out, remaps.remap(ClientRegistry::RecipeDisplay, kind)?);
    let slots = |cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, n: u32| {
        (0..n).try_for_each(|_| {
            copy_slot_display_777(cur, out, wire_registries, remaps, MAX_SLOT_DISPLAY_DEPTH)
        })
    };
    match name {
        // Ingredients, then the result and crafting station.
        "crafting_shapeless" => {
            let ingredients = copy_varint(cur, out)?;
            slots(cur, out, ingredients + 2)
        }
        "crafting_shaped" => {
            copy_varint(cur, out)?; // width
            copy_varint(cur, out)?; // height
            let ingredients = copy_varint(cur, out)?;
            slots(cur, out, ingredients + 2)
        }
        "furnace" => {
            slots(cur, out, 4)?; // ingredient, fuel, result, station
            copy_varint(cur, out)?; // duration
            copy_bytes(cur, out, 4) // experience
        }
        "stonecutter" => slots(cur, out, 3),
        "smithing" => slots(cur, out, 5),
        _ => None,
    }
}

/// Rewrites `recipe_book_add` from 26.3: each entry's display is rewritten
/// (see [`copy_slot_display_777`]); the ingredient `HolderSet`s of its
/// crafting requirements kept their codec and copy through.
pub(super) fn translate_recipe_book_add_777(
    id: u32,
    payload: &[u8],
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        copy_varint(&mut cur, &mut out)?; // display id
        copy_recipe_display_777(&mut cur, &mut out, wire_registries, remaps)?;
        copy_varint(&mut cur, &mut out)?; // group (optional varint)
        copy_varint(&mut cur, &mut out)?; // category
        copy_optional(&mut cur, &mut out, |cur, out| {
            (0..copy_varint(cur, out)?)
                .try_for_each(|_| translate_item_holder_set(cur, out, remaps))
        })?;
        copy_bytes(&mut cur, &mut out, 1)?; // flags
    }
    copy_bytes(&mut cur, &mut out, 1)?; // replace
    (cur.position() as usize == payload.len()).then_some(out)
}

/// Rewrites `place_ghost_recipe` from 26.3: container id, then a display.
pub(super) fn translate_place_ghost_recipe_777(
    id: u32,
    payload: &[u8],
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 32);
    wire::write_varint(&mut out, id);
    copy_varint(&mut cur, &mut out)?;
    copy_recipe_display_777(&mut cur, &mut out, wire_registries, remaps)?;
    (cur.position() as usize == payload.len()).then_some(out)
}

/// Rewrites `update_recipes` from 26.3: the property sets copy through, and
/// each stonecutter entry's option display is rewritten after its
/// ingredient `HolderSet` (`SelectableRecipe.SingleInputEntry`).
pub(super) fn translate_update_recipes_777(
    id: u32,
    payload: &[u8],
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        copy_utf(&mut cur, &mut out)?; // property set key
        for _ in 0..copy_varint(&mut cur, &mut out)? {
            let source_item = var_u32(&mut cur)?;
            wire::write_varint(&mut out, remaps.remap(ClientRegistry::Item, source_item)?);
        }
    }
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        translate_item_holder_set(&mut cur, &mut out, remaps)?;
        copy_slot_display_777(
            &mut cur,
            &mut out,
            wire_registries,
            remaps,
            MAX_SLOT_DISPLAY_DEPTH,
        )?;
    }
    (cur.position() as usize == payload.len()).then_some(out)
}
