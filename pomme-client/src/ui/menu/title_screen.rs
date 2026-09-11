//! Vanilla `TitleScreen`, drawn when the theme switcher is set to Default.
//!
//! Positions come straight from `TitleScreen.init` / `LogoRenderer` /
//! `SplashRenderer` in GUI units and are multiplied by the GUI scale, so the
//! constants below read the same as the reference.
//!
//! Deliberate gaps against the reference:
//! - TODO: no 2000 ms widget fade-in (`TitleScreen.extractRenderState`). The
//!   theme wipe already covers the switch; this would only show on first entry.
//! - TODO: the copyright line is clickable but not in the Tab ring, unlike
//!   vanilla's `PlainTextButton`.
//! - Realms notification icons, demo mode, the 1-in-10000 `minceraft.png`
//!   easter-egg logo and the IDE-only test-world button are all omitted.

use std::f32::consts::TAU;

use super::*;

/// `LogoRenderer`
const LOGO_W: f32 = 256.0;
const LOGO_H: f32 = 44.0;
const EDITION_W: f32 = 128.0;
const EDITION_H: f32 = 14.0;
const LOGO_HEIGHT_OFFSET: f32 = 30.0;
const EDITION_LOGO_OVERLAP: f32 = 7.0;

/// `SplashRenderer`
const SPLASH_WIDTH_OFFSET: f32 = 123.0;
const SPLASH_HEIGHT_OFFSET: f32 = 69.0;
const SPLASH_ANGLE: f32 = -0.349_065_84;
/// `SplashManager.DEFAULT_STYLE`, `0xFFFFFF00`.
const SPLASH_COLOR: [f32; 4] = [1.0, 1.0, 0.0, 1.0];

/// `TitleScreen.init`
const ROW_SPACING: f32 = 24.0;
const COL_W: f32 = 200.0;
const HALF_W: f32 = 98.0;
const ICON_W: f32 = 20.0;
const ICON_GAP: f32 = 4.0;
/// `CommonButtons` draws its icons at 15x15 regardless of the button size.
const ICON_SPRITE: f32 = 15.0;
const FOOTER_OFFSET: f32 = 10.0;

const COPYRIGHT: &str = "Copyright Mojang AB. Do not distribute!";

/// Dropdown widths, wide enough for the longest label at `DropdownStyle`'s
/// 10-unit pad plus 11-unit icon.
const DROP_LINKS_W: f32 = 74.0;
const DROP_THEME_W: f32 = 66.0;

