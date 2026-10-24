use std::io::Cursor;
use std::sync::OnceLock;

use azalea_protocol::common::tags::TagMap;
use azalea_registry::Registry;
use azalea_registry::builtin::{DataComponentKind, ItemKind};
use crossbeam_channel::Sender;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::ItemCodec;
use pomme_protocol::{Direction, PacketTable, Phase};

use crate::net::NetworkEvent;
use crate::player::inventory::item_resource_name;
use crate::recipe::{
    GhostRecipe, Ingredient, ItemStackTemplate, ItemTags, MAX_SLOT_DISPLAY_DEPTH,
    RecipeBookAddEntry, RecipeBookEntry, RecipeBookSettings, RecipeBookTypeSettings, RecipeData,
    RecipeDisplay, SlotDisplay, StonecutterRecipe, TrimPatternHolder,
};
use crate::ui::toast::RecipeToastEntry;

const MAX_COLLECTION: usize = 65_536;

pub fn item_tags(tags: &TagMap) -> ItemTags {
    let Some(items) = tags
        .iter()
        .find(|(registry, _)| registry.to_string() == "minecraft:item")
        .map(|(_, tags)| tags)
    else {
        return ItemTags::default();
    };

    ItemTags::from_entries(items.iter().map(|tag| {
        let elements = tag
            .elements
            .iter()
            .filter_map(|id| {
                let id = u32::try_from(*id).ok()?;
                match super::translate::active() {
                    Some(translation) => translation.remap_item_to_native(id),
                    None => Some(id),
                }
            })
            .collect();
        (tag.name.to_string(), elements)
    }))
}

#[derive(Clone, Copy)]
struct RecipePacketIds {
    add: u32,
    remove: u32,
    settings: u32,
    update: u32,
    ghost: u32,
}

fn packet_ids() -> RecipePacketIds {
    static IDS: OnceLock<RecipePacketIds> = OnceLock::new();
    *IDS.get_or_init(|| {
        let t = PacketTable::native();
        let id = |name| {
            t.id(Phase::Game, Direction::Clientbound, name)
                .unwrap_or_else(|| panic!("{name} in native game packet table"))
        };
        RecipePacketIds {
            add: id("recipe_book_add"),
            remove: id("recipe_book_remove"),
            settings: id("recipe_book_settings"),
            update: id("update_recipes"),
            ghost: id("place_ghost_recipe"),
        }
    })
}

/// Handles the recipe packets whose pinned Azalea structs do not match the
/// vanilla 26.2 wire format. The frame has already been translated into the
/// native packet/static-registry id space by
/// `Translation::translate_game_frame`.
///
/// `None` means this is not a recipe packet. Recipe packets are always consumed
/// here: malformed payloads become an error rather than falling through to an
/// incompatible typed decoder.
pub fn handle_raw_recipe_packet(
    packet_id: u32,
    cur: &mut Cursor<&[u8]>,
    event_tx: &Sender<NetworkEvent>,
) -> Option<Result<(), String>> {
    enum Parsed {
        Add(Vec<RecipeBookAddEntry>, bool),
        Remove(Vec<u32>),
        Settings(RecipeBookSettings),
        Update(RecipeData),
        Ghost(Box<GhostRecipe>),
    }

    let ids = packet_ids();
    let parsed = if packet_id == ids.add {
        parse_add(cur).map(|(entries, replace)| Parsed::Add(entries, replace))
    } else if packet_id == ids.remove {
        parse_remove(cur).map(Parsed::Remove)
    } else if packet_id == ids.settings {
        parse_settings(cur).map(Parsed::Settings)
    } else if packet_id == ids.update {
        parse_update(cur).map(Parsed::Update)
    } else if packet_id == ids.ghost {
        parse_ghost(cur).map(|ghost| Parsed::Ghost(Box::new(ghost)))
    } else {
        return None;
    };

    Some(parsed.and_then(|parsed| {
        ensure_eof(cur)?;
        match parsed {
            Parsed::Add(entries, replace) => {
                let toasts = entries
                    .iter()
                    .filter(|entry| entry.notification)
                    .map(|entry| toast_entry(&entry.contents.display))
                    .collect::<Vec<_>>();
                if !toasts.is_empty() {
                    let _ = event_tx.try_send(NetworkEvent::RecipeToastAdd { entries: toasts });
                }
                let _ = event_tx.try_send(NetworkEvent::RecipeBookAdd { entries, replace });
            }
            Parsed::Remove(ids) => {
                let _ = event_tx.try_send(NetworkEvent::RecipeBookRemove { ids });
            }
            Parsed::Settings(settings) => {
                let _ = event_tx.try_send(NetworkEvent::RecipeBookSettings(settings));
            }
            Parsed::Update(data) => {
                let _ = event_tx.try_send(NetworkEvent::RecipeData(data));
            }
            Parsed::Ghost(ghost) => {
                let _ = event_tx.try_send(NetworkEvent::GhostRecipe(ghost));
            }
        }
        Ok(())
    }))
}

