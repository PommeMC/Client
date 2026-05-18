use std::sync::OnceLock;
use std::time::Instant;

use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::Registry;
use azalea_registry::builtin::ItemKind;

use super::common::{
    self, FONT_SIZE, SLOT_LABEL_COLOR, SLOT_SIZE, SLOT_STRIDE, WHITE, hit_test, push_slot,
};
use super::creative_tab_data::{
    BUILDING_BLOCKS_ITEMS, COLORED_BLOCKS_ITEMS, COMBAT_ITEMS, FOOD_AND_DRINKS_ITEMS,
    FUNCTIONAL_BLOCKS_ITEMS, INGREDIENTS_ITEMS, NATURAL_BLOCKS_ITEMS, OP_BLOCKS_ITEMS,
    REDSTONE_BLOCKS_ITEMS, SPAWN_EGGS_ITEMS, TOOLS_AND_UTILITIES_ITEMS,
};
use crate::player::inventory::{Inventory, item_resource_name};
use crate::renderer::pipelines::menu_overlay::{CREATIVE_TAB_SPRITES, MenuElement, SpriteId};

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
                icon: "minecraft:bricks",
                title: "Building Blocks",
                items: ItemSource::Static(BUILDING_BLOCKS_ITEMS),
            },
            CreativeTab::ColoredBlocks => TabMeta {
                row: Row::Top,
                col: 2,
                icon: "minecraft:cyan_wool",
                title: "Colored Blocks",
                items: ItemSource::Static(COLORED_BLOCKS_ITEMS),
            },
            CreativeTab::NaturalBlocks => TabMeta {
                row: Row::Top,
                col: 3,
                icon: "minecraft:grass_block",
                title: "Natural Blocks",
                items: ItemSource::Static(NATURAL_BLOCKS_ITEMS),
            },
            CreativeTab::FunctionalBlocks => TabMeta {
                row: Row::Top,
                col: 4,
                icon: "minecraft:oak_sign",
                title: "Functional Blocks",
                items: ItemSource::Static(FUNCTIONAL_BLOCKS_ITEMS),
            },
            CreativeTab::RedstoneBlocks => TabMeta {
                row: Row::Top,
                col: 5,
                icon: "minecraft:redstone",
                title: "Redstone Blocks",
                items: ItemSource::Static(REDSTONE_BLOCKS_ITEMS),
            },
            CreativeTab::Hotbar => TabMeta {
                row: Row::Top,
                col: 6,
                icon: "minecraft:bookshelf",
                title: "Saved Hotbars",
                items: ItemSource::Empty,
            },
            CreativeTab::Search => TabMeta {
                row: Row::Top,
                col: 7,
                icon: "minecraft:compass",
                title: "Search",
                items: ItemSource::Search,
            },
            CreativeTab::ToolsAndUtilities => TabMeta {
                row: Row::Bottom,
                col: 1,
                icon: "minecraft:diamond_pickaxe",
                title: "Tools & Utilities",
                items: ItemSource::Static(TOOLS_AND_UTILITIES_ITEMS),
            },
            CreativeTab::Combat => TabMeta {
                row: Row::Bottom,
                col: 2,
                icon: "minecraft:netherite_sword",
                title: "Combat",
                items: ItemSource::Static(COMBAT_ITEMS),
            },
            CreativeTab::FoodAndDrinks => TabMeta {
                row: Row::Bottom,
                col: 3,
                icon: "minecraft:golden_apple",
                title: "Food & Drinks",
                items: ItemSource::Static(FOOD_AND_DRINKS_ITEMS),
            },
            CreativeTab::Ingredients => TabMeta {
                row: Row::Bottom,
                col: 4,
                icon: "minecraft:iron_ingot",
                title: "Ingredients",
                items: ItemSource::Static(INGREDIENTS_ITEMS),
            },
            CreativeTab::SpawnEggs => TabMeta {
                row: Row::Bottom,
                col: 5,
                icon: "minecraft:creeper_spawn_egg",
                title: "Spawn Eggs",
                items: ItemSource::Static(SPAWN_EGGS_ITEMS),
            },
            CreativeTab::OpBlocks => TabMeta {
                row: Row::Bottom,
                col: 6,
                icon: "minecraft:command_block",
                title: "Operator Utilities",
                items: ItemSource::Static(OP_BLOCKS_ITEMS),
            },
            CreativeTab::SurvivalInventory => TabMeta {
                row: Row::Bottom,
                col: 7,
                icon: "minecraft:chest",
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

    fn captures_typing(self) -> bool {
        matches!(self, CreativeTab::Search)
    }
}

