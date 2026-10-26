//! The Bedrock theme's title screen, laid out like Bedrock's `start_screen`
//! (Windows, signed in) from its `bedrock-samples` UI files.
//!
//! Sizes are Bedrock UI units times the GUI scale; Bedrock draws 4 px per
//! unit at 1080p, which is also Pomme's automatic scale there.
//!
//! Left out or stood in for, as Pomme has no equivalent:
//! - TODO: Play opens the world list until the Bedrock Play screen (Worlds,
//!   Friends and Servers tabs) exists.
//! - Realms and Marketplace draw but do nothing; the Marketplace art, its
//!   "Add-ons!" ribbon and the inbox button are online content.
//! - The featured server tile shows Pomme's dev server.
//! - Pomme's link and theme pickers sit in the top-left corner.

use super::*;
use crate::renderer::pipelines::skin_preview;

/// `main_buttons_stack_panel`: the top padding as a fraction of the height.
const TITLE_TOP_PAD: f32 = 0.12;
/// `title_panel_win10`: 55% of the width wide, `18.85%x` tall, offset down by
/// 10% of its own height.
const TITLE_W: f32 = 0.55;
const TITLE_PANEL_RATIO: f32 = 0.1885;
const TITLE_OFFSET: f32 = 0.10;
/// `title.png` is 1937x333, drawn at its own ratio inside the panel.
const TITLE_ASPECT: f32 = 333.0 / 1937.0;
/// `splash_text`, from the title panel's right middle.
const SPLASH_OFFSET: (f32, f32) = (-8.0, 10.0);

/// `main_buttons_panel` of `stacked_row`s, each holding a `main_button` 1
/// unit in from every edge.
const COLUMN_W: f32 = 150.0;
const ROW_H: f32 = 32.0;
/// `padding_3`, kept clear for the footer.
const BOTTOM_PAD: f32 = 10.0;

/// Form-fitting buttons (`light_content_form_fitting_button`,
/// `light_text_form_fitting_button`): a 24-unit content row, 5 in from each
/// side once `$button_offset` moves it 3 past the 2-unit border.
const FIT_H: f32 = 24.0;
const FIT_LEFT: f32 = 5.0;
const FIT_RIGHT: f32 = 5.0;
/// `friendsdrawer_button_panel` and the top-left pickers' corner offset.
const CORNER: f32 = 5.0;
/// `social_button_stackpanel`: an 18-unit slot holding the 12-unit glyph.
const ICON_SLOT: f32 = 18.0;
const SOCIAL_ICON: f32 = 12.0;
const SOCIAL_GAP: f32 = 3.0;
/// The pickers, sized like `inbox_button`.
const PICKER: f32 = 24.0;
const PICKER_ICON: f32 = 16.0;

/// `skin_panel` and `online_button_stack` are `50% - 75px` wide, raised 10%
/// of the height.
const SIDE_INSET: f32 = 75.0;
const SIDE_RAISE: f32 = 0.10;
/// `skin_panel`: 124 tall, raised 4 more.
const SKIN_PANEL_H: f32 = 124.0;
const SKIN_PANEL_RAISE: f32 = 4.0;
/// `skin_viewer_panel` is lifted 16 above the panel top.
const VIEWER_LIFT: f32 = 16.0;
/// Bedrock's paper doll and name tag are placed by engine renderers;
/// measured from a 1080p Bedrock screenshot against the viewer's top: the
/// model's height and where its head starts, and the name's top.
const MODEL_H: f32 = 72.5;
const MODEL_HEAD_TOP: f32 = 24.5;
const NAME_TOP: f32 = 9.0;
const NAMEPLATE_ALPHA: f32 = 0.25;
/// `dressing_room_button`: 25 tall at the bottom of the panel.
const DRESSING_H: f32 = 25.0;

/// `gathering_panel.horizontal_panel`'s left pad.
const LEFT_PAD: f32 = 5.0;
/// `gathering_badge` and `gathering_button`.
const TILE: (f32, f32) = (100.0, 30.0);
const TILE_BUTTON_H: f32 = 25.0;
/// The server the tile features.
const FEATURED_NAME: &str = "Pomme Dev";
const FEATURED_ADDRESS: &str = "dev.pomme.rs";
/// `lower_online_buttons_panel`: a 32 row offset 5% of the column in, then
/// the profile button's 5 pad.
const PROFILE_ROW_H: f32 = 32.0;
const PROFILE_INDENT: f32 = 0.05;
const PROFILE_PAD: f32 = 5.0;
/// `gamerpic_with_border`: a 16-unit head in an 18-unit black frame, offset 2
/// left.
const HEAD: f32 = 16.0;
const HEAD_GAP: f32 = 2.0;
const GAMERPIC_OFFSET: f32 = -2.0;

