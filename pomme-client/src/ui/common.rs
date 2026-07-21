use azalea_inventory::{ItemStack, ItemStackData};

use crate::benchmark::UploadStatus;
use crate::player::inventory::item_resource_name;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const FONT_SIZE: f32 = 8.0;
pub const BTN_H: f32 = 20.0;
pub const COL_DISABLED: [f32; 4] = [0.35, 0.36, 0.45, 1.0];
pub const SLOT_SIZE: f32 = 16.0;
pub const SLOT_STRIDE: f32 = 18.0;
pub const SLOT_LABEL_COLOR: [f32; 4] = [0.25, 0.25, 0.25, 1.0];
const BTN_BORDER: f32 = 3.0;

pub const fn rgb(hex: u32) -> [f32; 4] {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    ]
}

pub fn push_tooltip(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    text: &str,
) {
    elements.push(MenuElement::Tooltip {
        x: cursor.0,
        y: cursor.1,
        text: text.into(),
        scale: FONT_SIZE * gs,
        screen_w,
        screen_h,
    });
}

pub fn push_tooltip_lines(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    lines: Vec<TooltipLine>,
) {
    elements.push(MenuElement::TooltipLines {
        x: cursor.0,
        y: cursor.1,
        lines,
        scale: FONT_SIZE * gs,
        screen_w,
        screen_h,
    });
}

pub fn push_overlay(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32, alpha: f32) {
    elements.push(MenuElement::Rect {
        x: 0.0,
        y: 0.0,
        w: screen_w,
        h: screen_h,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, alpha],
    });
}

pub fn push_gradient_overlay(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    color_top: [f32; 4],
    color_bottom: [f32; 4],
) {
    elements.push(MenuElement::GradientRect {
        x: 0.0,
        y: 0.0,
        w: screen_w,
        h: screen_h,
        corner_radius: 0.0,
        color_top,
        color_bottom,
    });
}

/// What the caller should do after a result overlay handled this frame's input.
pub enum ResultAction {
    None,
    Dismiss,
    StartUpload,
    /// Re-copy the already-uploaded link to the clipboard.
    Recopy,
}

/// A centered results panel: dimmed backdrop, a large title, a column of detail
/// lines, an upload status line, and an "upload & copy link" button. Shared by
/// the benchmark result overlays. Returns what the caller should do based on
/// the click / escape this frame.
#[allow(clippy::too_many_arguments)]
pub fn push_results_overlay(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    title_y: f32,
    title: &str,
    lines: &[String],
    upload: Option<&UploadStatus>,
    cursor: (f32, f32),
    clicked: bool,
    escape: bool,
) -> ResultAction {
    let fs = FONT_SIZE * gs;
    let cx = screen_w / 2.0;
    push_overlay(elements, screen_w, screen_h, 0.5);
    elements.push(MenuElement::Text {
        x: cx,
        y: title_y,
        text: title.into(),
        scale: fs * 2.0,
        color: WHITE,
        centered: true,
    });
    for (i, line) in lines.iter().enumerate() {
        elements.push(MenuElement::Text {
            x: cx,
            y: title_y + fs * 2.0 + 10.0 + i as f32 * (fs + 4.0),
            text: line.clone(),
            scale: fs,
            color: [0.8, 0.85, 0.9, 1.0],
            centered: true,
        });
    }
    let lines_bottom = title_y + fs * 2.0 + 10.0 + lines.len() as f32 * (fs + 4.0);

    let status = match upload {
        None => None,
        Some(UploadStatus::Uploading) => Some(("Uploading...".to_string(), [0.8, 0.85, 0.9, 1.0])),
        Some(UploadStatus::Done { url, copied }) => Some(if *copied {
            (format!("Link copied: {url}"), [0.6, 0.9, 0.6, 1.0])
        } else {
            (
                format!("Uploaded (copy failed): {url}"),
                [0.9, 0.85, 0.5, 1.0],
            )
        }),
        Some(UploadStatus::Failed(e)) => Some((e.clone(), [0.95, 0.5, 0.5, 1.0])),
    };
    if let Some((text, color)) = status {
        elements.push(MenuElement::Text {
            x: cx,
            y: lines_bottom + 6.0,
            text,
            scale: fs,
            color,
            centered: true,
        });
    }

    // Debug builds produce unrepresentative timings, so don't let them be shared.
    let debug_build = crate::benchmark::is_debug_build();
    let (label, enabled) = if debug_build {
        ("Upload disabled (debug build)", false)
    } else {
        match upload {
            Some(UploadStatus::Uploading) => ("Uploading...", false),
            Some(UploadStatus::Done { .. }) => ("Copy link again", true),
            _ => ("Upload & copy link", true),
        }
    };
    let btn_w = 180.0 * gs;
    let btn_h = BTN_H * gs;
    let btn_x = cx - btn_w / 2.0;
    let btn_y = lines_bottom + fs + 12.0;
    push_button(
        elements, cursor, btn_x, btn_y, btn_w, btn_h, gs, fs, label, enabled,
    );

    if clicked && hit_test(cursor, [btn_x, btn_y, btn_w, btn_h]) {
        if debug_build {
            return ResultAction::None;
        }
        return match upload {
            Some(UploadStatus::Uploading) => ResultAction::None,
            Some(UploadStatus::Done { .. }) => ResultAction::Recopy,
            _ => ResultAction::StartUpload,
        };
    }
    if escape || clicked {
        return ResultAction::Dismiss;
    }
    ResultAction::None
}

