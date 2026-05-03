use super::*;

impl MainMenu {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_main(
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
        let border: [f32; 4] = [1.0, 1.0, 1.0, 0.05];

        struct BtnDef {
            label: &'static str,
            id: u8,
        }
        let buttons = [
            BtnDef {
                label: "Singleplayer",
                id: 0,
            },
            BtnDef {
                label: "Multiplayer",
                id: 1,
            },
            BtnDef {
                label: "Quit Game",
                id: 2,
            },
        ];

        let s = (screen_h / 400.0).max(1.0);
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

        let header_h = accent_bar_h + 14.0 * s + title_size + 4.0 * s + sub_size + 18.0 * s;
        let btns_total = buttons.len() as f32 * (btn_h + btn_gap) - btn_gap;
        let panel_h =
            (panel_pad + header_h + 1.0 + 16.0 * s + btns_total + 16.0 * s + icon_size + panel_pad)
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
            color: [0.94, 0.96, 0.99, 0.95],
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

        for (i, def) in buttons.iter().enumerate() {
            let by = cy + i as f32 * (btn_h + btn_gap);
            let rect = [btn_x, by, content_w, btn_h];
            let hovered = common::hit_test(cursor, rect);
            any_hovered |= hovered;

            elements.push(MenuElement::Rect {
                x: rect[0],
                y: rect[1],
                w: rect[2],
                h: rect[3],
                corner_radius: btn_r,
                color: if hovered { glass_hover } else { glass },
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
                    if hovered { 0.9 } else { 0.12 },
                ],
            });

            elements.push(MenuElement::Text {
                x: rect[0] + 18.0 * s,
                y: rect[1] + (rect[3] - font_size) / 2.0,
                text: def.label.into(),
                scale: font_size,
                color: if hovered { text_bright } else { text_col },
                centered: false,
            });

            if clicked && hovered {
                any_clicked = true;
                if def.id == 2 {
                    action = MenuAction::Quit;
                } else if self.auth_account.is_some() {
                    match def.id {
                        0 => {}
                        1 => {
                            self.screen = Screen::ServerList;
                            self.scroll_offset = 0.0;
                            self.selected_server = None;
                        }
                        _ => {}
                    }
                } else {
                    let pending = match def.id {
                        0 => AuthPending::Singleplayer,
                        _ => AuthPending::Multiplayer,
                    };
                    self.screen = Screen::AuthPrompt { pending };
                }
            }
        }

        let icon_area_y = panel_y + panel_h - panel_pad - icon_size;
        let icon_r = 7.0 * s;
        let icon_scale = 13.0 * s;
        let drop_style = DropdownStyle::new(gs);

        let bottom_icons: [(f32, char); 4] = [
            (btn_x, ICON_USER),
            (btn_x + icon_size + icon_gap, ICON_LINK),
            (btn_x + content_w - icon_size, ICON_GEAR),
            (
                btn_x + content_w - icon_size * 2.0 - icon_gap,
                ICON_PAINTBRUSH,
            ),
        ];

        for &(bx, icon) in &bottom_icons {
            let rect = [bx, icon_area_y, icon_size, icon_size];
            let hovered = common::hit_test(cursor, rect);
            any_hovered |= hovered;

            elements.push(MenuElement::Rect {
                x: bx,
                y: icon_area_y,
                w: icon_size,
                h: icon_size,
                corner_radius: icon_r,
                color: if hovered {
                    glass_hover
                } else {
                    [0.0, 0.0, 0.0, 0.0]
                },
            });
            elements.push(MenuElement::Icon {
                x: bx + icon_size / 2.0,
                y: icon_area_y + icon_size / 2.0,
                icon,
                scale: icon_scale,
                color: if hovered { text_bright } else { text_dim },
            });

            if clicked && hovered {
                any_clicked = true;
                match icon {
                    ICON_USER if self.auth_account.is_none() => {
                        self.screen = Screen::AuthPrompt {
                            pending: AuthPending::None,
                        };
                    }
                    ICON_LINK => {
                        self.links_open = !self.links_open;
                        if self.links_open {
                            self.theme_open = false;
                        }
                    }
                    ICON_GEAR => {
                        self.open_options();
                    }
                    ICON_PAINTBRUSH => {
                        self.theme_open = !self.theme_open;
                        if self.theme_open {
                            self.links_open = false;
                        }
                    }
                    _ => {}
                }
            }
        }

