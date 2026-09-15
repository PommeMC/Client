use super::*;
use crate::ui::text::TextSpan;

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

/// Renders a `TextFieldState` (vanilla EditBox port): border, background, the
/// horizontally-scrolled display window, the selection highlight, and the caret
/// (1px bar in insert mode, trailing `_` glyph otherwise), blinking per
/// `render_info`.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_text_field(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fs: f32,
    gs: f32,
    field: &TextFieldState,
    focused: bool,
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
    let text_x = x + pad;
    let text_y = y + (h - fs) / 2.0;
    let inner_w = w - pad * 2.0;
    let wf = |s: &str| text_width_fn(s, fs);
    let info = field.render_info(inner_w, focused, &wf);
    let value = field.value();
    let displayed = &value[info.display_start..info.display_end];

    elements.push(MenuElement::ScissorPush {
        x: text_x,
        y,
        w: inner_w,
        h,
    });
    common::push_field_text(
        elements, &info, displayed, text_x, text_y, fs, gs, gs, WHITE, None, &wf,
    );
    elements.push(MenuElement::ScissorPop);
}

/// One Tab step around a focus ring of `n` widgets (`None` = nothing focused
/// yet), wrapping at the ends. `n` must be non-zero.
pub(super) fn step_ring(cur: Option<usize>, n: usize, reverse: bool) -> usize {
    match cur {
        Some(f) if reverse => (f + n - 1) % n,
        Some(f) => (f + 1) % n,
        None if reverse => n - 1,
        None => 0,
    }
}

/// Feed a keyboard-focused but unmoused widget its own center as the cursor,
/// so the hover sprite paints (vanilla treats focused == hovered).
pub(super) fn focus_cursor(
    focused: bool,
    hovered: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cursor: (f32, f32),
) -> (f32, f32) {
    if focused && !hovered {
        (x + w / 2.0, y + h / 2.0)
    } else {
        cursor
    }
}

/// Per-frame keyboard focus state threaded through a screen's widget builders.
pub(super) struct FocusCtx {
    /// Running index assigned to each focusable as it is built.
    pub(super) next_index: usize,
    /// The focused widget index (from `MainMenu::focus`), if any.
    pub(super) focus: Option<usize>,
    /// Left button pressed this frame; a click on a widget focuses it.
    pub(super) clicked: bool,
    /// `MainMenu::screen_gen` at build time.
    pub(super) screen_gen: u32,
    /// Enter / Space pressed this frame (`InputWithModifiers.isSelection`).
    pub(super) activate: bool,
    /// Set once a keyboard activation fires, so the click sound still plays.
    pub(super) fired: bool,
}

impl FocusCtx {
    /// Claims the next focus index (only enabled widgets join the ring,
    /// matching vanilla Tab navigation) and reports whether the widget is
    /// focused. A click takes focus (`ContainerEventHandler.mouseClicked`);
    /// widgets built earlier in the frame see that next frame.
    pub(super) fn focused(&mut self, enabled: bool, hovered: bool) -> bool {
        if !enabled {
            return false;
        }
        let idx = self.next_index;
        self.next_index += 1;
        if self.clicked && hovered {
            self.focus = Some(idx);
        }
        self.focus == Some(idx)
    }
}