impl MainMenu {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_main_vanilla(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let cursor = input.cursor;
        let clicked = input.clicked;
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;
        let mut any_clicked = false;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        let cx = screen_w / 2.0;
        // `int topPos = this.height / 4 + 48`.
        let base = screen_h / 4.0 + 48.0 * gs;
        let col_x = cx - COL_W / 2.0 * gs;
        let col_w = COL_W * gs;

        // Logo, then the edition strip overlapping its last 7 units.
        elements.push(MenuElement::Image {
            x: cx - LOGO_W / 2.0 * gs,
            y: LOGO_HEIGHT_OFFSET * gs,
            w: LOGO_W * gs,
            h: LOGO_H * gs,
            sprite: SpriteId::MinecraftLogo,
            tint: WHITE,
        });
        elements.push(MenuElement::Image {
            x: cx - EDITION_W / 2.0 * gs,
            y: (LOGO_HEIGHT_OFFSET + LOGO_H - EDITION_LOGO_OVERLAP) * gs,
            w: EDITION_W * gs,
            h: EDITION_H * gs,
            sprite: SpriteId::MinecraftEdition,
            tint: WHITE,
        });

        self.push_splash(&mut elements, cx, gs, &text_width_fn);

        // Singleplayer and Realms are drawn so the column matches vanilla, but
        // neither is implemented, so both stay inactive.
        // TODO: enable Singleplayer once world loading exists. Realms never.
        let rows: [(&str, bool); 3] = [
            ("Singleplayer", false),
            ("Multiplayer", true),
            ("Minecraft Realms", false),
        ];
        for (i, (label, enabled)) in rows.iter().enumerate() {
            let y = base + i as f32 * ROW_SPACING * gs;
            let hit = push_button_f(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                cursor,
                clicked,
                col_x,
                y,
                col_w,
                btn_h,
                gs,
                label,
                *enabled,
            );
            if hit {
                any_clicked = true;
                if *label == "Multiplayer" {
                    self.set_screen(Screen::ServerList);
                }
            }
        }

        // `getHorizontalPosition`: three 20-wide buttons, 4 apart, centred.
        let icon_row_y = base + 3.0 * ROW_SPACING * gs;
        let icon_size = ICON_W * gs;
        let row_w = 3.0 * ICON_W + 2.0 * ICON_GAP;
        let icon_x0 = cx - row_w / 2.0 * gs;
        let friends_enabled = self.access_token.is_some();
        let icons: [(SpriteId, bool, &str); 3] = [
            (
                SpriteId::IconFriends,
                friends_enabled,
                if friends_enabled {
                    "Friends"
                } else {
                    "Sign in to use friends"
                },
            ),
            // TODO: no language selection yet.
            (SpriteId::IconLanguage, false, "Language..."),
            (
                SpriteId::IconAccessibility,
                true,
                "Accessibility Settings...",
            ),
        ];
        for (i, (sprite, enabled, tip)) in icons.iter().enumerate() {
            let x = icon_x0 + i as f32 * (ICON_W + ICON_GAP) * gs;
            if push_icon_button(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                cursor,
                clicked,
                x,
                icon_row_y,
                icon_size,
                gs,
                IconFace::Sprite {
                    id: *sprite,
                    w: ICON_SPRITE,
                    h: ICON_SPRITE,
                },
                *enabled,
                screen_w,
                screen_h,
                tip,
            ) {
                any_clicked = true;
                match sprite {
                    SpriteId::IconFriends => self.open_friends(),
                    SpriteId::IconAccessibility => {
                        self.settings_back = Screen::Main;
                        self.set_screen(Screen::OptionsAccessibility);
                    }
                    _ => {}
                }
            }
        }

        // The pair splits the 200-wide column with a 2-unit gutter.
        let bottom_y = base + 4.0 * ROW_SPACING * gs;
        let half_w = HALF_W * gs;
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            col_x,
            bottom_y,
            half_w,
            btn_h,
            gs,
            "Options...",
            true,
        ) {
            any_clicked = true;
            self.open_options();
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            cx + 2.0 * gs,
            bottom_y,
            half_w,
            btn_h,
            gs,
            "Quit Game",
            true,
        ) {
            any_clicked = true;
            action = MenuAction::Quit;
        }

        // Pomme's own entries; vanilla has no slot for these, so they sit in
        // the bottom-right corner in the same frame as the row above.
        let extras_y = screen_h - (FOOTER_OFFSET + 4.0 + ICON_W) * gs;
        let links_x = screen_w - (ICON_W * 2.0 + ICON_GAP + 4.0) * gs;
        let theme_x = screen_w - (ICON_W + 4.0) * gs;
        for (x, glyph, tip, is_links) in [
            (links_x, ICON_LINK, "Links", true),
            (theme_x, ICON_PAINTBRUSH, "Theme", false),
        ] {
            if push_icon_button(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                cursor,
                clicked,
                x,
                extras_y,
                icon_size,
                gs,
                IconFace::Glyph(glyph),
                true,
                screen_w,
                screen_h,
                tip,
            ) {
                any_clicked = true;
                if is_links {
                    self.toggle_links();
                } else {
                    self.toggle_theme();
                }
            }
        }

