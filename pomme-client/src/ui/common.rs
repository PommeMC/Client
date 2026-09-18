use azalea_inventory::components::{CustomName, Damage, ItemName, MaxDamage, Unbreakable};
use azalea_inventory::{ItemStack, ItemStackData};

use crate::benchmark::UploadStatus;
use crate::player::inventory::item_resource_name;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::text_edit::TextFieldRenderInfo;

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const FONT_SIZE: f32 = 8.0;
pub const BTN_H: f32 = 20.0;
/// Vanilla's inactive-widget text colour, `0xA0A0A0`
/// (`AbstractWidget.WithInactiveMessage.defaultInactiveMessage`).
pub const COL_DISABLED: [f32; 4] = [0.627, 0.627, 0.627, 1.0];
pub const SLOT_SIZE: f32 = 16.0;
pub const SLOT_STRIDE: f32 = 18.0;
pub const SLOT_LABEL_COLOR: [f32; 4] = [0.25, 0.25, 0.25, 1.0];
const BTN_BORDER: f32 = 3.0;

pub type SpansWidthFn<'a> = &'a dyn Fn(&[crate::ui::text::TextSpan], f32) -> f32;

/// The hover-name component: custom name, else item-name component.
fn item_hover_component(data: &ItemStackData) -> Option<azalea_chat::FormattedText> {
    if let Some(name) = data.get_component::<CustomName>() {
        return Some(name.name.clone());
    }
    data.get_component::<ItemName>()
        .map(|name| name.name.clone())
}

pub fn item_display_name(data: &ItemStackData) -> String {
    item_hover_component(data)
        .map(|name| name.to_string())
        .unwrap_or_else(|| crate::lang::item_display_name(data.kind))
}

/// The hover name as styled spans; `base_color` fills wherever the name
/// component carries no explicit color (vanilla's parent style).
pub fn item_display_spans(
    data: &ItemStackData,
    base_color: [f32; 4],
) -> Vec<crate::ui::text::TextSpan> {
    item_hover_component(data)
        .map(|name| crate::ui::text::format_text_spans(&name, base_color))
        .unwrap_or_else(|| {
            vec![crate::ui::text::TextSpan::new(
                crate::lang::item_display_name(data.kind),
                base_color,
            )]
        })
}

pub const fn rgb(hex: u32) -> [f32; 4] {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    ]
}

pub fn push_tooltip(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    text: &str,
) {
    elements.push(MenuElement::Tooltip {
        x: cursor.0,
        y: cursor.1,
        text: text.into(),
        scale: FONT_SIZE * gs,
        screen_w,
        screen_h,
    });
}

pub fn push_tooltip_lines(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    lines: Vec<TooltipLine>,
) {
    elements.push(MenuElement::TooltipLines {
        x: cursor.0,
        y: cursor.1,
        lines,
        scale: FONT_SIZE * gs,
        screen_w,
        screen_h,
    });
}

pub fn push_overlay(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32, alpha: f32) {
    elements.push(MenuElement::Rect {
        x: 0.0,
        y: 0.0,
        w: screen_w,
        h: screen_h,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, alpha],
    });
}

pub fn push_gradient_overlay(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    color_top: [f32; 4],
    color_bottom: [f32; 4],
) {
    elements.push(MenuElement::GradientRect {
        x: 0.0,
        y: 0.0,
        w: screen_w,
        h: screen_h,
        corner_radius: 0.0,
        color_top,
        color_bottom,
    });
}

/// What the caller should do after a result overlay handled this frame's input.
pub enum ResultAction {
    None,
    Dismiss,
    StartUpload,
    /// Re-copy the already-uploaded link to the clipboard.
    Recopy,
}