/// A focusable vanilla-style button. Assigns itself the next focus index,
/// paints the hover sprite when keyboard-focused (vanilla treats focused ==
/// hovered), and returns whether it was activated this frame — a mouse click on
/// it, or Enter/Space while focused.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_button_f(
    elements: &mut Vec<MenuElement>,
    ctx: &mut FocusCtx,
    any_hovered: &mut bool,
    cursor: (f32, f32),
    clicked: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    let real_hovered = enabled && common::hit_test(cursor, [x, y, w, h]);
    let focused = ctx.focused(enabled, real_hovered);
    let draw_cursor = focus_cursor(focused, real_hovered, x, y, w, h, cursor);
    common::push_button(
        elements,
        draw_cursor,
        x,
        y,
        w,
        h,
        gs,
        common::FONT_SIZE * gs,
        label,
        enabled,
    );
    *any_hovered |= real_hovered;
    let keyboard = focused && ctx.activate;
    if keyboard {
        ctx.fired = true;
    }
    (real_hovered && clicked) || keyboard
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

    let content_pad = SERVER_ENTRY_PAD * gs;
    let icon_w = 10.0 * gs;
    let icon_h = 8.0 * gs;
    let icon_x = entry_rect[0] + entry_rect[2] - content_pad - icon_w - 5.0 * gs;
    let icon_y = entry_rect[1] + content_pad;

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
            compat,
            player_names,
            ..
        } => {
            let motd_max_w = entry_rect[2] - content_pad * 2.0 - 32.0 * gs - 2.0 * gs;
            let line_h = fs * 1.2;
            let lines = wrap_motd_spans(motd, motd_max_w, fs, text_width_fn);
            for (i, line) in lines.iter().take(2).enumerate() {
                elements.push(MenuElement::McText {
                    x: text_x,
                    y: motd_y + i as f32 * line_h,
                    spans: line.clone(),
                    scale: fs,
                    centered: false,
                    shadow: true,
                });
            }

            let incompatible = *compat == Compat::Incompatible;
            let status_sprite = if incompatible {
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

            let status_text = if incompatible {
                version.clone()
            } else {
                format!("{online}/{max}")
            };
            let status_color = if incompatible { COL_RED } else { COL_DARK_DIM };
            let pw = text_width_fn(&status_text, fs);
            let status_x = icon_x - pw - 5.0 * gs;
            elements.push(MenuElement::Text {
                x: status_x,
                y: icon_y + 1.0 * gs,
                text: status_text,
                scale: fs,
                color: status_color,
                centered: false,
            });

            if common::hit_test(cursor, [icon_x, icon_y, icon_w, icon_h]) {
                if incompatible {
                    common::push_tooltip(
                        elements,
                        cursor,
                        screen_w,
                        screen_h,
                        gs,
                        "Incompatible version!",
                    );
                } else {
                    let mut lines = Vec::new();
                    if *compat == Compat::Translated {
                        lines.push(TooltipLine::new(
                            format!("Server version: {version}"),
                            WHITE,
                        ));
                    }
                    lines.push(TooltipLine::right_aligned(
                        format!("{latency_ms} ms"),
                        ping_color(*latency_ms),
                    ));
                    common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
                }
            } else if common::hit_test(cursor, [status_x, icon_y, pw, fs])
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
    spans: &[TextSpan],
    max_w: f32,
    fs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> Vec<Vec<TextSpan>> {
    let mut lines: Vec<Vec<TextSpan>> = Vec::new();
    let mut current_line: Vec<TextSpan> = Vec::new();
    let mut current_w: f32 = 0.0;

    for span in spans {
        let make_span = |text: String| TextSpan {
            text,
            color: span.color,
            bold: span.bold,
            italic: span.italic,
            strikethrough: span.strikethrough,
            underline: span.underline,
            obfuscated: span.obfuscated,
            shadow_color: span.shadow_color,
            font: span.font.clone(),
            inline_object: None,
            component_style: span.component_style.clone(),
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

fn ping_color(ms: u64) -> [f32; 4] {
    match ping_sprite(ms) {
        SpriteId::Ping5 => [0.33, 0.87, 0.33, 1.0],
        SpriteId::Ping4 | SpriteId::Ping3 => [0.92, 0.65, 0.2, 1.0],
        _ => COL_RED,
    }
}

pub(super) fn push_bottom_text(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    version: &str,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let fs = 7.0 * gs;
    let pad = 4.0 * gs;
    let y = screen_h - pad - fs;
    let col = [0.39, 0.55, 0.78, 0.3];

    elements.push(MenuElement::Text {
        x: pad,
        y,
        text: format!("Minecraft {version}"),
        scale: fs,
        color: col,
        centered: false,
    });

    let name = "Pomme";
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

    /// Draws an open list with its bottom edge at `drop_bottom` and returns
    /// the row clicked this frame. Clears `open` on a row click or a click
    /// away; `anchor` is the toggling icon, so clicking it doesn't count.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_list(
        &self,
        elements: &mut Vec<MenuElement>,
        any_hovered: &mut bool,
        cursor: (f32, f32),
        clicked: bool,
        open: &mut bool,
        anchor: [f32; 4],
        drop_x: f32,
        drop_bottom: f32,
        drop_w: f32,
        items: &[DropItem],
    ) -> Option<usize> {
        if !*open {
            return None;
        }
        let total_h = items.len() as f32 * self.item_h;
        let drop_y_top = drop_bottom - total_h;
        self.draw_background(elements, drop_x, drop_y_top, drop_w, total_h);
        let mut picked = None;
        for (i, item) in items.iter().enumerate() {
            let hovered = self.draw_item(
                elements,
                any_hovered,
                cursor,
                drop_x,
                drop_y_top,
                drop_w,
                i,
                items.len(),
                item.label,
                item.icon,
                DROP_TEXT_BRIGHT,
                item.color,
            );
            if clicked && hovered {
                picked = Some(i);
            }
        }
        // A row click closes; so does a click outside the list and its anchor.
        let outside = !common::hit_test(cursor, [drop_x, drop_y_top, drop_w, total_h])
            && !common::hit_test(cursor, anchor);
        if picked.is_some() || (clicked && outside) {
            *open = false;
        }
        picked
    }

    fn draw_background(&self, elements: &mut Vec<MenuElement>, x: f32, y: f32, w: f32, h: f32) {
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
    fn draw_item(
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

/// One row of a [`DropdownStyle`] list.
pub(super) struct DropItem {
    pub(super) label: &'static str,
    pub(super) icon: Option<(char, [f32; 4])>,
    pub(super) color: [f32; 4],
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

/// Tile pitch of the menu backdrop, in GUI units.
pub(super) const MENU_BG_TILE: f32 = 32.0;

/// The dimmed tiled backdrop drawn behind menu content regions.
pub(super) fn push_menu_backdrop(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
) {
    elements.push(MenuElement::TiledImage {
        x,
        y,
        w,
        h,
        sprite: SpriteId::MenuBackground,
        tile_size: MENU_BG_TILE * gs,
        tint: [0.25, 0.25, 0.25, 1.0],
    });
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 0.3],
    });
}

/// Vertical bounds of a header/footer screen, in framebuffer pixels.
pub(super) struct ChromeLayout {
    pub header_h: f32,
    pub content_top: f32,
    pub content_bottom: f32,
    pub done_y: f32,
}

/// Draws the shared header/footer frame: a dimmed tiled background between two
/// separators, with the title centered in the header.
pub(super) fn push_screen_chrome(
    elements: &mut Vec<MenuElement>,
    sw: f32,
    sh: f32,
    gs: f32,
    title: &str,
) -> ChromeLayout {
    let fs = common::FONT_SIZE * gs;
    let cx = sw / 2.0;
    let header_h = HEADER_FOOTER_H * gs;
    let footer_h = HEADER_FOOTER_H * gs;
    let sep_h = 2.0 * gs;
    let content_top = header_h + sep_h;
    let content_bottom = sh - footer_h - sep_h;

    push_menu_backdrop(
        elements,
        0.0,
        content_top,
        sw,
        content_bottom - content_top,
        gs,
    );
    elements.push(MenuElement::Text {
        x: cx,
        y: (header_h - fs) / 2.0,
        text: title.into(),
        scale: fs,
        color: WHITE,
        centered: true,
    });
    elements.push(MenuElement::Image {
        x: 0.0,
        y: header_h,
        w: sw,
        h: sep_h,
        sprite: SpriteId::HeaderSeparator,
        tint: WHITE,
    });
    elements.push(MenuElement::Image {
        x: 0.0,
        y: content_bottom,
        w: sw,
        h: sep_h,
        sprite: SpriteId::FooterSeparator,
        tint: WHITE,
    });
    ChromeLayout {
        header_h,
        content_top,
        content_bottom,
        done_y: sh - footer_h + (footer_h - common::BTN_H * gs) / 2.0,
    }
}

/// The 200-wide Done button vanilla centres in a header/footer screen's footer.
pub(super) fn push_done_button(
    elements: &mut Vec<MenuElement>,
    ctx: &mut FocusCtx,
    any_hovered: &mut bool,
    input: &MenuInput,
    chrome: &ChromeLayout,
    cx: f32,
    gs: f32,
) -> bool {
    let w = 200.0 * gs;
    push_button_f(
        elements,
        ctx,
        any_hovered,
        input.cursor,
        input.clicked,
        cx - w / 2.0,
        chrome.done_y,
        w,
        common::BTN_H * gs,
        gs,
        "Done",
        true,
    )
}

/// What sits inside an icon button: a GUI sprite at its native size, or one of
/// Pomme's Font Awesome glyphs for the entries vanilla has no sprite for.
pub(super) enum IconFace {
    Sprite { id: SpriteId, w: f32, h: f32 },
    Glyph(char),
}

/// Font Awesome glyphs carry their own padding, so they sit a little smaller
/// than the 15-unit sprites to read at the same weight.
const GLYPH_ICON_SIZE: f32 = 12.0;

/// Vanilla `SpriteIconButton`: a widget-button frame with the face centred on
/// it, rather than stretched to fill. `highlighted` paints the hover sprite,
/// which vanilla also uses for keyboard focus.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_icon_widget(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    gs: f32,
    face: IconFace,
    enabled: bool,
    highlighted: bool,
) {
    let (sprite, border) = if !enabled {
        (SpriteId::ButtonDisabled, 1.0)
    } else if highlighted {
        (SpriteId::ButtonHover, 3.0)
    } else {
        (SpriteId::ButtonNormal, 3.0)
    };
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w: size,
        h: size,
        sprite,
        border: border * gs,
        tint: WHITE,
    });
    match face {
        IconFace::Sprite { id, w, h } => {
            // `CenteredIcon` centres in integer GUI units.
            let units = size / gs;
            let center =
                |extent: f32, sprite: f32| ((extent / 2.0).floor() - (sprite / 2.0).floor()) * gs;
            elements.push(MenuElement::Image {
                x: x + center(units, w),
                y: y + center(units, h),
                w: w * gs,
                h: h * gs,
                sprite: id,
                tint: WHITE,
            });
        }
        IconFace::Glyph(icon) => elements.push(MenuElement::Icon {
            x: x + size / 2.0,
            y: y + size / 2.0,
            icon,
            scale: GLYPH_ICON_SIZE * gs,
            color: WHITE,
        }),
    }
}

const DROP_TEXT: [f32; 4] = [0.89, 0.90, 0.96, 0.85];
const DROP_TEXT_BRIGHT: [f32; 4] = [0.94, 0.95, 0.98, 1.0];
const DROP_ACCENT: [f32; 4] = [0.39, 0.71, 1.0, 0.9];
const DROP_LINK_ICON: [f32; 4] = [0.6, 0.7, 0.85, 0.8];

const LINKS: [(&str, char, &str); 3] = [
    ("Website", ICON_GLOBE, "https://pomme.rs"),
    ("Discord", ICON_COMMENT, "https://discord.gg/ucBA55bHPR"),
    (
        "GitHub",
        ICON_CODE,
        "https://github.com/PommeMC/Pomme-Client",
    ),
];
const THEMES: [(&str, PanoramaTheme); 2] = [
    ("Pomme", PanoramaTheme::Pomme),
    ("Default", PanoramaTheme::Default),
];

/// The Pomme link and theme dropdowns and the theme wipe, shared by both
/// title screens.
impl MainMenu {
    /// Opening either dropdown closes the other.
    pub(super) fn toggle_links(&mut self) {
        self.links_open = !self.links_open;
        self.theme_open = false;
    }

    pub(super) fn toggle_theme(&mut self) {
        self.theme_open = !self.theme_open;
        self.links_open = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_links_dropdown(
        &mut self,
        elements: &mut Vec<MenuElement>,
        any_hovered: &mut bool,
        cursor: (f32, f32),
        clicked: bool,
        style: &DropdownStyle,
        anchor: [f32; 4],
        drop_x: f32,
        drop_bottom: f32,
        drop_w: f32,
    ) {
        let items = LINKS.map(|(label, icon, _)| DropItem {
            label,
            icon: Some((icon, DROP_LINK_ICON)),
            color: DROP_TEXT,
        });
        if let Some(i) = style.push_list(
            elements,
            any_hovered,
            cursor,
            clicked,
            &mut self.links_open,
            anchor,
            drop_x,
            drop_bottom,
            drop_w,
            &items,
        ) {
            let _ = open::that(LINKS[i].2);
        }
    }

    /// Picking a different theme starts the wipe; the swap itself lands in
    /// `drive_theme_transition` once the strips have closed.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_theme_dropdown(
        &mut self,
        elements: &mut Vec<MenuElement>,
        any_hovered: &mut bool,
        cursor: (f32, f32),
        clicked: bool,
        style: &DropdownStyle,
        anchor: [f32; 4],
        drop_x: f32,
        drop_bottom: f32,
        drop_w: f32,
    ) {
        let items = THEMES.map(|(label, theme)| {
            let selected = theme == self.theme;
            DropItem {
                label,
                icon: selected.then_some((ICON_CHECK, DROP_ACCENT)),
                color: if selected { DROP_ACCENT } else { DROP_TEXT },
            }
        });
        if let Some(i) = style.push_list(
            elements,
            any_hovered,
            cursor,
            clicked,
            &mut self.theme_open,
            anchor,
            drop_x,
            drop_bottom,
            drop_w,
            &items,
        ) && THEMES[i].1 != self.theme
        {
            self.transition = Some(ThemeTransition {
                start: Instant::now(),
                target: THEMES[i].1,
                reloaded: false,
                open_start: None,
            });
        }
    }

    /// Advances the theme wipe, committing and persisting the new theme under
    /// the closed strips. Returns the reload action on the frame it commits.
    pub(super) fn drive_theme_transition(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
    ) -> Option<MenuAction> {
        let mut action = None;
        let mut committed = false;
        if let Some(ref mut tr) = self.transition {
            let close_t = (tr.start.elapsed().as_secs_f32() / CLOSE_DURATION).min(1.0);
            if close_t >= 1.0 && !tr.reloaded {
                tr.reloaded = true;
                self.theme = tr.target;
                committed = true;
                action = Some(MenuAction::ChangeTheme(tr.target));
            }
            let open_t = tr
                .open_start
                .map(|s| (s.elapsed().as_secs_f32() / OPEN_DURATION).min(1.0))
                .unwrap_or(0.0);
            emit_transition_strips(elements, screen_w, screen_h, close_t, open_t);
            if open_t >= 1.0 {
                self.transition = None;
            }
        }
        if committed {
            self.save_settings();
        }
        action
    }
}

/// Vanilla EditBox hint: shown only while the field is empty and unfocused.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_field_hint(
    elements: &mut Vec<MenuElement>,
    field: &TextFieldState,
    focused: bool,
    x: f32,
    y: f32,
    field_h: f32,
    fs: f32,
    gs: f32,
    hint: &str,
) {
    if field.value().is_empty() && !focused {
        elements.push(MenuElement::Text {
            x: x + 4.0 * gs,
            y: y + (field_h - fs) / 2.0,
            text: hint.into(),
            scale: fs,
            color: COL_DIM,
            centered: false,
        });
    }
}

pub(super) fn nine_slice(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    sprite: SpriteId,
    border: f32,
) {
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite,
        border,
        tint: WHITE,
    });
}

