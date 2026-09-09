//! The singleplayer world list, ported from vanilla `SelectWorldScreen` and
//! `WorldSelectionList`.
//!
//! Its geometry differs from the server list's: a taller header to fit the
//! search box, narrower rows, and a six-button footer.

use super::*;
use crate::ui::text::TextSpan;
use crate::ui::world_list::{GameMode, WorldSummary};

const ROW_W: f32 = 270.0;
const HEADER_H: f32 = 49.0;
const FOOTER_H: f32 = 60.0;
const WIDE_BTN_W: f32 = 150.0;
const NARROW_BTN_W: f32 = 71.0;
const ICON_SIZE: f32 = 32.0;
const SEARCH_W: f32 = 200.0;

/// Vanilla's `0x808080` for the folder and info lines.
const COL_GREY: [f32; 4] = [0.502, 0.502, 0.502, 1.0];
/// Vanilla's `0xFF0000` for a hardcore world.
const COL_HARDCORE: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

impl MainMenu {
    pub(super) fn open_world_list(&mut self) {
        self.rescan_worlds();
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
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let header_h = HEADER_H * gs;
        let footer_h = FOOTER_H * gs;
        let entry_h = ENTRY_H * gs;
        let row_w = ROW_W * gs;
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
        // Vanilla EditBox hint: shown only while empty and unfocused.
        if self.world_search.value().is_empty() && self.focused_field != Some(0) {
            elements.push(MenuElement::Text {
                x: search_x + 4.0 * gs,
                y: search_y + (field_h - fs) / 2.0,
                text: "Search...".into(),
                scale: fs,
                color: COL_DIM,
                centered: false,
            });
        }

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

            let rel = (cursor.0 - icon_x, cursor.1 - icon_y);
            let on_icon =
                hovered && rel.0 >= 0.0 && rel.0 < icon_size && rel.1 >= 0.0 && rel.1 < icon_size;

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
                text: format!(
                    "{} ({})",
                    world.folder,
                    format_last_played(world.last_played)
                ),
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
                self.selected_world = Some(world.folder.clone());
            }
        }

        elements.push(MenuElement::ScissorPop);
        push_scrollbar(
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
        let grid_w = narrow_w * 4.0 + gap * 3.0;
        let grid_x = (screen_w - grid_w) / 2.0;
        let row1_y = list_bottom + (footer_h - (btn_h * 2.0 + gap)) / 2.0;
        let row2_y = row1_y + btn_h + gap;
        let col = |n: f32| grid_x + (narrow_w + gap) * n;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        // Every action but Back arrives in a later layer.
        for (x, y, w, label) in [
            (grid_x, row1_y, wide_w, "Play Selected World"),
            (col(2.0), row1_y, wide_w, "Create New World"),
            (col(0.0), row2_y, narrow_w, "Edit"),
            (col(1.0), row2_y, narrow_w, "Delete"),
            (col(2.0), row2_y, narrow_w, "Re-Create"),
        ] {
            push_button_f(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                cursor,
                clicked,
                x,
                y,
                w,
                btn_h,
                gs,
                label,
                false,
            );
            // Vanilla keys tooltips off hover alone, ignoring the active flag.
            if common::hit_test(cursor, [x, y, w, btn_h]) {
                common::push_tooltip(
                    &mut elements,
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    "Not available yet",
                );
            }
        }

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
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: (clicked && any_hovered) || ctx.fired,
        }
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
        spans.push(grey(match world.game_mode {
            GameMode::Survival => "Survival Mode".into(),
            GameMode::Creative => "Creative Mode".into(),
        }));
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
