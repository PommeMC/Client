//! Client-side prediction of survival container clicks: a port of vanilla
//! `AbstractContainerMenu.doClick` for the player inventory (container 0). The
//! server stays authoritative and reconciles, so a wrong prediction only causes
//! a self-correcting glitch, never item dup/loss.

use azalea_inventory::components::{EquipmentSlot, Equippable};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::operations::{
    ClickOperation, PickupClick, QuickCraftKind, QuickMoveClick, ThrowClick,
};
use azalea_inventory::{ItemStack, ItemStackData, Menu, Player};

use crate::player::inventory::Inventory;

const SLOTS: usize = 46;
const CRAFT_RESULT: u16 = 0;
const HOTBAR_BASE: usize = 36;
const OFFHAND: usize = 45;

/// Apply a non-drag click to the local player menu, returning the changed
/// slots. Returns empty (mutating nothing) for ops we don't predict, leaving
/// those server-authoritative.
pub fn apply_click(
    inv: &mut Inventory,
    cursor: &mut ItemStack,
    op: &ClickOperation,
) -> Vec<(u16, ItemStack)> {
    // Crafting-result clicks need recipe logic; leave them to the server.
    if op.slot_num() == Some(CRAFT_RESULT) {
        return Vec::new();
    }
    let pre: Vec<ItemStack> = (0..SLOTS).map(|i| inv.slot(i).clone()).collect();
    let mut menu = build_menu(&pre);
    apply_op(&mut menu, cursor, op);

    let mut changed = Vec::new();
    for (i, before) in pre.iter().enumerate() {
        let after = menu.slot(i).cloned().unwrap_or(ItemStack::Empty);
        if after != *before {
            inv.set_slot(i, after.clone());
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
    inv: &Inventory,
    cursor: &ItemStack,
    kind: &QuickCraftKind,
    slots: &[u16],
) -> (Vec<(u16, ItemStack)>, ItemStack) {
    let ItemStack::Present(carried) = cursor else {
        return (Vec::new(), cursor.clone());
    };
    let eligible: Vec<u16> = slots
        .iter()
        .copied()
        .filter(|&s| drag_slot_eligible(inv, cursor, s))
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
        let it = inv.slot(s as usize);
        let existing = if same_item(cursor, it) { it.count() } else { 0 };
        let new_count = (place + existing).min(max);
        remaining -= new_count - existing;
        let mut stack = carried.clone();
        stack.count = new_count;
        changed.push((s, ItemStack::Present(stack)));
    }
    (changed, with_count(carried.clone(), remaining))
}

/// A drag can cover a slot only if it's empty or holds the same item as the
/// carried stack.
pub fn drag_slot_eligible(inv: &Inventory, cursor: &ItemStack, slot: u16) -> bool {
    if cursor.is_empty() {
        return false;
    }
    let it = inv.slot(slot as usize);
    it.is_empty() || same_item(cursor, it)
}

fn build_menu(slots: &[ItemStack]) -> Menu {
    let mut menu = Menu::Player(Player::default());
    for (i, item) in slots.iter().enumerate() {
        if let Some(s) = menu.slot_mut(i) {
            *s = item.clone();
        }
    }
    menu
}

fn apply_op(menu: &mut Menu, cursor: &mut ItemStack, op: &ClickOperation) {
    match op {
        ClickOperation::Pickup(p) => match p {
            PickupClick::Left { slot: Some(s) } => pickup_click(menu, cursor, *s as usize, true),
            PickupClick::Right { slot: Some(s) } => pickup_click(menu, cursor, *s as usize, false),
            PickupClick::Left { slot: None } | PickupClick::LeftOutside => {
                *cursor = ItemStack::Empty; // drop whole
            }
            PickupClick::Right { slot: None } | PickupClick::RightOutside => shrink(cursor, 1),
        },
        ClickOperation::QuickMove(q) => {
            let s = match q {
                QuickMoveClick::Left { slot } | QuickMoveClick::Right { slot } => *slot as usize,
            };
            quick_move(menu, s);
        }
        ClickOperation::Swap(sw) => swap_hotbar(menu, sw.source_slot as usize, sw.target_slot),
        ClickOperation::Throw(t) => match t {
            ThrowClick::Single { slot } => shrink_slot(menu, *slot as usize, 1),
            ThrowClick::All { slot } => put_slot(menu, *slot as usize, ItemStack::Empty),
        },
        ClickOperation::PickupAll(_) => pickup_all(menu, cursor),
        // Drag is handled at the send site; clone is creative-only.
        ClickOperation::QuickCraft(_) | ClickOperation::Clone(_) => {}
    }
}

/// Left/right click on a slot, following vanilla `doClick` PICKUP: `primary` is
/// left (whole stack), otherwise right (one / rounded-up half). Respects
/// `may_place` so restricted slots (armor) reject the wrong item.
fn pickup_click(menu: &mut Menu, cursor: &mut ItemStack, s: usize, primary: bool) {
    let mut slot_item = take_slot(menu, s);
    let mut carried = std::mem::take(cursor);
    if slot_item.is_empty() {
        let can_place = carried.as_present().is_some_and(|c| may_place(s, c));
        if can_place {
            let amount = if primary { carried.count() } else { 1 };
            safe_insert(&mut slot_item, &mut carried, amount);
        }
    } else if carried.is_empty() {
        let total = slot_item.count();
        let amount = if primary { total } else { (total + 1) / 2 };
        carried = slot_item.split(amount as u32);
    } else if carried.as_present().is_some_and(|c| may_place(s, c)) {
        if same_item(&carried, &slot_item) {
            let amount = if primary { carried.count() } else { 1 };
            safe_insert(&mut slot_item, &mut carried, amount);
        } else {
            std::mem::swap(&mut carried, &mut slot_item);
        }
    } else if same_item(&carried, &slot_item) {
        // Slot won't accept a placement but holds the same item: pull it into hand.
        merge_into(&mut carried, &mut slot_item);
    }
    put_slot(menu, s, slot_item);
    *cursor = carried;
}

/// Whether `item` may be placed into slot `s`: crafting result never, armor
/// slots only their matching equipment, everything else yes. Mirrors vanilla
/// `mayPlace`.
fn may_place(s: usize, item: &ItemStackData) -> bool {
    match s {
        0 => false,
        5..=8 => {
            let want = match s {
                5 => EquipmentSlot::Head,
                6 => EquipmentSlot::Chest,
                7 => EquipmentSlot::Legs,
                _ => EquipmentSlot::Feet,
            };
            item.get_component::<Equippable>().map(|c| c.slot) == Some(want)
        }
        _ => true,
    }
}

/// Move up to `amount` of `carried` into `slot` (empty or same item), capped to
/// the item's max stack, like vanilla `Slot::safeInsert`.
fn safe_insert(slot: &mut ItemStack, carried: &mut ItemStack, amount: i32) {
    let ItemStack::Present(c) = carried.clone() else {
        return;
    };
    let max = c.kind.max_stack_size();
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

/// Shift-click: let azalea's `quick_move_stack` move the stack to its
/// destination, repeating until it stops making progress (vanilla loops too).
fn quick_move(menu: &mut Menu, s: usize) {
    for _ in 0..SLOTS {
        let before = menu.slot(s).map(ItemStack::count).unwrap_or(0);
        if before == 0 {
            break;
        }
        menu.quick_move_stack(s);
        if menu.slot(s).map(ItemStack::count).unwrap_or(0) == before {
            break;
        }
    }
}

fn swap_hotbar(menu: &mut Menu, source: usize, target_slot: u8) {
    let target = match target_slot {
        0..=8 => HOTBAR_BASE + target_slot as usize,
        40 => OFFHAND,
        _ => return,
    };
    if source >= SLOTS {
        return;
    }
    let a = take_slot(menu, source);
    let b = take_slot(menu, target);
    put_slot(menu, source, b);
    put_slot(menu, target, a);
}

/// Double-click: gather matching items from the player inventory onto the
/// cursor up to a full stack, partial stacks first (vanilla `PICKUP_ALL`).
fn pickup_all(menu: &mut Menu, cursor: &mut ItemStack) {
    let ItemStack::Present(carried) = cursor else {
        return;
    };
    let max = carried.kind.max_stack_size();
    for pass in 0..2 {
        for s in 9..OFFHAND {
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
