use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use azalea_inventory::components::{
    self, CustomName, Damage, DataComponentTrait, Dye, DyeColor, DyedColor, Enchantments,
    EncodableDataComponent, PotionContents, ProvidesTrimMaterial, Trim,
};
use azalea_inventory::default_components::get_default_component;
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::builtin::{DataComponentKind, ItemKind, Potion};
use azalea_registry::{DataRegistry, DataRegistryKey, Holder, Registry};

use super::common::{FONT_SIZE, WHITE, hit_test, push_tooltip, push_tooltip_lines};
use super::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};
use crate::net::sender::PacketSender;
use crate::recipe::{
    Ingredient, ItemTags, RecipeBookEntry, RecipeBookState, RecipeBookType, RecipeDisplay,
    RecipeDisplayId, SlotDisplay, TrimPatternHolder,
};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};

const BOOK_W: f32 = 147.0;
const BOOK_H: f32 = 166.0;
const ITEMS_PER_PAGE: usize = 20;
const HIGHLIGHT_ANIMATION: Duration = Duration::from_millis(750);
const SEARCH_X: f32 = 25.0;
const SEARCH_Y: f32 = 13.0;
const SEARCH_W: f32 = 81.0;
const SEARCH_H: f32 = 14.0;
const FILTER_X: f32 = 110.0;
const FILTER_Y: f32 = 12.0;
const FILTER_W: f32 = 26.0;
const FILTER_H: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CraftabilitySource {
    PlayerInventory,
    CraftingTable,
    Furnace,
}

#[derive(Clone, Copy, Debug)]
pub struct RecipeBookScreenSpec {
    pub kind: RecipeBookType,
    pub container_id: i32,
    pub grid_width: usize,
    pub grid_height: usize,
    pub big_result_slot: bool,
    craftability_source: CraftabilitySource,
    pub toggle_x: f32,
    pub toggle_y: f32,
    pub panel_h: f32,
    /// Slots that are crafting/furnace inputs and should be cleared from the
    /// current ghost preview when the user manually clicks them.
    pub crafting_slots: &'static [usize],
}

impl RecipeBookScreenSpec {
    pub const fn player(container_id: i32) -> Self {
        Self {
            kind: RecipeBookType::Crafting,
            container_id,
            grid_width: 2,
            grid_height: 2,
            big_result_slot: false,
            craftability_source: CraftabilitySource::PlayerInventory,
            toggle_x: 104.0,
            toggle_y: 61.0,
            panel_h: 166.0,
            crafting_slots: &[0, 1, 2, 3, 4],
        }
    }

    pub const fn crafting_table(container_id: i32) -> Self {
        Self {
            kind: RecipeBookType::Crafting,
            container_id,
            grid_width: 3,
            grid_height: 3,
            big_result_slot: true,
            craftability_source: CraftabilitySource::CraftingTable,
            toggle_x: 5.0,
            toggle_y: 34.0,
            panel_h: 166.0,
            crafting_slots: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        }
    }

    pub const fn furnace(container_id: i32, kind: RecipeBookType) -> Self {
        Self {
            kind,
            container_id,
            grid_width: 1,
            grid_height: 1,
            big_result_slot: true,
            craftability_source: CraftabilitySource::Furnace,
            toggle_x: 20.0,
            toggle_y: 34.0,
            panel_h: 166.0,
            crafting_slots: &[0, 1, 2],
        }
    }
}

pub struct RecipeBookUiState {
    search: TextFieldState,
    search_focused: bool,
    screen: Option<(RecipeBookType, i32)>,
    selected_tab: usize,
    page: usize,
    overlay: Option<RecipeOverlay>,
    last_placed: Option<RecipeDisplayId>,
    last_clicked_recipe: Option<RecipeDisplayId>,
    recipe_animation_started: HashMap<RecipeDisplayId, Instant>,
    tab_animation_started: HashMap<i32, Instant>,
    tab_animation_highlights: HashMap<i32, HashSet<RecipeDisplayId>>,
    narrow: bool,
    cycle_time: Duration,
    cycle_last_update: Instant,
}

impl RecipeBookUiState {
    pub fn new() -> Self {
        Self {
            search: TextFieldState::new(50),
            search_focused: false,
            screen: None,
            selected_tab: 0,
            page: 0,
            overlay: None,
            last_placed: None,
            last_clicked_recipe: None,
            recipe_animation_started: HashMap::new(),
            tab_animation_started: HashMap::new(),
            tab_animation_highlights: HashMap::new(),
            narrow: false,
            cycle_time: Duration::ZERO,
            cycle_last_update: Instant::now(),
        }
    }

    pub fn captures_typing(&self) -> bool {
        self.search_focused
    }

    pub fn reset_for_closed_screen(&mut self, book: &mut RecipeBookState) {
        self.search.clear();
        self.search_focused = false;
        self.screen = None;
        self.selected_tab = 0;
        self.page = 0;
        self.overlay = None;
        self.last_placed = None;
        self.last_clicked_recipe = None;
        self.recipe_animation_started.clear();
        self.tab_animation_started.clear();
        self.tab_animation_highlights.clear();
        self.narrow = false;
        self.cycle_time = Duration::ZERO;
        self.cycle_last_update = Instant::now();
        book.ghost_recipe = None;
    }

    pub fn focus_search_from_chat_key(
        &mut self,
        book: &RecipeBookState,
        screen_active: bool,
    ) -> bool {
        if !screen_active {
            return false;
        }
        let Some((kind, _)) = self.screen else {
            return false;
        };
        if !book.settings.get(kind).open || self.search_focused {
            return false;
        }
        self.search_focused = true;
        self.search.set_focused(true);
        true
    }

    pub fn close_for_escape(&mut self, book: &mut RecipeBookState, sender: &PacketSender) -> bool {
        let Some((kind, _)) = self.screen else {
            return false;
        };
        let mut settings = book.settings.get(kind);
        if !self.narrow || !settings.open {
            return false;
        }
        settings.open = false;
        book.settings.set(kind, settings);
        self.search_focused = false;
        self.search.set_focused(false);
        self.overlay = None;
        let _ = sender.recipe_book_change_settings(kind, false, settings.filtering);
        true
    }

    pub fn crafting_slot_clicked(&mut self, book: &mut RecipeBookState) {
        self.last_placed = None;
        book.ghost_recipe = None;
    }

    fn ensure_screen(&mut self, spec: RecipeBookScreenSpec) {
        let key = (spec.kind, spec.container_id);
        if self.screen != Some(key) {
            self.screen = Some(key);
            self.selected_tab = 0;
            self.page = 0;
            self.overlay = None;
            self.last_placed = None;
            self.last_clicked_recipe = None;
            self.recipe_animation_started.clear();
            self.tab_animation_started.clear();
            self.tab_animation_highlights.clear();
            self.search.clear();
            self.search_focused = false;
            self.cycle_time = Duration::ZERO;
            self.cycle_last_update = Instant::now();
        }
    }

    fn update_cycle_time(&mut self, ctrl_held: bool) {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.cycle_last_update);
        self.cycle_last_update = now;
        if !ctrl_held {
            self.cycle_time += delta;
        }
    }

    fn cycle_index(&self) -> usize {
        (self.cycle_time.as_millis() / 1_500) as usize
    }
}

impl Default for RecipeBookUiState {
    fn default() -> Self {
        Self::new()
    }
}

fn reset_page_if_out_of_range(page: &mut usize, pages: usize) {
    if *page >= pages {
        *page = 0;
    }
}

fn animation_squeeze(started: Instant, now: Instant) -> Option<f32> {
    let elapsed = now.saturating_duration_since(started);
    if elapsed >= HIGHLIGHT_ANIMATION {
        return None;
    }
    let t = elapsed.as_secs_f32() / HIGHLIGHT_ANIMATION.as_secs_f32();
    Some(1.0 + 0.1 * (std::f32::consts::PI * t).sin())
}

fn tab_animation_squeeze(
    state: &mut RecipeBookUiState,
    key: i32,
    highlights: HashSet<RecipeDisplayId>,
    now: Instant,
) -> f32 {
    let changed = state.tab_animation_highlights.get(&key) != Some(&highlights);
    if changed {
        if highlights.is_empty() {
            state.tab_animation_highlights.remove(&key);
            state.tab_animation_started.remove(&key);
        } else {
            state.tab_animation_highlights.insert(key, highlights);
            state.tab_animation_started.insert(key, now);
        }
    }

    let squeeze = state
        .tab_animation_started
        .get(&key)
        .and_then(|started| animation_squeeze(*started, now));
    if squeeze.is_none() {
        // Keep the highlight set latched after the pulse ends so the same
        // unseen recipe cannot re-arm the animation on the next render.
        state.tab_animation_started.remove(&key);
    }
    squeeze.unwrap_or(1.0)
}

fn scale_rect_about(rect: [f32; 4], pivot: (f32, f32), scale: (f32, f32)) -> [f32; 4] {
    [
        pivot.0 + (rect[0] - pivot.0) * scale.0,
        pivot.1 + (rect[1] - pivot.1) * scale.1,
        rect[2] * scale.0,
        rect[3] * scale.1,
    ]
}

#[derive(Clone, Debug)]
struct RecipeOverlay {
    ids: Vec<RecipeDisplayId>,
    craftable_count: usize,
    button_index: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecipeBookFrame {
    pub visible: bool,
    pub narrow: bool,
    pub main_x_offset: f32,
    pub consumed_left_click: bool,
    pub consumed_right_click: bool,
}

#[derive(Clone)]
struct Collection {
    entries: Vec<RecipeBookEntry>,
    craftable: HashSet<RecipeDisplayId>,
}

impl Collection {
    fn selected(&self, filtering: bool) -> Vec<&RecipeBookEntry> {
        self.entries
            .iter()
            .filter(|entry| !filtering || self.craftable.contains(&entry.id))
            .collect()
    }

