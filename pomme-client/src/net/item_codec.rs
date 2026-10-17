//! Bridges pomme-protocol's item grammar to azalea's in-memory item types.
//!
//! pomme frames every stack and component itself (`pomme_protocol::item`);
//! azalea's structs only hold the values. Most components share vanilla's
//! layout and decode straight through azalea. The ones whose azalea layout
//! diverges from 26.2 are transcoded to azalea's layout where pomme reads
//! them, and dropped otherwise, so a stack never misdecodes. This module
//! goes away once pomme owns the in-memory type too.

use std::io::Cursor;
use std::sync::OnceLock;

use azalea_buf::AzBuf;
use azalea_inventory::components::{
    BundleContents, ChargedProjectiles, Container, DataComponentUnion, EquipmentSlot,
    SulfurCubeContent, UseRemainder,
};
use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
use azalea_registry::Registry;
use azalea_registry::builtin::{DataComponentKind, ItemKind};
use crossbeam_channel::Sender;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::{self, ComponentPatch, ItemCodec, Shape};
use pomme_protocol::version::NATIVE;
use pomme_protocol::wire::write_varint;
use pomme_protocol::{ClientRegistry, Direction, PacketTable, Phase};

use super::NetworkEvent;

/// Components azalea can't hold faithfully that pomme doesn't read; they're
/// dropped rather than misdecoded.
// TODO: carry these once pomme owns the in-memory item type.
pub(super) const DROPPED: &[&str] = &[
    // BlockPredicate lacks `DataComponentMatchers`; value matchers inverted.
    "can_place_on",
    "can_break",
    // Vanilla sends these persistent-only components as NBT.
    "intangible_projectile",
    "recipes",
    "lock",
    // Holder vs plain registry id.
    "banner_patterns",
    // The occupant's entity type is missing.
    "bees",
    // Same bytes, wrong registry (entity type for block-entity type).
    "block_entity_data",
    // Sparse vanilla ids, ordinal azalea ids.
    "tropical_fish/pattern",
    "rabbit/variant",
];

pub fn native() -> &'static ItemCodec {
    ItemCodec::for_protocol(NATIVE.protocol).expect("native item grammar")
}

/// `ItemStack.OPTIONAL_STREAM_CODEC` into azalea's stack.
pub fn read_stack(r: &mut Reader) -> DecodeResult<ItemStack> {
    Ok(native()
        .read_stack(r)?
        .map_or(ItemStack::Empty, |stack| to_azalea(&stack)))
}

/// Whether this session speaks the native protocol, the only one pomme's
/// item grammar decodes; translated sessions keep azalea's decode, which
/// their id remaps run after.
pub(super) fn native_session() -> bool {
    crate::version::session_protocol() == NATIVE.protocol
}

/// The native clientbound packets that carry items.
#[derive(Clone, Copy)]
pub(super) struct ItemPacketIds {
    container_set_content: u32,
    container_set_slot: u32,
    set_cursor_item: u32,
    set_player_inventory: u32,
    set_equipment: u32,
    pub set_entity_data: u32,
    pub update_advancements: u32,
}

pub(super) fn item_packet_ids() -> ItemPacketIds {
    static IDS: OnceLock<ItemPacketIds> = OnceLock::new();
    *IDS.get_or_init(|| {
        let table = PacketTable::native();
        let id = |name| {
            table
                .id(Phase::Game, Direction::Clientbound, name)
                .unwrap_or_else(|| panic!("{name} in packet table"))
        };
        ItemPacketIds {
            container_set_content: id("container_set_content"),
            container_set_slot: id("container_set_slot"),
            set_cursor_item: id("set_cursor_item"),
            set_player_inventory: id("set_player_inventory"),
            set_equipment: id("set_equipment"),
            set_entity_data: id("set_entity_data"),
            update_advancements: id("update_advancements"),
        }
    })
}

