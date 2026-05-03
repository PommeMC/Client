use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const FONT_SIZE: f32 = 8.0;
pub const BTN_H: f32 = 20.0;
pub const COL_DISABLED: [f32; 4] = [0.35, 0.36, 0.45, 1.0];
const BTN_BORDER: f32 = 3.0;

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
    let fs = DIGIT_WIDTH * gs;
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
        && cursor.0 <= rect[0] + rect[2]
        && cursor.1 >= rect[1]
        && cursor.1 <= rect[1] + rect[3]
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

    elements.push(MenuElement::Text {
        x: x + w / 2.0,
        y: y + (h - fs) / 2.0 + 1.0,
        text: label.into(),
        scale: fs,
        color: text_col,
        centered: true,
    });

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

    elements.push(MenuElement::Text {
        x: x + w / 2.0,
        y: y + (h - fs) / 2.0 + 1.0,
        text: label.into(),
        scale: fs,
        color: WHITE,
        centered: true,
    });

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
