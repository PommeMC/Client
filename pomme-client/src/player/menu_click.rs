//! Client-side prediction of survival container clicks: a port of vanilla
//! `AbstractContainerMenu.doClick` for the menus we open (player inventory,
//! crafting table, furnace, chests). The server stays authoritative and
//! reconciles, so a wrong prediction only causes a self-correcting glitch,
//! never item dup/loss.

use azalea_inventory::components::{BundleContents, Equippable};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::operations::{
    ClickOperation, PickupClick, QuickCraftKind, QuickMoveClick, ThrowClick,
};
use azalea_inventory::{ItemStack, ItemStackData, Menu, Player, SlotList};
use azalea_registry::builtin::ItemKind;

/// Which container menu a click applies to. `Furnace` covers the furnace,
/// blast furnace, and smoker menus, which share the same slot structure;
/// `Chest` covers every generic 9xN menu (chests, ender chests, barrels, ...).
/// `Crafter` carries the disabled grid slots as a bitmask (bit `i` = slot `i`),
/// which gate placement there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Player,
    CraftingTable,
    Furnace,
    Chest { rows: u8 },
    ShulkerBox,
    Anvil,
    Enchantment,
    Hopper,
    Dispenser,
    BrewingStand,
    Crafter { disabled: u16 },
}

impl ContainerKind {
    pub fn slot_count(self) -> usize {
        match self {
            Self::Player | Self::CraftingTable => 46,
            Self::Furnace | Self::Anvil => 39,
            Self::Chest { rows } => rows as usize * 9 + 36,
            Self::ShulkerBox => 63,
            Self::Enchantment => 38,
            Self::Hopper | Self::BrewingStand => 41,
            Self::Dispenser => 45,
            Self::Crafter { .. } => 46,
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
            Self::Hopper | Self::BrewingStand => 5,
            Self::Dispenser | Self::Crafter { .. } => 9,
        }
    }

    /// End (exclusive) of the player-inventory slots. Only the crafter puts a
    /// slot after them, its result.
    pub fn inv_end(self) -> usize {
        self.inv_start() + 36
    }

    /// The result slot whose clicks we can't predict, if this menu has one:
    /// crafting results need recipe logic, the anvil result costs XP and
    /// materials. Vanilla excludes the crafting result from double-click
    /// gathering (`canTakeItemForPickAll`) but not the anvil result; we skip
    /// that one too since its take costs aren't modeled here. The crafter's
    /// result (`NonInteractiveResultSlot`) ignores every click, so nothing
    /// changes there either. The furnace result slot is neither: taking from
    /// it is a plain pickup.
    fn crafting_result_slot(self) -> Option<usize> {
        match self {
            Self::Player | Self::CraftingTable => Some(0),
            Self::Anvil => Some(2),
            Self::Crafter { .. } => Some(45),
            Self::Furnace
            | Self::Chest { .. }
            | Self::ShulkerBox
            | Self::Enchantment
            | Self::Hopper
            | Self::Dispenser
            | Self::BrewingStand => None,
        }
    }

    /// Menu slot holding hotbar index `i` (0-8), the SWAP click target.
    pub fn hotbar_menu_slot(self, i: u8) -> u16 {
        match self {
            // The player menu's offhand slot sits after the hotbar.
            Self::Player => 36 + i as u16,
            _ => (self.inv_start() + 27) as u16 + i as u16,
        }
    }

    /// The offhand's menu slot, only present in the player menu. Other menus
    /// reach the offhand through the player inventory directly, which slot
    /// prediction can't see.
    pub fn offhand_menu_slot(self) -> Option<u16> {
        matches!(self, Self::Player).then_some(45)
    }

    /// Slots whose `mayPlace` needs server data: the brewing stand's
    /// ingredient (`brewing_reagent` set, 26.2's `PotionBrewing`) and fuel
    /// (`brewing_fuel` component, 26.2's tag). A click putting a new item
    /// there isn't predicted, so a refused one doesn't show for a round trip.
    fn unpredicted_place(self, s: usize) -> bool {
        matches!((self, s), (Self::BrewingStand, 3 | 4))
    }

