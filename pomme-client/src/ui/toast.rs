//! Toast popups sliding in at the top-right of the HUD (vanilla `ToastManager`
//! plus `AdvancementToast`, `RecipeToast`, and `TutorialToast`).

use std::cell::OnceCell;
use std::collections::HashMap;
use std::time::Instant;

use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::chat::wrap_spans;
use crate::ui::common::{FONT_SIZE, rgb};
use crate::ui::text::TextSpan;

const TOAST_WIDTH: i32 = 160; // Toast.DEFAULT_WIDTH
const SLOT_HEIGHT: i32 = 32; // Toast.SLOT_HEIGHT
const SLOT_COUNT: usize = 5;
const SLIDE_MS: i64 = 600; // ToastInstance.SLIDE_ANIMATION_DURATION_MS
/// Advancement/recipe display time. Vanilla scales this by the
/// `notificationDisplayTime` option (pomme has none, so the default 1.0
/// applies); tutorial toasts are unscaled even in vanilla.
const DISPLAY_TIME_MS: f64 = 5000.0;
/// Vanilla `font.lineHeight`.
const LINE_HEIGHT: i32 = 9;

const HEADER_COLOR_CHALLENGE: [f32; 4] = rgb(0xFF88FF); // vanilla -30465
const HEADER_COLOR_NORMAL: [f32; 4] = rgb(0xFFFF00); // vanilla -256
const PURPLE_TEXT: [f32; 4] = rgb(0x500050); // vanilla -11534256
const BLACK_TEXT: [f32; 4] = rgb(0x000000); // vanilla -16777216

#[derive(Clone, Copy, PartialEq)]
pub enum AdvancementFrame {
    Task,
    Challenge,
    Goal,
}

impl AdvancementFrame {
    /// Vanilla `AdvancementType.getDisplayName()`.
    fn header(self) -> &'static str {
        translate(match self {
            Self::Task => "advancements.toast.task",
            Self::Challenge => "advancements.toast.challenge",
            Self::Goal => "advancements.toast.goal",
        })
    }

    fn header_color(self) -> [f32; 4] {
        match self {
            Self::Challenge => HEADER_COLOR_CHALLENGE,
            _ => HEADER_COLOR_NORMAL,
        }
    }
}

pub struct AdvancementDisplay {
    pub title: Vec<TextSpan>,
    pub frame: AdvancementFrame,
    pub show_toast: bool,
    /// Bare item resource name for the icon; `None` for an empty icon stack.
    pub icon_item: Option<String>,
}

pub struct AdvancementData {
    pub display: Option<AdvancementDisplay>,
    pub requirements: Vec<Vec<String>>,
}

/// Decoded `ClientboundUpdateAdvancements`, azalea-free.
pub struct AdvancementsUpdate {
    pub reset: bool,
    pub added: Vec<(String, AdvancementData)>,
    pub removed: Vec<String>,
    /// Per advancement, the complete criterion -> obtained map.
    pub progress: Vec<(String, HashMap<String, bool>)>,
    pub show_advancements: bool,
}

pub struct RecipeToastEntry {
    /// Crafting-station icon item (vanilla `RecipeDisplay.craftingStation()`).
    pub category_item: Option<String>,
    /// Result icon item (vanilla `RecipeDisplay.result()`).
    pub unlocked_item: Option<String>,
}

/// The advancement definitions received so far (vanilla `ClientAdvancements`,
/// toast-relevant parts only). Progress is not stored: each packet carries the
/// complete progress map per advancement, so done-ness is evaluated per packet
/// exactly like vanilla's toast condition.
#[derive(Default)]
struct AdvancementStore {
    by_id: HashMap<String, AdvancementData>,
}

impl AdvancementStore {
    /// Vanilla `ClientAdvancements.update`, returning the toasts it would add.
    fn apply(&mut self, update: AdvancementsUpdate) -> Vec<ToastKind> {
        if update.reset {
            self.by_id.clear();
        }
        for id in &update.removed {
            self.by_id.remove(id);
        }
        for (id, data) in update.added {
            self.by_id.insert(id, data);
        }
        let mut toasts = Vec::new();
        for (id, criteria) in &update.progress {
            let Some(advancement) = self.by_id.get(id) else {
                tracing::warn!("Received progress for unknown advancement {id}");
                continue;
            };
            let is_done = advancement.requirements.iter().all(|group| {
                group
                    .iter()
                    .any(|c| criteria.get(c).copied().unwrap_or(false))
            });
            // The reset guard keeps the login sync from flooding toasts.
            if update.reset || !is_done || !update.show_advancements {
                continue;
            }
            let Some(display) = &advancement.display else {
                continue;
            };
            if !display.show_toast {
                continue;
            }
            toasts.push(ToastKind::Advancement(AdvancementToast {
                title: display.title.clone(),
                title_lines: OnceCell::new(),
                frame: display.frame,
                icon_item: display.icon_item.clone(),
            }));
        }
        toasts
    }
}

