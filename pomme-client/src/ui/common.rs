use azalea_inventory::components::{
    CustomName, Damage, ItemName, MaxDamage, PartialOrFullProfile, Profile, Unbreakable,
};
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_registry::builtin::ItemKind;
use azalea_registry::tags::items::BUNDLES;

use crate::benchmark::UploadStatus;
use crate::player::inventory::item_resource_name;
use crate::player::item_cooldowns::{CooldownGroup, ItemCooldowns};
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::bundle::Frac;
use crate::ui::text_edit::TextFieldRenderInfo;

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const FONT_SIZE: f32 = 8.0;
/// Vanilla `Font.lineHeight`: one line of text plus its spacing.
pub const LINE_HEIGHT: i32 = 9;
pub const BTN_H: f32 = 20.0;
/// `Tooltip.create` splits at this width.
pub const TOOLTIP_WRAP: f32 = 170.0;
/// Vanilla `CommonColors.LIGHT_GRAY`.
pub const LIGHT_GRAY: [f32; 4] = [0.627, 0.627, 0.627, 1.0];
/// Vanilla's inactive-widget text colour
/// (`AbstractWidget.WithInactiveMessage.defaultInactiveMessage`).
pub const COL_DISABLED: [f32; 4] = LIGHT_GRAY;
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

/// Vanilla `Window.getGuiScaledWidth` and `getGuiScaledHeight`: `px` in GUI
/// units, rounded up (`Window.calculateScale`).
pub fn gui_size(px: f32, gs: f32) -> f32 {
    (px / gs).ceil()
}

/// Vanilla `this.width / 2` in a screen, in framebuffer px.
pub fn gui_half_width(screen_w: f32, gs: f32) -> f32 {
    (gui_size(screen_w, gs) / 2.0).floor() * gs
}

/// Vanilla `(float)this.width / 2.0f`, unrounded.
pub fn gui_float_half_width(screen_w: f32, gs: f32) -> f32 {
    gui_size(screen_w, gs) * gs / 2.0
}

/// Vanilla `AbstractSelectionList.getRowLeft` for a full-width list.
pub fn gui_row_left(screen_w: f32, row_w: f32, gs: f32) -> f32 {
    gui_half_width(screen_w, gs) - (row_w / 2.0).floor() * gs
}

/// Vanilla `this.height / 2` in a screen, in framebuffer px.
pub fn gui_half_height(screen_h: f32, gs: f32) -> f32 {
    (gui_size(screen_h, gs) / 2.0).floor() * gs
}

/// Vanilla `this.height / 4` in a screen, in framebuffer px.
pub fn gui_quarter_height(screen_h: f32, gs: f32) -> f32 {
    (gui_size(screen_h, gs) / 4.0).floor() * gs
}

/// Vanilla `FrameLayout.alignInDimension`: an element `len` GUI px long
/// placed `align` of the way along `px`, in framebuffer px.
pub fn gui_aligned(px: f32, len: f32, gs: f32, align: f32) -> f32 {
    ((gui_size(px, gs) - len) * align).floor() * gs
}

/// Vanilla `(this.width - w) / 2`, or the same along the height.
pub fn gui_centered(px: f32, len: f32, gs: f32) -> f32 {
    gui_aligned(px, len, gs, 0.5)
}

/// Vanilla `guiHeight() - n`, in framebuffer px.
pub fn gui_from_bottom(screen_h: f32, n: f32, gs: f32) -> f32 {
    (gui_size(screen_h, gs) - n) * gs
}

pub fn item_display_name(data: &ItemStackData) -> String {
    item_hover_component(data)
        .map(|name| name.to_string())
        .unwrap_or_else(|| crate::lang::item_display_name(data.kind))
}

