use azalea_core::position::BlockPos;
use azalea_inventory::ItemStack;
use glam::DVec3;

use super::common::{FONT_SIZE, TextWidthFn, WHITE, push_item_count};
use crate::player::inventory::item_resource_name;
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
    pub position: DVec3,
    pub y_rot_deg: f32,
    pub x_rot_deg: f32,
    pub target_block: Option<(BlockPos, azalea_core::direction::Direction, String)>,
    pub chunk_count: u32,
    pub sections_drawn: u32,
    pub occlusion_on: bool,
    /// Mesh-scheduling tiers (visible, margin, hidden) of loaded columns when
    /// the visibility gate is active; `None` while it falls back to meshing
    /// all.
    pub mesh_gate: Option<(u32, u32, u32)>,
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
const XP_BAR_W: f32 = 182.0;
const XP_BAR_H: f32 = 5.0;

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
    armor: u32,
    // Gated to survival by the caller.
    air_bubbles: Option<AirBubbles>,
    eyes_in_water: bool,
    tick: u64,
    experience_level: i32,
    experience_progress: f32,
    game_mode: u8,
    hotbar: &[ItemStack],
    first_person: bool,
    debug: Option<&DebugInfo<'_>>,
    gui_scale_setting: u32,
    text_width_fn: TextWidthFn,
) {
    let gs = gui_scale(screen_w, screen_h, gui_scale_setting);
    let cx = screen_w / 2.0;
    let cy = screen_h / 2.0;

    if first_person {
        build_crosshair(elements, cx, cy);
    }

    if let Some(info) = debug {
        build_debug_overlay(elements, info, gs, text_width_fn);
    }

    let hotbar_w = HOTBAR_W * gs;
    let hotbar_h = HOTBAR_H * gs;
    let hotbar_x = (cx - hotbar_w / 2.0).round();
    let hotbar_y = (screen_h - hotbar_h).round();

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
    let sel_x = (hotbar_x - 1.0 * gs + selected_slot as f32 * SLOT_STRIDE * gs).round();
    let sel_y = (hotbar_y - 1.0 * gs).round();
    elements.push(MenuElement::Image {
        x: sel_x,
        y: sel_y,
        w: sel_w,
        h: sel_h,
        sprite: SpriteId::HotbarSelection,
        tint: WHITE,
    });

    let item_size = 16.0 * gs;
    for (i, item) in hotbar.iter().enumerate().take(9) {
        if let ItemStack::Present(data) = item {
            let ix = (hotbar_x + 3.0 * gs + i as f32 * SLOT_STRIDE * gs).round();
            let iy = (hotbar_y + 3.0 * gs).round();
            elements.push(MenuElement::ItemIcon {
                x: ix,
                y: iy,
                w: item_size,
                h: item_size,
                item_name: item_resource_name(data.kind),
                tint: WHITE,
            });
            if data.count > 1 {
                push_item_count(elements, ix, iy, item_size, gs, data.count);
            }
        }
    }

    let status_bar_y = (hotbar_y - (XP_BAR_H + 1.0 + 2.0) * gs).round();
    let is_survival = crate::player::is_survival(game_mode);
    if is_survival {
        build_status_bar(
            elements,
            hotbar_x,
            status_bar_y,
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
            status_bar_y,
            food as f32,
            true,
            SpriteId::FoodEmpty,
            SpriteId::FoodFull,
            SpriteId::FoodHalf,
            gs,
        );

        if armor > 0 {
            let armor_y = (status_bar_y - (ICON_SIZE + 1.0) * gs).round();
            build_status_bar(
                elements,
                hotbar_x,
                armor_y,
                armor as f32,
                false,
                SpriteId::ArmorEmpty,
                SpriteId::ArmorFull,
                SpriteId::ArmorHalf,
                gs,
            );
        }

        let xp_w = XP_BAR_W * gs;
        let xp_h = XP_BAR_H * gs;
        let xp_x = (cx - xp_w / 2.0).round();
        let xp_y = (hotbar_y - xp_h - 2.0 * gs).round();

        elements.push(MenuElement::Image {
            x: xp_x,
            y: xp_y,
            w: xp_w,
            h: xp_h,
            sprite: SpriteId::ExperienceBarBackground,
            tint: WHITE,
        });

        let fill_px = (experience_progress.clamp(0.0, 1.0) * XP_BAR_W).ceil() as i32;
        if fill_px > 0 {
            let fill_w = (fill_px as f32 * gs).round();
            elements.push(MenuElement::ScissorPush {
                x: xp_x,
                y: xp_y,
                w: fill_w,
                h: xp_h,
            });
            elements.push(MenuElement::Image {
                x: xp_x,
                y: xp_y,
                w: xp_w,
                h: xp_h,
                sprite: SpriteId::ExperienceBarProgress,
                tint: WHITE,
            });
            elements.push(MenuElement::ScissorPop);
        }

        if experience_level > 0 {
            let text = experience_level.to_string();
            let fs = FONT_SIZE * gs;
            let ty = (xp_y - 6.0 * gs).round();
            let shadow = [0.0, 0.0, 0.0, 1.0];
            let main = [0.5, 1.0, 0.125, 1.0];
            for (dx, dy) in [
                (1.0, 0.0),
                (-1.0, 0.0),
                (0.0, 1.0),
                (0.0, -1.0),
                (1.0, 1.0),
                (1.0, -1.0),
                (-1.0, 1.0),
                (-1.0, -1.0),
            ] {
                elements.push(MenuElement::Text {
                    x: (cx + dx * gs).round(),
                    y: (ty + dy * gs).round(),
                    text: text.clone(),
                    scale: fs,
                    color: shadow,
                    centered: true,
                });
            }
            elements.push(MenuElement::Text {
                x: cx,
                y: ty,
                text,
                scale: fs,
                color: main,
                centered: true,
            });
        }
    }

    if let Some(bubbles) = air_bubbles {
        let bubble_y = (status_bar_y - (ICON_SIZE * 2.0 + 1.0) * gs).round();
        let icon_size = ICON_SIZE * gs;
        let mut rng = crate::util::JavaRandom::new((tick as i64).wrapping_mul(312871));
        let wobbling = bubbles.empty == 10 && tick.is_multiple_of(2);
        for b in 1..=10i32 {
            let mut y = bubble_y;
            let sprite = if b <= bubbles.full {
                SpriteId::AirFull
            } else if bubbles.is_popping && b == bubbles.popping_pos && eyes_in_water {
                SpriteId::AirBursting
            } else if b <= 10 - bubbles.empty {
                continue;
            } else {
                if wobbling {
                    y += rng.next_int(2) as f32 * gs;
                }
                SpriteId::AirEmpty
            };
            let x = icon_row_x_rtl(hotbar_x + hotbar_w, b - 1, gs);
            elements.push(MenuElement::Image {
                x,
                y,
                w: icon_size,
                h: icon_size,
                sprite,
                tint: WHITE,
            });
        }
    }
}