struct AdvancementToast {
    title: Vec<TextSpan>,
    /// Lazily wrapped at 125 gui px (vanilla `font.split(title, 125)`); the
    /// width and scale-1 metrics never change, so the cache never invalidates.
    title_lines: OnceCell<Vec<Vec<TextSpan>>>,
    frame: AdvancementFrame,
    icon_item: Option<String>,
}

struct RecipeToast {
    entries: Vec<RecipeToastEntry>,
    last_changed: i64,
    changed: bool,
    displayed_index: usize,
}

// TODO: wire to a tutorial system; render type only for now.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum TutorialIcon {
    MovementKeys,
    Mouse,
    Tree,
    RecipeBook,
    WoodenPlanks,
    SocialInteractions,
    RightClick,
}

impl TutorialIcon {
    fn sprite(self) -> SpriteId {
        match self {
            Self::MovementKeys => SpriteId::ToastMovementKeys,
            Self::Mouse => SpriteId::ToastMouse,
            Self::Tree => SpriteId::ToastTree,
            Self::RecipeBook => SpriteId::ToastRecipeBook,
            Self::WoodenPlanks => SpriteId::ToastWoodenPlanks,
            Self::SocialInteractions => SpriteId::ToastSocialInteractions,
            Self::RightClick => SpriteId::ToastRightClick,
        }
    }
}

struct TutorialToast {
    id: u64,
    icon: TutorialIcon,
    /// Pre-wrapped display lines (vanilla wraps at 126 gui px in the
    /// constructor; callers wrap via `wrap_spans` with the title styled
    /// `PURPLE_TEXT` and the message `BLACK_TEXT`).
    lines: Vec<Vec<TextSpan>>,
    progressable: bool,
    time_to_display_ms: i64,
    progress: f32,
    smoothed_progress: f32,
    last_smoothing_time: i64,
    hidden: bool,
}

impl TutorialToast {
    fn content_height(&self) -> i32 {
        (self.lines.len().max(2) as i32) * 11
    }

    fn height(&self) -> i32 {
        7 + self.content_height() + 3
    }
}

enum ToastKind {
    Advancement(AdvancementToast),
    Recipe(RecipeToast),
    Tutorial(TutorialToast),
}

impl ToastKind {
    fn height(&self) -> i32 {
        match self {
            Self::Advancement(_) | Self::Recipe(_) => SLOT_HEIGHT,
            Self::Tutorial(t) => t.height(),
        }
    }

    /// Vanilla `Toast.occcupiedSlotCount` (`Mth.positiveCeilDiv(height, 32)`).
    fn occupied_slots(&self) -> usize {
        ((self.height() + SLOT_HEIGHT - 1) / SLOT_HEIGHT) as usize
    }

    /// Vanilla `Toast.update` + `getWantedVisibility` per subclass.
    fn update(&mut self, fully_visible_for_ms: i64) -> Visibility {
        match self {
            Self::Advancement(_) => {
                if fully_visible_for_ms as f64 >= DISPLAY_TIME_MS {
                    Visibility::Hide
                } else {
                    Visibility::Show
                }
            }
            Self::Recipe(r) => {
                if r.changed {
                    r.last_changed = fully_visible_for_ms;
                    r.changed = false;
                }
                if r.entries.is_empty() {
                    return Visibility::Hide;
                }
                let per_entry = (DISPLAY_TIME_MS / r.entries.len() as f64).max(1.0);
                r.displayed_index =
                    (fully_visible_for_ms as f64 / per_entry) as usize % r.entries.len();
                if (fully_visible_for_ms - r.last_changed) as f64 >= DISPLAY_TIME_MS {
                    Visibility::Hide
                } else {
                    Visibility::Show
                }
            }
            Self::Tutorial(t) => {
                if t.time_to_display_ms > 0 {
                    t.progress =
                        (fully_visible_for_ms as f32 / t.time_to_display_ms as f32).min(1.0);
                    t.smoothed_progress = t.progress;
                    t.last_smoothing_time = fully_visible_for_ms;
                    if fully_visible_for_ms > t.time_to_display_ms {
                        t.hidden = true;
                    }
                } else if t.progressable {
                    let lerp = ((fully_visible_for_ms - t.last_smoothing_time) as f32 / 100.0)
                        .clamp(0.0, 1.0);
                    t.smoothed_progress += (t.progress - t.smoothed_progress) * lerp;
                    t.last_smoothing_time = fully_visible_for_ms;
                }
                if t.hidden {
                    Visibility::Hide
                } else {
                    Visibility::Show
                }
            }
        }
    }

