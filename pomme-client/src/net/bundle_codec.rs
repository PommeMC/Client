//! 26.2 bundle item-template bridge at the network boundary.
//!
//! Vanilla changed `BundleContents` to `ItemStackTemplate.STREAM_CODEC`
//! (`item, count, patch`). The pinned Azalea type still decodes its entries
//! with `ItemStack.STREAM_CODEC` (`count, item, patch`). Keep that dependency
//! defect out of Pomme's UI/state by repairing it exactly once as decoded
//! inventory packets enter Pomme. This module can disappear when Pomme owns
//! the complete item-component wire codec.
//!
//! Inbound, only the inventory packets are repaired: equipment and entity
//! metadata hand Pomme an item's kind and count, never its bundle contents.
//! Outbound, `SetCreativeModeSlot` is the only packet carrying a full stack.

use azalea_inventory::components::BundleContents;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_protocol::packets::game::ServerboundGamePacket;
use azalea_registry::Registry;
use azalea_registry::builtin::ItemKind;

/// An inbound stack with its bundle entries in Pomme's order.
pub fn normalized(stack: &ItemStack) -> ItemStack {
    let mut stack = stack.clone();
    if crate::version::session_protocol() == pomme_protocol::version::NATIVE.protocol {
        swap_template_fields(&mut stack);
    }
    stack
}

/// Prepares a native 26.2 packet for Azalea's mismatched encoder.
pub fn encode_native_outbound(packet: &mut ServerboundGamePacket) {
    if let ServerboundGamePacket::SetCreativeModeSlot(p) = packet {
        swap_template_fields(&mut p.item_stack);
    }
}

/// Swaps every bundle entry's item and count, nested bundles included. Azalea
/// orders them `(count, item)` where 26.2 has `(item, count)`, so one swap
/// both repairs a decoded stack and readies one for encoding.
fn swap_template_fields(stack: &mut ItemStack) {
    let Some(contents) = stack.as_present().and_then(crate::ui::bundle::contents) else {
        return;
    };
    let items = contents
        .items
        .iter()
        .map(|entry| {
            let Some(raw) = entry.as_present() else {
                return ItemStack::Empty;
            };
            let Some(kind) = ItemKind::from_u32(raw.count as u32) else {
                return ItemStack::Empty;
            };
            let mut swapped = ItemStack::from(ItemStackData {
                count: raw.kind.to_u32() as i32,
                kind,
                component_patch: raw.component_patch.clone(),
            });
            swap_template_fields(&mut swapped);
            swapped
        })
        .collect();
    crate::ui::bundle::set_contents(stack, BundleContents { items });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_of(entry: ItemStack) -> ItemStack {
        ItemStack::new(ItemKind::Bundle, 1).with_component(BundleContents { items: vec![entry] })
    }

    fn first_entry(stack: &ItemStack) -> (ItemKind, i32) {
        let contents = crate::ui::bundle::contents(stack.as_present().unwrap()).unwrap();
        let entry = contents.items[0].as_present().unwrap();
        (entry.kind, entry.count)
    }

    #[test]
    fn native_registry_ids_explain_the_azalea_field_swap() {
        assert_eq!(ItemKind::Bread.to_u32(), 981);
        assert_eq!(ItemKind::Granite.to_u32(), 2);
    }

    #[test]
    fn swapping_repairs_a_decoded_entry_and_undoes_itself() {
        // Azalea decodes vanilla's bread x2 as item id 2 (granite) x981.
        let mut stack = bundle_of(ItemStack::new(ItemKind::Granite, 981));
        swap_template_fields(&mut stack);
        assert_eq!(first_entry(&stack), (ItemKind::Bread, 2));

        let nested = bundle_of(bundle_of(ItemStack::new(ItemKind::Bread, 2)));
        let mut stack = nested.clone();
        swap_template_fields(&mut stack);
        swap_template_fields(&mut stack);
        assert_eq!(stack, nested);
    }

    #[test]
    fn creative_slot_bundle_entries_go_out_as_item_count_patch() {
        use azalea_buf::AzBuf;
        use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;

        let mut packet =
            ServerboundGamePacket::SetCreativeModeSlot(ServerboundSetCreativeModeSlot {
                slot_num: 36,
                item_stack: bundle_of(ItemStack::new(ItemKind::Bread, 2)),
            });
        encode_native_outbound(&mut packet);
        let ServerboundGamePacket::SetCreativeModeSlot(p) = packet else {
            unreachable!()
        };
        let mut wire = Vec::new();
        p.item_stack.azalea_write(&mut wire).unwrap();
        // `ItemStackTemplate`: bread (VarInt 981), count 2, empty patch.
        let entry = [0xD5, 0x07, 0x02, 0x00, 0x00];
        assert!(wire.windows(entry.len()).any(|w| w == entry), "{wire:02X?}");
    }
}
