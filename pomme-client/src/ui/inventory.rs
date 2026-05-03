use azalea_inventory::ItemStack;

use super::common::{self, WHITE, push_item_count};
use crate::player::inventory::{Inventory, item_resource_name};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const INV_TEX_W: f32 = 176.0;
const INV_TEX_H: f32 = 166.0;
const SLOT_STRIDE: f32 = 18.0;
const SLOT_SIZE: f32 = 16.0;
const LABEL_COLOR: [f32; 4] = [0.25, 0.25, 0.25, 1.0];
const HIGHLIGHT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.5];

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

pub fn build_inventory(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    clicked: bool,
    inventory: &Inventory,
    gs: f32,
) -> bool {
    let scale = gs.min(screen_w / INV_TEX_W).min(screen_h / INV_TEX_H);
    let inv_w = INV_TEX_W * scale;
    let inv_h = INV_TEX_H * scale;
    let ox = (screen_w - inv_w) / 2.0;
    let oy = (screen_h - inv_h) / 2.0;

    common::push_overlay(elements, screen_w, screen_h, 0.5);

    elements.push(MenuElement::Image {
        x: ox,
        y: oy,
        w: inv_w,
        h: inv_h,
        sprite: SpriteId::InventoryBackground,
        tint: WHITE,
    });

    let fs = 6.0 * scale;

    elements.push(MenuElement::Text {
        x: ox + 97.0 * scale,
        y: oy + 6.0 * scale,
        text: "Crafting".into(),
        scale: fs,
        color: LABEL_COLOR,
        centered: false,
    });
    elements.push(MenuElement::Text {
        x: ox + 8.0 * scale,
        y: oy + 72.0 * scale,
        text: "Inventory".into(),
        scale: fs,
        color: LABEL_COLOR,
        centered: false,
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

    let outside = cursor.0 < ox || cursor.0 > ox + inv_w || cursor.1 < oy || cursor.1 > oy + inv_h;
    clicked && outside
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

    let hovered = cursor.0 >= x && cursor.0 < x + size && cursor.1 >= y && cursor.1 < y + size;

    if hovered {
        elements.push(MenuElement::Rect {
            x,
            y,
            w: size,
            h: size,
            corner_radius: 0.0,
            color: HIGHLIGHT_COLOR,
        });
    }

    match item {
        ItemStack::Empty => {
            if let Some(sprite) = empty_sprite {
                elements.push(MenuElement::Image {
                    x,
                    y,
                    w: size,
                    h: size,
                    sprite,
                    tint: WHITE,
                });
            }
        }
        ItemStack::Present(data) => {
            let name = item_resource_name(data.kind);
            elements.push(MenuElement::ItemIcon {
                x,
                y,
                w: size,
                h: size,
                item_name: name,
                tint: WHITE,
            });

            if data.count > 1 {
                push_item_count(elements, x, y, size, scale, data.count);
            }
        }
    }
}
