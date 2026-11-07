//! Recipe-book state, input handling, and shared layout.
mod render;
pub use render::render;
use render::*;
mod collections;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use azalea_inventory::components::{self, CustomName, Damage, Enchantments};
use azalea_inventory::item::MaxStackSizeExt;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::{DataRegistry, DataRegistryKey, Holder, Registry};
use collections::*;

use super::common::{
    FONT_SIZE, WHITE, hit_test, push_item_decorations, push_item_glyph, push_tooltip,
    push_tooltip_lines,
};
use super::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};
use crate::net::sender::PacketSender;
use crate::recipe::display::*;
use crate::recipe::{
    ItemTags, RecipeBookEntry, RecipeBookState, RecipeBookType, RecipeDisplay, RecipeDisplayId,
    SlotDisplay, TrimPatternHolder,
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

#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Selected tab by category (`None` is the search tab), as vanilla keys it.
    selected_tab: Option<u32>,
    page: usize,
    overlay: Option<RecipeOverlay>,
    last_placed: Option<RecipeDisplayId>,
    last_clicked_recipe: Option<RecipeDisplayId>,
    /// Per page button, when its highlight pulse started (`RecipeButton.init`).
    button_animation_started: [Option<Instant>; ITEMS_PER_PAGE],
    /// Per category tab, when its pulse started (`startAnimation`).
    tab_animation_started: HashMap<u32, Instant>,
    narrow: bool,
    /// Whether the book showed last frame, to run vanilla `initVisuals` when
    /// it opens.
    was_visible: bool,
    cycle_time: Duration,
    cycle_last_update: Instant,
    view: Option<RecipeBookView>,
    ghost: Option<GhostView>,
}

impl RecipeBookUiState {
    pub fn new() -> Self {
        Self {
            search: TextFieldState::new(50),
            search_focused: false,
            screen: None,
            selected_tab: None,
            page: 0,
            overlay: None,
            last_placed: None,
            last_clicked_recipe: None,
            button_animation_started: [None; ITEMS_PER_PAGE],
            tab_animation_started: HashMap::new(),
            narrow: false,
            was_visible: false,
            cycle_time: Duration::ZERO,
            cycle_last_update: Instant::now(),
            view: None,
            ghost: None,
        }
    }

    pub fn captures_typing(&self) -> bool {
        self.search_focused
    }

    /// Whether a recipe screen used the book since the last reset.
    pub fn has_screen(&self) -> bool {
        self.screen.is_some()
    }

    pub fn reset_for_closed_screen(&mut self, book: &mut RecipeBookState) {
        *self = Self::new();
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
        self.set_search_focus(true);
        true
    }

    pub fn close_for_escape(&mut self, book: &mut RecipeBookState, sender: &PacketSender) -> bool {
        let Some((kind, _)) = self.screen else {
            return false;
        };
        if !self.narrow || !book.settings.get(kind).open {
            return false;
        }
        self.set_book_open(book, sender, kind, false);
        true
    }

    /// Vanilla `slotClicked` on a crafting slot; craftability follows from
    /// the next frame's inventory.
    pub fn crafting_slot_clicked(&mut self, book: &mut RecipeBookState) {
        self.last_placed = None;
        self.clear_ghost(book);
    }

    fn ensure_screen(&mut self, spec: RecipeBookScreenSpec) {
        let key = (spec.kind, spec.container_id);
        if self.screen != Some(key) {
            *self = Self {
                screen: Some(key),
                ..Self::new()
            };
        }
    }

    fn set_search_focus(&mut self, focused: bool) {
        self.search_focused = focused;
        self.search.set_focused(focused);
    }

    /// Vanilla `setVisible`: store and send the setting; hiding also closes
    /// the overlay and unfocuses search.
    fn set_book_open(
        &mut self,
        book: &mut RecipeBookState,
        sender: &PacketSender,
        kind: RecipeBookType,
        open: bool,
    ) {
        let mut settings = book.settings.get(kind);
        settings.open = open;
        book.settings.set(kind, settings);
        let _ = sender.recipe_book_change_settings(kind, open, settings.filtering);
        self.set_search_focus(false);
        self.overlay = None;
    }

