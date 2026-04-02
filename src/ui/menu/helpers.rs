use super::*;
use crate::ui::server_list::MotdSpan;

pub(super) fn empty_result(blur: f32) -> MainMenuResult {
    MainMenuResult {
        elements: Vec::new(),
        action: MenuAction::None,
        cursor_pointer: false,
        blur,
        clicked_button: false,
    }
}

pub(super) fn push_separator(elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32) {
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
        color: COL_SEP,
    });
}

pub(super) fn push_outline(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
) {
    let t = 1.0 * gs;
    let c = WHITE;
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h: t,
        corner_radius: 0.0,
        color: c,
    });
    elements.push(MenuElement::Rect {
        x,
        y: y + h - t,
        w,
        h: t,
        corner_radius: 0.0,
        color: c,
    });
    elements.push(MenuElement::Rect {
        x,
        y: y + t,
        w: t,
        h: h - t * 2.0,
        corner_radius: 0.0,
        color: c,
    });
    elements.push(MenuElement::Rect {
        x: x + w - t,
        y: y + t,
        w: t,
        h: h - t * 2.0,
        corner_radius: 0.0,
        color: c,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_button(
    elements: &mut Vec<MenuElement>,
    any_hovered: &mut bool,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    let hovered = common::push_button(
        elements,
        cursor,
        x,
        y,
        w,
        h,
        gs,
        common::FONT_SIZE * gs,
        label,
        enabled,
    );
    *any_hovered |= hovered;
    hovered
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_text_field(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fs: f32,
    gs: f32,
    text: &str,
    focused: bool,
    cursor_blink: &Instant,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let border = if focused {
        FIELD_BORDER_FOCUS
    } else {
        FIELD_BORDER
    };
    elements.push(MenuElement::Rect {
        x: x - gs,
        y: y - gs,
        w: w + gs * 2.0,
        h: h + gs * 2.0,
        corner_radius: 0.0,
        color: border,
    });
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
        color: FIELD_BG,
    });

    let pad = 4.0 * gs;
    elements.push(MenuElement::Text {
        x: x + pad,
        y: y + (h - fs) / 2.0,
        text: text.into(),
        scale: fs,
        color: WHITE,
        centered: false,
    });

    if focused {
        let text_w = text_width_fn(text, fs);
        common::push_cursor_blink(
            elements,
            cursor_blink,
            x + pad,
            y + (h - fs) / 2.0,
            gs,
            fs,
            text_w,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_server_status(
    elements: &mut Vec<MenuElement>,
    ping_results: &std::collections::HashMap<String, PingState>,
    address: &str,
    text_x: f32,
    motd_y: f32,
    entry_rect: &[f32; 4],
    fs: f32,
    gs: f32,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let Some(state) = ping_results.get(address) else {
        elements.push(MenuElement::Text {
            x: text_x,
            y: motd_y,
            text: address.into(),
            scale: fs,
            color: COL_DARK_DIM,
            centered: false,
        });
        return;
    };

    let icon_w = 10.0 * gs;
    let icon_h = 8.0 * gs;
    let icon_x = entry_rect[0] + entry_rect[2] - icon_w - 5.0 * gs;
    let icon_y = entry_rect[1];

    match state {
        PingState::Pinging => {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let frame = match (millis / 100) % 8 {
                f if f > 4 => 8 - f,
                f => f,
            };
            let sprite = match frame {
                0 => SpriteId::Pinging1,
                1 => SpriteId::Pinging2,
                2 => SpriteId::Pinging3,
                3 => SpriteId::Pinging4,
                _ => SpriteId::Pinging5,
            };
            elements.push(MenuElement::Image {
                x: icon_x,
                y: icon_y,
                w: icon_w,
                h: icon_h,
                sprite,
                tint: WHITE,
            });
            elements.push(MenuElement::Text {
                x: text_x,
                y: motd_y,
                text: "Pinging...".into(),
                scale: fs,
                color: COL_DARK_DIM,
                centered: false,
            });
        }
        PingState::Success {
            motd,
            online,
            max,
            latency_ms,
            version,
            protocol_match,
            player_names,
            ..
        } => {
            let motd_max_w = entry_rect[2] - 32.0 * gs - 2.0 * gs;
            let line_h = fs * 1.2;
            let lines = wrap_motd_spans(motd, motd_max_w, fs, text_width_fn);
            for (i, line) in lines.iter().take(2).enumerate() {
                elements.push(MenuElement::McText {
                    x: text_x,
                    y: motd_y + i as f32 * line_h,
                    spans: line.clone(),
                    scale: fs,
                    centered: false,
                });
            }

            let status_sprite = if !protocol_match {
                SpriteId::Incompatible
            } else {
                ping_sprite(*latency_ms)
            };
            elements.push(MenuElement::Image {
                x: icon_x,
                y: icon_y,
                w: icon_w,
                h: icon_h,
                sprite: status_sprite,
                tint: WHITE,
            });

            let status_text = if !protocol_match {
                version.clone()
            } else {
                format!("{online}/{max}")
            };
            let status_color = if !protocol_match {
                COL_RED
            } else {
                COL_DARK_DIM
            };
            let pw = text_width_fn(&status_text, fs);
            let status_x = icon_x - pw - 5.0 * gs;
            elements.push(MenuElement::Text {
                x: status_x,
                y: entry_rect[1] + 1.0 * gs,
                text: status_text,
                scale: fs,
                color: status_color,
                centered: false,
            });

            if common::hit_test(cursor, [icon_x, icon_y, icon_w, icon_h]) {
                let tip = if !protocol_match {
                    "Incompatible version!".to_string()
                } else {
                    format!("{latency_ms} ms")
                };
                common::push_tooltip(elements, cursor, screen_w, screen_h, gs, &tip);
            } else if common::hit_test(cursor, [status_x, entry_rect[1], pw, fs])
                && !player_names.is_empty()
            {
                let tip = player_names.join("\n");
                common::push_tooltip(elements, cursor, screen_w, screen_h, gs, &tip);
            }
        }
        PingState::Failed(err) => {
            let display = if err.len() > 40 {
                "Can't connect to server"
            } else {
                err
            };
            elements.push(MenuElement::Text {
                x: text_x,
                y: motd_y,
                text: display.into(),
                scale: fs,
                color: COL_RED,
                centered: false,
            });
            elements.push(MenuElement::Image {
                x: icon_x,
                y: icon_y,
                w: icon_w,
                h: icon_h,
                sprite: SpriteId::Unreachable,
                tint: WHITE,
            });
        }
    }
}

fn wrap_motd_spans(
    spans: &[MotdSpan],
    max_w: f32,
    fs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> Vec<Vec<MotdSpan>> {
    let mut lines: Vec<Vec<MotdSpan>> = Vec::new();
    let mut current_line: Vec<MotdSpan> = Vec::new();
    let mut current_w: f32 = 0.0;

    for span in spans {
        let make_span = |text: String| MotdSpan {
            text,
            color: span.color,
            bold: span.bold,
            italic: span.italic,
            strikethrough: span.strikethrough,
            underline: span.underline,
        };

        for part in span.text.split_inclusive([' ', '\n']) {
            if part.contains('\n') {
                let text = part.trim_end_matches('\n');
                if !text.is_empty() {
                    current_line.push(make_span(text.to_string()));
                }
                lines.push(std::mem::take(&mut current_line));
                current_w = 0.0;
                continue;
            }

            let part_w = text_width_fn(part, fs);
            if current_w + part_w > max_w && !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
                current_w = 0.0;
            }
            current_w += part_w;
            if let Some(last) = current_line.last_mut()
                && last.color == span.color
                && last.bold == span.bold
            {
                last.text.push_str(part);
                continue;
            }
            current_line.push(make_span(part.to_string()));
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

fn ping_sprite(ms: u64) -> SpriteId {
    if ms < 150 {
        SpriteId::Ping5
    } else if ms < 300 {
        SpriteId::Ping4
    } else if ms < 600 {
        SpriteId::Ping3
    } else if ms < 1000 {
        SpriteId::Ping2
    } else {
        SpriteId::Ping1
    }
}

pub(super) fn push_bottom_text(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let fs = 7.0 * gs;
    let pad = 4.0 * gs;
    let y = screen_h - pad - fs;
    let col = [0.39, 0.55, 0.78, 0.3];

    elements.push(MenuElement::Text {
        x: pad,
        y,
        text: "Minecraft 1.21.11".into(),
        scale: fs,
        color: col,
        centered: false,
    });

    let name = "POMC";
    let tag = "early dev";
    let tag_size = fs * 0.65;
    let gap = 2.0 * gs;
    let nw = text_width_fn(name, fs);
    let tw = text_width_fn(tag, tag_size);
    let nx = screen_w - pad - nw - gap - tw;
    elements.push(MenuElement::Text {
        x: nx,
        y,
        text: name.into(),
        scale: fs,
        color: col,
        centered: false,
    });
    elements.push(MenuElement::Text {
        x: nx + nw + gap,
        y,
        text: tag.into(),
        scale: tag_size,
        color: col,
        centered: false,
    });
}

pub(super) struct DropdownStyle {
    pub(super) item_h: f32,
    radius: f32,
    font: f32,
    icon_scale: f32,
    pad: f32,
}

impl DropdownStyle {
    pub(super) fn new(gs: f32) -> Self {
        Self {
            item_h: 28.0 * gs,
            radius: 5.0 * gs,
            font: 9.0 * gs,
            icon_scale: 11.0 * gs,
            pad: 10.0 * gs,
        }
    }

    pub(super) fn draw_background(
        &self,
        elements: &mut Vec<MenuElement>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        elements.push(MenuElement::Rect {
            x,
            y,
            w,
            h,
            corner_radius: self.radius,
            color: [0.08, 0.08, 0.12, 0.92],
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_item(
        &self,
        elements: &mut Vec<MenuElement>,
        any_hovered: &mut bool,
        cursor: (f32, f32),
        drop_x: f32,
        drop_y: f32,
        drop_w: f32,
        index: usize,
        count: usize,
        label: &str,
        icon: Option<(char, [f32; 4])>,
        hover_color: [f32; 4],
        normal_color: [f32; 4],
    ) -> bool {
        let iy = drop_y + index as f32 * self.item_h;
        let rect = [drop_x, iy, drop_w, self.item_h];
        let hovered = common::hit_test(cursor, rect);
        *any_hovered |= hovered;

        if hovered {
            let r = if index == 0 || index == count - 1 {
                self.radius
            } else {
                0.0
            };
            elements.push(MenuElement::Rect {
                x: drop_x,
                y: iy,
                w: drop_w,
                h: self.item_h,
                corner_radius: r,
                color: [1.0, 1.0, 1.0, 0.08],
            });
        }

        if let Some((icon_char, icon_col)) = icon {
            elements.push(MenuElement::Icon {
                x: drop_x + self.pad + self.icon_scale / 2.0,
                y: iy + self.item_h / 2.0,
                icon: icon_char,
                scale: self.icon_scale,
                color: if hovered { hover_color } else { icon_col },
            });
        }

        elements.push(MenuElement::Text {
            x: drop_x + self.pad + self.icon_scale + 6.0,
            y: iy + (self.item_h - self.font) / 2.0,
            text: label.to_string(),
            scale: self.font,
            color: if hovered { hover_color } else { normal_color },
            centered: false,
        });

        hovered
    }
}

pub(super) fn ease_out_cubic(t: f32) -> f32 {
    let t = 1.0 - t;
    1.0 - t * t * t
}

pub(super) fn dismiss_dropdown(
    cursor: (f32, f32),
    clicked: bool,
    clicked_inside: bool,
    dropdown: [f32; 4],
    anchor: [f32; 4],
) -> bool {
    clicked
        && !clicked_inside
        && !common::hit_test(cursor, dropdown)
        && !common::hit_test(cursor, anchor)
}

pub(super) fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn emit_transition_strips(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    close_t: f32,
    open_t: f32,
) {
    let strip_w = screen_w / STRIP_COUNT as f32 + 1.0;
    let strip_h = screen_h * 2.0;
    let wave_spread = 0.3;
    for i in 0..STRIP_COUNT {
        let fi = i as f32 / STRIP_COUNT as f32;
        let close_ease =
            smoothstep(((close_t - fi * wave_spread) / (1.0 - wave_spread)).clamp(0.0, 1.0));
        let ri = (STRIP_COUNT - 1 - i) as f32 / STRIP_COUNT as f32;
        let open_ease =
            smoothstep(((open_t - ri * wave_spread) / (1.0 - wave_spread)).clamp(0.0, 1.0));
        let y = -strip_h + close_ease * screen_h - open_ease * screen_h;
        let sx = i as f32 * (strip_w - 1.0);
        let hue_shift = fi * 0.08;
        elements.push(MenuElement::Rect {
            x: sx,
            y,
            w: strip_w,
            h: strip_h,
            corner_radius: 0.0,
            color: [0.04 + hue_shift, 0.02, 0.12 + hue_shift * 0.5, 1.0],
        });
        elements.push(MenuElement::Rect {
            x: sx,
            y,
            w: 1.0,
            h: strip_h,
            corner_radius: 0.0,
            color: [0.3, 0.15, 0.5, 0.3 * (1.0 - open_ease)],
        });
    }
}