/// Decodes the native inventory packets with pomme's item grammar; `None`
/// for any other packet or a translated session.
pub fn handle_raw_item_packet(
    packet_id: u32,
    payload: &[u8],
    event_tx: &Sender<NetworkEvent>,
) -> Option<DecodeResult<()>> {
    if !native_session() {
        return None;
    }
    let mut r = Reader::new(payload);
    let event = read_item_packet(packet_id, &mut r).transpose()?;
    Some(event.and_then(|event| {
        r.ensure_eof()?;
        let _ = event_tx.try_send(event);
        Ok(())
    }))
}

fn read_item_packet(packet_id: u32, r: &mut Reader) -> DecodeResult<Option<NetworkEvent>> {
    let ids = item_packet_ids();
    Ok(Some(if packet_id == ids.container_set_content {
        // ClientboundContainerSetContentPacket.java:18
        let container_id = r.var_i32()?;
        let state_id = r.varint()?;
        let items = r.list(usize::MAX, read_stack)?;
        NetworkEvent::ContainerContent {
            container_id,
            items,
            carried: read_stack(r)?,
            state_id,
        }
    } else if packet_id == ids.container_set_slot {
        // ClientboundContainerSetSlotPacket.java:30-40
        NetworkEvent::ContainerSlot {
            container_id: r.var_i32()?,
            state_id: r.varint()?,
            index: r.u16()?,
            item: read_stack(r)?,
        }
    } else if packet_id == ids.set_cursor_item {
        NetworkEvent::CursorItem {
            item: read_stack(r)?,
        }
    } else if packet_id == ids.set_player_inventory {
        // ClientboundSetPlayerInventoryPacket.java:17
        NetworkEvent::PlayerInventorySlot {
            index: r.varint()?,
            item: read_stack(r)?,
        }
    } else if packet_id == ids.set_equipment {
        // ClientboundSetEquipmentPacket.java:34-44: slot ordinals, the high
        // bit marking another entry.
        let entity_id = r.var_i32()?;
        let mut slots = Vec::new();
        loop {
            let byte = r.u8()?;
            let slot = EquipmentSlot::from_byte(byte & 0x7F)
                .ok_or_else(|| r.error("unknown equipment slot"))?;
            slots.push((slot, read_stack(r)?));
            if byte & 0x80 == 0 {
                break;
            }
        }
        NetworkEvent::EntityEquipment { entity_id, slots }
    } else {
        return Ok(None);
    }))
}

/// A native `set_creative_mode_slot` frame, with the stack in vanilla's
/// layout.
pub fn creative_slot_frame(packet: &ServerboundSetCreativeModeSlot) -> Vec<u8> {
    pomme_protocol::wire::encode_set_creative_mode_slot(
        packet.slot_num as i16,
        from_azalea(&packet.item_stack).as_ref(),
    )
}

/// A decoded stack as azalea's value; an item azalea doesn't know is empty.
pub fn to_azalea(stack: &item::ItemStack) -> ItemStack {
    let Some(kind) = ItemKind::from_u32(stack.item) else {
        return ItemStack::Empty;
    };
    ItemStack::Present(ItemStackData {
        kind,
        count: stack.count,
        component_patch: azalea_patch(&stack.patch),
    })
}

/// A decoded patch as azalea's value.
pub fn azalea_patch(patch: &ComponentPatch) -> DataComponentPatch {
    let bytes = azalea_patch_bytes(patch);
    DataComponentPatch::azalea_read(&mut Cursor::new(&bytes[..]))
        .expect("every component was validated through azalea")
}

/// An azalea stack in vanilla's layout, for encoding; `None` when empty.
pub fn from_azalea(stack: &ItemStack) -> Option<item::ItemStack> {
    let data = stack.as_present()?;
    let codec = native();
    let mut patch = ComponentPatch::default();
    for (kind, value) in data.component_patch.iter() {
        let id = kind.to_u32();
        let Some(value) = value else {
            patch.removed.push(id);
            continue;
        };
        let mut bytes = Vec::new();
        value.encode(&mut bytes).ok()?;
        let Some(vanilla) = vanilla_component(&data.component_patch, kind, &bytes) else {
            tracing::debug!("dropping outbound {kind} component");
            continue;
        };
        // Only send what vanilla can read back.
        let mut r = Reader::new(&vanilla);
        if codec.read_component(&mut r, id).is_ok() && r.ensure_eof().is_ok() {
            patch.added.push((id, vanilla.into()));
        }
    }
    Some(item::ItemStack {
        item: data.kind.to_u32(),
        count: data.count,
        patch,
    })
}

