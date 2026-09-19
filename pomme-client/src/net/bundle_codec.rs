//! 26.2 bundle item-template bridge at the network boundary.
//!
//! Vanilla changed `BundleContents` to `ItemStackTemplate.STREAM_CODEC`
//! (`item, count, patch`). The pinned Azalea type still decodes its entries
//! with `ItemStack.STREAM_CODEC` (`count, item, patch`). Keep that dependency
//! defect out of Pomme's UI/state by repairing it exactly once as decoded
//! inventory packets enter Pomme. This module can disappear when Pomme owns
//! the complete item-component wire codec.

use azalea_inventory::components::{BundleContents, DataComponentUnion};
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::Registry;
use azalea_registry::builtin::{DataComponentKind, ItemKind};

pub fn normalize_26_2_templates(stack: &mut ItemStack) {
    if crate::version::session_protocol() != pomme_protocol::version::NATIVE.protocol {
        return;
    }
    normalize_templates(stack);
}

fn normalize_templates(stack: &mut ItemStack) {
    let ItemStack::Present(data) = stack else {
        return;
    };
    let Some(raw) = data.get_component::<BundleContents>() else {
        return;
    };
    if raw.items.is_empty() {
        return;
    }
    let items = raw.items.iter().map(normalize_template_stack).collect();
    let component = DataComponentUnion::from(BundleContents { items });
    // SAFETY: the union was constructed from BundleContents, so its arm is
    // exactly BundleContents::KIND.
    unsafe {
        data.component_patch
            .unchecked_insert_component(DataComponentKind::BundleContents, Some(component));
    }
}

fn normalize_template_stack(stack: &ItemStack) -> ItemStack {
    let ItemStack::Present(raw) = stack else {
        return ItemStack::Empty;
    };
    let Some(kind) = ItemKind::from_u32(raw.count as u32) else {
        return ItemStack::Empty;
    };
    let mut normalized = ItemStack::from(ItemStackData {
        count: raw.kind.to_u32() as i32,
        kind,
        component_patch: raw.component_patch.clone(),
    });
    normalize_templates(&mut normalized);
    normalized
}

/// Prepare a native 26.2 stack for Azalea's mismatched BundleContents encoder.
/// This is the inverse of `normalize_26_2_templates`: Azalea will serialize
/// these deliberately swapped fields as Vanilla's `(item, count, patch)`.
pub fn encode_26_2_templates(stack: &mut ItemStack) {
    if crate::version::session_protocol() != pomme_protocol::version::NATIVE.protocol {
        return;
    }
    encode_templates(stack);
}

fn encode_templates(stack: &mut ItemStack) {
    let ItemStack::Present(data) = stack else {
        return;
    };
    let Some(contents) = data.get_component::<BundleContents>() else {
        return;
    };
    let mut items = Vec::with_capacity(contents.items.len());
    for item in &contents.items {
        let ItemStack::Present(item_data) = item else {
            items.push(ItemStack::Empty);
            continue;
        };
        let mut nested = item.clone();
        encode_templates(&mut nested);
        let ItemStack::Present(nested_data) = nested else {
            unreachable!()
        };
        let Some(fake_kind) = ItemKind::from_u32(item_data.count as u32) else {
            // Valid Vanilla stacks cannot have a count that is not also a low
            // item registry id in the pinned Azalea representation.
            items.push(item.clone());
            continue;
        };
        items.push(ItemStack::from(ItemStackData {
            count: item_data.kind.to_u32() as i32,
            kind: fake_kind,
            component_patch: nested_data.component_patch,
        }));
    }
    let component = DataComponentUnion::from(BundleContents { items });
    // SAFETY: constructed from BundleContents immediately above.
    unsafe {
        data.component_patch
            .unchecked_insert_component(DataComponentKind::BundleContents, Some(component));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_registry_ids_explain_the_azalea_field_swap() {
        assert_eq!(ItemKind::Bread.to_u32(), 981);
        assert_eq!(ItemKind::HayBlock.to_u32(), 532);
        assert_eq!(ItemKind::Granite.to_u32(), 2);
        assert_eq!(ItemKind::Diorite.to_u32(), 4);
    }

    #[test]
    fn native_26_2_template_bridge_round_trips_nested_stack() {
        let inner = ItemStack::from(ItemStackData::new(ItemKind::Bread, 2));
        let mut bundle_data = ItemStackData::new(ItemKind::Bundle, 1);
        let component = DataComponentUnion::from(BundleContents { items: vec![inner] });
        unsafe {
            bundle_data
                .component_patch
                .unchecked_insert_component(DataComponentKind::BundleContents, Some(component));
        }
        let mut stack = ItemStack::from(bundle_data);
        encode_26_2_templates(&mut stack);
        normalize_26_2_templates(&mut stack);
        let data = stack.as_present().unwrap();
        let contents = data.get_component::<BundleContents>().unwrap();
        let item = contents.items[0].as_present().unwrap();
        assert_eq!(item.kind, ItemKind::Bread);
        assert_eq!(item.count, 2);
    }

    #[test]
    fn normalizes_vanilla_26_2_item_stack_template_order() {
        for (expected_kind, expected_count) in [(ItemKind::Bread, 2), (ItemKind::HayBlock, 4)] {
            let raw = ItemStack::from(ItemStackData::new(
                ItemKind::from_u32(expected_count as u32).unwrap(),
                expected_kind.to_u32() as i32,
            ));
            let normalized = normalize_template_stack(&raw);
            let data = normalized.as_present().unwrap();
            assert_eq!(data.kind, expected_kind);
            assert_eq!(data.count, expected_count);
        }
    }
}