    fn has_craftable(&self) -> bool {
        !self.craftable.is_empty()
    }
}

#[derive(Clone, Copy)]
struct TabInfo {
    category: Option<u32>,
    icon_a: &'static str,
    icon_b: Option<&'static str>,
}

pub fn visible(book: &RecipeBookState, kind: RecipeBookType) -> bool {
    book.settings.get(kind).open
}

pub fn width_too_narrow(screen_w: f32, gs: f32) -> bool {
    screen_w / gs.max(0.001) < 379.0
}

pub fn main_x_offset(book: &RecipeBookState, kind: RecipeBookType, screen_w: f32, gs: f32) -> f32 {
    if visible(book, kind) && !width_too_narrow(screen_w, gs) {
        77.0
    } else {
        0.0
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_input(
    state: &mut RecipeBookUiState,
    book: &mut RecipeBookState,
    sender: &PacketSender,
    spec: RecipeBookScreenSpec,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
    left_clicked: bool,
    right_clicked: bool,
    shift: bool,
    selection_key: bool,
    text_events: &[TextInputEvent],
    slots: &[ItemStack],
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> RecipeBookFrame {
    state.ensure_screen(spec);

    let narrow = width_too_narrow(screen_w, gs);
    state.narrow = narrow;
    let mut frame = RecipeBookFrame {
        visible: visible(book, spec.kind),
        narrow,
        main_x_offset: main_x_offset(book, spec.kind, screen_w, gs),
        ..Default::default()
    };

    let main = main_panel_rect(screen_w, screen_h, gs, spec.panel_h, frame.main_x_offset);
    let toggle = [
        main.0 + spec.toggle_x * main.4,
        main.1 + spec.toggle_y * main.4,
        20.0 * main.4,
        18.0 * main.4,
    ];
    if left_clicked && hit_test(cursor, toggle) {
        let mut settings = book.settings.get(spec.kind);
        settings.open = !settings.open;
        book.settings.set(spec.kind, settings);
        state.search_focused = false;
        state.overlay = None;
        let _ = sender.recipe_book_change_settings(spec.kind, settings.open, settings.filtering);
        frame.visible = settings.open;
        frame.main_x_offset = if settings.open && !narrow { 77.0 } else { 0.0 };
        frame.consumed_left_click = true;
    }

    if !frame.visible {
        return frame;
    }

    let (bx, by, scale) = book_origin(screen_w, screen_h, gs, narrow);
    let search_rect = [
        bx + SEARCH_X * scale,
        by + SEARCH_Y * scale,
        SEARCH_W * scale,
        SEARCH_H * scale,
    ];
    let magnifier_rect = [
        bx + 8.0 * scale,
        by + SEARCH_Y * scale,
        17.0 * scale,
        SEARCH_H * scale,
    ];
    let filter_rect = [
        bx + FILTER_X * scale,
        by + FILTER_Y * scale,
        FILTER_W * scale,
        FILTER_H * scale,
    ];

    if left_clicked {
        if hit_test(cursor, search_rect) || hit_test(cursor, magnifier_rect) {
            state.search_focused = true;
            state.search.set_focused(true);
            let text_x = search_rect[0] + 4.0 * scale;
            let rel = (cursor.0 - text_x).max(0.0);
            let fs = FONT_SIZE * scale;
            let wf = |s: &str| text_width_fn(s, fs);
            let inner_w = (SEARCH_W - 8.0) * scale;
            let pos = state.search.pos_from_click(rel, inner_w, &wf);
            state.search.on_click(pos, shift, inner_w, &wf);
            frame.consumed_left_click = true;
        } else {
            state.search_focused = false;
        }
    }

    if state.search_focused {
        let old = state.search.value().to_owned();
        let fs = FONT_SIZE * scale;
        let wf = |s: &str| text_width_fn(s, fs);
        let mut clipboard = SystemClipboard;
        for event in text_events {
            state
                .search
                .handle(event, &mut clipboard, (SEARCH_W - 8.0) * scale, &wf);
        }
        if state.search.value() != old {
            state.page = 0;
            state.overlay = None;
        }
    }

    if left_clicked && hit_test(cursor, filter_rect) {
        let mut settings = book.settings.get(spec.kind);
        settings.filtering = !settings.filtering;
        book.settings.set(spec.kind, settings);
        let _ = sender.recipe_book_change_settings(spec.kind, settings.open, settings.filtering);
        state.page = 0;
        state.overlay = None;
        frame.consumed_left_click = true;
    }

    let available = available_items(slots, spec);
    let tabs = tabs_for(spec.kind);
    let all_collections = collections(book, spec, &available);
    let visible_tabs = visible_tabs(
        tabs,
        &all_collections,
        book.settings.get(spec.kind).filtering,
    );
    if state.selected_tab >= visible_tabs.len() {
        state.selected_tab = 0;
        state.page = 0;
    }

    let mut tab_y = by + 3.0 * scale;
    for (visible_index, _tab) in visible_tabs.iter().enumerate() {
        let tab_rect = [bx - 30.0 * scale, tab_y, 35.0 * scale, 27.0 * scale];
        if left_clicked && hit_test(cursor, tab_rect) {
            if state.selected_tab != visible_index {
                state.selected_tab = visible_index;
                state.page = 0;
                state.overlay = None;
            }
            frame.consumed_left_click = true;
        }
        tab_y += 27.0 * scale;
    }

    let selected_tab = visible_tabs
        .get(state.selected_tab)
        .copied()
        .unwrap_or(tabs[0]);
    let filtered = filtered_collections(
        &all_collections,
        selected_tab.category,
        book.settings.get(spec.kind).filtering,
        state.search.value(),
        book,
    );
    let pages = filtered.len().div_ceil(ITEMS_PER_PAGE).max(1);
    reset_page_if_out_of_range(&mut state.page, pages);

    let back_rect = [
        bx + 38.0 * scale,
        by + 137.0 * scale,
        12.0 * scale,
        17.0 * scale,
    ];
    let forward_rect = [
        bx + 93.0 * scale,
        by + 137.0 * scale,
        12.0 * scale,
        17.0 * scale,
    ];
    if left_clicked && state.page > 0 && hit_test(cursor, back_rect) {
        state.page -= 1;
        state.overlay = None;
        frame.consumed_left_click = true;
    }
    if left_clicked && state.page + 1 < pages && hit_test(cursor, forward_rect) {
        state.page += 1;
        state.overlay = None;
        frame.consumed_left_click = true;
    }

    if let Some(overlay) = state.overlay.clone() {
        let overlay_rect = overlay_rect_for(&overlay, bx, by, scale);
        if left_clicked {
            if let Some(id) = hit_overlay_recipe(cursor, &overlay, overlay_rect, scale) {
                let use_max = shift;
                if state.last_placed != Some(id)
                    || can_craft_entry(book.known.get(&id), &available, &book.item_tags)
                {
                    let _ = sender.place_recipe(spec.container_id, id, use_max);
                    state.last_placed = Some(id);
                    state.last_clicked_recipe = Some(id);
                    book.ghost_recipe = None;
                }
                if narrow {
                    let mut settings = book.settings.get(spec.kind);
                    settings.open = false;
                    book.settings.set(spec.kind, settings);
                    let _ =
                        sender.recipe_book_change_settings(spec.kind, false, settings.filtering);
                    state.overlay = None;
                    frame.visible = false;
                    frame.main_x_offset = 0.0;
                }
                frame.consumed_left_click = true;
            } else {
                state.overlay = None;
                frame.consumed_left_click = true;
            }
        } else if right_clicked {
            state.overlay = None;
            frame.consumed_right_click = true;
        }
        return frame;
    }

    let start = state.page * ITEMS_PER_PAGE;
    let page_collections = filtered
        .iter()
        .skip(start)
        .take(ITEMS_PER_PAGE)
        .collect::<Vec<_>>();
    let cycle = state.cycle_index();
    for (index, collection) in page_collections.iter().enumerate() {
        let x = bx + (11.0 + 25.0 * (index % 5) as f32) * scale;
        let y = by + (31.0 + 25.0 * (index / 5) as f32) * scale;
        let rect = [x, y, 25.0 * scale, 25.0 * scale];
        let selected = collection.selected(book.settings.get(spec.kind).filtering);
        if selected.is_empty() {
            continue;
        }
        let current = selected[cycle % selected.len()];
        if left_clicked && hit_test(cursor, rect) {
            let craftable = collection.craftable.contains(&current.id);
            if craftable || state.last_placed != Some(current.id) {
                let _ = sender.place_recipe(spec.container_id, current.id, shift);
                state.last_placed = Some(current.id);
                state.last_clicked_recipe = Some(current.id);
                book.ghost_recipe = None;
            }
            if narrow {
                let mut settings = book.settings.get(spec.kind);
                settings.open = false;
                book.settings.set(spec.kind, settings);
                let _ = sender.recipe_book_change_settings(spec.kind, false, settings.filtering);
                frame.visible = false;
                frame.main_x_offset = 0.0;
            }
            frame.consumed_left_click = true;
        } else if right_clicked && hit_test(cursor, rect) && selected.len() > 1 {
            let mut ids = collection
                .entries
                .iter()
                .filter(|entry| collection.craftable.contains(&entry.id))
                .map(|entry| entry.id)
                .collect::<Vec<_>>();
            let craftable_count = ids.len();
            if !book.settings.get(spec.kind).filtering {
                ids.extend(
                    collection
                        .entries
                        .iter()
                        .filter(|entry| !collection.craftable.contains(&entry.id))
                        .map(|entry| entry.id),
                );
            }
            state.overlay = Some(RecipeOverlay {
                ids,
                craftable_count,
                button_index: index,
            });
            frame.consumed_right_click = true;
        }
    }

    if selection_key && let Some(id) = state.last_clicked_recipe {
        let craftable = book
            .known
            .get(&id)
            .is_some_and(|entry| can_craft(entry, &available, &book.item_tags));
        if craftable || state.last_placed != Some(id) {
            let _ = sender.place_recipe(spec.container_id, id, shift);
            state.last_placed = Some(id);
            book.ghost_recipe = None;
        }
    }

    frame
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    elements: &mut Vec<MenuElement>,
    state: &mut RecipeBookUiState,
    book: &mut RecipeBookState,
    sender: &PacketSender,
    spec: RecipeBookScreenSpec,
    frame: RecipeBookFrame,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
    slots: &[ItemStack],
    ctrl_held: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    state.update_cycle_time(ctrl_held || !frame.visible);
    let main = main_panel_rect(screen_w, screen_h, gs, spec.panel_h, frame.main_x_offset);
    let toggle = [
        main.0 + spec.toggle_x * main.4,
        main.1 + spec.toggle_y * main.4,
        20.0 * main.4,
        18.0 * main.4,
    ];

    if !(frame.visible && frame.narrow) {
        render_ghost_recipe(
            elements, state, book, spec, main, cursor, slots, screen_w, screen_h,
        );
    }

    if frame.visible && frame.narrow {
        // Vanilla only renders the recipe-book side of the screen in this
        // layout. Cover the already-built container cleanly before the book.
        elements.push(MenuElement::Rect {
            x: 0.0,
            y: 0.0,
            w: screen_w,
            h: screen_h,
            corner_radius: 0.0,
            color: [0.0, 0.0, 0.0, 0.65],
        });
    }

    if frame.visible {
        let (bx, by, scale) = book_origin(screen_w, screen_h, gs, frame.narrow);
        elements.push(MenuElement::Image {
            x: bx,
            y: by,
            w: BOOK_W * scale,
            h: BOOK_H * scale,
            sprite: SpriteId::RecipeBookBackground,
            tint: WHITE,
        });

        render_search(
            elements,
            state,
            bx,
            by,
            scale,
            cursor,
            screen_w,
            screen_h,
            text_width_fn,
        );
        render_filter(
            elements, book, spec.kind, bx, by, scale, cursor, screen_w, screen_h,
        );

        let available = available_items(slots, spec);
        let tabs = tabs_for(spec.kind);
        let all_collections = collections(book, spec, &available);
        let visible_tabs = visible_tabs(
            tabs,
            &all_collections,
            book.settings.get(spec.kind).filtering,
        );
        render_tabs(
            elements,
            state,
            book,
            &visible_tabs,
            &all_collections,
            bx,
            by,
            scale,
            cursor,
        );

        let selected_tab = visible_tabs
            .get(state.selected_tab)
            .copied()
            .unwrap_or(tabs[0]);
        let filtered = filtered_collections(
            &all_collections,
            selected_tab.category,
            book.settings.get(spec.kind).filtering,
            state.search.value(),
            book,
        );
        let total_pages = filtered.len().div_ceil(ITEMS_PER_PAGE).max(1);
        let start = state.page.min(total_pages - 1) * ITEMS_PER_PAGE;
        let cycle = state.cycle_index();
        let page = filtered
            .iter()
            .skip(start)
            .take(ITEMS_PER_PAGE)
            .collect::<Vec<_>>();
        let now = Instant::now();
        let mut shown_highlights = Vec::new();
        for (index, collection) in page.iter().enumerate() {
            let x = bx + (11.0 + 25.0 * (index % 5) as f32) * scale;
            let y = by + (31.0 + 25.0 * (index / 5) as f32) * scale;
            let selected = collection.selected(book.settings.get(spec.kind).filtering);
            if selected.is_empty() {
                continue;
            }
            for entry in &selected {
                if book.highlight.contains(&entry.id) {
                    shown_highlights.push(entry.id);
                    state
                        .recipe_animation_started
                        .entry(entry.id)
                        .or_insert(now);
                }
            }
            let mut squeeze = None;
            for entry in &selected {
                if let Some(started) = state.recipe_animation_started.get(&entry.id).copied() {
                    if let Some(value) = animation_squeeze(started, now) {
                        squeeze = Some(value);
                        break;
                    }
                    state.recipe_animation_started.remove(&entry.id);
                }
            }
            let squeeze = squeeze.unwrap_or(1.0);
            let pivot_x = x + 8.0 * scale;
            let pivot_y = y + 12.0 * scale;
            let button_rect = scale_rect_about(
                [x, y, 25.0 * scale, 25.0 * scale],
                (pivot_x, pivot_y),
                (squeeze, squeeze),
            );

            let entry_count = selected.len();
            let current = selected[cycle % entry_count];
            let result_cycle = cycle / entry_count;
            let craftable = collection.has_craftable();
            let multiple = entry_count > 1;
            let same_result = multiple && all_recipes_have_same_result(&selected, book);
            let sprite = match (craftable, multiple) {
                (true, false) => SpriteId::RecipeBookSlotCraftable,
                (true, true) => SpriteId::RecipeBookSlotManyCraftable,
                (false, false) => SpriteId::RecipeBookSlotUncraftable,
                (false, true) => SpriteId::RecipeBookSlotManyUncraftable,
            };
            elements.push(MenuElement::Image {
                x: button_rect[0],
                y: button_rect[1],
                w: button_rect[2],
                h: button_rect[3],
                sprite,
                tint: WHITE,
            });
            if let Some(item) = display_item(&current.display, book, result_cycle) {
                let push_animated_stack =
                    |elements: &mut Vec<MenuElement>,
                     offset_x: f32,
                     offset_y: f32,
                     item: &ResolvedSlotStack| {
                        let rect = scale_rect_about(
                            [
                                x + offset_x * scale,
                                y + offset_y * scale,
                                16.0 * scale,
                                16.0 * scale,
                            ],
                            (pivot_x, pivot_y),
                            (squeeze, squeeze),
                        );
                        push_book_stack(elements, rect[0], rect[1], rect[2], item);
                    };
                if same_result {
                    push_animated_stack(elements, 5.0, 5.0, &item);
                    push_animated_stack(elements, 3.0, 3.0, &item);
                } else {
                    push_animated_stack(elements, 4.0, 4.0, &item);
                }
                if hit_test(cursor, [x, y, 25.0 * scale, 25.0 * scale]) && state.overlay.is_none() {
                    let mut lines = stack_tooltip_lines(&item);
                    if multiple {
                        lines.push(TooltipLine::new("Right Click for More".into(), WHITE));
                    }
                    push_tooltip_lines(elements, cursor, screen_w, screen_h, scale, lines);
                }
            }
        }
        shown_highlights.sort_unstable();
        shown_highlights.dedup();
        for id in shown_highlights {
            book.mark_seen(id);
            let _ = sender.recipe_book_seen_recipe(id);
        }

        if total_pages > 1 {
            let label = format!("{}/{}", state.page + 1, total_pages);
            elements.push(MenuElement::Text {
                x: bx + 73.0 * scale,
                y: by + 141.0 * scale,
                text: label,
                scale: FONT_SIZE * scale,
                color: WHITE,
                centered: true,
            });
            if state.page > 0 {
                let rect = [
                    bx + 38.0 * scale,
                    by + 137.0 * scale,
                    12.0 * scale,
                    17.0 * scale,
                ];
                elements.push(MenuElement::Image {
                    x: rect[0],
                    y: rect[1],
                    w: rect[2],
                    h: rect[3],
                    sprite: if hit_test(cursor, rect) {
                        SpriteId::RecipeBookPageBackwardHighlighted
                    } else {
                        SpriteId::RecipeBookPageBackward
                    },
                    tint: WHITE,
                });
            }
            if state.page + 1 < total_pages {
                let rect = [
                    bx + 93.0 * scale,
                    by + 137.0 * scale,
                    12.0 * scale,
                    17.0 * scale,
                ];
                elements.push(MenuElement::Image {
                    x: rect[0],
                    y: rect[1],
                    w: rect[2],
                    h: rect[3],
                    sprite: if hit_test(cursor, rect) {
                        SpriteId::RecipeBookPageForwardHighlighted
                    } else {
                        SpriteId::RecipeBookPageForward
                    },
                    tint: WHITE,
                });
            }
        }

        if let Some(overlay) = &state.overlay {
            render_overlay(
                elements,
                overlay,
                book,
                bx,
                by,
                scale,
                cursor,
                state.cycle_index(),
            );
        }
    }

    elements.push(MenuElement::Image {
        x: toggle[0],
        y: toggle[1],
        w: toggle[2],
        h: toggle[3],
        sprite: if hit_test(cursor, toggle) {
            SpriteId::RecipeBookButtonHighlighted
        } else {
            SpriteId::RecipeBookButton
        },
        tint: WHITE,
    });
}

pub fn click_hits_book(
    book: &RecipeBookState,
    kind: RecipeBookType,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
) -> bool {
    if !visible(book, kind) {
        return false;
    }
    let narrow = width_too_narrow(screen_w, gs);
    if narrow {
        return true;
    }
    let (bx, by, scale) = book_origin(screen_w, screen_h, gs, false);
    hit_test(
        cursor,
        [
            bx - 30.0 * scale,
            by,
            (BOOK_W + 30.0) * scale,
            BOOK_H * scale,
        ],
    )
}

fn tabs_for(kind: RecipeBookType) -> &'static [TabInfo] {
    const CRAFTING: &[TabInfo] = &[
        TabInfo {
            category: None,
            icon_a: "minecraft:compass",
            icon_b: None,
        },
        TabInfo {
            category: Some(2),
            icon_a: "minecraft:iron_axe",
            icon_b: Some("minecraft:golden_sword"),
        },
        TabInfo {
            category: Some(0),
            icon_a: "minecraft:bricks",
            icon_b: None,
        },
        TabInfo {
            category: Some(3),
            icon_a: "minecraft:lava_bucket",
            icon_b: Some("minecraft:apple"),
        },
        TabInfo {
            category: Some(1),
            icon_a: "minecraft:redstone",
            icon_b: None,
        },
    ];
    const FURNACE: &[TabInfo] = &[
        TabInfo {
            category: None,
            icon_a: "minecraft:compass",
            icon_b: None,
        },
        TabInfo {
            category: Some(4),
            icon_a: "minecraft:porkchop",
            icon_b: None,
        },
        TabInfo {
            category: Some(5),
            icon_a: "minecraft:stone",
            icon_b: None,
        },
        TabInfo {
            category: Some(6),
            icon_a: "minecraft:lava_bucket",
            icon_b: Some("minecraft:emerald"),
        },
    ];
    const BLAST: &[TabInfo] = &[
        TabInfo {
            category: None,
            icon_a: "minecraft:compass",
            icon_b: None,
        },
        TabInfo {
            category: Some(7),
            icon_a: "minecraft:redstone_ore",
            icon_b: None,
        },
        TabInfo {
            category: Some(8),
            icon_a: "minecraft:iron_shovel",
            icon_b: Some("minecraft:golden_leggings"),
        },
    ];
    const SMOKER: &[TabInfo] = &[
        TabInfo {
            category: None,
            icon_a: "minecraft:compass",
            icon_b: None,
        },
        TabInfo {
            category: Some(9),
            icon_a: "minecraft:porkchop",
            icon_b: None,
        },
    ];
    match kind {
        RecipeBookType::Crafting => CRAFTING,
        RecipeBookType::Furnace => FURNACE,
        RecipeBookType::BlastFurnace => BLAST,
        RecipeBookType::Smoker => SMOKER,
    }
}

fn visible_tabs(tabs: &[TabInfo], collections: &[Collection], _filtering: bool) -> Vec<TabInfo> {
    tabs.iter()
        .copied()
        .filter(|tab| {
            tab.category.is_none()
                || collections.iter().any(|collection| {
                    collection
                        .entries
                        .first()
                        .is_some_and(|entry| entry.category == tab.category.unwrap())
                        && !collection.entries.is_empty()
                })
        })
        .collect()
}

fn collections(
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
    available: &HashMap<u32, u32>,
) -> Vec<Collection> {
    // Vanilla categorizes recipes first, preserving recipe-id iteration order
    // within each category/group, then search tabs concatenate categories in
    // their declared order (not numeric registry order).
    let entries = book.known_in_vanilla_order();

    let mut grouped = Vec::<((u32, Option<u32>), Vec<RecipeBookEntry>)>::new();
    let mut grouped_index = HashMap::<(u32, u32), usize>::new();
    for entry in entries {
        if !can_display(spec, &entry.display) || !kind_accepts_category(spec.kind, entry.category) {
            continue;
        }
        if let Some(group) = entry.group {
            let key = (entry.category, group);
            if let Some(&index) = grouped_index.get(&key) {
                grouped[index].1.push(entry.clone());
            } else {
                let index = grouped.len();
                grouped_index.insert(key, index);
                grouped.push(((entry.category, Some(group)), vec![entry.clone()]));
            }
        } else {
            grouped.push(((entry.category, None), vec![entry.clone()]));
        }
    }

    grouped.sort_by_key(|((category, _), _)| category_rank(spec.kind, *category));
    grouped
        .into_iter()
        .map(|(_, entries)| {
            let craftable = entries
                .iter()
                .filter(|entry| can_craft(entry, available, &book.item_tags))
                .map(|entry| entry.id)
                .collect();
            Collection { entries, craftable }
        })
        .collect()
}

fn category_rank(kind: RecipeBookType, category: u32) -> usize {
    let order: &[u32] = match kind {
        // SearchRecipeBookCategory.CRAFTING: equipment, building blocks, misc, redstone.
        RecipeBookType::Crafting => &[2, 0, 3, 1],
        RecipeBookType::Furnace => &[4, 5, 6],
        RecipeBookType::BlastFurnace => &[7, 8],
        RecipeBookType::Smoker => &[9],
    };
    order
        .iter()
        .position(|candidate| *candidate == category)
        .unwrap_or(order.len())
}

fn filtered_collections<'a>(
    collections: &'a [Collection],
    category: Option<u32>,
    filtering: bool,
    search: &str,
    book: &RecipeBookState,
) -> Vec<&'a Collection> {
    let needle = search.to_lowercase();
    collections
        .iter()
        .filter(|collection| {
            category.is_none_or(|cat| {
                collection
                    .entries
                    .first()
                    .is_some_and(|entry| entry.category == cat)
            })
        })
        .filter(|collection| !filtering || collection.has_craftable())
        .filter(|collection| {
            needle.is_empty()
                || collection
                    .entries
                    .iter()
                    .any(|entry| entry_matches_search(entry, book, &needle))
        })
        .collect()
}

fn entry_matches_search(entry: &RecipeBookEntry, book: &RecipeBookState, needle: &str) -> bool {
    let results = display_result_stacks(&entry.display, book);
    if let Some((namespace, path)) = needle.split_once(':') {
        let namespace = namespace.trim();
        let path = path.trim();
        return results.iter().any(|stack| {
            let namespace_matches = "minecraft".contains(namespace);
            let resource_path_matches =
                crate::player::inventory::item_resource_name(stack.stack.kind)
                    .to_lowercase()
                    .contains(path);
            let tooltip_matches = normal_tooltip_search_lines(stack)
                .iter()
                .any(|line| line.to_lowercase().contains(path));
            namespace_matches && (resource_path_matches || tooltip_matches)
        });
    }
    results.iter().any(|stack| {
        normal_tooltip_search_lines(stack)
            .iter()
            .any(|line| line.to_lowercase().contains(needle))
    })
}

fn direct_trim_pattern_name(pattern: &TrimPatternHolder) -> String {
    match pattern {
        TrimPatternHolder::Direct { description, .. } => description.to_string(),
        TrimPatternHolder::Reference(id) => {
            let Some(key) = azalea_registry::data::TrimPatternKey::ALL.get(*id as usize) else {
                return format!("Trim Pattern #{id}");
            };
            let ident: azalea_registry::identifier::Identifier = key.clone().into_ident();
            let translation_key = format!("trim_pattern.minecraft.{}", ident.path());
            crate::lang::translate(&translation_key)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!("{} Armor Trim", crate::lang::title_case_snake(ident.path()))
                })
        }
    }
}

