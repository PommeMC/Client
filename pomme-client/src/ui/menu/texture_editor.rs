//! In-game texture editor (UI shell): browse blocks as MC renders them, pick
//! one, and see its per-face textures with a (stubbed) import affordance.
//! Actual texture import + live atlas patch is a deferred backend phase.

use super::*;
use crate::player::inventory::item_resource_name;
use crate::ui::creative_inventory::all_block_items;

const IMPORT_STUB: &str = "Texture import not yet wired";
const PANEL_BG: [f32; 4] = [0.05, 0.055, 0.11, 1.0];

impl MainMenu {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_texture_editor(
        &mut self,
        sw: f32,
        sh: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        block_textures_fn: &dyn Fn(&str) -> Vec<crate::renderer::BlockTextureRef>,
    ) -> MainMenuResult {
        if input.escape {
            self.set_screen(Screen::Main);
            return helpers::empty_result(1.0);
        }

        let pal = Pal::dark();
        let cursor = input.cursor;
        let clicked = input.clicked;
        let s = (sh / 400.0).max(1.0);

        let margin = 16.0 * s;
        let pad = 8.0 * s;
        let header_h = 28.0 * s;
        let console_h = 22.0 * s;
        let ui_fs = 9.0 * s;
        let title_fs = 13.0 * s;

        let x0 = margin;
        let x1 = sw - margin;
        let y0 = margin;
        let y1 = sh - margin;

        let header_y = y0;
        let body_y = header_y + header_h + pad;
        let body_bottom = y1 - console_h - pad;
        let detail_w = (220.0 * s).min((x1 - x0) * 0.4);
        let detail_x = x1 - detail_w;
        let grid_x = x0;
        let grid_right = detail_x - pad;

        let search_h = 20.0 * s;
        let search_w = grid_right - grid_x;
        let grid_top = body_y + search_h + pad;
        let cell = 30.0 * s;
        let gap = 6.0 * s;
        let stride = cell + gap;
        let cols = (((grid_right - grid_x) + gap) / stride).floor().max(1.0) as usize;
        let visible_rows = (((body_bottom - grid_top) + gap) / stride).floor().max(1.0) as usize;

        if !input.typed_chars.is_empty() {
            self.tex_search.extend(input.typed_chars.iter());
            self.tex_scroll = 0.0;
        }
        if input.backspace {
            self.tex_search.pop();
            self.tex_scroll = 0.0;
        }
        self.cursor_blink = if input.typed_chars.is_empty() && !input.backspace {
            self.cursor_blink
        } else {
            Instant::now()
        };

        let needle = self.tex_search.to_lowercase();
        let items: Vec<ItemKind> = all_block_items()
            .iter()
            .copied()
            .filter(|k| needle.is_empty() || item_resource_name(*k).to_lowercase().contains(&needle))
            .collect();

        let row_count = items.len().div_ceil(cols);
        let max_scroll = row_count.saturating_sub(visible_rows) as f32;
        if input.scroll_delta != 0.0 {
            self.tex_scroll = (self.tex_scroll - input.scroll_delta).clamp(0.0, max_scroll);
        }
        self.tex_scroll = self.tex_scroll.clamp(0.0, max_scroll);
        let first_row = self.tex_scroll.floor() as usize;

        let mut elements = Vec::new();
        let mut any_hovered = false;
        let mut any_clicked = false;

        elements.push(MenuElement::FrostedRect {
            x: 0.0,
            y: 0.0,
            w: sw,
            h: sh,
            corner_radius: 0.0,
            tint: [0.035, 0.04, 0.08, 0.92],
        });

        let back_w = 64.0 * s;
        let back_rect = [x0, header_y, back_w, header_h];
        let back_hover = common::hit_test(cursor, back_rect);
        any_hovered |= back_hover;
        push_panel(&mut elements, back_rect, 6.0 * s, hover_col(&pal, back_hover));
        elements.push(MenuElement::Text {
            x: x0 + back_w / 2.0,
            y: header_y + (header_h - ui_fs) / 2.0,
            text: "\u{2190} Back".into(),
            scale: ui_fs,
            color: if back_hover { pal.bright } else { pal.text },
            centered: true,
        });
        if clicked && back_hover {
            self.set_screen(Screen::Main);
            return helpers::empty_result(1.0);
        }
        elements.push(MenuElement::Text {
            x: x0 + back_w + pad * 1.5,
            y: header_y + (header_h - title_fs) / 2.0,
            text: "Texture Editor".into(),
            scale: title_fs,
            color: pal.bright,
            centered: false,
        });

        push_panel(
            &mut elements,
            [grid_x, body_y, grid_right - grid_x, body_bottom - body_y],
            7.0 * s,
            PANEL_BG,
        );

        helpers::push_text_field(
            &mut elements,
            grid_x,
            body_y,
            search_w,
            search_h,
            ui_fs,
            s,
            &self.tex_search,
            true,
            false,
            &self.cursor_blink,
            text_width_fn,
        );
        if self.tex_search.is_empty() {
            elements.push(MenuElement::Text {
                x: grid_x + 6.0 * s,
                y: body_y + (search_h - ui_fs) / 2.0,
                text: "Search blocks\u{2026}".into(),
                scale: ui_fs,
                color: pal.dim,
                centered: false,
            });
        }

        elements.push(MenuElement::ScissorPush {
            x: grid_x,
            y: grid_top,
            w: grid_right - grid_x,
            h: body_bottom - grid_top,
        });
        let mut idx = first_row * cols;
        'rows: for row in 0..visible_rows {
            for col in 0..cols {
                if idx >= items.len() {
                    break 'rows;
                }
                let kind = items[idx];
                idx += 1;
                let cx = grid_x + col as f32 * stride;
                let cy = grid_top + row as f32 * stride;
                let rect = [cx, cy, cell, cell];
                let hovered = common::hit_test(cursor, rect);
                any_hovered |= hovered;
                let selected = self.tex_selected == Some(kind);
                let cell_col = if selected {
                    pal.glass_hover
                } else if hovered {
                    pal.glass
                } else {
                    [1.0, 1.0, 1.0, 0.04]
                };
                push_panel(&mut elements, rect, 4.0 * s, cell_col);
                elements.push(MenuElement::ItemIcon {
                    x: cx,
                    y: cy,
                    w: cell,
                    h: cell,
                    item_name: item_resource_name(kind),
                    tint: WHITE,
                });
                if clicked && hovered {
                    self.tex_selected = Some(kind);
                    any_clicked = true;
                }
            }
        }
        elements.push(MenuElement::ScissorPop);

