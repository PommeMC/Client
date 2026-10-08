//! The singleplayer world list, ported from vanilla `SelectWorldScreen` and
//! `WorldSelectionList`.
//!
//! Its geometry differs from the server list's: a taller header to fit the
//! search box, narrower rows, and a six-button footer.

use super::*;
use crate::ui::text::TextSpan;
use crate::ui::world_list::{Difficulty, GameMode, WorldSummary};

const WORLD_ROW_W: f32 = 270.0;
const HEADER_H: f32 = 49.0;
const FOOTER_H: f32 = 60.0;
const WIDE_BTN_W: f32 = 150.0;
const NARROW_BTN_W: f32 = 71.0;
const ICON_SIZE: f32 = 32.0;
const SEARCH_W: f32 = 200.0;
/// Vanilla's footer grid spaces columns by 8 and rows by 4 (`BTN_GAP`).
const FOOTER_COL_GAP: f32 = 8.0;

/// Vanilla's `0x808080` for the folder and info lines.
const COL_GREY: [f32; 4] = [0.502, 0.502, 0.502, 1.0];
/// Vanilla's `0xFF0000` for a hardcore world.
const COL_HARDCORE: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

impl MainMenu {
    pub(super) fn open_world_list(&mut self, gs: f32, wf: &dyn Fn(&str) -> f32) {
        self.rescan_worlds();
        // Vanilla skips an empty list entirely and opens the create screen.
        if self.world_list.worlds.is_empty() {
            self.open_create_world(gs, wf);
            return;
        }
        self.set_screen(Screen::WorldList);
        self.scroll_offset = 0.0;
        self.selected_world = None;
        self.world_search.clear();
    }

    fn rescan_worlds(&mut self) {
        self.world_list = crate::ui::world_list::WorldList::scan(&self.saves_dir);
    }