fn parse_add(cur: &mut Cursor<&[u8]>) -> Result<(Vec<RecipeBookAddEntry>, bool), String> {
    let entries = read_list(cur, |cur| {
        let id = read_var_u32(cur)?;
        let display = parse_recipe_display(cur)?;
        let group_wire = read_var_u32(cur)?;
        let group = group_wire.checked_sub(1);
        let category = read_var_u32(cur)?;
        let crafting_requirements = if read_bool(cur)? {
            Some(read_list(cur, parse_ingredient)?)
        } else {
            None
        };
        let flags = read_u8(cur)?;
        Ok(RecipeBookAddEntry {
            contents: RecipeBookEntry {
                id,
                display,
                group,
                category,
                crafting_requirements,
            },
            notification: flags & 1 != 0,
            highlight: flags & 2 != 0,
        })
    })?;
    Ok((entries, read_bool(cur)?))
}

fn parse_remove(cur: &mut Cursor<&[u8]>) -> Result<Vec<u32>, String> {
    read_list(cur, read_var_u32)
}

fn parse_settings(cur: &mut Cursor<&[u8]>) -> Result<RecipeBookSettings, String> {
    let mut read = || -> Result<RecipeBookTypeSettings, String> {
        Ok(RecipeBookTypeSettings {
            open: read_bool(cur)?,
            filtering: read_bool(cur)?,
        })
    };
    Ok(RecipeBookSettings {
        crafting: read()?,
        furnace: read()?,
        blast_furnace: read()?,
        smoker: read()?,
    })
}

fn parse_update(cur: &mut Cursor<&[u8]>) -> Result<RecipeData, String> {
    let item_sets = read_list(cur, |cur| {
        Ok((read_identifier(cur)?, read_list(cur, read_item_id)?))
    })?
    .into_iter()
    .collect();
    let stonecutter_recipes = read_list(cur, |cur| {
        Ok(StonecutterRecipe {
            input: parse_ingredient(cur)?,
            option_display: parse_slot_display(cur, MAX_SLOT_DISPLAY_DEPTH)?,
        })
    })?;
    Ok(RecipeData {
        item_sets,
        stonecutter_recipes,
    })
}

fn parse_ghost(cur: &mut Cursor<&[u8]>) -> Result<GhostRecipe, String> {
    Ok(GhostRecipe {
        container_id: read_var_i32(cur)?,
        recipe: parse_recipe_display(cur)?,
    })
}

fn parse_recipe_display(cur: &mut Cursor<&[u8]>) -> Result<RecipeDisplay, String> {
    let slot = |cur: &mut Cursor<&[u8]>| parse_slot_display(cur, MAX_SLOT_DISPLAY_DEPTH);
    match read_var_u32(cur)? {
        0 => Ok(RecipeDisplay::Shapeless {
            ingredients: parse_slot_vec(cur, MAX_SLOT_DISPLAY_DEPTH)?,
            result: slot(cur)?,
            crafting_station: slot(cur)?,
        }),
        1 => Ok(RecipeDisplay::Shaped {
            width: read_var_u32(cur)?,
            height: read_var_u32(cur)?,
            ingredients: parse_slot_vec(cur, MAX_SLOT_DISPLAY_DEPTH)?,
            result: slot(cur)?,
            crafting_station: slot(cur)?,
        }),
        2 => Ok(RecipeDisplay::Furnace {
            ingredient: slot(cur)?,
            fuel: slot(cur)?,
            result: slot(cur)?,
            crafting_station: slot(cur)?,
            duration: read_var_u32(cur)?,
            experience: read(cur, Reader::f32)?,
        }),
        3 => Ok(RecipeDisplay::Stonecutter {
            input: slot(cur)?,
            result: slot(cur)?,
            crafting_station: slot(cur)?,
        }),
        4 => Ok(RecipeDisplay::Smithing {
            template: slot(cur)?,
            base: slot(cur)?,
            addition: slot(cur)?,
            result: slot(cur)?,
            crafting_station: slot(cur)?,
        }),
        id => Err(format!("unknown recipe_display registry id {id}")),
    }
}