/// Vanilla `ItemStack.getStyledHoverName`: the hover name in its rarity color,
/// italic when custom. The name's own styling wins where it sets one.
pub fn styled_hover_name(data: &ItemStackData) -> Vec<crate::ui::text::TextSpan> {
    use azalea_inventory::components::Rarity;
    // Default-component rarities aren't synced; absent means common.
    let color = match data.get_component::<Rarity>().as_deref() {
        Some(Rarity::Uncommon) => rgb(0xffff55),
        Some(Rarity::Rare) => rgb(0x55ffff),
        Some(Rarity::Epic) => rgb(0xff55ff),
        _ => WHITE,
    };
    let italic = data.get_component::<CustomName>().is_some();
    let mut spans = item_display_spans(data, color);
    for span in &mut spans {
        span.italic |= italic;
    }
    spans
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

/// Vanilla `Screen.extractMenuBackground`: `menu_background.png`, or
/// `inworld_menu_background.png` over a world, both black at 64/255.
pub fn push_menu_background(elements: &mut Vec<MenuElement>, screen_w: f32, screen_h: f32) {
    push_overlay(elements, screen_w, screen_h, 64.0 / 255.0);
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

/// The system clipboard's text, empty when there's none or it can't be read.
pub(crate) fn get_clipboard() -> String {
    // TODO: Android's clipboard is a Java service (ClipboardManager), via JNI.
    #[cfg(target_os = "android")]
    return String::new();
    // SAFETY: the general pasteboard is safe to use from any thread.
    #[cfg(target_os = "ios")]
    return unsafe { objc2_ui_kit::UIPasteboard::generalPasteboard().string() }
        .map(|text| text.to_string())
        .unwrap_or_default();
    #[cfg(not(mobile))]
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.get_text())
        .unwrap_or_default()
}

/// Copy `text` to the system clipboard, returning whether it succeeded.
pub(crate) fn set_clipboard(text: &str) -> bool {
    #[cfg(target_os = "android")]
    {
        let _ = text;
        false
    }
    #[cfg(target_os = "ios")]
    {
        let text = objc2_foundation::NSString::from_str(text);
        // SAFETY: as in `get_clipboard`.
        unsafe { objc2_ui_kit::UIPasteboard::generalPasteboard().setString(Some(&text)) };
        true
    }
    #[cfg(not(mobile))]
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

/// `EditBox`'s box: its field sprite, picked by focus alone.
pub(crate) fn push_text_field_box(
    elements: &mut Vec<MenuElement>,
    rect: [f32; 4],
    scale: f32,
    focused: bool,
) {
    let [x, y, w, h] = rect;
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: if focused {
            SpriteId::WidgetTextFieldHighlighted
        } else {
            SpriteId::WidgetTextField
        },
        border: scale,
        tint: WHITE,
    });
}

