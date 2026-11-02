//! The crafter screen (vanilla `CrafterScreen` + `CrafterMenu`): grid slots
//! 0..9, player inventory 9..36, hotbar 36..45, then the result 45. Empty grid
//! slots toggle between enabled and disabled when clicked (see
//! `crafter_slot_clicked` in the in-game phase).

use std::time::Instant;

use azalea_inventory::ItemStack;

use super::common::{self, SLOT_STRIDE};
use super::container::{
    ContainerInput, ContainerResult, DragState, SlotCtx, centred_title_x, gesture_result,
    push_cursor_stack, push_panel,
};
use crate::player::menu_click::ContainerKind;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

pub const SLOT_RESULT: u16 = 45;
const SLOT_MAIN_BASE: u16 = 9;
const SLOT_HOTBAR_BASE: u16 = 36;
/// `CrafterMenu.isPowered`'s data index; 0..9 are the slots' disabled flags.
const DATA_POWERED: usize = 9;

/// The disabled grid slots in `data` (vanilla `isSlotDisabled`: a flag of 1).
pub fn disabled_slots(data: &[i16]) -> u16 {
    (0..9)
        .filter(|&i| data[i] == 1)
        .fold(0, |mask, i| mask | 1 << i)
}

#[allow(clippy::too_many_arguments)]
pub fn build_crafter(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    slots: &[ItemStack],
    data: &[i16],
    spectator: bool,
    title: &str,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> ContainerResult {
    let disabled = disabled_slots(data);
    let kind = ContainerKind::Crafter { disabled };
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        166.0,
        SpriteId::CrafterBackground,
    );
    panel.label(elements, centred_title_x(title, text_width_fn), 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    let mut ctx = SlotCtx::new(elements, &panel, cursor, kind, slots, cursor_item, drag);
    let item = |num: u16| slots.get(num as usize).unwrap_or(&ItemStack::Empty);
    for num in 0..9u16 {
        let x = 26.0 + (num % 3) as f32 * SLOT_STRIDE;
        let y = 17.0 + (num / 3) as f32 * SLOT_STRIDE;
        if disabled & 1 << num != 0 {
            ctx.sprite_slot(x, y, SpriteId::CrafterDisabledSlot, num);
        } else {
            ctx.slot(x, y, item(num), None, num);
        }
    }
    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE, 84.0);
    ctx.unhighlighted_fake_slot(134.0, 35.0, item(SLOT_RESULT), SLOT_RESULT);

    let (hovered, shown_cursor) = ctx.finish(cursor_item);
    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    // `extractRedstone` runs after the carried item, so it draws over it.
    // (width / 2 + 9, height / 2 - 48) is the panel's (97, 35) at any size.
    let powered = data[DATA_POWERED] == 1;
    panel.image(
        elements,
        if powered {
            SpriteId::CrafterPoweredRedstone
        } else {
            SpriteId::CrafterUnpoweredRedstone
        },
        97.0,
        35.0,
        16.0,
        16.0,
    );

    // An enabled empty grid slot under an empty cursor offers to disable it.
    if let Some(num) = hovered
        && num < 9
        && disabled & 1 << num == 0
        && cursor_item.is_empty()
        && item(num).is_empty()
        && !spectator
    {
        common::push_tooltip(
            elements,
            cursor,
            screen_w,
            screen_h,
            panel.scale,
            crate::lang::translate("gui.togglable_slot").unwrap_or("Click to disable slot"),
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_slots_read_the_flags() {
        let mut data = [0i16; 10];
        data[0] = 1;
        data[4] = 1;
        data[8] = 1;
        // The powered flag isn't a slot.
        data[DATA_POWERED] = 1;
        assert_eq!(disabled_slots(&data), 0b1_0001_0001);
    }
}