/// A centered results panel: dimmed backdrop, a large title, a column of detail
/// lines, an upload status line, and an "upload & copy link" button. Shared by
/// the benchmark result overlays. Returns what the caller should do based on
/// the click / escape this frame.
#[allow(clippy::too_many_arguments)]
pub fn push_results_overlay(
    elements: &mut Vec<MenuElement>,
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    title_y: f32,
    title: &str,
    lines: &[String],
    upload: Option<&UploadStatus>,
    cursor: (f32, f32),
    clicked: bool,
    escape: bool,
) -> ResultAction {
    let fs = FONT_SIZE * gs;
    let cx = screen_w / 2.0;
    push_overlay(elements, screen_w, screen_h, 0.5);
    elements.push(MenuElement::Text {
        x: cx,
        y: title_y,
        text: title.into(),
        scale: fs * 2.0,
        color: WHITE,
        centered: true,
    });
    for (i, line) in lines.iter().enumerate() {
        elements.push(MenuElement::Text {
            x: cx,
            y: title_y + fs * 2.0 + 10.0 + i as f32 * (fs + 4.0),
            text: line.clone(),
            scale: fs,
            color: [0.8, 0.85, 0.9, 1.0],
            centered: true,
        });
    }
    let lines_bottom = title_y + fs * 2.0 + 10.0 + lines.len() as f32 * (fs + 4.0);

    let status = match upload {
        None => None,
        Some(UploadStatus::Uploading) => Some(("Uploading...".to_string(), [0.8, 0.85, 0.9, 1.0])),
        Some(UploadStatus::Done { url, copied }) => Some(if *copied {
            (format!("Link copied: {url}"), [0.6, 0.9, 0.6, 1.0])
        } else {
            (
                format!("Uploaded (copy failed): {url}"),
                [0.9, 0.85, 0.5, 1.0],
            )
        }),
        Some(UploadStatus::Failed(e)) => Some((e.clone(), [0.95, 0.5, 0.5, 1.0])),
    };
    if let Some((text, color)) = status {
        elements.push(MenuElement::Text {
            x: cx,
            y: lines_bottom + 6.0,
            text,
            scale: fs,
            color,
            centered: true,
        });
    }

    // Debug builds produce unrepresentative timings, so don't let them be shared.
    let debug_build = crate::benchmark::is_debug_build();
    let (label, enabled) = if debug_build {
        ("Upload disabled (debug build)", false)
    } else {
        match upload {
            Some(UploadStatus::Uploading) => ("Uploading...", false),
            Some(UploadStatus::Done { .. }) => ("Copy link again", true),
            _ => ("Upload & copy link", true),
        }
    };
    let btn_w = 180.0 * gs;
    let btn_h = BTN_H * gs;
    let btn_x = cx - btn_w / 2.0;
    let btn_y = lines_bottom + fs + 12.0;
    push_button(
        elements, cursor, btn_x, btn_y, btn_w, btn_h, gs, fs, label, enabled,
    );

    if clicked && hit_test(cursor, [btn_x, btn_y, btn_w, btn_h]) {
        if debug_build {
            return ResultAction::None;
        }
        return match upload {
            Some(UploadStatus::Uploading) => ResultAction::None,
            Some(UploadStatus::Done { .. }) => ResultAction::Recopy,
            _ => ResultAction::StartUpload,
        };
    }
    if escape || clicked {
        return ResultAction::Dismiss;
    }
    ResultAction::None
}

/// Copy `text` to the system clipboard, returning whether it succeeded.
pub(crate) fn set_clipboard(text: &str) -> bool {
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("Failed to set clipboard: {err}");
            false
        }
    }
}

/// Vanilla-EditBox selection highlight: a solid blue block behind the (white)
/// text (`EditBox.extractWidgetRenderState` -> `graphics.textHighlight`).
pub(crate) const FIELD_SELECTION: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Selection highlight, display text, and caret for one text-field frame
/// (`EditBox.renderWidget`): blue block behind the shown slice, then a
/// `bar_w`-wide caret bar in insert mode or a trailing `_` glyph otherwise.
/// `pad_y` is the scaled-pixel unit for the vanilla overdraw: selection/caret
/// rects span `textY-1 .. textY+lineHeight+1` (one above, two below the 8px
/// glyph line). `ghost` is chat's inline suggestion suffix, drawn one `bar_w`
/// left of the caret (vanilla draws it at `cursorX - 1`, under the caret).
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_field_text(
    elements: &mut Vec<MenuElement>,
    info: &TextFieldRenderInfo,
    shown: &str,
    text_x: f32,
    text_y: f32,
    fs: f32,
    bar_w: f32,
    pad_y: f32,
    caret_color: [f32; 4],
    ghost: Option<(&str, [f32; 4])>,
    wf: &dyn Fn(&str) -> f32,
) {
    if let Some((a, b)) = info.selection {
        let x0 = text_x + wf(&shown[..a]);
        let x1 = text_x + wf(&shown[..b]);
        elements.push(MenuElement::Rect {
            x: x0,
            y: text_y - pad_y,
            w: x1 - x0,
            h: fs + 3.0 * pad_y,
            corner_radius: 0.0,
            color: FIELD_SELECTION,
        });
    }
    elements.push(MenuElement::Text {
        x: text_x,
        y: text_y,
        text: shown.into(),
        scale: fs,
        color: WHITE,
        centered: false,
    });
    let caret_x = text_x + wf(&shown[..info.caret_byte]);
    if let Some((text, color)) = ghost {
        elements.push(MenuElement::Text {
            x: caret_x - bar_w,
            y: text_y,
            text: text.into(),
            scale: fs,
            color,
            centered: false,
        });
    }
    if info.caret_visible {
        if info.insert_mode {
            elements.push(MenuElement::Rect {
                x: caret_x,
                y: text_y - pad_y,
                w: bar_w,
                h: fs + 3.0 * pad_y,
                corner_radius: 0.0,
                color: caret_color,
            });
        } else {
            // Vanilla appends the `_` one pixel after the text (`drawX += 1`).
            elements.push(MenuElement::Text {
                x: caret_x + bar_w,
                y: text_y,
                text: "_".into(),
                scale: fs,
                color: caret_color,
                centered: false,
            });
        }
    }
}