    pub(super) fn build_world_list(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = self.gui_scale(screen_w, screen_h);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let header_h = HEADER_H * gs;
        let footer_h = FOOTER_H * gs;
        let entry_h = ENTRY_H * gs;
        let row_w = WORLD_ROW_W * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.f5 {
            self.rescan_worlds();
        }
        if input.escape {
            self.set_screen(Screen::Main);
            return empty_result(2.0);
        }

        let list_top = header_h;
        let list_bottom = screen_h - footer_h;
        let list_h = list_bottom - list_top;

        let mut elements = Vec::new();
        let mut any_hovered = false;
        let mut action = MenuAction::None;
        let playable = crate::singleplayer::AVAILABLE;

        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: 8.0 * gs,
            text: "Select World".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });

        let search_w = SEARCH_W * gs;
        let search_x = screen_w / 2.0 - search_w / 2.0;
        let search_y = 21.0 * gs;
        let field_h = FIELD_H * gs;
        self.text_field(
            &mut elements,
            TextTarget::WorldSearch,
            0,
            input,
            search_x,
            search_y,
            search_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        push_field_hint(
            &mut elements,
            &self.world_search,
            self.focused_field == Some(0),
            search_x,
            search_y,
            field_h,
            fs,
            gs,
            "Search...",
        );

        push_menu_backdrop(&mut elements, 0.0, list_top, screen_w, list_h, gs);
        push_separator(
            &mut elements,
            0.0,
            list_top - SEP_H * gs,
            screen_w,
            SEP_H * gs,
        );
        push_separator(&mut elements, 0.0, list_bottom, screen_w, SEP_H * gs);

        let filter = self.world_search.value().to_lowercase();
        let visible: Vec<usize> = self
            .world_list
            .worlds
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.name.to_lowercase().contains(&filter) || w.folder.to_lowercase().contains(&filter)
            })
            .map(|(i, _)| i)
            .collect();

        let list_pad = 4.0 * gs;
        let total_content = list_pad * 2.0 + visible.len() as f32 * entry_h;
        self.scroll_region(input, [0.0, list_top, screen_w, list_h], total_content, gs);

        let list_left = screen_w / 2.0 - row_w / 2.0;

        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: list_top,
            w: screen_w,
            h: list_h,
        });

        for (slot, &idx) in visible.iter().enumerate() {
            let world = &self.world_list.worlds[idx];
            let ey = list_top + list_pad + slot as f32 * entry_h - self.scroll_offset;
            if ey + entry_h < list_top || ey > list_bottom {
                continue;
            }

            let rect = [list_left, ey, row_w, entry_h];
            let selected = self.selected_world.as_deref() == Some(world.folder.as_str());
            // The rows draw inside a scissor that does not clip hit-testing.
            let hovered =
                common::hit_test(cursor, rect) && cursor.1 >= list_top && cursor.1 <= list_bottom;
            any_hovered |= hovered;

            if selected || hovered {
                elements.push(MenuElement::Rect {
                    x: rect[0],
                    y: rect[1],
                    w: rect[2],
                    h: rect[3],
                    corner_radius: 0.0,
                    color: if selected {
                        [1.0, 1.0, 1.0, 0.12]
                    } else {
                        [1.0, 1.0, 1.0, 0.04]
                    },
                });
            }
            if selected {
                push_outline(&mut elements, rect[0], rect[1], rect[2], rect[3], gs);
            }

            let icon_size = ICON_SIZE * gs;
            let icon_x = rect[0] + SERVER_ENTRY_PAD * gs;
            let icon_y = rect[1] + SERVER_ENTRY_PAD * gs;
            let text_x = icon_x + 35.0 * gs;

            let icon = [icon_x, icon_y, icon_size, icon_size];
            push_icon(&mut elements, icon, SpriteId::UnknownServer);

            let on_icon = hovered && common::hit_test(cursor, icon);

            if hovered {
                // Vanilla dims the icon only, not the whole row.
                elements.push(MenuElement::Rect {
                    x: icon[0],
                    y: icon[1],
                    w: icon[2],
                    h: icon[3],
                    corner_radius: 0.0,
                    color: [0.274, 0.274, 0.274, 0.63],
                });
                push_icon(
                    &mut elements,
                    icon,
                    if on_icon {
                        SpriteId::WorldJoinHighlighted
                    } else {
                        SpriteId::WorldJoin
                    },
                );
            }

            // TODO: clip the three lines to the row's text width with an ellipsis
            // (vanilla `StringWidget::setMaxWidth`).
            elements.push(MenuElement::Text {
                x: text_x,
                y: icon_y + 1.0 * gs,
                text: world.name.clone(),
                scale: fs,
                color: WHITE,
                centered: false,
            });
            elements.push(MenuElement::Text {
                x: text_x,
                y: icon_y + 12.0 * gs,
                text: match world.last_played {
                    0 => world.folder.clone(),
                    millis => format!("{} ({})", world.folder, format_last_played(millis)),
                },
                scale: fs,
                color: COL_GREY,
                centered: false,
            });
            elements.push(MenuElement::TextSpans {
                x: text_x,
                y: icon_y + 21.0 * gs,
                spans: info_line(world),
                scale: fs,
                centered: false,
            });

            if clicked && hovered {
                let folder = world.folder.clone();
                // Vanilla `MouseHandler` keys the double click on time alone,
                // so the second click may land on a different row.
                let double = self.last_click_time.elapsed().as_millis() < DOUBLE_CLICK_MS;

                if playable && (on_icon || double) {
                    action = MenuAction::PlayWorld { folder };
                } else {
                    self.selected_world = Some(folder);
                    self.last_click_time = Instant::now();
                }
            }
        }

        elements.push(MenuElement::ScissorPop);
        push_list_scrollbar(
            &mut elements,
            screen_w,
            list_top,
            list_h,
            total_content,
            self.scroll_offset,
            gs,
        );

        let wide_w = WIDE_BTN_W * gs;
        let narrow_w = NARROW_BTN_W * gs;
        let col_gap = FOOTER_COL_GAP * gs;
        let grid_w = narrow_w * 4.0 + col_gap * 3.0;
        let grid_x = (screen_w - grid_w) / 2.0;
        let row1_y = list_bottom + (footer_h - (btn_h * 2.0 + gap)) / 2.0;
        let row2_y = row1_y + btn_h + gap;
        let col = |n: f32| grid_x + (narrow_w + col_gap) * n;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let has_sel = self.selected_world.is_some();
        let tipped = |elements: &mut Vec<MenuElement>,
                      ctx: &mut FocusCtx,
                      hov: &mut bool,
                      r: [f32; 4],
                      label: &str,
                      enabled: bool,
                      tip: Option<&str>| {
            let fired = push_button_f(
                elements, ctx, hov, cursor, clicked, r[0], r[1], r[2], r[3], gs, label, enabled,
            );
            if let Some(tip) = tip {
                push_hover_tooltip(elements, cursor, screen_w, screen_h, gs, r, tip);
            }
            fired
        };

        // Emission order is the tab ring, so keep vanilla's grid order.
        if tipped(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            [grid_x, row1_y, wide_w, btn_h],
            "Play Selected World",
            has_sel && playable,
            (!playable).then_some(crate::singleplayer::UNAVAILABLE_MESSAGE),
        ) && let Some(folder) = self.selected_world.clone()
        {
            action = MenuAction::PlayWorld { folder };
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            col(2.0),
            row1_y,
            wide_w,
            btn_h,
            gs,
            "Create New World",
            true,
        ) {
            self.open_create_world(gs, &|t: &str| text_width_fn(t, fs));
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            col(0.0),
            row2_y,
            narrow_w,
            btn_h,
            gs,
            "Edit",
            has_sel,
        ) && let Some(folder) = self.selected_world.clone()
        {
            self.open_edit_world(folder, gs, &|t: &str| text_width_fn(t, fs));
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            col(1.0),
            row2_y,
            narrow_w,
            btn_h,
            gs,
            "Delete",
            has_sel,
        ) && let Some(folder) = self.selected_world.clone()
        {
            self.set_screen(Screen::ConfirmDeleteWorld(folder));
        }
        tipped(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            [col(2.0), row2_y, narrow_w, btn_h],
            "Re-Create",
            false,
            Some("Not available yet"),
        );
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            col(3.0),
            row2_y,
            narrow_w,
            btn_h,
            gs,
            "Back",
            true,
        ) {
            self.set_screen(Screen::Main);
        }

        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: (clicked && any_hovered) || ctx.fired,
        }
    }
}

