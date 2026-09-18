use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use azalea_inventory::ItemStack;
use azalea_registry::Registry;

use super::common::{FONT_SIZE, WHITE, hit_test};
use super::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};
use crate::net::sender::PacketSender;
use crate::recipe::{
    Ingredient, ItemTags, RecipeBookEntry, RecipeBookState, RecipeBookType, RecipeDisplay,
    RecipeDisplayId, SlotDisplay,
};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const BOOK_W: f32 = 147.0;
const BOOK_H: f32 = 166.0;
const ITEMS_PER_PAGE: usize = 20;
const SEARCH_X: f32 = 25.0;
const SEARCH_Y: f32 = 13.0;
const SEARCH_W: f32 = 81.0;
const SEARCH_H: f32 = 14.0;
const FILTER_X: f32 = 110.0;
const FILTER_Y: f32 = 12.0;
const FILTER_W: f32 = 26.0;
const FILTER_H: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub struct RecipeBookScreenSpec {
    pub kind: RecipeBookType,
    pub container_id: i32,
    pub grid_width: usize,
    pub grid_height: usize,
    pub result_slot: usize,
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
            result_slot: 0,
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
            result_slot: 0,
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
            result_slot: 2,
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
    overlay: Option<Vec<RecipeDisplayId>>,
    last_placed: Option<RecipeDisplayId>,
    last_clicked_collection: Option<Vec<RecipeDisplayId>>,
    narrow: bool,
    ignore_next_typed_char: bool,
    started: Instant,
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
            last_clicked_collection: None,
            narrow: false,
            ignore_next_typed_char: false,
            started: Instant::now(),
        }
    }

    pub fn captures_typing(&self) -> bool {
        self.search_focused
    }

    pub fn reset_for_closed_screen(&mut self) {
        self.search_focused = false;
        self.overlay = None;
        self.last_placed = None;
        self.narrow = false;
        self.ignore_next_typed_char = false;
    }

    pub fn focus_search_from_chat_key(&mut self, book: &RecipeBookState) -> bool {
        let Some((kind, _)) = self.screen else {
            return false;
        };
        if !book.settings.get(kind).open {
            return false;
        }
        self.search_focused = true;
        self.search.set_focused(true);
        self.ignore_next_typed_char = true;
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
            self.last_clicked_collection = None;
            self.search.clear();
            self.search_focused = false;
        }
    }

    fn cycle_index(&self) -> usize {
        (self.started.elapsed().as_millis() / 1_500) as usize
    }
}

impl Default for RecipeBookUiState {
    fn default() -> Self {
        Self::new()
    }
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
            let text_x = search_rect[0] + scale;
            let rel = (cursor.0 - text_x).max(0.0);
            let fs = FONT_SIZE * scale;
            let wf = |s: &str| text_width_fn(s, fs);
            let pos = state
                .search
                .pos_from_click(rel, SEARCH_W * scale - 2.0 * scale, &wf);
            state
                .search
                .on_click(pos, shift, SEARCH_W * scale - 2.0 * scale, &wf);
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
            if state.ignore_next_typed_char && matches!(event, TextInputEvent::Char(_)) {
                state.ignore_next_typed_char = false;
                continue;
            }
            state
                .search
                .handle(event, &mut clipboard, SEARCH_W * scale - 2.0 * scale, &wf);
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

    let available = available_items(slots, spec.result_slot);
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
    state.page = state.page.min(pages - 1);

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

