//! The brewing stand screen (vanilla `BrewingStandScreen` +
//! `BrewingStandMenu`): bottle slots 0..3, ingredient 3, fuel 4, player
//! inventory 5..32, hotbar 32..41.

use std::time::Instant;

use azalea_inventory::ItemStack;

use super::container::{
    ContainerInput, ContainerResult, DragState, Panel, SlotCtx, centred_title_x, gesture_result,
    push_clipped_sprite, push_cursor_stack, push_panel,
};
use crate::player::menu_click::ContainerKind;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const SLOT_INGREDIENT: u16 = 3;
const SLOT_FUEL: u16 = 4;
const SLOT_MAIN_BASE: u16 = 5;
const SLOT_HOTBAR_BASE: u16 = 32;

const BOTTLES: [(f32, f32); 3] = [(56.0, 51.0), (79.0, 58.0), (102.0, 51.0)];

/// Indices into the menu's data values, matching vanilla
/// `BrewingStandBlockEntity`. The totals are 26.3's; older servers send only
/// the first two.
const DATA_BREW_TIME: usize = 0;
const DATA_FUEL: usize = 1;
const DATA_TOTAL_BREW_TIME: usize = 2;
const DATA_TOTAL_FUEL: usize = 3;

/// Vanilla `BUBBLELENGTHS`, indexed by `brewTime / 2 % 7`.
const BUBBLE_LENGTHS: [i32; 7] = [29, 24, 20, 16, 11, 6, 0];

#[allow(clippy::too_many_arguments)]
pub fn build_brewing_stand(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    slots: &[ItemStack],
    data: &[i16],
    has_totals: bool,
    title: &str,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> ContainerResult {
    let kind = ContainerKind::BrewingStand;
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        166.0,
        SpriteId::BrewingStandBackground,
    );
    push_progress(elements, &panel, data, has_totals);
    panel.label(elements, centred_title_x(title, text_width_fn), 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    let mut ctx = SlotCtx::new(elements, &panel, cursor, kind, slots, cursor_item, drag);
    let item = |num: u16| slots.get(num as usize).unwrap_or(&ItemStack::Empty);
    for (num, (x, y)) in (0..).zip(BOTTLES) {
        ctx.slot(x, y, item(num), Some(SpriteId::EmptyPotion), num);
    }
    ctx.slot(79.0, 17.0, item(SLOT_INGREDIENT), None, SLOT_INGREDIENT);
    ctx.slot(
        17.0,
        17.0,
        item(SLOT_FUEL),
        Some(SpriteId::EmptyBrewingFuel),
        SLOT_FUEL,
    );
    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE, 84.0);

    let (hovered, shown_cursor) = ctx.finish(cursor_item);
    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    gesture_result(
        input,
        hovered,
        &panel,
        cursor,
        kind,
        slots,
        cursor_item,
        drag,
        last_click,
    )
}

/// Vanilla `extractBackground`'s overlays: the fuel bar (left-anchored), the
/// brew arrow (top-anchored) and the bubbles (bottom-anchored), each cropped
/// like `blitSprite`'s source rectangle.
fn push_progress(elements: &mut Vec<MenuElement>, panel: &Panel, data: &[i16], has_totals: bool) {
    let fuel = fuel_length(data, has_totals);
    if fuel > 0 {
        push_clipped_sprite(
            elements,
            panel,
            SpriteId::BrewingFuelLength,
            [60.0, 44.0, 18.0, 4.0],
            [60.0, 44.0, fuel as f32, 4.0],
        );
    }
    let Some(arrow) = brew_length(data, has_totals) else {
        return;
    };
    if arrow > 0 {
        push_clipped_sprite(
            elements,
            panel,
            SpriteId::BrewingProgress,
            [97.0, 16.0, 9.0, 28.0],
            [97.0, 16.0, 9.0, arrow as f32],
        );
    }
    let bubbles = BUBBLE_LENGTHS[(i32::from(data[DATA_BREW_TIME]) / 2 % 7) as usize];
    if bubbles > 0 {
        let top = 14.0 + (29 - bubbles) as f32;
        push_clipped_sprite(
            elements,
            panel,
            SpriteId::BrewingBubbles,
            [63.0, 14.0, 12.0, 29.0],
            [63.0, top, 12.0, bubbles as f32],
        );
    }
}

/// The fuel bar's width out of 18: 26.3 scales by the total fuel
/// (`positiveCeilDiv(18 * fuel, totalFuel)`), older versions by a fixed 20.
fn fuel_length(data: &[i16], has_totals: bool) -> i32 {
    let fuel = i32::from(data[DATA_FUEL]);
    if !has_totals {
        return ((18 * fuel + 20 - 1) / 20).clamp(0, 18);
    }
    let total = i32::from(data[DATA_TOTAL_FUEL]);
    if total > 0 {
        // `Mth.positiveCeilDiv`: `-floorDiv(-a, b)`.
        (-(-18 * fuel).div_euclid(total)).clamp(0, 18)
    } else {
        0
    }
}

/// The brew arrow's height out of 28 while brewing, `None` when idle: 26.3
/// scales by the total brew time, older versions by a fixed 400 ticks.
fn brew_length(data: &[i16], has_totals: bool) -> Option<i32> {
    let ticks = i32::from(data[DATA_BREW_TIME]);
    let total = if has_totals {
        i32::from(data[DATA_TOTAL_BREW_TIME])
    } else {
        400
    };
    (ticks > 0 && total > 0).then(|| (28.0 * (1.0 - ticks as f32 / total as f32)) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(values: &[i16]) -> [i16; 10] {
        let mut d = [0; 10];
        d[..values.len()].copy_from_slice(values);
        d
    }

    #[test]
    fn fuel_bar_rounds_up_like_vanilla() {
        assert_eq!(fuel_length(&data(&[0, 20]), false), 18);
        assert_eq!(fuel_length(&data(&[0, 1]), false), 1);
        assert_eq!(fuel_length(&data(&[0, 0]), false), 0);
        assert_eq!(fuel_length(&data(&[0, 10, 0, 40]), true), 5);
        assert_eq!(fuel_length(&data(&[0, 11, 0, 40]), true), 5);
        assert_eq!(fuel_length(&data(&[0, 12, 0, 40]), true), 6);
        assert_eq!(fuel_length(&data(&[0, 10, 0, 0]), true), 0);
    }

    #[test]
    fn brew_arrow_grows_as_the_timer_counts_down() {
        assert_eq!(brew_length(&data(&[0]), false), None);
        assert_eq!(brew_length(&data(&[400]), false), Some(0));
        assert_eq!(brew_length(&data(&[200]), false), Some(14));
        assert_eq!(brew_length(&data(&[1]), false), Some(27));
        assert_eq!(brew_length(&data(&[100, 0, 200]), true), Some(14));
        assert_eq!(brew_length(&data(&[100, 0, 0]), true), None);
    }
}