fn parse_slot_vec(cur: &mut Cursor<&[u8]>, depth: usize) -> Result<Vec<SlotDisplay>, String> {
    read_list(cur, |cur| parse_slot_display(cur, depth))
}

/// One slot display, `depth` levels of nesting allowed below it.
fn parse_slot_display(cur: &mut Cursor<&[u8]>, depth: usize) -> Result<SlotDisplay, String> {
    let depth = depth
        .checked_sub(1)
        .ok_or_else(|| format!("slot display nested deeper than {MAX_SLOT_DISPLAY_DEPTH}"))?;
    let nested = |cur: &mut Cursor<&[u8]>| parse_slot_display(cur, depth).map(Box::new);
    match read_var_u32(cur)? {
        0 => Ok(SlotDisplay::Empty),
        1 => Ok(SlotDisplay::AnyFuel),
        2 => Ok(SlotDisplay::WithAnyPotion(nested(cur)?)),
        3 => Ok(SlotDisplay::OnlyWithComponent {
            contents: nested(cur)?,
            component: read_component_id(cur)?,
        }),
        4 => Ok(SlotDisplay::Item(read_item_id(cur)?)),
        5 => Ok(SlotDisplay::ItemStack(parse_item_stack_template(cur)?)),
        6 => Ok(SlotDisplay::Tag(read_identifier(cur)?)),
        7 => Ok(SlotDisplay::Dyed {
            dye: nested(cur)?,
            target: nested(cur)?,
        }),
        8 => Ok(SlotDisplay::SmithingTrim {
            base: nested(cur)?,
            material: nested(cur)?,
            trim_pattern: parse_trim_pattern(cur)?,
        }),
        9 => Ok(SlotDisplay::WithRemainder {
            input: nested(cur)?,
            remainder: nested(cur)?,
        }),
        10 => Ok(SlotDisplay::Composite(parse_slot_vec(cur, depth)?)),
        id => Err(format!("unknown slot_display registry id {id}")),
    }
}

fn parse_item_stack_template(cur: &mut Cursor<&[u8]>) -> Result<ItemStackTemplate, String> {
    let template =
        pomme_protocol::codec::Reader::on_cursor(cur, |r| ItemCodec::native().read_template(r))
            .map_err(|e| format!("item stack template: {e}"))?;
    Ok(ItemStackTemplate {
        item: template.item,
        count: template.count,
        components: super::item_codec::azalea_patch(&template.patch),
    })
}

fn parse_trim_pattern(cur: &mut Cursor<&[u8]>) -> Result<TrimPatternHolder, String> {
    let holder = read_var_u32(cur)?;
    if holder != 0 {
        return Ok(TrimPatternHolder::Reference(holder - 1));
    }
    Ok(TrimPatternHolder::Direct {
        asset_id: read_identifier(cur)?,
        description: Box::new(read_component(cur)?),
        decal: read_bool(cur)?,
    })
}