/// `start.text_panel`: a 10-tall strip 1 above the bottom, on `Black` at 0.6.
const FOOTER_H: f32 = 10.0;
const BACKING_ALPHA: f32 = 0.6;

const DARK_OUTLINE: [f32; 4] = [0.075, 0.075, 0.075, 1.0];
/// `$light_button_default_text_color`.
const LIGHT_TEXT: [f32; 4] = [0.3, 0.3, 0.3, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A Bedrock button's state (`new_ui_button_panel`'s default, hover and
/// pressed children; nothing here is ever locked).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Look {
    Normal,
    Hover,
    Pressed,
}

/// `light_button_assets`, or the realms `dark_button_assets`.
#[derive(Clone, Copy)]
enum Palette {
    Light,
    Realms,
}

impl Look {
    /// Pressed needs the cursor on the button, not just keyboard focus.
    fn of(button: &ButtonUse, mouse_held: bool) -> Self {
        if button.hovered && mouse_held {
            Self::Pressed
        } else if button.hovered || button.focused {
            Self::Hover
        } else {
            Self::Normal
        }
    }

    /// Draws the fill 1 unit inside the outline and returns the text colour.
    fn push_frame(
        self,
        palette: Palette,
        elements: &mut Vec<MenuElement>,
        rect: [f32; 4],
        gs: f32,
    ) -> [f32; 4] {
        let (fill, text) = match (palette, self) {
            (Palette::Light, Self::Normal) => (SpriteId::BedrockButton, LIGHT_TEXT),
            (Palette::Light, Self::Hover) => (SpriteId::BedrockButtonHover, WHITE),
            (Palette::Light, Self::Pressed) => (SpriteId::BedrockButtonPressed, WHITE),
            (Palette::Realms, Self::Normal) => (SpriteId::BedrockRealms, WHITE),
            (Palette::Realms, Self::Hover) => (SpriteId::BedrockRealmsHover, WHITE),
            (Palette::Realms, Self::Pressed) => (SpriteId::BedrockRealmsPressed, WHITE),
        };
        let outline = if self == Self::Normal {
            DARK_OUTLINE
        } else {
            WHITE
        };
        let [x, y, w, h] = rect;
        elements.push(MenuElement::NineSlice {
            x: x + gs,
            y: y + gs,
            w: w - 2.0 * gs,
            h: h - 2.0 * gs,
            sprite: fill,
            border: gs,
            tint: WHITE,
        });
        push_outline(elements, rect, gs, outline);
        text
    }

    /// `$button_pressed_offset`: pressed content sits 1 unit lower.
    fn drop(self, gs: f32) -> f32 {
        if self == Self::Pressed { gs } else { 0.0 }
    }

    /// Where a label centred in the `h`-tall row at `y` starts.
    fn label_y(self, y: f32, h: f32, gs: f32) -> f32 {
        y + (h - common::FONT_SIZE * gs) / 2.0 + self.drop(gs)
    }
}

/// Bedrock's colours are sRGB; sprite tints and icon colours are linear,
/// while text colours are linearised by the shader.
fn linear(color: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = color;
    [r.powf(2.2), g.powf(2.2), b.powf(2.2), a]
}

/// `focus_border_white`, a 1-unit ring tinted `color`.
fn push_outline(elements: &mut Vec<MenuElement>, rect: [f32; 4], gs: f32, color: [f32; 4]) {
    let [x, y, w, h] = rect;
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: SpriteId::BedrockOutline,
        border: gs,
        tint: linear(color),
    });
}

/// `common.dropdown_background`, nine-sliced 2 units in.
pub(super) fn push_dropdown_background(elements: &mut Vec<MenuElement>, rect: [f32; 4], gs: f32) {
    let [x, y, w, h] = rect;
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite: SpriteId::BedrockDropdown,
        border: 2.0 * gs,
        tint: WHITE,
    });
}