    fn clear_ghost(&mut self, book: &mut RecipeBookState) {
        book.ghost_recipe = None;
        self.ghost = None;
    }

    /// Vanilla `tryPlaceRecipe`: refuses an uncraftable recipe placed last.
    fn try_place_recipe(
        &mut self,
        book: &mut RecipeBookState,
        sender: &PacketSender,
        spec: RecipeBookScreenSpec,
        id: RecipeDisplayId,
        use_max: bool,
    ) -> bool {
        let craftable = self
            .view
            .as_ref()
            .is_some_and(|view| view.collections.iter().any(|c| c.craftable.contains(&id)));
        if !craftable && self.last_placed == Some(id) {
            return false;
        }
        self.last_placed = Some(id);
        self.clear_ghost(book);
        let _ = sender.place_recipe(spec.container_id, id, use_max);
        true
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

    /// Brings the derived view up to date, rebuilding only what vanilla
    /// does on each change: collections and tabs on `recipesUpdated`,
    /// craftability on an inventory change, the shown list on a tab, search
    /// or filter change.
    fn prepare(&mut self, book: &RecipeBookState, spec: RecipeBookScreenSpec, slots: &[ItemStack]) {
        let available = available_items(slots, spec);
        let recipes_updated = match &mut self.view {
            Some(view) if view.revision == book.revision() && view.spec == spec => {
                if view.available != available {
                    select_matching_recipes(&mut view.collections, &available, book.item_tags());
                    view.available = available;
                    view.shown_key = None;
                }
                false
            }
            _ => {
                let mut collections = build_collections(book, spec);
                select_matching_recipes(&mut collections, &available, book.item_tags());
                let tabs = visible_tabs(tabs_for(spec.kind), &collections);
                self.view = Some(RecipeBookView {
                    revision: book.revision(),
                    spec,
                    available,
                    collections,
                    search_index: None,
                    tabs,
                    shown_key: None,
                    shown: Vec::new(),
                    shown_generation: 0,
                    initialized_page: None,
                    page_buttons: Vec::new(),
                });
                true
            }
        };
        let visible = visible(book, spec.kind);
        let filtering = book.settings.get(spec.kind).filtering;
        if visible && (recipes_updated || !self.was_visible) {
            self.arm_tab_animations(book, filtering);
        }
        self.was_visible = visible;

        let view = self.view.as_mut().expect("view built above");
        let key = ShownKey {
            category: self.selected_tab,
            search: self.search.value().to_lowercase(),
            filtering,
        };
        if view.shown_key.as_ref() != Some(&key) {
            view.update_shown(&key, book);
            view.shown_key = Some(key);
            if self.page >= view.pages() {
                self.page = 0;
            }
        }
    }

    /// Vanilla `updateTabs`' `startAnimation`: a category tab pulses when a
    /// recipe it shows is still highlighted. The search tab never does.
    fn arm_tab_animations(&mut self, book: &RecipeBookState, filtering: bool) {
        let Some(view) = &self.view else {
            return;
        };
        let now = Instant::now();
        for category in view.tabs.iter().filter_map(|tab| tab.category) {
            let highlighted = view
                .collections
                .iter()
                .filter(|c| c.category() == category)
                .flat_map(|c| c.shown(filtering))
                .any(|entry| book.highlight.contains(&entry.id));
            if highlighted {
                self.tab_animation_started.insert(category, now);
            }
        }
    }

    /// Vanilla `updateButtonsForPage` / `RecipeButton.init`: resolves the
    /// page's buttons and marks their highlighted recipes seen, pulsing the
    /// buttons that showed one.
    fn init_page_buttons(&mut self, book: &mut RecipeBookState, sender: &PacketSender) {
        let Some(view) = &mut self.view else {
            return;
        };
        let page = (self.page, view.shown_generation);
        if view.initialized_page == Some(page) {
            return;
        }
        view.initialized_page = Some(page);
        let filtering = book.settings.get(view.spec.kind).filtering;
        let now = Instant::now();
        let start = self.page * ITEMS_PER_PAGE;
        let mut buttons = Vec::new();
        for (index, &collection) in view
            .shown
            .iter()
            .skip(start)
            .take(ITEMS_PER_PAGE)
            .enumerate()
        {
            let collection = &view.collections[collection];
            let entries = collection
                .shown(filtering)
                .into_iter()
                .map(|entry| (entry.id, display_result_stacks(&entry.display, book)))
                .collect::<Vec<_>>();
            let newly_shown = entries
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| book.highlight.contains(id))
                .collect::<Vec<_>>();
            if !newly_shown.is_empty() {
                for id in newly_shown {
                    book.mark_seen(id);
                    let _ = sender.recipe_book_seen_recipe(id);
                }
                self.button_animation_started[index] = Some(now);
            }
            buttons.push(PageButton {
                same_result: all_recipes_have_same_result(&entries),
                craftable: collection.has_craftable(),
                entries,
            });
        }
        view.page_buttons = buttons;
    }

