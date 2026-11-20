//! What the client makes of a stack beyond reading it: the hashes a click
//! sends in its components' place and the JSON its tooltip is built from,
//! both from pomme's own component forms (`pomme_item::components`).

use std::sync::OnceLock;

use pomme_item::{ItemStack, ItemStackData};
use pomme_protocol::{Direction, PacketTable, Phase};
use pomme_registry::Registry;

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

/// The stack as the JSON a hover event shows an item with, which item
/// tooltips are built from.
pub fn stack_json(
    stack: &ItemStackData,
    registries: &azalea_core::registry_holder::RegistryHolder,
) -> serde_json::Value {
    pomme_item::components::stack_json(stack, &SessionNames(registries))
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

#[cfg(test)]
mod tests {
    use pomme_item::components::{
        BundleContents, Component as _, Damage, EquipmentSlot, Equippable, MaxDamage, Trim,
    };
    use pomme_protocol::ClientRegistry;
    use pomme_protocol::codec::Reader;
    use pomme_protocol::item::ItemCodec;
    use pomme_protocol::wire::write_varint;
    use pomme_registry::Holder;
    use pomme_registry::builtin::{DataComponentKind, ItemKind};

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
