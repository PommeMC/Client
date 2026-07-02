use std::time::Instant;

use azalea_inventory::ItemStack;
use azalea_inventory::operations::ClickOperation;

use super::common::SLOT_STRIDE;
use super::container::{
    ContainerInput, DragState, SlotCtx, push_cursor_stack, push_panel, push_recipe_book_button,
    resolve_gesture,
};
use crate::player::inventory::{self, Inventory};
use crate::player::menu_click::ContainerKind;
use crate::renderer::PlayerPreview;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

// Vanilla player-menu slot indices, as u16 for click ops.
const SLOT_CRAFT_RESULT: u16 = inventory::CRAFT_OUTPUT as u16;
const SLOT_CRAFT_BASE: u16 = inventory::CRAFT_INPUT_START as u16;
const SLOT_ARMOR_BASE: u16 = inventory::ARMOR_START as u16;
const SLOT_MAIN_BASE: u16 = inventory::MAIN_START as u16;
const SLOT_HOTBAR_BASE: u16 = inventory::HOTBAR_START as u16;
const SLOT_OFFHAND: u16 = inventory::OFFHAND as u16;

const ARMOR_EMPTY_SPRITES: [SpriteId; 4] = [
    SpriteId::EmptyHelmet,
    SpriteId::EmptyChestplate,
    SpriteId::EmptyLeggings,
    SpriteId::EmptyBoots,
];

pub struct InventoryResult {
    pub clicked_outside: bool,
    /// Container-click operations to send this frame (usually 0-1; a drag
    /// release emits a start/add.../end sequence).
    pub ops: Vec<ClickOperation>,
    pub player_preview: PlayerPreview,
}

#[allow(clippy::too_many_arguments)]
pub fn build_inventory(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    inventory: &Inventory,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
) -> InventoryResult {
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        SpriteId::InventoryBackground,
    );
    panel.label(elements, 97.0, 6.0, "Crafting");

    let slots = inventory.slots();
    let mut ctx = SlotCtx::new(
        elements,
        &panel,
        cursor,
        ContainerKind::Player,
        slots,
        cursor_item,
        drag,
    );

    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE);

    let armor_ys = [8.0, 26.0, 44.0, 62.0];
    for i in 0..4u16 {
        let num = SLOT_ARMOR_BASE + i;
        ctx.slot(
            8.0,
            armor_ys[i as usize],
            inventory.slot(num as usize),
            Some(ARMOR_EMPTY_SPRITES[i as usize]),
            num,
        );
    }

    for row in 0..2u16 {
        for col in 0..2u16 {
            let num = SLOT_CRAFT_BASE + row * 2 + col;
            ctx.slot(
                98.0 + col as f32 * SLOT_STRIDE,
                18.0 + row as f32 * SLOT_STRIDE,
                inventory.slot(num as usize),
                None,
                num,
            );
        }
    }

    ctx.slot(
        154.0,
        28.0,
        inventory.craft_output(),
        None,
        SLOT_CRAFT_RESULT,
    );
    ctx.slot(
        77.0,
        62.0,
        inventory.offhand(),
        Some(SpriteId::EmptyShield),
        SLOT_OFFHAND,
    );

    let (hovered, shown_cursor) = ctx.finish(cursor_item);

    push_recipe_book_button(elements, &panel, cursor, 104.0, 61.0);
    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    let (ops, clicked_outside) = resolve_gesture(
        input,
        hovered,
        &panel,
        cursor,
        ContainerKind::Player,
        slots,
        cursor_item,
        drag,
        last_click,
    );

    InventoryResult {
        clicked_outside,
        ops,
        player_preview: PlayerPreview {
            rect: [
                panel.ox + 26.0 * panel.scale,
                panel.oy + 8.0 * panel.scale,
                49.0 * panel.scale,
                70.0 * panel.scale,
            ],
            gui_scale: panel.scale,
            cursor,
        },
    }
}