fn direct_trim_material_name(
    material: &Holder<azalea_registry::data::TrimMaterial, components::DirectTrimMaterial>,
) -> String {
    match material {
        Holder::Direct(material) => material.description.to_string(),
        Holder::Reference(material) => {
            let Some(key) =
                azalea_registry::data::TrimMaterialKey::ALL.get(material.protocol_id() as usize)
            else {
                return format!("Trim Material #{}", material.protocol_id() as usize);
            };
            let ident: azalea_registry::identifier::Identifier = key.clone().into_ident();
            let translation_key = format!("trim_material.minecraft.{}", ident.path());
            crate::lang::translate(&translation_key)
                .map(str::to_owned)
                .unwrap_or_else(|| crate::lang::title_case_snake(ident.path()))
        }
    }
}

fn stack_tooltip_lines(stack: &ResolvedSlotStack) -> Vec<TooltipLine> {
    let mut lines = match serde_json::to_value(&stack.stack) {
        Ok(value) => crate::ui::chat::item_tooltip_lines(&value, None, false),
        Err(_) => vec![TooltipLine::new(
            super::common::item_display_name(&stack.stack),
            WHITE,
        )],
    };
    if let Some(trim) = &stack.direct_trim {
        let insert_at = lines.len().min(1);
        let upgrade = crate::lang::translate("item.minecraft.smithing_template.upgrade")
            .unwrap_or("Upgrade")
            .to_owned();
        lines.insert(insert_at, TooltipLine::new(upgrade, WHITE));
        lines.insert(
            insert_at + 1,
            TooltipLine::new(
                format!(" {}", direct_trim_pattern_name(&trim.pattern)),
                WHITE,
            ),
        );
        lines.insert(
            insert_at + 2,
            TooltipLine::new(
                format!(" {}", direct_trim_material_name(&trim.material)),
                WHITE,
            ),
        );
    }
    lines
}

fn normal_tooltip_search_lines(stack: &ResolvedSlotStack) -> Vec<String> {
    stack_tooltip_lines(stack)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.text)
                .collect::<String>()
        })
        .collect()
}

fn kind_accepts_category(kind: RecipeBookType, category: u32) -> bool {
    match kind {
        RecipeBookType::Crafting => category <= 3,
        RecipeBookType::Furnace => (4..=6).contains(&category),
        RecipeBookType::BlastFurnace => (7..=8).contains(&category),
        RecipeBookType::Smoker => category == 9,
    }
}

fn can_display(spec: RecipeBookScreenSpec, display: &RecipeDisplay) -> bool {
    match (spec.kind, display) {
        (RecipeBookType::Crafting, RecipeDisplay::Shaped { width, height, .. }) => {
            *width as usize <= spec.grid_width && *height as usize <= spec.grid_height
        }
        (RecipeBookType::Crafting, RecipeDisplay::Shapeless { ingredients, .. }) => {
            ingredients.len() <= spec.grid_width * spec.grid_height
        }
        (
            RecipeBookType::Furnace | RecipeBookType::BlastFurnace | RecipeBookType::Smoker,
            RecipeDisplay::Furnace { .. },
        ) => true,
        _ => false,
    }
}

