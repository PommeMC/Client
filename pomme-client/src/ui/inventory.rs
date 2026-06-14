use azalea_inventory::ItemStack;

use super::common::{
    FONT_SIZE, SLOT_LABEL_COLOR, SLOT_SIZE, SLOT_STRIDE, WHITE, hit_test, push_gradient_overlay,
    push_slot,
};
use crate::player::inventory::Inventory;
use crate::renderer::PlayerPreview;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const INV_TEX_W: f32 = 176.0;
const INV_TEX_H: f32 = 166.0;

struct SlotPos {
    x: f32,
    y: f32,
}

const ARMOR_EMPTY_SPRITES: [SpriteId; 4] = [
    SpriteId::EmptyHelmet,
    SpriteId::EmptyChestplate,
    SpriteId::EmptyLeggings,
    SpriteId::EmptyBoots,
];

pub struct InventoryResult {
    pub clicked_outside: bool,
    pub player_preview: PlayerPreview,
}

pub fn build_inventory(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    inventory: &Inventory,
    gs: f32,
) -> InventoryResult {
    let scale = gs.min(screen_w / INV_TEX_W).min(screen_h / INV_TEX_H);
    let inv_w = INV_TEX_W * scale;
    let inv_h = INV_TEX_H * scale;
    let ox = (screen_w - inv_w) / 2.0;
    let oy = (screen_h - inv_h) / 2.0;

    push_gradient_overlay(
        elements,
        screen_w,
        screen_h,
        [0.0627, 0.0627, 0.0627, 0.7529],
        [0.0627, 0.0627, 0.0627, 0.8157],
    );

    elements.push(MenuElement::Image {
        x: ox,
        y: oy,
        w: inv_w,
        h: inv_h,
        sprite: SpriteId::InventoryBackground,
        tint: WHITE,
    });

    let fs = FONT_SIZE * scale;

    elements.push(MenuElement::TextFlat {
        x: ox + 97.0 * scale,
        y: oy + 6.0 * scale,
        text: "Crafting".into(),
        scale: fs,
        color: SLOT_LABEL_COLOR,
    });

    let hotbar = inventory.hotbar_slots();
    for col in 0..9usize {
        let slot = SlotPos {
            x: 8.0 + col as f32 * SLOT_STRIDE,
            y: 142.0,
        };
        build_slot(
            elements,
            ox,
            oy,
            scale,
            &slot,
            cursor,
            hotbar.get(col).unwrap_or(&ItemStack::Empty),
            None,
        );
    }

    let main = inventory.main_slots();
    for row in 0..3usize {
        for col in 0..9usize {
            let idx = row * 9 + col;
            let slot = SlotPos {
                x: 8.0 + col as f32 * SLOT_STRIDE,
                y: 84.0 + row as f32 * SLOT_STRIDE,
            };
            build_slot(
                elements,
                ox,
                oy,
                scale,
                &slot,
                cursor,
                main.get(idx).unwrap_or(&ItemStack::Empty),
                None,
            );
        }
    }

    let armor = inventory.armor_slots();
    let armor_ys = [8.0, 26.0, 44.0, 62.0];
    for i in 0..4usize {
        let slot = SlotPos {
            x: 8.0,
            y: armor_ys[i],
        };
        build_slot(
            elements,
            ox,
            oy,
            scale,
            &slot,
            cursor,
            armor.get(i).unwrap_or(&ItemStack::Empty),
            Some(ARMOR_EMPTY_SPRITES[i]),
        );
    }

    let craft_in = inventory.craft_input_slots();
    for row in 0..2usize {
        for col in 0..2usize {
            let idx = row * 2 + col;
            let slot = SlotPos {
                x: 98.0 + col as f32 * SLOT_STRIDE,
                y: 18.0 + row as f32 * SLOT_STRIDE,
            };
            build_slot(
                elements,
                ox,
                oy,
                scale,
                &slot,
                cursor,
                craft_in.get(idx).unwrap_or(&ItemStack::Empty),
                None,
            );
        }
    }

    let craft_out_slot = SlotPos { x: 154.0, y: 28.0 };
    build_slot(
        elements,
        ox,
        oy,
        scale,
        &craft_out_slot,
        cursor,
        inventory.craft_output(),
        None,
    );

    let offhand_slot = SlotPos { x: 77.0, y: 62.0 };
    build_slot(
        elements,
        ox,
        oy,
        scale,
        &offhand_slot,
        cursor,
        inventory.offhand(),
        Some(SpriteId::EmptyShield),
    );

    let book_x = ox + 104.0 * scale;
    let book_y = oy + 61.0 * scale;
    let book_hovered = hit_test(cursor, [book_x, book_y, 20.0 * scale, 18.0 * scale]);
    elements.push(MenuElement::Image {
        x: book_x,
        y: book_y,
        w: 20.0 * scale,
        h: 18.0 * scale,
        sprite: if book_hovered {
            SpriteId::RecipeBookButtonHighlighted
        } else {
            SpriteId::RecipeBookButton
        },
        tint: WHITE,
    });

    let outside = cursor.0 < ox || cursor.0 > ox + inv_w || cursor.1 < oy || cursor.1 > oy + inv_h;
    InventoryResult {
        clicked_outside: clicked && outside,
        player_preview: PlayerPreview {
            rect: [
                ox + 26.0 * scale,
                oy + 8.0 * scale,
                49.0 * scale,
                70.0 * scale,
            ],
            gui_scale: scale,
            cursor,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_slot(
    elements: &mut Vec<MenuElement>,
    ox: f32,
    oy: f32,
    scale: f32,
    slot: &SlotPos,
    cursor: (f32, f32),
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
) {
    let x = ox + slot.x * scale;
    let y = oy + slot.y * scale;
    let size = SLOT_SIZE * scale;
    push_slot(elements, x, y, size, scale, cursor, item, empty_sprite);
}