const DIGIT_WIDTH: f32 = 6.0;

pub fn push_item_count(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    gs: f32,
    count: i32,
) {
    let text = count.to_string();
    let char_w = DIGIT_WIDTH * gs;
    let text_w = text.len() as f32 * char_w;
    let fs = FONT_SIZE * gs;
    elements.push(MenuElement::Text {
        x: x + size + gs - text_w,
        y: y + size - fs + gs,
        text,
        scale: fs,
        color: WHITE,
        centered: false,
    });
}

pub fn hit_test(cursor: (f32, f32), rect: [f32; 4]) -> bool {
    cursor.0 >= rect[0]
        && cursor.0 < rect[0] + rect[2]
        && cursor.1 >= rect[1]
        && cursor.1 < rect[1] + rect[3]
}

#[allow(clippy::too_many_arguments)]
pub fn push_slot(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    cursor: (f32, f32),
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
) -> bool {
    let hovered = hit_test(cursor, [x, y, size, size]);
    let highlight = |sprite| MenuElement::Image {
        x: x - 4.0 * scale,
        y: y - 4.0 * scale,
        w: 24.0 * scale,
        h: 24.0 * scale,
        sprite,
        tint: WHITE,
    };
    if hovered {
        elements.push(highlight(SpriteId::SlotHighlightBack));
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
        ItemStack::Present(data) => push_item_icon(elements, x, y, size, scale, data),
    }
    if hovered {
        elements.push(highlight(SpriteId::SlotHighlightFront));
    }
    hovered
}

/// Draws an item icon and the vanilla item decorations Pomme supports.
pub fn push_item_icon(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    data: &ItemStackData,
) {
    elements.push(MenuElement::ItemIcon {
        x,
        y,
        w: size,
        h: size,
        item_name: item_resource_name(data.kind),
        tint: WHITE,
    });
    push_item_durability(elements, x, y, scale, data);
    if data.count > 1 {
        push_item_count(elements, x, y, size, scale, data.count);
    }
}

fn push_item_durability(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    scale: f32,
    data: &ItemStackData,
) {
    let Some((width, color)) = durability_bar(data) else {
        return;
    };

    let bar_x = x + 2.0 * scale;
    let bar_y = y + 13.0 * scale;
    elements.push(MenuElement::Rect {
        x: bar_x,
        y: bar_y,
        w: 13.0 * scale,
        h: 2.0 * scale,
        corner_radius: 0.0,
        color: [0.0, 0.0, 0.0, 1.0],
    });
    elements.push(MenuElement::Rect {
        x: bar_x,
        y: bar_y,
        w: width as f32 * scale,
        h: scale,
        corner_radius: 0.0,
        color,
    });
}

/// Vanilla `Item` durability bar metrics for a damageable stack.
fn durability_bar(data: &ItemStackData) -> Option<(i32, [f32; 4])> {
    if data.get_component::<Unbreakable>().is_some() {
        return None;
    }
    let max_damage = data.get_component::<MaxDamage>()?.amount;
    let damage = data.get_component::<Damage>()?.amount;
    if max_damage <= 0 {
        return None;
    }

    // ItemStack#getDamageValue clamps the component before Item's bar math.
    let damage = damage.clamp(0, max_damage);
    if damage == 0 {
        return None;
    }

    let width = (13.0 - damage as f32 * 13.0 / max_damage as f32)
        .round()
        .clamp(0.0, 13.0) as i32;
    let health = ((max_damage as f32 - damage as f32) / max_damage as f32).max(0.0);
    Some((width, durability_color(health)))
}