fn available_items(slots: &[ItemStack], spec: RecipeBookScreenSpec) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    match spec.craftability_source {
        CraftabilitySource::PlayerInventory => {
            // InventoryMenu: 2x2 crafting inputs plus the player's 36 inventory
            // items. Armor, offhand, and the crafting result are excluded.
            for index in (1..5).chain(9..45) {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
        CraftabilitySource::CraftingTable => {
            // CraftingMenu: 3x3 crafting inputs 1..9 and player inventory 10..45.
            for index in 1..46 {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
        CraftabilitySource::Furnace => {
            // AbstractFurnaceMenu delegates its three-slot SimpleContainer to
            // accountStack (components do not disqualify these), then adds the
            // player's inventory with accountSimpleStack.
            for index in 0..3 {
                account_stack(slots.get(index), &mut counts);
            }
            for index in 3..39 {
                account_simple_stack(slots.get(index), &mut counts);
            }
        }
    }
    counts
}

fn account_simple_stack(stack: Option<&ItemStack>, counts: &mut HashMap<u32, u32>) {
    let Some(ItemStack::Present(data)) = stack else {
        return;
    };
    if !is_usable_for_crafting(data) {
        return;
    }
    account_stack_data(data, counts);
}

fn account_stack(stack: Option<&ItemStack>, counts: &mut HashMap<u32, u32>) {
    let Some(ItemStack::Present(data)) = stack else {
        return;
    };
    account_stack_data(data, counts);
}

fn account_stack_data(data: &ItemStackData, counts: &mut HashMap<u32, u32>) {
    if data.count <= 0 {
        return;
    }
    let count = data.count.min(data.kind.max_stack_size()) as u32;
    *counts.entry(data.kind.to_u32()).or_default() += count;
}

fn is_usable_for_crafting(data: &ItemStackData) -> bool {
    let damaged = data
        .get_component::<Damage>()
        .is_some_and(|damage| damage.amount > 0);
    let enchanted = data
        .get_component::<Enchantments>()
        .is_some_and(|enchantments| !enchantments.levels.is_empty());
    let named = data.get_component::<CustomName>().is_some();
    !damaged && !enchanted && !named
}

fn can_craft_entry(
    entry: Option<&RecipeBookEntry>,
    available: &HashMap<u32, u32>,
    tags: &ItemTags,
) -> bool {
    entry.is_some_and(|entry| can_craft(entry, available, tags))
}

fn can_craft(entry: &RecipeBookEntry, available: &HashMap<u32, u32>, tags: &ItemTags) -> bool {
    let Some(requirements) = entry.crafting_requirements.as_ref() else {
        return false;
    };
    if requirements.is_empty() {
        return false;
    }
    let mut options = requirements
        .iter()
        .map(|ingredient| ingredient_items(ingredient, tags).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    if options.iter().any(Vec::is_empty) {
        return false;
    }
    options.sort_by_key(Vec::len);
    let mut remaining = available.clone();
    can_assign(&options, 0, &mut remaining)
}

fn can_assign(options: &[Vec<u32>], index: usize, remaining: &mut HashMap<u32, u32>) -> bool {
    if index == options.len() {
        return true;
    }
    for item in &options[index] {
        let Some(count) = remaining.get_mut(item) else {
            continue;
        };
        if *count == 0 {
            continue;
        }
        *count -= 1;
        if can_assign(options, index + 1, remaining) {
            *remaining.get_mut(item).expect("same item remains in map") += 1;
            return true;
        }
        *remaining.get_mut(item).expect("same item remains in map") += 1;
    }
    false
}

fn ingredient_items<'a>(
    ingredient: &'a Ingredient,
    tags: &'a ItemTags,
) -> Box<dyn Iterator<Item = u32> + 'a> {
    match ingredient {
        Ingredient::Items(items) => Box::new(items.iter().copied()),
        Ingredient::Tag(tag) => Box::new(
            tags.get(tag)
                .into_iter()
                .flat_map(|items| items.iter().copied()),
        ),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ResolvedDirectTrim {
    material: Holder<azalea_registry::data::TrimMaterial, components::DirectTrimMaterial>,
    pattern: TrimPatternHolder,
}

#[derive(Clone, Debug)]
struct ResolvedSlotStack {
    stack: ItemStackData,
    direct_trim: Option<ResolvedDirectTrim>,
}

impl ResolvedSlotStack {
    fn plain(stack: ItemStackData) -> Self {
        Self {
            stack,
            direct_trim: None,
        }
    }

    fn same_item_and_components(&self, other: &Self) -> bool {
        self.stack.is_same_item_and_components(&other.stack)
            && self.direct_trim == other.direct_trim
    }
}

fn display_result_stacks(
    display: &RecipeDisplay,
    book: &RecipeBookState,
) -> Vec<ResolvedSlotStack> {
    let result = match display {
        RecipeDisplay::Shapeless { result, .. }
        | RecipeDisplay::Shaped { result, .. }
        | RecipeDisplay::Furnace { result, .. }
        | RecipeDisplay::Stonecutter { result, .. }
        | RecipeDisplay::Smithing { result, .. } => result,
    };
    slot_stacks(result, book)
}

fn with_component<T>(stack: ItemStackData, component: T) -> ItemStackData
where
    T: DataComponentTrait + EncodableDataComponent,
{
    match ItemStack::Present(stack).with_component(component) {
        ItemStack::Present(stack) => stack,
        ItemStack::Empty => unreachable!("a positive recipe-display stack cannot become empty"),
    }
}

fn without_component<T: DataComponentTrait>(mut stack: ItemStackData) -> ItemStackData {
    // SAFETY: `None` represents an explicit removal, so there is no union value
    // whose runtime type needs to match the component kind.
    unsafe {
        stack
            .component_patch
            .unchecked_insert_component(T::KIND, None)
    };
    stack
}

fn stack_has_component_kind(stack: &ItemStackData, kind: DataComponentKind) -> bool {
    if let Some((_, value)) = stack
        .component_patch
        .iter()
        .find(|(patched_kind, _)| *patched_kind == kind)
    {
        // An explicit removal overrides an item default, while an explicit
        // value always satisfies OnlyWithComponent.
        return value.is_some();
    }

    match kind {
        DataComponentKind::CustomData => {
            get_default_component::<components::CustomData>(stack.kind).is_some()
        }
        DataComponentKind::MaxStackSize => {
            get_default_component::<components::MaxStackSize>(stack.kind).is_some()
        }
        DataComponentKind::MaxDamage => {
            get_default_component::<components::MaxDamage>(stack.kind).is_some()
        }
        DataComponentKind::Damage => {
            get_default_component::<components::Damage>(stack.kind).is_some()
        }
        DataComponentKind::Unbreakable => {
            get_default_component::<components::Unbreakable>(stack.kind).is_some()
        }
        DataComponentKind::CustomName => {
            get_default_component::<components::CustomName>(stack.kind).is_some()
        }
        DataComponentKind::ItemName => {
            get_default_component::<components::ItemName>(stack.kind).is_some()
        }
        DataComponentKind::ItemModel => {
            get_default_component::<components::ItemModel>(stack.kind).is_some()
        }
        DataComponentKind::Lore => get_default_component::<components::Lore>(stack.kind).is_some(),
        DataComponentKind::Rarity => {
            get_default_component::<components::Rarity>(stack.kind).is_some()
        }
        DataComponentKind::Enchantments => {
            get_default_component::<components::Enchantments>(stack.kind).is_some()
        }
        DataComponentKind::CanPlaceOn => {
            get_default_component::<components::CanPlaceOn>(stack.kind).is_some()
        }
        DataComponentKind::CanBreak => {
            get_default_component::<components::CanBreak>(stack.kind).is_some()
        }
        DataComponentKind::AttributeModifiers => {
            get_default_component::<components::AttributeModifiers>(stack.kind).is_some()
        }
        DataComponentKind::CustomModelData => {
            get_default_component::<components::CustomModelData>(stack.kind).is_some()
        }
        DataComponentKind::TooltipDisplay => {
            get_default_component::<components::TooltipDisplay>(stack.kind).is_some()
        }
        DataComponentKind::RepairCost => {
            get_default_component::<components::RepairCost>(stack.kind).is_some()
        }
        DataComponentKind::CreativeSlotLock => {
            get_default_component::<components::CreativeSlotLock>(stack.kind).is_some()
        }
        DataComponentKind::EnchantmentGlintOverride => {
            get_default_component::<components::EnchantmentGlintOverride>(stack.kind).is_some()
        }
        DataComponentKind::IntangibleProjectile => {
            get_default_component::<components::IntangibleProjectile>(stack.kind).is_some()
        }
        DataComponentKind::Food => get_default_component::<components::Food>(stack.kind).is_some(),
        DataComponentKind::Consumable => {
            get_default_component::<components::Consumable>(stack.kind).is_some()
        }
        DataComponentKind::UseRemainder => {
            get_default_component::<components::UseRemainder>(stack.kind).is_some()
        }
        DataComponentKind::UseCooldown => {
            get_default_component::<components::UseCooldown>(stack.kind).is_some()
        }
        DataComponentKind::DamageResistant => {
            get_default_component::<components::DamageResistant>(stack.kind).is_some()
        }
        DataComponentKind::Tool => get_default_component::<components::Tool>(stack.kind).is_some(),
        DataComponentKind::Weapon => {
            get_default_component::<components::Weapon>(stack.kind).is_some()
        }
        DataComponentKind::Enchantable => {
            get_default_component::<components::Enchantable>(stack.kind).is_some()
        }
        DataComponentKind::Equippable => {
            get_default_component::<components::Equippable>(stack.kind).is_some()
        }
        DataComponentKind::Repairable => {
            get_default_component::<components::Repairable>(stack.kind).is_some()
        }
        DataComponentKind::Glider => {
            get_default_component::<components::Glider>(stack.kind).is_some()
        }
        DataComponentKind::TooltipStyle => {
            get_default_component::<components::TooltipStyle>(stack.kind).is_some()
        }
        DataComponentKind::DeathProtection => {
            get_default_component::<components::DeathProtection>(stack.kind).is_some()
        }
        DataComponentKind::BlocksAttacks => {
            get_default_component::<components::BlocksAttacks>(stack.kind).is_some()
        }
        DataComponentKind::StoredEnchantments => {
            get_default_component::<components::StoredEnchantments>(stack.kind).is_some()
        }
        DataComponentKind::DyedColor => {
            get_default_component::<components::DyedColor>(stack.kind).is_some()
        }
        DataComponentKind::MapColor => {
            get_default_component::<components::MapColor>(stack.kind).is_some()
        }
        DataComponentKind::MapId => {
            get_default_component::<components::MapId>(stack.kind).is_some()
        }
        DataComponentKind::MapDecorations => {
            get_default_component::<components::MapDecorations>(stack.kind).is_some()
        }
        DataComponentKind::MapPostProcessing => {
            get_default_component::<components::MapPostProcessing>(stack.kind).is_some()
        }
        DataComponentKind::ChargedProjectiles => {
            get_default_component::<components::ChargedProjectiles>(stack.kind).is_some()
        }
        DataComponentKind::BundleContents => {
            get_default_component::<components::BundleContents>(stack.kind).is_some()
        }
        DataComponentKind::PotionContents => {
            get_default_component::<components::PotionContents>(stack.kind).is_some()
        }
        DataComponentKind::PotionDurationScale => {
            get_default_component::<components::PotionDurationScale>(stack.kind).is_some()
        }
        DataComponentKind::SuspiciousStewEffects => {
            get_default_component::<components::SuspiciousStewEffects>(stack.kind).is_some()
        }
        DataComponentKind::WritableBookContent => {
            get_default_component::<components::WritableBookContent>(stack.kind).is_some()
        }
        DataComponentKind::WrittenBookContent => {
            get_default_component::<components::WrittenBookContent>(stack.kind).is_some()
        }
        DataComponentKind::Trim => get_default_component::<components::Trim>(stack.kind).is_some(),
        DataComponentKind::DebugStickState => {
            get_default_component::<components::DebugStickState>(stack.kind).is_some()
        }
        DataComponentKind::EntityData => {
            get_default_component::<components::EntityData>(stack.kind).is_some()
        }
        DataComponentKind::BucketEntityData => {
            get_default_component::<components::BucketEntityData>(stack.kind).is_some()
        }
        DataComponentKind::BlockEntityData => {
            get_default_component::<components::BlockEntityData>(stack.kind).is_some()
        }
        DataComponentKind::Instrument => {
            get_default_component::<components::Instrument>(stack.kind).is_some()
        }
        DataComponentKind::ProvidesTrimMaterial => {
            get_default_component::<components::ProvidesTrimMaterial>(stack.kind).is_some()
        }
        DataComponentKind::OminousBottleAmplifier => {
            get_default_component::<components::OminousBottleAmplifier>(stack.kind).is_some()
        }
        DataComponentKind::JukeboxPlayable => {
            get_default_component::<components::JukeboxPlayable>(stack.kind).is_some()
        }
        DataComponentKind::ProvidesBannerPatterns => {
            get_default_component::<components::ProvidesBannerPatterns>(stack.kind).is_some()
        }
        DataComponentKind::Recipes => {
            get_default_component::<components::Recipes>(stack.kind).is_some()
        }
        DataComponentKind::LodestoneTracker => {
            get_default_component::<components::LodestoneTracker>(stack.kind).is_some()
        }
        DataComponentKind::FireworkExplosion => {
            get_default_component::<components::FireworkExplosion>(stack.kind).is_some()
        }
        DataComponentKind::Fireworks => {
            get_default_component::<components::Fireworks>(stack.kind).is_some()
        }
        DataComponentKind::Profile => {
            get_default_component::<components::Profile>(stack.kind).is_some()
        }
        DataComponentKind::NoteBlockSound => {
            get_default_component::<components::NoteBlockSound>(stack.kind).is_some()
        }
        DataComponentKind::BannerPatterns => {
            get_default_component::<components::BannerPatterns>(stack.kind).is_some()
        }
        DataComponentKind::BaseColor => {
            get_default_component::<components::BaseColor>(stack.kind).is_some()
        }
        DataComponentKind::PotDecorations => {
            get_default_component::<components::PotDecorations>(stack.kind).is_some()
        }
        DataComponentKind::Container => {
            get_default_component::<components::Container>(stack.kind).is_some()
        }
        DataComponentKind::BlockState => {
            get_default_component::<components::BlockState>(stack.kind).is_some()
        }
        DataComponentKind::Bees => get_default_component::<components::Bees>(stack.kind).is_some(),
        DataComponentKind::Lock => get_default_component::<components::Lock>(stack.kind).is_some(),
        DataComponentKind::ContainerLoot => {
            get_default_component::<components::ContainerLoot>(stack.kind).is_some()
        }
        DataComponentKind::BreakSound => {
            get_default_component::<components::BreakSound>(stack.kind).is_some()
        }
        DataComponentKind::VillagerVariant => {
            get_default_component::<components::VillagerVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfVariant => {
            get_default_component::<components::WolfVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfSoundVariant => {
            get_default_component::<components::WolfSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::WolfCollar => {
            get_default_component::<components::WolfCollar>(stack.kind).is_some()
        }
        DataComponentKind::FoxVariant => {
            get_default_component::<components::FoxVariant>(stack.kind).is_some()
        }
        DataComponentKind::SalmonSize => {
            get_default_component::<components::SalmonSize>(stack.kind).is_some()
        }
        DataComponentKind::ParrotVariant => {
            get_default_component::<components::ParrotVariant>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishPattern => {
            get_default_component::<components::TropicalFishPattern>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishBaseColor => {
            get_default_component::<components::TropicalFishBaseColor>(stack.kind).is_some()
        }
        DataComponentKind::TropicalFishPatternColor => {
            get_default_component::<components::TropicalFishPatternColor>(stack.kind).is_some()
        }
        DataComponentKind::MooshroomVariant => {
            get_default_component::<components::MooshroomVariant>(stack.kind).is_some()
        }
        DataComponentKind::RabbitVariant => {
            get_default_component::<components::RabbitVariant>(stack.kind).is_some()
        }
        DataComponentKind::PigVariant => {
            get_default_component::<components::PigVariant>(stack.kind).is_some()
        }
        DataComponentKind::CowVariant => {
            get_default_component::<components::CowVariant>(stack.kind).is_some()
        }
        DataComponentKind::ChickenVariant => {
            get_default_component::<components::ChickenVariant>(stack.kind).is_some()
        }
        DataComponentKind::FrogVariant => {
            get_default_component::<components::FrogVariant>(stack.kind).is_some()
        }
        DataComponentKind::HorseVariant => {
            get_default_component::<components::HorseVariant>(stack.kind).is_some()
        }
        DataComponentKind::PaintingVariant => {
            get_default_component::<components::PaintingVariant>(stack.kind).is_some()
        }
        DataComponentKind::LlamaVariant => {
            get_default_component::<components::LlamaVariant>(stack.kind).is_some()
        }
        DataComponentKind::AxolotlVariant => {
            get_default_component::<components::AxolotlVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatVariant => {
            get_default_component::<components::CatVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatCollar => {
            get_default_component::<components::CatCollar>(stack.kind).is_some()
        }
        DataComponentKind::SheepColor => {
            get_default_component::<components::SheepColor>(stack.kind).is_some()
        }
        DataComponentKind::ShulkerColor => {
            get_default_component::<components::ShulkerColor>(stack.kind).is_some()
        }
        DataComponentKind::UseEffects => {
            get_default_component::<components::UseEffects>(stack.kind).is_some()
        }
        DataComponentKind::MinimumAttackCharge => {
            get_default_component::<components::MinimumAttackCharge>(stack.kind).is_some()
        }
        DataComponentKind::DamageType => {
            get_default_component::<components::DamageType>(stack.kind).is_some()
        }
        DataComponentKind::PiercingWeapon => {
            get_default_component::<components::PiercingWeapon>(stack.kind).is_some()
        }
        DataComponentKind::KineticWeapon => {
            get_default_component::<components::KineticWeapon>(stack.kind).is_some()
        }
        DataComponentKind::SwingAnimation => {
            get_default_component::<components::SwingAnimation>(stack.kind).is_some()
        }
        DataComponentKind::ZombieNautilusVariant => {
            get_default_component::<components::ZombieNautilusVariant>(stack.kind).is_some()
        }
        DataComponentKind::AttackRange => {
            get_default_component::<components::AttackRange>(stack.kind).is_some()
        }
        DataComponentKind::AdditionalTradeCost => {
            get_default_component::<components::AdditionalTradeCost>(stack.kind).is_some()
        }
        DataComponentKind::Dye => get_default_component::<components::Dye>(stack.kind).is_some(),
        DataComponentKind::PigSoundVariant => {
            get_default_component::<components::PigSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::CowSoundVariant => {
            get_default_component::<components::CowSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::ChickenSoundVariant => {
            get_default_component::<components::ChickenSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::CatSoundVariant => {
            get_default_component::<components::CatSoundVariant>(stack.kind).is_some()
        }
        DataComponentKind::SulfurCubeContent => {
            get_default_component::<components::SulfurCubeContent>(stack.kind).is_some()
        }
    }
}

fn dye_texture_rgb(color: DyeColor) -> i32 {
    match color {
        DyeColor::White => 0xF9FFFE,
        DyeColor::Orange => 16_351_261,
        DyeColor::Magenta => 13_061_821,
        DyeColor::LightBlue => 3_847_130,
        DyeColor::Yellow => 16_701_501,
        DyeColor::Lime => 8_439_583,
        DyeColor::Pink => 15_961_002,
        DyeColor::Gray => 4_673_362,
        DyeColor::LightGray => 0x9D9D97,
        DyeColor::Cyan => 1_481_884,
        DyeColor::Purple => 8_991_416,
        DyeColor::Blue => 3_949_738,
        DyeColor::Brown => 8_606_770,
        DyeColor::Green => 6_192_150,
        DyeColor::Red => 11_546_150,
        DyeColor::Black => 0x1D1D21,
    }
}

fn rgb_channels(rgb: i32) -> (i32, i32, i32) {
    ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff)
}

fn apply_dye(mut target: ResolvedSlotStack, dye: DyeColor) -> ResolvedSlotStack {
    let mut red_total = 0;
    let mut green_total = 0;
    let mut blue_total = 0;
    let mut intensity_total = 0;
    let mut color_count = 0;

    if let Some(existing) = target.stack.get_component::<DyedColor>() {
        let (red, green, blue) = rgb_channels(existing.rgb);
        intensity_total += red.max(green).max(blue);
        red_total += red;
        green_total += green;
        blue_total += blue;
        color_count += 1;
    }

    let (red, green, blue) = rgb_channels(dye_texture_rgb(dye));
    intensity_total += red.max(green).max(blue);
    red_total += red;
    green_total += green;
    blue_total += blue;
    color_count += 1;

    let mut red = red_total / color_count;
    let mut green = green_total / color_count;
    let mut blue = blue_total / color_count;
    let average_intensity = intensity_total as f32 / color_count as f32;
    let result_intensity = red.max(green).max(blue) as f32;
    if result_intensity > 0.0 {
        red = (red as f32 * average_intensity / result_intensity) as i32;
        green = (green as f32 * average_intensity / result_intensity) as i32;
        blue = (blue as f32 * average_intensity / result_intensity) as i32;
    }

    target.stack.count = 1;
    target.stack = with_component(
        target.stack,
        DyedColor {
            rgb: (red << 16) | (green << 8) | blue,
        },
    );
    target
}

fn apply_trim(
    mut base: ResolvedSlotStack,
    material: &ResolvedSlotStack,
    pattern: &TrimPatternHolder,
) -> Option<ResolvedSlotStack> {
    let material = material.stack.get_component::<ProvidesTrimMaterial>()?;
    let resolved = ResolvedDirectTrim {
        material: material.value.clone(),
        pattern: pattern.clone(),
    };

    if let (Holder::Reference(material), TrimPatternHolder::Reference(pattern)) =
        (&resolved.material, &resolved.pattern)
    {
        let trim = Trim {
            material: *material,
            pattern: azalea_registry::data::TrimPattern::new_raw(*pattern),
        };
        if base.direct_trim.is_none()
            && base.stack.get_component::<Trim>().as_deref() == Some(&trim)
        {
            return None;
        }
        base.stack.count = 1;
        base.stack = with_component(base.stack, trim);
        base.direct_trim = None;
        return Some(base);
    }

    if base.direct_trim.as_ref() == Some(&resolved) {
        return None;
    }
    base.stack.count = 1;
    base.stack = without_component::<Trim>(base.stack);
    base.direct_trim = Some(resolved);
    Some(base)
}

#[derive(Clone, Copy)]
struct VanillaDemoRandom {
    seed: u64,
}

impl VanillaDemoRandom {
    fn from_identity(identity: usize) -> Self {
        let identity = identity as u32 as i32 as i64 as u64;
        Self {
            seed: (identity ^ 0x5DEE_CE66D) & 0xFFFF_FFFF_FFFF,
        }
    }

    fn next_bits(&mut self, bits: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(25_214_903_917).wrapping_add(11) & 0xFFFF_FFFF_FFFF;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        let bound = bound as i32;
        if bound & (bound - 1) == 0 {
            return (((bound as i64) * (self.next_bits(31) as i64)) >> 31) as usize;
        }
        loop {
            let sample = self.next_bits(31) as i32;
            let modulo = sample % bound;
            if sample
                .wrapping_sub(modulo)
                .wrapping_add(bound.wrapping_sub(1))
                >= 0
            {
                return modulo as usize;
            }
        }
    }
}

fn slot_stacks(slot: &SlotDisplay, book: &RecipeBookState) -> Vec<ResolvedSlotStack> {
    let basic = |id: u32| {
        ItemKind::from_u32(id).map(|kind| ResolvedSlotStack::plain(ItemStackData::new(kind, 1)))
    };
    match slot {
        SlotDisplay::Empty => Vec::new(),
        SlotDisplay::AnyFuel => vanilla_fuel_items(&book.item_tags)
            .into_iter()
            .filter_map(basic)
            .collect(),
        SlotDisplay::Item(id) => basic(*id).into_iter().collect(),
        SlotDisplay::ItemStack(template) => ItemKind::from_u32(template.item)
            .map(|kind| {
                ResolvedSlotStack::plain(ItemStackData {
                    kind,
                    count: template.count,
                    component_patch: template.components.clone(),
                })
            })
            .into_iter()
            .collect(),
        SlotDisplay::Tag(tag) => book
            .item_tags
            .ordered(tag)
            .into_iter()
            .flatten()
            .copied()
            .filter_map(basic)
            .collect(),
        SlotDisplay::Composite(parts) => parts
            .iter()
            .flat_map(|part| slot_stacks(part, book))
            .collect(),
        SlotDisplay::WithRemainder { input, .. } => slot_stacks(input, book),
        SlotDisplay::WithAnyPotion(inner) => {
            let base = slot_stacks(inner, book);
            let mut out = Vec::new();
            let mut id = 0;
            while let Some(potion) = Potion::from_u32(id) {
                for stack in &base {
                    let mut stack = stack.clone();
                    stack.stack = with_component(
                        stack.stack,
                        PotionContents {
                            potion: Some(potion),
                            ..PotionContents::default()
                        },
                    );
                    out.push(stack);
                }
                id += 1;
            }
            out
        }
        SlotDisplay::OnlyWithComponent {
            contents,
            component,
        } => {
            let Some(kind) = DataComponentKind::from_u32(*component) else {
                return Vec::new();
            };
            slot_stacks(contents, book)
                .into_iter()
                .filter(|stack| stack_has_component_kind(&stack.stack, kind))
                .collect()
        }
        SlotDisplay::Dyed { dye, target } => {
            let targets = slot_stacks(target, book);
            let dyes = slot_stacks(dye, book);
            let mut out = Vec::new();
            for index in 0..targets.len().saturating_mul(dyes.len()) {
                if out.len() == 16 {
                    break;
                }
                let target = targets[index % targets.len()].clone();
                let dye = &dyes[index / targets.len()];
                let dye = dye
                    .stack
                    .get_component::<Dye>()
                    .map_or(DyeColor::White, |component| component.color);
                out.push(apply_dye(target, dye));
            }
            out
        }
        SlotDisplay::SmithingTrim {
            base,
            material,
            trim_pattern,
        } => {
            let bases = slot_stacks(base, book);
            let materials = slot_stacks(material, book);
            if bases.is_empty() || materials.is_empty() {
                return Vec::new();
            }
            let mut random = VanillaDemoRandom::from_identity(slot as *const SlotDisplay as usize);
            let mut out = Vec::new();
            for _ in 0..256 {
                if out.len() == 16 {
                    break;
                }
                let base = &bases[random.next_int(bases.len())];
                let material = &materials[random.next_int(materials.len())];
                if let Some(trimmed) = apply_trim(base.clone(), material, trim_pattern) {
                    out.push(trimmed);
                }
            }
            out
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_ghost_recipe(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
    main: (f32, f32, f32, f32, f32),
    cursor: (f32, f32),
    slots: &[ItemStack],
    screen_w: f32,
    screen_h: f32,
) {
    let Some(ghost) = book
        .ghost_recipe
        .as_ref()
        .filter(|ghost| ghost.container_id == spec.container_id)
    else {
        return;
    };

    let scale = main.4;
    let cycle = state.cycle_index();
    match (&ghost.recipe, spec.kind) {
        (
            RecipeDisplay::Shaped {
                width,
                height,
                ingredients,
                result,
                ..
            },
            RecipeBookType::Crafting,
        ) => {
            let (grid_x, grid_y, result_x, result_y) = crafting_geometry(spec);
            render_ghost_slot(
                elements,
                result,
                book,
                cycle,
                main.0 + result_x * scale,
                main.1 + result_y * scale,
                scale,
                spec.big_result_slot,
                cursor,
                screen_w,
                screen_h,
            );
            for (grid_index, ingredient) in centered_shaped_slots(
                spec.grid_width,
                spec.grid_height,
                *width as usize,
                *height as usize,
                ingredients,
            ) {
                let col = grid_index % spec.grid_width;
                let row = grid_index / spec.grid_width;
                render_ghost_slot(
                    elements,
                    ingredient,
                    book,
                    cycle,
                    main.0 + (grid_x + col as f32 * 18.0) * scale,
                    main.1 + (grid_y + row as f32 * 18.0) * scale,
                    scale,
                    false,
                    cursor,
                    screen_w,
                    screen_h,
                );
            }
        }
        (
            RecipeDisplay::Shapeless {
                ingredients,
                result,
                ..
            },
            RecipeBookType::Crafting,
        ) => {
            let (grid_x, grid_y, result_x, result_y) = crafting_geometry(spec);
            render_ghost_slot(
                elements,
                result,
                book,
                cycle,
                main.0 + result_x * scale,
                main.1 + result_y * scale,
                scale,
                spec.big_result_slot,
                cursor,
                screen_w,
                screen_h,
            );
            for (index, ingredient) in ingredients
                .iter()
                .take(spec.grid_width * spec.grid_height)
                .enumerate()
            {
                let col = index % spec.grid_width;
                let row = index / spec.grid_width;
                render_ghost_slot(
                    elements,
                    ingredient,
                    book,
                    cycle,
                    main.0 + (grid_x + col as f32 * 18.0) * scale,
                    main.1 + (grid_y + row as f32 * 18.0) * scale,
                    scale,
                    false,
                    cursor,
                    screen_w,
                    screen_h,
                );
            }
        }
        (
            RecipeDisplay::Furnace {
                ingredient,
                fuel,
                result,
                ..
            },
            RecipeBookType::Furnace | RecipeBookType::BlastFurnace | RecipeBookType::Smoker,
        ) => {
            render_ghost_slot(
                elements,
                result,
                book,
                cycle,
                main.0 + 116.0 * scale,
                main.1 + 35.0 * scale,
                scale,
                spec.big_result_slot,
                cursor,
                screen_w,
                screen_h,
            );
            render_ghost_slot(
                elements,
                ingredient,
                book,
                cycle,
                main.0 + 56.0 * scale,
                main.1 + 17.0 * scale,
                scale,
                false,
                cursor,
                screen_w,
                screen_h,
            );
            if slots.get(1).is_none_or(ItemStack::is_empty) {
                render_ghost_slot(
                    elements,
                    fuel,
                    book,
                    cycle,
                    main.0 + 56.0 * scale,
                    main.1 + 53.0 * scale,
                    scale,
                    false,
                    cursor,
                    screen_w,
                    screen_h,
                );
            }
        }
        _ => {}
    }
}

fn crafting_geometry(spec: RecipeBookScreenSpec) -> (f32, f32, f32, f32) {
    if spec.grid_width == 2 {
        (98.0, 18.0, 154.0, 28.0)
    } else {
        (30.0, 17.0, 124.0, 35.0)
    }
}

fn centered_shaped_slots(
    grid_width: usize,
    grid_height: usize,
    recipe_width: usize,
    recipe_height: usize,
    ingredients: &[SlotDisplay],
) -> Vec<(usize, &SlotDisplay)> {
    let mut out = Vec::new();
    let mut iter = ingredients.iter();
    let mut grid_index = 0usize;
    let mut grid_y = 0usize;
    while grid_y < grid_height {
        let center_y = (recipe_height as f32) < grid_height as f32 / 2.0;
        let start_y = (grid_height as f32 / 2.0 - recipe_height as f32 / 2.0).floor() as usize;
        if center_y && start_y > grid_y {
            grid_index += grid_width;
            grid_y += 1;
        }
        if grid_y >= grid_height {
            break;
        }
        let mut grid_x = 0usize;
        while grid_x < grid_width {
            let center_x = (recipe_width as f32) < grid_width as f32 / 2.0;
            let start_x = (grid_width as f32 / 2.0 - recipe_width as f32 / 2.0).floor() as usize;
            let mut total_width = recipe_width;
            let mut add = grid_x < recipe_width;
            if center_x {
                total_width = start_x + recipe_width;
                add = start_x <= grid_x && grid_x < total_width;
            }
            if add {
                let Some(ingredient) = iter.next() else {
                    return out;
                };
                out.push((grid_index, ingredient));
            } else if total_width == grid_x {
                grid_index += grid_width - grid_x;
                break;
            }
            grid_index += 1;
            grid_x += 1;
        }
        grid_y += 1;
    }
    out
}

fn ghost_highlight_rect(x: f32, y: f32, scale: f32, big_result: bool) -> [f32; 4] {
    if big_result {
        [x - 4.0 * scale, y - 4.0 * scale, 24.0 * scale, 24.0 * scale]
    } else {
        [x, y, 16.0 * scale, 16.0 * scale]
    }
}

#[allow(clippy::too_many_arguments)]
fn render_ghost_slot(
    elements: &mut Vec<MenuElement>,
    slot: &SlotDisplay,
    book: &RecipeBookState,
    cycle: usize,
    x: f32,
    y: f32,
    scale: f32,
    big_result: bool,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let items = slot_stacks(slot, book);
    if items.is_empty() {
        return;
    }
    let item = &items[cycle % items.len()];
    let red = ghost_highlight_rect(x, y, scale, big_result);
    elements.push(MenuElement::Rect {
        x: red[0],
        y: red[1],
        w: red[2],
        h: red[3],
        corner_radius: 0.0,
        color: [1.0, 0.0, 0.0, 0.19],
    });
    push_book_stack(elements, x, y, 16.0 * scale, item);
    elements.push(MenuElement::Rect {
        x,
        y,
        w: 16.0 * scale,
        h: 16.0 * scale,
        corner_radius: 0.0,
        color: [1.0, 1.0, 1.0, 0.19],
    });
    if hit_test(cursor, [x, y, 16.0 * scale, 16.0 * scale]) {
        push_tooltip_lines(
            elements,
            cursor,
            screen_w,
            screen_h,
            scale,
            stack_tooltip_lines(item),
        );
    }
}

fn vanilla_fuel_items(tags: &ItemTags) -> Vec<u32> {
    use azalea_registry::builtin::ItemKind as I;

    fn push_item(out: &mut Vec<u32>, seen: &mut HashSet<u32>, item: I) {
        let id = item.to_u32();
        if seen.insert(id) {
            out.push(id);
        }
    }

    fn push_tag(out: &mut Vec<u32>, seen: &mut HashSet<u32>, tags: &ItemTags, tag: &str) {
        if let Some(items) = tags.ordered(tag) {
            for &id in items {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    push_item(&mut out, &mut seen, I::LavaBucket);
    push_item(&mut out, &mut seen, I::CoalBlock);
    push_item(&mut out, &mut seen, I::BlazeRod);
    push_item(&mut out, &mut seen, I::Coal);
    push_item(&mut out, &mut seen, I::Charcoal);
    push_tag(&mut out, &mut seen, tags, "minecraft:logs");
    push_tag(&mut out, &mut seen, tags, "minecraft:bamboo_blocks");
    push_tag(&mut out, &mut seen, tags, "minecraft:planks");
    push_item(&mut out, &mut seen, I::BambooMosaic);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_stairs");
    push_item(&mut out, &mut seen, I::BambooMosaicStairs);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_slabs");
    push_item(&mut out, &mut seen, I::BambooMosaicSlab);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_trapdoors");
    push_tag(
        &mut out,
        &mut seen,
        tags,
        "minecraft:wooden_pressure_plates",
    );
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_shelves");
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_fences");
    push_tag(&mut out, &mut seen, tags, "minecraft:fence_gates");
    push_item(&mut out, &mut seen, I::NoteBlock);
    push_item(&mut out, &mut seen, I::Bookshelf);
    push_item(&mut out, &mut seen, I::ChiseledBookshelf);
    push_item(&mut out, &mut seen, I::Lectern);
    push_item(&mut out, &mut seen, I::Jukebox);
    push_item(&mut out, &mut seen, I::Chest);
    push_item(&mut out, &mut seen, I::TrappedChest);
    push_item(&mut out, &mut seen, I::CraftingTable);
    push_item(&mut out, &mut seen, I::DaylightDetector);
    push_tag(&mut out, &mut seen, tags, "minecraft:banners");
    push_item(&mut out, &mut seen, I::Bow);
    push_item(&mut out, &mut seen, I::FishingRod);
    push_item(&mut out, &mut seen, I::Ladder);
    push_tag(&mut out, &mut seen, tags, "minecraft:signs");
    push_tag(&mut out, &mut seen, tags, "minecraft:hanging_signs");
    push_item(&mut out, &mut seen, I::WoodenShovel);
    push_item(&mut out, &mut seen, I::WoodenSword);
    push_item(&mut out, &mut seen, I::WoodenSpear);
    push_item(&mut out, &mut seen, I::WoodenHoe);
    push_item(&mut out, &mut seen, I::WoodenAxe);
    push_item(&mut out, &mut seen, I::WoodenPickaxe);
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_doors");
    push_tag(&mut out, &mut seen, tags, "minecraft:boats");
    push_tag(&mut out, &mut seen, tags, "minecraft:wool");
    push_tag(&mut out, &mut seen, tags, "minecraft:wooden_buttons");
    push_item(&mut out, &mut seen, I::Stick);
    push_tag(&mut out, &mut seen, tags, "minecraft:saplings");
    push_item(&mut out, &mut seen, I::Bowl);
    push_tag(&mut out, &mut seen, tags, "minecraft:wool_carpets");
    push_item(&mut out, &mut seen, I::DriedKelpBlock);
    push_item(&mut out, &mut seen, I::Crossbow);
    push_item(&mut out, &mut seen, I::Bamboo);
    push_item(&mut out, &mut seen, I::DeadBush);
    push_item(&mut out, &mut seen, I::ShortDryGrass);
    push_item(&mut out, &mut seen, I::TallDryGrass);
    push_item(&mut out, &mut seen, I::Scaffolding);
    push_item(&mut out, &mut seen, I::Loom);
    push_item(&mut out, &mut seen, I::Barrel);
    push_item(&mut out, &mut seen, I::CartographyTable);
    push_item(&mut out, &mut seen, I::FletchingTable);
    push_item(&mut out, &mut seen, I::SmithingTable);
    push_item(&mut out, &mut seen, I::Composter);
    push_item(&mut out, &mut seen, I::Azalea);
    push_item(&mut out, &mut seen, I::FloweringAzalea);
    push_item(&mut out, &mut seen, I::MangroveRoots);
    push_item(&mut out, &mut seen, I::LeafLitter);

    if let Some(non_flammable) = tags.get("minecraft:non_flammable_wood") {
        out.retain(|id| !non_flammable.contains(id));
    }
    out
}

fn display_item(
    display: &RecipeDisplay,
    book: &RecipeBookState,
    cycle: usize,
) -> Option<ResolvedSlotStack> {
    let items = display_result_stacks(display, book);
    (!items.is_empty()).then(|| items[cycle % items.len()].clone())
}

fn all_recipes_have_same_result(entries: &[&RecipeBookEntry], book: &RecipeBookState) -> bool {
    let mut items = entries
        .iter()
        .flat_map(|entry| display_result_stacks(&entry.display, book));
    let Some(first) = items.next() else {
        return true;
    };
    items.all(|item| first.same_item_and_components(&item))
}

fn push_book_stack(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    stack: &ResolvedSlotStack,
) {
    elements.push(MenuElement::ItemIcon {
        x,
        y,
        w: size,
        h: size,
        item_name: crate::player::inventory::item_resource_name(stack.stack.kind),
        tint: WHITE,
    });
}

#[allow(clippy::too_many_arguments)]
fn render_search(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    bx: f32,
    by: f32,
    scale: f32,
    cursor: (f32, f32),
    _screen_w: f32,
    _screen_h: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let x = bx + SEARCH_X * scale;
    let y = by + SEARCH_Y * scale;
    let w = SEARCH_W * scale;
    let h = SEARCH_H * scale;
    let fs = FONT_SIZE * scale;
    let text_x = x + 4.0 * scale;
    let text_y = y + 3.0 * scale;
    let inner_w = (SEARCH_W - 8.0) * scale;
    let wf = |s: &str| text_width_fn(s, fs);

    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: if state.search_focused || hit_test(cursor, [x, y, w, h]) {
            SpriteId::WidgetTextFieldHighlighted
        } else {
            SpriteId::WidgetTextField
        },
        border: scale,
        tint: WHITE,
    });

    let info = state.search.render_info(inner_w, state.search_focused, &wf);
    let shown = &state.search.value()[info.display_start..info.display_end];
    if shown.is_empty() && !state.search_focused {
        let mut hint = crate::ui::text::TextSpan::new(
            "Search...".into(),
            [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0, 1.0],
        );
        hint.italic = true;
        elements.push(MenuElement::TextSpans {
            x: text_x,
            y: text_y,
            spans: vec![hint],
            scale: fs,
            centered: false,
        });
    } else {
        super::common::push_field_text(
            elements, &info, shown, None, text_x, text_y, fs, scale, scale, WHITE, None, &wf,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_filter(
    elements: &mut Vec<MenuElement>,
    book: &RecipeBookState,
    kind: RecipeBookType,
    bx: f32,
    by: f32,
    scale: f32,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let rect = [
        bx + FILTER_X * scale,
        by + FILTER_Y * scale,
        FILTER_W * scale,
        FILTER_H * scale,
    ];
    let filtering = book.settings.get(kind).filtering;
    let hovered = hit_test(cursor, rect);
    let furnace = kind != RecipeBookType::Crafting;
    let sprite = match (furnace, filtering, hovered) {
        (false, true, false) => SpriteId::RecipeBookFilterEnabled,
        (false, false, false) => SpriteId::RecipeBookFilterDisabled,
        (false, true, true) => SpriteId::RecipeBookFilterEnabledHighlighted,
        (false, false, true) => SpriteId::RecipeBookFilterDisabledHighlighted,
        (true, true, false) => SpriteId::RecipeBookFurnaceFilterEnabled,
        (true, false, false) => SpriteId::RecipeBookFurnaceFilterDisabled,
        (true, true, true) => SpriteId::RecipeBookFurnaceFilterEnabledHighlighted,
        (true, false, true) => SpriteId::RecipeBookFurnaceFilterDisabledHighlighted,
    };
    elements.push(MenuElement::Image {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite,
        tint: WHITE,
    });
    if hovered {
        let text = if filtering {
            match kind {
                RecipeBookType::Crafting => "Showing Craftable",
                RecipeBookType::Furnace => "Showing Smeltable",
                RecipeBookType::BlastFurnace => "Showing Blastable",
                RecipeBookType::Smoker => "Showing Smokable",
            }
        } else {
            "Showing All"
        };
        push_tooltip(elements, cursor, screen_w, screen_h, scale, text);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tabs(
    elements: &mut Vec<MenuElement>,
    state: &mut RecipeBookUiState,
    book: &RecipeBookState,
    tabs: &[TabInfo],
    collections: &[Collection],
    bx: f32,
    by: f32,
    scale: f32,
    _cursor: (f32, f32),
) {
    let filtering = state
        .screen
        .map(|(kind, _)| book.settings.get(kind).filtering)
        .unwrap_or(false);
    let now = Instant::now();
    let mut y = by + 3.0 * scale;
    for (index, tab) in tabs.iter().enumerate() {
        let selected = index == state.selected_tab;
        let highlighted = collections
            .iter()
            .filter(|collection| {
                tab.category.is_none_or(|category| {
                    collection
                        .entries
                        .first()
                        .is_some_and(|entry| entry.category == category)
                })
            })
            .flat_map(|collection| {
                collection.entries.iter().filter_map(|entry| {
                    (book.highlight.contains(&entry.id)
                        && (!filtering || collection.craftable.contains(&entry.id)))
                    .then_some(entry.id)
                })
            })
            .collect::<HashSet<_>>();
        let animation_key = tab.category.map_or(-1, |category| category as i32);
        let squeeze = tab_animation_squeeze(state, animation_key, highlighted, now);

        let base_x = bx - 30.0 * scale;
        let x = base_x - if selected { 2.0 * scale } else { 0.0 };
        let pivot_x = base_x + 8.0 * scale;
        let pivot_y = y + 12.0 * scale;
        let tab_rect = scale_rect_about(
            [x, y, 35.0 * scale, 27.0 * scale],
            (pivot_x, pivot_y),
            (1.0, squeeze),
        );
        elements.push(MenuElement::Image {
            x: tab_rect[0],
            y: tab_rect[1],
            w: tab_rect[2],
            h: tab_rect[3],
            sprite: if selected {
                SpriteId::RecipeBookTabSelected
            } else {
                SpriteId::RecipeBookTab
            },
            tint: WHITE,
        });
        let icon_offset = if selected { -2.0 } else { 0.0 };
        let icon_y = pivot_y + (y + 5.0 * scale - pivot_y) * squeeze;
        let icon_h = 16.0 * scale * squeeze;
        if let Some(icon_b) = tab.icon_b {
            elements.push(MenuElement::ItemIcon {
                x: bx + (-27.0 + icon_offset) * scale,
                y: icon_y,
                w: 16.0 * scale,
                h: icon_h,
                item_name: tab
                    .icon_a
                    .strip_prefix("minecraft:")
                    .unwrap_or(tab.icon_a)
                    .into(),
                tint: WHITE,
            });
            elements.push(MenuElement::ItemIcon {
                x: bx + (-16.0 + icon_offset) * scale,
                y: icon_y,
                w: 16.0 * scale,
                h: icon_h,
                item_name: icon_b.strip_prefix("minecraft:").unwrap_or(icon_b).into(),
                tint: WHITE,
            });
        } else {
            elements.push(MenuElement::ItemIcon {
                x: bx + (-21.0 + icon_offset) * scale,
                y: icon_y,
                w: 16.0 * scale,
                h: icon_h,
                item_name: tab
                    .icon_a
                    .strip_prefix("minecraft:")
                    .unwrap_or(tab.icon_a)
                    .into(),
                tint: WHITE,
            });
        }
        y += 27.0 * scale;
    }
}

fn overlay_rect_for(overlay: &RecipeOverlay, bx: f32, by: f32, scale: f32) -> [f32; 4] {
    let total = overlay.ids.len();
    let max_row = if total <= 16 { 4 } else { 5 };
    let cols = total.min(max_row).max(1);
    let rows = total.div_ceil(max_row).max(1);

    // OverlayRecipeComponent::init anchors to the clicked 25px recipe button,
    // then nudges by whole button widths to keep the popup inside the book's
    // vanilla bounds around its center.
    let mut x = 11.0 + 25.0 * (overlay.button_index % 5) as f32;
    let mut y = 31.0 + 25.0 * (overlay.button_index / 5) as f32;
    let center_x = BOOK_W / 2.0;
    let center_y = 13.0 + BOOK_H / 2.0;
    let right = x + cols as f32 * 25.0;
    let max_left = center_x + 50.0;
    if right > max_left {
        x -= 25.0 * ((right - max_left) / 25.0).floor();
    }
    let bottom = y + rows as f32 * 25.0;
    let max_bottom = center_y + 50.0;
    if bottom > max_bottom {
        y -= 25.0 * ((bottom - max_bottom) / 25.0).ceil();
    }
    let min_top = center_y - 100.0;
    if y < min_top {
        y -= 25.0 * ((y - min_top) / 25.0).ceil();
    }

    [
        bx + x * scale,
        by + y * scale,
        (cols * 25 + 8) as f32 * scale,
        (rows * 25 + 8) as f32 * scale,
    ]
}

fn hit_overlay_recipe(
    cursor: (f32, f32),
    overlay: &RecipeOverlay,
    rect: [f32; 4],
    scale: f32,
) -> Option<RecipeDisplayId> {
    let max_row = if overlay.ids.len() <= 16 { 4 } else { 5 };
    overlay.ids.iter().enumerate().find_map(|(index, id)| {
        let x = rect[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale;
        let y = rect[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale;
        hit_test(cursor, [x, y, 24.0 * scale, 24.0 * scale]).then_some(*id)
    })
}

#[allow(clippy::too_many_arguments)]
fn render_overlay(
    elements: &mut Vec<MenuElement>,
    overlay: &RecipeOverlay,
    book: &RecipeBookState,
    bx: f32,
    by: f32,
    scale: f32,
    cursor: (f32, f32),
    cycle: usize,
) {
    let rect = overlay_rect_for(overlay, bx, by, scale);
    elements.push(MenuElement::NineSlice {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: SpriteId::RecipeBookOverlay,
        border: 4.0 * scale,
        tint: WHITE,
    });
    let max_row = if overlay.ids.len() <= 16 { 4 } else { 5 };
    for (index, id) in overlay.ids.iter().enumerate() {
        let Some(entry) = book.known.get(id) else {
            continue;
        };
        let x = rect[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale;
        let y = rect[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale;
        let hovered = hit_test(cursor, [x, y, 24.0 * scale, 24.0 * scale]);
        let craftable = index < overlay.craftable_count;
        let furnace = matches!(entry.display, RecipeDisplay::Furnace { .. });
        let sprite = match (furnace, craftable, hovered) {
            (false, true, false) => SpriteId::RecipeBookCraftingOverlay,
            (false, true, true) => SpriteId::RecipeBookCraftingOverlayHighlighted,
            (false, false, false) => SpriteId::RecipeBookCraftingOverlayDisabled,
            (false, false, true) => SpriteId::RecipeBookCraftingOverlayDisabledHighlighted,
            (true, true, false) => SpriteId::RecipeBookFurnaceOverlay,
            (true, true, true) => SpriteId::RecipeBookFurnaceOverlayHighlighted,
            (true, false, false) => SpriteId::RecipeBookFurnaceOverlayDisabled,
            (true, false, true) => SpriteId::RecipeBookFurnaceOverlayDisabledHighlighted,
        };
        elements.push(MenuElement::Image {
            x,
            y,
            w: 24.0 * scale,
            h: 24.0 * scale,
            sprite,
            tint: WHITE,
        });
        render_overlay_ingredients(elements, entry, book, x, y, scale, cycle);
    }
}

fn render_overlay_ingredients(
    elements: &mut Vec<MenuElement>,
    entry: &RecipeBookEntry,
    book: &RecipeBookState,
    x: f32,
    y: f32,
    scale: f32,
    cycle: usize,
) {
    let mut draw = |grid_x: usize, grid_y: usize, slot: &SlotDisplay| {
        let items = slot_stacks(slot, book);
        if items.is_empty() {
            return;
        }
        let item = &items[cycle % items.len()];
        // Vanilla scales a normal 16px item by 0.375 around the ingredient
        // grid position. This resolves to a 6px icon at button + 2 + 7*grid.
        push_book_stack(
            elements,
            x + (2.0 + grid_x as f32 * 7.0) * scale,
            y + (2.0 + grid_y as f32 * 7.0) * scale,
            6.0 * scale,
            item,
        );
    };

    match &entry.display {
        RecipeDisplay::Furnace { ingredient, .. } => draw(1, 1, ingredient),
        RecipeDisplay::Shaped {
            width,
            height,
            ingredients,
            ..
        } => {
            for (grid_index, ingredient) in
                centered_shaped_slots(3, 3, *width as usize, *height as usize, ingredients)
            {
                draw(grid_index % 3, grid_index / 3, ingredient);
            }
        }
        RecipeDisplay::Shapeless { ingredients, .. } => {
            for (index, ingredient) in ingredients.iter().take(9).enumerate() {
                draw(index % 3, index / 3, ingredient);
            }
        }
        _ => {}
    }
}

fn main_panel_rect(
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    panel_h: f32,
    x_offset: f32,
) -> (f32, f32, f32, f32, f32) {
    let scale = gs.min(screen_w / 176.0).min(screen_h / panel_h);
    let w = 176.0 * scale;
    let h = panel_h * scale;
    let ox = (screen_w - w) / 2.0 + x_offset * scale;
    let oy = (screen_h - h) / 2.0;
    (ox, oy, w, h, scale)
}

fn book_origin(screen_w: f32, screen_h: f32, gs: f32, narrow: bool) -> (f32, f32, f32) {
    let scale = gs.min(screen_w / BOOK_W).min(screen_h / BOOK_H);
    let x_offset = if narrow { 0.0 } else { 86.0 };
    let bx = (screen_w - BOOK_W * scale) / 2.0 - x_offset * scale;
    let by = (screen_h - BOOK_H * scale) / 2.0;
    (bx, by, scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{RecipeBookAddEntry, RecipeBookEntry, RecipeDisplay, SlotDisplay};

    fn recipe(id: u32, category: u32, requirements: Vec<Ingredient>) -> RecipeBookAddEntry {
        RecipeBookAddEntry {
            contents: RecipeBookEntry {
                id,
                display: RecipeDisplay::Shapeless {
                    ingredients: vec![SlotDisplay::Item(1)],
                    result: SlotDisplay::Item(2),
                    crafting_station: SlotDisplay::Item(3),
                },
                group: None,
                category,
                crafting_requirements: Some(requirements),
            },
            highlight: false,
            notification: false,
        }
    }

    #[test]
    fn craftability_respects_duplicate_ingredient_counts() {
        let entry = recipe(
            1,
            0,
            vec![Ingredient::Items(vec![5]), Ingredient::Items(vec![5])],
        )
        .contents;
        let tags = ItemTags::default();
        assert!(!can_craft(&entry, &HashMap::from([(5, 1)]), &tags));
        assert!(can_craft(&entry, &HashMap::from([(5, 2)]), &tags));
    }

    #[test]
    fn craftability_backtracks_across_alternatives() {
        let entry = recipe(
            1,
            0,
            vec![Ingredient::Items(vec![5, 6]), Ingredient::Items(vec![5])],
        )
        .contents;
        assert!(can_craft(
            &entry,
            &HashMap::from([(5, 1), (6, 1)]),
            &ItemTags::default()
        ));
    }

    #[test]
    fn missing_crafting_requirements_are_never_craftable() {
        let mut entry = recipe(1, 0, vec![Ingredient::Items(vec![5])]).contents;
        entry.crafting_requirements = None;
        assert!(!can_craft(
            &entry,
            &HashMap::from([(5, 64)]),
            &ItemTags::default()
        ));
    }

    #[test]
    fn player_craftability_excludes_armor_offhand_and_damaged_stacks() {
        use azalea_inventory::DataComponentPatch;
        use azalea_inventory::components::{Damage, DataComponentUnion};
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let diamond = ItemKind::Diamond.to_u32();
        let mut slots = vec![ItemStack::Empty; 46];
        slots[5] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 2));
        slots[45] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 3));
        assert!(!available_items(&slots, RecipeBookScreenSpec::player(0)).contains_key(&diamond));

        let mut patch = DataComponentPatch::default();
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::Damage,
                Some(DataComponentUnion::from(Damage { amount: 1 })),
            );
        }
        slots[9] = ItemStack::Present(ItemStackData {
            kind: ItemKind::Diamond,
            count: 4,
            component_patch: patch,
        });
        assert!(!available_items(&slots, RecipeBookScreenSpec::player(0)).contains_key(&diamond));

        slots[9] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 4));
        assert_eq!(
            available_items(&slots, RecipeBookScreenSpec::player(0)).get(&diamond),
            Some(&4)
        );
    }

    #[test]
    fn furnace_craftability_counts_output_container_stack() {
        use azalea_registry::builtin::ItemKind;

        let diamond = ItemKind::Diamond.to_u32();
        let mut slots = vec![ItemStack::Empty; 39];
        slots[2] = ItemStack::Present(ItemStackData::new(ItemKind::Diamond, 7));
        assert_eq!(
            available_items(
                &slots,
                RecipeBookScreenSpec::furnace(1, RecipeBookType::Furnace)
            )
            .get(&diamond),
            Some(&7)
        );
    }

    #[test]
    fn player_grid_rejects_three_wide_recipe() {
        let spec = RecipeBookScreenSpec::player(0);
        let display = RecipeDisplay::Shaped {
            width: 3,
            height: 1,
            ingredients: vec![SlotDisplay::Item(1); 3],
            result: SlotDisplay::Item(2),
            crafting_station: SlotDisplay::Item(3),
        };
        assert!(!can_display(spec, &display));
        assert!(can_display(
            RecipeBookScreenSpec::crafting_table(1),
            &display
        ));
    }

    #[test]
    fn shaped_ghost_centers_one_by_one_in_three_by_three_grid() {
        let ingredient = SlotDisplay::Item(1);
        let slots = centered_shaped_slots(3, 3, 1, 1, std::slice::from_ref(&ingredient));
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].0, 4);
        assert!(matches!(slots[0].1, SlotDisplay::Item(1)));
    }

    #[test]
    fn shaped_ghost_keeps_two_high_recipe_at_top_and_centers_horizontally() {
        let ingredients = vec![SlotDisplay::Item(1), SlotDisplay::Item(2)];
        let slots = centered_shaped_slots(3, 3, 1, 2, &ingredients);
        let indexes = slots
            .into_iter()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(indexes, vec![1, 4]);
    }

    #[test]
    fn recipe_collections_group_only_matching_group_and_category() {
        let mut book = RecipeBookState::default();
        let mut a = recipe(1, 0, vec![Ingredient::Items(vec![5])]);
        let mut b = recipe(2, 0, vec![Ingredient::Items(vec![6])]);
        let mut c = recipe(3, 1, vec![Ingredient::Items(vec![7])]);
        a.contents.group = Some(42);
        b.contents.group = Some(42);
        c.contents.group = Some(42);
        book.apply_add(vec![a, b, c], false);
        let grouped = collections(
            &book,
            RecipeBookScreenSpec::crafting_table(1),
            &HashMap::from([(5, 1), (6, 1), (7, 1)]),
        );
        assert_eq!(grouped.len(), 2);
        assert!(
            grouped
                .iter()
                .any(|collection| collection.entries.len() == 2)
        );
        assert!(
            grouped
                .iter()
                .any(|collection| collection.entries.len() == 1)
        );
    }

    #[test]
    fn narrow_threshold_matches_vanilla_379_gui_units() {
        assert!(width_too_narrow(378.0, 1.0));
        assert!(!width_too_narrow(379.0, 1.0));
        assert!(width_too_narrow(756.0, 2.0));
        assert!(!width_too_narrow(758.0, 2.0));
    }

    #[test]
    fn vanilla_fuel_items_preserve_tag_order_and_exclude_non_flammable_wood() {
        use azalea_registry::builtin::ItemKind as I;

        let oak = I::OakLog.to_u32();
        let mangrove = I::MangroveLog.to_u32();
        let crimson = I::CrimsonStem.to_u32();
        let tags = ItemTags::from_entries([
            ("minecraft:logs".into(), vec![mangrove, oak, crimson]),
            ("minecraft:non_flammable_wood".into(), vec![crimson]),
        ]);

        let fuels = vanilla_fuel_items(&tags);
        let mangrove_pos = fuels.iter().position(|id| *id == mangrove).unwrap();
        let oak_pos = fuels.iter().position(|id| *id == oak).unwrap();
        assert!(mangrove_pos < oak_pos);
        assert!(!fuels.contains(&crimson));
    }
    #[test]
    fn crafting_search_collections_follow_vanilla_category_order() {
        let mut book = RecipeBookState::default();
        book.apply_add(
            vec![
                recipe(1, 0, vec![Ingredient::Items(vec![1])]),
                recipe(2, 1, vec![Ingredient::Items(vec![1])]),
                recipe(3, 2, vec![Ingredient::Items(vec![1])]),
                recipe(4, 3, vec![Ingredient::Items(vec![1])]),
            ],
            false,
        );
        let grouped = collections(
            &book,
            RecipeBookScreenSpec::crafting_table(1),
            &HashMap::from([(1, 4)]),
        );
        let categories = grouped
            .iter()
            .map(|collection| collection.entries[0].category)
            .collect::<Vec<_>>();
        assert_eq!(categories, vec![2, 0, 3, 1]);
    }

    #[test]
    fn overlay_position_uses_clicked_button_and_vanilla_clamping() {
        let overlay = RecipeOverlay {
            ids: vec![1, 2, 3, 4],
            craftable_count: 2,
            button_index: 4,
        };
        let rect = overlay_rect_for(&overlay, 100.0, 200.0, 1.0);
        assert_eq!(rect, [136.0, 231.0, 108.0, 33.0]);
    }

    #[test]
    fn overlay_renders_ingredients_instead_of_result_item() {
        use azalea_registry::builtin::ItemKind as I;

        let ingredient = I::Stick.to_u32();
        let result = I::Diamond.to_u32();
        let entry = RecipeBookEntry {
            id: 1,
            display: RecipeDisplay::Shapeless {
                ingredients: vec![SlotDisplay::Item(ingredient)],
                result: SlotDisplay::Item(result),
                crafting_station: SlotDisplay::Empty,
            },
            group: None,
            category: 0,
            crafting_requirements: None,
        };
        let book = RecipeBookState::default();
        let mut elements = Vec::new();
        render_overlay_ingredients(&mut elements, &entry, &book, 10.0, 20.0, 1.0, 0);
        let names = elements
            .iter()
            .filter_map(|element| match element {
                MenuElement::ItemIcon { item_name, .. } => Some(item_name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["stick"]);
        assert!(!names.contains(&"diamond"));
    }

    #[test]
    fn grouped_recipe_tooltip_appends_explicit_more_line_to_full_stack_tooltip() {
        use azalea_registry::builtin::ItemKind;

        let stack = ResolvedSlotStack::plain(ItemStackData::new(ItemKind::OakPlanks, 1));
        let mut lines = stack_tooltip_lines(&stack);
        lines.push(TooltipLine::new("Right Click for More".into(), WHITE));
        assert!(lines.len() >= 2);
        assert_eq!(lines[0].spans[0].text, "Oak Planks");
        assert_eq!(lines.last().unwrap().spans[0].text, "Right Click for More");
    }

    #[test]
    fn player_result_ghost_stays_inside_normal_slot_bounds() {
        let player = RecipeBookScreenSpec::player(0);
        let crafting = RecipeBookScreenSpec::crafting_table(1);
        assert!(!player.big_result_slot);
        assert!(crafting.big_result_slot);
        assert_eq!(
            ghost_highlight_rect(10.0, 20.0, 1.0, false),
            [10.0, 20.0, 16.0, 16.0]
        );
        assert_eq!(
            ghost_highlight_rect(10.0, 20.0, 1.0, true),
            [6.0, 16.0, 24.0, 24.0]
        );
    }

    #[test]
    fn selecting_overlay_recipe_keeps_popup_open_on_wide_screen() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.overlay = Some(RecipeOverlay {
            ids: vec![1],
            craftable_count: 0,
            button_index: 0,
        });

        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        let (bx, by, scale) = book_origin(400.0, 300.0, 1.0, false);
        let overlay = state.overlay.clone().unwrap();
        let rect = overlay_rect_for(&overlay, bx, by, scale);
        let frame = handle_input(
            &mut state,
            &mut book,
            &sender,
            spec,
            400.0,
            300.0,
            1.0,
            (rect[0] + 5.0, rect[1] + 6.0),
            true,
            false,
            false,
            false,
            &[],
            &[ItemStack::Empty],
            &|text, _| text.len() as f32,
        );
        assert!(frame.visible);
        assert!(state.overlay.is_some());
    }

    #[test]
    fn selecting_overlay_recipe_closes_book_on_narrow_screen() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.overlay = Some(RecipeOverlay {
            ids: vec![1],
            craftable_count: 0,
            button_index: 0,
        });

        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        let (bx, by, scale) = book_origin(300.0, 300.0, 1.0, true);
        let overlay = state.overlay.clone().unwrap();
        let rect = overlay_rect_for(&overlay, bx, by, scale);
        let frame = handle_input(
            &mut state,
            &mut book,
            &sender,
            spec,
            300.0,
            300.0,
            1.0,
            (rect[0] + 5.0, rect[1] + 6.0),
            true,
            false,
            false,
            false,
            &[],
            &[ItemStack::Empty],
            &|text, _| text.len() as f32,
        );
        assert!(!frame.visible);
        assert!(state.overlay.is_none());
        assert!(!book.settings.crafting.open);
    }

    #[test]
    fn search_matches_vanilla_plain_and_identifier_modes_with_components() {
        use azalea_chat::FormattedText;
        use azalea_inventory::DataComponentPatch;
        use azalea_inventory::components::{CustomName, DataComponentUnion, Lore};
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let mut patch = DataComponentPatch::default();
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::CustomName,
                Some(DataComponentUnion::from(CustomName {
                    name: FormattedText::from("Fancy Gem"),
                })),
            );
            patch.unchecked_insert_component(
                DataComponentKind::Lore,
                Some(DataComponentUnion::from(Lore {
                    lines: vec![FormattedText::from("Secret Recipe")],
                })),
            );
        }
        let entry = RecipeBookEntry {
            id: 9,
            display: RecipeDisplay::Shapeless {
                ingredients: Vec::new(),
                result: SlotDisplay::ItemStack(crate::recipe::ItemStackTemplate {
                    item: ItemKind::Diamond.to_u32(),
                    count: 1,
                    components: patch,
                }),
                crafting_station: SlotDisplay::Empty,
            },
            group: None,
            category: 0,
            crafting_requirements: None,
        };
        let book = RecipeBookState::default();

        assert!(entry_matches_search(&entry, &book, "fancy"));
        assert!(entry_matches_search(&entry, &book, "secret"));
        assert!(!entry_matches_search(&entry, &book, "diamond"));
        assert!(!entry_matches_search(&entry, &book, " fancy"));
        assert!(entry_matches_search(&entry, &book, " minecraft : diamond "));
    }

    #[test]
    fn stale_recipe_screen_does_not_capture_chat_key() {
        let spec = RecipeBookScreenSpec::player(0);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;

        assert!(!state.focus_search_from_chat_key(&book, false));
        assert!(!state.captures_typing());

        assert!(state.focus_search_from_chat_key(&book, true));
        assert!(state.captures_typing());
    }

    #[test]
    fn focused_recipe_search_does_not_consume_chat_key_again() {
        let spec = RecipeBookScreenSpec::player(0);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;

        assert!(state.focus_search_from_chat_key(&book, true));
        assert!(!state.focus_search_from_chat_key(&book, true));
        assert!(state.search_focused);
    }

    #[test]
    fn first_character_after_chat_key_focus_is_not_dropped() {
        let spec = RecipeBookScreenSpec::player(0);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        assert!(state.focus_search_from_chat_key(&book, true));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        handle_input(
            &mut state,
            &mut book,
            &sender,
            spec,
            400.0,
            300.0,
            1.0,
            (0.0, 0.0),
            false,
            false,
            false,
            false,
            &[TextInputEvent::Char('s')],
            &vec![ItemStack::Empty; 46],
            &|text, _| text.len() as f32,
        );

        assert_eq!(state.search.value(), "s");
    }

    #[test]
    fn repeat_selection_stays_pinned_to_exact_clicked_recipe_across_cycle_change() {
        use crate::net::sender::Outbound;

        let spec = RecipeBookScreenSpec::player(0);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.last_clicked_recipe = Some(11);
        state.last_placed = None;
        state.cycle_time = Duration::from_millis(4_500); // visually cycle elsewhere

        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        book.apply_add(
            vec![
                recipe(11, 0, vec![Ingredient::Items(vec![1])]),
                recipe(22, 0, vec![Ingredient::Items(vec![1])]),
            ],
            false,
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        handle_input(
            &mut state,
            &mut book,
            &sender,
            spec,
            400.0,
            300.0,
            1.0,
            (0.0, 0.0),
            false,
            false,
            false,
            true,
            &[],
            &vec![ItemStack::Empty; 46],
            &|text, _| text.len() as f32,
        );

        match rx.try_recv().expect("repeat place packet") {
            Outbound::Raw(bytes) => assert_eq!(
                bytes,
                pomme_protocol::wire::encode_place_recipe(0, 11, false)
            ),
            _ => panic!("repeat placement must use native raw recipe encoding"),
        }
        assert_eq!(state.last_clicked_recipe, Some(11));
    }

    #[test]
    fn closing_recipe_screen_resets_transient_ui_and_ghost_state() {
        let spec = RecipeBookScreenSpec::crafting_table(3);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.selected_tab = 2;
        state.page = 3;
        state.overlay = Some(RecipeOverlay {
            ids: vec![1],
            craftable_count: 0,
            button_index: 0,
        });
        state.recipe_animation_started.insert(1, Instant::now());
        state.tab_animation_started.insert(0, Instant::now());

        let mut book = RecipeBookState::default();
        book.ghost_recipe = Some(crate::recipe::GhostRecipe {
            container_id: 3,
            recipe: recipe(1, 0, Vec::new()).contents.display,
        });
        state.reset_for_closed_screen(&mut book);

        assert!(state.screen.is_none());
        assert_eq!(state.selected_tab, 0);
        assert_eq!(state.page, 0);
        assert!(state.overlay.is_none());
        assert!(state.recipe_animation_started.is_empty());
        assert!(state.tab_animation_started.is_empty());
        assert!(state.tab_animation_highlights.is_empty());
        assert!(book.ghost_recipe.is_none());
    }

    #[test]
    fn ghost_alternatives_preserve_slot_display_order_and_duplicates() {
        let book = RecipeBookState::default();
        let slot = SlotDisplay::Composite(vec![
            SlotDisplay::Item(3),
            SlotDisplay::Item(1),
            SlotDisplay::Item(3),
        ]);
        let ids = slot_stacks(&slot, &book)
            .into_iter()
            .map(|stack| stack.stack.kind.to_u32())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 1, 3]);
    }

    #[test]
    fn with_any_potion_resolves_component_bearing_stacks() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot =
            SlotDisplay::WithAnyPotion(Box::new(SlotDisplay::Item(ItemKind::Potion.to_u32())));
        let stacks = slot_stacks(&slot, &book);
        assert!(stacks.len() > 1);
        assert!(stacks.iter().all(|stack| {
            stack.stack.kind == ItemKind::Potion
                && stack
                    .stack
                    .get_component::<PotionContents>()
                    .is_some_and(|contents| contents.potion.is_some())
        }));
    }

    #[test]
    fn only_with_component_uses_effective_dye_defaults() {
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let book = RecipeBookState::default();
        let slot = SlotDisplay::OnlyWithComponent {
            contents: Box::new(SlotDisplay::Composite(vec![
                SlotDisplay::Item(ItemKind::WhiteDye.to_u32()),
                SlotDisplay::Item(ItemKind::Diamond.to_u32()),
            ])),
            component: DataComponentKind::Dye.to_u32(),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::WhiteDye);
        assert!(stacks[0].stack.get_component::<Dye>().is_some());
    }

    #[test]
    fn only_with_component_supports_non_dye_default_components() {
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let book = RecipeBookState::default();
        let slot = SlotDisplay::OnlyWithComponent {
            contents: Box::new(SlotDisplay::Composite(vec![
                SlotDisplay::Item(ItemKind::IronChestplate.to_u32()),
                SlotDisplay::Item(ItemKind::Diamond.to_u32()),
            ])),
            component: DataComponentKind::MaxDamage.to_u32(),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::IronChestplate);
        assert!(
            stacks[0]
                .stack
                .get_component::<components::MaxDamage>()
                .is_some()
        );
    }

    #[test]
    fn dyed_demo_writes_vanilla_dyed_color_component() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::Dyed {
            dye: Box::new(SlotDisplay::Item(ItemKind::RedDye.to_u32())),
            target: Box::new(SlotDisplay::Item(ItemKind::LeatherChestplate.to_u32())),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].stack.kind, ItemKind::LeatherChestplate);
        assert_eq!(stacks[0].stack.count, 1);
        assert_eq!(
            stacks[0].stack.get_component::<DyedColor>().unwrap().rgb,
            0xB02E26
        );
    }

    #[test]
    fn smithing_trim_demo_writes_trim_component() {
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::SmithingTrim {
            base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
            material: Box::new(SlotDisplay::Item(ItemKind::Diamond.to_u32())),
            trim_pattern: TrimPatternHolder::Reference(0),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 16);
        assert!(stacks.iter().all(|stack| {
            stack.stack.kind == ItemKind::IronChestplate
                && stack.stack.count == 1
                && stack.stack.get_component::<Trim>().is_some()
                && stack.direct_trim.is_none()
        }));
    }

    #[test]
    fn direct_trim_pattern_is_preserved_in_resolved_stack_and_tooltip() {
        use azalea_chat::FormattedText;
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::SmithingTrim {
            base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
            material: Box::new(SlotDisplay::Item(ItemKind::Diamond.to_u32())),
            trim_pattern: TrimPatternHolder::Direct {
                asset_id: "minecraft:test".into(),
                description: FormattedText::from("Test Trim"),
                decal: false,
            },
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 16);
        let stack = &stacks[0];
        assert_eq!(stack.stack.kind, ItemKind::IronChestplate);
        assert!(stack.stack.get_component::<Trim>().is_none());
        assert!(matches!(
            stack.direct_trim.as_ref().map(|trim| &trim.pattern),
            Some(TrimPatternHolder::Direct { asset_id, .. }) if asset_id == "minecraft:test"
        ));
        assert!(
            normal_tooltip_search_lines(stack)
                .iter()
                .any(|line| line.contains("Test Trim"))
        );
    }

    #[test]
    fn direct_trim_material_is_preserved_in_resolved_stack_and_tooltip() {
        use azalea_chat::FormattedText;
        use azalea_inventory::DataComponentPatch;
        use azalea_inventory::components::{
            AssetInfo, DataComponentUnion, DirectTrimMaterial, MaterialAssetGroup,
        };
        use azalea_registry::builtin::{DataComponentKind, ItemKind};

        let mut patch = DataComponentPatch::default();
        let direct_material = DirectTrimMaterial {
            assets: MaterialAssetGroup {
                assert_name: AssetInfo {
                    suffix: "test".into(),
                },
                override_armor_assets: Vec::new(),
            },
            description: FormattedText::from("Test Material"),
        };
        unsafe {
            patch.unchecked_insert_component(
                DataComponentKind::ProvidesTrimMaterial,
                Some(DataComponentUnion::from(ProvidesTrimMaterial {
                    value: Holder::Direct(direct_material),
                })),
            );
        }

        let book = RecipeBookState::default();
        let slot = SlotDisplay::SmithingTrim {
            base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
            material: Box::new(SlotDisplay::ItemStack(crate::recipe::ItemStackTemplate {
                item: ItemKind::Diamond.to_u32(),
                count: 1,
                components: patch,
            })),
            trim_pattern: TrimPatternHolder::Reference(0),
        };
        let stacks = slot_stacks(&slot, &book);
        assert_eq!(stacks.len(), 16);
        let stack = &stacks[0];
        assert!(matches!(
            stack.direct_trim.as_ref().map(|trim| &trim.material),
            Some(Holder::Direct(material)) if material.description.to_string() == "Test Material"
        ));
        assert!(
            normal_tooltip_search_lines(stack)
                .iter()
                .any(|line| line.contains("Test Material"))
        );
    }

    #[test]
    fn page_shrink_resets_to_first_page() {
        let mut page = 3;
        reset_page_if_out_of_range(&mut page, 2);
        assert_eq!(page, 0);
        let mut page = 1;
        reset_page_if_out_of_range(&mut page, 2);
        assert_eq!(page, 1);
    }

    #[test]
    fn ctrl_freezes_recipe_cycle_clock() {
        let mut state = RecipeBookUiState::new();
        state.cycle_time = Duration::from_millis(1_490);
        state.cycle_last_update = Instant::now() - Duration::from_millis(30);
        state.update_cycle_time(true);
        assert_eq!(state.cycle_time, Duration::from_millis(1_490));
        assert_eq!(state.cycle_index(), 0);

        state.cycle_last_update = Instant::now() - Duration::from_millis(30);
        state.update_cycle_time(false);
        assert!(state.cycle_time >= Duration::from_millis(1_520));
        assert_eq!(state.cycle_index(), 1);
    }

    #[test]
    fn highlight_animation_lasts_exactly_fifteen_ticks() {
        let started = Instant::now();
        assert_eq!(animation_squeeze(started, started), Some(1.0));
        assert!(animation_squeeze(started, started + Duration::from_millis(375)).unwrap() > 1.0);
        assert_eq!(
            animation_squeeze(started, started + HIGHLIGHT_ANIMATION),
            None
        );
    }

    #[test]
    fn tab_highlight_does_not_rearm_until_highlight_set_changes() {
        let mut state = RecipeBookUiState::new();
        let started = Instant::now();
        let highlights = HashSet::from([7]);

        assert_eq!(
            tab_animation_squeeze(&mut state, 2, highlights.clone(), started),
            1.0
        );
        assert!(state.tab_animation_started.contains_key(&2));

        let expired = started + HIGHLIGHT_ANIMATION;
        assert_eq!(
            tab_animation_squeeze(&mut state, 2, highlights.clone(), expired),
            1.0
        );
        assert!(!state.tab_animation_started.contains_key(&2));
        assert_eq!(state.tab_animation_highlights.get(&2), Some(&highlights));

        let later = expired + Duration::from_secs(1);
        assert_eq!(tab_animation_squeeze(&mut state, 2, highlights, later), 1.0);
        assert!(!state.tab_animation_started.contains_key(&2));

        let changed = HashSet::from([7, 8]);
        assert_eq!(tab_animation_squeeze(&mut state, 2, changed, later), 1.0);
        assert_eq!(state.tab_animation_started.get(&2), Some(&later));
        assert!(
            tab_animation_squeeze(
                &mut state,
                2,
                HashSet::from([7, 8]),
                later + Duration::from_millis(375)
            ) > 1.0
        );
    }
}
