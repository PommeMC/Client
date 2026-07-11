use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use azalea_inventory::components::{
    Damage, Enchantments, EquipmentSlot, Equippable, MaxDamage, Rarity,
};
use azalea_inventory::default_components::get_default_component;
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::builtin::{DataComponentKind, ItemKind};

use super::common::{
    self, FONT_SIZE, SLOT_LABEL_COLOR, SLOT_SIZE, SLOT_STRIDE, WHITE, hit_test, push_slot, rgb,
};
use super::creative_tab_data::{
    BUILDING_BLOCKS_ITEMS, COLORED_BLOCKS_ITEMS, COMBAT_ITEMS, FOOD_AND_DRINKS_ITEMS,
    FUNCTIONAL_BLOCKS_ITEMS, INGREDIENTS_ITEMS, NATURAL_BLOCKS_ITEMS, OP_BLOCKS_ITEMS,
    REDSTONE_BLOCKS_ITEMS, SPAWN_EGGS_ITEMS, TOOLS_AND_UTILITIES_ITEMS,
};
use crate::lang::item_display_name;
use crate::player::inventory::{Inventory, item_resource_name};
use crate::renderer::pipelines::menu_overlay::{
    CREATIVE_TAB_SPRITES, MenuElement, SpriteId, TooltipLine,
};

const TEX_W: f32 = 195.0;
const TEX_H: f32 = 136.0;
const GRID_COLS: usize = 9;
const GRID_ROWS: usize = 5;
const GRID_ORIGIN_X: f32 = 9.0;
const GRID_ORIGIN_Y: f32 = 18.0;
const SCROLLBAR_X: f32 = 175.0;
const SCROLLBAR_TRACK_Y: f32 = 18.0;
const SCROLLBAR_TRACK_H: f32 = 112.0;
const SCROLLBAR_HANDLE_W: f32 = 12.0;
const SCROLLBAR_HANDLE_H: f32 = 15.0;
const SCROLLBAR_HANDLE_PAD: f32 = 2.0;
const SCROLLBAR_HIT_W: f32 = 14.0;
const SEARCH_BOX_X: f32 = 82.0;
const SEARCH_BOX_Y: f32 = 6.0;
const SEARCH_BOX_H: f32 = 9.0;
const TAB_W: f32 = 26.0;
const TAB_H: f32 = 32.0;
const TAB_STRIDE: f32 = 27.0;
const TAB_TOP_HIT_Y: f32 = -32.0;
const TAB_BOTTOM_HIT_Y: f32 = 136.0;
const TAB_TOP_RENDER_Y: f32 = -28.0;
const TAB_BOTTOM_RENDER_Y: f32 = 132.0;
const TAB_ICON_SIZE: f32 = 16.0;
const TITLE_X: f32 = 8.0;
const TITLE_Y: f32 = 6.0;

const HOTBAR_Y: f32 = 112.0;
const INV_MAIN_Y: f32 = 54.0;
const INV_ARMOR_X: f32 = 54.0;
const INV_ARMOR_Y: f32 = 6.0;
const INV_ARMOR_COL_STRIDE: f32 = 54.0;
const INV_ARMOR_ROW_STRIDE: f32 = 27.0;
const INV_OFFHAND_X: f32 = 35.0;
const INV_OFFHAND_Y: f32 = 20.0;
const INV_TRASH_X: f32 = 173.0;
const INV_TRASH_Y: f32 = 112.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CreativeTab {
    BuildingBlocks,
    ColoredBlocks,
    NaturalBlocks,
    FunctionalBlocks,
    RedstoneBlocks,
    #[allow(dead_code)]
    Hotbar,
    Search,
    ToolsAndUtilities,
    Combat,
    FoodAndDrinks,
    Ingredients,
    SpawnEggs,
    #[allow(dead_code)]
    OpBlocks,
    SurvivalInventory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Top,
    Bottom,
}

enum ItemSource {
    Static(&'static [ItemKind]),
    Search,
    Empty,
}

struct TabMeta {
    row: Row,
    col: u8,
    icon: &'static str,
    title: &'static str,
    items: ItemSource,
}

impl CreativeTab {
    fn meta(self) -> TabMeta {
        match self {
            CreativeTab::BuildingBlocks => TabMeta {
                row: Row::Top,
                col: 1,
                icon: "bricks",
                title: "Building Blocks",
                items: ItemSource::Static(BUILDING_BLOCKS_ITEMS),
            },
            CreativeTab::ColoredBlocks => TabMeta {
                row: Row::Top,
                col: 2,
                icon: "cyan_wool",
                title: "Colored Blocks",
                items: ItemSource::Static(COLORED_BLOCKS_ITEMS),
            },
            CreativeTab::NaturalBlocks => TabMeta {
                row: Row::Top,
                col: 3,
                icon: "grass_block",
                title: "Natural Blocks",
                items: ItemSource::Static(NATURAL_BLOCKS_ITEMS),
            },
            CreativeTab::FunctionalBlocks => TabMeta {
                row: Row::Top,
                col: 4,
                icon: "oak_sign",
                title: "Functional Blocks",
                items: ItemSource::Static(FUNCTIONAL_BLOCKS_ITEMS),
            },
            CreativeTab::RedstoneBlocks => TabMeta {
                row: Row::Top,
                col: 5,
                icon: "redstone",
                title: "Redstone Blocks",
                items: ItemSource::Static(REDSTONE_BLOCKS_ITEMS),
            },
            CreativeTab::Hotbar => TabMeta {
                row: Row::Top,
                col: 6,
                icon: "bookshelf",
                title: "Saved Hotbars",
                items: ItemSource::Empty,
            },
            CreativeTab::Search => TabMeta {
                row: Row::Top,
                col: 7,
                icon: "compass",
                title: "Search",
                items: ItemSource::Search,
            },
            CreativeTab::ToolsAndUtilities => TabMeta {
                row: Row::Bottom,
                col: 1,
                icon: "diamond_pickaxe",
                title: "Tools & Utilities",
                items: ItemSource::Static(TOOLS_AND_UTILITIES_ITEMS),
            },
            CreativeTab::Combat => TabMeta {
                row: Row::Bottom,
                col: 2,
                icon: "netherite_sword",
                title: "Combat",
                items: ItemSource::Static(COMBAT_ITEMS),
            },
            CreativeTab::FoodAndDrinks => TabMeta {
                row: Row::Bottom,
                col: 3,
                icon: "golden_apple",
                title: "Food & Drinks",
                items: ItemSource::Static(FOOD_AND_DRINKS_ITEMS),
            },
            CreativeTab::Ingredients => TabMeta {
                row: Row::Bottom,
                col: 4,
                icon: "iron_ingot",
                title: "Ingredients",
                items: ItemSource::Static(INGREDIENTS_ITEMS),
            },
            CreativeTab::SpawnEggs => TabMeta {
                row: Row::Bottom,
                col: 5,
                icon: "creeper_spawn_egg",
                title: "Spawn Eggs",
                items: ItemSource::Static(SPAWN_EGGS_ITEMS),
            },
            CreativeTab::OpBlocks => TabMeta {
                row: Row::Bottom,
                col: 6,
                icon: "command_block",
                title: "Operator Utilities",
                items: ItemSource::Static(OP_BLOCKS_ITEMS),
            },
            CreativeTab::SurvivalInventory => TabMeta {
                row: Row::Bottom,
                col: 7,
                icon: "chest",
                title: "Survival Inventory",
                items: ItemSource::Empty,
            },
        }
    }