/// Vanilla keys tooltips off hover alone, ignoring the button's active flag.
fn push_hover_tooltip(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    rect: [f32; 4],
    text: &str,
) {
    if common::hit_test(cursor, rect) {
        common::push_tooltip(elements, cursor, screen_w, screen_h, gs, text);
    }
}

fn push_icon(elements: &mut Vec<MenuElement>, rect: [f32; 4], sprite: SpriteId) {
    elements.push(MenuElement::Image {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        sprite,
        tint: WHITE,
    });
}

/// Vanilla `LevelSummary::createInfo`, minus the version-compatibility states
/// pomme has no model for.
fn info_line(world: &WorldSummary) -> Vec<TextSpan> {
    let grey = |text: String| TextSpan::new(text, COL_GREY);

    let mut spans = Vec::new();
    if world.hardcore {
        spans.push(TextSpan::new("Hardcore Mode".into(), COL_HARDCORE));
    } else {
        spans.push(grey(world.game_mode.label().into()));
    }
    if world.allow_commands {
        spans.push(grey(", Commands".into()));
    }
    spans.push(grey(format!(", Version: {}", world.version)));
    spans
}

/// Vanilla shows the last-played time in the short local date format; pomme has
/// no localisation, so this is the en-US one it would pick.
fn format_last_played(millis: u64) -> String {
    const FORMAT: &[time::format_description::FormatItem<'_>] = time::macros::format_description!(
        "[month padding:none]/[day padding:none]/[year repr:last_two], \
         [hour repr:12 padding:none]:[minute] [period]"
    );

    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    time::OffsetDateTime::from_unix_timestamp((millis / 1000) as i64)
        .map(|t| t.to_offset(offset))
        .ok()
        .and_then(|t| t.format(&FORMAT).ok())
        .unwrap_or_else(|| "unknown".into())
}

