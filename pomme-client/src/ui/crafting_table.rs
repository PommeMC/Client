//! The crafting table screen (vanilla `CraftingScreen` + `CraftingMenu`):
//! result slot 0, 3x3 grid 1..9, player inventory 10..36, hotbar 37..45.

use std::time::Instant;

use azalea_inventory::ItemStack;

use super::common::SLOT_STRIDE;
use super::container::{
    ContainerInput, ContainerResult, DragState, SlotCtx, push_cursor_stack, push_panel,
    push_recipe_book_button, resolve_gesture,
};
use crate::player::menu_click::ContainerKind;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const SLOT_RESULT: u16 = 0;
const SLOT_GRID_BASE: u16 = 1;
const SLOT_MAIN_BASE: u16 = 10;
const SLOT_HOTBAR_BASE: u16 = 37;

#[allow(clippy::too_many_arguments)]
pub fn build_crafting_table(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    slots: &[ItemStack],
    title: &str,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
) -> ContainerResult {
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        166.0,
        SpriteId::CraftingTableBackground,
    );
    panel.label(elements, 29.0, 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    let mut ctx = SlotCtx::new(
        elements,
        &panel,
        cursor,
        ContainerKind::CraftingTable,
        slots,
        cursor_item,
        drag,
    );

    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE, 84.0);

    for row in 0..3u16 {
        for col in 0..3u16 {
            let num = SLOT_GRID_BASE + row * 3 + col;
            let item = slots.get(num as usize).unwrap_or(&ItemStack::Empty);
            ctx.slot(
                30.0 + col as f32 * SLOT_STRIDE,
                17.0 + row as f32 * SLOT_STRIDE,
                item,
                None,
                num,
            );
        }
    }

    let result = slots.get(SLOT_RESULT as usize).unwrap_or(&ItemStack::Empty);
    ctx.slot(124.0, 35.0, result, None, SLOT_RESULT);

    let (hovered, shown_cursor) = ctx.finish(cursor_item);

    push_recipe_book_button(elements, &panel, cursor, 5.0, 34.0);
    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    let (ops, clicked_outside) = resolve_gesture(
        input,
        hovered,
        &panel,
        cursor,
        ContainerKind::CraftingTable,
        slots,
        cursor_item,
        drag,
        last_click,
    );

    ContainerResult {
        clicked_outside,
        ops,
        button: None,
    }
}