    fn scrollable(self) -> bool {
        matches!(
            self.meta().items,
            ItemSource::Static(_) | ItemSource::Search
        )
    }

    fn is_inventory_tab(self) -> bool {
        matches!(self, CreativeTab::SurvivalInventory)
    }

    fn shows_title(self) -> bool {
        !self.is_inventory_tab()
    }

    fn background_sprite(self) -> SpriteId {
        match self {
            CreativeTab::Search => SpriteId::CreativeSearchBackground,
            CreativeTab::SurvivalInventory => SpriteId::CreativeInventoryBackground,
            _ => SpriteId::CreativeItemsBackground,
        }
    }

    pub fn captures_typing(self) -> bool {
        matches!(self, CreativeTab::Search)
    }
}

// `Hotbar` is kept as a variant for when saved hotbars ship.
const TABS: [CreativeTab; 12] = [
    CreativeTab::BuildingBlocks,
    CreativeTab::ColoredBlocks,
    CreativeTab::NaturalBlocks,
    CreativeTab::FunctionalBlocks,
    CreativeTab::RedstoneBlocks,
    CreativeTab::Search,
    CreativeTab::ToolsAndUtilities,
    CreativeTab::Combat,
    CreativeTab::FoodAndDrinks,
    CreativeTab::Ingredients,
    CreativeTab::SpawnEggs,
    CreativeTab::SurvivalInventory,
];

pub struct CreativeState {
    pub tab: CreativeTab,
    pub scroll: f32,
    pub search: String,
    /// Client-side carried stack (the item riding the cursor). Creative-only.
    pub cursor_item: ItemStack,
    /// Active click-drag distribution, if a button is held across slots.
    drag: Option<DragState>,
    /// Last left-click (slot, time) for double-click detection.
    last_left_click: Option<(u16, Instant)>,
    cursor_blink: Instant,
    scroll_dragging: bool,
}

struct DragState {
    button: ClickKind,
    slots: Vec<u16>,
}

impl CreativeState {
    pub fn new() -> Self {
        Self {
            tab: CreativeTab::BuildingBlocks,
            scroll: 0.0,
            search: String::new(),
            cursor_item: ItemStack::Empty,
            drag: None,
            last_left_click: None,
            cursor_blink: Instant::now(),
            scroll_dragging: false,
        }
    }

    fn reset_blink(&mut self) {
        self.cursor_blink = Instant::now();
    }

