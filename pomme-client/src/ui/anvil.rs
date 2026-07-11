//! The anvil screen (vanilla `AnvilScreen` + `AnvilMenu`): input slots 0-1,
//! result 2, player inventory 3..29, hotbar 30..38, plus the rename text
//! field. The repair cost arrives as menu data value 0.

use std::time::Instant;

use azalea_inventory::components::{CustomName, ItemName};
use azalea_inventory::{ItemStack, ItemStackData};

use super::common::{FONT_SIZE, WHITE};
use super::container::{
    ContainerInput, ContainerResult, DragState, Panel, SlotCtx, push_cursor_stack, push_panel,
    resolve_gesture,
};
use crate::player::menu_click::ContainerKind;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId};

const MAX_NAME_LENGTH: usize = 50;
const COST_GREEN: [f32; 4] = [0.502, 1.0, 0.125, 1.0];
const COST_RED: [f32; 4] = [1.0, 0.376, 0.376, 1.0];

/// The rename field's client state (vanilla `AnvilScreen.name` +
/// `AnvilMenu.itemName`), kept on the open container.
pub struct AnvilState {
    pub text: String,
    /// The last name accepted and sent to the server.
    sent: String,
    /// Input slot 0 as last seen, to detect changes (vanilla `slotChanged`
    /// resets the field text whenever the slot is set).
    last_input: ItemStack,
    blink: Instant,
}

impl AnvilState {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            sent: String::new(),
            last_input: ItemStack::Empty,
            blink: Instant::now(),
        }
    }
}

/// The item's hover name: custom name, else item-name component, else the
/// translated kind name.
fn hover_name(data: &ItemStackData) -> String {
    if let Some(c) = data.get_component::<CustomName>() {
        return c.name.to_string();
    }
    if let Some(c) = data.get_component::<ItemName>() {
        return c.name.to_string();
    }
    crate::lang::item_display_name(data.kind)
}