/// `settings_common.radio_visuals`: a 10-unit radio (or the item's icon) 3
/// units in, the label 16 further, over `dropDownHoverBG` when hovered or
/// `dropDownSelectBG` when checked.
pub(super) fn push_dropdown_row(
    elements: &mut Vec<MenuElement>,
    rect: [f32; 4],
    gs: f32,
    item: &DropItem,
    hovered: bool,
) {
    let [x, y, ..] = rect;
    if hovered {
        push_image(elements, rect, SpriteId::BedrockDropdownHover, WHITE);
    } else if item.radio == Some(true) {
        push_image(elements, rect, SpriteId::BedrockDropdownSelect, WHITE);
    }
    let left = x + 3.0 * gs;
    // The visuals are 16 tall at the row's top.
    let mid = y + 8.0 * gs;
    match (item.radio, item.icon) {
        (Some(on), _) => {
            let sprite = match (on, hovered) {
                (true, false) => SpriteId::BedrockRadioOn,
                (true, true) => SpriteId::BedrockRadioOnHover,
                (false, false) => SpriteId::BedrockRadioOff,
                (false, true) => SpriteId::BedrockRadioOffHover,
            };
            let radio = [left, mid - 5.0 * gs, 10.0 * gs, 10.0 * gs];
            push_image(elements, radio, sprite, WHITE);
        }
        (None, Some((icon, _))) => elements.push(MenuElement::Icon {
            x: left + 5.0 * gs,
            y: mid,
            icon,
            scale: 10.0 * gs,
            color: WHITE,
        }),
        (None, None) => {}
    }
    let label_y = mid + (0.5 - common::FONT_SIZE / 2.0) * gs;
    push_label(
        elements,
        left + 16.0 * gs,
        label_y,
        gs,
        item.label,
        WHITE,
        false,
    );
}

/// Bedrock's `Black` texture at `alpha`, its backing for labels and tiles.
fn push_backing(elements: &mut Vec<MenuElement>, rect: [f32; 4], alpha: f32) {
    push_image(
        elements,
        rect,
        SpriteId::BedrockBlack,
        [1.0, 1.0, 1.0, alpha],
    );
}

fn push_image(elements: &mut Vec<MenuElement>, rect: [f32; 4], sprite: SpriteId, tint: [f32; 4]) {
    let [x, y, w, h] = rect;
    elements.push(MenuElement::Image {
        x,
        y,
        w,
        h,
        sprite,
        tint,
    });
}

/// `text` on a backing from 1 unit above and left of its `h`-tall line,
/// `pad` wider than the text and 2 taller; left edge at `x`.
#[allow(clippy::too_many_arguments)]
fn push_backed_text(
    elements: &mut Vec<MenuElement>,
    [x, y]: [f32; 2],
    text_w: f32,
    pad: f32,
    h: f32,
    gs: f32,
    text: &str,
    alpha: f32,
) {
    let rect = [
        (x - 1.0) * gs,
        (y - 1.0) * gs,
        (text_w + pad) * gs,
        (h + 2.0) * gs,
    ];
    push_backing(elements, rect, alpha);
    let ty = y + (h - common::FONT_SIZE) / 2.0;
    push_label(elements, x * gs, ty * gs, gs, text, WHITE, false);
}

/// Unshadowed default-font text, as Bedrock's button labels draw.
fn push_label(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    gs: f32,
    text: &str,
    color: [f32; 4],
    centered: bool,
) {
    elements.push(MenuElement::McText {
        x,
        y,
        spans: vec![crate::ui::text::TextSpan::new(text.into(), color)],
        scale: common::FONT_SIZE * gs,
        centered,
        shadow: false,
    });
}

/// What one Bedrock button needs from the frame being built.
struct Buttons<'a> {
    elements: &'a mut Vec<MenuElement>,
    ctx: &'a mut FocusCtx,
    any_hovered: &'a mut bool,
    input: &'a MenuInput,
    gs: f32,
}

impl Buttons<'_> {
    /// Draws a button frame at `rect`; returns whether it fired, its look and
    /// its text colour.
    fn frame(&mut self, rect: [f32; 4], palette: Palette) -> (bool, Look, [f32; 4]) {
        let button = self.ctx.button(
            self.any_hovered,
            self.input.cursor,
            self.input.clicked,
            rect,
            true,
        );
        let look = Look::of(&button, self.input.mouse_held);
        let text = look.push_frame(palette, self.elements, rect, self.gs);
        (button.activated, look, text)
    }

    /// A button with its label centred.
    fn text(&mut self, rect: [f32; 4], palette: Palette, label: &str) -> bool {
        let (fired, look, color) = self.frame(rect, palette);
        let [x, y, w, h] = rect;
        let ty = look.label_y(y, h, self.gs);
        push_label(self.elements, x + w / 2.0, ty, self.gs, label, color, true);
        fired
    }

    /// A form-fitting button at `(x, y)`, `icon_w` units of icon slot before
    /// the label. Returns whether it fired, its look and where it drew, so
    /// the caller can draw the icon at the look's offset.
    fn fit(
        &mut self,
        x: f32,
        y: f32,
        h: f32,
        icon_w: f32,
        label: &str,
        label_w: f32,
    ) -> (bool, Look, [f32; 4]) {
        let gs = self.gs;
        let rect = [x, y, fit_width(icon_w, label_w) * gs, h * gs];
        let (fired, look, color) = self.frame(rect, Palette::Light);
        let tx = x + (FIT_LEFT + icon_w) * gs;
        let ty = look.label_y(y, h * gs, gs);
        push_label(self.elements, tx, ty, gs, label, color, false);
        (fired, look, rect)
    }
}