const TABS: [CreativeTab; 13] = [
    CreativeTab::BuildingBlocks,
    CreativeTab::ColoredBlocks,
    CreativeTab::NaturalBlocks,
    CreativeTab::FunctionalBlocks,
    CreativeTab::RedstoneBlocks,
    CreativeTab::Hotbar,
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
    cursor_blink: Instant,
}

impl CreativeState {
    pub fn new() -> Self {
        Self {
            tab: CreativeTab::Search,
            scroll: 0.0,
            search: String::new(),
            cursor_blink: Instant::now(),
        }
    }

    fn reset_blink(&mut self) {
        self.cursor_blink = Instant::now();
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
    Place(ItemStack, u16),
}

#[allow(clippy::too_many_arguments)]
pub fn build_creative_inventory(
    elements: &mut Vec<MenuElement>,
    state: &mut CreativeState,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    scroll_delta: f32,
    typed_chars: &[char],
    backspace: bool,
    inventory: &Inventory,
    selected_hotbar: u8,
    gs: f32,
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
        state.search.clear();
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

    if state.tab.is_inventory_tab() {
        draw_inventory_layout(elements, ox, oy, scale, cursor, inventory);
    } else {
        let items = visible_items(state);
        let scrollable = state.tab.scrollable();
        let max_scroll_rows = if scrollable {
            items.len().div_ceil(GRID_COLS).saturating_sub(GRID_ROWS)
        } else {
            0
        };

        if scrollable && max_scroll_rows > 0 {
            let inside = hit_test(cursor, [ox, oy, inv_w, inv_h]);
            if inside && scroll_delta != 0.0 {
                let step = 1.0 / max_scroll_rows as f32;
                state.scroll = (state.scroll - scroll_delta.signum() * step).clamp(0.0, 1.0);
            }
        } else {
            state.scroll = 0.0;
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
                let hovered =
                    push_slot(elements, slot_x, slot_y, size, scale, cursor, &item, None);
                if hovered
                    && clicked
                    && scrollable
                    && let ItemStack::Present(data) = item
                {
                    let slot_num = 36 + selected_hotbar as u16;
                    action = CreativeAction::Place(ItemStack::Present(data), slot_num);
                }
            }
        }

        draw_player_hotbar(elements, ox, oy, scale, cursor, inventory);

        if scrollable {
            draw_scrollbar(elements, ox, oy, scale, state.scroll, max_scroll_rows == 0);
        }
    }

    let outside = !hit_test(cursor, [ox, oy, inv_w, inv_h]);
    if clicked && outside && tab_hit.is_none() && matches!(action, CreativeAction::None) {
        action = CreativeAction::Close;
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

fn draw_player_hotbar(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    cursor: (f32, f32),
    inventory: &Inventory,
) {
    let size = SLOT_SIZE * scale;
    let hotbar = inventory.hotbar_slots();
    for col in 0..GRID_COLS {
        let (x, y) = slot_xy(ox, oy, scale, GRID_ORIGIN_X + col as f32 * SLOT_STRIDE, HOTBAR_Y);
        let item = item_or_empty(hotbar, col);
        push_slot(elements, x, y, size, scale, cursor, &item, None);
    }
}

fn draw_inventory_layout(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    cursor: (f32, f32),
    inventory: &Inventory,
) {
    let size = SLOT_SIZE * scale;

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
        let item = item_or_empty(armor, i);
        push_slot(elements, x, y, size, scale, cursor, &item, None);
    }

    let (x, y) = slot_xy(ox, oy, scale, INV_OFFHAND_X, INV_OFFHAND_Y);
    push_slot(elements, x, y, size, scale, cursor, inventory.offhand(), None);

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
            let item = item_or_empty(main, idx);
            push_slot(elements, x, y, size, scale, cursor, &item, None);
        }
    }

    draw_player_hotbar(elements, ox, oy, scale, cursor, inventory);

    let (trash_x, trash_y) = slot_xy(ox, oy, scale, INV_TRASH_X, INV_TRASH_Y);
    push_slot(
        elements,
        trash_x,
        trash_y,
        size,
        scale,
        cursor,
        &ItemStack::Empty,
        None,
    );
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
    let handle_y = track_y + scroll * (track_h - handle_h);
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
            let needle = state.search.to_lowercase();
            all_items_cached()
                .iter()
                .filter(|kind| {
                    needle.is_empty() || item_resource_name(**kind).to_lowercase().contains(&needle)
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

fn all_items_cached() -> &'static [ItemKind] {
    static CACHE: OnceLock<Vec<ItemKind>> = OnceLock::new();
    CACHE.get_or_init(|| {
        (0u32..)
            .map_while(ItemKind::from_u32)
            .filter(|k| !matches!(k, ItemKind::Air))
            .collect()
    })
}
