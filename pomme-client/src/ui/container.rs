//! Shared pieces for container screens (survival inventory, crafting table,
//! furnace): the click/drag gesture state machine and per-frame slot drawing.

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
use crate::player::menu_click::{self, ContainerKind};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const DOUBLE_CLICK_MS: u128 = 250;

/// Active click-drag: which button, and the slots covered so far.
pub type DragState = (QuickCraftKind, Vec<u16>);

/// What a container screen's frame produced.
pub struct ContainerResult {
    pub clicked_outside: bool,
    /// Container-click operations to send this frame (usually 0-1; a drag
    /// release emits a start/add.../end sequence).
    pub ops: Vec<ClickOperation>,
    /// Menu button clicked this frame (`ServerboundContainerButtonClick`),
    /// e.g. an enchantment option.
    pub button: Option<u32>,
}

/// Input for a container screen this frame.
pub struct ContainerInput {
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub middle_pressed: bool,
    pub left_held: bool,
    pub right_held: bool,
    pub shift: bool,
}

/// The centered container panel's placement on screen.
pub struct Panel {
    pub scale: f32,
    pub ox: f32,
    pub oy: f32,
    pub w: f32,
    pub h: f32,
}

impl Panel {
    pub fn contains(&self, cursor: (f32, f32)) -> bool {
        hit_test(cursor, [self.ox, self.oy, self.w, self.h])
    }

    /// A dark, unshadowed menu label at GUI-unit position.
    pub fn label(&self, elements: &mut Vec<MenuElement>, x: f32, y: f32, text: &str) {
        elements.push(MenuElement::TextFlat {
            x: self.ox + x * self.scale,
            y: self.oy + y * self.scale,
            text: text.into(),
            scale: FONT_SIZE * self.scale,
            color: SLOT_LABEL_COLOR,
        });
    }

    /// An untinted sprite at a GUI-unit rectangle.
    pub fn image(
        &self,
        elements: &mut Vec<MenuElement>,
        sprite: SpriteId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        elements.push(MenuElement::Image {
            x: self.ox + x * self.scale,
            y: self.oy + y * self.scale,
            w: w * self.scale,
            h: h * self.scale,
            sprite,
            tint: WHITE,
        });
    }
}

/// The dimmed backdrop and the centered panel placement for a `panel_w` x
/// `panel_h` (GUI units) container, without a background sprite.
pub fn push_backdrop(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    panel_w: f32,
    panel_h: f32,
) -> Panel {
    let scale = gs.min(screen_w / panel_w).min(screen_h / panel_h);
    let w = panel_w * scale;
    let h = panel_h * scale;
    let ox = (screen_w - w) / 2.0;
    let oy = (screen_h - h) / 2.0;

    push_gradient_overlay(
        elements,
        screen_w,
        screen_h,
        [0.0627, 0.0627, 0.0627, 0.7529],
        [0.0627, 0.0627, 0.0627, 0.8157],
    );

    Panel {
        scale,
        ox,
        oy,
        w,
        h,
    }
}

/// The dimmed backdrop and the centered 176 x `panel_h` container background
/// sprite.
pub fn push_panel(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    panel_h: f32,
    sprite: SpriteId,
) -> Panel {
    let panel = push_backdrop(elements, screen_w, screen_h, gs, 176.0, panel_h);
    panel.image(elements, sprite, 0.0, 0.0, 176.0, panel_h);
    panel
}

/// A sprite scissored to a sub-rectangle, both `[x, y, w, h]` in GUI units.
pub fn push_clipped_sprite(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    sprite: SpriteId,
    rect: [f32; 4],
    clip: [f32; 4],
) {
    let s = panel.scale;
    elements.push(MenuElement::ScissorPush {
        x: panel.ox + clip[0] * s,
        y: panel.oy + clip[1] * s,
        w: clip[2] * s,
        h: clip[3] * s,
    });
    panel.image(elements, sprite, rect[0], rect[1], rect[2], rect[3]);
    elements.push(MenuElement::ScissorPop);
}

