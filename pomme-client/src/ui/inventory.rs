use std::collections::HashMap;
use std::time::Instant;

use azalea_inventory::ItemStack;
use azalea_inventory::operations::{
    ClickOperation, PickupAllClick, PickupClick, QuickCraftClick, QuickCraftKind, QuickCraftStatus,
    QuickMoveClick,
};

use super::common::{
    FONT_SIZE, SLOT_LABEL_COLOR, SLOT_SIZE, SLOT_STRIDE, WHITE, hit_test, push_gradient_overlay,
    push_item_icon, push_slot,
};
use crate::player::inventory::{self, Inventory};
use crate::player::menu_click;
use crate::renderer::PlayerPreview;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const INV_TEX_W: f32 = 176.0;
const INV_TEX_H: f32 = 166.0;
const DOUBLE_CLICK_MS: u128 = 250;

// Vanilla player-menu slot indices, as u16 for click ops.
const SLOT_CRAFT_RESULT: u16 = inventory::CRAFT_OUTPUT as u16;
const SLOT_CRAFT_BASE: u16 = inventory::CRAFT_INPUT_START as u16;
const SLOT_ARMOR_BASE: u16 = inventory::ARMOR_START as u16;
const SLOT_MAIN_BASE: u16 = inventory::MAIN_START as u16;
const SLOT_HOTBAR_BASE: u16 = inventory::HOTBAR_START as u16;
const SLOT_OFFHAND: u16 = inventory::OFFHAND as u16;

/// Active click-drag: which button, and the slots covered so far.
pub type DragState = (QuickCraftKind, Vec<u16>);

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
    /// Container-click operations to send this frame (usually 0-1; a drag
    /// release emits a start/add.../end sequence).
    pub ops: Vec<ClickOperation>,
    pub player_preview: PlayerPreview,
}

