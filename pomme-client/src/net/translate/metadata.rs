use super::*;

/// Rewrites `set_entity_data` (`entityId`, then `(u8 index, varint
/// serializer, value)` entries terminated by `0xFF`) by remapping each
/// entry's serializer id through the wire version's `serializer_map`. Value
/// layouts are identical between the versions (verified serializer by
/// serializer); they're skipped, not decoded, except particle values, whose
/// type ids are remapped in place ([`translate_particles`]). An entry using
/// a serializer the native version dropped (1.21.8's `compound_tag`) is
/// stripped rather than failing the packet. An item-stack value can't
/// always be walked without full component codecs — the remainder is copied
/// verbatim, which is correct unless a shifted serializer follows one (no
/// vanilla entity sends one after an untranslatable stack).
/// TODO: translate shoulder-parrot NBT to 26.2's OptionalInt variant
/// instead of stripping it, so shoulder parrots show on old servers.
/// TODO: lift pre-1.21.6 hanging-entity indices (an item frame's item/rotation
/// at 8/9, a painting's variant at 8) into 26.2's numbering instead of passing
/// the index byte through, so those entities can render on those wires.
pub(super) fn translate_entity_data(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    loop {
        let index = read_u8(&mut cur)?;
        if index == 0xFF {
            out.push(index);
            break;
        }
        let old = var_u32(&mut cur)?;
        let Some(new) = ids.serializer_map.to_native(old) else {
            if ids
                .serializer_map
                .names()
                .get(old as usize)
                .is_some_and(|name| name == "compound_tag")
            {
                // compound_tag values are network NBT; drop the entry.
                skip_nbt_root(&mut cur, ids.v763.is_some())?;
                continue;
            }
            if ids
                .serializer_map
                .names()
                .get(old as usize)
                .is_some_and(|name| name == "dye_color")
            {
                // Only the cushion sends one, and its add_entity is dropped.
                varint_span(&mut cur)?;
                continue;
            }
            return None;
        };
        out.push(index);
        wire::write_varint(&mut out, new);
        let value_at = cur.position() as usize;
        if ids.v764.is_some() && matches!(new, 5 | 6) {
            // 764 component values are JSON strings; a skip would desync
            // the rest of the list, so a bad one drops the packet.
            let done = if new == 6 {
                copy_optional(&mut cur, &mut out, transcode_component)
            } else {
                transcode_component(&mut cur, &mut out)
            };
            done?;
            continue;
        }
        if new == 7 {
            if let Some(items) = &ids.items {
                items.items(remaps).stack(&mut cur, &mut out)?;
                continue;
            }
            let walked = if ids.v765.is_some() {
                translate_item_765(&mut cur, &mut out, ids.v763.is_some()).map(|()| true)
            } else {
                translate_item_stack(&mut cur, &mut out, remaps, ids.v772.is_some())
            };
            if walked == Some(true) {
                continue;
            }
            tracing::debug!("Copying entity data tail verbatim past an item stack");
            if walked.is_none() {
                out.extend_from_slice(&payload[value_at..]);
            }
            return Some(out);
        }
        if new == 16 || new == 17 {
            let mut particles = Vec::new();
            translate_particles(&mut cur, &mut particles, ids, remaps, new == 17)?;
            out.extend_from_slice(&particles);
            continue;
        }
        if !skip_metadata_value(&mut cur, new)? {
            tracing::debug!("Copying entity data tail verbatim past serializer {old}");
            out.extend_from_slice(&payload[value_at..]);
            return Some(out);
        }
        out.extend_from_slice(&payload[value_at..cur.position() as usize]);
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Rewrites a pre-1.21.9 `profile` component value (optional name, optional
/// uuid, property map) into 26.2's `ResolvableProfile`: an either bool
/// picking full/partial profile — the old triple matches the partial arm
/// byte for byte — plus a skin patch, empty here (four absent optionals).
pub(super) fn translate_old_profile(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let value_at = cur.position() as usize;
    skip_optional(cur, skip_utf)?; // name
    skip_optional(cur, |c| advance(c, 16))?; // uuid
    let properties = var_u32(cur)?;
    for _ in 0..properties {
        skip_utf(cur)?; // property name
        skip_utf(cur)?; // value
        skip_optional(cur, skip_utf)?; // signature
    }
    out.push(0); // either: the partial-profile arm
    out.extend_from_slice(&cur.get_ref()[value_at..cur.position() as usize]);
    out.extend_from_slice(&[0; 4]); // empty PlayerSkin.Patch
    Some(())
}

/// Advances past one entity-data value of the given native-version
/// serializer (the caller remaps first). `Some(false)` means the value (and
/// thus anything after it) can't be walked; `None` means the data is
/// malformed.
pub(super) fn skip_metadata_value(cur: &mut Cursor<&[u8]>, serializer: u32) -> Option<bool> {
    let Some(serializer) = pomme_protocol::entity_data::SERIALIZERS_26_2.get(serializer as usize)
    else {
        return Some(false);
    };
    Reader::on_cursor(cur, |r| serializer.skip_native_value(r)).ok()
}

/// Rewrites one particle (or, with `list`, a counted particle list):
/// each type id is remapped into 26.2's space, a color payload
/// (`entity_effect`/`tinted_leaves`, an ARGB int on wire versions that have
/// it) is copied through, and payload-less particles pass bare. `None` (a
/// particle 26.2 dropped, or a payload the walker can't rewrite) fails the
/// packet — a verbatim fallback would leave wire-space ids and, in entity
/// data, any following entries' shifted serializer ids in place.
/// TODO: rewrite the remaining `PAYLOAD_PARTICLES` payloads (block states,
/// dust colors, items) so entities and explosions using them translate.
pub(super) fn translate_particles(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &GameIds,
    remaps: &RegistryRemaps,
    list: bool,
) -> Option<()> {
    let count = if list {
        let count = var_u32(cur)?;
        wire::write_varint(out, count);
        count
    } else {
        1
    };
    for _ in 0..count {
        let old = var_u32(cur)?;
        let new = remaps.remap(ClientRegistry::ParticleType, old)?;
        wire::write_varint(out, new);
        let name = RegistryTable::native().name_of(ClientRegistry::ParticleType, new)?;
        match name {
            "entity_effect" | "tinted_leaves" if ids.color_particles => {
                let color_at = cur.position() as usize;
                advance(cur, 4)?;
                out.extend_from_slice(&cur.get_ref()[color_at..cur.position() as usize]);
            }
            n if PAYLOAD_PARTICLES.contains(&n) => {
                if ids.v777.is_none() {
                    tracing::debug!("Dropping a packet with an untranslatable {n} particle");
                    return None;
                }
                copy_particle_payload(cur, out, n, remaps)?;
            }
            _ => {}
        }
    }
    Some(())
}

/// Copies one 26.3 particle payload through (`net/minecraft/core/particles`
/// option codecs, byte-identical to 26.2's): block states stay in wire space
/// like every block id pomme reads, and an `item`'s ids are remapped. `None`
/// for a payload that can't be sized (an item component the native version
/// lacks).
pub(super) fn copy_particle_payload(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    name: &str,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let at = cur.position() as usize;
    match name {
        "block" | "block_crumble" | "block_marker" | "dust_pillar" | "falling_dust" | "shriek" => {
            varint_span(cur)?;
        }
        "dragon_breath" | "sculk_charge" | "entity_effect" | "tinted_leaves" | "flash"
        | "geyser" | "geyser_plume" => advance(cur, 4)?,
        "dust" | "effect" | "instant_effect" | "geyser_base" | "geyser_poof" => advance(cur, 8)?,
        "dust_color_transition" => advance(cur, 12)?,
        "trail" => {
            advance(cur, 28)?; // target, color
            varint_span(cur)?; // duration
        }
        "vibration" => {
            match var_u32(cur)? {
                0 => advance(cur, 8)?, // block position
                1 => {
                    varint_span(cur)?; // entity id
                    advance(cur, 4)?; // y offset
                }
                _ => return None,
            }
            varint_span(cur)?; // arrival ticks
        }
        "item" => {
            let item = var_u32(cur)?;
            wire::write_varint(out, remaps.remap(ClientRegistry::Item, item)?);
            let count = varint_span(cur)?;
            out.extend_from_slice(&cur.get_ref()[count]);
            return copy_component_patch(cur, out, remaps);
        }
        _ => return None,
    }
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    Some(())
}