/// Selection highlight, display text, and caret for one text-field frame
/// (`EditBox.renderWidget`): blue block behind the shown slice, then a
/// `bar_w`-wide caret bar in insert mode or a trailing `_` glyph otherwise.
/// `pad_y` is the scaled-pixel unit for the vanilla overdraw: selection/caret
/// rects span `textY-1 .. textY+lineHeight+1` (one above, two below the 8px
/// glyph line). `ghost` is chat's inline suggestion suffix, drawn one `bar_w`
/// left of the caret (vanilla draws it at `cursorX - 1`, under the caret).
/// `spans` styles the shown text (ChatScreen's Brigadier coloring); `None`
/// draws it plain white.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_field_text(
    elements: &mut Vec<MenuElement>,
    info: &TextFieldRenderInfo,
    shown: &str,
    spans: Option<&[crate::ui::text::TextSpan]>,
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
    elements.push(match spans {
        Some(spans) => MenuElement::McText {
            x: text_x,
            y: text_y,
            spans: spans.to_vec(),
            scale: fs,
            centered: false,
            shadow: false,
        },
        None => MenuElement::Text {
            x: text_x,
            y: text_y,
            text: shown.into(),
            scale: fs,
            color: WHITE,
            centered: false,
        },
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
    elements.push(MenuElement::Text {
        x: x + size + gs - text_w,
        y: y + 9.0 * gs,
        text,
        scale: FONT_SIZE * gs,
        color: WHITE,
        centered: false,
    });
}

/// The hand over something clickable (vanilla `CursorTypes.POINTING_HAND`),
/// else the arrow.
pub fn cursor_icon(pointer: bool) -> winit::window::CursorIcon {
    if pointer {
        winit::window::CursorIcon::Pointer
    } else {
        winit::window::CursorIcon::Default
    }
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
    push_slot_with_owner(
        elements,
        x,
        y,
        size,
        scale,
        cursor,
        item,
        empty_sprite,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_slot_with_owner(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    cursor: (f32, f32),
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
    use_player_team: bool,
) -> bool {
    let hovered = slot_hovered(cursor, x, y, size, scale);
    push_slot_contents(
        elements,
        x,
        y,
        size,
        scale,
        hovered,
        item,
        empty_sprite,
        use_player_team,
    );
    hovered
}

/// A slot's item, or its empty-slot sprite, between the hover highlights.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_slot_contents(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    hovered: bool,
    item: &ItemStack,
    empty_sprite: Option<SpriteId>,
    use_player_team: bool,
) {
    push_highlighted(elements, x, y, scale, hovered, |elements| match item {
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
            push_item_icon_with_owner(elements, x, y, size, scale, data, use_player_team)
        }
    });
}

/// `draw` between a hovered slot's back and front highlights.
pub(crate) fn push_highlighted(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    scale: f32,
    hovered: bool,
    draw: impl FnOnce(&mut Vec<MenuElement>),
) {
    if hovered {
        push_slot_highlight(elements, x, y, scale, SpriteId::SlotHighlightBack);
    }
    draw(elements);
    if hovered {
        push_slot_highlight(elements, x, y, scale, SpriteId::SlotHighlightFront);
    }
}

/// Whether the cursor is over the slot at (x, y). Vanilla `isHovering`
/// reaches a pixel past each edge, so slots 18 apart leave no gap.
pub(crate) fn slot_hovered(cursor: (f32, f32), x: f32, y: f32, size: f32, scale: f32) -> bool {
    hit_test(
        cursor,
        [x - scale, y - scale, size + 2.0 * scale, size + 2.0 * scale],
    )
}

/// The hovered slot's 24x24 highlight (back or front), centred on the slot.
fn push_slot_highlight(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    scale: f32,
    sprite: SpriteId,
) {
    elements.push(MenuElement::Image {
        x: x - 4.0 * scale,
        y: y - 4.0 * scale,
        w: 24.0 * scale,
        h: 24.0 * scale,
        sprite,
        tint: WHITE,
    });
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
    push_item_icon_with_owner(elements, x, y, size, scale, data, true);
}

/// The identity vanilla's `PlayerSkinRenderCache` keys on: a whole
/// `ResolvableProfile`, so two heads with the same id but different textures
/// are different entries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HeadProfile {
    pub(crate) id: Option<uuid::Uuid>,
    pub(crate) name: Option<String>,
    pub(crate) textures: Option<String>,
    /// The `skinPatch`'s body texture, `namespace:path` under `textures/`,
    /// which replaces whatever skin the profile has.
    pub(crate) patch_texture: Option<String>,
}

impl HeadProfile {
    /// A player head item's `profile` component.
    pub(crate) fn of_stack(data: &ItemStackData) -> Option<Self> {
        if data.kind != ItemKind::PlayerHead {
            return None;
        }
        let profile = data.get_component::<Profile>()?;
        let textures = |properties: &azalea_auth::game_profile::GameProfileProperties| {
            properties.map.get("textures").map(|p| p.value.clone())
        };
        let patch_texture = profile
            .skin_patch
            .body
            .as_ref()
            .map(|texture| format!("{}:{}", texture.id.namespace(), texture.id.path()));
        Some(match &*profile.unpack {
            PartialOrFullProfile::Partial(partial) => Self {
                id: partial.id,
                name: partial.name.clone(),
                textures: textures(&partial.properties),
                patch_texture,
            },
            PartialOrFullProfile::Full(full) => Self {
                id: Some(full.uuid),
                name: Some(full.name.clone()),
                textures: textures(&full.properties),
                patch_texture,
            },
        })
    }

    /// `ResolvableProfile.create`: a profile carrying properties, or both or
    /// neither of id and name, is `Static` and never looks anything up; one
    /// with exactly one of them is `Dynamic` and resolves first.
    pub(crate) fn is_static(&self) -> bool {
        self.textures.is_some() || self.id.is_some() == self.name.is_some()
    }