    /// Vanilla `fillGhostRecipe`: resolves the ghost slots once per
    /// placement, not per frame.
    fn refresh_ghost(
        &mut self,
        book: &RecipeBookState,
        spec: RecipeBookScreenSpec,
        slots: &[ItemStack],
    ) {
        let current = book
            .ghost_recipe
            .as_ref()
            .filter(|ghost| ghost.container_id == spec.container_id);
        if self.ghost.as_ref().map(|g| &g.recipe) == current {
            return;
        }
        self.ghost = current.map(|recipe| GhostView {
            recipe: recipe.clone(),
            slots: resolve_ghost_slots(&recipe.recipe, spec, book, slots),
        });
    }
}

impl Default for RecipeBookUiState {
    fn default() -> Self {
        Self::new()
    }
}

/// The book's derived data, rebuilt only on the changes vanilla reacts to.
struct RecipeBookView {
    revision: u64,
    spec: RecipeBookScreenSpec,
    available: HashMap<u32, u32>,
    collections: Vec<Collection>,
    /// Per collection, every recipe result's search text, built on the first
    /// search (`SessionSearchTrees.updateRecipes`).
    search_index: Option<Vec<Vec<SearchText>>>,
    /// Tabs with any fitting recipe (`updateTabs`).
    tabs: Vec<TabInfo>,
    shown_key: Option<ShownKey>,
    /// Indices into `collections` the selected tab, search and filter show.
    shown: Vec<usize>,
    /// Bumped whenever `shown` is recomputed, so the page buttons re-init.
    shown_generation: u64,
    /// `(page, shown_generation)` the current `page_buttons` were built for.
    initialized_page: Option<(usize, u64)>,
    page_buttons: Vec<PageButton>,
}

impl RecipeBookView {
    fn pages(&self) -> usize {
        self.shown.len().div_ceil(ITEMS_PER_PAGE).max(1)
    }

    /// Vanilla `updateCollections`: the selected tab's collections with any
    /// fitting recipe, narrowed by the search and the craftable filter.
    fn update_shown(&mut self, key: &ShownKey, book: &RecipeBookState) {
        let search = if key.search.is_empty() {
            None
        } else {
            Some(self.search_index.get_or_insert_with(|| {
                self.collections
                    .iter()
                    .map(|collection| collection_search_text(collection, book))
                    .collect()
            }))
        };
        self.shown = self
            .collections
            .iter()
            .enumerate()
            .filter(|(_, collection)| collection.in_tab(key.category))
            .filter(|(_, collection)| !collection.shown(key.filtering).is_empty())
            .filter(|(index, _)| {
                search.as_ref().is_none_or(|search| {
                    search[*index].iter().any(|text| text.matches(&key.search))
                })
            })
            .map(|(index, _)| index)
            .collect();
        self.shown_generation += 1;
    }
}