fn component_name(id: u32) -> Option<&'static str> {
    native()
        .registries()
        .name_of(ClientRegistry::DataComponentType, id)
}

/// A patch re-encoded in azalea's layout, keeping only the components azalea
/// reads back to exactly their span.
fn azalea_patch_bytes(patch: &ComponentPatch) -> Vec<u8> {
    let kept: Vec<(u32, Vec<u8>)> = patch
        .added
        .iter()
        .filter_map(|(id, value)| {
            let bytes = azalea_component(*id, value);
            if bytes.is_none() {
                tracing::debug!("dropping {:?} component", component_name(*id));
            }
            Some((*id, bytes?))
        })
        .collect();
    let mut out = Vec::new();
    write_varint(&mut out, kept.len() as u32);
    write_varint(&mut out, patch.removed.len() as u32);
    for (id, bytes) in &kept {
        write_varint(&mut out, *id);
        out.extend_from_slice(bytes);
    }
    for id in &patch.removed {
        write_varint(&mut out, *id);
    }
    out
}

/// One vanilla component value in azalea's layout, when azalea reads it
/// back to exactly that span.
fn azalea_component(id: u32, value: &[u8]) -> Option<Vec<u8>> {
    if DROPPED.contains(&component_name(id)?) {
        return None;
    }
    transcode_to_azalea(id, value)
}

/// [`azalea_component`] without the drop list.
pub(super) fn transcode_to_azalea(id: u32, value: &[u8]) -> Option<Vec<u8>> {
    let name = component_name(id)?;
    let kind = DataComponentKind::from_u32(id)?;
    let mut r = Reader::new(value);
    let bytes = match name {
        "use_remainder" | "sulfur_cube_content" => azalea_template(&mut r).ok()?,
        "charged_projectiles" | "bundle_contents" => {
            let mut out = Vec::new();
            let n = r.len(usize::MAX).ok()?;
            write_varint(&mut out, n as u32);
            for _ in 0..n {
                out.extend(azalea_template(&mut r).ok()?);
            }
            out
        }
        "container" => {
            let mut out = Vec::new();
            let n = r.len(256).ok()?;
            write_varint(&mut out, n as u32);
            for _ in 0..n {
                if r.bool().ok()? {
                    out.extend(azalea_template(&mut r).ok()?);
                } else {
                    // azalea's empty stack: a zero count.
                    out.push(0);
                }
            }
            out
        }
        "equippable" => transcode_equippable(&mut r, true).ok()??,
        "consumable" => transcode_consumable(&mut r, true).ok()??,
        "death_protection" => transcode_effects(&mut r, true).ok()??,
        "trim" => transcode_trim(&mut r, true).ok()??,
        _ => r.bytes(value.len()).ok()?.to_vec(),
    };
    r.ensure_eof().ok()?;
    let mut cur = Cursor::new(&bytes[..]);
    let mut union = DataComponentUnion::azalea_read_as(kind, &mut cur).ok()?;
    // SAFETY: `union` was just read as `kind`.
    unsafe { union.drop_as(kind) };
    (cur.position() as usize == bytes.len()).then_some(bytes)
}

/// A vanilla `ItemStackTemplate` (item, count, patch) as azalea's stack
/// (count, item, patch).
fn azalea_template(r: &mut Reader) -> DecodeResult<Vec<u8>> {
    let template = native().read_template(r)?;
    let mut out = Vec::new();
    write_varint(&mut out, template.count as u32);
    write_varint(&mut out, template.item);
    out.extend(azalea_patch_bytes(&template.patch));
    Ok(out)
}

