//! Vanilla `DebugOptionsScreen`, opened with F3 + F6: each debug entry's
//! status, the two profiles, and a search over entry paths.

use super::*;
use crate::ui::debug_entries::{self, DebugEntryStatus, DebugProfile};
use crate::ui::text::TextSpan;

const TITLE: &str = "Debug Options";
const WARNING: &str = "These options are for testing purposes only. They may slow down your \
                       computer, crash the game, or eat your pet rock.";
const HEADER_H: f32 = 61.0;
/// The header's `LinearLayout` spacing, in both directions.
const GAP: f32 = 8.0;
/// `OptionList.getRowWidth`.
const LIST_ROW_W: f32 = 350.0;
const ITEM_H: f32 = 20.0;
const STATUS_W: f32 = 60.0;
const STATUS_H: f32 = 16.0;
const COL_WARNING: u32 = 0xDF5050;
const COL_UNSELECTED: u32 = 0xBABABA;

/// The buttons in their row order, with the colour each label takes when
/// it's the entry's status.
const STATUS_BUTTONS: [(DebugEntryStatus, &str, u32); 3] = [
    (DebugEntryStatus::Never, "OFF", 0xFFFFFF),
    (DebugEntryStatus::InOverlay, "In Overlay", 0xFFFF55),
    (DebugEntryStatus::AlwaysOn, "Always", COL_WARNING),
];

impl MainMenu {
    pub fn open_debug_options(&mut self, key_bindings: &KeyBindings) {
        self.set_screen(Screen::DebugOptions, key_bindings);
        self.scroll_offset = 0.0;
        self.debug_search.clear();
        // `setInitialFocus(searchBox)`.
        self.focused_field = Some(0);
        self.debug_search.set_focused(true);
    }

    pub fn is_debug_options(&self) -> bool {
        matches!(self.screen, Screen::DebugOptions)
    }

    /// `onClose`: back to the game.
    pub fn close_debug_options(&mut self, key_bindings: &KeyBindings) {
        self.set_screen(Screen::Main, key_bindings);
    }

