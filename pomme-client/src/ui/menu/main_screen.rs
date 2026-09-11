use super::*;

impl MainMenu {
    /// The title screen, in whichever theme is selected. Branching here
    /// rather than at the screen dispatcher keeps the Friends dialog's
    /// blurred backdrop, which re-runs this builder, on the same theme.
    pub(super) fn build_main(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        match self.theme {
            PanoramaTheme::Pomme => self.build_main_pomme(screen_w, screen_h, input, text_width_fn),
            PanoramaTheme::Default => {
                self.build_main_vanilla(screen_w, screen_h, input, text_width_fn)
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn build_main_pomme(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: impl Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let cursor = input.cursor;
        let clicked = input.clicked;

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;
        let mut any_clicked = false;

        let anim_t = self
            .menu_open_time
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f32();
        let panel_t = ease_out_cubic((anim_t / 2.0).min(1.0));

        let accent: [f32; 4] = [0.29, 0.87, 0.5, 1.0];
        let glass: [f32; 4] = [0.07, 0.08, 0.16, 0.55];
        let glass_hover: [f32; 4] = [0.12, 0.14, 0.25, 0.65];
        let text_col: [f32; 4] = [0.89, 0.90, 0.96, 0.85];
        let text_bright: [f32; 4] = [0.94, 0.95, 0.98, 1.0];
        let text_dim: [f32; 4] = [0.53, 0.56, 0.69, 0.6];
        let text_disabled: [f32; 4] = [0.45, 0.47, 0.58, 0.35];
        let border: [f32; 4] = [1.0, 1.0, 1.0, 0.05];

        struct BtnDef {
            label: &'static str,
            id: u8,
            enabled: bool,
        }
        let buttons = [
            BtnDef {
                label: "Singleplayer",
                id: 0,
                enabled: false,
            },
            BtnDef {
                label: "Multiplayer",
                id: 1,
                enabled: true,
            },
            BtnDef {
                label: "Quit Game",
                id: 2,
                enabled: true,
            },
        ];

        // Clamp by width too: past this the panel width clamp would bind and
        // text scaled by height alone would outgrow the panel.
        let s = (screen_h / 400.0).max(1.0).min(screen_w * 0.4 / 260.0);
        let panel_w = (260.0 * s).min(screen_w * 0.4);
        let panel_pad = 28.0 * s;
        let panel_r = 14.0 * s;
        let accent_bar_h = 3.0 * s;
        let title_size = 40.0 * s;
        let sub_size = 9.0 * s;
        let content_w = panel_w - panel_pad * 2.0;
        let btn_h = 36.0 * s;
        let btn_gap = 5.0 * s;
        let btn_r = 8.0 * s;
        let font_size = 11.0 * s;
        let accent_w = 3.0 * s;
        let icon_size = 28.0 * s;
        let icon_gap = 6.0 * s;
        let icon_row_gap = icon_gap;

        let header_h = accent_bar_h + 14.0 * s + title_size + 4.0 * s + sub_size + 18.0 * s;
        let btns_total = buttons.len() as f32 * (btn_h + btn_gap) - btn_gap;
        let icons_total = icon_size * 2.0 + icon_row_gap;
        let panel_h = (panel_pad
            + header_h
            + 1.0
            + 16.0 * s
            + btns_total
            + 16.0 * s
            + icons_total
            + panel_pad)
            .min(screen_h * 0.9);
        let panel_margin = (screen_w * 0.06).max(12.0);
        let panel_start_x = -panel_w;
        let panel_final_x = panel_margin;
        let panel_x = panel_start_x + (panel_final_x - panel_start_x) * panel_t;
        let panel_y = (screen_h - panel_h) / 2.0;
        let btn_x = panel_x + panel_pad;

        elements.push(MenuElement::FrostedRect {
            x: panel_x,
            y: panel_y,
            w: panel_w,
            h: panel_h,
            corner_radius: panel_r,
            tint: [0.055, 0.06, 0.13, 0.72],
        });

        let mut cy = panel_y + panel_pad;

        elements.push(MenuElement::Rect {
            x: btn_x,
            y: cy,
            w: 50.0 * s,
            h: accent_bar_h,
            corner_radius: accent_bar_h * 0.5,
            color: [accent[0], accent[1], accent[2], 0.7],
        });
        cy += accent_bar_h + 14.0 * s;

        let pomme_w = text_width_fn("Pomme", title_size);
        elements.push(MenuElement::Text {
            x: btn_x,
            y: cy,
            text: "Pomme".into(),
            scale: title_size,
            color: COL_WORDMARK,
            centered: false,
        });

        let sub_x = btn_x + pomme_w + 8.0 * s;
        let sub_y1 = cy + title_size - sub_size * 2.0 - 4.0 * s;
        let sub_y2 = cy + title_size - sub_size - 1.0 * s;
        let badge_t = ((anim_t - 2.0) / 0.3).clamp(0.0, 1.0);
        let badge_scale = ease_out_cubic(badge_t);

        let rust_w = text_width_fn("Rust", sub_size);
        let badge_pad_x = 5.0 * s;
        let badge_pad_y = 2.5 * s;
        let badge_w = rust_w + badge_pad_x * 2.0;
        let badge_h = sub_size + badge_pad_y * 2.0;
        let badge_r = badge_h * 0.5;

        if badge_t < 1.0 {
            elements.push(MenuElement::Text {
                x: sub_x,
                y: sub_y1,
                text: "Java".into(),
                scale: sub_size,
                color: text_dim,
                centered: false,
            });
        }

        if badge_t > 0.0 {
            let bx = sub_x - badge_pad_x;
            let by = sub_y1 - badge_pad_y;
            let bw = badge_w * badge_scale;
            elements.push(MenuElement::Rect {
                x: bx,
                y: by,
                w: bw,
                h: badge_h,
                corner_radius: badge_r,
                color: [accent[0], accent[1], accent[2], 0.9 * badge_scale],
            });
            if badge_t >= 0.5 {
                let text_a = ((badge_t - 0.5) / 0.5).min(1.0);
                elements.push(MenuElement::Text {
                    x: sub_x,
                    y: sub_y1,
                    text: "Rust".into(),
                    scale: sub_size,
                    color: [0.05, 0.05, 0.1, text_a],
                    centered: false,
                });
            }
        }

        elements.push(MenuElement::Text {
            x: sub_x,
            y: sub_y2,
            text: "Edition".into(),
            scale: sub_size,
            color: text_dim,
            centered: false,
        });
        cy += title_size + 18.0 * s;

        elements.push(MenuElement::Rect {
            x: btn_x,
            y: cy,
            w: content_w,
            h: 1.0,
            corner_radius: 0.5,
            color: border,
        });
        cy += 1.0 + 16.0 * s;

        // TODO: fold the bottom icon rows into the focus ring too (arrow-key
        // nearest-rect navigation is still deferred).
        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        for (i, def) in buttons.iter().enumerate() {
            let by = cy + i as f32 * (btn_h + btn_gap);
            let rect = [btn_x, by, content_w, btn_h];
            let focused = ctx.focused(def.enabled);
            let hovered = def.enabled && common::hit_test(cursor, rect);
            any_hovered |= hovered;
            // Keyboard focus shows the same highlight as hover (vanilla).
            let active = hovered || focused;

            elements.push(MenuElement::Rect {
                x: rect[0],
                y: rect[1],
                w: rect[2],
                h: rect[3],
                corner_radius: btn_r,
                color: if active { glass_hover } else { glass },
            });

            let bar_margin = btn_h * 0.18;
            elements.push(MenuElement::Rect {
                x: rect[0],
                y: rect[1] + bar_margin,
                w: accent_w,
                h: rect[3] - bar_margin * 2.0,
                corner_radius: accent_w * 0.5,
                color: [
                    accent[0],
                    accent[1],
                    accent[2],
                    if !def.enabled {
                        0.04
                    } else if active {
                        0.9
                    } else {
                        0.12
                    },
                ],
            });

            elements.push(MenuElement::Text {
                x: rect[0] + 18.0 * s,
                y: rect[1] + (rect[3] - font_size) / 2.0,
                text: def.label.into(),
                scale: font_size,
                color: if !def.enabled {
                    text_disabled
                } else if active {
                    text_bright
                } else {
                    text_col
                },
                centered: false,
            });

            if (clicked && hovered) || (focused && ctx.activate) {
                any_clicked = true;
                if def.id == 2 {
                    action = MenuAction::Quit;
                } else {
                    match def.id {
                        0 => {}
                        1 => {
                            self.set_screen(Screen::ServerList);
                            self.scroll_offset = 0.0;
                            self.selected_server = None;
                        }
                        _ => {}
                    }
                }
            }
        }

        let new_row_y = panel_y + panel_h - panel_pad - icon_size;
        let icon_area_y = new_row_y - icon_size - icon_row_gap;
        let icon_r = 7.0 * s;
        let icon_scale = 13.0 * s;
        let drop_style = DropdownStyle::new(gs);

        let icon_btn =
            |elements: &mut Vec<MenuElement>, bx: f32, by: f32, icon: char, enabled: bool| {
                let hovered = common::hit_test(cursor, [bx, by, icon_size, icon_size]);
                if hovered && enabled {
                    elements.push(MenuElement::Rect {
                        x: bx,
                        y: by,
                        w: icon_size,
                        h: icon_size,
                        corner_radius: icon_r,
                        color: glass_hover,
                    });
                }
                elements.push(MenuElement::Icon {
                    x: bx + icon_size / 2.0,
                    y: by + icon_size / 2.0,
                    icon,
                    scale: icon_scale,
                    color: if !enabled {
                        text_disabled
                    } else if hovered {
                        text_bright
                    } else {
                        text_dim
                    },
                });
                hovered
            };

        let bottom_icons: [(f32, char, bool); 4] = [
            (btn_x, ICON_USER, false),
            (btn_x + icon_size + icon_gap, ICON_LINK, true),
            (btn_x + content_w - icon_size, ICON_GEAR, true),
            (
                btn_x + content_w - icon_size * 2.0 - icon_gap,
                ICON_PAINTBRUSH,
                true,
            ),
        ];

        for &(bx, icon, enabled) in &bottom_icons {
            let hovered = icon_btn(&mut elements, bx, icon_area_y, icon, enabled);
            any_hovered |= enabled && hovered;

            if enabled && clicked && hovered {
                any_clicked = true;
                match icon {
                    ICON_LINK => self.toggle_links(),
                    ICON_GEAR => {
                        self.open_options();
                    }
                    ICON_PAINTBRUSH => self.toggle_theme(),
                    _ => {}
                }
            }
        }

        // Second row mirrors vanilla 26.2: friends, language and accessibility,
        // centered. Friends needs a signed-in account, so it's disabled offline.
        let friends_enabled = self.access_token.is_some();
        let friends_tip = if friends_enabled {
            "Friends"
        } else {
            "Sign in to use friends"
        };
        let new_row_w = icon_size * 3.0 + icon_gap * 2.0;
        let new_x0 = btn_x + (content_w - new_row_w) / 2.0;
        let new_icons: [(f32, char, bool, &str); 3] = [
            (new_x0, ICON_USERS, friends_enabled, friends_tip),
            (
                new_x0 + icon_size + icon_gap,
                ICON_LANGUAGE,
                false,
                "Language",
            ),
            (
                new_x0 + (icon_size + icon_gap) * 2.0,
                ICON_UNIVERSAL_ACCESS,
                true,
                "Accessibility Settings",
            ),
        ];

        for &(bx, icon, enabled, tip) in &new_icons {
            let hovered = icon_btn(&mut elements, bx, new_row_y, icon, enabled);
            any_hovered |= enabled && hovered;

            if hovered {
                common::push_tooltip(&mut elements, cursor, screen_w, screen_h, gs, tip);
            }

            if enabled && clicked && hovered {
                any_clicked = true;
                match icon {
                    ICON_USERS => self.open_friends(),
                    ICON_LANGUAGE => {
                        self.settings_back = Screen::Main;
                        self.set_screen(Screen::OptionsLanguage);
                    }
                    ICON_UNIVERSAL_ACCESS => {
                        self.settings_back = Screen::Main;
                        self.set_screen(Screen::OptionsAccessibility);
                    }
                    _ => {}
                }
            }
        }

        // Links open rightwards off their icon; the theme picker is at the far
        // end of the row, so it right-aligns to its own icon instead.
        let links_x = btn_x + icon_size + icon_gap;
        self.push_links_dropdown(
            &mut elements,
            &mut any_hovered,
            cursor,
            clicked,
            &drop_style,
            [links_x, icon_area_y, icon_size, icon_size],
            links_x,
            icon_area_y - 2.0 * s,
            140.0 * s,
        );

        let theme_x = btn_x + content_w - icon_size * 2.0 - icon_gap;
        let theme_w = 120.0 * s;
        self.push_theme_dropdown(
            &mut elements,
            &mut any_hovered,
            cursor,
            clicked,
            &drop_style,
            [theme_x, icon_area_y, icon_size, icon_size],
            theme_x + icon_size - theme_w,
            icon_area_y - 2.0 * s,
            theme_w,
        );

        let footer_size = 8.0 * s;
        let footer_pad = 8.0 * s;
        let footer_y = screen_h - footer_pad - footer_size;
        let footer_col = [0.4, 0.45, 0.6, 0.2];
        elements.push(MenuElement::Text {
            x: footer_pad,
            y: footer_y,
            text: self.version.clone(),
            scale: footer_size,
            color: footer_col,
            centered: false,
        });
        let copy = "Pomme early dev";
        let copy_w = text_width_fn(copy, footer_size);
        elements.push(MenuElement::Text {
            x: screen_w - footer_pad - copy_w,
            y: footer_y,
            text: copy.into(),
            scale: footer_size,
            color: footer_col,
            centered: false,
        });

        if let Some(theme_action) = self.drive_theme_transition(&mut elements, screen_w, screen_h) {
            action = theme_action;
        }

        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            blur: 1.0,
            clicked_button: any_clicked,
        }
    }
}
