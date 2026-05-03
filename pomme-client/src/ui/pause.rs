use super::common::{self, WHITE};
use crate::renderer::pipelines::menu_overlay::MenuElement;

const FULL_W: f32 = 204.0;
const HALF_W: f32 = 98.0;
const PADDING: f32 = 4.0;
const MENU_PADDING_TOP: f32 = 50.0;

pub enum PauseAction {
    None,
    Resume,
    Disconnect,
    Options,
    Benchmark,
}

pub fn build_pause_menu(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    gs: f32,
) -> PauseAction {
    let mut action = PauseAction::None;
    let fs = common::FONT_SIZE * gs;

    common::push_overlay(elements, screen_w, screen_h, 0.47);

    let full_w = FULL_W * gs;
    let half_w = HALF_W * gs;
    let btn_h = common::BTN_H * gs;
    let pad = PADDING * gs;
    let top_pad = MENU_PADDING_TOP * gs;

    let grid_w = (half_w + pad) * 2.0 + pad * 2.0;
    let grid_h = (top_pad + btn_h) + 4.0 * (pad + btn_h);

    let grid_x = (screen_w - grid_w) / 2.0;
    let grid_y = (screen_h - grid_h) * 0.25;

    let col1_x = grid_x + pad;
    let col2_x = col1_x + half_w + pad * 2.0;
    let full_x = col1_x;

    let row_y = |row: u32| -> f32 { grid_y + top_pad + row as f32 * (btn_h + pad) };

    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: grid_y + 40.0 * gs - top_pad,
        text: "Game Menu".into(),
        scale: fs,
        color: WHITE,
        centered: true,
    });

    if common::push_button(
        elements,
        cursor,
        full_x,
        row_y(0),
        full_w,
        btn_h,
        gs,
        fs,
        "Back to Game",
        true,
    ) && clicked
    {
        action = PauseAction::Resume;
    }

    common::push_button(
        elements,
        cursor,
        col1_x,
        row_y(1),
        half_w,
        btn_h,
        gs,
        fs,
        "Advancements",
        false,
    );
    common::push_button(
        elements,
        cursor,
        col2_x,
        row_y(1),
        half_w,
        btn_h,
        gs,
        fs,
        "Statistics",
        false,
    );

    common::push_button(
        elements,
        cursor,
        col1_x,
        row_y(2),
        half_w,
        btn_h,
        gs,
        fs,
        "Give Feedback",
        false,
    );
    common::push_button(
        elements,
        cursor,
        col2_x,
        row_y(2),
        half_w,
        btn_h,
        gs,
        fs,
        "Report Bugs",
        false,
    );

    if common::push_button(
        elements,
        cursor,
        col1_x,
        row_y(3),
        half_w,
        btn_h,
        gs,
        fs,
        "Options...",
        true,
    ) && clicked
    {
        action = PauseAction::Options;
    }
    if common::push_button(
        elements,
        cursor,
        col2_x,
        row_y(3),
        half_w,
        btn_h,
        gs,
        fs,
        "Benchmark",
        true,
    ) && clicked
    {
        action = PauseAction::Benchmark;
    }

    if common::push_button(
        elements,
        cursor,
        full_x,
        row_y(4),
        full_w,
        btn_h,
        gs,
        fs,
        "Disconnect",
        true,
    ) && clicked
    {
        action = PauseAction::Disconnect;
    }

    action
}
