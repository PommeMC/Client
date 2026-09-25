use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::builtin::ItemKind;

pub const PLAYER_SLOTS: usize = 46;
pub const HOTBAR_START: usize = 36;
const HOTBAR_END: usize = 45;
pub const MAIN_START: usize = 9;
const MAIN_END: usize = 36;
pub const ARMOR_START: usize = 5;
const ARMOR_END: usize = 9;
pub const CRAFT_INPUT_START: usize = 1;
pub const CRAFT_OUTPUT: usize = 0;
pub const OFFHAND: usize = 45;

/// A vanilla `Inventory` index (hotbar 0-8, main 9-35, armor feet to head
/// 36-39, offhand 40) as its `InventoryMenu` slot.
pub fn menu_slot_for_inventory_index(index: u32) -> Option<usize> {
    let index = index as usize;
    Some(match index {
        0..=8 => HOTBAR_START + index,
        9..=35 => index,
        36..=39 => ARMOR_START + (39 - index),
        40 => OFFHAND,
        _ => return None,
    })
}

pub struct Inventory {
    slots: Vec<ItemStack>,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: vec![ItemStack::Empty; PLAYER_SLOTS],
        }
    }

    pub fn set_contents(&mut self, items: Vec<ItemStack>) {
        self.slots = items;
        self.slots.resize(PLAYER_SLOTS, ItemStack::Empty);
    }

    pub fn set_slot(&mut self, index: usize, item: ItemStack) {
        if index < self.slots.len() {
            self.slots[index] = item;
        }
    }

    pub fn slot(&self, index: usize) -> &ItemStack {
        self.slots.get(index).unwrap_or(&ItemStack::Empty)
    }

    pub fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    pub fn main_slots(&self) -> &[ItemStack] {
        &self.slots[MAIN_START..MAIN_END]
    }

    pub fn hotbar_slots(&self) -> &[ItemStack] {
        &self.slots[HOTBAR_START..HOTBAR_END]
    }

    pub fn armor_slots(&self) -> &[ItemStack] {
        &self.slots[ARMOR_START..ARMOR_END]
    }

    pub fn craft_output(&self) -> &ItemStack {
        self.slot(CRAFT_OUTPUT)
    }

    pub fn offhand(&self) -> &ItemStack {
        self.slot(OFFHAND)
    }

    /// The non-empty stack in the selected hotbar slot.
    pub fn held_stack(&self, selected: u8) -> Option<&ItemStackData> {
        match self.hotbar_slots().get(selected as usize) {
            Some(ItemStack::Present(data)) if data.count > 0 => Some(data),
            _ => None,
        }
    }

    /// Remove one item (or the whole stack) from the selected hotbar slot,
    /// vanilla `Inventory.removeFromSelected`. Returns whether anything was
    /// removed; the server spawns the dropped item entity.
    pub fn remove_from_selected(&mut self, selected: u8, whole_stack: bool) -> bool {
        let index = HOTBAR_START + (selected as usize).min(8);
        match &mut self.slots[index] {
            ItemStack::Present(data) if data.count > 0 => {
                if whole_stack || data.count == 1 {
                    self.slots[index] = ItemStack::Empty;
                } else {
                    data.count -= 1;
                }
                true
            }
            _ => false,
        }
    }
}

/// Vanilla `Item.canFitInsideContainerItems`: false only for shulker boxes.
pub fn can_fit_inside_container_items(kind: ItemKind) -> bool {
    !azalea_registry::tags::items::SHULKER_BOXES.contains(&kind)
}

pub fn item_resource_name(kind: ItemKind) -> String {
    kind.to_string()
        .strip_prefix("minecraft:")
        .unwrap_or("air")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_indices_map_to_inventory_menu_slots() {
        // Vanilla `InventoryMenu`: armor head..feet at 5..8, hotbar last.
        let mapped = |i| menu_slot_for_inventory_index(i);
        assert_eq!(mapped(0), Some(HOTBAR_START));
        assert_eq!(mapped(8), Some(HOTBAR_END - 1));
        assert_eq!(mapped(9), Some(MAIN_START));
        assert_eq!(mapped(35), Some(MAIN_END - 1));
        assert_eq!(mapped(36), Some(ARMOR_END - 1));
        assert_eq!(mapped(39), Some(ARMOR_START));
        assert_eq!(mapped(40), Some(OFFHAND));
        assert_eq!(mapped(41), None);
    }
}
