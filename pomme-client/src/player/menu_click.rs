//! Client-side prediction of survival container clicks: a port of vanilla
//! `AbstractContainerMenu.doClick` for the menus we open (player inventory,
//! crafting table, furnace, chests). The server stays authoritative and
//! reconciles, so a wrong prediction only causes a self-correcting glitch,
//! never item dup/loss.

use azalea_inventory::components::{EquipmentSlot, Equippable};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::operations::{ClickOperation, PickupClick, QuickCraftKind, QuickMoveClick};
use azalea_inventory::{ItemStack, ItemStackData, Menu, Player, SlotList};
use azalea_registry::builtin::ItemKind;

/// Which container menu a click applies to. `Furnace` covers the furnace,
/// blast furnace, and smoker menus, which share the same slot structure;
/// `Chest` covers every generic 9xN menu (chests, ender chests, barrels, ...).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Player,
    CraftingTable,
    Furnace,
    Chest { rows: u8 },
    ShulkerBox,
    Anvil,
    Enchantment,
}

impl ContainerKind {
    pub fn slot_count(self) -> usize {
        match self {
            Self::Player | Self::CraftingTable => 46,
            Self::Furnace | Self::Anvil => 39,
            Self::Chest { rows } => rows as usize * 9 + 36,
            Self::ShulkerBox => 63,
            Self::Enchantment => 38,
        }
    }

    /// First menu slot backed by the player inventory; menu slot `i` maps to
    /// player inventory slot `i - inv_start() + 9` from here on.
    pub fn inv_start(self) -> usize {
        match self {
            Self::Player | Self::CraftingTable => 10,
            Self::Furnace | Self::Anvil => 3,
            Self::Chest { rows } => rows as usize * 9,
            Self::ShulkerBox => 27,
            Self::Enchantment => 2,
        }
    }

    /// The result slot whose clicks we can't predict, if this menu has one:
    /// crafting results need recipe logic, the anvil result costs XP and
    /// materials. Vanilla excludes the crafting result from double-click
    /// gathering (`canTakeItemForPickAll`) but not the anvil result; we skip
    /// that one too since its take costs aren't modeled here. The furnace
    /// result slot is neither: taking from it is a plain pickup.
    fn crafting_result_slot(self) -> Option<usize> {
        match self {
            Self::Player | Self::CraftingTable => Some(0),
            Self::Anvil => Some(2),
            Self::Furnace | Self::Chest { .. } | Self::ShulkerBox | Self::Enchantment => None,
        }
    }

    /// Per-slot stack limit where a menu overrides the item's own maximum
    /// (vanilla `Slot::getMaxStackSize`): the enchanting item slot holds one.
    fn slot_limit(self, s: usize) -> i32 {
        match (self, s) {
            (Self::Enchantment, 0) => 1,
            _ => i32::MAX,
        }
    }