pub(super) fn push_scrollbar(
    elements: &mut Vec<MenuElement>,
    right_x: f32,
    top: f32,
    h: f32,
    total: f32,
    scroll: f32,
    gs: f32,
) {
    let max_scroll = (total - h).max(0.0);
    if max_scroll <= 0.0 {
        return;
    }
    // 6px track, inset 2px from the content's right edge (vanilla spacing).
    let track_w = 6.0 * gs;
    let track_x = right_x - track_w - 2.0 * gs;
    let thumb_h = (h * h / total).max(16.0 * gs); // vanilla min thumb is larger
    let thumb_y = top + (scroll / max_scroll) * (h - thumb_h);
    nine_slice(
        elements,
        track_x,
        top,
        track_w,
        h,
        SpriteId::ScrollerBackground,
        gs,
    );
    nine_slice(
        elements,
        track_x,
        thumb_y,
        track_w,
        thumb_h,
        SpriteId::Scroller,
        gs,
    );
}

const CONFIRM_BTN_W: f32 = 150.0;

impl MainMenu {
    /// Vanilla `ConfirmScreen`: question, warning and a yes/no row, centred on
    /// the screen. `Some(true)` on yes, `Some(false)` on no or Escape.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_confirm(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        question: &str,
        warning: &str,
        yes: &str,
    ) -> (MainMenuResult, Option<bool>) {
        if input.escape {
            return (empty_result(2.0), Some(false));
        }

        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let btn_w = CONFIRM_BTN_W * gs;
        let gap = BTN_GAP * gs;
        let spacing = 8.0 * gs;
        let row_pad = 16.0 * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;
        let cx = screen_w / 2.0;

        let column_h = (fs + spacing) * 2.0 + row_pad + btn_h;
        let mut y = (screen_h - column_h) / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;
        for text in [question, warning] {
            elements.push(MenuElement::Text {
                x: cx,
                y,
                text: text.into(),
                scale: fs,
                color: WHITE,
                centered: true,
            });
            y += fs + spacing;
        }
        y += row_pad;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);
        let mut choice = None;
        for (i, (label, x)) in [(yes, cx - btn_w - gap / 2.0), ("Cancel", cx + gap / 2.0)]
            .into_iter()
            .enumerate()
        {
            if push_button_f(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                cursor,
                clicked,
                x,
                y,
                btn_w,
                btn_h,
                gs,
                label,
                true,
            ) {
                choice = Some(i == 0);
            }
        }
        self.finish_focus(&ctx);

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        (
            MainMenuResult {
                elements,
                action: MenuAction::None,
                cursor_pointer: any_hovered,
                blur: 2.0,
                clicked_button: (clicked && any_hovered) || ctx.fired,
            },
            choice,
        )
    }
}