/// Applies this frame's typing to the rename field, mirroring vanilla
/// `AnvilScreen.slotChanged` + `onNameChanged`: reset the text when input
/// slot 0 changes, edit it while the slot is filled, normalize an unchanged
/// default name to "", and return the name to send when it differs from the
/// last one sent.
pub fn update_rename(
    state: &mut AnvilState,
    slots: &[ItemStack],
    typed: &[char],
    backspace: bool,
) -> Option<String> {
    let input = slots.first().cloned().unwrap_or(ItemStack::Empty);
    if input != state.last_input {
        state.text = input.as_present().map(hover_name).unwrap_or_default();
        state.last_input = input.clone();
        state.blink = Instant::now();
    }
    let ItemStack::Present(data) = &input else {
        return None;
    };

    if backspace && state.text.pop().is_some() {
        state.blink = Instant::now();
    }
    for &ch in typed {
        // Vanilla's allowed-chat-character filter plus the 50-char cap.
        if state.text.chars().count() < MAX_NAME_LENGTH && ch >= ' ' && ch != '\x7f' && ch != '§' {
            state.text.push(ch);
            state.blink = Instant::now();
        }
    }

    let mut name = state.text.clone();
    if data.get_component::<CustomName>().is_none() && name == hover_name(data) {
        name = String::new();
    }
    if name != state.sent {
        state.sent = name.clone();
        Some(name)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_anvil(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    cursor: (f32, f32),
    input: &ContainerInput,
    slots: &[ItemStack],
    data: &[i16],
    title: &str,
    state: &AnvilState,
    experience_level: i32,
    creative: bool,
    cursor_item: &ItemStack,
    drag: &mut Option<DragState>,
    last_click: &mut Option<(u16, Instant)>,
    gs: f32,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) -> ContainerResult {
    let panel = push_panel(
        elements,
        screen_w,
        screen_h,
        gs,
        166.0,
        SpriteId::AnvilBackground,
    );
    panel.label(elements, 60.0, 6.0, title);
    panel.label(elements, 8.0, 72.0, "Inventory");

    let item = |num: u16| slots.get(num as usize).unwrap_or(&ItemStack::Empty);
    let editable = item(0).is_present();

    let field_sprite = if editable {
        SpriteId::AnvilTextField
    } else {
        SpriteId::AnvilTextFieldDisabled
    };
    panel.image(elements, field_sprite, 59.0, 20.0, 110.0, 16.0);
    push_name_text(elements, &panel, state, editable, text_width_fn);

    if (item(0).is_present() || item(1).is_present()) && item(2).is_empty() {
        panel.image(elements, SpriteId::AnvilError, 99.0, 45.0, 28.0, 21.0);
    }

    push_cost_label(
        elements,
        &panel,
        data[0],
        item(2).is_present(),
        experience_level,
        creative,
        text_width_fn,
    );

    let mut ctx = SlotCtx::new(
        elements,
        &panel,
        cursor,
        ContainerKind::Anvil,
        slots,
        cursor_item,
        drag,
    );
    ctx.player_rows(slots, 3, 30, 84.0);
    ctx.slot(27.0, 47.0, item(0), None, 0);
    ctx.slot(76.0, 47.0, item(1), None, 1);
    ctx.slot(134.0, 47.0, item(2), None, 2);
    let (hovered, shown_cursor) = ctx.finish(cursor_item);

    push_cursor_stack(elements, cursor, panel.scale, &shown_cursor);

    let (ops, clicked_outside) = resolve_gesture(
        input,
        hovered,
        &panel,
        cursor,
        ContainerKind::Anvil,
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

/// The rename text at (62,24): white shadowed, right-shifted inside its
/// 103-wide clip when overflowing (vanilla's EditBox keeps the end-of-text
/// cursor in view), with a blinking append caret while editable.
fn push_name_text(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    state: &AnvilState,
    editable: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    let s = panel.scale;
    let fs = FONT_SIZE * s;
    let field_x = panel.ox + 62.0 * s;
    let field_w = 103.0 * s;
    let text_y = panel.oy + 24.0 * s;
    let tw = text_width_fn(&state.text, fs);
    let caret_w = text_width_fn("_", fs);
    let shift = (field_w - tw - caret_w).min(0.0);

    elements.push(MenuElement::ScissorPush {
        x: field_x,
        y: panel.oy + 20.0 * s,
        w: field_w,
        h: 16.0 * s,
    });
    elements.push(MenuElement::Text {
        x: field_x + shift,
        y: text_y,
        text: state.text.clone(),
        scale: fs,
        color: WHITE,
        centered: false,
    });
    if editable && state.blink.elapsed().as_millis() % 1000 < 500 {
        elements.push(MenuElement::Text {
            x: field_x + shift + tw,
            y: text_y,
            text: "_".into(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
    }
    elements.push(MenuElement::ScissorPop);
}

/// The right-aligned repair cost line (vanilla `AnvilScreen.extractLabels`):
/// hidden at cost 0 or with no result, "Too Expensive!" from cost 40 outside
/// creative, red when the player can't afford the levels.
fn push_cost_label(
    elements: &mut Vec<MenuElement>,
    panel: &Panel,
    cost: i16,
    has_result: bool,
    experience_level: i32,
    creative: bool,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
) {
    // Vanilla gates on cost > 0; a short-wrapped negative cost shows nothing.
    if cost <= 0 {
        return;
    }
    let (line, color) = if cost >= 40 && !creative {
        ("Too Expensive!".to_string(), COST_RED)
    } else if !has_result {
        return;
    } else if !creative && experience_level < cost as i32 {
        (format!("Enchantment Cost: {cost}"), COST_RED)
    } else {
        (format!("Enchantment Cost: {cost}"), COST_GREEN)
    };

    let s = panel.scale;
    let fs = FONT_SIZE * s;
    let tx = 166.0 * s - text_width_fn(&line, fs);
    elements.push(MenuElement::Rect {
        x: panel.ox + tx - 2.0 * s,
        y: panel.oy + 67.0 * s,
        w: 168.0 * s - tx + 2.0 * s,
        h: 12.0 * s,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 0.31],
    });
    elements.push(MenuElement::Text {
        x: panel.ox + tx,
        y: panel.oy + 69.0 * s,
        text: line,
        scale: fs,
        color,
        centered: false,
    });
}