fn parse_ingredient(cur: &mut Cursor<&[u8]>) -> Result<Ingredient, String> {
    let encoded = read_var_u32(cur)?;
    if encoded == 0 {
        return Ok(Ingredient::Tag(read_identifier(cur)?));
    }
    let count = usize::try_from(encoded - 1).map_err(|_| "ingredient too large".to_owned())?;
    if count > MAX_COLLECTION {
        return Err(format!(
            "ingredient item count {count} exceeds {MAX_COLLECTION}"
        ));
    }
    let items = read_n(cur, count, read_item_id)?;
    // Vanilla `Ingredient`'s constructor rejects both.
    if items.is_empty() {
        return Err("ingredient can't be empty".to_owned());
    }
    if items.contains(&ItemKind::Air.to_u32()) {
        return Err("ingredient can't contain air".to_owned());
    }
    Ok(Ingredient::Items(items))
}

fn toast_entry(display: &RecipeDisplay) -> RecipeToastEntry {
    let (station, result) = match display {
        RecipeDisplay::Shapeless {
            crafting_station,
            result,
            ..
        }
        | RecipeDisplay::Shaped {
            crafting_station,
            result,
            ..
        }
        | RecipeDisplay::Furnace {
            crafting_station,
            result,
            ..
        }
        | RecipeDisplay::Stonecutter {
            crafting_station,
            result,
            ..
        }
        | RecipeDisplay::Smithing {
            crafting_station,
            result,
            ..
        } => (crafting_station, result),
    };
    RecipeToastEntry {
        category_item: slot_first_item(station),
        unlocked_item: slot_first_item(result),
    }
}

fn slot_first_item(slot: &SlotDisplay) -> Option<String> {
    let id = match slot {
        SlotDisplay::Empty | SlotDisplay::AnyFuel | SlotDisplay::Tag(_) => return None,
        SlotDisplay::Item(id) => *id,
        SlotDisplay::ItemStack(stack) => stack.item,
        SlotDisplay::WithAnyPotion(contents) | SlotDisplay::OnlyWithComponent { contents, .. } => {
            return slot_first_item(contents);
        }
        SlotDisplay::Dyed { target, .. } => return slot_first_item(target),
        SlotDisplay::SmithingTrim { base, .. } => return slot_first_item(base),
        SlotDisplay::WithRemainder { input, .. } => return slot_first_item(input),
        SlotDisplay::Composite(contents) => return contents.iter().find_map(slot_first_item),
    };
    azalea_registry::builtin::ItemKind::from_u32(id).map(item_resource_name)
}

