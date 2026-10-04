use super::common;
use crate::renderer::pipelines::menu_overlay::MenuElement;

const BTN_W: f32 = 200.0;
pub const BUTTON_DELAY_TICKS: u32 = 20;

pub fn buttons_ready(ticks: u32) -> bool {
    ticks >= BUTTON_DELAY_TICKS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathAction {
    None,
    Respawn,
    TitleScreen,
    ShowConfirm,
}

fn push_gradient(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32) {
    common::push_gradient_overlay(
        elements,
        screen_w,
        screen_h,
        [0.080, 0.0, 0.0, 0.376],
        [0.216, 0.029, 0.029, 0.627],
    );
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
    score: i32,
    hardcore: bool,
    buttons_enabled: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> DeathAction {
    let mut action = DeathAction::None;
    let fs = common::FONT_SIZE * gs;
    let btn_h = common::BTN_H * gs;
    let btn_w = BTN_W * gs;
    let cx = screen_w / 2.0;

    push_gradient(elements, screen_w, screen_h);

    let title_fs = fs * 2.0;
    elements.push(MenuElement::Text {
        x: cx,
        y: 30.0 * gs,
        text: if hardcore { "Game Over!" } else { "You Died!" }.into(),
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
    let score_value = score.to_string();
    let score_str = score_value.as_str();
    let label_w = text_width_fn(score_label, fs);
    let value_w = text_width_fn(score_str, fs);
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
        text: score_str.into(),
        scale: fs,
        color: [1.0, 1.0, 0.091, 1.0],
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
        if hardcore {
            "Spectate World"
        } else {
            "Respawn"
        },
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
        action = if hardcore {
            DeathAction::TitleScreen
        } else {
            DeathAction::ShowConfirm
        };
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
    buttons_enabled: bool,
) -> DeathAction {
    let mut action = DeathAction::None;
    let fs = common::FONT_SIZE * gs;
    let btn_h = common::BTN_H * gs;
    let cx = screen_w / 2.0;
    let cy = screen_h / 2.0;

    push_gradient(elements, screen_w, screen_h);

    elements.push(MenuElement::Text {
        x: cx,
        y: cy - 30.0 * gs,
        text: "Are you sure you want to quit?".into(),
        scale: fs,
        color: [1.0, 1.0, 1.0, 1.0],
        centered: true,
    });

    let confirm_btn_w = 150.0 * gs;
    let gap = 4.0 * gs;
    let btn_y = cy + 10.0 * gs;
    let total_w = confirm_btn_w * 2.0 + gap;
    let left_x = cx - total_w / 2.0;

    let h = common::push_button(
        elements,
        cursor,
        left_x,
        btn_y,
        confirm_btn_w,
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
        left_x + confirm_btn_w + gap,
        btn_y,
        confirm_btn_w,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build(hardcore: bool, cursor: (f32, f32), enabled: bool) -> (DeathAction, Vec<MenuElement>) {
        let mut elements = Vec::new();
        let action = build_death_screen(
            &mut elements,
            800.0,
            600.0,
            cursor,
            true,
            1.0,
            "",
            0,
            hardcore,
            enabled,
            &|_, _| 0.0,
        );
        (action, elements)
    }

    fn has_text(elements: &[MenuElement], expected: &str) -> bool {
        elements
            .iter()
            .any(|element| matches!(element, MenuElement::Text { text, .. } if text == expected))
    }

    #[test]
    fn normal_death_screen_uses_respawn_and_confirm_flow() {
        let (respawn, elements) = build(false, (400.0, 232.0), true);
        assert_eq!(respawn, DeathAction::Respawn);
        assert!(has_text(&elements, "You Died!"));
        assert!(has_text(&elements, "Respawn"));

        let (title, _) = build(false, (400.0, 256.0), true);
        assert_eq!(title, DeathAction::ShowConfirm);
    }

    #[test]
    fn hardcore_death_screen_uses_spectate_and_direct_exit() {
        let (spectate, elements) = build(true, (400.0, 232.0), true);
        assert_eq!(spectate, DeathAction::Respawn);
        assert!(has_text(&elements, "Game Over!"));
        assert!(has_text(&elements, "Spectate World"));

        let (title, _) = build(true, (400.0, 256.0), true);
        assert_eq!(title, DeathAction::TitleScreen);
    }

    #[test]
    fn death_buttons_ignore_clicks_until_enabled() {
        let (action, _) = build(false, (400.0, 232.0), false);
        assert_eq!(action, DeathAction::None);
    }

    #[test]
    fn death_buttons_enable_after_exactly_twenty_client_ticks() {
        assert!(!buttons_ready(19));
        assert!(buttons_ready(20));
        assert!(buttons_ready(21));
    }
}