/// Copy `text` to the system clipboard, returning whether it succeeded.
#[cfg(target_os = "linux")]
pub(crate) fn set_clipboard(text: &str) -> bool {
    // On Linux the selection is served by the living `Clipboard` instance and
    // dies with it, so a detached thread holds it via `wait()` until another
    // app takes the clipboard over (a newer copy unblocks its predecessor).
    let text = text.to_string();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            use arboard::SetExtLinux;
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set().wait().text(text);
            }
        })
        .is_ok()
}

/// Copy `text` to the system clipboard, returning whether it succeeded.
#[cfg(not(target_os = "linux"))]
pub(crate) fn set_clipboard(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text.to_string()))
        .is_ok()
}

const DIGIT_WIDTH: f32 = 6.0;

pub fn push_item_count(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    gs: f32,
    count: i32,
) {
    let text = count.to_string();
    let char_w = DIGIT_WIDTH * gs;
    let text_w = text.len() as f32 * char_w;
    let fs = FONT_SIZE * gs;
    elements.push(MenuElement::Text {
        x: x + size + gs - text_w,
        y: y + size - fs,
        text,
        scale: fs,
        color: WHITE,
        centered: false,
    });
}

pub fn hit_test(cursor: (f32, f32), rect: [f32; 4]) -> bool {
    cursor.0 >= rect[0]
        && cursor.0 < rect[0] + rect[2]
        && cursor.1 >= rect[1]
        && cursor.1 < rect[1] + rect[3]
}

#[allow(clippy::too_many_arguments)]
pub fn push_slot(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    cursor: (f32, f32),
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
) -> bool {
    let hovered = hit_test(cursor, [x, y, size, size]);
    let highlight = |sprite| MenuElement::Image {
        x: x - 4.0 * scale,
        y: y - 4.0 * scale,
        w: 24.0 * scale,
        h: 24.0 * scale,
        sprite,
        tint: WHITE,
    };
    if hovered {
        elements.push(highlight(SpriteId::SlotHighlightBack));
    }
    match item {
        ItemStack::Empty => {
            if let Some(sprite) = empty_sprite {
                elements.push(MenuElement::Image {
                    x,
                    y,
                    w: size,
                    h: size,
                    sprite,
                    tint: WHITE,
                });
            }
        }
        ItemStack::Present(data) => push_item_icon(elements, x, y, size, scale, data),
    }
    if hovered {
        elements.push(highlight(SpriteId::SlotHighlightFront));
    }
    hovered
}

/// Draws an item icon (and its stack count when > 1) at the given position.
pub fn push_item_icon(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    data: &ItemStackData,
) {
    elements.push(MenuElement::ItemIcon {
        x,
        y,
        w: size,
        h: size,
        item_name: item_resource_name(data.kind),
        tint: WHITE,
    });
    if data.count > 1 {
        push_item_count(elements, x, y, size, scale, data.count);
    }
}

/// Measures rendered text width in framebuffer px at the given font size.
pub type TextWidthFn<'a> = &'a dyn Fn(&str, f32) -> f32;

/// Inputs for the vanilla scrolling-label treatment: labels wider than the
/// widget are clipped to it and slide back and forth over time.
pub struct LabelScroll<'a> {
    pub text_width_fn: TextWidthFn<'a>,
    pub time_secs: f64,
}