/// One azalea component value in vanilla's layout.
fn vanilla_component(
    patch: &DataComponentPatch,
    kind: DataComponentKind,
    azalea: &[u8],
) -> Option<Vec<u8>> {
    let template = |stack: &ItemStack| {
        let mut out = Vec::new();
        item::write_template(&mut out, &from_azalea(stack)?);
        Some(out)
    };
    let templates = |items: &[ItemStack]| {
        let mut out = Vec::new();
        let items: Vec<_> = items.iter().filter_map(from_azalea).collect();
        write_varint(&mut out, items.len() as u32);
        for stack in &items {
            item::write_template(&mut out, stack);
        }
        out
    };
    let mut r = Reader::new(azalea);
    let bytes = match kind {
        DataComponentKind::UseRemainder => template(&patch.get::<UseRemainder>()?.convert_into)?,
        DataComponentKind::SulfurCubeContent => {
            template(&patch.get::<SulfurCubeContent>()?.absorbed_block_item_stack)?
        }
        DataComponentKind::ChargedProjectiles => {
            templates(&patch.get::<ChargedProjectiles>()?.items)
        }
        DataComponentKind::BundleContents => templates(&patch.get::<BundleContents>()?.items),
        DataComponentKind::Container => {
            let items = &patch.get::<Container>()?.items;
            let mut out = Vec::new();
            write_varint(&mut out, items.len() as u32);
            for slot in items {
                match from_azalea(slot) {
                    Some(stack) => {
                        out.push(1);
                        item::write_template(&mut out, &stack);
                    }
                    None => out.push(0),
                }
            }
            out
        }
        DataComponentKind::Equippable => transcode_equippable(&mut r, false).ok()??,
        DataComponentKind::Consumable => transcode_consumable(&mut r, false).ok()??,
        DataComponentKind::DeathProtection => transcode_effects(&mut r, false).ok()??,
        DataComponentKind::Trim => transcode_trim(&mut r, false).ok()??,
        _ => return Some(azalea.to_vec()),
    };
    Some(bytes)
}

// The transcoders below rewrite one component between vanilla's layout and
// azalea's: `to_azalea` picks the direction. `Ok(None)` means the value has
// no representation on the other side.

/// A vanilla `ByteBufCodecs.holder` (id + 1, or 0 and an inline value) where
/// azalea stores a plain registry id; an inline value has no azalea form.
fn transcode_holder_id(
    r: &mut Reader,
    out: &mut Vec<u8>,
    to_azalea: bool,
) -> DecodeResult<Option<()>> {
    let id = r.varint()?;
    let id = if to_azalea {
        let Some(id) = id.checked_sub(1) else {
            return Ok(None);
        };
        id
    } else {
        id + 1
    };
    write_varint(out, id);
    Ok(Some(()))
}

/// Vanilla `EquipmentSlot.id` against azalea's ordinals.
const EQUIPMENT_SLOT_IDS: [u32; 8] = [0, 5, 1, 2, 3, 4, 6, 7];

/// Equippable.java:48: the slot id and both sounds diverge.
fn transcode_equippable(r: &mut Reader, to_azalea: bool) -> DecodeResult<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let slot = r.varint()?;
    let slot = if to_azalea {
        // ByIdMap.OutOfBoundsStrategy.ZERO
        Some(
            EQUIPMENT_SLOT_IDS
                .iter()
                .position(|&id| id == slot)
                .unwrap_or(0),
        )
    } else {
        EQUIPMENT_SLOT_IDS.get(slot as usize).map(|&id| id as usize)
    };
    let Some(slot) = slot else { return Ok(None) };
    write_varint(&mut out, slot as u32);
    if transcode_holder_id(r, &mut out, to_azalea)?.is_none() {
        return Ok(None);
    }
    // asset id, camera overlay, allowed entities, then five flags.
    let start = r.pos();
    for _ in 0..2 {
        if r.bool()? {
            r.identifier()?;
        }
    }
    if r.bool()? {
        native().skip(r, &Shape::HolderSet)?;
    }
    r.bytes(5)?;
    out.extend_from_slice(r.since(start));
    if transcode_holder_id(r, &mut out, to_azalea)?.is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Vanilla `ItemUseAnimation` ids past azalea's last variant (bundle, spear).
const AZALEA_USE_ANIMATIONS: u32 = 10;