    /// Vanilla `Toast.getSoundEvent`, played when the toast enters a slot.
    fn sound_event(&self) -> Option<&'static str> {
        match self {
            Self::Advancement(a) if a.frame == AdvancementFrame::Challenge => {
                Some("ui.toast.challenge_complete")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Visibility {
    Show,
    Hide,
}

impl Visibility {
    fn sound(self) -> &'static str {
        match self {
            Self::Show => "ui.toast.in",
            Self::Hide => "ui.toast.out",
        }
    }
}

/// Vanilla `ToastManager.ToastInstance`: per-toast slide animation state.
struct ToastInstance {
    kind: ToastKind,
    first_slot: usize,
    slot_count: usize,
    animation_start_ms: i64,
    became_fully_visible_at_ms: i64,
    visibility: Visibility,
    fully_visible_for_ms: i64,
    visible_portion: f32,
    finished: bool,
}

impl ToastInstance {
    fn new(kind: ToastKind, first_slot: usize, slot_count: usize) -> Self {
        // Vanilla resetToast(): sentinels -1, HIDE, zeroed animation.
        Self {
            kind,
            first_slot,
            slot_count,
            animation_start_ms: -1,
            became_fully_visible_at_ms: -1,
            visibility: Visibility::Hide,
            fully_visible_for_ms: 0,
            visible_portion: 0.0,
            finished: false,
        }
    }

    /// Line-by-line port of vanilla `ToastInstance.update`.
    fn update(&mut self, now: i64) {
        if self.animation_start_ms == -1 {
            self.animation_start_ms = now;
            self.visibility = Visibility::Show;
        }
        if self.visibility == Visibility::Show && now - self.animation_start_ms <= SLIDE_MS {
            self.became_fully_visible_at_ms = now;
        }
        self.fully_visible_for_ms = now - self.became_fully_visible_at_ms;
        // calculateVisiblePortion: quadratic ease-in, mirrored while hiding.
        let mut progress =
            ((now - self.animation_start_ms) as f32 / SLIDE_MS as f32).clamp(0.0, 1.0);
        progress *= progress;
        self.visible_portion = match self.visibility {
            Visibility::Hide => 1.0 - progress,
            Visibility::Show => progress,
        };
        let wanted = self.kind.update(self.fully_visible_for_ms);
        if wanted != self.visibility {
            // Rewind so the reverse slide starts from the current portion,
            // keeping vanilla's int truncation.
            self.animation_start_ms =
                now - ((1.0 - self.visible_portion) * SLIDE_MS as f32) as i32 as i64;
            self.visibility = wanted;
        }
        self.finished =
            self.visibility == Visibility::Hide && now - self.animation_start_ms > SLIDE_MS;
    }
}

/// Vanilla `ToastManager`: five 32px slots below the top-right corner, a queue
/// for toasts that don't fit, and the shared slide animation.
pub struct ToastState {
    epoch: Instant,
    visible: Vec<ToastInstance>,
    queued: Vec<ToastKind>,
    occupied: [bool; SLOT_COUNT],
    advancements: AdvancementStore,
    next_tutorial_id: u64,
}

impl Default for ToastState {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            visible: Vec::new(),
            queued: Vec::new(),
            occupied: [false; SLOT_COUNT],
            advancements: AdvancementStore::default(),
            next_tutorial_id: 0,
        }
    }
}

impl ToastState {
    pub fn apply_advancements(&mut self, update: AdvancementsUpdate) {
        self.queued.extend(self.advancements.apply(update));
    }

