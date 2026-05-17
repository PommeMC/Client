use std::sync::OnceLock;

use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::Registry;
use azalea_registry::builtin::ItemKind;

use super::common::{self, WHITE, hit_test, push_item_count};
use crate::player::inventory::{Inventory, item_resource_name};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const TEX_W: f32 = 195.0;
const TEX_H: f32 = 136.0;
const SLOT_SIZE: f32 = 16.0;
const SLOT_STRIDE: f32 = 18.0;
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
const SEARCH_BOX_W: f32 = 80.0;
const SEARCH_BOX_H: f32 = 9.0;
const TAB_W: f32 = 26.0;
const TAB_H: f32 = 28.0;
const TAB_GAP: f32 = 1.0;

const LABEL_COLOR: [f32; 4] = [0.25, 0.25, 0.25, 1.0];
const HIGHLIGHT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.5];
const TAB_BG: [f32; 4] = [0.78, 0.78, 0.78, 1.0];
const TAB_BG_HOVER: [f32; 4] = [0.88, 0.88, 0.88, 1.0];
const TAB_BG_SELECTED: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
const SEARCH_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
const SEARCH_CARET: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CreativeTab {
    Search,
    SurvivalInventory,
}

impl CreativeTab {
    fn label(self) -> &'static str {
        match self {
            CreativeTab::Search => "Search",
            CreativeTab::SurvivalInventory => "Inventory",
        }
    }

    fn scrollable(self) -> bool {
        matches!(self, CreativeTab::Search)
    }
}

const TABS: [CreativeTab; 2] = [CreativeTab::Search, CreativeTab::SurvivalInventory];

pub struct CreativeState {
    pub tab: CreativeTab,
    pub scroll: f32,
    pub search: String,
}