/// Consumable.java:49: its own sound is a holder on both sides; the
/// animation and the effects' sounds diverge.
fn transcode_consumable(r: &mut Reader, to_azalea: bool) -> DecodeResult<Option<Vec<u8>>> {
    let start = r.pos();
    r.f32()?;
    if r.varint()? >= AZALEA_USE_ANIMATIONS {
        return Ok(None);
    }
    native().skip(r, &item::SOUND)?;
    r.bool()?;
    let mut out = r.since(start).to_vec();
    let Some(effects) = transcode_effects(r, to_azalea)? else {
        return Ok(None);
    };
    out.extend(effects);
    Ok(Some(out))
}

/// A list of `ConsumeEffect`s; only `play_sound`'s sound diverges.
fn transcode_effects(r: &mut Reader, to_azalea: bool) -> DecodeResult<Option<Vec<u8>>> {
    let codec = native();
    let registries = codec.registries();
    let mut out = Vec::new();
    let n = r.len(usize::MAX)?;
    write_varint(&mut out, n as u32);
    for _ in 0..n {
        let start = r.pos();
        let kind = r.varint()?;
        match registries.name_of(ClientRegistry::ConsumeEffectType, kind) {
            Some("apply_effects") => {
                for _ in 0..r.len(usize::MAX)? {
                    codec.skip(r, &item::MOB_EFFECT_INSTANCE)?;
                }
                r.f32()?;
            }
            Some("remove_effects") => codec.skip(r, &Shape::HolderSet)?,
            Some("clear_all_effects") => {}
            Some("teleport_randomly") => {
                r.f32()?;
            }
            Some("play_sound") => {
                out.extend_from_slice(r.since(start));
                if transcode_holder_id(r, &mut out, to_azalea)?.is_none() {
                    return Ok(None);
                }
                continue;
            }
            _ => return Ok(None),
        }
        out.extend_from_slice(r.since(start));
    }
    Ok(Some(out))
}