/// Input for the survival inventory this frame.
pub struct InventoryInput {
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub left_held: bool,
    pub right_held: bool,
    pub shift: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn build_inventory(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &InventoryInput,
    inventory: &Inventory,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
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

    // Live drag preview: what each covered slot would receive and the remainder
    // left on the cursor. Read-only; the real change happens on release.
    let drag_preview: Option<(HashMap<u16, ItemStack>, ItemStack)> =
        drag.as_ref().map(|(kind, slots)| {
            let (changed, remainder) =
                menu_click::drag_distribution(inventory, cursor_item, kind, slots);
            (changed.into_iter().collect(), remainder)
        });

    let mut hovered = None;
    let mut slot = |elements: &mut Vec<MenuElement>, pos: SlotPos, item: &ItemStack, empty, num| {
        let shown = drag_preview
            .as_ref()
            .and_then(|(m, _)| m.get(&num))
            .unwrap_or(item);
        hovered = hovered.or(build_slot(
            elements, ox, oy, scale, &pos, cursor, shown, empty, num,
        ));
    };

    let hotbar = inventory.hotbar_slots();
    for col in 0..9usize {
        let pos = SlotPos {
            x: 8.0 + col as f32 * SLOT_STRIDE,
            y: 142.0,
        };
        let item = hotbar.get(col).unwrap_or(&ItemStack::Empty);
        slot(elements, pos, item, None, SLOT_HOTBAR_BASE + col as u16);
    }

    let main = inventory.main_slots();
    for row in 0..3usize {
        for col in 0..9usize {
            let idx = row * 9 + col;
            let pos = SlotPos {
                x: 8.0 + col as f32 * SLOT_STRIDE,
                y: 84.0 + row as f32 * SLOT_STRIDE,
            };
            let item = main.get(idx).unwrap_or(&ItemStack::Empty);
            slot(elements, pos, item, None, SLOT_MAIN_BASE + idx as u16);
        }
    }

    let armor = inventory.armor_slots();
    let armor_ys = [8.0, 26.0, 44.0, 62.0];
    for i in 0..4usize {
        let pos = SlotPos {
            x: 8.0,
            y: armor_ys[i],
        };
        let item = armor.get(i).unwrap_or(&ItemStack::Empty);
        slot(
            elements,
            pos,
            item,
            Some(ARMOR_EMPTY_SPRITES[i]),
            SLOT_ARMOR_BASE + i as u16,
        );
    }

    let craft_in = inventory.craft_input_slots();
    for row in 0..2usize {
        for col in 0..2usize {
            let idx = row * 2 + col;
            let pos = SlotPos {
                x: 98.0 + col as f32 * SLOT_STRIDE,
                y: 18.0 + row as f32 * SLOT_STRIDE,
            };
            let item = craft_in.get(idx).unwrap_or(&ItemStack::Empty);
            slot(elements, pos, item, None, SLOT_CRAFT_BASE + idx as u16);
        }
    }

    slot(
        elements,
        SlotPos { x: 154.0, y: 28.0 },
        inventory.craft_output(),
        None,
        SLOT_CRAFT_RESULT,
    );

    slot(
        elements,
        SlotPos { x: 77.0, y: 62.0 },
        inventory.offhand(),
        Some(SpriteId::EmptyShield),
        SLOT_OFFHAND,
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

    // The carried stack rides the cursor, on top of everything; while dragging
    // it shows the un-distributed remainder.
    let cursor_stack = drag_preview.as_ref().map(|(_, r)| r).unwrap_or(cursor_item);
    if let ItemStack::Present(data) = cursor_stack {
        let size = SLOT_SIZE * scale;
        push_item_icon(
            elements,
            cursor.0 - size / 2.0,
            cursor.1 - size / 2.0,
            size,
            scale,
            data,
        );
    }

    let carrying = cursor_item.is_present();
    let outside = !hit_test(cursor, [ox, oy, inv_w, inv_h]);
    let (ops, clicked_outside) = resolve_gesture(
        input,
        hovered,
        outside,
        carrying,
        inventory,
        cursor_item,
        drag,
        last_click,
    );

    InventoryResult {
        clicked_outside,
        ops,
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

/// Turns this frame's input + hover into container-click operations, driving
/// the drag state machine. The server applies and resyncs, so no local
/// prediction.
#[allow(clippy::too_many_arguments)]
fn resolve_gesture(
    input: &InventoryInput,
    hovered: Option<u16>,
    outside: bool,
    carrying: bool,
    inventory: &Inventory,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
) -> (Vec<ClickOperation>, bool) {
    let mut ops = Vec::new();

    if let Some((kind, slots)) = drag {
        let held = matches!(
            (&kind, input.left_held, input.right_held),
            (QuickCraftKind::Left, true, _) | (QuickCraftKind::Right, _, true)
        );
        if held {
            // Match vanilla's accumulation so our slot set (and split) equals the
            // server's: only eligible slots, and only while items remain to share.
            if let Some(slot) = hovered
                && !slots.contains(&slot)
                && (cursor_item.count() as usize) > slots.len()
                && menu_click::drag_slot_eligible(inventory, cursor_item, slot)
            {
                slots.push(slot);
            }
            return (ops, false);
        }
        // Released: distribute across 2+ slots; one covered slot converts to a
        // normal click (vanilla quickCraftToSlots), none falls back to a click
        // wherever the cursor is now (vanilla mouseReleased, -999 outside).
        let kind = kind.clone();
        let slots = std::mem::take(slots);
        *drag = None;
        if slots.len() >= 2 {
            ops.push(quick_craft(&kind, QuickCraftStatus::Start));
            for s in slots {
                ops.push(quick_craft(&kind, QuickCraftStatus::Add { slot: s }));
            }
            ops.push(quick_craft(&kind, QuickCraftStatus::End));
        } else if let Some(&s) = slots.first() {
            ops.push(pickup(&kind, Some(s)));
        } else if carrying {
            // Vanilla only falls back to a click while still carrying.
            if let Some(s) = hovered {
                ops.push(pickup(&kind, Some(s)));
            } else if outside {
                ops.push(pickup(&kind, None));
            }
        }
        return (ops, false);
    }

    if !(input.left_pressed || input.right_pressed) {
        return (ops, false);
    }
    let kind = if input.left_pressed {
        QuickCraftKind::Left
    } else {
        QuickCraftKind::Right
    };

    if outside {
        // Outside click: drop the cursor stack, else request a close.
        if carrying {
            ops.push(pickup(&kind, None));
            return (ops, false);
        }
        return (ops, input.left_pressed);
    }

    let Some(slot) = hovered else {
        // Panel background: no-op, but a carrying press still enters the drag
        // state machine like vanilla (with no slots covered yet).
        if carrying {
            *drag = Some((kind, Vec::new()));
        }
        return (ops, false);
    };

    // Timing-based like vanilla; the server only gathers if it has a cursor item
    // (avoids depending on the round-trip-lagged local carried state).
    let double = input.left_pressed
        && matches!(last_click, Some((s, t)) if *s == slot && t.elapsed().as_millis() <= DOUBLE_CLICK_MS);

    if input.shift {
        ops.push(ClickOperation::QuickMove(match kind {
            QuickCraftKind::Left => QuickMoveClick::Left { slot },
            _ => QuickMoveClick::Right { slot },
        }));
    } else if double {
        ops.push(ClickOperation::PickupAll(PickupAllClick {
            slot,
            reversed: false,
        }));
        *last_click = None;
    } else {
        if carrying {
            // Start a drag; only an eligible slot joins the covered set (vanilla
            // gates every quick-craft slot on mayPlace). A single-slot or empty
            // set resolves to a normal click on release.
            let slots = if menu_click::drag_slot_eligible(inventory, cursor_item, slot) {
                vec![slot]
            } else {
                Vec::new()
            };
            *drag = Some((kind, slots));
        } else {
            ops.push(pickup(&kind, Some(slot)));
        }
        if input.left_pressed {
            *last_click = Some((slot, Instant::now()));
        }
    }
    (ops, false)
}

fn pickup(kind: &QuickCraftKind, slot: Option<u16>) -> ClickOperation {
    ClickOperation::Pickup(match kind {
        QuickCraftKind::Left => PickupClick::Left { slot },
        _ => PickupClick::Right { slot },
    })
}

fn quick_craft(kind: &QuickCraftKind, status: QuickCraftStatus) -> ClickOperation {
    ClickOperation::QuickCraft(QuickCraftClick {
        kind: kind.clone(),
        status,
    })
}

/// Draws a slot; returns its number when hovered (regardless of click).
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
    slot_num: u16,
) -> Option<u16> {
    let x = ox + slot.x * scale;
    let y = oy + slot.y * scale;
    let size = SLOT_SIZE * scale;
    if push_slot(elements, x, y, size, scale, cursor, item, empty_sprite) {
        Some(slot_num)
    } else {
        None
    }
}
