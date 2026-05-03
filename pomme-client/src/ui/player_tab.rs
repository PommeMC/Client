//! Ported 1:1 from vanilla `PlayerTabOverlay.extractRenderState`
//! (reference/26.1/decompiled/.../PlayerTabOverlay.java:103-204).

use crate::player::tab_list::{TabList, TabListPlayer};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::common::FONT_SIZE;

const MAX_ROWS_PER_COL: usize = 20;
const HEAD_COL_W: i32 = 9;
const LINE_HEIGHT: i32 = 9;
const SPECTATOR_GAME_MODE: u8 = 3;

const BG_BACKDROP: [f32; 4] = [0.0, 0.0, 0.0, 0.5]; // Integer.MIN_VALUE (0x80000000)
const BG_ROW: [f32; 4] = [1.0, 1.0, 1.0, 0x20 as f32 / 255.0]; // 0x20FFFFFF
const COL_NAME: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const COL_SPECTATOR: [f32; 4] = [1.0, 1.0, 1.0, 0x90 as f32 / 255.0]; // 0x90FFFFFF

/// `text_width(text, FONT_SIZE)` must return the width in pixels at 1x GUI scale.
pub fn build_player_tab_overlay(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    tab_list: &TabList,
    gs: f32,
    text_width: &dyn Fn(&str, f32) -> f32,
) {
    let players = tab_list.sorted_listed();
    if players.is_empty() {
        return;
    }

    let gw = (screen_w / gs).floor();
    let font_w = |s: &str| text_width(s, FONT_SIZE);

    let display_names: Vec<String> = players.iter().map(|p| display_name_for(p)).collect();
    let max_name_w: i32 = display_names
        .iter()
        .map(|n| font_w(n).ceil() as i32)
        .max()
        .unwrap_or(0);

    let slots = players.len();
    let mut rows = slots;
    let mut cols = 1usize;
    while rows > MAX_ROWS_PER_COL {
        cols += 1;
        rows = slots.div_ceil(cols);
    }

    let cols_i = cols as i32;
    let slot_width =
        ((cols_i * (HEAD_COL_W + max_name_w + 13)).min(gw as i32 - 50) / cols_i).max(1);
    let grid_w = slot_width * cols_i + (cols_i - 1) * 5;
    let xxo = (gw as i32) / 2 - grid_w / 2;
    let cx = (gw as i32) / 2;

    let wrap_width_px = (gw - 50.0).max(1.0);
    let header_lines = tab_list
        .header
        .as_ref()
        .map(|h| wrap_text(h, wrap_width_px, &font_w));
    let footer_lines = tab_list
        .footer
        .as_ref()
        .map(|f| wrap_text(f, wrap_width_px, &font_w));

    let mut max_line_w = grid_w;
    for lines in [&header_lines, &footer_lines].into_iter().flatten() {
        for l in lines {
            max_line_w = max_line_w.max(font_w(l).ceil() as i32);
        }
    }

    let push_fill =
        |elements: &mut Vec<MenuElement>, x1: i32, y1: i32, x2: i32, y2: i32, color: [f32; 4]| {
            elements.push(MenuElement::Rect {
                x: x1 as f32 * gs,
                y: y1 as f32 * gs,
                w: (x2 - x1) as f32 * gs,
                h: (y2 - y1) as f32 * gs,
                corner_radius: 0.0,
                color,
            });
        };
    let push_text =
        |elements: &mut Vec<MenuElement>, s: String, x: i32, y: i32, color: [f32; 4]| {
            elements.push(MenuElement::Text {
                x: x as f32 * gs,
                y: y as f32 * gs,
                text: s,
                scale: FONT_SIZE * gs,
                color,
                centered: false,
            });
        };
    let push_image =
        |elements: &mut Vec<MenuElement>, x: i32, y: i32, w: i32, h: i32, sprite: SpriteId| {
            elements.push(MenuElement::Image {
                x: x as f32 * gs,
                y: y as f32 * gs,
                w: w as f32 * gs,
                h: h as f32 * gs,
                sprite,
                tint: [1.0, 1.0, 1.0, 1.0],
            });
        };

    let draw_text_block = |elements: &mut Vec<MenuElement>, lines: &[String], top: i32| {
        push_fill(
            elements,
            cx - max_line_w / 2 - 1,
            top - 1,
            cx + max_line_w / 2 + 1,
            top + lines.len() as i32 * LINE_HEIGHT,
            BG_BACKDROP,
        );
        for (i, line) in lines.iter().enumerate() {
            let lw = font_w(line).ceil() as i32;
            push_text(
                elements,
                line.clone(),
                cx - lw / 2,
                top + i as i32 * LINE_HEIGHT,
                COL_NAME,
            );
        }
    };

    let mut yyo: i32 = 10;
    if let Some(lines) = &header_lines {
        draw_text_block(elements, lines, yyo);
        yyo += lines.len() as i32 * LINE_HEIGHT + 1;
    }

    push_fill(
        elements,
        cx - max_line_w / 2 - 1,
        yyo - 1,
        cx + max_line_w / 2 + 1,
        yyo + rows as i32 * 9,
        BG_BACKDROP,
    );

    for i in 0..slots {
        let col = (i / rows) as i32;
        let row = (i % rows) as i32;
        let row_left = xxo + col * slot_width + col * 5;
        let yo = yyo + row * 9;

        push_fill(
            elements,
            row_left,
            yo,
            row_left + slot_width,
            yo + 8,
            BG_ROW,
        );
        push_image(elements, row_left, yo, 8, 8, SpriteId::SteveHead);

        let info = players[i];
        let name_color = if info.game_mode == SPECTATOR_GAME_MODE {
            COL_SPECTATOR
        } else {
            COL_NAME
        };
        push_text(
            elements,
            display_names[i].clone(),
            row_left + HEAD_COL_W,
            yo,
            name_color,
        );
        push_image(
            elements,
            row_left + slot_width - 11,
            yo,
            10,
            8,
            ping_sprite(info.latency),
        );
    }

    if let Some(lines) = &footer_lines {
        yyo += rows as i32 * 9 + 1;
        draw_text_block(elements, lines, yyo);
    }
}

fn display_name_for(p: &TabListPlayer) -> String {
    p.display_name.clone().unwrap_or_else(|| p.name.clone())
}

fn ping_sprite(latency: i32) -> SpriteId {
    if latency < 0 {
        SpriteId::PingUnknown
    } else if latency < 150 {
        SpriteId::Ping5
    } else if latency < 300 {
        SpriteId::Ping4
    } else if latency < 600 {
        SpriteId::Ping3
    } else if latency < 1000 {
        SpriteId::Ping2
    } else {
        SpriteId::Ping1
    }
}

fn wrap_text(text: &str, max_width: f32, font_w: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        let mut current = String::new();
        for word in raw_line.split(' ') {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }
            if font_w(&format!("{current} {word}")) <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        lines.push(current);
    }
    lines
}