    /// The skin from the game's assets it draws while its own loads, or
    /// when it has none: the `skinPatch` texture, which it always draws, else
    /// `DefaultPlayerSkin.get(partialProfile())`, picked by the partial
    /// profile's id (an offline id from the name when it has no id, else
    /// nil). A `namespace:path` under `textures/`.
    pub(crate) fn asset_skin(&self) -> std::borrow::Cow<'_, str> {
        const DEFAULT_SKINS: [&str; 18] = [
            "minecraft:entity/player/slim/alex",
            "minecraft:entity/player/slim/ari",
            "minecraft:entity/player/slim/efe",
            "minecraft:entity/player/slim/kai",
            "minecraft:entity/player/slim/makena",
            "minecraft:entity/player/slim/noor",
            "minecraft:entity/player/slim/steve",
            "minecraft:entity/player/slim/sunny",
            "minecraft:entity/player/slim/zuri",
            "minecraft:entity/player/wide/alex",
            "minecraft:entity/player/wide/ari",
            "minecraft:entity/player/wide/efe",
            "minecraft:entity/player/wide/kai",
            "minecraft:entity/player/wide/makena",
            "minecraft:entity/player/wide/noor",
            "minecraft:entity/player/wide/steve",
            "minecraft:entity/player/wide/sunny",
            "minecraft:entity/player/wide/zuri",
        ];
        if let Some(texture) = &self.patch_texture {
            return texture.into();
        }
        let id = self
            .id
            .or_else(|| {
                self.name
                    .as_deref()
                    .map(crate::user::UserData::offline_uuid)
            })
            .unwrap_or_default();
        let hash = crate::world::waypoints::java_uuid_hash(&id);
        DEFAULT_SKINS[hash.rem_euclid(DEFAULT_SKINS.len() as i32) as usize].into()
    }

    /// The key an [`Self::asset_skin`] texture is held under.
    pub(crate) fn asset_skin_key(texture: &str) -> String {
        format!("asset:{texture}")
    }

    /// The keys of the skins it can draw, in preference order: its own,
    /// unless a `skinPatch` replaces it, then [`Self::asset_skin`].
    pub(crate) fn skin_keys(&self) -> impl Iterator<Item = String> {
        let own = self.patch_texture.is_none().then(|| self.skin_key());
        own.into_iter()
            .chain([Self::asset_skin_key(&self.asset_skin())])
    }

    /// The key its skin is held under: its id, else its skin textures, else
    /// its name.
    pub(crate) fn skin_key(&self) -> String {
        match (self.id, &self.textures, &self.name) {
            (Some(id), _, _) => id.to_string(),
            (None, Some(textures), _) => format!("textures:{textures}"),
            (None, None, name) => name.clone().unwrap_or_default(),
        }
    }
}

pub(crate) fn push_item_icon_with_owner(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    data: &ItemStackData,
    use_player_team: bool,
) {
    push_item_glyph(
        elements,
        [x, y, size, size],
        item_resource_name(data.kind),
        Some(data),
        use_player_team,
    );
    push_item_decorations(elements, x, y, size, scale, data);
}

/// The item alone, with no count or bar (vanilla `GuiGraphics.fakeItem`).
pub(crate) fn push_item_glyph(
    elements: &mut Vec<MenuElement>,
    [x, y, w, h]: [f32; 4],
    item_name: String,
    data: Option<&ItemStackData>,
    use_player_team: bool,
) {
    elements.push(MenuElement::ItemIcon {
        x,
        y,
        w,
        h,
        item_name,
        item_stack: data.cloned(),
        use_player_team,
        item_tints: Vec::new(),
        tint: WHITE,
    });
}

/// Vanilla `GuiGraphics.itemDecorations`: durability bar, cooldown and stack
/// count.
pub(crate) fn push_item_decorations(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    size: f32,
    scale: f32,
    data: &ItemStackData,
) {
    push_item_bar(elements, x, y, scale, data);
    elements.push(MenuElement::ItemCooldown {
        x,
        y,
        scale,
        group: CooldownGroup::of(data),
    });
    if data.count > 1 {
        push_item_count(elements, x, y, size, scale, data.count);
    }
}

