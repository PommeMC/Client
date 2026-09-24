use std::io::Cursor;
use std::sync::OnceLock;

use azalea_buf::{AzBuf, AzBufVar};
use azalea_protocol::common::tags::TagMap;
use azalea_registry::Registry;
use azalea_registry::builtin::{DataComponentKind, ItemKind};
use azalea_registry::identifier::Identifier;
use crossbeam_channel::Sender;
use pomme_protocol::{Direction, PacketTable, Phase};

use crate::net::NetworkEvent;
use crate::player::inventory::item_resource_name;
use crate::recipe::{
    GhostRecipe, Ingredient, ItemStackTemplate, ItemTags, RecipeBookAddEntry, RecipeBookEntry,
    RecipeBookSettings, RecipeBookTypeSettings, RecipeData, RecipeDisplay, SlotDisplay,
    StonecutterRecipe, TrimPatternHolder,
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
    let len = read_len(cur)?;
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        let id = read_var_u32(cur)?;
        let display = parse_recipe_display(cur)?;
        let group_wire = read_var_u32(cur)?;
        let group = group_wire.checked_sub(1);
        let category = read_var_u32(cur)?;
        let crafting_requirements = if read_bool(cur)? {
            let count = read_len(cur)?;
            let mut requirements = Vec::with_capacity(count);
            for _ in 0..count {
                requirements.push(parse_ingredient(cur)?);
            }
            Some(requirements)
        } else {
            None
        };
        let flags = read_u8(cur)?;
        entries.push(RecipeBookAddEntry {
            contents: RecipeBookEntry {
                id,
                display,
                group,
                category,
                crafting_requirements,
            },
            notification: flags & 1 != 0,
            highlight: flags & 2 != 0,
        });
    }
    Ok((entries, read_bool(cur)?))
}

fn parse_remove(cur: &mut Cursor<&[u8]>) -> Result<Vec<u32>, String> {
    let len = read_len(cur)?;
    (0..len).map(|_| read_var_u32(cur)).collect()
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
    let item_set_count = read_len(cur)?;
    let mut item_sets = std::collections::HashMap::with_capacity(item_set_count);
    for _ in 0..item_set_count {
        let name = read_identifier(cur)?;
        let item_count = read_len(cur)?;
        let mut items = Vec::with_capacity(item_count);
        for _ in 0..item_count {
            items.push(read_item_id(cur)?);
        }
        item_sets.insert(name, items);
    }

    let stonecutter_count = read_len(cur)?;
    let mut stonecutter_recipes = Vec::with_capacity(stonecutter_count);
    for _ in 0..stonecutter_count {
        stonecutter_recipes.push(StonecutterRecipe {
            input: parse_ingredient(cur)?,
            option_display: parse_slot_display(cur)?,
        });
    }
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
    match read_var_u32(cur)? {
        0 => Ok(RecipeDisplay::Shapeless {
            ingredients: parse_slot_vec(cur)?,
            result: parse_slot_display(cur)?,
            crafting_station: parse_slot_display(cur)?,
        }),
        1 => Ok(RecipeDisplay::Shaped {
            width: read_var_u32(cur)?,
            height: read_var_u32(cur)?,
            ingredients: parse_slot_vec(cur)?,
            result: parse_slot_display(cur)?,
            crafting_station: parse_slot_display(cur)?,
        }),
        2 => Ok(RecipeDisplay::Furnace {
            ingredient: parse_slot_display(cur)?,
            fuel: parse_slot_display(cur)?,
            result: parse_slot_display(cur)?,
            crafting_station: parse_slot_display(cur)?,
            duration: read_var_u32(cur)?,
            experience: f32::azalea_read(cur).map_err(buf_err)?,
        }),
        3 => Ok(RecipeDisplay::Stonecutter {
            input: parse_slot_display(cur)?,
            result: parse_slot_display(cur)?,
            crafting_station: parse_slot_display(cur)?,
        }),
        4 => Ok(RecipeDisplay::Smithing {
            template: parse_slot_display(cur)?,
            base: parse_slot_display(cur)?,
            addition: parse_slot_display(cur)?,
            result: parse_slot_display(cur)?,
            crafting_station: parse_slot_display(cur)?,
        }),
        id => Err(format!("unknown recipe_display registry id {id}")),
    }
}