    pub(super) fn build_debug_options(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: common::TextWidthFn,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        if input.escape {
            self.close_debug_options(key_bindings);
            return empty_result();
        }
        self.cycle_fields(input, 1);

        let gs = self.gui_scale(sw, sh);
        let fs = common::FONT_SIZE * gs;
        let line_h = common::LINE_HEIGHT as f32 * gs;
        let cx = sw / 2.0;
        let mut any_hovered = false;
        let mut any_clicked = false;
        let mut elements = vec![MenuElement::BlurBackground];

        self.focus_advance(input);
        let ctx = &mut self.make_focus_ctx(input);
        let chrome = push_chrome_frame(&mut elements, sw, sh, gs, HEADER_H);

        // The header's column, centred in it: the title row (a spacer, the
        // title and the search box) and the warning below.
        let warning = crate::ui::text::wrap_plain(WARNING, LIST_ROW_W, |ch| {
            text_width_fn(ch.encode_utf8(&mut [0; 4]), common::FONT_SIZE)
        });
        let column_h = common::BTN_H + GAP + (common::LINE_HEIGHT as usize * warning.len()) as f32;
        let top = ((HEADER_H - column_h) / 2.0).floor() * gs;
        let search_w = (LIST_ROW_W / 3.0).floor() * gs;
        let title_w = text_width_fn(TITLE, fs);
        let title_x = cx - (2.0 * search_w + title_w + 2.0 * GAP * gs) / 2.0 + search_w + GAP * gs;
        elements.push(MenuElement::Text {
            x: title_x,
            y: top + 5.0 * gs,
            text: TITLE.into(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
        let search_x = title_x + title_w + GAP * gs;
        let field_h = common::BTN_H * gs;
        self.text_field(
            &mut elements,
            TextTarget::DebugSearch,
            0,
            input,
            search_x,
            top,
            search_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        push_field_hint(
            &mut elements,
            &self.debug_search,
            self.focused_field == Some(0),
            search_x,
            top,
            field_h,
            fs,
            gs,
            "Search...",
        );
        for (i, line) in warning.into_iter().enumerate() {
            elements.push(MenuElement::Text {
                x: cx,
                y: top + (common::BTN_H + GAP) * gs + line_h * i as f32,
                text: line,
                scale: fs,
                color: common::rgb(COL_WARNING),
                centered: true,
            });
        }

        let entries = debug_entries::options_list(self.debug_search.value());
        let categories: Vec<_> = entries
            .chunk_by(|left, right| left.category == right.category)
            .collect();
        let item_h = ITEM_H * gs;
        let list_y = chrome.content_top;
        let list_h = chrome.content_bottom - list_y;
        let total_h = (entries.len() + categories.len()) as f32 * item_h + 4.0 * gs;
        let (row_cursor, row_clicked) =
            self.scroll_list(input, sw, list_y, list_h, total_h, item_h);

        let row_left = cx - LIST_ROW_W / 2.0 * gs;
        let row_right = row_left + LIST_ROW_W * gs;
        push_scrollbar(
            &mut elements,
            row_right + 8.0 * gs,
            list_y,
            list_h,
            total_h,
            self.scroll_offset,
            gs,
            32.0 * gs,
        );
        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: list_y,
            w: sw,
            h: list_h,
        });
        let status_w = STATUS_W * gs;
        let mut y = list_y + 2.0 * gs - self.scroll_offset;
        for category in categories {
            let mut label = TextSpan::new(category[0].category.label().into(), WHITE);
            label.bold = true;
            label.underline = true;
            elements.push(MenuElement::TextSpans {
                x: cx,
                y: y + 6.0 * gs,
                spans: vec![label],
                scale: fs,
                centered: true,
            });
            y += item_h;

            for entry in category {
                // TODO: an entry not allowed under the server's reduced debug
                // info draws its name grey and italic with a tooltip, once
                // Pomme tracks that flag.
                elements.push(MenuElement::Text {
                    x: row_left + 2.0 * gs,
                    y: y + 7.0 * gs,
                    text: entry.path().into(),
                    scale: fs,
                    color: WHITE,
                    centered: false,
                });
                let current = self.debug_entries.status(entry.id);
                let mut x = row_right - 2.0 * gs - status_w * STATUS_BUTTONS.len() as f32;
                for (status, text, selected_color) in STATUS_BUTTONS {
                    let selected = status == current;
                    let color = if selected {
                        selected_color
                    } else {
                        COL_UNSELECTED
                    };
                    if push_button_label_f(
                        &mut elements,
                        ctx,
                        &mut any_hovered,
                        row_cursor,
                        row_clicked,
                        x,
                        y + 2.0 * gs,
                        status_w,
                        STATUS_H * gs,
                        gs,
                        common::WidgetLabel::Spans(vec![TextSpan::new(
                            text.into(),
                            common::rgb(color),
                        )]),
                        !selected,
                        None,
                    ) {
                        self.debug_entries.set_status(entry.id, status);
                        any_clicked = true;
                    }
                    x += status_w;
                }
                y += item_h;
            }
        }
        elements.push(MenuElement::ScissorPop);

        // The footer row, 4 apart: both profiles, the debug GUI scale, Done.
        let gap = 4.0 * gs;
        let profile_w = 120.0 * gs;
        let scale_w = 110.0 * gs;
        let done_w = 60.0 * gs;
        let mut x = cx - (2.0 * profile_w + scale_w + done_w + 3.0 * gap) / 2.0;
        for profile in DebugProfile::ALL {
            if push_footer_button(
                &mut elements,
                ctx,
                &mut any_hovered,
                input,
                &chrome,
                x,
                profile_w,
                gs,
                profile.label(),
                !self.debug_entries.is_using_profile(profile),
            ) {
                self.debug_entries.load_profile(profile);
                any_clicked = true;
            }
            x += profile_w + gap;
        }
        // TODO: the debug GUI scale option (`Options.debugGuiScale`), which
        // draws the debug overlay at its own scale.
        push_footer_button(
            &mut elements,
            ctx,
            &mut any_hovered,
            input,
            &chrome,
            x,
            scale_w,
            gs,
            "GUI Scale: Unchanged",
            false,
        );
        x += scale_w + gap;
        if push_footer_button(
            &mut elements,
            ctx,
            &mut any_hovered,
            input,
            &chrome,
            x,
            done_w,
            gs,
            "Done",
            true,
        ) {
            self.close_debug_options(key_bindings);
            let mut result = empty_result();
            result.clicked_button = true;
            return result;
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            clicked_button: any_clicked,
        }
    }
}