/// Vanilla `GuiGraphics.itemCooldown`: swaps each `ItemCooldown` for a
/// translucent white fill over the cooldown's remaining share of the slot,
/// rising from the bottom, or drops it when the item isn't on cooldown.
pub(crate) fn resolve_item_cooldowns(
    elements: &mut Vec<MenuElement>,
    cooldowns: &ItemCooldowns,
    partial_tick: f32,
) {
    elements.retain_mut(|element| {
        let MenuElement::ItemCooldown { x, y, scale, group } = element else {
            return true;
        };
        let cooldown = cooldowns.cooldown_percent(group, partial_tick);
        if cooldown <= 0.0 {
            return false;
        }
        let top = (16.0 * (1.0 - cooldown)).floor();
        *element = MenuElement::Rect {
            x: *x,
            y: *y + top * *scale,
            w: 16.0 * *scale,
            h: (16.0 * cooldown).ceil() * *scale,
            corner_radius: 0.0,
            color: [1.0, 1.0, 1.0, 127.0 / 255.0],
        };
        true
    });
}

/// Vanilla `Item.MAX_BAR_WIDTH`.
const MAX_BAR_WIDTH: i32 = 13;

fn push_item_bar(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    scale: f32,
    data: &ItemStackData,
) {
    let Some((width, color)) = item_bar(data) else {
        return;
    };

    let mut fill = |w: i32, h: f32, color| {
        elements.push(MenuElement::Rect {
            x: x + 2.0 * scale,
            y: y + 13.0 * scale,
            w: w as f32 * scale,
            h: h * scale,
            corner_radius: 0.0,
            color,
        });
    };
    fill(MAX_BAR_WIDTH, 2.0, [0.0, 0.0, 0.0, 1.0]);
    fill(width, 1.0, color);
}

/// Bar width and colour, following `BundleItem`'s override of `Item`'s bar.
pub(crate) fn item_bar(data: &ItemStackData) -> Option<(i32, [f32; 4])> {
    if BUNDLES.contains(&data.kind) {
        bundle_bar(data)
    } else {
        durability_bar(data)
    }
}

// BundleItem FULL_BAR_COLOR / BAR_COLOR: colorFromFloat(1, 1, .33, .33) and
// colorFromFloat(1, .44, .53, 1), channels floored.
const BUNDLE_FULL_BAR_COLOR: [f32; 4] = rgb(0xFF5454);
const BUNDLE_BAR_COLOR: [f32; 4] = rgb(0x7087FF);

fn bundle_bar(data: &ItemStackData) -> Option<(i32, [f32; 4])> {
    // getWeightSafe: an overflowing weight counts as full.
    let weight = crate::ui::bundle::contents(data)
        .map_or(Some(Frac::ZERO), |c| crate::ui::bundle::weight(&c.items))
        .unwrap_or(Frac::ONE);
    if weight.0 <= 0 {
        return None;
    }
    let width = (1 + weight.mul_and_truncate(12)).min(MAX_BAR_WIDTH as i64) as i32;
    let color = if weight.is_full() {
        BUNDLE_FULL_BAR_COLOR
    } else {
        BUNDLE_BAR_COLOR
    };
    Some((width, color))
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

    let max_width = MAX_BAR_WIDTH as f32;
    let width = (max_width - damage as f32 * max_width / max_damage as f32)
        .round()
        .clamp(0.0, max_width) as i32;
    let health = ((max_damage as f32 - damage as f32) / max_damage as f32).max(0.0);
    Some((width, durability_color(health)))
}

fn durability_color(health: f32) -> [f32; 4] {
    // Mth.hsvToRgb(health / 3, 1, 1) with vanilla's float order and truncation.
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
    let channel = |value: f32| ((value * 255.0) as i32).clamp(0, 255) as u32;
    rgb((channel(red) << 16) | (channel(green) << 8) | channel(blue))
}

/// Measures rendered text width in framebuffer px at the given font size.
pub type TextWidthFn<'a> = &'a dyn Fn(&str, f32) -> f32;

/// Inputs for the vanilla scrolling-label treatment: labels wider than the
/// widget are clipped to it and slide back and forth over time.
pub struct LabelScroll<'a> {
    pub text_width_fn: TextWidthFn<'a>,
    pub time_secs: f64,
}

/// A widget label: plain text in one color, or styled spans.
pub enum WidgetLabel<'a> {
    Text(&'a str, [f32; 4]),
    /// Spans with their own colours, which an inactive widget keeps, as a
    /// component's style colour wins over the label colour in vanilla.
    Spans(Vec<crate::ui::text::TextSpan>),
}