#[derive(PartialEq)]
struct ShownKey {
    category: Option<u32>,
    /// Lowercased, as vanilla searches.
    search: String,
    filtering: bool,
}

/// One page button's resolved recipes (`RecipeButton.selectedEntries`).
struct PageButton {
    entries: Vec<(RecipeDisplayId, Vec<ResolvedSlotStack>)>,
    same_result: bool,
    craftable: bool,
}

struct GhostView {
    recipe: crate::recipe::GhostRecipe,
    slots: Vec<GhostSlot>,
}

struct GhostSlot {
    /// GUI units from the main panel's origin.
    pos: (f32, f32),
    role: GhostSlotRole,
    items: Vec<ResolvedSlotStack>,
}

fn animation_squeeze(started: Instant, now: Instant) -> Option<f32> {
    let elapsed = now.saturating_duration_since(started);
    if elapsed >= HIGHLIGHT_ANIMATION {
        return None;
    }
    let t = elapsed.as_secs_f32() / HIGHLIGHT_ANIMATION.as_secs_f32();
    Some(1.0 + 0.1 * (std::f32::consts::PI * t).sin())
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
    /// Per recipe, its furnace flag and ingredient grid resolved at open
    /// (`OverlayRecipeComponent.init`).
    recipes: Vec<OverlayRecipe>,
}

