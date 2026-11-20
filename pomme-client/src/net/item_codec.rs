//! The bridge to the inventory fork, which still writes the JSON a stack's
//! tooltip is built from.
//!
//! A stack (`pomme_item`) holds each component as its vanilla bytes, and the
//! client reads them through pomme's own values (`pomme_item::components`).
//! For a hash or the JSON the bytes become the fork's structs: most share
//! vanilla's layout and decode as they are, the ones whose fork layout
//! diverges are transcoded first, and a few the fork can't hold are left
//! out. Both go as pomme ports them.

use std::io::Cursor;
use std::sync::OnceLock;

use azalea_buf::AzBuf;
use azalea_inventory::components::DataComponentUnion;
use azalea_inventory::{
    DataComponentPatch, ItemStack as ForkStack, ItemStackData as ForkStackData,
};
use pomme_item::{ItemStack, ItemStackData};
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::{self, ComponentPatch, ItemCodec, Shape};
use pomme_protocol::wire::write_varint;
use pomme_protocol::{ClientRegistry, Direction, PacketTable, Phase};
use pomme_registry::Registry;
use pomme_registry::builtin::{DataComponentKind, ItemKind};

/// Components the fork can't hold faithfully; they're left out rather than
/// misdecoded.
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

/// The native clientbound packets the item-gated raw handlers own.
#[derive(Clone, Copy)]
pub(super) struct ItemPacketIds {
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
            set_entity_data: id("set_entity_data"),
            update_advancements: id("update_advancements"),
        }
    })
}

/// The stack's JSON form, which item tooltips are built from.
// TODO: serialized by the fork, so a component it can't hold is missing.
pub fn stack_json(stack: &ItemStackData) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(to_fork(&stack.to_wire()))
}

/// The stack as the fork's value, which hashes and serializes its
/// components; an item the fork doesn't know is empty.
pub(super) fn to_fork(stack: &item::ItemStack) -> ForkStack {
    let Some(kind) = ItemKind::from_u32(stack.item) else {
        return ForkStack::Empty;
    };
    ForkStack::Present(ForkStackData {
        kind,
        count: stack.count,
        component_patch: fork_patch(&stack.patch),
    })
}

/// A decoded patch as the fork's value.
fn fork_patch(patch: &ComponentPatch) -> DataComponentPatch {
    let bytes = azalea_patch_bytes(patch);
    DataComponentPatch::azalea_read(&mut Cursor::new(&bytes[..]))
        .expect("every component was validated through azalea")
}

/// The connection's registries, for the entry names a hash holds.
struct SessionNames<'a>(&'a azalea_core::registry_holder::RegistryHolder);

impl pomme_item::hash::Names for SessionNames<'_> {
    fn name(&self, registry: &str, id: u32) -> Option<pomme_registry::Identifier> {
        self.0
            .protocol_id_to_identifier(registry.into(), id)
            .cloned()
    }
}

/// `HashedStack.create`: the stack with each component value replaced by
/// the hash of its `CODEC` form; `None` when empty.
// TODO: a value with no form yet goes unhashed, so the server resyncs a slot
// holding one.
pub fn hashed_stack(
    stack: &ItemStack,
    registries: &azalea_core::registry_holder::RegistryHolder,
) -> Option<pomme_protocol::serverbound::HashedStack> {
    let data = stack.as_present()?;
    let names = SessionNames(registries);
    let mut hashed = pomme_protocol::serverbound::HashedStack {
        item: data.kind.to_u32(),
        count: data.count,
        added: Vec::new(),
        removed: Vec::new(),
    };
    for (kind, value) in data.component_patch.iter() {
        let Some(bytes) = value else {
            hashed.removed.push(kind.to_u32());
            continue;
        };
        if let Some(value) = pomme_item::components::value_of(kind, bytes, &names) {
            hashed.added.push((kind.to_u32(), value.hash().as_i32()));
        }
    }
    Some(hashed)
}

