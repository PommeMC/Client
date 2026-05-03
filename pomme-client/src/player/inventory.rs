use azalea_inventory::ItemStack;
use azalea_registry::builtin::ItemKind;

const PLAYER_SLOTS: usize = 46;
const HOTBAR_START: usize = 36;
const HOTBAR_END: usize = 45;
const MAIN_START: usize = 9;
const MAIN_END: usize = 36;
const ARMOR_START: usize = 5;
const ARMOR_END: usize = 9;
const CRAFT_INPUT_START: usize = 1;
const CRAFT_INPUT_END: usize = 5;
const CRAFT_OUTPUT: usize = 0;
const OFFHAND: usize = 45;

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

    pub fn main_slots(&self) -> &[ItemStack] {
        &self.slots[MAIN_START..MAIN_END]
    }

    pub fn hotbar_slots(&self) -> &[ItemStack] {
        &self.slots[HOTBAR_START..HOTBAR_END]
    }

    pub fn armor_slots(&self) -> &[ItemStack] {
        &self.slots[ARMOR_START..ARMOR_END]
    }

    pub fn craft_input_slots(&self) -> &[ItemStack] {
        &self.slots[CRAFT_INPUT_START..CRAFT_INPUT_END]
    }

    pub fn craft_output(&self) -> &ItemStack {
        self.slot(CRAFT_OUTPUT)
    }

    pub fn offhand(&self) -> &ItemStack {
        self.slot(OFFHAND)
    }
}

pub fn item_resource_name(kind: ItemKind) -> String {
    kind.to_string()
        .strip_prefix("minecraft:")
        .unwrap_or("air")
        .to_string()
}