        push_panel(
            &mut elements,
            [detail_x, body_y, detail_w, body_bottom - body_y],
            7.0 * s,
            PANEL_BG,
        );
        if let Some(kind) = self.tex_selected {
            let name = item_resource_name(kind);
            let icon = 48.0 * s;
            elements.push(MenuElement::ItemIcon {
                x: detail_x + (detail_w - icon) / 2.0,
                y: body_y + pad,
                w: icon,
                h: icon,
                item_name: name.clone(),
                tint: WHITE,
            });
            elements.push(MenuElement::Text {
                x: detail_x + detail_w / 2.0,
                y: body_y + pad + icon + 4.0 * s,
                text: name.clone(),
                scale: ui_fs,
                color: pal.bright,
                centered: true,
            });

            let faces = block_textures_fn(&name);
            let mut ry = body_y + pad + icon + 4.0 * s + ui_fs + pad;
            if faces.is_empty() {
                elements.push(MenuElement::Text {
                    x: detail_x + detail_w / 2.0,
                    y: ry,
                    text: "No editable block textures".into(),
                    scale: ui_fs,
                    color: pal.dim,
                    centered: true,
                });
            }
            let row_h = 26.0 * s;
            let btn_w = 50.0 * s;
            let sw_sz = row_h - 6.0 * s;
            for (label, key, region) in &faces {
                if ry + row_h > body_bottom {
                    break;
                }
                let sw_x = detail_x + pad;
                let sw_y = ry + 3.0 * s;
                push_panel(
                    &mut elements,
                    [sw_x - 1.0 * s, sw_y - 1.0 * s, sw_sz + 2.0 * s, sw_sz + 2.0 * s],
                    0.0,
                    [0.0, 0.0, 0.0, 0.4],
                );
                elements.push(MenuElement::AtlasTexture {
                    x: sw_x,
                    y: sw_y,
                    w: sw_sz,
                    h: sw_sz,
                    region: *region,
                    tint: WHITE,
                });
                let text_x = sw_x + sw_sz + 6.0 * s;
                elements.push(MenuElement::Text {
                    x: text_x,
                    y: ry + 2.0 * s,
                    text: label.clone(),
                    scale: ui_fs,
                    color: pal.text,
                    centered: false,
                });
                elements.push(MenuElement::Text {
                    x: text_x,
                    y: ry + 2.0 * s + ui_fs + 2.0 * s,
                    text: key.clone(),
                    scale: ui_fs * 0.8,
                    color: pal.dim,
                    centered: false,
                });
                let rect = [detail_x + detail_w - pad - btn_w, sw_y, btn_w, sw_sz];
                let hovered = button(
                    &mut elements,
                    &mut any_hovered,
                    cursor,
                    rect,
                    ui_fs * 0.85,
                    "Import",
                    5.0 * s,
                    &pal,
                    true,
                );
                if clicked && hovered {
                    any_clicked = true;
                    self.tex_status = format!("{IMPORT_STUB}: {key}");
                }
                ry += row_h;
            }
        } else {
            elements.push(MenuElement::Text {
                x: detail_x + detail_w / 2.0,
                y: body_y + (body_bottom - body_y) / 2.0,
                text: "Select a block".into(),
                scale: ui_fs,
                color: pal.dim,
                centered: true,
            });
        }

        let console_y = y1 - console_h;
        push_panel(
            &mut elements,
            [x0, console_y, x1 - x0, console_h],
            6.0 * s,
            PANEL_BG,
        );
        let status = if self.tex_status.is_empty() {
            "Ready"
        } else {
            &self.tex_status
        };
        elements.push(MenuElement::Text {
            x: x0 + pad,
            y: console_y + (console_h - ui_fs) / 2.0,
            text: format!("\u{203a} {status}"),
            scale: ui_fs,
            color: pal.dim,
            centered: false,
        });

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 1.0,
            clicked_button: any_clicked,
        }
    }
}