    if let Some(ids) = state.overlay.clone() {
        let entries = ids
            .iter()
            .filter_map(|id| book.known.get(id).cloned())
            .collect::<Vec<_>>();
        let overlay_rect = overlay_rect_for(&entries, bx + 11.0 * scale, by + 31.0 * scale, scale);
        if left_clicked {
            if let Some(id) = hit_overlay_recipe(cursor, &entries, overlay_rect, scale) {
                let use_max = shift;
                if state.last_placed != Some(id)
                    || can_craft_entry(book.known.get(&id), &available, &book.item_tags)
                {
                    let _ = sender.place_recipe(spec.container_id, id, use_max);
                    state.last_placed = Some(id);
                    state.last_clicked_collection = Some(ids);
                    book.ghost_recipe = None;
                }
                state.overlay = None;
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
                state.last_clicked_collection =
                    Some(selected.iter().map(|entry| entry.id).collect());
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
            state.overlay = Some(selected.iter().map(|entry| entry.id).collect());
            frame.consumed_right_click = true;
        }
    }

    if selection_key
        && let Some(ids) = state.last_clicked_collection.clone()
        && let Some(id) = ids.get(cycle % ids.len()).copied()
    {
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
    state: &RecipeBookUiState,
    book: &mut RecipeBookState,
    sender: &PacketSender,
    spec: RecipeBookScreenSpec,
    frame: RecipeBookFrame,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
    slots: &[ItemStack],
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
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

        let available = available_items(slots, spec.result_slot);
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
                }
            }
            let current = selected[cycle % selected.len()];
            let craftable = collection.has_craftable();
            let multiple = selected.len() > 1;
            let sprite = match (craftable, multiple) {
                (true, false) => SpriteId::RecipeBookSlotCraftable,
                (true, true) => SpriteId::RecipeBookSlotManyCraftable,
                (false, false) => SpriteId::RecipeBookSlotUncraftable,
                (false, true) => SpriteId::RecipeBookSlotManyUncraftable,
            };
            elements.push(MenuElement::Image {
                x,
                y,
                w: 25.0 * scale,
                h: 25.0 * scale,
                sprite,
                tint: WHITE,
            });
            if let Some(item) = display_item(&current.display, book, cycle) {
                push_book_item(
                    elements,
                    x + 4.0 * scale,
                    y + 4.0 * scale,
                    16.0 * scale,
                    item,
                );
                if hit_test(cursor, [x, y, 25.0 * scale, 25.0 * scale]) && state.overlay.is_none() {
                    let mut name = item_name(item);
                    if multiple {
                        name.push_str("\nMore recipes");
                    }
                    elements.push(MenuElement::Tooltip {
                        x: cursor.0,
                        y: cursor.1,
                        text: name,
                        scale,
                        screen_w,
                        screen_h,
                    });
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

        if let Some(ids) = &state.overlay {
            let entries = ids
                .iter()
                .filter_map(|id| book.known.get(id).cloned())
                .collect::<Vec<_>>();
            render_overlay(
                elements,
                &entries,
                book,
                bx + 11.0 * scale,
                by + 31.0 * scale,
                scale,
                cursor,
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

fn visible_tabs(tabs: &[TabInfo], collections: &[Collection], filtering: bool) -> Vec<TabInfo> {
    tabs.iter()
        .copied()
        .filter(|tab| {
            tab.category.is_none()
                || collections.iter().any(|collection| {
                    collection
                        .entries
                        .first()
                        .is_some_and(|entry| entry.category == tab.category.unwrap())
                        && !collection.selected(filtering).is_empty()
                })
        })
        .collect()
}

fn collections(
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
    available: &HashMap<u32, u32>,
) -> Vec<Collection> {
    let mut entries = book.known.values().cloned().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.id);
    let mut grouped: BTreeMap<(u32, bool, u32), Vec<RecipeBookEntry>> = BTreeMap::new();
    for entry in entries {
        if !can_display(spec, &entry.display) || !kind_accepts_category(spec.kind, entry.category) {
            continue;
        }
        let key = match entry.group {
            Some(group) => (entry.category, true, group),
            None => (entry.category, false, entry.id),
        };
        grouped.entry(key).or_default().push(entry);
    }
    grouped
        .into_values()
        .map(|entries| {
            let craftable = entries
                .iter()
                .filter(|entry| can_craft(entry, available, &book.item_tags))
                .map(|entry| entry.id)
                .collect();
            Collection { entries, craftable }
        })
        .collect()
}

fn filtered_collections<'a>(
    collections: &'a [Collection],
    category: Option<u32>,
    filtering: bool,
    search: &str,
    book: &RecipeBookState,
) -> Vec<&'a Collection> {
    let needle = search.trim().to_lowercase();
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
    display_item_ids(&entry.display, book)
        .into_iter()
        .any(|id| item_name(id).to_lowercase().contains(needle))
        || entry
            .crafting_requirements
            .as_ref()
            .into_iter()
            .flatten()
            .flat_map(|ingredient| ingredient_items(ingredient, &book.item_tags))
            .any(|id| item_name(id).to_lowercase().contains(needle))
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

fn available_items(slots: &[ItemStack], result_slot: usize) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    for (index, stack) in slots.iter().enumerate() {
        if index == result_slot {
            continue;
        }
        if let ItemStack::Present(data) = stack {
            *counts.entry(data.kind.to_u32()).or_default() += data.count as u32;
        }
    }
    counts
}

fn can_craft_entry(
    entry: Option<&RecipeBookEntry>,
    available: &HashMap<u32, u32>,
    tags: &ItemTags,
) -> bool {
    entry.is_some_and(|entry| can_craft(entry, available, tags))
}

fn can_craft(entry: &RecipeBookEntry, available: &HashMap<u32, u32>, tags: &ItemTags) -> bool {
    let requirements = entry
        .crafting_requirements
        .clone()
        .unwrap_or_else(|| display_requirements(&entry.display, tags));
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

fn display_requirements(display: &RecipeDisplay, tags: &ItemTags) -> Vec<Ingredient> {
    let slots: &[SlotDisplay] = match display {
        RecipeDisplay::Shapeless { ingredients, .. }
        | RecipeDisplay::Shaped { ingredients, .. } => ingredients,
        RecipeDisplay::Furnace { ingredient, .. } => {
            return slot_to_ingredient(ingredient, tags).into_iter().collect();
        }
        _ => return Vec::new(),
    };
    slots
        .iter()
        .filter_map(|slot| slot_to_ingredient(slot, tags))
        .collect()
}

fn slot_to_ingredient(slot: &SlotDisplay, tags: &ItemTags) -> Option<Ingredient> {
    match slot {
        SlotDisplay::Item(id) => Some(Ingredient::Items(vec![*id])),
        SlotDisplay::ItemStack(stack) => Some(Ingredient::Items(vec![stack.item])),
        SlotDisplay::Tag(tag) => Some(Ingredient::Items(tags.get(tag)?.iter().copied().collect())),
        SlotDisplay::Composite(parts) => Some(Ingredient::Items(
            parts
                .iter()
                .flat_map(|part| slot_item_ids(part, tags))
                .collect(),
        )),
        SlotDisplay::WithAnyPotion(inner)
        | SlotDisplay::OnlyWithComponent {
            contents: inner, ..
        } => slot_to_ingredient(inner, tags),
        SlotDisplay::Dyed { target, .. } => slot_to_ingredient(target, tags),
        SlotDisplay::SmithingTrim { base, .. } => slot_to_ingredient(base, tags),
        SlotDisplay::WithRemainder { input, .. } => slot_to_ingredient(input, tags),
        SlotDisplay::Empty | SlotDisplay::AnyFuel => None,
    }
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

fn slot_item_ids(slot: &SlotDisplay, tags: &ItemTags) -> Vec<u32> {
    match slot {
        SlotDisplay::Empty | SlotDisplay::AnyFuel => Vec::new(),
        SlotDisplay::Item(id) => vec![*id],
        SlotDisplay::ItemStack(stack) => vec![stack.item],
        SlotDisplay::Tag(tag) => tags
            .get(tag)
            .map(|items| items.iter().copied().collect())
            .unwrap_or_default(),
        SlotDisplay::WithAnyPotion(inner)
        | SlotDisplay::OnlyWithComponent {
            contents: inner, ..
        } => slot_item_ids(inner, tags),
        SlotDisplay::Dyed { target, .. } => slot_item_ids(target, tags),
        SlotDisplay::SmithingTrim { base, .. } => slot_item_ids(base, tags),
        SlotDisplay::WithRemainder { input, .. } => slot_item_ids(input, tags),
        SlotDisplay::Composite(parts) => parts
            .iter()
            .flat_map(|part| slot_item_ids(part, tags))
            .collect(),
    }
}

fn display_item_ids(display: &RecipeDisplay, book: &RecipeBookState) -> Vec<u32> {
    let result = match display {
        RecipeDisplay::Shapeless { result, .. }
        | RecipeDisplay::Shaped { result, .. }
        | RecipeDisplay::Furnace { result, .. }
        | RecipeDisplay::Stonecutter { result, .. }
        | RecipeDisplay::Smithing { result, .. } => result,
    };
    slot_item_ids(result, &book.item_tags)
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
                true,
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
                true,
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
                true,
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

#[allow(clippy::too_many_arguments)]
fn render_ghost_slot(
    elements: &mut Vec<MenuElement>,
    slot: &SlotDisplay,
    book: &RecipeBookState,
    cycle: usize,
    x: f32,
    y: f32,
    scale: f32,
    result: bool,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let mut items = ghost_slot_items(slot, book);
    if items.is_empty() {
        return;
    }
    items.sort_unstable();
    items.dedup();
    let item = items[cycle % items.len()];
    let red = if result {
        [x - 4.0 * scale, y - 4.0 * scale, 24.0 * scale, 24.0 * scale]
    } else {
        [x, y, 16.0 * scale, 16.0 * scale]
    };
    elements.push(MenuElement::Rect {
        x: red[0],
        y: red[1],
        w: red[2],
        h: red[3],
        corner_radius: 0.0,
        color: [1.0, 0.0, 0.0, 0.19],
    });
    push_book_item(elements, x, y, 16.0 * scale, item);
    elements.push(MenuElement::Rect {
        x,
        y,
        w: 16.0 * scale,
        h: 16.0 * scale,
        corner_radius: 0.0,
        color: [1.0, 1.0, 1.0, 0.19],
    });
    if hit_test(cursor, [x, y, 16.0 * scale, 16.0 * scale]) {
        elements.push(MenuElement::Tooltip {
            x: cursor.0,
            y: cursor.1,
            text: item_name(item),
            scale,
            screen_w,
            screen_h,
        });
    }
}

fn ghost_slot_items(slot: &SlotDisplay, book: &RecipeBookState) -> Vec<u32> {
    if matches!(slot, SlotDisplay::AnyFuel) {
        return vanilla_fuel_items(&book.item_tags);
    }
    slot_item_ids(slot, &book.item_tags)
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

fn display_item(display: &RecipeDisplay, book: &RecipeBookState, cycle: usize) -> Option<u32> {
    let items = display_item_ids(display, book);
    (!items.is_empty()).then(|| items[cycle % items.len()])
}

fn item_name(id: u32) -> String {
    azalea_registry::builtin::ItemKind::from_u32(id)
        .map(crate::lang::item_display_name)
        .unwrap_or_else(|| format!("Item #{id}"))
}

fn item_resource(id: u32) -> Option<String> {
    azalea_registry::builtin::ItemKind::from_u32(id)
        .map(crate::player::inventory::item_resource_name)
}

fn push_book_item(elements: &mut Vec<MenuElement>, x: f32, y: f32, size: f32, id: u32) {
    if let Some(item_name) = item_resource(id) {
        elements.push(MenuElement::ItemIcon {
            x,
            y,
            w: size,
            h: size,
            item_name,
            tint: WHITE,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_search(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    bx: f32,
    by: f32,
    scale: f32,
    _cursor: (f32, f32),
    _screen_w: f32,
    _screen_h: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let x = bx + SEARCH_X * scale;
    let y = by + SEARCH_Y * scale;
    let fs = FONT_SIZE * scale;
    let wf = |s: &str| text_width_fn(s, fs);
    let inner_w = SEARCH_W * scale - 2.0 * scale;
    let info = state.search.render_info(inner_w, state.search_focused, &wf);
    let shown = &state.search.value()[info.display_start..info.display_end];
    if shown.is_empty() && !state.search_focused {
        elements.push(MenuElement::Text {
            x: x + scale,
            y: y + 3.0 * scale,
            text: "Search...".into(),
            scale: fs,
            color: [0.5, 0.5, 0.5, 1.0],
            centered: false,
        });
    } else {
        super::common::push_field_text(
            elements,
            &info,
            shown,
            x + scale,
            y + 3.0 * scale,
            fs,
            scale,
            scale,
            WHITE,
            None,
            &wf,
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
                RecipeBookType::Crafting => "Showing craftable",
                RecipeBookType::Furnace => "Showing smeltable",
                RecipeBookType::BlastFurnace => "Showing blastable",
                RecipeBookType::Smoker => "Showing smokable",
            }
        } else {
            "Showing all recipes"
        };
        elements.push(MenuElement::Tooltip {
            x: cursor.0,
            y: cursor.1,
            text: text.into(),
            scale,
            screen_w,
            screen_h,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tabs(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
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
    let mut y = by + 3.0 * scale;
    for (index, tab) in tabs.iter().enumerate() {
        let selected = index == state.selected_tab;
        let x = bx - (if selected { 32.0 } else { 30.0 }) * scale;
        elements.push(MenuElement::Image {
            x,
            y,
            w: 35.0 * scale,
            h: 27.0 * scale,
            sprite: if selected {
                SpriteId::RecipeBookTabSelected
            } else {
                SpriteId::RecipeBookTab
            },
            tint: WHITE,
        });
        let icon_offset = if selected { -2.0 } else { 0.0 };
        if let Some(icon_b) = tab.icon_b {
            elements.push(MenuElement::ItemIcon {
                x: bx + (-27.0 + icon_offset) * scale,
                y: y + 5.0 * scale,
                w: 16.0 * scale,
                h: 16.0 * scale,
                item_name: tab.icon_a.into(),
                tint: WHITE,
            });
            elements.push(MenuElement::ItemIcon {
                x: bx + (-16.0 + icon_offset) * scale,
                y: y + 5.0 * scale,
                w: 16.0 * scale,
                h: 16.0 * scale,
                item_name: icon_b.into(),
                tint: WHITE,
            });
        } else {
            elements.push(MenuElement::ItemIcon {
                x: bx + (-21.0 + icon_offset) * scale,
                y: y + 5.0 * scale,
                w: 16.0 * scale,
                h: 16.0 * scale,
                item_name: tab.icon_a.into(),
                tint: WHITE,
            });
        }
        let _has_highlight = tab.category.is_some_and(|cat| {
            collections.iter().any(|collection| {
                collection.entries.iter().any(|entry| {
                    entry.category == cat
                        && book.highlight.contains(&entry.id)
                        && (!filtering || collection.craftable.contains(&entry.id))
                })
            })
        });
        y += 27.0 * scale;
    }
}

fn overlay_rect_for(
    entries: &[RecipeBookEntry],
    anchor_x: f32,
    anchor_y: f32,
    scale: f32,
) -> [f32; 4] {
    let max_row = if entries.len() <= 16 { 4 } else { 5 };
    let cols = entries.len().min(max_row).max(1);
    let rows = entries.len().div_ceil(max_row).max(1);
    [
        anchor_x,
        anchor_y,
        (cols * 25 + 8) as f32 * scale,
        (rows * 25 + 8) as f32 * scale,
    ]
}

fn hit_overlay_recipe(
    cursor: (f32, f32),
    entries: &[RecipeBookEntry],
    rect: [f32; 4],
    scale: f32,
) -> Option<RecipeDisplayId> {
    let max_row = if entries.len() <= 16 { 4 } else { 5 };
    entries.iter().enumerate().find_map(|(index, entry)| {
        let x = rect[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale;
        let y = rect[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale;
        hit_test(cursor, [x, y, 24.0 * scale, 24.0 * scale]).then_some(entry.id)
    })
}

fn render_overlay(
    elements: &mut Vec<MenuElement>,
    entries: &[RecipeBookEntry],
    book: &RecipeBookState,
    anchor_x: f32,
    anchor_y: f32,
    scale: f32,
    cursor: (f32, f32),
) {
    let rect = overlay_rect_for(entries, anchor_x, anchor_y, scale);
    elements.push(MenuElement::NineSlice {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: SpriteId::RecipeBookOverlay,
        border: 4.0 * scale,
        tint: WHITE,
    });
    let max_row = if entries.len() <= 16 { 4 } else { 5 };
    for (index, entry) in entries.iter().enumerate() {
        let x = rect[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale;
        let y = rect[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale;
        let hovered = hit_test(cursor, [x, y, 24.0 * scale, 24.0 * scale]);
        let furnace = matches!(entry.display, RecipeDisplay::Furnace { .. });
        let sprite = match (furnace, hovered) {
            (false, false) => SpriteId::RecipeBookCraftingOverlay,
            (false, true) => SpriteId::RecipeBookCraftingOverlayHighlighted,
            (true, false) => SpriteId::RecipeBookFurnaceOverlay,
            (true, true) => SpriteId::RecipeBookFurnaceOverlayHighlighted,
        };
        elements.push(MenuElement::Image {
            x,
            y,
            w: 24.0 * scale,
            h: 24.0 * scale,
            sprite,
            tint: WHITE,
        });
        if let Some(item) = display_item(&entry.display, book, 0) {
            push_book_item(
                elements,
                x + 4.0 * scale,
                y + 4.0 * scale,
                16.0 * scale,
                item,
            );
        }
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
}