    /// Discards the carried stack and any pending drag/double-click state.
    /// Must run whenever the screen closes, or a stale drag would re-commit on
    /// reopen.
    pub fn reset_interaction(&mut self) {
        self.cursor_item = ItemStack::Empty;
        self.drag = None;
        self.last_left_click = None;
    }
}

impl Default for CreativeState {
    fn default() -> Self {
        Self::new()
    }
}

pub enum CreativeAction {
    None,
    Close,
    /// Set a real player-inventory slot to the given stack (creative set-slot).
    SetSlot(u16, ItemStack),
    /// Set several slots at once (drag distribution).
    SetSlots(Vec<(u16, ItemStack)>),
}

#[allow(clippy::too_many_arguments)]
pub fn build_creative_inventory(
    elements: &mut Vec<MenuElement>,
    state: &mut CreativeState,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    middle_clicked: bool,
    right_clicked: bool,
    scroll_delta: f32,
    typed_chars: &[char],
    backspace: bool,
    inventory: &Inventory,
    gs: f32,
    advanced_tooltips: bool,
    left_held: bool,
    right_held: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> CreativeAction {
    if state.tab.captures_typing() {
        if backspace {
            state.search.pop();
            state.reset_blink();
        }
        for &ch in typed_chars {
            if state.search.len() < 50 && !ch.is_control() {
                state.search.push(ch);
                state.reset_blink();
            }
        }
    }

    let scale = gs.min(screen_w / TEX_W).min(screen_h / TEX_H);
    let inv_w = TEX_W * scale;
    let inv_h = TEX_H * scale;
    let ox = (screen_w - inv_w) / 2.0;
    let oy = (screen_h - inv_h) / 2.0;

    common::push_overlay(elements, screen_w, screen_h, 0.5);

    draw_tabs(elements, state, ox, oy, scale, false);

    elements.push(MenuElement::Image {
        x: ox,
        y: oy,
        w: inv_w,
        h: inv_h,
        sprite: state.tab.background_sprite(),
        tint: WHITE,
    });

    let mut action = CreativeAction::None;

    let tab_hit = tab_hit_test(ox, oy, scale, cursor, clicked);
    if let Some(new_tab) = tab_hit
        && new_tab != state.tab
    {
        state.tab = new_tab;
        state.scroll = 0.0;
        state.reset_blink();
    }

    draw_tabs(elements, state, ox, oy, scale, true);

    if state.tab.shows_title() {
        elements.push(MenuElement::TextFlat {
            x: ox + TITLE_X * scale,
            y: oy + TITLE_Y * scale,
            text: state.tab.meta().title.into(),
            scale: FONT_SIZE * scale,
            color: SLOT_LABEL_COLOR,
        });
    }

    let size = SLOT_SIZE * scale;
    let tt = TooltipCtx {
        cursor,
        screen_w,
        screen_h,
        gs,
        advanced: advanced_tooltips,
        clicked,
        right_clicked,
        carrying: state.cursor_item.is_present(),
    };

    let drag_preview: Option<DragPreview> = match (&state.drag, &state.cursor_item) {
        (Some(drag), ItemStack::Present(carried)) => {
            Some(compute_drag_preview(carried, drag, inventory))
        }
        _ => None,
    };
    let empty_preview = HashMap::new();
    let preview_map = drag_preview
        .as_ref()
        .map(|p| &p.slots)
        .unwrap_or(&empty_preview);

    let real_hit: Option<SlotHit> = if state.tab.is_inventory_tab() {
        draw_inventory_layout(elements, ox, oy, scale, inventory, &tt, preview_map)
    } else {
        let items = visible_items(state);
        let scrollable = state.tab.scrollable();
        let max_scroll_rows = if scrollable {
            items.len().div_ceil(GRID_COLS).saturating_sub(GRID_ROWS)
        } else {
            0
        };

        let mut grid_clicked = clicked;
        if scrollable && max_scroll_rows > 0 {
            let inside = hit_test(cursor, [ox, oy, inv_w, inv_h]);
            if inside && scroll_delta != 0.0 {
                let step = 1.0 / max_scroll_rows as f32;
                state.scroll = (state.scroll - scroll_delta.signum() * step).clamp(0.0, 1.0);
            }
            if update_scroll_drag(state, ox, oy, scale, cursor, clicked, left_held) {
                grid_clicked = false;
            }
        } else {
            state.scroll = 0.0;
            state.scroll_dragging = false;
        }

        let scroll_row_offset = (state.scroll * max_scroll_rows as f32).round() as usize;
        let item_offset = scroll_row_offset * GRID_COLS;

        if matches!(state.tab, CreativeTab::Search) {
            draw_search_box(
                elements,
                &state.search,
                &state.cursor_blink,
                ox,
                oy,
                scale,
                text_width_fn,
            );
        }

        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let global_idx = item_offset + row * GRID_COLS + col;
                let item = items.get(global_idx).cloned().unwrap_or(ItemStack::Empty);
                let (slot_x, slot_y) = slot_xy(
                    ox,
                    oy,
                    scale,
                    GRID_ORIGIN_X + col as f32 * SLOT_STRIDE,
                    GRID_ORIGIN_Y + row as f32 * SLOT_STRIDE,
                );
                let hovered = push_slot(elements, slot_x, slot_y, size, scale, cursor, &item, None);
                if hovered {
                    push_item_tooltip(elements, &item, &tt);
                    if middle_clicked
                        && state.cursor_item.is_empty()
                        && let ItemStack::Present(data) = &item
                    {
                        state.cursor_item = stack_with_count(data, data.kind.max_stack_size());
                    } else if grid_clicked || right_clicked {
                        match std::mem::replace(&mut state.cursor_item, ItemStack::Empty) {
                            ItemStack::Present(mut carried) => {
                                if right_clicked {
                                    carried.count -= 1;
                                    if carried.count > 0 {
                                        state.cursor_item = ItemStack::Present(carried);
                                    }
                                } else if let ItemStack::Present(clicked) = &item
                                    && carried.is_same_item_and_components(clicked)
                                {
                                    // Left-click the same item: grow the carried stack up to its
                                    // max. A different item or empty cell discards it (stays
                                    // empty).
                                    if carried.count < carried.kind.max_stack_size() {
                                        carried.count += 1;
                                    }
                                    state.cursor_item = ItemStack::Present(carried);
                                }
                            }
                            ItemStack::Empty => {
                                if let ItemStack::Present(data) = &item {
                                    state.cursor_item = ItemStack::Present(data.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        let hit = draw_player_hotbar(elements, ox, oy, scale, inventory, &tt, preview_map);

        if scrollable {
            draw_scrollbar(elements, ox, oy, scale, state.scroll, max_scroll_rows == 0);
        }
        hit
    };

    // Real-slot clicks and click-drag distribution.
    if let Some(drag) = &mut state.drag {
        let held = match drag.button {
            ClickKind::Left => left_held,
            ClickKind::Right => right_held,
        };
        if held {
            // Extend the drag onto a newly hovered, eligible slot.
            if let Some(hit) = &real_hit
                && hit.slot != SLOT_TRASH
                && let ItemStack::Present(carried) = &state.cursor_item
                && !drag.slots.contains(&hit.slot)
                && carried.count as usize > drag.slots.len()
                && drag_slot_eligible(carried, inventory, hit.slot)
            {
                drag.slots.push(hit.slot);
            }
        } else {
            // Released: commit the split and keep the remainder on the cursor.
            if let Some(preview) = &drag_preview {
                if let ItemStack::Present(carried) = state.cursor_item.clone() {
                    state.cursor_item = stack_with_count(&carried, preview.remainder);
                }
                let items: Vec<(u16, ItemStack)> =
                    preview.slots.iter().map(|(k, v)| (*k, v.clone())).collect();
                if !items.is_empty() {
                    action = CreativeAction::SetSlots(items);
                }
            }
            state.drag = None;
        }
    } else if let Some(hit) = &real_hit
        && let Some(kind) = hit.click
    {
        let double = matches!(kind, ClickKind::Left) && is_double_click(state, hit.slot);
        if hit.slot == SLOT_TRASH {
            state.cursor_item = ItemStack::Empty;
        } else if double && state.cursor_item.is_present() {
            let items = double_click_gather(state, inventory);
            if !items.is_empty() {
                action = CreativeAction::SetSlots(items);
            }
            state.last_left_click = None;
        } else {
            if matches!(kind, ClickKind::Left) {
                state.last_left_click = Some((hit.slot, Instant::now()));
            }
            if let ItemStack::Present(carried) = &state.cursor_item {
                // Carrying: start a drag on an eligible slot, else swap immediately.
                if drag_slot_eligible(carried, inventory, hit.slot) {
                    state.drag = Some(DragState {
                        button: kind,
                        slots: vec![hit.slot],
                    });
                } else {
                    action = apply_slot_action(state, inventory, hit.slot, kind);
                }
            } else {
                // Empty cursor: pick up / take half immediately.
                action = apply_slot_action(state, inventory, hit.slot, kind);
            }
        }
    }

    push_tab_tooltip(elements, ox, oy, scale, &tt);

    let outside = !hit_test(cursor, [ox, oy, inv_w, inv_h]);
    if clicked && outside && tab_hit.is_none() && matches!(action, CreativeAction::None) {
        if state.cursor_item.is_present() {
            // TODO: drop the carried item into the world; for now just discard it.
            state.cursor_item = ItemStack::Empty;
        } else {
            action = CreativeAction::Close;
        }
    }

    // Draw the carried item on the cursor; while dragging, show the remainder.
    let cursor_stack = match (state.drag.is_some(), &state.cursor_item, &drag_preview) {
        (true, ItemStack::Present(carried), Some(preview)) => {
            stack_with_count(carried, preview.remainder)
        }
        _ => state.cursor_item.clone(),
    };
    if let ItemStack::Present(data) = &cursor_stack {
        common::push_item_icon(
            elements,
            cursor.0 - size / 2.0,
            cursor.1 - size / 2.0,
            size,
            scale,
            data,
        );
    }

    action
}

fn tab_sprite(row: Row, col: u8, selected: bool) -> SpriteId {
    let r = if matches!(row, Row::Top) { 0 } else { 1 };
    let s = if selected { 1 } else { 0 };
    let c = (col.clamp(1, 7) - 1) as usize;
    CREATIVE_TAB_SPRITES[r][s][c]
}

fn tab_x(col: u8, scale: f32, ox: f32) -> f32 {
    let local = if col >= 6 {
        TEX_W - TAB_STRIDE * (8.0 - col as f32) + 1.0
    } else {
        (col as f32 - 1.0) * TAB_STRIDE
    };
    ox + local * scale
}

fn draw_tabs(
    elements: &mut Vec<MenuElement>,
    state: &CreativeState,
    ox: f32,
    oy: f32,
    scale: f32,
    selected_pass: bool,
) {
    let tab_w = TAB_W * scale;
    let tab_h = TAB_H * scale;
    let icon_size = TAB_ICON_SIZE * scale;
    for &tab in TABS.iter() {
        let selected = state.tab == tab;
        if selected != selected_pass {
            continue;
        }
        let meta = tab.meta();
        let x = tab_x(meta.col, scale, ox);
        let (render_y_off, icon_y_off) = match meta.row {
            Row::Top => (TAB_TOP_RENDER_Y, 9.0),
            Row::Bottom => (TAB_BOTTOM_RENDER_Y, 7.0),
        };
        let render_y = oy + render_y_off * scale;
        elements.push(MenuElement::Image {
            x,
            y: render_y,
            w: tab_w,
            h: tab_h,
            sprite: tab_sprite(meta.row, meta.col, selected),
            tint: WHITE,
        });
        elements.push(MenuElement::ItemIcon {
            x: x + (tab_w - icon_size) / 2.0,
            y: render_y + icon_y_off * scale,
            w: icon_size,
            h: icon_size,
            item_name: meta.icon.into(),
            tint: WHITE,
        });
    }
}

fn tab_hit_test(
    ox: f32,
    oy: f32,
    scale: f32,
    cursor: (f32, f32),
    clicked: bool,
) -> Option<CreativeTab> {
    if !clicked {
        return None;
    }
    let tab_w = TAB_W * scale;
    let tab_h = TAB_H * scale;
    for &tab in TABS.iter() {
        let meta = tab.meta();
        let x = tab_x(meta.col, scale, ox);
        let hit_y_off = match meta.row {
            Row::Top => TAB_TOP_HIT_Y,
            Row::Bottom => TAB_BOTTOM_HIT_Y,
        };
        let hit_y = oy + hit_y_off * scale;
        if hit_test(cursor, [x, hit_y, tab_w, tab_h]) {
            return Some(tab);
        }
    }
    None
}

fn slot_xy(ox: f32, oy: f32, scale: f32, sx: f32, sy: f32) -> (f32, f32) {
    (ox + sx * scale, oy + sy * scale)
}

fn item_or_empty(slots: &[ItemStack], idx: usize) -> ItemStack {
    slots.get(idx).cloned().unwrap_or(ItemStack::Empty)
}

struct TooltipCtx {
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    advanced: bool,
    clicked: bool,
    right_clicked: bool,
    carrying: bool,
}

#[derive(Clone, Copy)]
enum ClickKind {
    Left,
    Right,
}

/// A hovered real slot, plus which button (if any) was just pressed on it.
struct SlotHit {
    slot: u16,
    click: Option<ClickKind>,
}

/// Pending click-drag distribution: the stack each covered slot would receive
/// and the count left on the cursor.
struct DragPreview {
    slots: HashMap<u16, ItemStack>,
    remainder: i32,
}

const TOOLTIP_NAME_COLOR: [f32; 4] = rgb(0xFFFFFF);
const TOOLTIP_TAB_COLOR: [f32; 4] = rgb(0x5555FF);
const TOOLTIP_ADVANCED_COLOR: [f32; 4] = rgb(0x555555);
const TOOLTIP_LORE_COLOR: [f32; 4] = rgb(0xAAAAAA);
const RARITY_UNCOMMON: [f32; 4] = rgb(0xFFFF55);
const RARITY_RARE: [f32; 4] = rgb(0x55FFFF);
const RARITY_EPIC: [f32; 4] = rgb(0xFF55FF);

fn rarity_color(kind: ItemKind) -> [f32; 4] {
    match get_default_component::<Rarity>(kind) {
        Some(Rarity::Uncommon) => RARITY_UNCOMMON,
        Some(Rarity::Rare) => RARITY_RARE,
        Some(Rarity::Epic) => RARITY_EPIC,
        _ => TOOLTIP_NAME_COLOR,
    }
}

// Fixed list rather than `from_u32(0..)` so registry-ID shifts between MC
// versions don't silently change counts.
const TRACKED_COMPONENT_KINDS: &[DataComponentKind] = &[
    DataComponentKind::MaxStackSize,
    DataComponentKind::MaxDamage,
    DataComponentKind::Damage,
    DataComponentKind::ItemName,
    DataComponentKind::ItemModel,
    DataComponentKind::Lore,
    DataComponentKind::Rarity,
    DataComponentKind::Enchantments,
    DataComponentKind::AttributeModifiers,
    DataComponentKind::RepairCost,
    DataComponentKind::EnchantmentGlintOverride,
    DataComponentKind::Food,
    DataComponentKind::Consumable,
    DataComponentKind::UseRemainder,
    DataComponentKind::UseCooldown,
    DataComponentKind::Tool,
    DataComponentKind::Weapon,
    DataComponentKind::AttackRange,
    DataComponentKind::Enchantable,
    DataComponentKind::Equippable,
    DataComponentKind::Repairable,
    DataComponentKind::Glider,
    DataComponentKind::BlocksAttacks,
    DataComponentKind::DamageResistant,
];

fn total_component_count(data: &ItemStackData) -> usize {
    TRACKED_COMPONENT_KINDS
        .iter()
        .filter(|&&kind| {
            data.component_patch.has_kind(kind) || default_has_component(data.kind, kind)
        })
        .count()
}

fn default_has_component(item: ItemKind, kind: DataComponentKind) -> bool {
    macro_rules! check {
        ($($ck:ident => $t:ty),* $(,)?) => {
            match kind {
                $( DataComponentKind::$ck => get_default_component::<$t>(item).is_some(), )*
                _ => false,
            }
        };
    }
    check! {
        MaxStackSize => azalea_inventory::components::MaxStackSize,
        MaxDamage => MaxDamage,
        Damage => Damage,
        ItemName => azalea_inventory::components::ItemName,
        ItemModel => azalea_inventory::components::ItemModel,
        Lore => azalea_inventory::components::Lore,
        Rarity => Rarity,
        Enchantments => Enchantments,
        AttributeModifiers => azalea_inventory::components::AttributeModifiers,
        RepairCost => azalea_inventory::components::RepairCost,
        EnchantmentGlintOverride => azalea_inventory::components::EnchantmentGlintOverride,
        Food => azalea_inventory::components::Food,
        Consumable => azalea_inventory::components::Consumable,
        UseRemainder => azalea_inventory::components::UseRemainder,
        UseCooldown => azalea_inventory::components::UseCooldown,
        Tool => azalea_inventory::components::Tool,
        Weapon => azalea_inventory::components::Weapon,
        AttackRange => azalea_inventory::components::AttackRange,
        Enchantable => azalea_inventory::components::Enchantable,
        Equippable => azalea_inventory::components::Equippable,
        Repairable => azalea_inventory::components::Repairable,
        Glider => azalea_inventory::components::Glider,
        BlocksAttacks => azalea_inventory::components::BlocksAttacks,
        DamageResistant => azalea_inventory::components::DamageResistant,
    }
}

fn lore_lines(data: &ItemStackData) -> Vec<TooltipLine> {
    let mut lines = Vec::new();
    if let Some(damage) = data.component_patch.get::<Damage>() {
        let max = data
            .component_patch
            .get::<MaxDamage>()
            .map(|m| m.amount)
            .or_else(|| get_default_component::<MaxDamage>(data.kind).map(|m| m.amount))
            .unwrap_or(0);
        if max > 0 {
            lines.push(TooltipLine::new(
                format!("Durability: {} / {}", max - damage.amount, max),
                TOOLTIP_LORE_COLOR,
            ));
        }
    }
    if let Some(ench) = data.component_patch.get::<Enchantments>() {
        for (enchantment, level) in &ench.levels {
            lines.push(TooltipLine::new(
                format!("{:?} {}", enchantment, roman(*level)),
                TOOLTIP_LORE_COLOR,
            ));
        }
    }
    lines
}

fn roman(n: i32) -> &'static str {
    match n {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        8 => "VIII",
        9 => "IX",
        10 => "X",
        _ => "",
    }
}

fn tabs_containing(kind: ItemKind) -> &'static [&'static str] {
    static INDEX: OnceLock<HashMap<ItemKind, Vec<&'static str>>> = OnceLock::new();
    static EMPTY: &[&str] = &[];
    INDEX
        .get_or_init(|| {
            let mut map: HashMap<ItemKind, Vec<&'static str>> = HashMap::new();
            for tab in TABS.iter() {
                let meta = tab.meta();
                if let ItemSource::Static(list) = meta.items {
                    for &k in list {
                        map.entry(k).or_default().push(meta.title);
                    }
                }
            }
            map
        })
        .get(&kind)
        .map(Vec::as_slice)
        .unwrap_or(EMPTY)
}

fn build_item_tooltip_lines(data: &ItemStackData, advanced: bool) -> Vec<TooltipLine> {
    let kind = data.kind;
    let mut lines = Vec::new();
    lines.push(TooltipLine::new(
        item_display_name(kind),
        rarity_color(kind),
    ));
    lines.extend(lore_lines(data));
    for &title in tabs_containing(kind) {
        lines.push(TooltipLine::new(title.to_string(), TOOLTIP_TAB_COLOR));
    }
    if advanced {
        lines.push(TooltipLine::new(
            format!("minecraft:{}", item_resource_name(kind)),
            TOOLTIP_ADVANCED_COLOR,
        ));
        lines.push(TooltipLine::new(
            format!("{} component(s)", total_component_count(data)),
            TOOLTIP_ADVANCED_COLOR,
        ));
    }
    lines
}

fn push_item_tooltip(elements: &mut Vec<MenuElement>, item: &ItemStack, tt: &TooltipCtx) {
    if tt.carrying {
        return;
    }
    if let ItemStack::Present(data) = item {
        elements.push(MenuElement::TooltipLines {
            x: tt.cursor.0,
            y: tt.cursor.1,
            lines: build_item_tooltip_lines(data, tt.advanced),
            scale: FONT_SIZE * tt.gs,
            screen_w: tt.screen_w,
            screen_h: tt.screen_h,
        });
    }
}

fn push_tab_tooltip(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    tt: &TooltipCtx,
) {
    if tt.carrying {
        return;
    }
    let inset_w = 21.0 * scale;
    let inset_h = 27.0 * scale;
    for &tab in TABS.iter() {
        let meta = tab.meta();
        let x = tab_x(meta.col, scale, ox);
        let hit_y_off = match meta.row {
            Row::Top => TAB_TOP_HIT_Y,
            Row::Bottom => TAB_BOTTOM_HIT_Y,
        };
        let inset_x = x + 3.0 * scale;
        let inset_y = oy + hit_y_off * scale + 3.0 * scale;
        if hit_test(tt.cursor, [inset_x, inset_y, inset_w, inset_h]) {
            common::push_tooltip(
                elements,
                tt.cursor,
                tt.screen_w,
                tt.screen_h,
                tt.gs,
                meta.title,
            );
            return;
        }
    }
}

/// Reports the hovered slot (and which button, if any, was just pressed) so the
/// caller can drive clicks and drag accumulation. Returns for empty slots too.
#[allow(clippy::too_many_arguments)]
fn slot_with_tooltip(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
    tt: &TooltipCtx,
    slot_num: Option<u16>,
) -> Option<SlotHit> {
    let hovered = push_slot(elements, x, y, size, scale, tt.cursor, item, empty_sprite);
    if hovered {
        push_item_tooltip(elements, item, tt);
        if let Some(slot) = slot_num {
            let click = if tt.clicked {
                Some(ClickKind::Left)
            } else if tt.right_clicked {
                Some(ClickKind::Right)
            } else {
                None
            };
            return Some(SlotHit { slot, click });
        }
    }
    None
}

/// A single (non-drag) click on a real player-inventory slot. With an empty
/// cursor it picks up the slot (left = whole, right = half); while carrying it
/// swaps — reachable only for a different item, since empty/same-item slots go
/// through the drag path — unless the slot rejects the item (armor).
fn apply_slot_action(
    state: &mut CreativeState,
    inventory: &Inventory,
    slot_num: u16,
    kind: ClickKind,
) -> CreativeAction {
    let slot_item = inventory.slot(slot_num as usize).clone();
    match std::mem::replace(&mut state.cursor_item, ItemStack::Empty) {
        ItemStack::Empty => match (&slot_item, kind) {
            (ItemStack::Empty, _) => CreativeAction::None,
            (ItemStack::Present(_), ClickKind::Left) => {
                state.cursor_item = slot_item;
                CreativeAction::SetSlot(slot_num, ItemStack::Empty)
            }
            (ItemStack::Present(data), ClickKind::Right) => {
                let take = (data.count + 1) / 2;
                state.cursor_item = stack_with_count(data, take);
                CreativeAction::SetSlot(slot_num, stack_with_count(data, data.count - take))
            }
        },
        ItemStack::Present(carried) => {
            if !may_place(slot_num, &carried) {
                state.cursor_item = ItemStack::Present(carried);
                return CreativeAction::None;
            }
            state.cursor_item = slot_item;
            CreativeAction::SetSlot(slot_num, ItemStack::Present(carried))
        }
    }
}

/// A copy of `item` at `count`, or `Empty` when `count` drops to zero or below.
fn stack_with_count(item: &ItemStackData, count: i32) -> ItemStack {
    if count > 0 {
        let mut s = item.clone();
        s.count = count;
        ItemStack::Present(s)
    } else {
        ItemStack::Empty
    }
}

const DOUBLE_CLICK_MS: u128 = 250;

fn is_double_click(state: &CreativeState, slot: u16) -> bool {
    matches!(state.last_left_click, Some((s, t)) if s == slot && t.elapsed().as_millis() <= DOUBLE_CLICK_MS)
}

/// Double-click gather: fill the carried stack to its max by pulling matching
/// items from the real player inventory (partial stacks first, then full),
/// matching vanilla `PICKUP_ALL`. Returns the drained slots.
fn double_click_gather(state: &mut CreativeState, inventory: &Inventory) -> Vec<(u16, ItemStack)> {
    let ItemStack::Present(mut carried) = state.cursor_item.clone() else {
        return Vec::new();
    };
    let max = carried.kind.max_stack_size();
    let mut changed: HashMap<u16, ItemStack> = HashMap::new();
    for pass in 0..2 {
        for slot in SLOT_MAIN_BASE..=SLOT_OFFHAND {
            if carried.count >= max {
                break;
            }
            let existing = match changed.get(&slot) {
                Some(ItemStack::Present(d)) => d.clone(),
                Some(ItemStack::Empty) => continue,
                None => match inventory.slot(slot as usize) {
                    ItemStack::Present(d) => d.clone(),
                    ItemStack::Empty => continue,
                },
            };
            if !carried.is_same_item_and_components(&existing) {
                continue;
            }
            if pass == 0 && existing.count >= existing.kind.max_stack_size() {
                continue;
            }
            let take = (max - carried.count).min(existing.count);
            carried.count += take;
            changed.insert(slot, stack_with_count(&existing, existing.count - take));
        }
    }
    state.cursor_item = ItemStack::Present(carried);
    changed.into_iter().collect()
}

/// Vanilla `ArmorSlot.mayPlace`: armor slots only accept items whose
/// Equippable component targets that slot; anything goes elsewhere.
fn may_place(slot_num: u16, item: &ItemStackData) -> bool {
    // Vanilla InventoryMenu.SLOT_IDS: armor menu slots 5..=8 are head..feet.
    let required = match slot_num {
        5 => EquipmentSlot::Head,
        6 => EquipmentSlot::Chest,
        7 => EquipmentSlot::Legs,
        8 => EquipmentSlot::Feet,
        _ => return true,
    };
    get_default_component::<Equippable>(item.kind).is_some_and(|e| e.slot == required)
}

/// A drag can cover a slot only if the item may go there and the slot is empty
/// or holds the same item as the carried stack.
fn drag_slot_eligible(carried: &ItemStackData, inventory: &Inventory, slot_num: u16) -> bool {
    if !may_place(slot_num, carried) {
        return false;
    }
    match inventory.slot(slot_num as usize) {
        ItemStack::Empty => true,
        ItemStack::Present(existing) => carried.is_same_item_and_components(existing),
    }
}

/// Distribute the carried stack across the dragged slots: left splits it
/// evenly, right places one each. Returns each slot's resulting stack and the
/// remainder.
fn compute_drag_preview(
    carried: &ItemStackData,
    drag: &DragState,
    inventory: &Inventory,
) -> DragPreview {
    let n = drag.slots.len() as i32;
    let place = match drag.button {
        ClickKind::Left => carried.count / n.max(1),
        ClickKind::Right => 1,
    };
    let max = carried.kind.max_stack_size();
    let mut remainder = carried.count;
    let mut slots = HashMap::new();
    for &slot in &drag.slots {
        let existing = match inventory.slot(slot as usize) {
            ItemStack::Present(d) if carried.is_same_item_and_components(d) => d.count,
            _ => 0,
        };
        let new_count = (place + existing).min(max);
        remainder -= new_count - existing;
        slots.insert(slot, stack_with_count(carried, new_count));
    }
    DragPreview { slots, remainder }
}

// Vanilla `PlayerInventory` slot indices.
const SLOT_ARMOR_BASE: u16 = 5;
const SLOT_MAIN_BASE: u16 = 9;
const SLOT_HOTBAR_BASE: u16 = 36;
const SLOT_OFFHAND: u16 = 45;
/// Sentinel for the inventory-tab trash slot (not a real inventory index).
const SLOT_TRASH: u16 = u16::MAX;

fn draw_player_hotbar(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    inventory: &Inventory,
    tt: &TooltipCtx,
    preview: &HashMap<u16, ItemStack>,
) -> Option<SlotHit> {
    let size = SLOT_SIZE * scale;
    let hotbar = inventory.hotbar_slots();
    let mut clicked_slot = None;
    for col in 0..GRID_COLS {
        let (x, y) = slot_xy(
            ox,
            oy,
            scale,
            GRID_ORIGIN_X + col as f32 * SLOT_STRIDE,
            HOTBAR_Y,
        );
        let slot_num = SLOT_HOTBAR_BASE + col as u16;
        let item = slot_display(preview, slot_num, item_or_empty(hotbar, col));
        clicked_slot = clicked_slot.or(slot_with_tooltip(
            elements,
            x,
            y,
            size,
            scale,
            &item,
            None,
            tt,
            Some(slot_num),
        ));
    }
    clicked_slot
}

/// Overrides a slot's drawn stack with its drag preview, when one is pending.
fn slot_display(preview: &HashMap<u16, ItemStack>, slot_num: u16, actual: ItemStack) -> ItemStack {
    preview.get(&slot_num).cloned().unwrap_or(actual)
}

fn draw_inventory_layout(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    inventory: &Inventory,
    tt: &TooltipCtx,
    preview: &HashMap<u16, ItemStack>,
) -> Option<SlotHit> {
    let size = SLOT_SIZE * scale;
    let mut clicked_slot = None;

    let armor = inventory.armor_slots();
    for i in 0..4 {
        let col = (i / 2) as f32;
        let row = (i % 2) as f32;
        let (x, y) = slot_xy(
            ox,
            oy,
            scale,
            INV_ARMOR_X + col * INV_ARMOR_COL_STRIDE,
            INV_ARMOR_Y + row * INV_ARMOR_ROW_STRIDE,
        );
        let slot_num = SLOT_ARMOR_BASE + i as u16;
        let item = slot_display(preview, slot_num, item_or_empty(armor, i));
        clicked_slot = clicked_slot.or(slot_with_tooltip(
            elements,
            x,
            y,
            size,
            scale,
            &item,
            None,
            tt,
            Some(slot_num),
        ));
    }

    let (x, y) = slot_xy(ox, oy, scale, INV_OFFHAND_X, INV_OFFHAND_Y);
    let offhand = slot_display(preview, SLOT_OFFHAND, inventory.offhand().clone());
    clicked_slot = clicked_slot.or(slot_with_tooltip(
        elements,
        x,
        y,
        size,
        scale,
        &offhand,
        None,
        tt,
        Some(SLOT_OFFHAND),
    ));

    let main = inventory.main_slots();
    for row in 0..3usize {
        for col in 0..GRID_COLS {
            let idx = row * GRID_COLS + col;
            let (x, y) = slot_xy(
                ox,
                oy,
                scale,
                GRID_ORIGIN_X + col as f32 * SLOT_STRIDE,
                INV_MAIN_Y + row as f32 * SLOT_STRIDE,
            );
            let slot_num = SLOT_MAIN_BASE + idx as u16;
            let item = slot_display(preview, slot_num, item_or_empty(main, idx));
            clicked_slot = clicked_slot.or(slot_with_tooltip(
                elements,
                x,
                y,
                size,
                scale,
                &item,
                None,
                tt,
                Some(slot_num),
            ));
        }
    }

    clicked_slot = clicked_slot.or(draw_player_hotbar(
        elements, ox, oy, scale, inventory, tt, preview,
    ));

    let (trash_x, trash_y) = slot_xy(ox, oy, scale, INV_TRASH_X, INV_TRASH_Y);
    clicked_slot = clicked_slot.or(slot_with_tooltip(
        elements,
        trash_x,
        trash_y,
        size,
        scale,
        &ItemStack::Empty,
        None,
        tt,
        Some(SLOT_TRASH),
    ));

    clicked_slot
}

fn draw_search_box(
    elements: &mut Vec<MenuElement>,
    text: &str,
    cursor_blink: &Instant,
    ox: f32,
    oy: f32,
    scale: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let x = ox + SEARCH_BOX_X * scale;
    let y = oy + SEARCH_BOX_Y * scale;
    let h = SEARCH_BOX_H * scale;
    let pad = 1.0 * scale;
    let fs = FONT_SIZE * scale;
    let text_y = y + (h - fs) / 2.0;
    elements.push(MenuElement::Text {
        x: x + pad,
        y: text_y,
        text: text.into(),
        scale: fs,
        color: WHITE,
        centered: false,
    });
    if cursor_blink.elapsed().as_millis() % 1000 < 500 {
        let caret_x = x + pad + text_width_fn(text, fs);
        elements.push(MenuElement::Text {
            x: caret_x,
            y: text_y,
            text: "_".into(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
    }
}

/// Returns `true` if the click was consumed by the scrollbar.
fn update_scroll_drag(
    state: &mut CreativeState,
    ox: f32,
    oy: f32,
    scale: f32,
    cursor: (f32, f32),
    clicked: bool,
    mouse_held: bool,
) -> bool {
    let hit_x = ox + SCROLLBAR_X * scale;
    let hit_y = oy + SCROLLBAR_TRACK_Y * scale;
    let hit_w = SCROLLBAR_HIT_W * scale;
    let hit_h = SCROLLBAR_TRACK_H * scale;
    let mut consumed = false;
    if clicked && hit_test(cursor, [hit_x, hit_y, hit_w, hit_h]) {
        state.scroll_dragging = true;
        consumed = true;
    }
    if !mouse_held {
        state.scroll_dragging = false;
    }
    if state.scroll_dragging {
        let track_y = oy + SCROLLBAR_TRACK_Y * scale;
        let half_handle = SCROLLBAR_HANDLE_H * scale / 2.0;
        let usable = (SCROLLBAR_TRACK_H - SCROLLBAR_HANDLE_H - SCROLLBAR_HANDLE_PAD) * scale;
        state.scroll = ((cursor.1 - track_y - half_handle) / usable).clamp(0.0, 1.0);
    }
    consumed
}

fn draw_scrollbar(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    scroll: f32,
    disabled: bool,
) {
    let track_x = ox + SCROLLBAR_X * scale;
    let track_y = oy + SCROLLBAR_TRACK_Y * scale;
    let track_h = SCROLLBAR_TRACK_H * scale;
    let handle_w = SCROLLBAR_HANDLE_W * scale;
    let handle_h = SCROLLBAR_HANDLE_H * scale;
    let handle_y = track_y + scroll * (track_h - handle_h - SCROLLBAR_HANDLE_PAD * scale);
    let sprite = if disabled {
        SpriteId::CreativeScrollerDisabled
    } else {
        SpriteId::CreativeScroller
    };
    elements.push(MenuElement::Image {
        x: track_x,
        y: handle_y,
        w: handle_w,
        h: handle_h,
        sprite,
        tint: WHITE,
    });
}

fn visible_items(state: &CreativeState) -> Vec<ItemStack> {
    match state.tab.meta().items {
        ItemSource::Static(list) => list.iter().map(|&kind| stack_of(kind)).collect(),
        ItemSource::Search => {
            let raw = state.search.to_lowercase();
            let needle = raw.strip_prefix('#').unwrap_or(&raw);
            search_items_cached()
                .iter()
                .filter(|kind| {
                    needle.is_empty() || item_resource_name(**kind).to_lowercase().contains(needle)
                })
                .map(|&kind| stack_of(kind))
                .collect()
        }
        ItemSource::Empty => Vec::new(),
    }
}

fn stack_of(kind: ItemKind) -> ItemStack {
    ItemStack::Present(ItemStackData {
        kind,
        count: 1,
        component_patch: Default::default(),
    })
}

fn search_items_cached() -> &'static [ItemKind] {
    static CACHE: OnceLock<Vec<ItemKind>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for tab in TABS.iter() {
            if let ItemSource::Static(list) = tab.meta().items {
                for &kind in list {
                    if seen.insert(kind) {
                        out.push(kind);
                    }
                }
            }
        }
        out
    })
}