impl WidgetLabel<'_> {
    #[cfg(feature = "test-harness")]
    fn text(&self) -> String {
        match self {
            Self::Text(text, _) => (*text).to_owned(),
            Self::Spans(spans) => crate::ui::text::span_text(spans),
        }
    }

    /// Plain text recoloured to the inactive text colour.
    fn disabled(self) -> Self {
        match self {
            Self::Text(text, _) => Self::Text(text, COL_DISABLED),
            spans @ Self::Spans(_) => spans,
        }
    }

    fn width(&self, text_width_fn: TextWidthFn<'_>, fs: f32) -> f32 {
        match self {
            Self::Text(text, _) => text_width_fn(text, fs),
            Self::Spans(spans) => {
                let text: String = spans.iter().map(|span| span.text.as_str()).collect();
                text_width_fn(&text, fs)
            }
        }
    }

    fn into_element(self, x: f32, y: f32, fs: f32, centered: bool) -> MenuElement {
        match self {
            Self::Text(text, color) => MenuElement::Text {
                x,
                y,
                text: text.into(),
                scale: fs,
                color,
                centered,
            },
            Self::Spans(spans) => MenuElement::TextSpans {
                x,
                y,
                spans,
                scale: fs,
                centered,
            },
        }
    }
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
    label: WidgetLabel<'_>,
    scroll: Option<&LabelScroll<'_>>,
) {
    let ty = y + (h - fs) / 2.0 + 1.0;
    if let Some(s) = scroll {
        let margin = 2.0 * gs;
        let avail = w - 2.0 * margin;
        let lw = label.width(s.text_width_fn, fs);
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
            elements.push(label.into_element(x + margin - pos, ty, fs, false));
            elements.push(MenuElement::ScissorPop);
            return;
        }
    }
    elements.push(label.into_element(x + w / 2.0, ty, fs, true));
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
    push_button_label(
        elements,
        cursor,
        x,
        y,
        w,
        h,
        gs,
        fs,
        WidgetLabel::Text(label, WHITE),
        enabled,
        None,
    )
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
    push_button_label(
        elements,
        cursor,
        x,
        y,
        w,
        h,
        gs,
        fs,
        WidgetLabel::Text(label, WHITE),
        enabled,
        Some(scroll),
    )
}