fn parse_slot_vec(cur: &mut Cursor<&[u8]>) -> Result<Vec<SlotDisplay>, String> {
    let len = read_len(cur)?;
    (0..len).map(|_| parse_slot_display(cur)).collect()
}

fn parse_slot_display(cur: &mut Cursor<&[u8]>) -> Result<SlotDisplay, String> {
    match read_var_u32(cur)? {
        0 => Ok(SlotDisplay::Empty),
        1 => Ok(SlotDisplay::AnyFuel),
        2 => Ok(SlotDisplay::WithAnyPotion(Box::new(parse_slot_display(
            cur,
        )?))),
        3 => Ok(SlotDisplay::OnlyWithComponent {
            contents: Box::new(parse_slot_display(cur)?),
            component: read_component_id(cur)?,
        }),
        4 => Ok(SlotDisplay::Item(read_item_id(cur)?)),
        5 => Ok(SlotDisplay::ItemStack(parse_item_stack_template(cur)?)),
        6 => Ok(SlotDisplay::Tag(read_identifier(cur)?)),
        7 => Ok(SlotDisplay::Dyed {
            dye: Box::new(parse_slot_display(cur)?),
            target: Box::new(parse_slot_display(cur)?),
        }),
        8 => Ok(SlotDisplay::SmithingTrim {
            base: Box::new(parse_slot_display(cur)?),
            material: Box::new(parse_slot_display(cur)?),
            trim_pattern: parse_trim_pattern(cur)?,
        }),
        9 => Ok(SlotDisplay::WithRemainder {
            input: Box::new(parse_slot_display(cur)?),
            remainder: Box::new(parse_slot_display(cur)?),
        }),
        10 => Ok(SlotDisplay::Composite(parse_slot_vec(cur)?)),
        id => Err(format!("unknown slot_display registry id {id}")),
    }
}

fn parse_item_stack_template(cur: &mut Cursor<&[u8]>) -> Result<ItemStackTemplate, String> {
    let item = read_item_id(cur)?;
    let count = read_var_i32(cur)?;
    let components = azalea_inventory::DataComponentPatch::azalea_read(cur).map_err(buf_err)?;
    if count == 0 || item == ItemKind::Air.to_u32() {
        return Err("item stack template must be non-empty".to_owned());
    }
    Ok(ItemStackTemplate {
        item,
        count,
        components,
    })
}

fn parse_trim_pattern(cur: &mut Cursor<&[u8]>) -> Result<TrimPatternHolder, String> {
    let holder = read_var_u32(cur)?;
    if holder != 0 {
        return Ok(TrimPatternHolder::Reference(holder - 1));
    }
    Ok(TrimPatternHolder::Direct {
        asset_id: read_identifier(cur)?,
        description: azalea_chat::FormattedText::azalea_read(cur).map_err(buf_err)?,
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
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(read_item_id(cur)?);
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

fn read_len(cur: &mut Cursor<&[u8]>) -> Result<usize, String> {
    let len =
        usize::try_from(read_var_u32(cur)?).map_err(|_| "collection length overflow".to_owned())?;
    if len > MAX_COLLECTION {
        return Err(format!("collection length {len} exceeds {MAX_COLLECTION}"));
    }
    Ok(len)
}

fn read_var_u32(cur: &mut Cursor<&[u8]>) -> Result<u32, String> {
    u32::azalea_read_var(cur).map_err(buf_err)
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
    i32::azalea_read_var(cur).map_err(buf_err)
}

fn read_bool(cur: &mut Cursor<&[u8]>) -> Result<bool, String> {
    bool::azalea_read(cur).map_err(buf_err)
}

fn read_u8(cur: &mut Cursor<&[u8]>) -> Result<u8, String> {
    u8::azalea_read(cur).map_err(buf_err)
}

fn read_identifier(cur: &mut Cursor<&[u8]>) -> Result<String, String> {
    Identifier::azalea_read(cur)
        .map(|id| id.to_string())
        .map_err(buf_err)
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

fn buf_err(error: azalea_buf::BufReadError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use pomme_protocol::wire;

    use super::*;

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
            assert_eq!(
                parse_item_stack_template(&mut cur).unwrap_err(),
                "item stack template must be non-empty"
            );
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
            parse_slot_display(&mut cur).unwrap_err(),
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