/// ArmorTrim.java:38: material and pattern are holders in vanilla, plain
/// registry ids in azalea.
fn transcode_trim(r: &mut Reader, to_azalea: bool) -> DecodeResult<Option<Vec<u8>>> {
    let mut out = Vec::new();
    for _ in 0..2 {
        if transcode_holder_id(r, &mut out, to_azalea)?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use azalea_inventory::components::{Damage, Equippable, Trim};

    use super::*;

    fn component(name: &str) -> u32 {
        native()
            .registries()
            .id_of(ClientRegistry::DataComponentType, name)
            .unwrap()
    }

    fn var(buf: &mut Vec<u8>, v: u32) {
        write_varint(buf, v);
    }

    /// A vanilla optional stack of `count` `item` with one component.
    fn stack_with(item: ItemKind, name: &str, value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        var(&mut buf, 1);
        var(&mut buf, item.to_u32());
        var(&mut buf, 1);
        var(&mut buf, 0);
        var(&mut buf, component(name));
        buf.extend(value);
        buf
    }

    fn decode(bytes: &[u8]) -> ItemStack {
        let mut r = Reader::new(bytes);
        let stack = read_stack(&mut r).unwrap();
        r.ensure_eof().unwrap();
        stack
    }

    /// One bread x2 as a vanilla `ItemStackTemplate`.
    const BREAD_TEMPLATE: [u8; 5] = [0xD5, 0x07, 0x02, 0x00, 0x00];

    #[test]
    fn bundle_entries_are_item_then_count() {
        assert_eq!(ItemKind::Bread.to_u32(), 981);
        let mut value = vec![1];
        value.extend(BREAD_TEMPLATE);
        let stack = decode(&stack_with(ItemKind::Bundle, "bundle_contents", &value));
        let contents = stack.get_component::<BundleContents>().unwrap();
        let entry = contents.items[0].as_present().unwrap();
        assert_eq!((entry.kind, entry.count), (ItemKind::Bread, 2));
    }

    #[test]
    fn creative_slots_go_out_delimited_in_vanilla_layout() {
        let bundle = ItemStack::new(ItemKind::Bundle, 1).with_component(BundleContents {
            items: vec![ItemStack::new(ItemKind::Bread, 2)],
        });
        let frame = creative_slot_frame(&ServerboundSetCreativeModeSlot {
            slot_num: 36,
            item_stack: bundle,
        });
        let mut expected = Vec::new();
        var(
            &mut expected,
            pomme_protocol::wire::game_serverbound_id("set_creative_mode_slot"),
        );
        expected.extend(36i16.to_be_bytes());
        var(&mut expected, 1);
        var(&mut expected, ItemKind::Bundle.to_u32());
        var(&mut expected, 1);
        var(&mut expected, 0);
        var(&mut expected, component("bundle_contents"));
        var(&mut expected, 1 + BREAD_TEMPLATE.len() as u32);
        expected.push(1);
        expected.extend(BREAD_TEMPLATE);
        assert_eq!(frame, expected);
    }

    #[test]
    fn equippable_slot_ids_and_sounds_round_trip() {
        let mut value = Vec::new();
        var(&mut value, 5); // vanilla offhand id
        var(&mut value, 12); // sound holder: id 11
        value.extend([0, 0, 0]); // no asset, overlay or entity set
        value.extend([1, 1, 1, 0, 0]);
        var(&mut value, 13); // shearing sound holder: id 12
        let stack = decode(&stack_with(ItemKind::Elytra, "equippable", &value));
        let equippable = stack.get_component::<Equippable>().unwrap();
        assert_eq!(
            equippable.slot,
            azalea_inventory::components::EquipmentSlot::Offhand
        );
        assert_eq!(equippable.equip_sound.to_u32(), 11);

        let out = from_azalea(&stack).unwrap();
        assert_eq!(&*out.patch.added[0].1, &value[..]);
    }

    #[test]
    fn trim_references_transcode_and_inline_values_drop() {
        let stack = decode(&stack_with(ItemKind::DiamondChestplate, "trim", &[3, 2]));
        let trim = stack.get_component::<Trim>().unwrap();
        assert_eq!((trim.material.to_u32(), trim.pattern.to_u32()), (2, 1));
        assert_eq!(&*from_azalea(&stack).unwrap().patch.added[0].1, &[3, 2]);

        // An inline pattern: asset id, an NBT text description, decal flag.
        let mut inline = vec![3, 0];
        inline.extend([1, b'x', 8, 0, 1, b'y', 0]);
        let stack = decode(&stack_with(ItemKind::DiamondChestplate, "trim", &inline));
        assert!(stack.get_component::<Trim>().is_none());
    }

    #[test]
    fn unrepresentable_components_drop_without_losing_the_stack() {
        let mut bytes = Vec::new();
        var(&mut bytes, 1);
        var(&mut bytes, ItemKind::DiamondPickaxe.to_u32());
        var(&mut bytes, 2);
        var(&mut bytes, 0);
        var(&mut bytes, component("can_break"));
        var(&mut bytes, 0); // no predicates
        var(&mut bytes, component("damage"));
        var(&mut bytes, 7);
        let stack = decode(&bytes);
        let data = stack.as_present().unwrap();
        assert_eq!(data.component_patch.get::<Damage>().unwrap().amount, 7);
        assert!(!data.component_patch.has_kind(DataComponentKind::CanBreak));
    }

    #[test]
    fn container_set_content_decodes_natively() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut payload = Vec::new();
        var(&mut payload, 1); // container id
        var(&mut payload, 9); // state id
        var(&mut payload, 2);
        payload.push(0); // empty slot
        let mut value = vec![1];
        value.extend(BREAD_TEMPLATE);
        payload.extend(stack_with(ItemKind::Bundle, "bundle_contents", &value));
        payload.push(0); // empty carried item
        let id = item_packet_ids().container_set_content;
        handle_raw_item_packet(id, &payload, &tx).unwrap().unwrap();
        let NetworkEvent::ContainerContent {
            container_id,
            items,
            carried,
            state_id,
        } = rx.try_recv().unwrap()
        else {
            panic!("wrong event");
        };
        assert_eq!((container_id, state_id), (1, 9));
        assert!(items[0].is_empty() && carried.is_empty());
        assert_eq!(items[1].kind(), ItemKind::Bundle);

        payload.push(0xEE);
        assert!(handle_raw_item_packet(id, &payload, &tx).unwrap().is_err());
    }
}