/// A button with any label, scrolled when `scroll` is given and it overflows.
#[allow(clippy::too_many_arguments)]
pub fn push_button_label(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    gs: f32,
    fs: f32,
    label: WidgetLabel<'_>,
    enabled: bool,
    scroll: Option<&LabelScroll<'_>>,
) -> bool {
    let hovered = enabled && hit_test(cursor, [x, y, w, h]);
    #[cfg(feature = "test-harness")]
    crate::harness::buttons::note(&label.text(), [x, y, w, h]);

    let sprite = if !enabled {
        SpriteId::ButtonDisabled
    } else if hovered {
        SpriteId::ButtonHover
    } else {
        SpriteId::ButtonNormal
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

    let label = if enabled { label } else { label.disabled() };
    push_widget_label(elements, x, y, w, h, gs, fs, label, scroll);

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
    push_widget_label(
        elements,
        x,
        y,
        w,
        h,
        gs,
        fs,
        WidgetLabel::Text(label, text_col),
        Some(scroll),
    );

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
    use azalea_inventory::components::{BundleContents, Damage, MaxDamage, Unbreakable};
    use azalea_inventory::{ItemStack, ItemStackData};
    use azalea_registry::builtin::ItemKind;

    use super::{
        BUNDLE_BAR_COLOR, BUNDLE_FULL_BAR_COLOR, HeadProfile, MenuElement, item_bar,
        push_item_count, push_item_icon, push_slot_with_owner, resolve_item_cooldowns,
    };
    use crate::player::item_cooldowns::ItemCooldowns;

    #[test]
    fn screen_positions_round_in_gui_units() {
        use super::{
            gui_aligned, gui_centered, gui_from_bottom, gui_half_width, gui_quarter_height,
        };
        // 1280x721 at scale 3 is 427x241 GUI px, both rounded up.
        assert_eq!(gui_half_width(1280.0, 3.0), 213.0 * 3.0);
        assert_eq!(gui_half_width(1281.0, 3.0), 213.0 * 3.0);
        assert_eq!(gui_quarter_height(721.0, 3.0), 60.0 * 3.0);
        assert_eq!(gui_centered(1281.0, 200.0, 3.0), 113.0 * 3.0);
        assert_eq!(gui_centered(1281.0, 15.0, 3.0), 206.0 * 3.0);
        assert_eq!(gui_aligned(721.0, 100.0, 3.0, 0.25), 35.0 * 3.0);
        assert_eq!(gui_from_bottom(721.0, 22.0, 3.0), 219.0 * 3.0);
    }

    #[test]
    fn a_cooldown_covers_its_remaining_share_from_the_bottom() {
        let shield = ItemStackData::new(ItemKind::Shield, 1);
        let mut elements = Vec::new();
        push_item_icon(&mut elements, 10.0, 20.0, 32.0, 2.0, &shield);
        push_item_icon(
            &mut elements,
            50.0,
            20.0,
            32.0,
            2.0,
            &ItemStackData::new(ItemKind::Stick, 1),
        );
        let mut cooldowns = ItemCooldowns::default();
        cooldowns.set("minecraft:shield".into(), 4);
        cooldowns.tick();

        resolve_item_cooldowns(&mut elements, &cooldowns, 0.0);
        let rects: Vec<_> = elements
            .iter()
            .filter_map(|e| match e {
                MenuElement::Rect { x, y, w, h, .. } => Some([*x, *y, *w, *h]),
                MenuElement::ItemCooldown { .. } => panic!("unresolved cooldown"),
                _ => None,
            })
            .collect();
        // Three of four ticks left: the top quarter of the 16-unit slot clear.
        assert_eq!(rects, [[10.0, 28.0, 32.0, 24.0]]);
    }

    #[test]
    fn a_head_draws_its_patch_or_its_ids_default_skin() {
        let head = |id, name: Option<&str>, patch: Option<&str>| HeadProfile {
            id,
            name: name.map(str::to_owned),
            textures: None,
            patch_texture: patch.map(str::to_owned),
        };
        // `UUID.hashCode` of the nil id is 0: the first default skin.
        assert_eq!(
            head(None, None, None).asset_skin(),
            "minecraft:entity/player/slim/alex"
        );
        // A name alone picks by its offline id.
        assert_eq!(
            head(None, Some("Steve"), None).asset_skin(),
            head(
                Some(crate::user::UserData::offline_uuid("Steve")),
                None,
                None
            )
            .asset_skin()
        );
        let patched = head(None, Some("Steve"), Some("pack:skins/knight"));
        assert_eq!(patched.asset_skin(), "pack:skins/knight");
        assert_eq!(
            patched.skin_keys().collect::<Vec<_>>(),
            ["asset:pack:skins/knight"]
        );
        assert_eq!(head(None, Some("Steve"), None).skin_keys().count(), 2);
    }

    fn present(stack: ItemStack) -> ItemStackData {
        stack.as_present().expect("stack should be present").clone()
    }

    fn damaged_pickaxe(damage: i32) -> ItemStackData {
        present(ItemStack::new(ItemKind::IronPickaxe, 1).with_component(Damage { amount: damage }))
    }

    fn pickaxe_max_damage() -> i32 {
        ItemStackData::new(ItemKind::IronPickaxe, 1)
            .get_component::<MaxDamage>()
            .expect("iron pickaxe should have max damage")
            .amount
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
        let max_damage = pickaxe_max_damage();
        assert!(item_bar(&damaged_pickaxe(0)).is_none());

        let halfway = damaged_pickaxe(max_damage / 2);
        let (width, color) = item_bar(&halfway).expect("damaged item should show a bar");
        assert_eq!(width, 7);
        assert_eq!(color, [1.0, 1.0, 0.0, 1.0]);

        let broken = damaged_pickaxe(max_damage);
        let (width, color) = item_bar(&broken).expect("fully damaged item still has a bar");
        assert_eq!(width, 0);
        assert_eq!(color, [1.0, 0.0, 0.0, 1.0]);

        assert!(item_bar(&damaged_pickaxe(-1)).is_none());

        let over_damaged = damaged_pickaxe(max_damage + 1);
        let (width, color) =
            item_bar(&over_damaged).expect("over-max damage should clamp to max damage");
        assert_eq!(width, 0);
        assert_eq!(color, [1.0, 0.0, 0.0, 1.0]);

        let unbreakable = present(
            ItemStack::new(ItemKind::IronPickaxe, 1)
                .with_component(Damage { amount: 1 })
                .with_component(Unbreakable),
        );
        assert!(item_bar(&unbreakable).is_none());

        let damage_without_max =
            present(ItemStack::new(ItemKind::Stone, 1).with_component(Damage { amount: 1 }));
        assert!(item_bar(&damage_without_max).is_none());

        let max_without_damage =
            present(ItemStack::new(ItemKind::Stone, 1).with_component(MaxDamage { amount: 10 }));
        assert!(item_bar(&max_without_damage).is_none());
    }

    #[test]
    fn item_icon_places_vanilla_durability_rectangles() {
        let data = damaged_pickaxe(pickaxe_max_damage() / 2);
        let mut elements = Vec::new();

        push_item_icon(&mut elements, 10.0, 20.0, 32.0, 2.0, &data);

        // The icon, the bar's two rectangles, then the cooldown placeholder.
        assert_eq!(elements.len(), 4);
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

    fn bundle(items: Vec<ItemStack>) -> ItemStackData {
        present(ItemStack::new(ItemKind::Bundle, 1).with_component(BundleContents { items }))
    }

    #[test]
    fn bundle_bar_matches_vanilla_fullness() {
        assert!(item_bar(&ItemStackData::new(ItemKind::Bundle, 1)).is_none());
        assert!(item_bar(&bundle(Vec::new())).is_none());

        let stone = |count| ItemStack::new(ItemKind::Stone, count);
        assert_eq!(
            item_bar(&bundle(vec![stone(1)])),
            Some((1, BUNDLE_BAR_COLOR))
        );
        assert_eq!(
            item_bar(&bundle(vec![stone(32)])),
            Some((7, BUNDLE_BAR_COLOR))
        );
        assert_eq!(
            item_bar(&bundle(vec![stone(64)])),
            Some((13, BUNDLE_FULL_BAR_COLOR))
        );
        assert_eq!(
            item_bar(&bundle(vec![ItemStack::new(ItemKind::IronPickaxe, 1)])),
            Some((13, BUNDLE_FULL_BAR_COLOR))
        );

        let nested = ItemStack::new(ItemKind::Bundle, 1);
        assert_eq!(item_bar(&bundle(vec![nested])), Some((1, BUNDLE_BAR_COLOR)));

        let damaged_empty = present(
            ItemStack::new(ItemKind::Bundle, 1)
                .with_component(MaxDamage { amount: 10 })
                .with_component(Damage { amount: 5 }),
        );
        assert!(item_bar(&damaged_empty).is_none());
    }

    #[test]
    fn fake_item_icon_does_not_use_player_team_owner() {
        let item = ItemStack::from(ItemKind::Stone);
        let mut elements = Vec::new();
        push_slot_with_owner(
            &mut elements,
            0.0,
            0.0,
            18.0,
            1.0,
            (-1.0, -1.0),
            &item,
            None,
            false,
        );
        assert!(elements.iter().any(|element| matches!(
            element,
            MenuElement::ItemIcon {
                use_player_team: false,
                ..
            }
        )));
    }

    #[test]
    fn slot_hover_reaches_one_gui_pixel_past_each_edge() {
        let item = ItemStack::from(ItemKind::Stone);
        // A 16-unit slot at GUI scale 2, as vanilla's `isHovering` sees it.
        let hovered = |cursor| {
            push_slot_with_owner(
                &mut Vec::new(),
                20.0,
                20.0,
                32.0,
                2.0,
                cursor,
                &item,
                None,
                true,
            )
        };
        assert!(hovered((18.0, 30.0)));
        assert!(hovered((53.9, 53.9)));
        assert!(!hovered((17.9, 30.0)));
        assert!(!hovered((54.0, 30.0)));
    }
}