    /// Vanilla `RecipeToast.addOrUpdate`: entries merge into the existing
    /// recipe toast (visible first, then queued) and extend its display time.
    pub fn add_recipes(&mut self, entries: Vec<RecipeToastEntry>) {
        let Self {
            visible, queued, ..
        } = self;
        let existing = visible
            .iter_mut()
            .find_map(|i| match &mut i.kind {
                ToastKind::Recipe(r) => Some(r),
                _ => None,
            })
            .or_else(|| {
                queued.iter_mut().find_map(|k| match k {
                    ToastKind::Recipe(r) => Some(r),
                    _ => None,
                })
            });
        match existing {
            Some(toast) => {
                toast.entries.extend(entries);
                toast.changed = true;
            }
            None => queued.push(ToastKind::Recipe(RecipeToast {
                entries,
                last_changed: 0,
                changed: true,
                displayed_index: 0,
            })),
        }
    }

    /// Queues a tutorial toast, returning a handle for progress/hide updates.
    /// `lines` are pre-wrapped at 126 gui px (see `TutorialToast::lines`).
    #[allow(dead_code)]
    pub fn add_tutorial(
        &mut self,
        icon: TutorialIcon,
        lines: Vec<Vec<TextSpan>>,
        progressable: bool,
        time_to_display_ms: i64,
    ) -> u64 {
        let id = self.next_tutorial_id;
        self.next_tutorial_id += 1;
        self.queued.push(ToastKind::Tutorial(TutorialToast {
            id,
            icon,
            lines,
            progressable,
            time_to_display_ms,
            progress: 0.0,
            smoothed_progress: 0.0,
            last_smoothing_time: 0,
            hidden: false,
        }));
        id
    }

    #[allow(dead_code)]
    pub fn update_tutorial_progress(&mut self, id: u64, progress: f32) {
        if let Some(t) = self.tutorial_mut(id) {
            t.progress = progress;
        }
    }

    #[allow(dead_code)]
    pub fn hide_tutorial(&mut self, id: u64) {
        if let Some(t) = self.tutorial_mut(id) {
            t.hidden = true;
        }
    }

    fn tutorial_mut(&mut self, id: u64) -> Option<&mut TutorialToast> {
        let Self {
            visible, queued, ..
        } = self;
        visible
            .iter_mut()
            .map(|i| &mut i.kind)
            .chain(queued.iter_mut())
            .find_map(|k| match k {
                ToastKind::Tutorial(t) if t.id == id => Some(t),
                _ => None,
            })
    }

    pub fn clear(&mut self) {
        // Vanilla ToastManager.clear, plus the advancement store (vanilla
        // recreates ClientAdvancements per connection).
        self.visible.clear();
        self.queued.clear();
        self.occupied = [false; SLOT_COUNT];
        self.advancements.by_id.clear();
    }

    /// Vanilla `ToastManager.update`, run once per frame. Returns the UI sound
    /// events to play (at most one visibility in/out sound per call, plus
    /// per-call-deduped toast sounds).
    pub fn update(&mut self) -> Vec<&'static str> {
        let now = self.epoch.elapsed().as_millis() as i64;
        let Self {
            visible,
            queued,
            occupied,
            ..
        } = self;
        let mut sounds = Vec::new();
        let mut flip_sound_played = false;
        visible.retain_mut(|inst| {
            let previous = inst.visibility;
            inst.update(now);
            if inst.visibility != previous && !flip_sound_played {
                flip_sound_played = true;
                sounds.push(inst.visibility.sound());
            }
            if inst.finished {
                occupied[inst.first_slot..inst.first_slot + inst.slot_count].fill(false);
                false
            } else {
                true
            }
        });
        if !queued.is_empty() && occupied.iter().any(|o| !o) {
            // Vanilla scans the whole queue: a blocked multi-slot toast does
            // not hold back narrower ones behind it.
            let mut played_toast_sounds: Vec<&'static str> = Vec::new();
            let mut still_queued = Vec::new();
            for kind in std::mem::take(queued) {
                let slot_count = kind.occupied_slots();
                match find_free_slots_index(occupied, slot_count) {
                    None => still_queued.push(kind),
                    Some(first_slot) => {
                        occupied[first_slot..first_slot + slot_count].fill(true);
                        if let Some(event) = kind.sound_event()
                            && !played_toast_sounds.contains(&event)
                        {
                            played_toast_sounds.push(event);
                            sounds.push(event);
                        }
                        visible.push(ToastInstance::new(kind, first_slot, slot_count));
                    }
                }
            }
            *queued = still_queued;
        }
        sounds
    }

    /// Emits the visible toasts (vanilla `ToastManager.extractRenderState`).
    /// The caller gates on the HUD being visible, matching vanilla.
    pub fn build(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) {
        for inst in &self.visible {
            if inst.finished {
                continue;
            }
            // Toast.xPos / yPos; yPos uses the toast's own height, not the
            // 32px slot height, replicating the vanilla multi-slot quirk.
            let ox = (screen_w - TOAST_WIDTH as f32 * inst.visible_portion * gs).round();
            let oy = ((inst.first_slot as i32 * inst.kind.height()) as f32 * gs).round();
            match &inst.kind {
                ToastKind::Advancement(a) => build_advancement(
                    a,
                    inst.fully_visible_for_ms,
                    elements,
                    ox,
                    oy,
                    gs,
                    text_width_fn,
                ),
                ToastKind::Recipe(r) => build_recipe(r, elements, ox, oy, gs),
                ToastKind::Tutorial(t) => build_tutorial(t, elements, ox, oy, gs),
            }
        }
    }
}