fn durability_color(health: f32) -> [f32; 4] {
    // Item#getBarColor calls Mth.hsvToRgb(health / 3, 1, 1). Keep vanilla's
    // float operation order and channel truncation so boundary colours match.
    let hue = health / 3.0;
    let scaled_hue = hue * 6.0;
    let sector = scaled_hue as i32 % 6;
    let fraction = scaled_hue - sector as f32;
    let p = 0.0;
    let q = 1.0 - fraction;
    let t = fraction;
    let (red, green, blue) = match sector {
        0 => (1.0, t, p),
        1 => (q, 1.0, p),
        2 => (p, 1.0, t),
        3 => (p, q, 1.0),
        4 => (t, p, 1.0),
        5 => (1.0, p, q),
        _ => unreachable!("durability hue must remain in the vanilla HSV range"),
    };
    let channel = |value: f32| ((value * 255.0) as i32).clamp(0, 255) as f32 / 255.0;
    [channel(red), channel(green), channel(blue), 1.0]
}

/// Measures rendered text width in framebuffer px at the given font size.
pub type TextWidthFn<'a> = &'a dyn Fn(&str, f32) -> f32;

/// Inputs for the vanilla scrolling-label treatment: labels wider than the
/// widget are clipped to it and slide back and forth over time.
pub struct LabelScroll<'a> {
    pub text_width_fn: TextWidthFn<'a>,
    pub time_secs: f64,
}

/// Widget label: centered when it fits, otherwise (with `scroll`) clipped and
/// oscillated like vanilla's ActiveTextCollector.defaultScrollingHelper.
#[allow(clippy::too_many_arguments)]
fn push_widget_label(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    color: [f32; 4],
    scroll: Option<&LabelScroll<'_>>,
) {
    let ty = y + (h - fs) / 2.0 + 1.0;
    if let Some(s) = scroll {
        let margin = 2.0 * gs;
        let avail = w - 2.0 * margin;
        let lw = (s.text_width_fn)(label, fs);
        if lw > avail {
            let max_pos = lw - avail;
            let period = ((max_pos / gs) as f64 * 0.5).max(3.0);
            let alpha = (std::f64::consts::FRAC_PI_2
                * (std::f64::consts::TAU * s.time_secs / period).cos())
            .sin()
                / 2.0
                + 0.5;
            let pos = alpha as f32 * max_pos;
            elements.push(MenuElement::ScissorPush {
                x: x + margin,
                y,
                w: avail,
                h,
            });
            elements.push(MenuElement::Text {
                x: x + margin - pos,
                y: ty,
                text: label.into(),
                scale: fs,
                color,
                centered: false,
            });
            elements.push(MenuElement::ScissorPop);
            return;
        }
    }
    elements.push(MenuElement::Text {
        x: x + w / 2.0,
        y: ty,
        text: label.into(),
        scale: fs,
        color,
        centered: true,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn push_button(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
) -> bool {
    push_button_inner(elements, cursor, x, y, w, h, gs, fs, label, enabled, None)
}

#[allow(clippy::too_many_arguments)]
pub fn push_button_scrolling(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
    scroll: &LabelScroll<'_>,
) -> bool {
    push_button_inner(
        elements,
        cursor,
        x,
        y,
        w,
        h,
        gs,
        fs,
        label,
        enabled,
        Some(scroll),
    )
}

#[allow(clippy::too_many_arguments)]
fn push_button_inner(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    enabled: bool,
    scroll: Option<&LabelScroll<'_>>,
) -> bool {
    let hovered = enabled && hit_test(cursor, [x, y, w, h]);

    let (sprite, text_col) = if !enabled {
        (SpriteId::ButtonDisabled, COL_DISABLED)
    } else if hovered {
        (SpriteId::ButtonHover, WHITE)
    } else {
        (SpriteId::ButtonNormal, WHITE)
    };

    let border = if enabled { BTN_BORDER } else { 1.0 };
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite,
        border: border * gs,
        tint: WHITE,
    });

    push_widget_label(elements, x, y, w, h, gs, fs, label, text_col, scroll);

    hovered
}

#[allow(clippy::too_many_arguments)]
pub fn push_slider(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    mouse_pressed: bool,
    mouse_held: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: &str,
    value: f32,
    enabled: bool,
    focused: bool,
    can_change_value: bool,
    dragging: bool,
    scroll: &LabelScroll<'_>,
) -> SliderResult {
    let hovered = enabled && hit_test(cursor, [x, y, w, h]);
    let handle_w = 8.0 * gs;
    let track_w = w - handle_w;
    let handle_x = x + value.clamp(0.0, 1.0) * track_w;

    let actively_dragging = enabled && dragging && mouse_held;
    let start_drag = hovered && mouse_pressed && !dragging;

    let new_value = if actively_dragging || start_drag {
        let rel = (cursor.0 - x - handle_w / 2.0) / track_w;
        Some(rel.clamp(0.0, 1.0))
    } else {
        None
    };

    // `getSprite` / `getHandleSprite`: a focused slider highlights its handle
    // while Left/Right can move it, and its track once Enter has locked it.
    let track_sprite = if enabled && focused && !can_change_value {
        SpriteId::SliderTrackHover
    } else {
        SpriteId::SliderTrack
    };
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: track_sprite,
        // `widget/slider.png.mcmeta` declares a 1px border, not the button's 3.
        border: 1.0 * gs,
        tint: WHITE,
    });

    let handle_sprite = if enabled
        && (hovered || actively_dragging || start_drag || (focused && can_change_value))
    {
        SpriteId::SliderHandleHover
    } else {
        SpriteId::SliderHandle
    };
    elements.push(MenuElement::Image {
        x: handle_x,
        y,
        w: handle_w,
        h,
        sprite: handle_sprite,
        tint: WHITE,
    });

    let text_col = if enabled { WHITE } else { COL_DISABLED };
    push_widget_label(elements, x, y, w, h, gs, fs, label, text_col, Some(scroll));

    SliderResult {
        hovered,
        dragging: actively_dragging || start_drag,
        new_value,
    }
}

