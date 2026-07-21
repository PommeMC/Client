//! F3+F4 game-mode switcher overlay, vanilla `GameModeSwitcherScreen`:
//! F4 cycles the selection while F3 stays held; releasing F3 applies it.

use super::common;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};
use crate::ui::text::TextSpan;

const SLOT_AREA: f32 = 26.0;
const SLOT_STRIDE: f32 = 31.0;
/// `GameModeIcon.values().length * 31 - 5`.
const ALL_SLOTS_WIDTH: f32 = 119.0;
const AQUA: [f32; 4] = [1.0 / 3.0, 1.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Icon order and cycle direction (`GameModeIcon`): mode id and item icon.
const ICONS: [(u8, &str, &str); 4] = [
    (1, "grass_block", "Creative Mode"),
    (0, "iron_sword", "Survival Mode"),
    (2, "map", "Adventure Mode"),
    (3, "ender_eye", "Spectator Mode"),
];

pub struct GameModeSwitcherState {
    /// Selected game-mode id (vanilla `currentlyHovered`).
    pub selected: u8,
    /// Mouse position when the overlay opened; hover only starts selecting
    /// after the mouse moves (vanilla `firstMouseX/Y`).
    first_cursor: Option<(f32, f32)>,
}

impl GameModeSwitcherState {
    /// Vanilla `getDefaultSelected`: the previous mode, else survival from
    /// creative and creative from everything else.
    pub fn open(current_mode: u8, previous_mode: Option<u8>) -> Self {
        let selected = previous_mode.unwrap_or(if current_mode == 1 { 0 } else { 1 });
        Self {
            selected,
            first_cursor: None,
        }
    }

    /// F4 while open: advance to the next mode in icon order.
    pub fn cycle(&mut self) {
        let i = icon_index(self.selected);
        self.selected = ICONS[(i + 1) % ICONS.len()].0;
        // Re-arm the hover guard like vanilla's `setFirstMousePos = false`.
        self.first_cursor = None;
    }
}

fn icon_index(mode: u8) -> usize {
    ICONS.iter().position(|(m, _, _)| *m == mode).unwrap_or(0)
}

pub fn build_game_mode_switcher(
    elements: &mut Vec<MenuElement>,
    state: &mut GameModeSwitcherState,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    gs: f32,
) {
    let cx = (screen_w / 2.0).round();
    let cy = (screen_h / 2.0).round();

    // The dialog is the top-left 125x75 of a 128x128 texture; the rest is
    // transparent, so the full sprite draws correctly.
    elements.push(MenuElement::Image {
        x: cx - 62.0 * gs,
        y: cy - 58.0 * gs,
        w: 128.0 * gs,
        h: 128.0 * gs,
        sprite: SpriteId::GameModeSwitcherBackground,
        tint: WHITE,
    });

    elements.push(MenuElement::McText {
        x: cx,
        y: cy - 51.0 * gs,
        spans: vec![TextSpan::new(
            ICONS[icon_index(state.selected)].2.into(),
            WHITE,
        )],
        scale: common::FONT_SIZE * gs,
        centered: true,
        shadow: true,
    });
    // "debug.gamemodes.select_next" with the key name in aqua.
    elements.push(MenuElement::McText {
        x: cx,
        y: cy + 5.0 * gs,
        spans: vec![
            TextSpan::new("F4".into(), AQUA),
            TextSpan::new(" Next".into(), WHITE),
        ],
        scale: common::FONT_SIZE * gs,
        centered: true,
        shadow: true,
    });

    let x0 = cx - (ALL_SLOTS_WIDTH / 2.0).floor() * gs;
    let y0 = cy - 31.0 * gs;
    if state.first_cursor.is_none() {
        state.first_cursor = Some(cursor);
    }
    let mouse_moved = state.first_cursor != Some(cursor);

    for (i, (mode, item, _)) in ICONS.iter().enumerate() {
        let x = x0 + i as f32 * SLOT_STRIDE * gs;
        elements.push(MenuElement::Image {
            x,
            y: y0,
            w: SLOT_AREA * gs,
            h: SLOT_AREA * gs,
            sprite: SpriteId::GameModeSwitcherSlot,
            tint: WHITE,
        });
        if *mode == state.selected {
            elements.push(MenuElement::Image {
                x,
                y: y0,
                w: SLOT_AREA * gs,
                h: SLOT_AREA * gs,
                sprite: SpriteId::GameModeSwitcherSelection,
                tint: WHITE,
            });
        }
        elements.push(MenuElement::ItemIcon {
            x: x + 5.0 * gs,
            y: y0 + 5.0 * gs,
            w: 16.0 * gs,
            h: 16.0 * gs,
            item_name: (*item).into(),
            tint: WHITE,
        });
        if mouse_moved && common::hit_test(cursor, [x, y0, SLOT_AREA * gs, SLOT_AREA * gs]) {
            state.selected = *mode;
        }
    }
}
