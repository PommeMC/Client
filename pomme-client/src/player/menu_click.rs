//! Client-side prediction of survival container clicks: a port of vanilla
//! `AbstractContainerMenu.doClick` for the menus we open (player inventory,
//! crafting table). The server stays authoritative and reconciles, so a wrong
//! prediction only causes a self-correcting glitch, never item dup/loss.

use azalea_inventory::components::{EquipmentSlot, Equippable};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::operations::{ClickOperation, PickupClick, QuickCraftKind, QuickMoveClick};
use azalea_inventory::{ItemStack, ItemStackData, Menu, Player, SlotList};

/// Which container menu a click applies to. Both menus have 46 slots with the
/// result/output at index 0.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Player,
    CraftingTable,
}

impl ContainerKind {
    fn build_menu(self, slots: &[ItemStack]) -> Menu {
        let mut menu = match self {
            Self::Player => Menu::Player(Player::default()),
            Self::CraftingTable => Menu::Crafting {
                result: ItemStack::Empty,
                grid: SlotList::default(),
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

    /// Whether `item` may be placed into slot `s`: the result slot never,
    /// player armor slots only their matching equipment, everything else yes.
    /// Mirrors vanilla `mayPlace`.
    fn may_place(self, s: usize, item: &ItemStackData) -> bool {
        match (self, s) {
            (_, 0) => false,
            (Self::Player, 5..=8) => {
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
    if op.slot_num() == Some(0) {
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
        let new_count = (place + existing).min(max);
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
            quick_move(menu, s);
        }
        ClickOperation::PickupAll(_) => pickup_all(menu, cursor),
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
            safe_insert(&mut slot_item, &mut carried, amount);
        }
    } else if carried.is_empty() {
        let total = slot_item.count();
        let amount = if primary { total } else { (total + 1) / 2 };
        carried = slot_item.split(amount as u32);
    } else if carried.as_present().is_some_and(|c| kind.may_place(s, c)) {
        if same_item(&carried, &slot_item) {
            let amount = if primary { carried.count() } else { 1 };
            safe_insert(&mut slot_item, &mut carried, amount);
        } else if carried
            .as_present()
            .is_some_and(|c| c.count <= c.kind.max_stack_size())
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
    for _ in 0..menu.len() {
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

/// Double-click: gather matching items from every slot but the result slot
/// onto the cursor up to a full stack, partial stacks first (vanilla
/// `PICKUP_ALL` + `canTakeItemForPickAll`).
fn pickup_all(menu: &mut Menu, cursor: &mut ItemStack) {
    let ItemStack::Present(carried) = cursor else {
        return;
    };
    let max = carried.kind.max_stack_size();
    for pass in 0..2 {
        for s in 1..menu.len() {
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