pub struct SliderResult {
    pub hovered: bool,
    pub dragging: bool,
    pub new_value: Option<f32>,
}

#[cfg(test)]
mod tests {
    use azalea_inventory::components::{Damage, MaxDamage, Unbreakable};
    use azalea_inventory::{ItemStack, ItemStackData};
    use azalea_registry::builtin::ItemKind;

    use super::{MenuElement, durability_bar, push_item_count, push_item_icon};

    fn damaged_pickaxe(damage: i32) -> ItemStackData {
        ItemStack::new(ItemKind::IronPickaxe, 1)
            .with_component(Damage { amount: damage })
            .as_present()
            .expect("pickaxe stack should be present")
            .clone()
    }

    #[test]
    fn item_count_uses_vanilla_y_coordinate() {
        let mut elements = Vec::new();
        push_item_count(&mut elements, 10.0, 20.0, 32.0, 2.0, 64);

        let MenuElement::Text { x, y, .. } = &elements[0] else {
            panic!("item count should render as text");
        };
        assert_eq!(*x, 20.0);
        assert_eq!(*y, 38.0);
    }

    #[test]
    fn durability_bar_matches_vanilla_visibility_width_and_color() {
        let pristine = ItemStackData::new(ItemKind::IronPickaxe, 1);
        let max_damage = pristine
            .get_component::<MaxDamage>()
            .expect("iron pickaxe should have max damage")
            .amount;
        assert!(durability_bar(&pristine).is_none());

        let halfway = damaged_pickaxe(max_damage / 2);
        let (width, color) = durability_bar(&halfway).expect("damaged item should show a bar");
        assert_eq!(width, 7);
        assert_eq!(color, [1.0, 1.0, 0.0, 1.0]);

        let broken = damaged_pickaxe(max_damage);
        let (width, color) = durability_bar(&broken).expect("fully damaged item still has a bar");
        assert_eq!(width, 0);
        assert_eq!(color, [1.0, 0.0, 0.0, 1.0]);

        let unbreakable = ItemStack::new(ItemKind::IronPickaxe, 1)
            .with_component(Damage { amount: 1 })
            .with_component(Unbreakable)
            .as_present()
            .expect("pickaxe stack should be present")
            .clone();
        assert!(durability_bar(&unbreakable).is_none());
    }

    #[test]
    fn item_icon_places_vanilla_durability_rectangles() {
        let pristine = ItemStackData::new(ItemKind::IronPickaxe, 1);
        let max_damage = pristine
            .get_component::<MaxDamage>()
            .expect("iron pickaxe should have max damage")
            .amount;
        let data = damaged_pickaxe(max_damage / 2);
        let mut elements = Vec::new();

        push_item_icon(&mut elements, 10.0, 20.0, 32.0, 2.0, &data);

        assert_eq!(elements.len(), 3);
        let MenuElement::Rect {
            x, y, w, h, color, ..
        } = &elements[1]
        else {
            panic!("durability background should be a rectangle");
        };
        assert_eq!((*x, *y, *w, *h), (14.0, 46.0, 26.0, 4.0));
        assert_eq!(*color, [0.0, 0.0, 0.0, 1.0]);

        let MenuElement::Rect {
            x, y, w, h, color, ..
        } = &elements[2]
        else {
            panic!("durability fill should be a rectangle");
        };
        assert_eq!((*x, *y, *w, *h), (14.0, 46.0, 14.0, 2.0));
        assert_eq!(*color, [1.0, 1.0, 0.0, 1.0]);
    }
}