#[derive(Clone, Debug)]
struct OverlayRecipe {
    furnace: bool,
    ingredients: Vec<((usize, usize), Vec<ResolvedSlotStack>)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecipeBookFrame {
    pub visible: bool,
    pub narrow: bool,
    pub main_x_offset: f32,
    /// GUI scale of the main panel, for drawing the carried stack on top.
    pub panel_scale: f32,
    pub consumed_left_click: bool,
    pub consumed_right_click: bool,
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

/// Screen rects of the book's widgets (vanilla `RecipeBookComponent` and
/// `RecipeBookPage` positions), shared by input and rendering.
struct BookLayout {
    x: f32,
    y: f32,
    scale: f32,
}

impl BookLayout {
    fn new(screen_w: f32, screen_h: f32, gs: f32, narrow: bool) -> Self {
        let (x, y, scale) = book_origin(screen_w, screen_h, gs, narrow);
        Self { x, y, scale }
    }

    /// `[x, y, w, h]` in book GUI units, to screen pixels.
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        [
            self.x + x * self.scale,
            self.y + y * self.scale,
            w * self.scale,
            h * self.scale,
        ]
    }

    fn book(&self) -> [f32; 4] {
        self.rect(0.0, 0.0, BOOK_W, BOOK_H)
    }

    fn search(&self) -> [f32; 4] {
        self.rect(SEARCH_X, SEARCH_Y, SEARCH_W, SEARCH_H)
    }

    /// `magnifierIconPlacement`: from x+8 up to the search box's left edge.
    fn magnifier(&self) -> [f32; 4] {
        self.rect(8.0, SEARCH_Y, SEARCH_X, SEARCH_H)
    }

    fn filter(&self) -> [f32; 4] {
        self.rect(FILTER_X, FILTER_Y, FILTER_W, FILTER_H)
    }

    fn tab(&self, index: usize) -> [f32; 4] {
        self.rect(-30.0, 3.0 + 27.0 * index as f32, 35.0, 27.0)
    }

    fn button(&self, index: usize) -> [f32; 4] {
        self.rect(
            11.0 + 25.0 * (index % 5) as f32,
            31.0 + 25.0 * (index / 5) as f32,
            25.0,
            25.0,
        )
    }

    fn back(&self) -> [f32; 4] {
        self.rect(38.0, 137.0, 12.0, 17.0)
    }

    fn forward(&self) -> [f32; 4] {
        self.rect(93.0, 137.0, 12.0, 17.0)
    }
}

fn toggle_rect(spec: RecipeBookScreenSpec, main: &MainPanel) -> [f32; 4] {
    [
        main.x + spec.toggle_x * main.scale,
        main.y + spec.toggle_y * main.scale,
        20.0 * main.scale,
        18.0 * main.scale,
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Click {
    Left,
    Right,
}

/// What a click on the visible book did (vanilla `mouseClicked`).
enum BookClick {
    Ignored,
    Consumed,
    /// A recipe button or overlay recipe was left-clicked.
    Recipe(RecipeDisplayId),
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
    state.prepare(book, spec, slots);
    state.refresh_ghost(book, spec, slots);

    let click = if left_clicked {
        Some(Click::Left)
    } else if right_clicked {
        Some(Click::Right)
    } else {
        None
    };
    let mut consumed = false;
    let layout = BookLayout::new(screen_w, screen_h, gs, narrow);
    if let Some(click) = click
        && visible(book, spec.kind)
    {
        match book_click(
            state,
            book,
            sender,
            spec,
            &layout,
            cursor,
            click,
            shift,
            text_width_fn,
        ) {
            BookClick::Recipe(id) => {
                if state.try_place_recipe(book, sender, spec, id, shift) {
                    state.last_clicked_recipe = Some(id);
                    if narrow {
                        state.set_book_open(book, sender, spec.kind, false);
                    }
                    consumed = true;
                }
            }
            BookClick::Consumed => consumed = true,
            BookClick::Ignored => {}
        }
        // `AbstractRecipeBookScreen.mouseClicked`: a narrow screen's open
        // book swallows every click.
        consumed |= narrow;
    }

    let main = main_panel(
        spec,
        screen_w,
        screen_h,
        gs,
        main_x_offset(book, spec.kind, screen_w, gs),
    );
    if !consumed && click == Some(Click::Left) && hit_test(cursor, toggle_rect(spec, &main)) {
        let open = !visible(book, spec.kind);
        state.set_book_open(book, sender, spec.kind, open);
        consumed = true;
    }

    if state.search_focused {
        let fs = FONT_SIZE * layout.scale;
        let wf = |s: &str| text_width_fn(s, fs);
        let mut clipboard = SystemClipboard;
        for event in text_events {
            state
                .search
                .handle(event, &mut clipboard, (SEARCH_W - 8.0) * layout.scale, &wf);
        }
    }

    if selection_key
        && visible(book, spec.kind)
        && let Some(id) = state.last_clicked_recipe
    {
        state.try_place_recipe(book, sender, spec, id, shift);
    }

    // Book changes above (search, tab, filter, page, visibility) take effect
    // this frame.
    state.prepare(book, spec, slots);
    state.init_page_buttons(book, sender);

    let visible = visible(book, spec.kind);
    let main_x_offset = main_x_offset(book, spec.kind, screen_w, gs);
    RecipeBookFrame {
        visible,
        narrow,
        main_x_offset,
        panel_scale: main_panel(spec, screen_w, screen_h, gs, main_x_offset).scale,
        consumed_left_click: consumed && click == Some(Click::Left),
        consumed_right_click: consumed && click == Some(Click::Right),
    }
}

/// Vanilla `RecipeBookComponent.mouseClicked`: the page (overlay, arrows,
/// buttons) first, then the search box, filter and tabs.
#[allow(clippy::too_many_arguments)]
fn book_click(
    state: &mut RecipeBookUiState,
    book: &mut RecipeBookState,
    sender: &PacketSender,
    spec: RecipeBookScreenSpec,
    layout: &BookLayout,
    cursor: (f32, f32),
    click: Click,
    shift: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> BookClick {
    let left = click == Click::Left;
    let view = state.view.as_ref().expect("prepared view");

    // `RecipeBookPage.mouseClicked`.
    if let Some(overlay) = &state.overlay {
        let picked = left
            .then(|| hit_overlay_recipe(cursor, overlay, layout))
            .flatten();
        if picked.is_none() {
            state.overlay = None;
        }
        return picked.map_or(BookClick::Consumed, BookClick::Recipe);
    }
    let pages = view.pages();
    if left && state.page + 1 < pages && hit_test(cursor, layout.forward()) {
        state.page += 1;
        return BookClick::Consumed;
    }
    if left && state.page > 0 && hit_test(cursor, layout.back()) {
        state.page -= 1;
        return BookClick::Consumed;
    }
    let cycle = state.cycle_index();
    let filtering = book.settings.get(spec.kind).filtering;
    for (index, button) in view.page_buttons.iter().enumerate() {
        if !hit_test(cursor, layout.button(index)) {
            continue;
        }
        if left {
            return BookClick::Recipe(button.entries[cycle % button.entries.len()].0);
        }
        if button.entries.len() > 1 {
            let collection = &view.collections[view.shown[state.page * ITEMS_PER_PAGE + index]];
            state.overlay = Some(open_overlay(collection, filtering, index, book));
        }
        return BookClick::Consumed;
    }

    // The magnifier focuses search for any button, without moving the
    // cursor; the box itself takes left clicks.
    if hit_test(cursor, layout.magnifier()) {
        state.set_search_focus(true);
        return BookClick::Consumed;
    }
    let search = layout.search();
    if left && hit_test(cursor, search) {
        state.set_search_focus(true);
        let scale = layout.scale;
        let fs = FONT_SIZE * scale;
        let wf = |s: &str| text_width_fn(s, fs);
        let inner_w = (SEARCH_W - 8.0) * scale;
        let rel = (cursor.0 - (search[0] + 4.0 * scale)).max(0.0);
        let pos = state.search.pos_from_click(rel, inner_w, &wf);
        state.search.on_click(pos, shift, inner_w, &wf);
        return BookClick::Consumed;
    }
    state.set_search_focus(false);

    if left && hit_test(cursor, layout.filter()) {
        let mut settings = book.settings.get(spec.kind);
        settings.filtering = !settings.filtering;
        book.settings.set(spec.kind, settings);
        let _ = sender.recipe_book_change_settings(spec.kind, settings.open, settings.filtering);
        return BookClick::Consumed;
    }
    if left {
        let view = state.view.as_ref().expect("prepared view");
        if let Some(tab) = (0..view.tabs.len()).find(|&i| hit_test(cursor, layout.tab(i))) {
            let category = view.tabs[tab].category;
            if state.selected_tab != category {
                state.selected_tab = category;
                state.page = 0;
            }
            return BookClick::Consumed;
        }
    }
    BookClick::Ignored
}

/// Vanilla `OverlayRecipeComponent.init`: craftable recipes first, then
/// (unless filtering) the uncraftable ones, ingredients resolved once.
fn open_overlay(
    collection: &Collection,
    filtering: bool,
    button_index: usize,
    book: &RecipeBookState,
) -> RecipeOverlay {
    let fitting = collection.shown(false);
    let (craftable, uncraftable): (Vec<_>, Vec<_>) = fitting
        .into_iter()
        .partition(|entry| collection.craftable.contains(&entry.id));
    let craftable_count = craftable.len();
    let entries = craftable
        .into_iter()
        .chain(uncraftable.into_iter().filter(|_| !filtering))
        .collect::<Vec<_>>();
    RecipeOverlay {
        ids: entries.iter().map(|entry| entry.id).collect(),
        craftable_count,
        button_index,
        recipes: entries
            .iter()
            .map(|entry| OverlayRecipe {
                furnace: matches!(entry.display, RecipeDisplay::Furnace { .. }),
                ingredients: overlay_ingredients(&entry.display)
                    .into_iter()
                    .map(|(pos, slot)| (pos, slot_stacks(slot, book)))
                    .collect(),
            })
            .collect(),
    }
}

/// A vanilla lang string, or `fallback` when the key is missing.
fn lang_or(key: &str, fallback: &str) -> String {
    crate::lang::translate(key).unwrap_or(fallback).to_owned()
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum GhostSlotRole {
    Ingredient,
    /// `big` is the 24px result box of crafting screens (`isResultSlotBig`).
    Result {
        big: bool,
    },
}

/// The container panel the book sits beside, in screen pixels.
struct MainPanel {
    x: f32,
    y: f32,
    scale: f32,
}

fn main_panel(
    spec: RecipeBookScreenSpec,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    x_offset: f32,
) -> MainPanel {
    let scale = super::container::panel_scale(screen_w, screen_h, gs, 176.0, spec.panel_h);
    MainPanel {
        x: (screen_w - 176.0 * scale) / 2.0 + x_offset * scale,
        y: (screen_h - spec.panel_h * scale) / 2.0,
        scale,
    }
}

fn book_origin(screen_w: f32, screen_h: f32, gs: f32, narrow: bool) -> (f32, f32, f32) {
    let scale = super::container::panel_scale(screen_w, screen_h, gs, BOOK_W, BOOK_H);
    let x_offset = if narrow { 0.0 } else { 86.0 };
    let bx = (screen_w - BOOK_W * scale) / 2.0 - x_offset * scale;
    let by = (screen_h - BOOK_H * scale) / 2.0;
    (bx, by, scale)
}

/// Tabs with any recipe that fits this screen (`updateVisibility`); the
/// search tab always shows.
fn visible_tabs(tabs: &[TabInfo], collections: &[Collection]) -> Vec<TabInfo> {
    tabs.iter()
        .copied()
        .filter(|tab| {
            tab.category
                .is_none_or(|category| collections.iter().any(|c| c.category() == category))
        })
        .collect()
}

/// Vanilla `ClientRecipeBook.rebuildCollections` for this screen's book:
/// recipes grouped by (category, group) in `values()` order, search tabs
/// concatenating categories in tab order. Collections with nothing that
/// fits this screen never show (`hasAnySelected`), so they're dropped.
fn resolve_ghost_slots(
    display: &RecipeDisplay,
    spec: RecipeBookScreenSpec,
    book: &RecipeBookState,
    slots: &[ItemStack],
) -> Vec<GhostSlot> {
    let slot = |pos: (f32, f32), role: GhostSlotRole, display: &SlotDisplay| GhostSlot {
        pos,
        role,
        items: slot_stacks(display, book),
    };
    let result = GhostSlotRole::Result {
        big: spec.big_result_slot,
    };
    let mut out = Vec::new();
    match (display, spec.kind) {
        (
            RecipeDisplay::Shaped {
                result: result_display,
                ..
            }
            | RecipeDisplay::Shapeless {
                result: result_display,
                ..
            },
            RecipeBookType::Crafting,
        ) => {
            let (grid_x, grid_y, result_x, result_y) = crafting_geometry(spec);
            out.push(slot((result_x, result_y), result, result_display));
            for (grid_index, ingredient) in
                crafting_grid_slots(display, spec.grid_width, spec.grid_height)
            {
                let col = (grid_index % spec.grid_width) as f32;
                let row = (grid_index / spec.grid_width) as f32;
                out.push(slot(
                    (grid_x + col * 18.0, grid_y + row * 18.0),
                    GhostSlotRole::Ingredient,
                    ingredient,
                ));
            }
        }
        (
            RecipeDisplay::Furnace {
                ingredient,
                fuel,
                result: result_display,
                ..
            },
            RecipeBookType::Furnace | RecipeBookType::BlastFurnace | RecipeBookType::Smoker,
        ) => {
            out.push(slot((116.0, 35.0), result, result_display));
            out.push(slot((56.0, 17.0), GhostSlotRole::Ingredient, ingredient));
            if slots.get(1).is_none_or(ItemStack::is_empty) {
                out.push(slot((56.0, 53.0), GhostSlotRole::Ingredient, fuel));
            }
        }
        _ => {}
    }
    out.retain(|slot| !slot.items.is_empty());
    out
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;