    /// Per-slot stack limit where a menu overrides the item's own maximum
    /// (vanilla `Slot::getMaxStackSize`): the enchanting item slot and the
    /// brewing stand's bottle slots hold one.
    fn slot_limit(self, s: usize) -> i32 {
        match (self, s) {
            (Self::Enchantment, 0) | (Self::BrewingStand, 0..=2) => 1,
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
            Self::Hopper => Menu::Hopper {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::Dispenser => Menu::Generic3x3 {
                contents: SlotList::default(),
                player: SlotList::default(),
            },
            Self::BrewingStand => Menu::BrewingStand {
                bottles: SlotList::default(),
                ingredient: ItemStack::Empty,
                fuel: ItemStack::Empty,
                player: SlotList::default(),
            },
            Self::Crafter { .. } => Menu::Crafter3x3 {
                contents: SlotList::default(),
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
            // `PotionSlot`: 26.2's four items, which are also everything 26.3's
            // `brewing_input` set and `brewing_potion_inputs` tag hold.
            (Self::BrewingStand, 0..=2) => matches!(
                item.kind,
                ItemKind::Potion
                    | ItemKind::SplashPotion
                    | ItemKind::LingeringPotion
                    | ItemKind::GlassBottle
            ),
            (Self::Crafter { disabled }, 0..=8) => disabled & (1 << s) == 0,
            (Self::Crafter { .. }, 45) => false,
            (Self::Player, 5..=8) => {
                let want = crate::player::inventory::ARMOR_EQUIPMENT
                    [s - crate::player::inventory::ARMOR_START];
                item.get_component::<Equippable>().map(|c| c.slot) == Some(want)
            }
            (Self::ShulkerBox, 0..=26) => {
                crate::player::inventory::can_fit_inside_container_items(item.kind)
            }
            (Self::Enchantment, 1) => item.kind == ItemKind::LapisLazuli,
            _ => true,
        }
    }

    /// Vanilla `Slot.allowModification`: `mayPickup && mayPlace(getItem())`.
    // TODO: `mayPickup` (binding-cursed armor outside creative) isn't modelled.
    fn allow_modification(self, s: usize, item: &ItemStackData) -> bool {
        self.may_place(s, item)
    }
}

/// A bundle click's local side: the hovered bundle's selection going in, and
/// the `BundleItem` sound and selection reset coming out.
#[derive(Default)]
pub struct BundleClick {
    /// The hovered bundle's slot and selected entry; cleared when the click
    /// resets it (`removeOne`, `toggleSelectedItem(-1)`).
    pub selection: Option<(u16, i32)>,
    pub sound: Option<crate::ui::bundle::Sound>,
}

/// Predict a non-drag click against the given menu slots, returning the
/// changed slots (the caller applies them). Returns empty for ops we don't
/// predict, leaving those server-authoritative.
pub fn apply_click(
    kind: ContainerKind,
    slots: &[ItemStack],
    cursor: &mut ItemStack,
    op: &ClickOperation,
    creative: bool,
    bundle: &mut BundleClick,
) -> Vec<(u16, ItemStack)> {
    // Crafting-result clicks need recipe logic; leave them to the server.
    if op
        .slot_num()
        .is_some_and(|s| Some(s as usize) == kind.crafting_result_slot())
    {
        return Vec::new();
    }
    // Vanilla routes a player-slot shift-click by whether the item is
    // smeltable or fuel (or a brewing reagent or fuel); recipes and fuel
    // values live server-side, so leave those to the server too.
    if matches!(kind, ContainerKind::Furnace | ContainerKind::BrewingStand)
        && matches!(op, ClickOperation::QuickMove(_))
        && op
            .slot_num()
            .is_some_and(|s| s as usize >= kind.inv_start())
    {
        return Vec::new();
    }
    let mut menu = kind.build_menu(slots);
    let (carried, selection) = (cursor.clone(), bundle.selection);
    apply_op(kind, &mut menu, cursor, op, creative, bundle);

    let mut changed = Vec::new();
    for (i, before) in slots.iter().enumerate() {
        // Slots past azalea's menu (the crafter result) are left as they are.
        let Some(after) = menu.slot(i) else {
            continue;
        };
        if after == before {
            continue;
        }
        if kind.unpredicted_place(i) && !after.is_empty() && !same_item(before, after) {
            *cursor = carried;
            *bundle = BundleClick {
                selection,
                sound: None,
            };
            return Vec::new();
        }
        changed.push((i as u16, after.clone()));
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

fn apply_op(
    kind: ContainerKind,
    menu: &mut Menu,
    cursor: &mut ItemStack,
    op: &ClickOperation,
    creative: bool,
    bundle: &mut BundleClick,
) {
    match op {
        ClickOperation::Pickup(p) => match p {
            PickupClick::Left { slot: Some(s) } => {
                pickup_click(kind, menu, cursor, *s as usize, true, bundle)
            }
            PickupClick::Right { slot: Some(s) } => {
                pickup_click(kind, menu, cursor, *s as usize, false, bundle)
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
        ClickOperation::Swap(s) => {
            let target = match s.target_slot {
                i @ 0..=8 => Some(kind.hotbar_menu_slot(i) as usize),
                40 => kind.offhand_menu_slot().map(usize::from),
                _ => None,
            };
            // Unmappable target (offhand outside the player menu): leave the
            // swap to the server.
            let Some(target) = target else {
                return;
            };
            swap_click(kind, menu, s.source_slot as usize, target);
        }
        ClickOperation::Throw(t) => {
            // Vanilla THROW only acts with an empty cursor.
            if cursor.is_present() {
                return;
            }
            match t {
                ThrowClick::Single { slot } => {
                    let mut item = take_slot(menu, *slot as usize);
                    shrink(&mut item, 1);
                    put_slot(menu, *slot as usize, item);
                }
                ThrowClick::All { slot } => put_slot(menu, *slot as usize, ItemStack::Empty),
            }
        }
        ClickOperation::Clone(c) => {
            // Vanilla CLONE: creative only, empty cursor, fills to a full stack.
            if creative
                && cursor.is_empty()
                && let Some(ItemStack::Present(d)) = menu.slot(c.slot as usize)
            {
                let mut full = d.clone();
                full.count = full.kind.max_stack_size();
                *cursor = ItemStack::Present(full);
            }
        }
        // Drag is handled at the send site.
        ClickOperation::QuickCraft(_) => {}
    }
}

/// Vanilla `doClick` SWAP: exchange the hovered slot with a hotbar/offhand
/// slot (`held` in vanilla terms), respecting `mayPlace` and slot limits.
fn swap_click(kind: ContainerKind, menu: &mut Menu, source: usize, target: usize) {
    let held = take_slot(menu, target);
    let slot_item = take_slot(menu, source);
    let (new_slot, new_held) = match (held, slot_item) {
        (ItemStack::Empty, ItemStack::Empty) => (ItemStack::Empty, ItemStack::Empty),
        (ItemStack::Empty, item) => (ItemStack::Empty, item),
        (held @ ItemStack::Present(_), slot_item) => {
            let h = held.as_present().unwrap();
            let max = h.kind.max_stack_size().min(kind.slot_limit(source));
            if !kind.may_place(source, h) || h.count > max {
                // Over-limit swaps re-add overflow to the inventory
                // server-side; not predicted.
                (slot_item, held)
            } else {
                (held, slot_item)
            }
        }
    };
    put_slot(menu, source, new_slot);
    put_slot(menu, target, new_held);
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
    bundle: &mut BundleClick,
) {
    let mut slot_item = take_slot(menu, s);
    let mut carried = std::mem::take(cursor);
    // Vanilla `tryItemClickBehaviourOverride`: the carried stack first.
    let overridden = stacked_on_other(kind, s, &mut carried, &mut slot_item, primary, bundle)
        || other_stacked_on_me(kind, s, &mut slot_item, &mut carried, primary, bundle);
    if overridden {
        put_slot(menu, s, slot_item);
        *cursor = carried;
        return;
    }
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

fn bundle_contents(stack: &ItemStack) -> Option<BundleContents> {
    stack
        .as_present()
        .filter(|d| crate::ui::bundle::is_bundle(d))
        .and_then(crate::ui::bundle::contents)
        .map(std::borrow::Cow::into_owned)
}

/// Vanilla `BundleItem.overrideStackedOnOther`: the carried bundle clicked on
/// a slot. Picking a bundle up cleared its selection, so it removes entry 0.
fn stacked_on_other(
    kind: ContainerKind,
    s: usize,
    carried: &mut ItemStack,
    slot: &mut ItemStack,
    primary: bool,
    bundle: &mut BundleClick,
) -> bool {
    use crate::ui::bundle::{NO_SELECTION, Sound, remove_one, try_insert};
    let Some(mut contents) = bundle_contents(carried) else {
        return false;
    };
    if primary && !slot.is_empty() {
        bundle.sound = Some(Sound::insert(
            try_transfer(kind, s, &mut contents, slot) > 0,
        ));
    } else if !primary && slot.is_empty() {
        if let Some(mut removed) = remove_one(&mut contents, NO_SELECTION) {
            // `Slot.safeInsert`.
            if removed.as_present().is_some_and(|d| kind.may_place(s, d)) {
                let count = removed.count();
                safe_insert(kind, s, slot, &mut removed, count);
            }
            if removed.is_empty() {
                bundle.sound = Some(Sound::RemoveOne);
            } else {
                try_insert(&mut contents, &mut removed);
            }
        }
    } else {
        return false;
    }
    crate::ui::bundle::set_contents(carried, contents);
    true
}

/// Vanilla `BundleItem.overrideOtherStackedOnMe`: the cursor clicked on a
/// bundle in a slot.
fn other_stacked_on_me(
    kind: ContainerKind,
    s: usize,
    slot: &mut ItemStack,
    carried: &mut ItemStack,
    primary: bool,
    bundle: &mut BundleClick,
) -> bool {
    use crate::ui::bundle::{NO_SELECTION, Sound, remove_one, try_insert};
    let Some(mut contents) = bundle_contents(slot) else {
        return false;
    };
    let hovered = bundle.selection.filter(|(slot, _)| usize::from(*slot) == s);
    let allow = slot
        .as_present()
        .is_some_and(|d| kind.allow_modification(s, d));
    let (handled, reset) = if primary && !carried.is_empty() {
        let inserted = allow && try_insert(&mut contents, carried) > 0;
        bundle.sound = Some(Sound::insert(inserted));
        (true, false)
    } else if !primary && carried.is_empty() {
        let selected = hovered.map_or(NO_SELECTION, |(_, selected)| selected);
        let removed = allow.then(|| remove_one(&mut contents, selected)).flatten();
        let reset = removed.is_some();
        if let Some(removed) = removed {
            bundle.sound = Some(Sound::RemoveOne);
            *carried = removed;
        }
        (true, reset)
    } else {
        // Picking the bundle up, or right-clicking it while holding
        // something: `toggleSelectedItem(-1)` and the ordinary click.
        (false, true)
    };
    if reset && hovered.is_some() {
        bundle.selection = None;
    }
    if handled {
        crate::ui::bundle::set_contents(slot, contents);
    }
    handled
}

/// Vanilla `BundleContents.Mutable.tryTransfer` through `Slot.safeTake`.
fn try_transfer(
    kind: ContainerKind,
    s: usize,
    contents: &mut BundleContents,
    slot: &mut ItemStack,
) -> i32 {
    use crate::ui::bundle::{item_weight, max_amount_to_add, try_insert, weight};
    let Some(other) = slot.as_present() else {
        return 0;
    };
    let (Some(current), Some(each)) = (weight(&contents.items), item_weight(other)) else {
        return 0;
    };
    let max_amount = max_amount_to_add(current, each);
    // `Slot.tryRemove`: an unmodifiable slot only gives up its whole stack.
    if !kind.allow_modification(s, other) && max_amount < other.count {
        return 0;
    }
    let take = other.count.min(max_amount);
    if take <= 0 {
        return 0;
    }
    let mut taken = slot.split(take as u32);
    try_insert(contents, &mut taken)
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
/// predicted), as does the brewing stand (`BrewingStandMenu`, player-bound
/// reversed). The crafter routes like a chest (`CrafterMenu`). The player and
/// crafting menus use azalea's `quick_move_stack`,
/// whose routing matches vanilla closely enough for them.
fn quick_move(kind: ContainerKind, menu: &mut Menu, s: usize) {
    for _ in 0..menu.len() {
        let before = menu.slot(s).map(ItemStack::count).unwrap_or(0);
        if before == 0 {
            break;
        }
        match kind {
            ContainerKind::Chest { .. }
            | ContainerKind::ShulkerBox
            | ContainerKind::Hopper
            | ContainerKind::Dispenser
            | ContainerKind::Crafter { .. } => {
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
            ContainerKind::BrewingStand => {
                move_item_stack_to(kind, menu, s, 5..menu.len(), true);
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

#[cfg(test)]
mod bundle_tests {
    use super::*;
    use crate::ui::bundle::Sound;

    fn bundle(items: Vec<ItemStack>) -> ItemStack {
        ItemStack::new(ItemKind::Bundle, 1).with_component(BundleContents { items })
    }

    /// A right-click on a player-inventory bundle, with the hovered selection.
    fn right_click(
        slot_bundle: ItemStack,
        selection: Option<(u16, i32)>,
    ) -> (ItemStack, BundleClick) {
        let mut slots = vec![ItemStack::Empty; crate::player::inventory::PLAYER_SLOTS];
        slots[9] = slot_bundle;
        let mut cursor = ItemStack::Empty;
        let mut click = BundleClick {
            selection,
            sound: None,
        };
        apply_click(
            ContainerKind::Player,
            &slots,
            &mut cursor,
            &ClickOperation::Pickup(PickupClick::Right { slot: Some(9) }),
            false,
            &mut click,
        );
        (cursor, click)
    }

    #[test]
    fn removing_the_selected_entry_clears_the_selection() {
        let stacks = bundle(vec![
            ItemStack::new(ItemKind::Stone, 3),
            ItemStack::new(ItemKind::Dirt, 2),
        ]);
        let (cursor, click) = right_click(stacks, Some((9, 1)));
        assert_eq!(cursor.as_present().map(|d| d.kind), Some(ItemKind::Dirt));
        assert_eq!(click.selection, None);
        assert_eq!(click.sound, Some(Sound::RemoveOne));
    }

    #[test]
    fn a_selection_on_another_slot_survives() {
        let (cursor, click) = right_click(
            bundle(vec![ItemStack::new(ItemKind::Stone, 3)]),
            Some((10, 0)),
        );
        assert_eq!(cursor.as_present().map(|d| d.kind), Some(ItemKind::Stone));
        assert_eq!(click.selection, Some((10, 0)));
    }
}

#[cfg(test)]
mod quick_move_tests {
    use super::*;

    #[test]
    fn hopper_and_dispenser_shift_clicks_cross_at_the_player_inventory() {
        for kind in [ContainerKind::Hopper, ContainerKind::Dispenser] {
            let last = kind.slot_count() - 1;
            let mut slots = vec![ItemStack::Empty; kind.slot_count()];
            slots[0] = ItemStack::new(ItemKind::Stone, 5);
            let mut menu = kind.build_menu(&slots);

            // Contents fill the player inventory from the end of the hotbar.
            quick_move(kind, &mut menu, 0);
            assert!(menu.slot(0).is_some_and(ItemStack::is_empty));
            assert_eq!(menu.slot(last).map(ItemStack::count), Some(5));

            // The player inventory fills the contents from the first slot.
            quick_move(kind, &mut menu, last);
            assert_eq!(menu.slot(0).map(ItemStack::count), Some(5));
        }
    }

    /// A predicted left click on `slot`.
    fn left_click(
        kind: ContainerKind,
        slots: &[ItemStack],
        cursor: &mut ItemStack,
        slot: u16,
    ) -> Vec<(u16, ItemStack)> {
        let op = ClickOperation::Pickup(PickupClick::Left { slot: Some(slot) });
        apply_click(kind, slots, cursor, &op, false, &mut BundleClick::default())
    }

    #[test]
    fn crafter_result_sits_after_the_hotbar_and_ignores_clicks() {
        let kind = ContainerKind::Crafter { disabled: 0 };
        assert_eq!(kind.inv_end(), 45);
        assert_eq!(kind.hotbar_menu_slot(0), 36);
        assert_eq!(kind.hotbar_menu_slot(8), 44);

        let mut slots = vec![ItemStack::Empty; kind.slot_count()];
        slots[0] = ItemStack::new(ItemKind::Stone, 1);
        slots[45] = ItemStack::new(ItemKind::Stone, 1);
        let mut cursor = ItemStack::Empty;

        // The result is never predicted, and a grid click leaves it alone.
        let changed = left_click(kind, &slots, &mut cursor, 45);
        assert!(changed.is_empty());
        assert!(cursor.is_empty());
        let changed = left_click(kind, &slots, &mut cursor, 0);
        assert_eq!(changed, vec![(0, ItemStack::Empty)]);
        assert_eq!(cursor.count(), 1);
    }

    #[test]
    fn crafter_disabled_slots_refuse_items() {
        let kind = ContainerKind::Crafter { disabled: 0b1 };
        let mut slots = vec![ItemStack::Empty; kind.slot_count()];
        slots[36] = ItemStack::new(ItemKind::Stone, 5);
        let mut menu = kind.build_menu(&slots);

        // A shift-click from the hotbar skips the disabled first slot.
        quick_move(kind, &mut menu, 36);
        assert!(menu.slot(0).is_some_and(ItemStack::is_empty));
        assert_eq!(menu.slot(1).map(ItemStack::count), Some(5));

        let mut cursor = ItemStack::new(ItemKind::Stone, 5);
        let empty = vec![ItemStack::Empty; kind.slot_count()];
        let changed = left_click(kind, &empty, &mut cursor, 0);
        assert!(changed.is_empty());
        assert_eq!(cursor.count(), 5);
    }

    #[test]
    fn brewing_bottle_slots_take_one_potion_input() {
        let kind = ContainerKind::BrewingStand;
        let slots = vec![ItemStack::Empty; kind.slot_count()];

        let mut cursor = ItemStack::new(ItemKind::GlassBottle, 3);
        let changed = left_click(kind, &slots, &mut cursor, 0);
        assert_eq!(changed, vec![(0, ItemStack::new(ItemKind::GlassBottle, 1))]);
        assert_eq!(cursor.count(), 2);

        let mut cursor = ItemStack::new(ItemKind::Stone, 3);
        let changed = left_click(kind, &slots, &mut cursor, 0);
        assert!(changed.is_empty());

        // A new item in the ingredient or fuel slot waits for the server, which
        // knows what they take; topping up what's there is predicted.
        let changed = left_click(kind, &slots, &mut cursor, 4);
        assert!(changed.is_empty());
        assert_eq!(cursor.count(), 3);
        let mut topped = slots.clone();
        topped[4] = ItemStack::new(ItemKind::BlazePowder, 1);
        let mut cursor = ItemStack::new(ItemKind::BlazePowder, 2);
        let changed = left_click(kind, &topped, &mut cursor, 4);
        assert_eq!(changed, vec![(4, ItemStack::new(ItemKind::BlazePowder, 3))]);
        assert!(cursor.is_empty());

        // The menu's own slots shift-click into the player inventory, hotbar
        // end first.
        let mut slots = slots;
        slots[3] = ItemStack::new(ItemKind::NetherWart, 4);
        let mut menu = kind.build_menu(&slots);
        quick_move(kind, &mut menu, 3);
        assert_eq!(menu.slot(40).map(ItemStack::count), Some(4));
    }
}