        if self.links_open {
            let anchor_x = btn_x + icon_size + icon_gap;
            let drop_w = 140.0 * s;
            let drop_x = anchor_x;
            let drop_y = icon_area_y - 2.0 * s;
            let links: [(&str, char, &str); 3] = [
                ("Website", ICON_GLOBE, "https://website.com"),
                ("Discord", ICON_COMMENT, "https://discord.gg/ucBA55bHPR"),
                ("GitHub", ICON_CODE, "https://github.com"),
            ];
            let total_h = links.len() as f32 * drop_style.item_h;
            let drop_y_top = drop_y - total_h;
            drop_style.draw_background(&mut elements, drop_x, drop_y_top, drop_w, total_h);
            let mut clicked_inside = false;
            for (i, (label, icon, url)) in links.iter().enumerate() {
                let item = drop_style.draw_item(
                    &mut elements,
                    &mut any_hovered,
                    cursor,
                    drop_x,
                    drop_y_top,
                    drop_w,
                    i,
                    links.len(),
                    label,
                    Some((*icon, [0.6, 0.7, 0.85, 0.8])),
                    text_bright,
                    text_col,
                );
                if item {
                    clicked_inside = true;
                }
                if clicked && item {
                    let _ = open::that(url);
                    self.links_open = false;
                }
            }
            if dismiss_dropdown(
                cursor,
                clicked,
                clicked_inside,
                [drop_x, drop_y_top, drop_w, total_h],
                [anchor_x, icon_area_y, icon_size, icon_size],
            ) {
                self.links_open = false;
            }
        }

        if self.theme_open {
            let anchor_x = btn_x + content_w - icon_size * 2.0 - icon_gap;
            let drop_w = 120.0 * s;
            let drop_x = anchor_x + icon_size - drop_w;
            let drop_y = icon_area_y - 2.0 * s;
            let themes: [(&str, PanoramaTheme); 2] = [
                ("Pomme", PanoramaTheme::Pomme),
                ("Default", PanoramaTheme::Default),
            ];
            let total_h = themes.len() as f32 * drop_style.item_h;
            let drop_y_top = drop_y - total_h;
            drop_style.draw_background(&mut elements, drop_x, drop_y_top, drop_w, total_h);
            let mut clicked_inside = false;
            for (i, (label, theme_val)) in themes.iter().enumerate() {
                let selected = self.theme == *theme_val;
                let check = if selected {
                    Some((ICON_CHECK, [0.39, 0.71, 1.0, 0.9]))
                } else {
                    None
                };
                let text_c = if selected {
                    [0.39, 0.71, 1.0, 0.9]
                } else {
                    text_col
                };
                let item = drop_style.draw_item(
                    &mut elements,
                    &mut any_hovered,
                    cursor,
                    drop_x,
                    drop_y_top,
                    drop_w,
                    i,
                    themes.len(),
                    label,
                    check,
                    text_bright,
                    text_c,
                );
                if item {
                    clicked_inside = true;
                }
                if clicked && item && !selected {
                    self.transition = Some(ThemeTransition {
                        start: Instant::now(),
                        target: *theme_val,
                        reloaded: false,
                        open_start: None,
                    });
                    self.theme_open = false;
                } else if clicked && item {
                    self.theme_open = false;
                }
            }
            if dismiss_dropdown(
                cursor,
                clicked,
                clicked_inside,
                [drop_x, drop_y_top, drop_w, total_h],
                [anchor_x, icon_area_y, icon_size, icon_size],
            ) {
                self.theme_open = false;
            }
        }

        let footer_size = 8.0 * s;
        let footer_pad = 8.0 * s;
        let footer_y = screen_h - footer_pad - footer_size;
        let footer_col = [0.4, 0.45, 0.6, 0.2];
        elements.push(MenuElement::Text {
            x: footer_pad,
            y: footer_y,
            text: "1.21.11".into(),
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

        if let Some(ref mut tr) = self.transition {
            let close_t = (tr.start.elapsed().as_secs_f32() / CLOSE_DURATION).min(1.0);
            if close_t >= 1.0 && !tr.reloaded {
                tr.reloaded = true;
                self.theme = tr.target;
                action = MenuAction::ChangeTheme(tr.target);
            }
            let open_t = tr
                .open_start
                .map(|s| (s.elapsed().as_secs_f32() / OPEN_DURATION).min(1.0))
                .unwrap_or(0.0);
            emit_transition_strips(&mut elements, screen_w, screen_h, close_t, open_t);
            if open_t >= 1.0 {
                self.transition = None;
            }
        }

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            blur: 1.0,
            clicked_button: any_clicked,
        }
    }
}
