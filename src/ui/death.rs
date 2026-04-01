use crate::renderer::pipelines::menu_overlay::MenuElement;

use super::common;

const BTN_W: f32 = 200.0;

pub enum DeathAction {
    None,
    Respawn,
    TitleScreen,
    ShowConfirm,
}

fn push_gradient(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32) {
    elements.push(MenuElement::GradientRect {
        x: 0.0,
        y: 0.0,
        w: screen_w,
        h: screen_h,
        corner_radius: 0.0,
        color_top: [0.314, 0.0, 0.0, 0.376],
        color_bottom: [0.125, 0.0, 0.0, 0.627],
    });
}

#[allow(clippy::too_many_arguments)]
pub fn build_death_screen(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    gs: f32,
    message: &str,
    ticks: u32,
) -> DeathAction {
    let mut action = DeathAction::None;
    let fs = common::FONT_SIZE * gs;
    let btn_h = common::BTN_H * gs;
    let btn_w = BTN_W * gs;
    let cx = screen_w / 2.0;
    let buttons_enabled = ticks >= 20;

    push_gradient(elements, screen_w, screen_h);

    let title_fs = fs * 2.0;
    elements.push(MenuElement::Text {
        x: cx,
        y: 60.0 * gs,
        text: "You Died!".into(),
        scale: title_fs,
        color: [1.0, 1.0, 1.0, 1.0],
        centered: true,
    });

    if !message.is_empty() {
        elements.push(MenuElement::Text {
            x: cx,
            y: 85.0 * gs,
            text: message.into(),
            scale: fs,
            color: [1.0, 1.0, 1.0, 1.0],
            centered: true,
        });
    }

    let score_label = "Score: ";
    let score_value = "0";
    let char_w = fs * 0.6;
    let label_w = char_w * score_label.len() as f32;
    let value_w = char_w * score_value.len() as f32;
    let total_w = label_w + value_w;
    let score_x = cx - total_w / 2.0;
    elements.push(MenuElement::Text {
        x: score_x,
        y: 100.0 * gs,
        text: score_label.into(),
        scale: fs,
        color: [1.0, 1.0, 1.0, 1.0],
        centered: false,
    });
    elements.push(MenuElement::Text {
        x: score_x + label_w,
        y: 100.0 * gs,
        text: score_value.into(),
        scale: fs,
        color: [1.0, 1.0, 0.33, 1.0],
        centered: false,
    });

    let respawn_y = screen_h / 4.0 + 72.0 * gs;
    let h = common::push_button(
        elements,
        cursor,
        cx - btn_w / 2.0,
        respawn_y,
        btn_w,
        btn_h,
        gs,
        fs,
        "Respawn",
        buttons_enabled,
    );
    if clicked && h {
        action = DeathAction::Respawn;
    }

    let title_y = screen_h / 4.0 + 96.0 * gs;
    let h = common::push_button(
        elements,
        cursor,
        cx - btn_w / 2.0,
        title_y,
        btn_w,
        btn_h,
        gs,
        fs,
        "Title Screen",
        buttons_enabled,
    );
    if clicked && h {
        action = DeathAction::ShowConfirm;
    }

    action
}

pub fn build_death_confirm(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    gs: f32,
    ticks: u32,
) -> DeathAction {
    let mut action = DeathAction::None;
    let fs = common::FONT_SIZE * gs;
    let btn_h = common::BTN_H * gs;
    let btn_w = BTN_W * gs;
    let cx = screen_w / 2.0;
    let cy = screen_h / 2.0;
    let buttons_enabled = ticks >= 20;

    push_gradient(elements, screen_w, screen_h);

    elements.push(MenuElement::Text {
        x: cx,
        y: cy - 30.0 * gs,
        text: "Are you sure you want to quit?".into(),
        scale: fs,
        color: [1.0, 1.0, 1.0, 1.0],
        centered: true,
    });

    let gap = 4.0 * gs;
    let btn_y = cy + 10.0 * gs;
    let h = common::push_button(
        elements,
        cursor,
        cx - btn_w / 2.0,
        btn_y,
        btn_w,
        btn_h,
        gs,
        fs,
        "Title Screen",
        buttons_enabled,
    );
    if clicked && h {
        action = DeathAction::TitleScreen;
    }

    let h = common::push_button(
        elements,
        cursor,
        cx - btn_w / 2.0,
        btn_y + btn_h + gap,
        btn_w,
        btn_h,
        gs,
        fs,
        "Respawn",
        buttons_enabled,
    );
    if clicked && h {
        action = DeathAction::Respawn;
    }

    action
}