/// A length-prefixed list of `read` elements.
fn read_list<T>(
    cur: &mut Cursor<&[u8]>,
    read: impl FnMut(&mut Cursor<&[u8]>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let len = read_len(cur)?;
    read_n(cur, len, read)
}

/// `len` elements. Every element takes at least one byte, so the up-front
/// allocation is capped by what's left of the frame: a forged length can't
/// reserve more than the packet could fill.
fn read_n<T>(
    cur: &mut Cursor<&[u8]>,
    len: usize,
    mut read: impl FnMut(&mut Cursor<&[u8]>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let remaining = cur.get_ref().len().saturating_sub(cur.position() as usize);
    let mut out = Vec::with_capacity(len.min(remaining));
    for _ in 0..len {
        out.push(read(cur)?);
    }
    Ok(out)
}

fn read_len(cur: &mut Cursor<&[u8]>) -> Result<usize, String> {
    let len =
        usize::try_from(read_var_u32(cur)?).map_err(|_| "collection length overflow".to_owned())?;
    if len > MAX_COLLECTION {
        return Err(format!("collection length {len} exceeds {MAX_COLLECTION}"));
    }
    Ok(len)
}

fn read_var_u32(cur: &mut Cursor<&[u8]>) -> Result<u32, String> {
    read(cur, Reader::varint)
}

fn read_item_id(cur: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let id = read_var_u32(cur)?;
    ItemKind::from_u32(id)
        .map(|_| id)
        .ok_or_else(|| format!("unknown item registry id {id}"))
}

fn read_component_id(cur: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let id = read_var_u32(cur)?;
    DataComponentKind::from_u32(id)
        .map(|_| id)
        .ok_or_else(|| format!("unknown data component registry id {id}"))
}

fn read_var_i32(cur: &mut Cursor<&[u8]>) -> Result<i32, String> {
    read(cur, Reader::var_i32)
}

fn read_bool(cur: &mut Cursor<&[u8]>) -> Result<bool, String> {
    read(cur, Reader::bool)
}

fn read_u8(cur: &mut Cursor<&[u8]>) -> Result<u8, String> {
    read(cur, Reader::u8)
}

/// Vanilla `Identifier.STREAM_CODEC`, namespaced as `Identifier.parse` does.
fn read_identifier(cur: &mut Cursor<&[u8]>) -> Result<String, String> {
    read(cur, |r| {
        r.identifier()
            .map(crate::chat_component::normalize_identifier)
    })
}

/// A text component as network NBT
/// (`ComponentSerialization.STREAM_CODEC`).
fn read_component(cur: &mut Cursor<&[u8]>) -> Result<crate::chat_component::Component, String> {
    let mut pos = cur.position() as usize;
    let component = super::chat::read_component(cur.get_ref(), &mut pos)?;
    cur.set_position(pos as u64);
    Ok(component)
}

fn ensure_eof(cur: &Cursor<&[u8]>) -> Result<(), String> {
    let consumed = cur.position() as usize;
    let total = cur.get_ref().len();
    if consumed == total {
        Ok(())
    } else {
        Err(format!(
            "recipe packet has {} trailing bytes",
            total - consumed
        ))
    }
}

/// One pomme-protocol read on the rest of `cur`.
fn read<'a, T>(
    cur: &mut Cursor<&'a [u8]>,
    f: impl FnOnce(&mut Reader<'a>) -> DecodeResult<T>,
) -> Result<T, String> {
    Reader::on_cursor(cur, f).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use pomme_protocol::wire;

    use super::*;

    /// An inline trim pattern as vanilla encodes it (`CodecOracle`).
    #[test]
    fn direct_trim_pattern_matches_vanilla_encoder() {
        let bytes = pomme_protocol::vanilla_fixtures::case("trim_pattern/direct");
        let mut cur = Cursor::new(bytes.as_slice());
        let TrimPatternHolder::Direct {
            asset_id,
            description,
            decal,
        } = parse_trim_pattern(&mut cur).unwrap()
        else {
            panic!("expected a direct trim pattern");
        };
        assert_eq!(cur.position() as usize, bytes.len());
        assert_eq!(asset_id, "minecraft:test");
        assert_eq!(description.plain_text(), "Test Trim");
        assert!(decal);
    }

    #[test]
    fn slot_displays_nested_past_the_depth_cap_are_rejected() {
        // `wrappers` with_any_potion (2) levels around an empty display.
        let nested = |wrappers: usize| {
            let mut bytes = vec![2; wrappers];
            bytes.push(0);
            bytes
        };
        let at_cap = nested(MAX_SLOT_DISPLAY_DEPTH - 1);
        let mut cur = Cursor::new(at_cap.as_slice());
        assert!(parse_slot_display(&mut cur, MAX_SLOT_DISPLAY_DEPTH).is_ok());
        let past_cap = nested(MAX_SLOT_DISPLAY_DEPTH);
        let mut cur = Cursor::new(past_cap.as_slice());
        assert_eq!(
            parse_slot_display(&mut cur, MAX_SLOT_DISPLAY_DEPTH).unwrap_err(),
            format!("slot display nested deeper than {MAX_SLOT_DISPLAY_DEPTH}")
        );
    }

    #[test]
    fn forged_list_length_fails_on_missing_elements() {
        let mut bytes = Vec::new();
        wire::write_varint(&mut bytes, MAX_COLLECTION as u32);
        bytes.push(0);
        let mut cur = Cursor::new(bytes.as_slice());
        assert!(read_list(&mut cur, read_var_u32).is_err());
    }

    #[test]
    fn direct_ingredients_must_be_non_empty_and_air_free() {
        for (items, error) in [
            (vec![], "ingredient can't be empty"),
            (
                vec![ItemKind::Diamond.to_u32(), ItemKind::Air.to_u32()],
                "ingredient can't contain air",
            ),
        ] {
            let mut bytes = Vec::new();
            wire::write_varint(&mut bytes, items.len() as u32 + 1);
            for item in items {
                wire::write_varint(&mut bytes, item);
            }
            let mut cur = Cursor::new(bytes.as_slice());
            assert_eq!(parse_ingredient(&mut cur).unwrap_err(), error);
        }
    }

    #[test]
    fn item_stack_template_is_item_then_count_not_item_stack_order() {
        let mut bytes = Vec::new();
        wire::write_varint(&mut bytes, 53); // native dripstone_block
        wire::write_varint(&mut bytes, 3); // count
        bytes.extend_from_slice(&[0, 0]); // empty component patch
        let mut cur = Cursor::new(bytes.as_slice());
        let template = parse_item_stack_template(&mut cur).unwrap();
        assert_eq!(template.item, 53);
        assert_eq!(template.count, 3);
        assert_eq!(cur.position() as usize, bytes.len());
    }

    #[test]
    fn item_stack_template_rejects_empty_templates() {
        for (item, count) in [(ItemKind::Air.to_u32(), 1), (ItemKind::Diamond.to_u32(), 0)] {
            let mut bytes = Vec::new();
            wire::write_varint(&mut bytes, item);
            wire::write_varint(&mut bytes, count);
            bytes.extend_from_slice(&[0, 0]);
            let mut cur = Cursor::new(bytes.as_slice());
            let err = parse_item_stack_template(&mut cur).unwrap_err();
            assert!(err.contains("empty item template"), "{err}");
        }
    }

    #[test]
    fn recipe_registry_ids_are_validated_at_decode_boundary() {
        let mut ingredient = Vec::new();
        wire::write_varint(&mut ingredient, 2); // one direct holder + sentinel
        wire::write_varint(&mut ingredient, u32::MAX);
        let mut cur = Cursor::new(ingredient.as_slice());
        assert_eq!(
            parse_ingredient(&mut cur).unwrap_err(),
            format!("unknown item registry id {}", u32::MAX)
        );

        let mut component = Vec::new();
        wire::write_varint(&mut component, 3); // only_with_component
        wire::write_varint(&mut component, 0); // empty nested display
        wire::write_varint(&mut component, u32::MAX);
        let mut cur = Cursor::new(component.as_slice());
        assert_eq!(
            parse_slot_display(&mut cur, MAX_SLOT_DISPLAY_DEPTH).unwrap_err(),
            format!("unknown data component registry id {}", u32::MAX)
        );
    }

    #[test]
    fn trim_pattern_reference_uses_holder_plus_one_encoding() {
        let bytes = [6];
        let mut cur = Cursor::new(bytes.as_slice());
        assert!(matches!(
            parse_trim_pattern(&mut cur).unwrap(),
            TrimPatternHolder::Reference(5)
        ));
    }

    #[test]
    fn add_packet_preserves_flags_and_optional_group() {
        let mut bytes = Vec::new();
        wire::write_varint(&mut bytes, 1); // entries
        wire::write_varint(&mut bytes, 9); // display id
        wire::write_varint(&mut bytes, 0); // shapeless
        wire::write_varint(&mut bytes, 0); // ingredient displays
        wire::write_varint(&mut bytes, 0); // empty result
        wire::write_varint(&mut bytes, 0); // empty station
        wire::write_varint(&mut bytes, 4); // group 3 + sentinel
        wire::write_varint(&mut bytes, 2); // category
        bytes.push(0); // no requirements
        bytes.push(3); // notification + highlight
        bytes.push(1); // replace

        let mut cur = Cursor::new(bytes.as_slice());
        let (entries, replace) = parse_add(&mut cur).unwrap();
        assert!(replace);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].contents.group, Some(3));
        assert!(entries[0].notification);
        assert!(entries[0].highlight);
        assert_eq!(cur.position() as usize, bytes.len());
    }

    #[test]
    fn trailing_bytes_do_not_emit_recipe_events() {
        let mut bytes = vec![0; 8]; // four open/filter setting pairs
        bytes.push(0x7f); // malformed trailing data
        let mut cur = Cursor::new(bytes.as_slice());
        let (tx, rx) = crossbeam_channel::unbounded();

        let result = handle_raw_recipe_packet(packet_ids().settings, &mut cur, &tx)
            .expect("settings is a handled recipe packet");
        assert!(result.is_err());
        assert!(rx.try_recv().is_err());
    }
}