    fn build_menu(self, slots: &[ItemStack]) -> Menu {
        let mut menu = match self {
            Self::Player => Menu::Player(Player::default()),
            Self::CraftingTable => Menu::Crafting {
                result: ItemStack::Empty,
                grid: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Furnace => Menu::Furnace {
                ingredient: ItemStack::Empty,
                fuel: ItemStack::Empty,
                result: ItemStack::Empty,
                player: SlotList::default(),
            },
            Self::Chest { rows: 1 } => Menu::Generic9x1 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Chest { rows: 2 } => Menu::Generic9x2 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Chest { rows: 3 } => Menu::Generic9x3 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Chest { rows: 4 } => Menu::Generic9x4 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Chest { rows: 5 } => Menu::Generic9x5 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Chest { .. } => Menu::Generic9x6 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::ShulkerBox => Menu::ShulkerBox {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Anvil => Menu::Anvil {
                first: ItemStack::Empty,
                second: ItemStack::Empty,
                result: ItemStack::Empty,
                player: SlotList::default(),
            },
            Self::Enchantment => Menu::Enchantment {
                item: ItemStack::Empty,
                lapis: ItemStack::Empty,
                player: SlotList::default(),
            },
        };
        for (i, item) in slots.iter().enumerate() {
            if let Some(s) = menu.slot_mut(i) {
                *s = item.clone();
            }
        }
        menu
    }

    /// Whether `item` may be placed into slot `s`: result/output slots never,
    /// player armor slots only their matching equipment, shulker contents no
    /// shulker boxes, everything else yes. Mirrors vanilla `mayPlace`. The
    /// furnace fuel slot accepts anything here: fuel values live server-side,
    /// so the server reconciles bad placements.
    fn may_place(self, s: usize, item: &ItemStackData) -> bool {
        match (self, s) {
            (Self::Player | Self::CraftingTable, 0) => false,
            (Self::Furnace | Self::Anvil, 2) => false,
            (Self::Player, 5..=8) => {
                let want = match s {
                    5 => EquipmentSlot::Head,
                    6 => EquipmentSlot::Chest,
                    7 => EquipmentSlot::Legs,
                    _ => EquipmentSlot::Feet,
                };
                item.get_component::<Equippable>().map(|c| c.slot) == Some(want)
            }
            (Self::ShulkerBox, 0..=26) => {
                !crate::player::inventory::item_resource_name(item.kind).ends_with("shulker_box")
            }
            (Self::Enchantment, 1) => item.kind == ItemKind::LapisLazuli,
            _ => true,
        }
    }
}

/// Predict a non-drag click against the given menu slots, returning the
/// changed slots (the caller applies them). Returns empty for ops we don't
/// predict, leaving those server-authoritative.
pub fn apply_click(
    kind: ContainerKind,
    slots: &[ItemStack],
    cursor: &mut ItemStack,
    op: &ClickOperation,
) -> Vec<(u16, ItemStack)> {
    // Crafting-result clicks need recipe logic; leave them to the server.
    if op
        .slot_num()
        .is_some_and(|s| Some(s as usize) == kind.crafting_result_slot())
    {
        return Vec::new();
    }
    // Vanilla routes a player-slot shift-click by whether the item is
    // smeltable or fuel; recipes and fuel values live server-side, so leave
    // those to the server too.
    if kind == ContainerKind::Furnace
        && matches!(op, ClickOperation::QuickMove(_))
        && op
            .slot_num()
            .is_some_and(|s| s as usize >= kind.inv_start())
    {
        return Vec::new();
    }
    let mut menu = kind.build_menu(slots);
    apply_op(kind, &mut menu, cursor, op);

    let mut changed = Vec::new();
    for (i, before) in slots.iter().enumerate() {
        let after = menu.slot(i).cloned().unwrap_or(ItemStack::Empty);
        if after != *before {
            changed.push((i as u16, after));
        }
    }
    changed
}

/// Distribute the carried stack across the dragged slots (left = even split,
/// right = one each), matching vanilla quick-craft. Returns each covered slot's
/// resulting stack and the remainder left on the cursor. Read-only: used for
/// both the live preview and the release commit.
pub fn drag_distribution(
    container: ContainerKind,
    slots: &[ItemStack],
    cursor: &ItemStack,
    kind: &QuickCraftKind,
    covered: &[u16],
) -> (Vec<(u16, ItemStack)>, ItemStack) {
    let ItemStack::Present(carried) = cursor else {
        return (Vec::new(), cursor.clone());
    };
    let eligible: Vec<u16> = covered
        .iter()
        .copied()
        .filter(|&s| drag_slot_eligible(container, slots, cursor, s))
        .collect();
    let n = eligible.len() as i32;
    if n == 0 {
        return (Vec::new(), cursor.clone());
    }
    let max = carried.kind.max_stack_size();
    let place = match kind {
        QuickCraftKind::Left => carried.count / n,
        QuickCraftKind::Right => 1,
        QuickCraftKind::Middle => max,
    };
    let mut remaining = carried.count;
    let mut changed = Vec::new();
    for &s in &eligible {
        let it = slots.get(s as usize).unwrap_or(&ItemStack::Empty);
        let existing = if same_item(cursor, it) { it.count() } else { 0 };
        let new_count = (place + existing).min(max.min(container.slot_limit(s as usize)));
        remaining -= new_count - existing;
        let mut stack = carried.clone();
        stack.count = new_count;
        changed.push((s, ItemStack::Present(stack)));
    }
    (changed, with_count(carried.clone(), remaining))
}

/// A drag can cover a slot only if the item may go there (vanilla gates
/// quick-craft slots on `mayPlace`) and it's empty or holds the same item as
/// the carried stack.
pub fn drag_slot_eligible(
    container: ContainerKind,
    slots: &[ItemStack],
    cursor: &ItemStack,
    slot: u16,
) -> bool {
    let ItemStack::Present(carried) = cursor else {
        return false;
    };
    if !container.may_place(slot as usize, carried) {
        return false;
    }
    let it = slots.get(slot as usize).unwrap_or(&ItemStack::Empty);
    it.is_empty() || same_item(cursor, it)
}

fn apply_op(kind: ContainerKind, menu: &mut Menu, cursor: &mut ItemStack, op: &ClickOperation) {
    match op {
        ClickOperation::Pickup(p) => match p {
            PickupClick::Left { slot: Some(s) } => {
                pickup_click(kind, menu, cursor, *s as usize, true)
            }
            PickupClick::Right { slot: Some(s) } => {
                pickup_click(kind, menu, cursor, *s as usize, false)
            }
            PickupClick::Left { slot: None } | PickupClick::LeftOutside => {
                *cursor = ItemStack::Empty; // drop whole
            }
            PickupClick::Right { slot: None } | PickupClick::RightOutside => shrink(cursor, 1),
        },
        ClickOperation::QuickMove(q) => {
            let s = match q {
                QuickMoveClick::Left { slot } | QuickMoveClick::Right { slot } => *slot as usize,
            };
            quick_move(kind, menu, s);
        }
        ClickOperation::PickupAll(_) => pickup_all(kind, menu, cursor),
        // Drag is handled at the send site; the rest have no UI path yet.
        ClickOperation::Swap(_)
        | ClickOperation::Throw(_)
        | ClickOperation::QuickCraft(_)
        | ClickOperation::Clone(_) => {}
    }
}

/// Left/right click on a slot, following vanilla `doClick` PICKUP: `primary` is
/// left (whole stack), otherwise right (one / rounded-up half). Respects
/// `may_place` so restricted slots (armor) reject the wrong item.
fn pickup_click(
    kind: ContainerKind,
    menu: &mut Menu,
    cursor: &mut ItemStack,
    s: usize,
    primary: bool,
) {
    let mut slot_item = take_slot(menu, s);
    let mut carried = std::mem::take(cursor);
    if slot_item.is_empty() {
        let can_place = carried.as_present().is_some_and(|c| kind.may_place(s, c));
        if can_place {
            let amount = if primary { carried.count() } else { 1 };
            safe_insert(kind, s, &mut slot_item, &mut carried, amount);
        }
    } else if carried.is_empty() {
        let total = slot_item.count();
        let amount = if primary { total } else { (total + 1) / 2 };
        carried = slot_item.split(amount as u32);
    } else if carried.as_present().is_some_and(|c| kind.may_place(s, c)) {
        if same_item(&carried, &slot_item) {
            let amount = if primary { carried.count() } else { 1 };
            safe_insert(kind, s, &mut slot_item, &mut carried, amount);
        } else if carried
            .as_present()
            .is_some_and(|c| c.count <= c.kind.max_stack_size().min(kind.slot_limit(s)))
        {
            // Vanilla swaps only when the carried stack fits the slot's limit.
            std::mem::swap(&mut carried, &mut slot_item);
        }
    } else if same_item(&carried, &slot_item) {
        // Slot won't accept a placement but holds the same item: pull it into hand.
        merge_into(&mut carried, &mut slot_item);
    }
    put_slot(menu, s, slot_item);
    *cursor = carried;
}

/// Move up to `amount` of `carried` into `slot` (empty or same item), capped to
/// the item's max stack and the slot's own limit, like vanilla
/// `Slot::safeInsert`.
fn safe_insert(
    kind: ContainerKind,
    s: usize,
    slot: &mut ItemStack,
    carried: &mut ItemStack,
    amount: i32,
) {
    let ItemStack::Present(c) = carried.clone() else {
        return;
    };
    let max = c.kind.max_stack_size().min(kind.slot_limit(s));
    let take = match slot {
        ItemStack::Empty => amount.min(c.count).min(max),
        ItemStack::Present(d) => amount.min(c.count).min((max - d.count).max(0)),
    };
    if take <= 0 {
        return;
    }
    match slot {
        ItemStack::Present(d) => d.count += take,
        ItemStack::Empty => {
            let mut d = c;
            d.count = take;
            *slot = ItemStack::Present(d);
        }
    }
    shrink(carried, take);
}

/// Shift-click, repeating until it stops making progress (vanilla loops too).
/// Chests move between the contents and player regions like `ChestMenu`
/// (player-bound reversed); the anvil only ever tries the input slots
/// (`canMoveIntoInputSlots` is true, making `ItemCombinerMenu`'s main/hotbar
/// branches dead code); the furnace moves its own slots to the player region
/// like `AbstractFurnaceMenu` (result reversed; player-slot clicks never get
/// predicted). The player and crafting menus use azalea's `quick_move_stack`,
/// whose routing matches vanilla closely enough for them.
fn quick_move(kind: ContainerKind, menu: &mut Menu, s: usize) {
    for _ in 0..menu.len() {
        let before = menu.slot(s).map(ItemStack::count).unwrap_or(0);
        if before == 0 {
            break;
        }
        match kind {
            ContainerKind::Chest { .. } | ContainerKind::ShulkerBox => {
                let split = kind.inv_start();
                if s < split {
                    move_item_stack_to(kind, menu, s, split..menu.len(), true);
                } else {
                    move_item_stack_to(kind, menu, s, 0..split, false);
                }
            }
            ContainerKind::Anvil => {
                if s < 3 {
                    move_item_stack_to(kind, menu, s, 3..menu.len(), false);
                } else {
                    move_item_stack_to(kind, menu, s, 0..2, false);
                }
            }
            ContainerKind::Furnace => {
                move_item_stack_to(kind, menu, s, 3..menu.len(), s == 2);
            }
            ContainerKind::Enchantment => {
                // Vanilla `EnchantmentMenu.quickMoveStack`: menu slots go to
                // the player region; lapis fills its slot; anything else moves
                // a single item into the empty item slot.
                if s < 2 {
                    move_item_stack_to(kind, menu, s, 2..menu.len(), true);
                } else if menu
                    .slot(s)
                    .and_then(ItemStack::as_present)
                    .is_some_and(|d| d.kind == ItemKind::LapisLazuli)
                {
                    move_item_stack_to(kind, menu, s, 1..2, true);
                } else if menu.slot(0).is_some_and(ItemStack::is_empty)
                    && let ItemStack::Present(d) = take_slot(menu, s)
                {
                    put_slot(menu, s, with_count(d.clone(), d.count - 1));
                    put_slot(menu, 0, with_count(d, 1));
                }
            }
            _ => {
                menu.quick_move_stack(s);
            }
        }
        if menu.slot(s).map(ItemStack::count).unwrap_or(0) == before {
            break;
        }
    }
}

/// Vanilla `AbstractContainerMenu.moveItemStackTo`: first merge into matching
/// stacks across `range` (back to front when `reverse`), then place the
/// remainder into the first empty slot that accepts it.
fn move_item_stack_to(
    kind: ContainerKind,
    menu: &mut Menu,
    src: usize,
    range: std::ops::Range<usize>,
    reverse: bool,
) {
    let mut moving = take_slot(menu, src);
    let indices: Vec<usize> = if reverse {
        range.rev().collect()
    } else {
        range.collect()
    };

    if moving
        .as_present()
        .is_some_and(|d| d.kind.max_stack_size() > 1)
    {
        for &i in &indices {
            if moving.is_empty() {
                break;
            }
            if let Some(slot) = menu.slot_mut(i)
                && same_item(slot, &moving)
            {
                merge_into(slot, &mut moving);
            }
        }
    }

    if let ItemStack::Present(data) = &moving {
        for &i in &indices {
            if !kind.may_place(i, data) {
                continue;
            }
            if let Some(slot) = menu.slot_mut(i)
                && slot.is_empty()
            {
                *slot = std::mem::take(&mut moving);
                break;
            }
        }
    }

    put_slot(menu, src, moving);
}

/// Double-click: gather matching items from every slot but the crafting-result
/// slot onto the cursor up to a full stack, partial stacks first (vanilla
/// `PICKUP_ALL` + `canTakeItemForPickAll`).
fn pickup_all(kind: ContainerKind, menu: &mut Menu, cursor: &mut ItemStack) {
    let ItemStack::Present(carried) = cursor else {
        return;
    };
    let max = carried.kind.max_stack_size();
    for pass in 0..2 {
        for s in 0..menu.len() {
            if Some(s) == kind.crafting_result_slot() {
                continue;
            }
            if cursor.count() >= max {
                break;
            }
            let slot_count = menu.slot(s).map(ItemStack::count).unwrap_or(0);
            if slot_count == 0 || !same_item(cursor, menu.slot(s).unwrap()) {
                continue;
            }
            if pass == 0 && slot_count >= max {
                continue; // leave full stacks for the second pass
            }
            let take = (max - cursor.count()).min(slot_count);
            shrink_slot(menu, s, take);
            if let ItemStack::Present(c) = cursor {
                c.count += take;
            }
        }
    }
}

fn merge_into(dst: &mut ItemStack, src: &mut ItemStack) {
    if let (ItemStack::Present(d), ItemStack::Present(s)) = (&mut *dst, &mut *src) {
        let moved = (d.kind.max_stack_size() - d.count).max(0).min(s.count);
        d.count += moved;
        s.count -= moved;
    }
    src.update_empty();
}

fn take_slot(menu: &mut Menu, s: usize) -> ItemStack {
    menu.slot_mut(s)
        .map(std::mem::take)
        .unwrap_or(ItemStack::Empty)
}

fn put_slot(menu: &mut Menu, s: usize, item: ItemStack) {
    if let Some(sl) = menu.slot_mut(s) {
        *sl = item;
    }
}

fn shrink(item: &mut ItemStack, n: i32) {
    if let ItemStack::Present(d) = item {
        d.count -= n;
    }
    item.update_empty();
}

fn shrink_slot(menu: &mut Menu, s: usize, n: i32) {
    if let Some(sl) = menu.slot_mut(s) {
        shrink(sl, n);
    }
}

fn same_item(a: &ItemStack, b: &ItemStack) -> bool {
    match (a, b) {
        (ItemStack::Present(x), ItemStack::Present(y)) => x.is_same_item_and_components(y),
        _ => false,
    }
}

fn with_count(mut data: ItemStackData, count: i32) -> ItemStack {
    if count > 0 {
        data.count = count;
        ItemStack::Present(data)
    } else {
        ItemStack::Empty
    }
}