        let footer_y = screen_h - FOOTER_OFFSET * gs;
        elements.push(MenuElement::Text {
            x: 2.0 * gs,
            y: footer_y,
            text: self.version.clone(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
        let copy_w = text_width_fn(COPYRIGHT, fs);
        let copy_x = screen_w - copy_w - 2.0 * gs;
        let copy_hovered = common::hit_test(cursor, [copy_x, footer_y, copy_w, FOOTER_OFFSET * gs]);
        any_hovered |= copy_hovered;
        elements.push(MenuElement::Text {
            x: copy_x,
            y: footer_y,
            text: COPYRIGHT.into(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
        if copy_hovered {
            // `PlainTextButton` underlines on hover; `MenuElement::Text` carries
            // no underline flag, so draw the rule directly.
            elements.push(MenuElement::Rect {
                x: copy_x,
                y: footer_y + fs,
                w: copy_w,
                h: gs,
                corner_radius: 0.0,
                color: WHITE,
            });
            if clicked {
                any_clicked = true;
                self.settings_back = Screen::Main;
                self.set_screen(Screen::OptionsCredits);
            }
        }

        // Both icons sit against the right edge, so both lists right-align to
        // it rather than opening off-screen.
        let drop_style = DropdownStyle::new(gs);
        let drop_bottom = extras_y - 2.0 * gs;
        let drop_right = screen_w - 4.0 * gs;
        let links_w = DROP_LINKS_W * gs;
        self.push_links_dropdown(
            &mut elements,
            &mut any_hovered,
            cursor,
            clicked,
            &drop_style,
            [links_x, extras_y, icon_size, icon_size],
            drop_right - links_w,
            drop_bottom,
            links_w,
        );
        let theme_w = DROP_THEME_W * gs;
        self.push_theme_dropdown(
            &mut elements,
            &mut any_hovered,
            cursor,
            clicked,
            &drop_style,
            [theme_x, extras_y, icon_size, icon_size],
            drop_right - theme_w,
            drop_bottom,
            theme_w,
        );

        if let Some(theme_action) = self.drive_theme_transition(&mut elements, screen_w, screen_h) {
            action = theme_action;
        }

        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            // Vanilla's `TitleScreen.extractBackground` is empty: no blur and no
            // menu-background tint, just the panorama. (`panorama_overlay.png`
            // is a fully transparent 1x1 in 26.2, so it draws nothing.)
            blur: 0.0,
            clicked_button: any_clicked,
        }
    }

    /// `SplashRenderer.extractRenderState`.
    fn push_splash(
        &self,
        elements: &mut Vec<MenuElement>,
        cx: f32,
        gs: f32,
        text_width_fn: &impl Fn(&str, f32) -> f32,
    ) {
        let Some(ref splash) = self.splash else {
            return;
        };

        // Width in GUI units, which is what vanilla's `font.width` returns.
        let text_w = text_width_fn(splash, common::FONT_SIZE);
        let millis = self.created.elapsed().as_millis() % 1000;
        let phase = 1.8 - ((millis as f32 / 1000.0 * TAU).sin() * 0.1).abs();
        let text_scale = phase * 100.0 / (text_w + 32.0);

        let pivot = (cx + SPLASH_WIDTH_OFFSET * gs, SPLASH_HEIGHT_OFFSET * gs);
        let scale = common::FONT_SIZE * gs * text_scale;
        elements.push(MenuElement::McTextRotated {
            // `accept(LEFT, -textWidth / 2, -8, ...)` in the scaled frame.
            x: pivot.0 - text_w / 2.0 * gs * text_scale,
            y: pivot.1 - scale,
            pivot,
            rotation: SPLASH_ANGLE,
            spans: vec![crate::ui::text::TextSpan::new(splash.clone(), SPLASH_COLOR)],
            scale,
            shadow: true,
        });
    }
}

/// An icon button in the vanilla frame, wired into the Tab ring. Returns
/// whether it was activated: clicked, or Enter/Space while focused.
#[allow(clippy::too_many_arguments)]
fn push_icon_button(
    elements: &mut Vec<MenuElement>,
    ctx: &mut FocusCtx,
    any_hovered: &mut bool,
    cursor: (f32, f32),
    clicked: bool,
    x: f32,
    y: f32,
    size: f32,
    gs: f32,
    face: IconFace,
    enabled: bool,
    screen_w: f32,
    screen_h: f32,
    tooltip: &str,
) -> bool {
    let focused = ctx.focused(enabled);
    let hovered = enabled && common::hit_test(cursor, [x, y, size, size]);
    push_icon_widget(elements, x, y, size, gs, face, enabled, hovered || focused);
    *any_hovered |= hovered;
    let keyboard = focused && ctx.activate;
    if keyboard {
        ctx.fired = true;
    }
    if hovered {
        common::push_tooltip(elements, cursor, screen_w, screen_h, gs, tooltip);
    }
    (hovered && clicked) || keyboard
}