fn component_name(id: u32) -> Option<&'static str> {
    ItemCodec::native()
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
        "equippable" => transcode_equippable(&mut r).ok()??,
        "consumable" => transcode_consumable(&mut r).ok()??,
        "death_protection" => transcode_effects(&mut r).ok()??,
        "trim" => transcode_trim(&mut r).ok()??,
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
    let template = ItemCodec::native().read_template(r)?;
    let mut out = Vec::new();
    write_varint(&mut out, template.count as u32);
    write_varint(&mut out, template.item);
    out.extend(azalea_patch_bytes(&template.patch));
    Ok(out)
}

// The transcoders below rewrite one component from vanilla's layout to
// azalea's. `Ok(None)` means the value has no azalea form.

/// A vanilla `ByteBufCodecs.holder` (id + 1, or 0 and an inline value) where
/// azalea stores a plain registry id; an inline value has no azalea form.
fn transcode_holder_id(r: &mut Reader, out: &mut Vec<u8>) -> DecodeResult<Option<()>> {
    let Some(id) = r.varint()?.checked_sub(1) else {
        return Ok(None);
    };
    write_varint(out, id);
    Ok(Some(()))
}

/// Vanilla `EquipmentSlot.id` against azalea's ordinals.
const EQUIPMENT_SLOT_IDS: [u32; 8] = [0, 5, 1, 2, 3, 4, 6, 7];

/// Equippable.java:48: the slot id and both sounds diverge.
fn transcode_equippable(r: &mut Reader) -> DecodeResult<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let slot = r.varint()?;
    // ByIdMap.OutOfBoundsStrategy.ZERO
    let slot = EQUIPMENT_SLOT_IDS
        .iter()
        .position(|&id| id == slot)
        .unwrap_or(0);
    write_varint(&mut out, slot as u32);
    if transcode_holder_id(r, &mut out)?.is_none() {
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
        ItemCodec::native().skip(r, &Shape::HolderSet)?;
    }
    r.bytes(5)?;
    out.extend_from_slice(r.since(start));
    if transcode_holder_id(r, &mut out)?.is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Vanilla `ItemUseAnimation` ids past azalea's last variant (bundle, spear).
const AZALEA_USE_ANIMATIONS: u32 = 10;

/// Consumable.java:49: its own sound is a holder on both sides; the
/// animation and the effects' sounds diverge.
fn transcode_consumable(r: &mut Reader) -> DecodeResult<Option<Vec<u8>>> {
    let start = r.pos();
    r.f32()?;
    if r.varint()? >= AZALEA_USE_ANIMATIONS {
        return Ok(None);
    }
    ItemCodec::native().skip(r, &item::SOUND)?;
    r.bool()?;
    let mut out = r.since(start).to_vec();
    let Some(effects) = transcode_effects(r)? else {
        return Ok(None);
    };
    out.extend(effects);
    Ok(Some(out))
}

