//! The furnace family screen (vanilla `AbstractFurnaceScreen` +
//! `AbstractFurnaceMenu`): ingredient slot 0, fuel 1, result 2, player
//! inventory 3..29, hotbar 30..38. Covers the furnace, blast furnace, and
//! smoker, which differ only in textures.

use std::time::Instant;

use azalea_inventory::ItemStack;

use super::common::FONT_SIZE;
use super::container::{
    ContainerInput, ContainerResult, DragState, Panel, SlotCtx, push_clipped_sprite,
    push_cursor_stack, push_panel, push_recipe_book_button, resolve_gesture,
};
use crate::player::menu_click::ContainerKind;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const SLOT_INGREDIENT: u16 = 0;
const SLOT_FUEL: u16 = 1;
const SLOT_RESULT: u16 = 2;
const SLOT_MAIN_BASE: u16 = 3;
const SLOT_HOTBAR_BASE: u16 = 30;

/// Indices into the menu's data values (`ClientboundContainerSetData`),
/// matching vanilla `AbstractFurnaceBlockEntity`.
const DATA_LIT_TIME: usize = 0;
const DATA_LIT_DURATION: usize = 1;
const DATA_COOKING_PROGRESS: usize = 2;
const DATA_COOKING_TOTAL_TIME: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FurnaceVariant {
    Furnace,
    BlastFurnace,
    Smoker,
}

impl FurnaceVariant {
    /// This variant's (background, lit progress, burn progress) sprites.
    fn sprites(self) -> (SpriteId, SpriteId, SpriteId) {
        match self {
            Self::Furnace => (
                SpriteId::FurnaceBackground,
                SpriteId::FurnaceLitProgress,
                SpriteId::FurnaceBurnProgress,
            ),
            Self::BlastFurnace => (
                SpriteId::BlastFurnaceBackground,
                SpriteId::BlastFurnaceLitProgress,
                SpriteId::BlastFurnaceBurnProgress,
            ),
            Self::Smoker => (
                SpriteId::SmokerBackground,
                SpriteId::SmokerLitProgress,
                SpriteId::SmokerBurnProgress,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_furnace(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    variant: FurnaceVariant,
    slots: &[ItemStack],
    data: &[i16],
    title: &str,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> ContainerResult {
    let (background, lit_sprite, burn_sprite) = variant.sprites();
    let panel = push_panel(elements, screen_w, screen_h, gs, 166.0, background);
    // Vanilla centers the furnace title: (imageWidth - font.width(title)) / 2.
    let title_x = ((176.0 - text_width_fn(title, FONT_SIZE)) / 2.0).floor();
    panel.label(elements, title_x, 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    push_progress_overlays(elements, &panel, lit_sprite, burn_sprite, data);

    let mut ctx = SlotCtx::new(
        elements,
        &panel,
        cursor,
        ContainerKind::Furnace,
        slots,
        cursor_item,
        drag,
    );

    ctx.player_rows(slots, SLOT_MAIN_BASE, SLOT_HOTBAR_BASE, 84.0);

    let item = |num: u16| slots.get(num as usize).unwrap_or(&ItemStack::Empty);
    ctx.slot(56.0, 17.0, item(SLOT_INGREDIENT), None, SLOT_INGREDIENT);
    ctx.slot(56.0, 53.0, item(SLOT_FUEL), None, SLOT_FUEL);
    ctx.slot(116.0, 35.0, item(SLOT_RESULT), None, SLOT_RESULT);

    let (hovered, shown_cursor) = ctx.finish(cursor_item);

    push_recipe_book_button(elements, &panel, cursor, 20.0, 34.0);
    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    let (ops, clicked_outside) = resolve_gesture(
        input,
        hovered,
        &panel,
        cursor,
        ContainerKind::Furnace,
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

/// The flame (remaining fuel, bottom-anchored) and arrow (cook progress,
/// left-anchored), scissored to the filled fraction like vanilla's
/// `blitSprite` source crop.
fn push_progress_overlays(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    lit_sprite: SpriteId,
    burn_sprite: SpriteId,
    data: &[i16],
) {
    if data[DATA_LIT_TIME] > 0 {
        // ceil(progress * 13) + 1 rows of the 14px flame, from the bottom.
        let lit_duration = if data[DATA_LIT_DURATION] == 0 {
            200
        } else {
            data[DATA_LIT_DURATION]
        };
        let lit = (data[DATA_LIT_TIME] as f32 / lit_duration as f32).clamp(0.0, 1.0);
        let h = (lit * 13.0).ceil() + 1.0;
        push_clipped_sprite(
            elements,
            panel,
            lit_sprite,
            [56.0, 36.0, 14.0, 14.0],
            [56.0, 50.0 - h, 14.0, h],
        );
    }

    let cook = if data[DATA_COOKING_TOTAL_TIME] == 0 || data[DATA_COOKING_PROGRESS] == 0 {
        0.0
    } else {
        (data[DATA_COOKING_PROGRESS] as f32 / data[DATA_COOKING_TOTAL_TIME] as f32).clamp(0.0, 1.0)
    };
    // ceil(progress * 24) columns of the 24px arrow, from the left.
    let w = (cook * 24.0).ceil();
    if w > 0.0 {
        push_clipped_sprite(
            elements,
            panel,
            burn_sprite,
            [79.0, 34.0, 24.0, 16.0],
            [79.0, 34.0, w, 16.0],
        );
    }
}