/// Vanilla `ToastManager.findFreeSlotsIndex`: first run of `required`
/// contiguous free slots.
fn find_free_slots_index(occupied: &[bool; SLOT_COUNT], required: usize) -> Option<usize> {
    if occupied.iter().filter(|o| !**o).count() < required {
        return None;
    }
    let mut consecutive = 0;
    for (i, used) in occupied.iter().enumerate() {
        if *used {
            consecutive = 0;
            continue;
        }
        consecutive += 1;
        if consecutive == required {
            return Some(i + 1 - consecutive);
        }
    }
    None
}

fn translate(key: &'static str) -> &'static str {
    crate::lang::translate(key).unwrap_or(key)
}

fn with_alpha(spans: &[TextSpan], alpha: f32) -> Vec<TextSpan> {
    spans
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.color[3] *= alpha;
            s
        })
        .collect()
}

/// Alpha byte for the advancement crossfade: a 300ms window scaled to `peak`
/// and floored. Callers skip drawing below 4, like vanilla `drawString`.
fn fade_alpha(elapsed_ms: i64, peak: f32) -> f32 {
    ((elapsed_ms as f32 / 300.0).clamp(0.0, 1.0) * peak).floor()
}

fn push_background(
    elements: &mut Vec<MenuElement>,
    sprite: SpriteId,
    ox: f32,
    oy: f32,
    height: i32,
    gs: f32,
) {
    elements.push(MenuElement::Image {
        x: ox,
        y: oy,
        w: TOAST_WIDTH as f32 * gs,
        h: height as f32 * gs,
        sprite,
        tint: [1.0; 4],
    });
}

fn push_text(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    text: &str,
    color: [f32; 4],
    gs: f32,
) {
    elements.push(MenuElement::TextFlat {
        x,
        y,
        text: text.to_string(),
        scale: FONT_SIZE * gs,
        color,
    });
}

fn push_spans(elements: &mut Vec<MenuElement>, x: f32, y: f32, spans: Vec<TextSpan>, gs: f32) {
    elements.push(MenuElement::McText {
        x,
        y,
        spans,
        scale: FONT_SIZE * gs,
        centered: false,
        shadow: false,
    });
}

fn push_item(elements: &mut Vec<MenuElement>, item: Option<&String>, x: f32, y: f32, size: f32) {
    if let Some(item) = item {
        elements.push(MenuElement::ItemIcon {
            x,
            y,
            w: size,
            h: size,
            item_name: item.clone(),
            tint: [1.0; 4],
        });
    }
}