/// Widget label: centered when it fits, otherwise (with `scroll`) clipped and
/// oscillated like vanilla's ActiveTextCollector.defaultScrollingHelper.
#[allow(clippy::too_many_arguments)]
fn push_widget_label(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    color: [f32; 4],
    scroll: Option<&LabelScroll<'_>>,
) {
    let ty = y + (h - fs) / 2.0 + 1.0;
    if let Some(s) = scroll {
        let margin = 2.0 * gs;
        let avail = w - 2.0 * margin;
        let lw = (s.text_width_fn)(label, fs);
        if lw > avail {
            let max_pos = lw - avail;
            let period = ((max_pos / gs) as f64 * 0.5).max(3.0);
            let alpha = (std::f64::consts::FRAC_PI_2
                * (std::f64::consts::TAU * s.time_secs / period).cos())
            .sin()
                / 2.0
                + 0.5;
            let pos = alpha as f32 * max_pos;
            elements.push(MenuElement::ScissorPush {
                x: x + margin,
                y,
                w: avail,
                h,
            });
            elements.push(MenuElement::Text {
                x: x + margin - pos,
                y: ty,
                text: label.into(),
                scale: fs,
                color,
                centered: false,
            });
            elements.push(MenuElement::ScissorPop);
            return;
        }
    }
    elements.push(MenuElement::Text {
        x: x + w / 2.0,
        y: ty,
        text: label.into(),
        scale: fs,
        color,
        centered: true,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn push_button(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    push_button_inner(elements, cursor, x, y, w, h, gs, fs, label, enabled, None)
}

#[allow(clippy::too_many_arguments)]
pub fn push_button_scrolling(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
    scroll: &LabelScroll<'_>,
) -> bool {
    push_button_inner(
        elements,
        cursor,
        x,
        y,
        w,
        h,
        gs,
        fs,
        label,
        enabled,
        Some(scroll),
    )
}

#[allow(clippy::too_many_arguments)]
fn push_button_inner(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
    scroll: Option<&LabelScroll<'_>>,
) -> bool {
    let hovered = enabled && hit_test(cursor, [x, y, w, h]);

    let (sprite, text_col) = if !enabled {
        (SpriteId::ButtonDisabled, COL_DISABLED)
    } else if hovered {
        (SpriteId::ButtonHover, WHITE)
    } else {
        (SpriteId::ButtonNormal, WHITE)
    };

    let border = if enabled { BTN_BORDER } else { 1.0 };
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite,
        border: border * gs,
        tint: WHITE,
    });

    push_widget_label(elements, x, y, w, h, gs, fs, label, text_col, scroll);

    hovered
}

#[allow(clippy::too_many_arguments)]
pub fn push_slider(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    mouse_held: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    value: f32,
    dragging: bool,
    scroll: &LabelScroll<'_>,
) -> SliderResult {
    let hovered = hit_test(cursor, [x, y, w, h]);
    let handle_w = 8.0 * gs;
    let track_w = w - handle_w;
    let handle_x = x + value.clamp(0.0, 1.0) * track_w;

    let actively_dragging = dragging && mouse_held;
    let start_drag = hovered && mouse_held && !dragging;

    let new_value = if actively_dragging || start_drag {
        let rel = (cursor.0 - x - handle_w / 2.0) / track_w;
        Some(rel.clamp(0.0, 1.0))
    } else {
        None
    };

    let track_sprite = SpriteId::SliderTrack;
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: track_sprite,
        border: BTN_BORDER * gs,
        tint: WHITE,
    });

    let handle_sprite = if actively_dragging || start_drag || hovered {
        SpriteId::SliderHandleHover
    } else {
        SpriteId::SliderHandle
    };
    elements.push(MenuElement::Image {
        x: handle_x,
        y,
        w: handle_w,
        h,
        sprite: handle_sprite,
        tint: WHITE,
    });

    push_widget_label(elements, x, y, w, h, gs, fs, label, WHITE, Some(scroll));

    SliderResult {
        hovered,
        dragging: actively_dragging || start_drag,
        new_value,
    }
}

pub struct SliderResult {
    pub hovered: bool,
    pub dragging: bool,
    pub new_value: Option<f32>,
}

pub fn push_cursor_blink(
    elements: &mut Vec<MenuElement>,
    cursor_blink: &std::time::Instant,
    x: f32,
    y: f32,
    gs: f32,
    fs: f32,
    text_width: f32,
) {
    if cursor_blink.elapsed().as_millis() % 1000 < 500 {
        elements.push(MenuElement::Rect {
            x: x + text_width,
            y,
            w: 1.0 * gs,
            h: fs,
            corner_radius: 0.0,
            color: WHITE,
        });
    }
}