/// A form-fitting button's width in units around `icon_w` and a label.
fn fit_width(icon_w: f32, label_w: f32) -> f32 {
    FIT_LEFT + icon_w + label_w + FIT_RIGHT
}

/// The title panel's rect and the centre column's top, in units, for a
/// `w` x `h` unit screen (`main_buttons_stack_panel`'s two fill rows split
/// what the title, the four rows and the bottom pad leave).
fn title_and_column(w: f32, h: f32) -> ([f32; 4], f32) {
    let panel_w = w * TITLE_W;
    let panel_h = panel_w * TITLE_PANEL_RATIO;
    let pad = h * TITLE_TOP_PAD;
    let panel = [
        (w - panel_w) / 2.0,
        pad + panel_h * TITLE_OFFSET,
        panel_w,
        panel_h,
    ];
    let fill = ((h - pad - panel_h - ROW_H * 4.0 - BOTTOM_PAD) / 2.0).max(0.0);
    (panel, pad + panel_h + fill)
}

impl MainMenu {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_main_bedrock(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let gs = self.gui_scale(screen_w, screen_h);
        let (uw, uh) = (screen_w / gs, screen_h / gs);
        let units = |text: &str| text_width_fn(text, common::FONT_SIZE);

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;
        let mut any_clicked = false;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let (panel, column_top) = title_and_column(uw, uh);
        let [px, py, pw, ph] = panel;
        let logo_h = pw * TITLE_ASPECT;
        let logo = [
            px * gs,
            (py + (ph - logo_h) / 2.0) * gs,
            pw * gs,
            logo_h * gs,
        ];
        push_image(&mut elements, logo, SpriteId::BedrockTitle, WHITE);
        self.push_splash(
            &mut elements,
            (
                (px + pw + SPLASH_OFFSET.0) * gs,
                (py + ph / 2.0 + SPLASH_OFFSET.1) * gs,
            ),
            gs,
            &text_width_fn,
            false,
        );

        let mut buttons = Buttons {
            elements: &mut elements,
            ctx: &mut ctx,
            any_hovered: &mut any_hovered,
            input,
            gs,
        };

        let column_x = (uw - COLUMN_W) / 2.0;
        let rows = [
            ("Play", Palette::Light),
            ("Settings", Palette::Light),
            ("Realms", Palette::Realms),
            ("Marketplace", Palette::Light),
        ];
        for (i, (label, palette)) in rows.into_iter().enumerate() {
            let y = column_top + i as f32 * ROW_H;
            let rect = [
                (column_x + 1.0) * gs,
                (y + 1.0) * gs,
                (COLUMN_W - 2.0) * gs,
                (ROW_H - 2.0) * gs,
            ];
            if buttons.text(rect, palette, label) {
                any_clicked = true;
                match label {
                    "Play" => self.open_world_list(
                        gs,
                        &|t: &str| text_width_fn(t, common::FONT_SIZE * gs),
                        key_bindings,
                    ),
                    "Settings" => self.open_options(key_bindings),
                    _ => {}
                }
            }
        }

        // Social, top right.
        let signed_in = self.access_token.is_some();
        let social = match &*self.friends_data.read() {
            friends::FriendsState::Loaded(lists) if signed_in => {
                let online = lists
                    .friends
                    .iter()
                    .filter(|f| f.status.is_online())
                    .count();
                format!("Social ({online})")
            }
            _ => "Social".to_owned(),
        };
        let social_w = units(&social);
        let social_icon_w = ICON_SLOT + SOCIAL_GAP;
        let social_x = uw - CORNER - fit_width(social_icon_w, social_w);
        let (fired, look, rect) = buttons.fit(
            social_x * gs,
            CORNER * gs,
            FIT_H,
            social_icon_w,
            &social,
            social_w,
        );
        let glyph = if look == Look::Normal {
            SpriteId::BedrockSocial
        } else {
            SpriteId::BedrockSocialHover
        };
        let inset = (ICON_SLOT - SOCIAL_ICON) / 2.0;
        let icon = [
            rect[0] + (FIT_LEFT + inset) * gs,
            rect[1] + (FIT_H - SOCIAL_ICON) / 2.0 * gs + look.drop(gs),
            SOCIAL_ICON * gs,
            SOCIAL_ICON * gs,
        ];
        push_image(buttons.elements, icon, glyph, WHITE);
        if fired {
            any_clicked = true;
            if signed_in {
                self.open_friends(key_bindings);
            } else {
                self.open_account(key_bindings);
            }
        }

        // Pomme's pickers, top left.
        let mut picker_rects = [[0.0; 4]; 2];
        for (i, (glyph, is_links)) in [(ICON_LINK, true), (ICON_PAINTBRUSH, false)]
            .into_iter()
            .enumerate()
        {
            let rect = [
                (CORNER + i as f32 * (PICKER + SOCIAL_GAP)) * gs,
                CORNER * gs,
                PICKER * gs,
                PICKER * gs,
            ];
            picker_rects[i] = rect;
            let (fired, look, color) = buttons.frame(rect, Palette::Light);
            buttons.elements.push(MenuElement::Icon {
                x: rect[0] + rect[2] / 2.0,
                y: rect[1] + rect[3] / 2.0 + look.drop(gs),
                icon: glyph,
                scale: PICKER_ICON * gs,
                color: linear(color),
            });
            if fired {
                any_clicked = true;
                if is_links {
                    self.toggle_links();
                } else {
                    self.toggle_theme();
                }
            }
        }

        // The skin panel, bottom right.
        let side_w = uw / 2.0 - SIDE_INSET;
        let side_bottom = uh - uh * SIDE_RAISE;
        let skin_top = side_bottom - SKIN_PANEL_RAISE - SKIN_PANEL_H;
        let skin_cx = uw - side_w / 2.0;
        let viewer_top = skin_top - VIEWER_LIFT;
        let anchor_y =
            viewer_top + MODEL_HEAD_TOP + MODEL_H * skin_preview::MODEL_ANCHOR_BELOW_HEAD;
        self.bedrock_skin = Some(crate::renderer::MenuSkin {
            at: (skin_cx / uw, anchor_y / uh),
            scale: MODEL_H / (skin_preview::menu_model_height() * uh),
        });
        let name_w = units(&self.username);
        push_backed_text(
            buttons.elements,
            [skin_cx - name_w / 2.0, viewer_top + NAME_TOP],
            name_w,
            2.0,
            common::FONT_SIZE,
            gs,
            &self.username,
            NAMEPLATE_ALPHA,
        );
        let dressing = "Dressing Room";
        let dressing_w = units(dressing);
        let (fired, _, _) = buttons.fit(
            (skin_cx - fit_width(0.0, dressing_w) / 2.0) * gs,
            (skin_top + SKIN_PANEL_H - DRESSING_H) * gs,
            DRESSING_H,
            0.0,
            dressing,
            dressing_w,
        );
        let mut open_account = fired;

        // The left column, bottom up: the Profile row, then the featured
        // server's join button and tile.
        let profile_y = side_bottom - PROFILE_ROW_H + (PROFILE_ROW_H - FIT_H) / 2.0;
        let head_w = ICON_SLOT + HEAD_GAP;
        let profile = "Profile";
        let profile_w = units(profile);
        let (fired, look, rect) = buttons.fit(
            (side_w * PROFILE_INDENT + PROFILE_PAD) * gs,
            profile_y * gs,
            FIT_H,
            head_w,
            profile,
            profile_w,
        );
        let frame = [
            rect[0] + (FIT_LEFT + GAMERPIC_OFFSET) * gs,
            rect[1] + (FIT_H - ICON_SLOT) / 2.0 * gs + look.drop(gs),
            ICON_SLOT * gs,
            ICON_SLOT * gs,
        ];
        push_outline(buttons.elements, frame, gs, BLACK);
        let head = [frame[0] + gs, frame[1] + gs, HEAD * gs];
        match &self.player_face {
            Some(uuid) => buttons.elements.push(MenuElement::SkinFace {
                x: head[0],
                y: head[1],
                size: head[2],
                uuid: uuid.clone(),
                tint: WHITE,
            }),
            None => push_image(
                buttons.elements,
                [head[0], head[1], head[2], head[2]],
                SpriteId::SteveHead,
                WHITE,
            ),
        }
        open_account |= fired;
        if open_account {
            any_clicked = true;
            self.open_account(key_bindings);
        }

        // Only beside the column, not over it.
        if LEFT_PAD + TILE.0 <= side_w {
            let server = ServerEntry {
                name: FEATURED_NAME.to_owned(),
                address: FEATURED_ADDRESS.to_owned(),
                protocol: None,
                extra: Default::default(),
            };
            let join_y = side_bottom - PROFILE_ROW_H - TILE_BUTTON_H;
            let tile_y = join_y - TILE.1;
            let tile = [LEFT_PAD * gs, tile_y * gs, TILE.0 * gs, TILE.1 * gs];
            push_backing(buttons.elements, tile, BACKING_ALPHA);
            let icon = TILE.1 - 2.0;
            buttons.elements.push(MenuElement::Favicon {
                x: (LEFT_PAD + (TILE.0 - icon) / 2.0) * gs,
                y: (tile_y + 1.0) * gs,
                size: icon * gs,
                address: server.address.clone(),
            });
            push_outline(buttons.elements, tile, gs, WHITE);
            let rect = [LEFT_PAD * gs, join_y * gs, TILE.0 * gs, TILE_BUTTON_H * gs];
            if buttons.text(rect, Palette::Light, &server.name) {
                any_clicked = true;
                action = MenuAction::Connect {
                    server: server.address.clone(),
                    username: self.username.clone(),
                    protocol: servers::join_protocol(
                        &self.ping_results.read(),
                        &server.address,
                        server.protocol,
                    ),
                };
            }
            if !self.ping_results.read().contains_key(&server.address) {
                ping_all_servers(
                    &self.rt,
                    &[server],
                    &self.ping_results,
                    &self.ping_generation,
                );
            }
        }

        // The footer: the copyright bottom left, the version bottom right.
        let footer_y = uh - 1.0 - FOOTER_H;
        let version = format!("v{}", self.version);
        // `start.copyright`'s backing is 4 wider than its text, `version`'s 2.
        for (text, pad, right) in [
            ("\u{a9}Mojang AB", 4.0, false),
            (version.as_str(), 2.0, true),
        ] {
            let w = units(text);
            let x = if right { uw - 1.0 - w } else { 1.0 };
            push_backed_text(
                buttons.elements,
                [x, footer_y],
                w,
                pad,
                FOOTER_H,
                gs,
                text,
                BACKING_ALPHA,
            );
        }

        // The pickers' lists hang below them.
        let drop_style = DropdownStyle::bedrock(gs);
        let rows = LINKS.len().max(THEMES.len());
        let drop_bottom = (CORNER + PICKER + 2.0) * gs + drop_style.list_h(rows);
        self.push_links_dropdown(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            input.clicked,
            &drop_style,
            picker_rects[0],
            picker_rects[0][0],
            drop_bottom,
            DROP_LINKS_W * gs,
        );
        self.push_theme_dropdown(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            input.clicked,
            &drop_style,
            picker_rects[1],
            picker_rects[1][0],
            drop_bottom,
            DROP_THEME_W * gs,
        );

        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            clicked_button: any_clicked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_bedrock_at_1080p() {
        // A 1920x1009 window at 4 px per unit.
        let (panel, column_top) = title_and_column(480.0, 252.25);
        let logo_h = panel[2] * TITLE_ASPECT;
        let logo_top = panel[1] + (panel[3] - logo_h) / 2.0;
        assert!((panel[0] - 108.0).abs() < 0.01 && (panel[2] - 264.0).abs() < 0.01);
        assert!((logo_top - 37.4).abs() < 0.1 && (logo_top + logo_h - 82.8).abs() < 0.1);
        assert!((column_top - 97.14).abs() < 0.01);
    }

    #[test]
    fn looks() {
        let used = |hovered, focused| ButtonUse {
            hit: hovered,
            hovered,
            focused,
            activated: false,
        };
        assert_eq!(Look::of(&used(false, false), false), Look::Normal);
        assert_eq!(Look::of(&used(false, true), false), Look::Hover);
        assert_eq!(Look::of(&used(true, false), true), Look::Pressed);
        // Holding the mouse elsewhere doesn't press a focused button.
        assert_eq!(Look::of(&used(false, true), true), Look::Hover);
    }
}
