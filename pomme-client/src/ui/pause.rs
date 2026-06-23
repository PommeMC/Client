use super::common;
use super::common::WHITE;
use crate::renderer::pipelines::menu_overlay::MenuElement;

const FULL_W: f32 = 204.0;
const HALF_W: f32 = 98.0;
const PADDING: f32 = 4.0;
const MENU_PADDING_TOP: f32 = 50.0;

/// Which pause screen is showing. The pause menu is a small stack:
/// `Main` -> `Benchmark` -> `ChunkLoader`.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseScreen {
    #[default]
    Main,
    Benchmark,
    ChunkLoader,
}

#[derive(Clone, Copy)]
pub enum PauseAction {
    None,
    Resume,
    Disconnect,
    Options,
    ReportBugs,
    OpenBenchmark,
    StartFpsBenchmark,
    OpenChunkLoader,
    StartChunkLoad(u32),
    Back,
}

#[allow(clippy::too_many_arguments)]
pub fn build_pause_menu(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    gs: f32,
    screen: PauseScreen,
    server_rd: u32,
) -> PauseAction {
    match screen {
        PauseScreen::Main => build_main(elements, screen_w, screen_h, cursor, clicked, gs),
        PauseScreen::Benchmark => build_submenu(
            elements,
            screen_w,
            screen_h,
            cursor,
            clicked,
            gs,
            "Benchmark",
            None,
            &[
                ("FPS / Frametime", PauseAction::StartFpsBenchmark),
                ("Chunk Loader", PauseAction::OpenChunkLoader),
                ("Back", PauseAction::Back),
            ],
        ),
        PauseScreen::ChunkLoader => {
            let subtitle = if server_rd > 0 {
                format!("Server render distance: {server_rd}")
            } else {
                "Server render distance: unknown".to_string()
            };
            build_submenu(
                elements,
                screen_w,
                screen_h,
                cursor,
                clicked,
                gs,
                "Chunk Loader",
                Some(&subtitle),
                &[
                    ("Render Distance 8", PauseAction::StartChunkLoad(8)),
                    ("Render Distance 16", PauseAction::StartChunkLoad(16)),
                    ("Render Distance 24", PauseAction::StartChunkLoad(24)),
                    ("Render Distance 32", PauseAction::StartChunkLoad(32)),
                    ("Back", PauseAction::Back),
                ],
            )
        }
    }
}

fn build_main(
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
        text: "Game".into(),
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
        "Return to Game",
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
    if common::push_button(
        elements,
        cursor,
        col2_x,
        row_y(2),
        half_w,
        btn_h,
        gs,
        fs,
        "Report Bugs",
        true,
    ) && clicked
    {
        action = PauseAction::ReportBugs;
    }

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
        action = PauseAction::OpenBenchmark;
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

/// A simple centered column of full-width buttons under a title, used by the
/// benchmark sub-screens.
#[allow(clippy::too_many_arguments)]
fn build_submenu(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    gs: f32,
    title: &str,
    subtitle: Option<&str>,
    items: &[(&str, PauseAction)],
) -> PauseAction {
    let mut action = PauseAction::None;
    let fs = common::FONT_SIZE * gs;

    common::push_overlay(elements, screen_w, screen_h, 0.47);

    let full_w = FULL_W * gs;
    let btn_h = common::BTN_H * gs;
    let pad = PADDING * gs;
    let top_pad = MENU_PADDING_TOP * gs;

    let n = items.len() as f32;
    let grid_h = top_pad + n * btn_h + (n - 1.0).max(0.0) * pad;
    let grid_y = (screen_h - grid_h) * 0.25;
    let x = (screen_w - full_w) / 2.0;

    let title_y = grid_y + 40.0 * gs - top_pad;
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: title_y,
        text: title.into(),
        scale: fs,
        color: WHITE,
        centered: true,
    });
    if let Some(sub) = subtitle {
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: title_y + fs * 1.4,
            text: sub.into(),
            scale: fs * 0.8,
            color: [0.7, 0.74, 0.8, 1.0],
            centered: true,
        });
    }

    for (i, (label, item_action)) in items.iter().enumerate() {
        let y = grid_y + top_pad + i as f32 * (btn_h + pad);
        if common::push_button(elements, cursor, x, y, full_w, btn_h, gs, fs, label, true)
            && clicked
        {
            action = *item_action;
        }
    }

    action
}
