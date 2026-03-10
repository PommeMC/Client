use crate::renderer::pipelines::menu_overlay::MenuElement;

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const BTN_NORMAL: [f32; 4] = [0.3, 0.3, 0.3, 0.8];
pub const BTN_HOVER: [f32; 4] = [0.45, 0.45, 0.55, 0.9];
pub const BTN_DISABLED: [f32; 4] = [0.12, 0.12, 0.12, 0.7];
pub const COL_DISABLED: [f32; 4] = [0.4, 0.4, 0.4, 1.0];

pub fn push_overlay(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32, alpha: f32) {
    elements.push(MenuElement::Rect {
        x: 0.0, y: 0.0, w: screen_w, h: screen_h,
        corner_radius: 0.0, color: [0.0, 0.0, 0.0, alpha],
    });
}

pub fn hit_test(cursor: (f32, f32), rect: [f32; 4]) -> bool {
    cursor.0 >= rect[0] && cursor.0 <= rect[0] + rect[2]
        && cursor.1 >= rect[1] && cursor.1 <= rect[1] + rect[3]
}

pub fn push_button(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32, y: f32, w: f32, h: f32,
    gs: f32, fs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    let hovered = enabled && hit_test(cursor, [x, y, w, h]);

    let (bg, text_col) = if !enabled {
        (BTN_DISABLED, COL_DISABLED)
    } else if hovered {
        (BTN_HOVER, WHITE)
    } else {
        (BTN_NORMAL, WHITE)
    };

    elements.push(MenuElement::Rect {
        x, y, w, h,
        corner_radius: 2.0 * gs,
        color: bg,
    });
    elements.push(MenuElement::Text {
        x: x + w / 2.0, y: y + (h - fs) / 2.0,
        text: label.into(), scale: fs,
        color: text_col, centered: true,
    });

    hovered
}

pub fn push_cursor_blink(
    elements: &mut Vec<MenuElement>,
    cursor_blink: &std::time::Instant,
    x: f32, y: f32,
    gs: f32, fs: f32,
    text_width: f32,
) {
    if cursor_blink.elapsed().as_millis() % 1000 < 500 {
        elements.push(MenuElement::Rect {
            x: x + text_width, y,
            w: 1.0 * gs, h: fs,
            corner_radius: 0.0, color: WHITE,
        });
    }
}