impl CreativeState {
    pub fn new() -> Self {
        Self {
            tab: CreativeTab::Search,
            scroll: 0.0,
            search: String::new(),
        }
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
) -> CreativeAction {
    if state.tab == CreativeTab::Search {
        if backspace {
            state.search.pop();
        }
        for &ch in typed_chars {
            if state.search.len() < 50 && !ch.is_control() {
                state.search.push(ch);
            }
        }
    }

    let scale = gs.min(screen_w / TEX_W).min(screen_h / TEX_H);
    let inv_w = TEX_W * scale;
    let inv_h = TEX_H * scale;
    let ox = (screen_w - inv_w) / 2.0;
    let oy = (screen_h - inv_h) / 2.0;

    common::push_overlay(elements, screen_w, screen_h, 0.5);

    let bg_sprite = if state.tab == CreativeTab::Search {
        SpriteId::CreativeSearchBackground
    } else {
        SpriteId::CreativeItemsBackground
    };
    elements.push(MenuElement::Image {
        x: ox,
        y: oy,
        w: inv_w,
        h: inv_h,
        sprite: bg_sprite,
        tint: WHITE,
    });

    let mut action = CreativeAction::None;

    let tab_action = draw_tabs(elements, state, ox, oy, scale, cursor, clicked);
    if let Some(new_tab) = tab_action
        && new_tab != state.tab
    {
        state.tab = new_tab;
        state.scroll = 0.0;
    }

    let items = visible_items(state, inventory);
    let max_scroll_rows = if state.tab.scrollable() {
        items.len().div_ceil(GRID_COLS).saturating_sub(GRID_ROWS)
    } else {
        0
    };

    if state.tab.scrollable() && max_scroll_rows > 0 {
        let inv_y =
            cursor.1 >= oy && cursor.1 <= oy + inv_h && cursor.0 >= ox && cursor.0 <= ox + inv_w;
        if inv_y && scroll_delta != 0.0 {
            let step = 1.0 / max_scroll_rows as f32;
            state.scroll = (state.scroll - scroll_delta.signum() * step).clamp(0.0, 1.0);
        }
    } else {
        state.scroll = 0.0;
    }

    let scroll_row_offset = (state.scroll * max_scroll_rows as f32).round() as usize;
    let item_offset = scroll_row_offset * GRID_COLS;

    if state.tab == CreativeTab::Search {
        draw_search_box(elements, &state.search, ox, oy, scale);
    }

    for row in 0..GRID_ROWS {
        for col in 0..GRID_COLS {
            let slot_idx = row * GRID_COLS + col;
            let global_idx = item_offset + slot_idx;
            let item = items.get(global_idx).cloned().unwrap_or(ItemStack::Empty);
            let slot_x = ox + (GRID_ORIGIN_X + col as f32 * SLOT_STRIDE) * scale;
            let slot_y = oy + (GRID_ORIGIN_Y + row as f32 * SLOT_STRIDE) * scale;
            let size = SLOT_SIZE * scale;
            let slot_clicked = draw_slot(
                elements, slot_x, slot_y, size, scale, &item, cursor, clicked,
            );
            if slot_clicked
                && let ItemStack::Present(data) = item
                && let CreativeTab::Search = state.tab
            {
                let slot_num = 36 + selected_hotbar as u16;
                action = CreativeAction::Place(ItemStack::Present(data), slot_num);
            }
        }
    }

    if state.tab.scrollable() {
        draw_scrollbar(elements, ox, oy, scale, state.scroll, max_scroll_rows == 0);
    }

    let outside = !hit_test(cursor, [ox, oy, inv_w, inv_h]);
    if clicked && outside {
        action = CreativeAction::Close;
    }

    action
}

fn draw_tabs(
    elements: &mut Vec<MenuElement>,
    state: &CreativeState,
    ox: f32,
    oy: f32,
    scale: f32,
    cursor: (f32, f32),
    clicked: bool,
) -> Option<CreativeTab> {
    let mut hit: Option<CreativeTab> = None;
    let tab_w = TAB_W * scale;
    let tab_h = TAB_H * scale;
    let label_size = 6.0 * scale;
    for (i, tab) in TABS.iter().enumerate() {
        let x = ox + (TAB_GAP + i as f32 * (TAB_W + TAB_GAP)) * scale;
        let y = oy - tab_h + 2.0 * scale;
        let selected = state.tab == *tab;
        let hovered = hit_test(cursor, [x, y, tab_w, tab_h]);
        let color = if selected {
            TAB_BG_SELECTED
        } else if hovered {
            TAB_BG_HOVER
        } else {
            TAB_BG
        };
        elements.push(MenuElement::Rect {
            x,
            y,
            w: tab_w,
            h: tab_h,
            corner_radius: 2.0 * scale,
            color,
        });
        elements.push(MenuElement::Text {
            x: x + tab_w / 2.0,
            y: y + (tab_h - label_size) / 2.0,
            text: tab.label().into(),
            scale: label_size,
            color: LABEL_COLOR,
            centered: true,
        });
        if hovered && clicked {
            hit = Some(*tab);
        }
    }
    hit
}

fn draw_search_box(elements: &mut Vec<MenuElement>, text: &str, ox: f32, oy: f32, scale: f32) {
    let x = ox + SEARCH_BOX_X * scale;
    let y = oy + SEARCH_BOX_Y * scale;
    let w = SEARCH_BOX_W * scale;
    let h = SEARCH_BOX_H * scale;
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
        color: SEARCH_BG,
    });
    let pad = 1.0 * scale;
    let fs = 6.0 * scale;
    elements.push(MenuElement::Text {
        x: x + pad,
        y: y + (h - fs) / 2.0,
        text: text.into(),
        scale: fs,
        color: WHITE,
        centered: false,
    });
    let caret_x = x + pad + text.len() as f32 * fs * 0.6;
    elements.push(MenuElement::Rect {
        x: caret_x,
        y: y + 1.5 * scale,
        w: 0.75 * scale,
        h: h - 3.0 * scale,
        corner_radius: 0.0,
        color: SEARCH_CARET,
    });
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
    let color = if disabled {
        [0.45, 0.45, 0.45, 1.0]
    } else {
        [0.75, 0.75, 0.75, 1.0]
    };
    elements.push(MenuElement::Rect {
        x: track_x,
        y: handle_y,
        w: handle_w,
        h: handle_h,
        corner_radius: 0.0,
        color,
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_slot(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    item: &ItemStack,
    cursor: (f32, f32),
    clicked: bool,
) -> bool {
    let hovered = hit_test(cursor, [x, y, size, size]);
    if hovered {
        elements.push(MenuElement::Rect {
            x,
            y,
            w: size,
            h: size,
            corner_radius: 0.0,
            color: HIGHLIGHT_COLOR,
        });
    }
    if let ItemStack::Present(data) = item {
        elements.push(MenuElement::ItemIcon {
            x,
            y,
            w: size,
            h: size,
            item_name: item_resource_name(data.kind),
            tint: WHITE,
        });
        if data.count > 1 {
            push_item_count(elements, x, y, size, scale, data.count);
        }
    }
    hovered && clicked
}

fn visible_items(state: &CreativeState, inventory: &Inventory) -> Vec<ItemStack> {
    match state.tab {
        CreativeTab::Search => {
            let needle = state.search.to_lowercase();
            all_items_cached()
                .iter()
                .filter(|kind| {
                    needle.is_empty() || item_resource_name(**kind).to_lowercase().contains(&needle)
                })
                .map(|&kind| stack_of(kind))
                .collect()
        }
        CreativeTab::SurvivalInventory => {
            let mut v = Vec::with_capacity(36);
            v.extend(inventory.main_slots().iter().cloned());
            v.extend(inventory.hotbar_slots().iter().cloned());
            v
        }
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
