use azalea_core::position::BlockPos;

use super::common::WHITE;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

pub struct FrameTimings {
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub present_ms: f32,
}

pub struct DebugInfo<'a> {
    pub fps: u32,
    pub position: glam::Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub target_block: Option<(BlockPos, azalea_core::direction::Direction, String)>,
    pub chunk_count: u32,
    pub gpu_name: &'a str,
    pub vulkan_version: &'a str,
    pub screen_w: u32,
    pub screen_h: u32,
    pub timings: Option<FrameTimings>,
}

const CROSSHAIR_SIZE: f32 = 10.0;
const CROSSHAIR_THICKNESS: f32 = 2.0;

const HOTBAR_W: f32 = 182.0;
const HOTBAR_H: f32 = 22.0;
const SELECTION_W: f32 = 24.0;
const SELECTION_H: f32 = 24.0;
const SLOT_STRIDE: f32 = 20.0;
const ICON_SIZE: f32 = 9.0;
const ICON_STRIDE: f32 = 8.0;

pub fn max_gui_scale(screen_w: f32, screen_h: f32) -> u32 {
    let mut scale = 1;
    while (screen_w / (scale + 1) as f32) >= 320.0 && (screen_h / (scale + 1) as f32) >= 240.0 {
        scale += 1;
    }
    scale
}