/// Right-aligned status icon x, matching vanilla `xRight - i * 8 - 9`.
fn icon_row_x_rtl(x_right: f32, i: i32, gs: f32) -> f32 {
    (x_right - i as f32 * ICON_STRIDE * gs - ICON_SIZE * gs).round()
}

pub struct AirBubbles {
    pub full: i32,
    pub popping_pos: i32,
    pub empty: i32,
    pub is_popping: bool,
}

/// None when the air row is hidden (full air and not underwater).
pub fn air_bubbles(air_supply: i32, eyes_in_water: bool) -> Option<AirBubbles> {
    let max_air = crate::player::MAX_AIR_SUPPLY;
    if !eyes_in_water && air_supply >= max_air {
        return None;
    }
    let air = air_supply.clamp(0, max_air);
    let bubble_count =
        |offset: i32| -> i32 { (((air + offset) * 10 + max_air - 1) / max_air).clamp(0, 10) };
    let full = bubble_count(-2);
    let popping_pos = bubble_count(0);
    let empty_delay = if air == 0 || !eyes_in_water { 0 } else { 1 };
    Some(AirBubbles {
        full,
        popping_pos,
        empty: 10 - bubble_count(empty_delay),
        is_popping: full != popping_pos,
    })
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
    // Ceil like vanilla so a partial heart still shows while alive.
    let halves = value.ceil().max(0.0) as u32;
    let full_icons = (halves / 2) as u8;
    let has_half = halves % 2 == 1;

    for i in 0..10u8 {
        let x = if right_to_left {
            icon_row_x_rtl(x_start, i as i32, gs)
        } else {
            (x_start + i as f32 * stride).round()
        };
        let iy = (y - icon_size).round();

        elements.push(MenuElement::Image {
            x,
            y: iy,
            w: icon_size,
            h: icon_size,
            sprite: bg,
            tint: WHITE,
        });

        let overlay = if i < full_icons {
            Some(full)
        } else if i == full_icons && has_half {
            Some(half)
        } else {
            None
        };
        if let Some(sprite) = overlay {
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

fn build_debug_overlay(
    elements: &mut Vec<MenuElement>,
    info: &DebugInfo<'_>,
    gs: f32,
    text_width_fn: TextWidthFn,
) {
    let fs = super::common::FONT_SIZE * gs;
    let pad = 4.0 * gs;

    let pos = info.position;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let cx = bx.div_euclid(16);
    let cz = bz.div_euclid(16);
    let facing = facing_name(info.y_rot_deg);
    let y_rot_deg = info.y_rot_deg;
    let x_rot_deg = info.x_rot_deg;

    let mut left_lines: Vec<String> = vec![
        format!("Pomme ({}fps)", info.fps),
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
        format!("Facing: {} ({:.1} / {:.1})", facing, y_rot_deg, x_rot_deg),
        String::new(),
        format!("Chunks: {} loaded", info.chunk_count),
        format!(
            "Sections drawn: {} (occlusion {})",
            info.sections_drawn,
            if info.occlusion_on { "on" } else { "off" }
        ),
        match info.mesh_gate {
            Some((vis, margin, hidden)) => {
                format!("Mesh gate: vis {vis} / margin {margin} / hidden {hidden}")
            }
            None => "Mesh gate: off (meshing all)".to_string(),
        },
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

    push_debug_lines(elements, &left_lines, pad, pad, fs, true, text_width_fn);

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
    push_debug_lines(
        elements,
        &right_lines,
        right_x,
        pad,
        fs,
        false,
        text_width_fn,
    );
}

fn push_debug_lines(
    elements: &mut Vec<MenuElement>,
    lines: &[String],
    x: f32,
    start_y: f32,
    fs: f32,
    left_align: bool,
    text_width_fn: TextWidthFn,
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
            x - text_width_fn(line, fs)
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

fn facing_name(y_rot_deg: f32) -> &'static str {
    let deg = y_rot_deg.rem_euclid(360.0) as u32;
    match deg {
        315..=359 | 0..=44 => "South (+Z)",
        45..=134 => "West (-X)",
        135..=224 => "North (-Z)",
        225..=314 => "East (+X)",
        _ => "South (+Z)",
    }
}