/// The (inert) recipe book toggle at GUI-unit position.
pub fn push_recipe_book_button(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    cursor: (f32, f32),
    x: f32,
    y: f32,
) {
    let bx = panel.ox + x * panel.scale;
    let by = panel.oy + y * panel.scale;
    let w = 20.0 * panel.scale;
    let h = 18.0 * panel.scale;
    elements.push(MenuElement::Image {
        x: bx,
        y: by,
        w,
        h,
        sprite: if hit_test(cursor, [bx, by, w, h]) {
            SpriteId::RecipeBookButtonHighlighted
        } else {
            SpriteId::RecipeBookButton
        },
        tint: WHITE,
    });
}

/// Per-frame slot drawing context: positions slots in GUI units, substitutes
/// the live drag preview, and accumulates the hovered slot.
pub struct SlotCtx<'e> {
    elements: &'e mut Vec<MenuElement>,
    scale: f32,
    ox: f32,
    oy: f32,
    cursor: (f32, f32),
    /// What each drag-covered slot would receive and the remainder left on the
    /// cursor. Read-only; the real change happens on release.
    preview: Option<(HashMap<u16, ItemStack>, ItemStack)>,
    hovered: Option<u16>,
}

impl<'e> SlotCtx<'e> {
    pub fn new(
        elements: &'e mut Vec<MenuElement>,
        panel: &Panel,
        cursor: (f32, f32),
        kind: ContainerKind,
        slots: &[ItemStack],
        cursor_item: &ItemStack,
        drag: &Option<DragState>,
    ) -> Self {
        let preview = drag.as_ref().map(|(drag_kind, covered)| {
            let (changed, remainder) =
                menu_click::drag_distribution(kind, slots, cursor_item, drag_kind, covered);
            (changed.into_iter().collect(), remainder)
        });
        Self {
            elements,
            scale: panel.scale,
            ox: panel.ox,
            oy: panel.oy,
            cursor,
            preview,
            hovered: None,
        }
    }

    /// Draws a slot at GUI-unit position (x, y), recording it as hovered when
    /// the cursor is over it.
    pub fn slot(&mut self, x: f32, y: f32, item: &ItemStack, empty: Option<SpriteId>, num: u16) {
        let shown = self
            .preview
            .as_ref()
            .and_then(|(m, _)| m.get(&num))
            .unwrap_or(item);
        let px = self.ox + x * self.scale;
        let py = self.oy + y * self.scale;
        let size = SLOT_SIZE * self.scale;
        if push_slot(
            self.elements,
            px,
            py,
            size,
            self.scale,
            self.cursor,
            shown,
            empty,
        ) {
            self.hovered = self.hovered.or(Some(num));
        }
    }

    /// The three main-inventory rows starting at GUI-unit `main_y` and the
    /// hotbar 58 below, reading container slots starting at the given bases.
    pub fn player_rows(
        &mut self,
        slots: &[ItemStack],
        main_base: u16,
        hotbar_base: u16,
        main_y: f32,
    ) {
        for row in 0..3u16 {
            for col in 0..9u16 {
                let num = main_base + row * 9 + col;
                let item = slots.get(num as usize).unwrap_or(&ItemStack::Empty);
                self.slot(
                    8.0 + col as f32 * SLOT_STRIDE,
                    main_y + row as f32 * SLOT_STRIDE,
                    item,
                    None,
                    num,
                );
            }
        }
        for col in 0..9u16 {
            let num = hotbar_base + col;
            let item = slots.get(num as usize).unwrap_or(&ItemStack::Empty);
            self.slot(
                8.0 + col as f32 * SLOT_STRIDE,
                main_y + 58.0,
                item,
                None,
                num,
            );
        }
    }

    /// Ends slot drawing: the hovered slot, and the stack that should ride the
    /// cursor (the un-distributed drag remainder while dragging).
    pub fn finish(self, cursor_item: &ItemStack) -> (Option<u16>, ItemStack) {
        let shown = self
            .preview
            .map(|(_, r)| r)
            .unwrap_or_else(|| cursor_item.clone());
        (self.hovered, shown)
    }
}

