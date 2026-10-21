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

use super::common::{
    FONT_SIZE, WHITE, hit_test, push_item_decorations, push_item_glyph, push_tooltip,
    push_tooltip_lines,
};
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

#[derive(Clone)]
struct Collection {
    /// Every recipe of the group (`RecipeCollection.getRecipes`).
    entries: Vec<RecipeBookEntry>,
    /// Recipes that fit this screen (vanilla's `selected`).
    fitting: HashSet<RecipeDisplayId>,
    craftable: HashSet<RecipeDisplayId>,
}

impl Collection {
    fn category(&self) -> u32 {
        self.entries[0].category
    }

    fn in_tab(&self, category: Option<u32>) -> bool {
        category.is_none_or(|category| self.category() == category)
    }

    /// Vanilla `getSelectedRecipes(CRAFTABLE)` when filtering, else `ANY`.
    fn shown(&self, filtering: bool) -> Vec<&RecipeBookEntry> {
        self.entries
            .iter()
            .filter(|entry| self.fitting.contains(&entry.id))
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

#[allow(clippy::too_many_arguments)]
pub fn render(
    elements: &mut Vec<MenuElement>,
    state: &mut RecipeBookUiState,
    book: &RecipeBookState,
    spec: RecipeBookScreenSpec,
    frame: RecipeBookFrame,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    cursor: (f32, f32),
    ctrl_held: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    state.update_cycle_time(ctrl_held || !frame.visible);
    let main = main_panel(spec, screen_w, screen_h, gs, frame.main_x_offset);
    // A narrow screen's open book hides the screen; otherwise the ghost and
    // the toggle (a screen widget) draw below the book's stratum.
    if !(frame.visible && frame.narrow) {
        render_ghost_recipe(elements, state, &main, cursor, screen_w, screen_h);
        render_toggle(elements, spec, &main, cursor);
    }
    if !frame.visible {
        return;
    }

    let layout = BookLayout::new(screen_w, screen_h, gs, frame.narrow);
    let scale = layout.scale;
    let book_rect = layout.book();
    elements.push(MenuElement::Image {
        x: book_rect[0],
        y: book_rect[1],
        w: book_rect[2],
        h: book_rect[3],
        sprite: SpriteId::RecipeBookBackground,
        tint: WHITE,
    });
    render_search(elements, state, &layout, text_width_fn);
    render_filter(
        elements, book, spec.kind, &layout, cursor, screen_w, screen_h,
    );
    render_tabs(elements, state, &layout);

    let Some(view) = &state.view else {
        return;
    };
    let now = Instant::now();
    let cycle = state.cycle_index();
    let overlay_open = state.overlay.is_some();
    for (index, button) in view.page_buttons.iter().enumerate() {
        let rect = layout.button(index);
        let squeeze = state.button_animation_started[index]
            .and_then(|started| animation_squeeze(started, now))
            .unwrap_or(1.0);
        let pivot = (rect[0] + 8.0 * scale, rect[1] + 12.0 * scale);
        let scaled = |x: f32, y: f32, size: f32| {
            scale_rect_about(
                [
                    rect[0] + x * scale,
                    rect[1] + y * scale,
                    size * scale,
                    size * scale,
                ],
                pivot,
                (squeeze, squeeze),
            )
        };
        let multiple = button.entries.len() > 1;
        let sprite = match (button.craftable, multiple) {
            (true, false) => SpriteId::RecipeBookSlotCraftable,
            (true, true) => SpriteId::RecipeBookSlotManyCraftable,
            (false, false) => SpriteId::RecipeBookSlotUncraftable,
            (false, true) => SpriteId::RecipeBookSlotManyUncraftable,
        };
        let slot = scaled(0.0, 0.0, 25.0);
        elements.push(MenuElement::Image {
            x: slot[0],
            y: slot[1],
            w: slot[2],
            h: slot[3],
            sprite,
            tint: WHITE,
        });
        // `RecipeButton.getDisplayStack`.
        let (_, items) = &button.entries[cycle % button.entries.len()];
        let Some(item) =
            (!items.is_empty()).then(|| &items[(cycle / button.entries.len()) % items.len()])
        else {
            continue;
        };
        let offsets: &[f32] = if multiple && button.same_result {
            &[5.0, 3.0]
        } else {
            &[4.0]
        };
        for &offset in offsets {
            let icon = scaled(offset, offset, 16.0);
            push_book_stack(elements, icon[0], icon[1], icon[2], item);
        }
        if !overlay_open && hit_test(cursor, rect) {
            let mut lines = stack_tooltip_lines(item);
            if multiple {
                lines.push(TooltipLine::new(
                    lang_or("gui.recipebook.moreRecipes", "Right Click for More"),
                    WHITE,
                ));
            }
            push_tooltip_lines(elements, cursor, screen_w, screen_h, scale, lines);
        }
    }

    let pages = view.pages();
    if pages > 1 {
        elements.push(MenuElement::Text {
            x: layout.x + 73.0 * scale,
            y: layout.y + 141.0 * scale,
            text: lang_or("gui.recipebook.page", "%s/%s")
                .replacen("%s", &(state.page + 1).to_string(), 1)
                .replacen("%s", &pages.to_string(), 1),
            scale: FONT_SIZE * scale,
            color: WHITE,
            centered: true,
        });
        let arrows = [
            (
                state.page > 0,
                layout.back(),
                SpriteId::RecipeBookPageBackward,
                SpriteId::RecipeBookPageBackwardHighlighted,
                ("gui.recipebook.previous_page", "Previous Page"),
            ),
            (
                state.page + 1 < pages,
                layout.forward(),
                SpriteId::RecipeBookPageForward,
                SpriteId::RecipeBookPageForwardHighlighted,
                ("gui.recipebook.next_page", "Next Page"),
            ),
        ];
        for (shown, rect, sprite, hovered_sprite, (key, fallback)) in arrows {
            if !shown {
                continue;
            }
            let hovered = hit_test(cursor, rect);
            elements.push(MenuElement::Image {
                x: rect[0],
                y: rect[1],
                w: rect[2],
                h: rect[3],
                sprite: if hovered { hovered_sprite } else { sprite },
                tint: WHITE,
            });
            if hovered {
                push_tooltip(
                    elements,
                    cursor,
                    screen_w,
                    screen_h,
                    scale,
                    &lang_or(key, fallback),
                );
            }
        }
    }

    if let Some(overlay) = &state.overlay {
        render_overlay(elements, overlay, &layout, cursor, cycle);
    }
}

fn render_toggle(
    elements: &mut Vec<MenuElement>,
    spec: RecipeBookScreenSpec,
    main: &MainPanel,
    cursor: (f32, f32),
) {
    let rect = toggle_rect(spec, main);
    elements.push(MenuElement::Image {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite: if hit_test(cursor, rect) {
            SpriteId::RecipeBookButtonHighlighted
        } else {
            SpriteId::RecipeBookButton
        },
        tint: WHITE,
    });
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

fn direct_trim_pattern_name(pattern: &TrimPatternHolder) -> String {
    match pattern {
        TrimPatternHolder::Direct { description, .. } => description.plain_text(),
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

/// Most stacks one slot display resolves to. Vanilla has no limit, but its
/// data never comes close; nested `with_any_potion` multiplies by every
/// potion per level, so hostile data would otherwise exhaust memory.
const MAX_RESOLVED_STACKS: usize = 4096;

fn slot_stacks(slot: &SlotDisplay, book: &RecipeBookState) -> Vec<ResolvedSlotStack> {
    let basic = |id: u32| {
        ItemKind::from_u32(id).map(|kind| ResolvedSlotStack::plain(ItemStackData::new(kind, 1)))
    };
    match slot {
        SlotDisplay::Empty => Vec::new(),
        SlotDisplay::AnyFuel => vanilla_fuel_items(book.item_tags())
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
            .item_tags()
            .ordered(tag)
            .into_iter()
            .flatten()
            .copied()
            .filter_map(basic)
            .collect(),
        SlotDisplay::Composite(parts) => parts
            .iter()
            .flat_map(|part| slot_stacks(part, book))
            .take(MAX_RESOLVED_STACKS)
            .collect(),
        SlotDisplay::WithRemainder { input, .. } => slot_stacks(input, book),
        SlotDisplay::WithAnyPotion(inner) => {
            let base = slot_stacks(inner, book);
            let mut out = Vec::new();
            'potions: for potion in (0..).map_while(Potion::from_u32) {
                for stack in &base {
                    if out.len() == MAX_RESOLVED_STACKS {
                        break 'potions;
                    }
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

fn push_book_stack(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    stack: &ResolvedSlotStack,
) {
    push_item_glyph(
        elements,
        [x, y, size, size],
        crate::player::inventory::item_resource_name(stack.stack.kind),
        Some(&stack.stack),
        true,
    );
}

/// A tab icon (vanilla `fakeItem` of the icon's default stack).
fn push_tab_icon(elements: &mut Vec<MenuElement>, rect: [f32; 4], icon: &str) {
    let name = icon.strip_prefix("minecraft:").unwrap_or(icon).to_owned();
    push_item_glyph(elements, rect, name, None, true);
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
fn build_collections(book: &RecipeBookState, spec: RecipeBookScreenSpec) -> Vec<Collection> {
    let mut grouped = Vec::<(usize, Vec<RecipeBookEntry>)>::new();
    let mut grouped_index = HashMap::<(u32, u32), usize>::new();
    for entry in book.known_in_vanilla_order() {
        let Some(rank) = category_rank(spec.kind, entry.category) else {
            continue;
        };
        match entry.group {
            Some(group) => match grouped_index.entry((entry.category, group)) {
                std::collections::hash_map::Entry::Occupied(slot) => {
                    grouped[*slot.get()].1.push(entry.clone());
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(grouped.len());
                    grouped.push((rank, vec![entry.clone()]));
                }
            },
            None => grouped.push((rank, vec![entry.clone()])),
        }
    }
    grouped.sort_by_key(|(rank, _)| *rank);
    grouped
        .into_iter()
        .map(|(_, entries)| Collection {
            fitting: entries
                .iter()
                .filter(|entry| can_display(spec, &entry.display))
                .map(|entry| entry.id)
                .collect(),
            craftable: HashSet::new(),
            entries,
        })
        .filter(|collection| !collection.fitting.is_empty())
        .collect()
}

/// Vanilla `selectMatchingRecipes`: a fitting recipe is craftable when the
/// available items cover its requirements.
fn select_matching_recipes(
    collections: &mut [Collection],
    available: &HashMap<u32, u32>,
    tags: &ItemTags,
) {
    for collection in collections {
        collection.craftable = collection
            .entries
            .iter()
            .filter(|entry| collection.fitting.contains(&entry.id))
            .filter(|entry| can_craft(entry, available, tags))
            .map(|entry| entry.id)
            .collect();
    }
}

/// Position of `category` among `kind`'s tabs, which is also vanilla's
/// search-tab concatenation order (`SearchRecipeBookCategory`); `None` for
/// another book's category.
fn category_rank(kind: RecipeBookType, category: u32) -> Option<usize> {
    tabs_for(kind)
        .iter()
        .filter_map(|tab| tab.category)
        .position(|candidate| candidate == category)
}

/// One recipe result's search text (`SessionSearchTrees`): its tooltip
/// lines and item id path, lowercased.
struct SearchText {
    path: String,
    lines: Vec<String>,
}

impl SearchText {
    /// Vanilla `FullTextSearchTree.search`: plain text matches tooltip
    /// lines; `namespace:path` intersects the namespace with path-or-name.
    fn matches(&self, needle: &str) -> bool {
        let contains = |needle: &str| self.lines.iter().any(|line| line.contains(needle));
        match needle.split_once(':') {
            Some((namespace, path)) => {
                let path = path.trim();
                "minecraft".contains(namespace.trim())
                    && (self.path.contains(path) || contains(path))
            }
            None => contains(needle),
        }
    }
}

/// Search text for every result of every recipe in `collection`, not just
/// the fitting ones (`collection.getRecipes()`).
fn collection_search_text(collection: &Collection, book: &RecipeBookState) -> Vec<SearchText> {
    collection
        .entries
        .iter()
        .flat_map(|entry| display_result_stacks(&entry.display, book))
        .map(|stack| SearchText {
            path: crate::player::inventory::item_resource_name(stack.stack.kind).to_lowercase(),
            lines: stack_tooltip_lines(&stack)
                .into_iter()
                .map(|line| strip_formatting(&line_text(line)).trim().to_lowercase())
                .filter(|line| !line.is_empty())
                .collect(),
        })
        .collect()
}

fn line_text(line: TooltipLine) -> String {
    line.spans.into_iter().map(|span| span.text).collect()
}

/// Vanilla `ChatFormatting.stripFormatting`: drops `§` codes.
fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§'
            && chars.peek().is_some_and(
                |code| matches!(code.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r'),
            )
        {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Vanilla `RecipeDisplayEntry.canCraft`: needs requirements, each taking one
/// available item.
fn can_craft(entry: &RecipeBookEntry, available: &HashMap<u32, u32>, tags: &ItemTags) -> bool {
    let Some(requirements) = entry.crafting_requirements.as_ref() else {
        return false;
    };
    let options = requirements
        .iter()
        .map(|ingredient| ingredient_items(ingredient, tags).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    can_match(&options, available)
}

/// Vanilla `StackedContents.RecipePicker.tryPick(1)`: whether every
/// ingredient gets one of its items, no item used beyond its count. Bipartite
/// matching by augmenting paths, polynomial where trying every assignment is
/// exponential.
fn can_match(options: &[Vec<u32>], available: &HashMap<u32, u32>) -> bool {
    let mut holders = HashMap::<u32, Vec<usize>>::new();
    (0..options.len()).all(|ingredient| {
        augment(
            ingredient,
            options,
            available,
            &mut holders,
            &mut HashSet::new(),
        )
    })
}

/// Gives `ingredient` an item, moving earlier holders elsewhere if needed.
fn augment(
    ingredient: usize,
    options: &[Vec<u32>],
    available: &HashMap<u32, u32>,
    holders: &mut HashMap<u32, Vec<usize>>,
    visited: &mut HashSet<u32>,
) -> bool {
    for &item in &options[ingredient] {
        let capacity = available.get(&item).copied().unwrap_or(0) as usize;
        if capacity == 0 || !visited.insert(item) {
            continue;
        }
        let held = holders.entry(item).or_default();
        if held.len() < capacity {
            held.push(ingredient);
            return true;
        }
        for slot in 0..held.len() {
            let other = holders[&item][slot];
            if augment(other, options, available, holders, visited) {
                holders.get_mut(&item).expect("item has holders")[slot] = ingredient;
                return true;
            }
        }
    }
    false
}

/// Vanilla `RecipeButton.allRecipesHaveSameResultDisplay`.
fn all_recipes_have_same_result(entries: &[(RecipeDisplayId, Vec<ResolvedSlotStack>)]) -> bool {
    let mut items = entries.iter().flat_map(|(_, items)| items);
    let Some(first) = items.next() else {
        return true;
    };
    items.all(|item| first.same_item_and_components(item))
}

/// Vanilla `fillGhostRecipe` for each screen: slot positions in main-panel
/// GUI units with their resolved alternatives. The fuel ghost only fills an
/// empty fuel slot.
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

fn render_ghost_recipe(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    main: &MainPanel,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let Some(ghost) = &state.ghost else {
        return;
    };
    let scale = main.scale;
    let cycle = state.cycle_index();
    for slot in &ghost.slots {
        let x = main.x + slot.pos.0 * scale;
        let y = main.y + slot.pos.1 * scale;
        let item = &slot.items[cycle % slot.items.len()];
        let red = ghost_highlight_rect(
            x,
            y,
            scale,
            slot.role == GhostSlotRole::Result { big: true },
        );
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
        if matches!(slot.role, GhostSlotRole::Result { .. }) {
            push_item_decorations(elements, x, y, 16.0 * scale, scale, &item.stack);
        }
        // The hovered slot's area (`isHovering`): one pixel around the item.
        if hit_test(cursor, [x - scale, y - scale, 18.0 * scale, 18.0 * scale]) {
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
}

fn render_search(
    elements: &mut Vec<MenuElement>,
    state: &RecipeBookUiState,
    layout: &BookLayout,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let scale = layout.scale;
    let [x, y, w, h] = layout.search();
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
        // `EditBox` picks its sprite by focus alone.
        sprite: if state.search_focused {
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
            lang_or("gui.recipebook.search_hint", "Search..."),
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

fn render_filter(
    elements: &mut Vec<MenuElement>,
    book: &RecipeBookState,
    kind: RecipeBookType,
    layout: &BookLayout,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
) {
    let rect = layout.filter();
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
        let (key, fallback) = if !filtering {
            ("gui.recipebook.toggleRecipes.all", "Showing All")
        } else {
            match kind {
                RecipeBookType::Crafting => (
                    "gui.recipebook.toggleRecipes.craftable",
                    "Showing Craftable",
                ),
                RecipeBookType::Furnace => (
                    "gui.recipebook.toggleRecipes.smeltable",
                    "Showing Smeltable",
                ),
                RecipeBookType::BlastFurnace => (
                    "gui.recipebook.toggleRecipes.blastable",
                    "Showing Blastable",
                ),
                RecipeBookType::Smoker => {
                    ("gui.recipebook.toggleRecipes.smokable", "Showing Smokable")
                }
            }
        };
        push_tooltip(
            elements,
            cursor,
            screen_w,
            screen_h,
            layout.scale,
            &lang_or(key, fallback),
        );
    }
}

fn render_tabs(elements: &mut Vec<MenuElement>, state: &RecipeBookUiState, layout: &BookLayout) {
    let Some(view) = &state.view else {
        return;
    };
    let scale = layout.scale;
    let now = Instant::now();
    for (index, tab) in view.tabs.iter().enumerate() {
        let selected = tab.category == state.selected_tab;
        let squeeze = tab
            .category
            .and_then(|category| state.tab_animation_started.get(&category))
            .and_then(|started| animation_squeeze(*started, now))
            .unwrap_or(1.0);
        let base = layout.tab(index);
        let shift = if selected { -2.0 * scale } else { 0.0 };
        let pivot = (base[0] + 8.0 * scale, base[1] + 12.0 * scale);
        let tab_rect = scale_rect_about(
            [base[0] + shift, base[1], base[2], base[3]],
            pivot,
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
        let icon_y = pivot.1 + (base[1] + 5.0 * scale - pivot.1) * squeeze;
        let icon_rect = |x: f32| {
            [
                base[0] + shift + x * scale,
                icon_y,
                16.0 * scale,
                16.0 * scale * squeeze,
            ]
        };
        if let Some(icon_b) = tab.icon_b {
            push_tab_icon(elements, icon_rect(3.0), tab.icon_a);
            push_tab_icon(elements, icon_rect(14.0), icon_b);
        } else {
            push_tab_icon(elements, icon_rect(9.0), tab.icon_a);
        }
    }
}

/// The popup's columns: 4 for up to 16 recipes, else 5.
fn overlay_columns(overlay: &RecipeOverlay) -> usize {
    if overlay.ids.len() <= 16 { 4 } else { 5 }
}

fn overlay_rect_for(overlay: &RecipeOverlay, layout: &BookLayout) -> [f32; 4] {
    let total = overlay.ids.len();
    let max_row = overlay_columns(overlay);
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
    layout.rect(x, y, (cols * 25 + 8) as f32, (rows * 25 + 8) as f32)
}

/// The popup's `index`th recipe button.
fn overlay_button_rect(
    overlay: &RecipeOverlay,
    popup: [f32; 4],
    scale: f32,
    index: usize,
) -> [f32; 4] {
    let max_row = overlay_columns(overlay);
    [
        popup[0] + (4.0 + 25.0 * (index % max_row) as f32) * scale,
        popup[1] + (5.0 + 25.0 * (index / max_row) as f32) * scale,
        24.0 * scale,
        24.0 * scale,
    ]
}

fn hit_overlay_recipe(
    cursor: (f32, f32),
    overlay: &RecipeOverlay,
    layout: &BookLayout,
) -> Option<RecipeDisplayId> {
    let popup = overlay_rect_for(overlay, layout);
    (0..overlay.ids.len())
        .find(|&index| {
            hit_test(
                cursor,
                overlay_button_rect(overlay, popup, layout.scale, index),
            )
        })
        .map(|index| overlay.ids[index])
}

fn render_overlay(
    elements: &mut Vec<MenuElement>,
    overlay: &RecipeOverlay,
    layout: &BookLayout,
    cursor: (f32, f32),
    cycle: usize,
) {
    let scale = layout.scale;
    let popup = overlay_rect_for(overlay, layout);
    elements.push(MenuElement::NineSlice {
        x: popup[0],
        y: popup[1],
        w: popup[2],
        h: popup[3],
        sprite: SpriteId::RecipeBookOverlay,
        border: 4.0 * scale,
        tint: WHITE,
    });
    for (index, recipe) in overlay.recipes.iter().enumerate() {
        let rect = overlay_button_rect(overlay, popup, scale, index);
        let hovered = hit_test(cursor, rect);
        let craftable = index < overlay.craftable_count;
        let sprite = match (recipe.furnace, craftable, hovered) {
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
            x: rect[0],
            y: rect[1],
            w: rect[2],
            h: rect[3],
            sprite,
            tint: WHITE,
        });
        for ((grid_x, grid_y), items) in &recipe.ingredients {
            if items.is_empty() {
                continue;
            }
            // Vanilla scales a normal 16px item by 0.375 around the ingredient
            // grid position. This resolves to a 6px icon at button + 2 + 7*grid.
            push_book_stack(
                elements,
                rect[0] + (2.0 + *grid_x as f32 * 7.0) * scale,
                rect[1] + (2.0 + *grid_y as f32 * 7.0) * scale,
                6.0 * scale,
                &items[cycle % items.len()],
            );
        }
    }
}

/// A recipe's ingredient displays on the popup's 3×3 grid.
fn overlay_ingredients(display: &RecipeDisplay) -> Vec<((usize, usize), &SlotDisplay)> {
    if let RecipeDisplay::Furnace { ingredient, .. } = display {
        return vec![((1, 1), ingredient)];
    }
    crafting_grid_slots(display, 3, 3)
        .into_iter()
        .map(|(grid_index, ingredient)| ((grid_index % 3, grid_index / 3), ingredient))
        .collect()
}

/// A crafting recipe's ingredients by index on a `width`×`height` grid:
/// shaped recipes centered (`PlaceRecipeHelper`), shapeless in order.
fn crafting_grid_slots(
    display: &RecipeDisplay,
    width: usize,
    height: usize,
) -> Vec<(usize, &SlotDisplay)> {
    match display {
        RecipeDisplay::Shaped {
            width: recipe_width,
            height: recipe_height,
            ingredients,
            ..
        } => centered_shaped_slots(
            width,
            height,
            *recipe_width as usize,
            *recipe_height as usize,
            ingredients,
        ),
        RecipeDisplay::Shapeless { ingredients, .. } => ingredients
            .iter()
            .take(width * height)
            .enumerate()
            .collect(),
        _ => Vec::new(),
    }
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
        let grouped = build_collections(&book, RecipeBookScreenSpec::crafting_table(1));
        let mut sizes = grouped.iter().map(|c| c.entries.len()).collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 2]);
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
            (1..=4)
                .map(|id| recipe(id, id - 1, vec![Ingredient::Items(vec![1])]))
                .collect(),
            false,
        );
        let grouped = build_collections(&book, RecipeBookScreenSpec::crafting_table(1));
        let categories = grouped.iter().map(Collection::category).collect::<Vec<_>>();
        assert_eq!(categories, vec![2, 0, 3, 1]);
    }

    #[test]
    fn collections_keep_recipes_that_do_not_fit_for_search() {
        let mut book = RecipeBookState::default();
        let mut small = recipe(1, 0, vec![Ingredient::Items(vec![1])]);
        let mut wide = recipe(2, 0, vec![Ingredient::Items(vec![1])]);
        small.contents.group = Some(7);
        wide.contents.group = Some(7);
        wide.contents.display = RecipeDisplay::Shaped {
            width: 3,
            height: 1,
            ingredients: vec![SlotDisplay::Item(1); 3],
            result: SlotDisplay::Item(2),
            crafting_station: SlotDisplay::Empty,
        };
        book.apply_add(vec![small, wide], false);
        let grouped = build_collections(&book, RecipeBookScreenSpec::player(0));
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].entries.len(), 2);
        assert_eq!(grouped[0].fitting, HashSet::from([1]));
        assert_eq!(grouped[0].shown(false).len(), 1);
    }

    #[test]
    fn overlay_position_uses_clicked_button_and_vanilla_clamping() {
        let overlay = RecipeOverlay {
            ids: vec![1, 2, 3, 4],
            craftable_count: 2,
            button_index: 4,
            recipes: Vec::new(),
        };
        let layout = BookLayout {
            x: 100.0,
            y: 200.0,
            scale: 1.0,
        };
        assert_eq!(
            overlay_rect_for(&overlay, &layout),
            [136.0, 231.0, 108.0, 33.0]
        );
    }

    #[test]
    fn overlay_shows_ingredients_instead_of_result_item() {
        let display = RecipeDisplay::Shapeless {
            ingredients: vec![SlotDisplay::Item(ItemKind::Stick.to_u32())],
            result: SlotDisplay::Item(ItemKind::Diamond.to_u32()),
            crafting_station: SlotDisplay::Empty,
        };
        assert_eq!(
            overlay_ingredients(&display),
            vec![((0, 0), &SlotDisplay::Item(ItemKind::Stick.to_u32()))]
        );
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
            recipes: Vec::new(),
        });

        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        let layout = BookLayout::new(400.0, 300.0, 1.0, false);
        let overlay = state.overlay.clone().unwrap();
        let rect = overlay_rect_for(&overlay, &layout);
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
            recipes: Vec::new(),
        });

        let mut book = RecipeBookState::default();
        book.settings.crafting.open = true;
        book.apply_add(vec![recipe(1, 0, vec![Ingredient::Items(vec![1])])], false);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        let layout = BookLayout::new(300.0, 300.0, 1.0, true);
        let overlay = state.overlay.clone().unwrap();
        let rect = overlay_rect_for(&overlay, &layout);
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
        let collection = Collection {
            fitting: HashSet::from([entry.id]),
            craftable: HashSet::new(),
            entries: vec![entry],
        };
        let texts = collection_search_text(&collection, &book);
        let matches = |needle: &str| texts.iter().any(|text| text.matches(needle));

        assert!(matches("fancy"));
        assert!(matches("secret"));
        assert!(!matches("diamond"));
        assert!(!matches(" fancy"));
        assert!(matches(" minecraft : diamond "));
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
        state.selected_tab = Some(2);
        state.page = 3;
        state.overlay = Some(RecipeOverlay {
            ids: vec![1],
            craftable_count: 0,
            button_index: 0,
            recipes: Vec::new(),
        });
        state.button_animation_started[0] = Some(Instant::now());
        state.tab_animation_started.insert(0, Instant::now());

        let mut book = RecipeBookState::default();
        book.ghost_recipe = Some(crate::recipe::GhostRecipe {
            container_id: 3,
            recipe: recipe(1, 0, Vec::new()).contents.display,
        });
        state.reset_for_closed_screen(&mut book);

        assert!(state.screen.is_none());
        assert_eq!(state.selected_tab, None);
        assert_eq!(state.page, 0);
        assert!(state.overlay.is_none());
        assert!(state.button_animation_started.iter().all(Option::is_none));
        assert!(state.tab_animation_started.is_empty());
        assert!(book.ghost_recipe.is_none());
    }

    #[test]
    fn nested_potion_fan_out_stops_at_the_resolved_stack_cap() {
        let book = RecipeBookState::default();
        let mut slot = SlotDisplay::Item(ItemKind::Potion.to_u32());
        for _ in 0..4 {
            slot = SlotDisplay::WithAnyPotion(Box::new(slot));
        }
        assert_eq!(slot_stacks(&slot, &book).len(), MAX_RESOLVED_STACKS);
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
        use azalea_registry::builtin::ItemKind;

        let book = RecipeBookState::default();
        let slot = SlotDisplay::SmithingTrim {
            base: Box::new(SlotDisplay::Item(ItemKind::IronChestplate.to_u32())),
            material: Box::new(SlotDisplay::Item(ItemKind::Diamond.to_u32())),
            trim_pattern: TrimPatternHolder::Direct {
                asset_id: "minecraft:test".into(),
                description: Box::new(crate::chat_component::Component::text("Test Trim")),
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
            tooltip_text(stack)
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
            tooltip_text(stack)
                .iter()
                .any(|line| line.contains("Test Material"))
        );
    }

    fn tooltip_text(stack: &ResolvedSlotStack) -> Vec<String> {
        stack_tooltip_lines(stack)
            .into_iter()
            .map(line_text)
            .collect()
    }

    fn result_recipe(id: u32, category: u32, result: ItemKind) -> RecipeBookAddEntry {
        let mut entry = recipe(id, category, vec![Ingredient::Items(vec![1])]);
        entry.contents.display = RecipeDisplay::Shapeless {
            ingredients: vec![SlotDisplay::Item(1)],
            result: SlotDisplay::Item(result.to_u32()),
            crafting_station: SlotDisplay::Empty,
        };
        entry
    }

    fn open_book(spec: RecipeBookScreenSpec, entries: Vec<RecipeBookAddEntry>) -> RecipeBookState {
        let mut book = RecipeBookState::default();
        let mut settings = book.settings.get(spec.kind);
        settings.open = true;
        book.settings.set(spec.kind, settings);
        book.apply_add(entries, false);
        book
    }

    fn set_search(state: &mut RecipeBookUiState, text: &str) {
        state.search.set_value(text, 1_000.0, &|s| s.len() as f32);
    }

    #[test]
    fn search_keeps_the_page_unless_it_falls_out_of_range() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let mut entries = (0..21)
            .map(|id| result_recipe(id, 0, ItemKind::Stone))
            .collect::<Vec<_>>();
        entries.extend((21..25).map(|id| result_recipe(id, 0, ItemKind::Granite)));
        let book = open_book(spec, entries);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.prepare(&book, spec, &[]);
        state.page = 1;

        set_search(&mut state, "minecraft:stone");
        state.prepare(&book, spec, &[]);
        assert_eq!(state.view.as_ref().unwrap().shown.len(), 21);
        assert_eq!(state.page, 1);

        set_search(&mut state, "minecraft:granite");
        state.prepare(&book, spec, &[]);
        assert_eq!(state.view.as_ref().unwrap().shown.len(), 4);
        assert_eq!(state.page, 0);
    }

    #[test]
    fn a_click_with_the_overlay_open_only_closes_it() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let mut book = open_book(spec, vec![result_recipe(1, 0, ItemKind::Stone)]);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.overlay = Some(RecipeOverlay {
            ids: vec![1],
            craftable_count: 0,
            button_index: 0,
            recipes: Vec::new(),
        });
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = PacketSender::new(tx);
        let filter = BookLayout::new(400.0, 300.0, 1.0, false).filter();
        let frame = handle_input(
            &mut state,
            &mut book,
            &sender,
            spec,
            400.0,
            300.0,
            1.0,
            (filter[0] + 1.0, filter[1] + 1.0),
            true,
            false,
            false,
            false,
            &[],
            &[],
            &|text, _| text.len() as f32,
        );
        assert!(frame.consumed_left_click);
        assert!(state.overlay.is_none());
        assert!(!book.settings.get(spec.kind).filtering);
    }

    #[test]
    fn the_selected_tab_follows_its_category_when_another_tab_disappears() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let mut book = open_book(
            spec,
            vec![
                result_recipe(1, 2, ItemKind::Stone),
                result_recipe(2, 0, ItemKind::Granite),
            ],
        );
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.selected_tab = Some(0);
        state.prepare(&book, spec, &[]);
        assert_eq!(state.view.as_ref().unwrap().tabs.len(), 3);

        book.remove([1]);
        state.prepare(&book, spec, &[]);
        let view = state.view.as_ref().unwrap();
        assert_eq!(view.tabs.len(), 2);
        assert_eq!(state.selected_tab, Some(0));
        assert_eq!(view.shown.len(), 1);
        assert_eq!(view.collections[view.shown[0]].category(), 0);
    }

    #[test]
    fn tab_pulses_arm_on_open_and_recipe_updates_not_when_seen() {
        let spec = RecipeBookScreenSpec::crafting_table(1);
        let highlighted = |id| {
            let mut entry = result_recipe(id, 0, ItemKind::Stone);
            entry.highlight = true;
            entry
        };
        let mut book = open_book(spec, vec![highlighted(1)]);
        let mut state = RecipeBookUiState::new();
        state.ensure_screen(spec);
        state.prepare(&book, spec, &[]);
        assert!(state.tab_animation_started.contains_key(&0));

        state.tab_animation_started.clear();
        book.mark_seen(1);
        state.prepare(&book, spec, &[]);
        assert!(state.tab_animation_started.is_empty());

        book.apply_add(vec![highlighted(2)], false);
        state.prepare(&book, spec, &[]);
        assert!(state.tab_animation_started.contains_key(&0));
    }

    /// The exhaustive assignment `can_match` replaces, as an oracle.
    fn brute_force_match(options: &[Vec<u32>], remaining: &mut HashMap<u32, u32>) -> bool {
        let Some((first, rest)) = options.split_first() else {
            return true;
        };
        first.iter().any(|item| {
            let Some(count) = remaining.get_mut(item).filter(|count| **count > 0) else {
                return false;
            };
            *count -= 1;
            let found = brute_force_match(rest, remaining);
            *remaining.get_mut(item).expect("item stays") += 1;
            found
        })
    }

    #[test]
    fn craftability_matching_agrees_with_exhaustive_assignment() {
        let mut rng = fastrand::Rng::with_seed(11);
        for _ in 0..2_000 {
            let options = (0..rng.usize(0..6))
                .map(|_| {
                    (0..rng.usize(0..4))
                        .map(|_| rng.u32(0..5))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let available = (0..5)
                .filter_map(|item| {
                    let count = rng.u32(0..3);
                    (count > 0).then_some((item, count))
                })
                .collect::<HashMap<_, _>>();
            assert_eq!(
                can_match(&options, &available),
                brute_force_match(&options, &mut available.clone()),
                "{options:?} with {available:?}"
            );
        }
    }

    #[test]
    fn a_chest_from_mixed_planks_needs_eight_planks() {
        let planks = (100..108).collect::<Vec<u32>>();
        let options = vec![planks.clone(); 8];
        let one_each = planks.iter().map(|&p| (p, 1)).collect::<HashMap<_, _>>();
        assert!(can_match(&options, &one_each));
        let seven = planks[..7]
            .iter()
            .map(|&p| (p, 1))
            .collect::<HashMap<_, _>>();
        assert!(!can_match(&options, &seven));
    }

    #[test]
    fn empty_requirements_are_craftable() {
        let entry = recipe(1, 0, Vec::new()).contents;
        assert!(can_craft(&entry, &HashMap::new(), &ItemTags::default()));
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
}