/// Vanilla `AdvancementToast.extractRenderState`.
fn build_advancement(
    toast: &AdvancementToast,
    fully_visible_for_ms: i64,
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    push_background(
        elements,
        SpriteId::ToastAdvancement,
        ox,
        oy,
        SLOT_HEIGHT,
        gs,
    );
    let lines = toast
        .title_lines
        .get_or_init(|| wrap_spans(&toast.title, 125.0, &|s| text_width_fn(s, FONT_SIZE)));
    let x = ox + 30.0 * gs;
    if lines.len() == 1 {
        push_text(
            elements,
            x,
            oy + 7.0 * gs,
            toast.frame.header(),
            toast.frame.header_color(),
            gs,
        );
        push_spans(elements, x, oy + 18.0 * gs, lines[0].clone(), gs);
    } else if fully_visible_for_ms < 1500 {
        // Header fades out over the first 1500ms.
        let alpha = fade_alpha(1500 - fully_visible_for_ms, 255.0);
        if alpha >= 4.0 {
            let c = toast.frame.header_color();
            push_text(
                elements,
                x,
                oy + 11.0 * gs,
                toast.frame.header(),
                [c[0], c[1], c[2], alpha / 255.0],
                gs,
            );
        }
    } else {
        // Title lines fade in, vertically centered (alpha peaks at 252 as in
        // vanilla).
        let alpha = fade_alpha(fully_visible_for_ms - 1500, 252.0);
        if alpha >= 4.0 {
            let base_y = SLOT_HEIGHT / 2 - lines.len() as i32 * LINE_HEIGHT / 2;
            for (i, line) in lines.iter().enumerate() {
                push_spans(
                    elements,
                    x,
                    oy + (base_y + i as i32 * LINE_HEIGHT) as f32 * gs,
                    with_alpha(line, alpha / 255.0),
                    gs,
                );
            }
        }
    }
    push_item(
        elements,
        toast.icon_item.as_ref(),
        ox + 8.0 * gs,
        oy + 8.0 * gs,
        16.0 * gs,
    );
}

/// Vanilla `RecipeToast.extractRenderState`.
fn build_recipe(toast: &RecipeToast, elements: &mut Vec<MenuElement>, ox: f32, oy: f32, gs: f32) {
    push_background(elements, SpriteId::ToastRecipe, ox, oy, SLOT_HEIGHT, gs);
    let x = ox + 30.0 * gs;
    push_text(
        elements,
        x,
        oy + 7.0 * gs,
        translate("recipe.toast.title"),
        PURPLE_TEXT,
        gs,
    );
    push_text(
        elements,
        x,
        oy + 18.0 * gs,
        translate("recipe.toast.description"),
        BLACK_TEXT,
        gs,
    );
    let Some(entry) = toast.entries.get(toast.displayed_index) else {
        return;
    };
    // Category item at 0.6 scale: fakeItem(3, 3) in the scaled pose lands at
    // (1.8, 1.8) with a 9.6px icon.
    push_item(
        elements,
        entry.category_item.as_ref(),
        ox + 1.8 * gs,
        oy + 1.8 * gs,
        9.6 * gs,
    );
    push_item(
        elements,
        entry.unlocked_item.as_ref(),
        ox + 8.0 * gs,
        oy + 8.0 * gs,
        16.0 * gs,
    );
}

/// Vanilla `TutorialToast.extractRenderState`.
fn build_tutorial(
    toast: &TutorialToast,
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    gs: f32,
) {
    let height = toast.height();
    elements.push(MenuElement::NineSlice {
        x: ox,
        y: oy,
        w: TOAST_WIDTH as f32 * gs,
        h: height as f32 * gs,
        sprite: SpriteId::ToastTutorial,
        border: 3.0 * gs,
        tint: [1.0; 4],
    });
    elements.push(MenuElement::Image {
        x: ox + 6.0 * gs,
        y: oy + 6.0 * gs,
        w: 20.0 * gs,
        h: 20.0 * gs,
        sprite: toast.icon.sprite(),
        tint: [1.0; 4],
    });
    let text_height = toast.lines.len() as i32 * 11;
    let text_top = 7 + (toast.content_height() - text_height) / 2;
    for (i, line) in toast.lines.iter().enumerate() {
        push_spans(
            elements,
            ox + 30.0 * gs,
            oy + (text_top + i as i32 * 11) as f32 * gs,
            line.clone(),
            gs,
        );
    }
    if toast.progressable {
        let bar_y = oy + (height - 4) as f32 * gs;
        elements.push(MenuElement::Rect {
            x: ox + 3.0 * gs,
            y: bar_y,
            w: 154.0 * gs,
            h: 1.0 * gs,
            corner_radius: 0.0,
            color: [1.0; 4],
        });
        // Vanilla -16755456 / -11206656.
        let color = if toast.progress >= toast.smoothed_progress {
            rgb(0x005500)
        } else {
            rgb(0x550000)
        };
        // Vanilla fills 3..(int)(3 + 154 * smoothed): the width truncates to
        // whole gui px.
        let fill = ((3.0 + 154.0 * toast.smoothed_progress) as i32 - 3).max(0);
        elements.push(MenuElement::Rect {
            x: ox + 3.0 * gs,
            y: bar_y,
            w: fill as f32 * gs,
            h: 1.0 * gs,
            corner_radius: 0.0,
            color,
        });
    }
}