/// Which of vanilla's three create-world tabs is showing.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum CreateTab {
    #[default]
    Game,
    World,
    More,
}

/// Vanilla offers three modes here but stores two: hardcore is Survival plus a
/// flag, and it forces difficulty to Hard and cheats off.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SelectedMode {
    #[default]
    Survival,
    Hardcore,
    Creative,
}

impl SelectedMode {
    fn cycle(self) -> Self {
        match self {
            Self::Survival => Self::Hardcore,
            Self::Hardcore => Self::Creative,
            Self::Creative => Self::Survival,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Survival => "Survival",
            Self::Hardcore => "Hardcore",
            Self::Creative => "Creative",
        }
    }

    /// Vanilla `selectWorld.gameMode.<name>.info`.
    fn info(self) -> &'static str {
        match self {
            Self::Survival => {
                "Explore a mysterious world where you build, collect, craft, and fight monsters."
            }
            Self::Hardcore => {
                "Survival Mode locked to 'Hard' difficulty. You can't respawn if you die."
            }
            Self::Creative => {
                "Create, build, and explore without limits. You can fly, have endless materials, \
                 and can't be hurt by monsters."
            }
        }
    }

    fn stored(self) -> (GameMode, bool) {
        match self {
            Self::Survival => (GameMode::Survival, false),
            Self::Hardcore => (GameMode::Survival, true),
            Self::Creative => (GameMode::Creative, false),
        }
    }
}

#[derive(Default)]
pub(super) struct CreateWorldState {
    tab: CreateTab,
    mode: SelectedMode,
    difficulty: Difficulty,
    /// `None` until the player touches the control, which is when vanilla stops
    /// deriving it from the game mode.
    allow_commands: Option<bool>,
    folder: String,
    folder_for: String,
}

impl CreateWorldState {
    fn hardcore(&self) -> bool {
        self.mode == SelectedMode::Hardcore
    }

    fn difficulty(&self) -> Difficulty {
        if self.hardcore() {
            Difficulty::Hard
        } else {
            self.difficulty
        }
    }

    /// The field this tab shows, if any.
    pub(super) fn field_target(&self) -> Option<TextTarget> {
        match self.tab {
            CreateTab::Game => Some(TextTarget::WorldName),
            CreateTab::World => Some(TextTarget::WorldSeed),
            CreateTab::More => None,
        }
    }

    fn allow_commands(&self) -> bool {
        if self.hardcore() {
            return false;
        }
        self.allow_commands
            .unwrap_or(self.mode == SelectedMode::Creative)
    }
}

const TAB_BAR_H: f32 = 24.0;
const TAB_BAR_MAX_W: f32 = 400.0;
const TAB_BAR_MARGIN: f32 = 28.0;
const NAME_FIELD_W: f32 = 208.0;
const SEED_FIELD_W: f32 = 308.0;
const OPTION_W: f32 = 210.0;
const HALF_OPTION_W: f32 = 150.0;
const SWITCH_W: f32 = 44.0;
const LABEL_GAP: f32 = 4.0;
const ROW_GAP: f32 = 8.0;
const COL_GAP: f32 = 10.0;
/// Vanilla `EditWorldScreen` spaces its column by 5 with 20-unit spacers.
const EDIT_SPACING: f32 = 5.0;
const EDIT_SPACER: f32 = 20.0;

impl MainMenu {
    pub(super) fn open_create_world(&mut self, gs: f32, wf: &dyn Fn(&str) -> f32) {
        self.create = CreateWorldState::default();
        self.world_name
            .set_value("New World", (NAME_FIELD_W - FIELD_TEXT_PAD) * gs, wf);
        self.world_seed.clear();
        self.set_screen(Screen::CreateWorld);
        self.focused_field = Some(0);
        self.world_name.set_focused(true);
    }