pub fn gui_scale(screen_w: f32, screen_h: f32, setting: u32) -> f32 {
    let max = max_gui_scale(screen_w, screen_h);
    if setting == 0 {
        max as f32
    } else {
        setting.min(max) as f32
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_hud(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    selected_slot: u8,
    health: f32,
    food: u32,
    air_supply: i32,
    debug: Option<&DebugInfo<'_>>,
    gui_scale_setting: u32,
) {
    let gs = gui_scale(screen_w, screen_h, gui_scale_setting);
    let cx = screen_w / 2.0;
    let cy = screen_h / 2.0;

    build_crosshair(elements, cx, cy);

    if let Some(info) = debug {
        build_debug_overlay(elements, info, gs);
    }

    let hotbar_w = HOTBAR_W * gs;
    let hotbar_h = HOTBAR_H * gs;
    let hotbar_x = cx - hotbar_w / 2.0;
    let hotbar_y = screen_h - hotbar_h;

    elements.push(MenuElement::Image {
        x: hotbar_x,
        y: hotbar_y,
        w: hotbar_w,
        h: hotbar_h,
        sprite: SpriteId::Hotbar,
        tint: WHITE,
    });

    let sel_w = SELECTION_W * gs;
    let sel_h = SELECTION_H * gs;
    let sel_x = hotbar_x - 1.0 * gs + selected_slot as f32 * SLOT_STRIDE * gs;
    let sel_y = hotbar_y - 1.0 * gs;
    elements.push(MenuElement::Image {
        x: sel_x,
        y: sel_y,
        w: sel_w,
        h: sel_h,
        sprite: SpriteId::HotbarSelection,
        tint: WHITE,
    });

    build_status_bar(
        elements,
        hotbar_x,
        hotbar_y - 2.0 * gs,
        health,
        false,
        SpriteId::HeartContainer,
        SpriteId::HeartFull,
        SpriteId::HeartHalf,
        gs,
    );
    build_status_bar(
        elements,
        hotbar_x + hotbar_w,
        hotbar_y - 2.0 * gs,
        food as f32,
        true,
        SpriteId::FoodEmpty,
        SpriteId::FoodFull,
        SpriteId::FoodHalf,
        gs,
    );

    if air_supply < crate::player::MAX_AIR_SUPPLY {
        let bubble_y = hotbar_y - 2.0 * gs - ICON_SIZE * gs - 1.0 * gs;
        let bubbles = (air_supply.max(0) as f32 / 30.0).ceil() as u8;
        let stride = ICON_STRIDE * gs;
        let icon_size = ICON_SIZE * gs;
        for i in 0..10u8 {
            let bx = hotbar_x + hotbar_w - (i as f32 + 1.0) * stride;
            let color = if i < bubbles {
                [0.3, 0.6, 1.0, 0.9]
            } else {
                [0.2, 0.2, 0.2, 0.4]
            };
            elements.push(MenuElement::Rect {
                x: bx + 1.0 * gs,
                y: bubble_y + 1.0 * gs,
                w: icon_size - 2.0 * gs,
                h: icon_size - 2.0 * gs,
                corner_radius: icon_size / 2.0,
                color,
            });
        }
    }
}

fn build_crosshair(elements: &mut Vec<MenuElement>, cx: f32, cy: f32) {
    elements.push(MenuElement::Rect {
        x: cx - CROSSHAIR_SIZE,
        y: cy - CROSSHAIR_THICKNESS / 2.0,
        w: CROSSHAIR_SIZE * 2.0,
        h: CROSSHAIR_THICKNESS,
        corner_radius: 0.0,
        color: WHITE,
    });
    elements.push(MenuElement::Rect {
        x: cx - CROSSHAIR_THICKNESS / 2.0,
        y: cy - CROSSHAIR_SIZE,
        w: CROSSHAIR_THICKNESS,
        h: CROSSHAIR_SIZE * 2.0,
        corner_radius: 0.0,
        color: WHITE,
    });
}

#[allow(clippy::too_many_arguments)]
fn build_status_bar(
    elements: &mut Vec<MenuElement>,
    x_start: f32,
    y: f32,
    value: f32,
    right_to_left: bool,
    bg: SpriteId,
    full: SpriteId,
    half: SpriteId,
    gs: f32,
) {
    let icon_size = ICON_SIZE * gs;
    let stride = ICON_STRIDE * gs;
    let full_icons = (value / 2.0).floor() as u8;
    let has_half = (value % 2.0) >= 1.0;

    for i in 0..10u8 {
        let x = if right_to_left {
            x_start - (i as f32 + 1.0) * stride
        } else {
            x_start + i as f32 * stride
        };
        let iy = y - icon_size;

        elements.push(MenuElement::Image {
            x,
            y: iy,
            w: icon_size,
            h: icon_size,
            sprite: bg,
            tint: WHITE,
        });

        let icon = if i < full_icons {
            Some(full)
        } else if i == full_icons && has_half {
            Some(half)
        } else {
            None
        };
        if let Some(sprite) = icon {
            elements.push(MenuElement::Image {
                x,
                y: iy,
                w: icon_size,
                h: icon_size,
                sprite,
                tint: WHITE,
            });
        }
    }
}

fn build_debug_overlay(elements: &mut Vec<MenuElement>, info: &DebugInfo<'_>, gs: f32) {
    let fs = super::common::FONT_SIZE * gs;
    let pad = 4.0 * gs;

    let pos = info.position;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let cx = bx.div_euclid(16);
    let cz = bz.div_euclid(16);
    let facing = facing_name(info.yaw);
    let yaw_deg = info.yaw.to_degrees();
    let pitch_deg = info.pitch.to_degrees();

    let mut left_lines: Vec<String> = vec![
        format!("POMC ({}fps)", info.fps),
        String::new(),
        format!("XYZ: {:.3} / {:.5} / {:.3}", pos.x, pos.y, pos.z),
        format!("Block: {} {} {}", bx, by, bz),
        format!(
            "Chunk: {} {} in [{}, {}]",
            bx.rem_euclid(16),
            bz.rem_euclid(16),
            cx,
            cz
        ),
        format!("Facing: {} ({:.1} / {:.1})", facing, yaw_deg, pitch_deg),
        String::new(),
        format!("Chunks: {} loaded", info.chunk_count),
    ];

    if let Some((target, face, name)) = &info.target_block {
        left_lines.push(String::new());
        left_lines.push(format!(
            "Targeted Block: {}, {}, {}",
            target.x, target.y, target.z
        ));
        left_lines.push(format!("minecraft:{name}"));
        left_lines.push(format!("Face: {:?}", face));
    }

    push_debug_lines(elements, &left_lines, pad, pad, fs, true);

    let mut right_lines: Vec<String> = vec![
        info.vulkan_version.to_string(),
        format!("GPU: {}", info.gpu_name),
        format!("Display: {}x{}", info.screen_w, info.screen_h),
    ];

    if let Some(t) = &info.timings {
        right_lines.push(String::new());
        right_lines.push(format!("Frame: {:.2}ms", t.frame_ms));
        right_lines.push(format!("  Fence: {:.2}ms", t.fence_ms));
        right_lines.push(format!("  Acquire: {:.2}ms", t.acquire_ms));
        right_lines.push(format!("  Cull: {:.2}ms", t.cull_ms));
        right_lines.push(format!("  Draw: {:.2}ms", t.draw_ms));
        right_lines.push(format!("  Present: {:.2}ms", t.present_ms));
    }
    let right_x = info.screen_w as f32 - pad;
    push_debug_lines(elements, &right_lines, right_x, pad, fs, false);
}

fn push_debug_lines(
    elements: &mut Vec<MenuElement>,
    lines: &[String],
    x: f32,
    start_y: f32,
    fs: f32,
    left_align: bool,
) {
    let line_h = fs * 1.25;
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let y = start_y + i as f32 * line_h;
        let tx = if left_align {
            x
        } else {
            x - line.len() as f32 * fs * 0.6
        };
        elements.push(MenuElement::Text {
            x: tx,
            y,
            text: line.clone(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
    }
}

fn facing_name(yaw: f32) -> &'static str {
    let deg = yaw.to_degrees().rem_euclid(360.0);
    match deg as u32 {
        315..=360 | 0..=44 => "South (+Z)",
        45..=134 => "West (-X)",
        135..=224 => "North (-Z)",
        225..=314 => "East (+X)",
        _ => "South (+Z)",
    }
}