/// A list of `ConsumeEffect`s; only `play_sound`'s sound diverges.
fn transcode_effects(r: &mut Reader) -> DecodeResult<Option<Vec<u8>>> {
    let codec = ItemCodec::native();
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
                if transcode_holder_id(r, &mut out)?.is_none() {
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
fn transcode_trim(r: &mut Reader) -> DecodeResult<Option<Vec<u8>>> {
    let mut out = Vec::new();
    for _ in 0..2 {
        if transcode_holder_id(r, &mut out)?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use pomme_item::components::{
        BundleContents, Component as _, Damage, EquipmentSlot, Equippable, MaxDamage, Trim,
    };
    use pomme_registry::Holder;

    use super::*;

    fn component(name: &str) -> u32 {
        ItemCodec::native()
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
        let stack = ItemCodec::native().read_stack(&mut r).unwrap();
        r.ensure_eof().unwrap();
        stack.map_or(ItemStack::Empty, |stack| ItemStack::from_wire(&stack))
    }

    /// One bread x2 as a vanilla `ItemStackTemplate`.
    const BREAD_TEMPLATE: [u8; 5] = [0xD5, 0x07, 0x02, 0x00, 0x00];

    #[test]
    fn a_removed_component_reads_as_absent_not_as_its_default() {
        let mut bundle = ItemStackData::new(ItemKind::Bundle, 1);
        bundle
            .component_patch
            .remove(DataComponentKind::BundleContents);
        assert_eq!(bundle.component::<BundleContents>(), None);

        let pickaxe = ItemStackData::new(ItemKind::IronPickaxe, 1);
        assert!(pickaxe.component::<MaxDamage>().is_some());
        let unbreakable = pickaxe.without::<MaxDamage>();
        assert!(unbreakable.component::<MaxDamage>().is_none());
    }

    #[test]
    fn bundle_entries_are_item_then_count() {
        assert_eq!(ItemKind::Bread.to_u32(), 981);
        let mut value = vec![1];
        value.extend(BREAD_TEMPLATE);
        let stack = decode(&stack_with(ItemKind::Bundle, "bundle_contents", &value));
        let contents = stack.component::<BundleContents>().unwrap();
        let entry = contents.items[0].as_present().unwrap();
        assert_eq!((entry.kind, entry.count), (ItemKind::Bread, 2));
    }

    #[test]
    fn creative_slots_go_out_delimited_in_vanilla_layout() {
        let bundle = ItemStack::new(ItemKind::Bundle, 1).with(&BundleContents {
            items: vec![ItemStack::new(ItemKind::Bread, 2)],
        });
        let frame = pomme_protocol::serverbound::GamePacket::SetCreativeModeSlot {
            slot: 36,
            stack: bundle.to_wire(),
        }
        .encode();
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
    fn an_equippable_reads_its_slot_by_id_and_its_sounds_as_holders() {
        let mut value = Vec::new();
        var(&mut value, 5); // vanilla offhand id
        var(&mut value, 12); // sound holder: id 11
        value.extend([0, 0, 0]); // no asset, overlay or entity set
        value.extend([1, 1, 1, 0, 0]);
        var(&mut value, 13); // shearing sound holder: id 12
        let stack = decode(&stack_with(ItemKind::Elytra, "equippable", &value));
        let equippable = stack.component::<Equippable>().unwrap();
        assert_eq!(equippable.slot, EquipmentSlot::Offhand);
        assert!(matches!(equippable.equip_sound, Holder::Reference(sound) if sound.to_u32() == 11));
        assert_eq!(*equippable.to_bytes(), value[..]);
    }

    #[test]
    fn a_trim_reads_registry_entries_and_inline_values() {
        let stack = decode(&stack_with(ItemKind::DiamondChestplate, "trim", &[3, 2]));
        let trim = stack.component::<Trim>().unwrap();
        assert!(matches!(trim.material, Holder::Reference(material) if material.to_u32() == 2));
        assert!(matches!(trim.pattern, Holder::Reference(pattern) if pattern.to_u32() == 1));
        assert_eq!(*trim.to_bytes(), [3, 2]);

        // An inline pattern: asset id, an NBT text description, decal flag.
        let mut inline = vec![3, 0];
        inline.push(11);
        inline.extend(b"minecraft:x");
        inline.extend([8, 0, 1, b'y', 0]);
        let stack = decode(&stack_with(ItemKind::DiamondChestplate, "trim", &inline));
        let trim = stack.component::<Trim>().unwrap();
        let Holder::Direct(pattern) = &trim.pattern else {
            panic!("an inline pattern, got {:?}", trim.pattern);
        };
        assert_eq!(pattern.asset_id.to_string(), "minecraft:x");
        assert!(!pattern.decal);
        assert_eq!(*trim.to_bytes(), inline[..]);
    }

    #[test]
    fn a_component_the_fork_cant_hold_stays_on_the_stack_unread() {
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
        assert_eq!(data.component::<Damage>().unwrap().amount, 7);
        assert_eq!(
            data.component_patch.get(DataComponentKind::CanBreak),
            Some(&[0][..])
        );
    }

    #[test]
    fn container_set_content_decodes() {
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
        let id = PacketTable::native()
            .id(Phase::Game, Direction::Clientbound, "container_set_content")
            .unwrap();
        let handle = |payload: &[u8]| {
            crate::net::container_wire::handle_raw_container_packet(id, payload, &tx).unwrap()
        };
        handle(&payload).unwrap();
        let crate::net::NetworkEvent::ContainerContent {
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
        assert!(handle(&payload).is_err());
    }
}