    pub(super) fn build_create_world(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = self.gui_scale(screen_w, screen_h);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let field_h = FIELD_H * gs;
        let row_gap = ROW_GAP * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;
        let cx = screen_w / 2.0;

        if input.escape {
            self.leave_create_world();
            return empty_result(2.0);
        }

        if self.create.field_target().is_some() {
            self.cycle_fields(input, 1);
        }

        let mut elements = Vec::new();
        let mut any_hovered = false;
        let inert = |elements: &mut Vec<MenuElement>,
                     any_hovered: &mut bool,
                     x: f32,
                     y: f32,
                     w: f32,
                     label: &str| {
            push_button(
                elements,
                any_hovered,
                cursor,
                x,
                y,
                w,
                btn_h,
                gs,
                label,
                false,
            );
            push_hover_tooltip(
                elements,
                cursor,
                screen_w,
                screen_h,
                gs,
                [x, y, w, btn_h],
                "Not available yet",
            );
        };

        // Tab bar across the top.
        let tab_h = TAB_BAR_H * gs;
        let bar_w = screen_w.min(TAB_BAR_MAX_W * gs) - TAB_BAR_MARGIN * gs;
        let tab_w = bar_w / 3.0;
        let bar_x = (screen_w - bar_w) / 2.0;
        for (i, (tab, label)) in [
            (CreateTab::Game, "Game"),
            (CreateTab::World, "World"),
            (CreateTab::More, "More"),
        ]
        .into_iter()
        .enumerate()
        {
            let x = bar_x + tab_w * i as f32;
            let active = self.create.tab == tab;
            let hovered = common::hit_test(cursor, [x, 0.0, tab_w, tab_h]);
            any_hovered |= hovered;
            let sprite = match (active, hovered) {
                (true, true) => SpriteId::TabSelectedHighlighted,
                (true, false) => SpriteId::TabSelected,
                (false, true) => SpriteId::TabHighlighted,
                (false, false) => SpriteId::Tab,
            };
            nine_slice(&mut elements, x, 0.0, tab_w, tab_h, sprite, 2.0 * gs);
            // The selected tab opens onto the page; the others sit 3 lower.
            let label_top = if active {
                push_menu_backdrop(
                    &mut elements,
                    x + 2.0 * gs,
                    2.0 * gs,
                    tab_w - 4.0 * gs,
                    tab_h - 2.0 * gs,
                    gs,
                );
                0.0
            } else {
                3.0 * gs
            };
            elements.push(MenuElement::Text {
                x: x + tab_w / 2.0,
                y: label_top + (tab_h - label_top - fs) / 2.0,
                text: label.into(),
                scale: fs,
                color: WHITE,
                centered: true,
            });
            if active {
                // Vanilla underlines the selected tab's label.
                let uw = text_width_fn(label, fs).min(tab_w - LABEL_GAP * gs);
                elements.push(MenuElement::Rect {
                    x: x + (tab_w - uw) / 2.0,
                    y: tab_h - 2.0 * gs,
                    w: uw,
                    h: gs,
                    corner_radius: 0.0,
                    color: WHITE,
                });
            }
            if clicked && hovered && !active {
                self.create.tab = tab;
                self.focused_field = None;
            }
        }
        // The header separator runs either side of the tabs, not under them.
        for (x, w) in [(0.0, bar_x), (bar_x + bar_w, screen_w - bar_x - bar_w)] {
            elements.push(MenuElement::Image {
                x,
                y: tab_h - SEP_H * gs,
                w,
                h: SEP_H * gs,
                sprite: SpriteId::HeaderSeparator,
                tint: WHITE,
            });
        }

        // Vanilla places the tab body a sixth of the way down the free space.
        let footer_h = HEADER_FOOTER_H * gs;
        let field_block = fs + LABEL_GAP * gs + field_h;
        let stack_h = match self.create.tab {
            CreateTab::Game => field_block + (btn_h + row_gap) * 3.0,
            CreateTab::World => {
                btn_h + row_gap + field_block + row_gap + btn_h * 2.0 + LABEL_GAP * gs
            }
            CreateTab::More => btn_h * 3.0 + row_gap * 2.0,
        };
        let mut y = tab_h + (screen_h - footer_h - tab_h - stack_h) / 6.0;
        let opt_w = OPTION_W * gs;
        let opt_x = cx - opt_w / 2.0;
        let half = HALF_OPTION_W * gs;

        match self.create.tab {
            CreateTab::Game => {
                let rect = self.labelled_field(
                    &mut elements,
                    input,
                    "World Name",
                    TextTarget::WorldName,
                    cx,
                    &mut y,
                    NAME_FIELD_W * gs,
                    LABEL_GAP * gs,
                    gs,
                    text_width_fn,
                );
                self.refresh_target_folder();
                if common::hit_test(cursor, rect) {
                    let tip = format!("Save folder: {}", self.create.folder);
                    common::push_tooltip(&mut elements, cursor, screen_w, screen_h, gs, &tip);
                }
                y += row_gap;

                let hardcore = self.create.hardcore();
                let cheats = if self.create.allow_commands() {
                    "ON"
                } else {
                    "OFF"
                };
                let rows = [
                    (
                        format!("Game Mode: {}", self.create.mode.label()),
                        true,
                        self.create.mode.info(),
                    ),
                    (
                        format!("Difficulty: {}", self.create.difficulty().label()),
                        !hardcore,
                        self.create.difficulty().info(),
                    ),
                    (
                        format!("Allow Cheats: {cheats}"),
                        !hardcore,
                        "Commands like /gamemode, /experience",
                    ),
                ];
                for (i, (label, enabled, info)) in rows.into_iter().enumerate() {
                    if push_button(
                        &mut elements,
                        &mut any_hovered,
                        cursor,
                        opt_x,
                        y,
                        opt_w,
                        btn_h,
                        gs,
                        &label,
                        enabled,
                    ) && clicked
                    {
                        match i {
                            0 => self.create.mode = self.create.mode.cycle(),
                            1 => self.create.difficulty = self.create.difficulty.cycle(),
                            _ => self.create.allow_commands = Some(!self.create.allow_commands()),
                        }
                    }
                    push_hover_tooltip(
                        &mut elements,
                        cursor,
                        screen_w,
                        screen_h,
                        gs,
                        [opt_x, y, opt_w, btn_h],
                        info,
                    );
                    y += btn_h + row_gap;
                }
            }
            CreateTab::World => {
                let left = cx - (half * 2.0 + COL_GAP * gs) / 2.0;
                for (i, label) in ["World Type: Default", "Customize"].into_iter().enumerate() {
                    let x = left + (half + COL_GAP * gs) * i as f32;
                    inert(&mut elements, &mut any_hovered, x, y, half, label);
                }
                y += btn_h + row_gap;

                let rect = self.labelled_field(
                    &mut elements,
                    input,
                    "Seed for the world generator",
                    TextTarget::WorldSeed,
                    cx,
                    &mut y,
                    SEED_FIELD_W * gs,
                    LABEL_GAP * gs,
                    gs,
                    text_width_fn,
                );
                push_field_hint(
                    &mut elements,
                    &self.world_seed,
                    self.focused_field == Some(0),
                    rect[0],
                    rect[1],
                    rect[3],
                    fs,
                    gs,
                    "Leave blank for a random seed",
                );
                y += row_gap;

                let switch_w = SWITCH_W * gs;
                let switch_x = left + half * 2.0 + COL_GAP * gs - switch_w;
                for (label, state) in [("Generate Structures", "ON"), ("Bonus Chest", "OFF")] {
                    elements.push(MenuElement::Text {
                        x: left,
                        y: y + (btn_h - fs) / 2.0,
                        text: label.into(),
                        scale: fs,
                        color: common::COL_DISABLED,
                        centered: false,
                    });
                    inert(
                        &mut elements,
                        &mut any_hovered,
                        switch_x,
                        y,
                        switch_w,
                        state,
                    );
                    y += btn_h + LABEL_GAP * gs;
                }
            }
            CreateTab::More => {
                for label in ["Game Rules", "Experiments", "Data Packs"] {
                    inert(&mut elements, &mut any_hovered, opt_x, y, opt_w, label);
                    y += btn_h + row_gap;
                }
            }
        }

        let footer_y = screen_h - footer_h + (footer_h - btn_h) / 2.0;
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            cx - half - row_gap / 2.0,
            footer_y,
            half,
            btn_h,
            gs,
            "Create New World",
            true,
        ) && clicked
        {
            self.create_world();
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            cx + row_gap / 2.0,
            footer_y,
            half,
            btn_h,
            gs,
            "Cancel",
            true,
        ) && clicked
        {
            self.leave_create_world();
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: clicked && any_hovered,
        }
    }

    /// A caption over a text field, advancing `y` past both and returning the
    /// field's rect for hit-testing.
    #[allow(clippy::too_many_arguments)]
    fn labelled_field(
        &mut self,
        elements: &mut Vec<MenuElement>,
        input: &MenuInput,
        caption: &str,
        target: TextTarget,
        cx: f32,
        y: &mut f32,
        w: f32,
        label_gap: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> [f32; 4] {
        let fs = common::FONT_SIZE * gs;
        let field_h = FIELD_H * gs;
        let x = cx - w / 2.0;
        elements.push(MenuElement::Text {
            x,
            y: *y,
            text: caption.into(),
            scale: fs,
            color: COL_DIM,
            centered: false,
        });
        *y += fs + label_gap;
        self.text_field(
            elements,
            target,
            0,
            input,
            x,
            *y,
            w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        let rect = [x, *y, w, field_h];
        *y += field_h;
        rect
    }

    /// Back to whatever opened this: the list, or the title screen when there
    /// were no worlds to list.
    fn leave_create_world(&mut self) {
        let back = if self.world_list.worlds.is_empty() {
            Screen::Main
        } else {
            Screen::WorldList
        };
        self.set_screen(back);
    }

    fn create_world(&mut self) {
        let name = self.world_name.value().trim();
        let (game_mode, hardcore) = self.create.mode.stored();
        let summary = crate::ui::world_list::WorldSummary {
            name: name.to_owned(),
            folder: self.world_list.available_folder_name(name),
            last_played: 0,
            game_mode,
            hardcore,
            allow_commands: self.create.allow_commands(),
            difficulty: self.create.difficulty(),
            seed: self.world_seed.value().trim().to_owned(),
            version: self.version.clone(),
            extra: Default::default(),
        };
        if let Err(e) = self.world_list.create(summary) {
            tracing::error!("Failed to create world: {e}");
        }
        self.set_screen(Screen::WorldList);
    }

    /// Vanilla recomputes this per keystroke; per frame would stat the disk for
    /// an answer that only changes when the name does.
    fn refresh_target_folder(&mut self) {
        if self.create.folder_for != self.world_name.value() {
            self.create.folder_for = self.world_name.value().to_owned();
            self.create.folder = self
                .world_list
                .available_folder_name(&self.create.folder_for);
        }
    }
}

impl MainMenu {
    pub(super) fn open_edit_world(&mut self, folder: String, gs: f32, wf: &dyn Fn(&str) -> f32) {
        let name = self.world_display_name(&folder);
        self.set_screen(Screen::EditWorld(folder));
        self.world_name
            .set_value(&name, (FORM_W - FIELD_TEXT_PAD) * gs, wf);
        self.focused_field = Some(0);
        self.world_name.set_focused(true);
    }

    /// Vanilla `EditWorldScreen`: rename plus a column of world-file actions,
    /// centred on the screen.
    pub(super) fn build_edit_world(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Some(folder) = self.screen_folder() else {
            return empty_result(2.0);
        };

        let gs = self.gui_scale(screen_w, screen_h);
        let fs = common::FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = common::BTN_H * gs;
        let field_h = FIELD_H * gs;
        let spacing = EDIT_SPACING * gs;
        let spacer = EDIT_SPACER * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.set_screen(Screen::WorldList);
            return empty_result(2.0);
        }

        self.cycle_fields(input, 1);

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let cx = screen_w / 2.0;
        let form_x = cx - form_w / 2.0;
        let button = |elements: &mut Vec<MenuElement>,
                      hovered: &mut bool,
                      x: f32,
                      y: f32,
                      w: f32,
                      label: &str,
                      enabled: bool| {
            push_button(
                elements, hovered, cursor, x, y, w, btn_h, gs, label, enabled,
            )
        };

        elements.push(MenuElement::Text {
            x: cx,
            y: 15.0 * gs,
            text: "Edit World".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });

        // Spacer, label, field, five buttons, spacer, Save row.
        let column_h = spacer * 2.0 + fs + field_h + btn_h * 6.0 + spacing * 8.0;
        let mut y = (screen_h - column_h) / 2.0 + spacer + spacing;

        self.labelled_field(
            &mut elements,
            input,
            "World Name",
            TextTarget::WorldName,
            cx,
            &mut y,
            form_w,
            spacing,
            gs,
            text_width_fn,
        );
        y += spacing;

        // Vanilla enables Reset Icon only while an icon file exists, and none
        // can until worlds are playable.
        let icon = self.saves_dir.join(&folder).join("icon.png");
        button(
            &mut elements,
            &mut any_hovered,
            form_x,
            y,
            form_w,
            "Reset Icon",
            icon.is_file(),
        );
        y += btn_h + spacing;

        if button(
            &mut elements,
            &mut any_hovered,
            form_x,
            y,
            form_w,
            "Open World Folder",
            true,
        ) && clicked
        {
            let _ = open::that_detached(self.saves_dir.join(&folder));
        }
        y += btn_h + spacing;

        for label in ["Make Backup", "Open Backups Folder", "Optimize World"] {
            button(
                &mut elements,
                &mut any_hovered,
                form_x,
                y,
                form_w,
                label,
                false,
            );
            push_hover_tooltip(
                &mut elements,
                cursor,
                screen_w,
                screen_h,
                gs,
                [form_x, y, form_w, btn_h],
                "Not available yet",
            );
            y += btn_h + spacing;
        }
        y += spacer + spacing;

        let gap = BTN_GAP * gs;
        let half = (form_w - gap) / 2.0;
        let name = self.world_name.value().trim().to_owned();
        let save_hit = button(
            &mut elements,
            &mut any_hovered,
            form_x,
            y,
            half,
            "Save",
            !name.is_empty(),
        );
        // Vanilla also saves on Enter while the name field is focused.
        let enter = self.focused_field == Some(0) && input.enter;
        if !name.is_empty() && ((save_hit && clicked) || enter) {
            if let Err(e) = self.world_list.rename(&folder, &name) {
                tracing::error!("Failed to rename world: {e}");
            }
            self.set_screen(Screen::WorldList);
        }
        if button(
            &mut elements,
            &mut any_hovered,
            form_x + half + gap,
            y,
            half,
            "Cancel",
            true,
        ) && clicked
        {
            self.set_screen(Screen::WorldList);
        }

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: clicked && any_hovered,
        }
    }

    pub(super) fn build_confirm_delete_world(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Some(folder) = self.screen_folder() else {
            return empty_result(2.0);
        };
        let warning = format!(
            "'{}' will be lost forever! (A long time!)",
            self.world_display_name(&folder)
        );
        let (result, choice) = self.build_confirm(
            screen_w,
            screen_h,
            input,
            text_width_fn,
            "Are you sure you want to delete this world?",
            &warning,
            "Delete",
        );
        if choice == Some(true) {
            // TODO: blocking. Fine while a world is one small file; the layer
            // that fills them with terrain should move this off the frame thread.
            if let Err(e) = self.world_list.delete(&folder) {
                tracing::error!("Failed to delete world: {e}");
            }
            self.selected_world = None;
        }
        if choice.is_some() {
            self.set_screen(Screen::WorldList);
        }
        result
    }

    /// A world's display name, falling back to its folder if it has gone.
    fn world_display_name(&self, folder: &str) -> String {
        self.world_list
            .get(folder)
            .map_or_else(|| folder.to_owned(), |w| w.name.clone())
    }

    /// The folder of whichever world screen is showing.
    fn screen_folder(&self) -> Option<String> {
        match &self.screen {
            Screen::EditWorld(f) | Screen::ConfirmDeleteWorld(f) => Some(f.clone()),
            _ => None,
        }
    }
}