/// The carried stack rides the cursor, on top of everything.
pub fn push_cursor_stack(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    scale: f32,
    item: &ItemStack,
) {
    if let ItemStack::Present(data) = item {
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
}

/// Turns this frame's input + hover into container-click operations, driving
/// the drag state machine. The server applies and resyncs, so no local
/// prediction. Returns the ops and whether an empty-handed click landed
/// outside the panel (a close request).
#[allow(clippy::too_many_arguments)]
pub fn resolve_gesture(
    input: &ContainerInput,
    hovered: Option<u16>,
    panel: &Panel,
    cursor: (f32, f32),
    kind: ContainerKind,
    slots: &[ItemStack],
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
) -> (Vec<ClickOperation>, bool) {
    let carrying = cursor_item.is_present();
    let outside = !panel.contains(cursor);
    let mut ops = Vec::new();

    if let Some((drag_kind, covered)) = drag {
        let held = matches!(
            (&drag_kind, input.left_held, input.right_held),
            (QuickCraftKind::Left, true, _) | (QuickCraftKind::Right, _, true)
        );
        if held {
            // Match vanilla's accumulation so our slot set (and split) equals the
            // server's: only eligible slots, and only while items remain to share.
            if let Some(slot) = hovered
                && !covered.contains(&slot)
                && (cursor_item.count() as usize) > covered.len()
                && menu_click::drag_slot_eligible(kind, slots, cursor_item, slot)
            {
                covered.push(slot);
            }
            return (ops, false);
        }
        // Released: distribute across 2+ slots; one covered slot converts to a
        // normal click (vanilla quickCraftToSlots), none falls back to a click
        // wherever the cursor is now (vanilla mouseReleased, -999 outside).
        let drag_kind = drag_kind.clone();
        let covered = std::mem::take(covered);
        *drag = None;
        if covered.len() >= 2 {
            ops.push(quick_craft(&drag_kind, QuickCraftStatus::Start));
            for s in covered {
                ops.push(quick_craft(&drag_kind, QuickCraftStatus::Add { slot: s }));
            }
            ops.push(quick_craft(&drag_kind, QuickCraftStatus::End));
        } else if let Some(&s) = covered.first() {
            ops.push(pickup(&drag_kind, Some(s)));
        } else if carrying {
            // Vanilla only falls back to a click while still carrying.
            if let Some(s) = hovered {
                ops.push(pickup(&drag_kind, Some(s)));
            } else if outside {
                ops.push(pickup(&drag_kind, None));
            }
        }
        return (ops, false);
    }

    if !(input.left_pressed || input.right_pressed) {
        return (ops, false);
    }
    let click_kind = if input.left_pressed {
        QuickCraftKind::Left
    } else {
        QuickCraftKind::Right
    };

    if outside {
        // Outside click: drop the cursor stack, else request a close.
        if carrying {
            ops.push(pickup(&click_kind, None));
            return (ops, false);
        }
        return (ops, input.left_pressed);
    }

    let Some(slot) = hovered else {
        // Panel background: no-op, but a carrying press still enters the drag
        // state machine like vanilla (with no slots covered yet).
        if carrying {
            *drag = Some((click_kind, Vec::new()));
        }
        return (ops, false);
    };

    // Timing-based like vanilla; the server only gathers if it has a cursor item
    // (avoids depending on the round-trip-lagged local carried state).
    let double = input.left_pressed
        && matches!(last_click, Some((s, t)) if *s == slot && t.elapsed().as_millis() <= DOUBLE_CLICK_MS);

    if input.shift {
        ops.push(ClickOperation::QuickMove(match click_kind {
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
            let covered = if menu_click::drag_slot_eligible(kind, slots, cursor_item, slot) {
                vec![slot]
            } else {
                Vec::new()
            };
            *drag = Some((click_kind, covered));
        } else {
            ops.push(pickup(&click_kind, Some(slot)));
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
