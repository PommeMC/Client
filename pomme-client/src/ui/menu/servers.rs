use super::*;

impl MainMenu {
    pub(super) fn build_server_list(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let header_h = HEADER_H * gs;
        let sep_h = SEP_H * gs;
        let entry_h = ENTRY_H * gs;
        let row_w = ROW_W * gs;
        let gap = BTN_GAP * gs;
        let fs = common::FONT_SIZE * gs;
        let btn_h = common::BTN_H * gs;
        let top_w = TOP_BTN_W * gs;
        let bot_w = BOT_BTN_W * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        let footer_h = 60.0 * gs;
        let list_top = header_h;
        let list_bottom = screen_h - footer_h;
        let list_h = list_bottom - list_top;

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;

        if input.f5 {
            self.refresh_servers();
        }
        if input.escape {
            self.set_screen(Screen::Main);
            return MainMenuResult {
                elements: Vec::new(),
                action: MenuAction::None,
                cursor_pointer: false,
                blur: 1.0,
                clicked_button: false,
            };
        }

        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: (header_h - fs) / 2.0,
            text: "Play Multiplayer".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });

        elements.push(MenuElement::TiledImage {
            x: 0.0,
            y: list_top,
            w: screen_w,
            h: list_h,
            sprite: SpriteId::MenuBackground,
            tile_size: 32.0 * gs,
            tint: [0.25, 0.25, 0.25, 1.0],
        });
        elements.push(MenuElement::Rect {
            x: 0.0,
            y: list_top,
            w: screen_w,
            h: list_h,
            corner_radius: 0.0,
            color: [0.0, 0.0, 0.0, 0.3],
        });

        push_separator(&mut elements, 0.0, list_top - sep_h, screen_w, sep_h);
        push_separator(&mut elements, 0.0, list_bottom, screen_w, sep_h);

        let list_pad = 4.0 * gs;
        let entries_h = self.server_list.servers.len() as f32 * entry_h;
        let total_content = list_pad + entries_h + list_pad + fs * 3.0;
        let max_scroll = (total_content - list_h).max(0.0);
        if common::hit_test(cursor, [0.0, list_top, screen_w, list_h]) {
            self.scroll_offset -= input.scroll_delta * 20.0 * gs;
        }
        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);

        let list_cx = screen_w / 2.0;
        let list_left = list_cx - row_w / 2.0;
        let ping_results = self.ping_results.read().clone();

        // Persist pinged protocols for joins before a ping completes.
        let mut protocol_changed = false;
        for server in &mut self.server_list.servers {
            if let Some(PingState::Success { protocol, .. }) = ping_results.get(&server.address)
                && server.protocol != Some(*protocol)
            {
                server.protocol = Some(*protocol);
                protocol_changed = true;
            }
        }
        if protocol_changed {
            self.server_list.save();
        }

        elements.push(MenuElement::ScissorPush {
            x: 0.0,
            y: list_top,
            w: screen_w,
            h: list_h,
        });

        let mut pending_swap: Option<(usize, usize)> = None;
        // Ping each entry the first frame its row is visible (absent = INITIAL),
        // deferred past the loop to keep the `servers` borrow off `self.rt`.
        let mut to_ping: Vec<ServerEntry> = Vec::new();
        for (i, server) in self.server_list.servers.iter().enumerate() {
            let ey = list_top + list_pad + i as f32 * entry_h - self.scroll_offset;
            if ey + entry_h < list_top || ey > list_bottom {
                continue;
            }
            if !ping_results.contains_key(&server.address) {
                to_ping.push(server.clone());
            }

            let selected = self.selected_server == Some(i);
            let rect = [list_left, ey, row_w, entry_h];
            let hovered =
                common::hit_test(cursor, rect) && cursor.1 >= list_top && cursor.1 <= list_bottom;
            any_hovered |= hovered;

            if selected || hovered {
                elements.push(MenuElement::Rect {
                    x: rect[0],
                    y: rect[1],
                    w: rect[2],
                    h: rect[3],
                    corner_radius: 0.0,
                    color: if selected {
                        [1.0, 1.0, 1.0, 0.12]
                    } else {
                        [1.0, 1.0, 1.0, 0.04]
                    },
                });
            }
            if selected {
                push_outline(&mut elements, rect[0], rect[1], rect[2], rect[3], gs);
            }

            let icon_size = 32.0 * gs;
            let icon_pad = SERVER_ENTRY_PAD * gs;
            let icon_x = rect[0] + icon_pad;
            let icon_y = rect[1] + icon_pad;
            let text_x = icon_x + icon_size + 3.0 * gs;
            let name_y = icon_y + 1.0 * gs;

            elements.push(MenuElement::Favicon {
                x: icon_x,
                y: icon_y,
                size: icon_size,
                address: server.address.clone(),
            });

            let rel_x = cursor.0 - icon_x;
            let rel_y = cursor.1 - icon_y;
            let on_icon =
                hovered && rel_x >= 0.0 && rel_x < icon_size && rel_y >= 0.0 && rel_y < icon_size;
            let right_half = rel_x >= icon_size / 2.0;
            let top_left = !right_half && rel_y < icon_size / 2.0;
            let bottom_left = !right_half && rel_y >= icon_size / 2.0;

            if hovered {
                elements.push(MenuElement::Rect {
                    x: icon_x,
                    y: icon_y,
                    w: icon_size,
                    h: icon_size,
                    corner_radius: 0.0,
                    color: [0.274, 0.274, 0.274, 0.63],
                });

                if on_icon {
                    let mut push_icon = |sprite| {
                        elements.push(MenuElement::Image {
                            x: icon_x,
                            y: icon_y,
                            w: icon_size,
                            h: icon_size,
                            sprite,
                            tint: WHITE,
                        });
                    };
                    push_icon(if right_half {
                        SpriteId::ServerJoinHighlighted
                    } else {
                        SpriteId::ServerJoin
                    });
                    if i > 0 {
                        push_icon(if top_left {
                            SpriteId::ServerMoveUpHighlighted
                        } else {
                            SpriteId::ServerMoveUp
                        });
                    }
                    if i < self.server_list.servers.len() - 1 {
                        push_icon(if bottom_left {
                            SpriteId::ServerMoveDownHighlighted
                        } else {
                            SpriteId::ServerMoveDown
                        });
                    }
                }
            }
            elements.push(MenuElement::Text {
                x: text_x,
                y: name_y,
                text: server.name.clone(),
                scale: fs,
                color: WHITE,
                centered: false,
            });

            let motd_y = icon_y + 12.0 * gs;
            push_server_status(
                &mut elements,
                &ping_results,
                &server.address,
                text_x,
                motd_y,
                &rect,
                fs,
                gs,
                cursor,
                screen_w,
                screen_h,
                text_width_fn,
            );

            if clicked && hovered {
                if on_icon && right_half {
                    action = MenuAction::Connect {
                        server: server.address.clone(),
                        username: self.username.clone(),
                        protocol: join_protocol(&ping_results, &server.address, server.protocol),
                    };
                } else if on_icon && top_left && i > 0 {
                    pending_swap = Some((i, i - 1));
                } else if on_icon && bottom_left && i < self.server_list.servers.len() - 1 {
                    pending_swap = Some((i, i + 1));
                } else {
                    let now = Instant::now();
                    let is_double = self.last_click_index == Some(i)
                        && now.duration_since(self.last_click_time).as_millis() < DOUBLE_CLICK_MS;

                    if is_double {
                        action = MenuAction::Connect {
                            server: server.address.clone(),
                            username: self.username.clone(),
                            protocol: join_protocol(
                                &ping_results,
                                &server.address,
                                server.protocol,
                            ),
                        };
                    } else {
                        self.selected_server = Some(i);
                        self.last_click_time = now;
                        self.last_click_index = Some(i);
                    }
                }
            }
        }

        if !to_ping.is_empty() {
            ping_all_servers(
                &self.rt,
                &to_ping,
                &self.ping_results,
                &self.ping_generation,
            );
        }

        if let Some((a, b)) = pending_swap {
            self.server_list.swap(a, b);
            self.selected_server = Some(b);
        }

        if self.server_list.servers.is_empty() {
            elements.push(MenuElement::Text {
                x: screen_w / 2.0,
                y: list_top + 40.0 * gs,
                text: "No servers added".into(),
                scale: fs,
                color: COL_DIM,
                centered: true,
            });
        }

        let lan_y = list_top + list_pad + entries_h + list_pad - self.scroll_offset;
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: lan_y,
            text: "Scanning for games on your local network".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        let loading_dots = match (millis / 300) % 4 {
            0 => "O o o",
            1 => "o O o",
            2 => "o o O",
            _ => "o O o",
        };
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: lan_y + fs * 1.5,
            text: loading_dots.into(),
            scale: fs,
            color: COL_DIM,
            centered: true,
        });

        elements.push(MenuElement::ScissorPop);

        if max_scroll > 0.0 {
            let track_w = 6.0 * gs;
            let track_x = screen_w - track_w - 2.0 * gs;
            let thumb_frac = list_h / total_content;
            let thumb_h = (list_h * thumb_frac).max(8.0 * gs);
            let thumb_y = list_top + (self.scroll_offset / max_scroll) * (list_h - thumb_h);
            elements.push(MenuElement::NineSlice {
                x: track_x,
                y: list_top,
                w: track_w,
                h: list_h,
                sprite: SpriteId::ScrollerBackground,
                border: gs,
                tint: WHITE,
            });
            elements.push(MenuElement::NineSlice {
                x: track_x,
                y: thumb_y,
                w: track_w,
                h: thumb_h,
                sprite: SpriteId::Scroller,
                border: gs,
                tint: WHITE,
            });
        }

        let has_sel = self.selected_server.is_some();
        let buttons_h = btn_h * 2.0 + gap;
        let footer_pad = (footer_h - buttons_h) / 2.0;
        let footer_y = list_bottom + footer_pad;

        let row1_w = top_w * 3.0 + gap * 2.0;
        let row1_x = (screen_w - row1_w) / 2.0;

        // Prefill geometry for the edit forms opened from these buttons.
        let form_inner = FORM_W * gs - 8.0 * gs;
        let wf = |s: &str| text_width_fn(s, fs);

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);

        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row1_x,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Join Server",
            has_sel,
        ) && let Some(idx) = self.selected_server
            && let Some(server) = self.server_list.servers.get(idx)
        {
            action = MenuAction::Connect {
                server: server.address.clone(),
                username: self.username.clone(),
                protocol: join_protocol(&ping_results, &server.address, server.protocol),
            };
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row1_x + top_w + gap,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Direct Connect",
            true,
        ) {
            self.edit_address
                .set_value(&self.last_mp_ip, form_inner, &wf);
            self.set_screen(Screen::DirectConnect);
            self.focused_field = Some(0);
            self.edit_address.set_focused(true);
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row1_x + (top_w + gap) * 2.0,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Add Server",
            true,
        ) {
            self.edit_name.clear();
            self.edit_address.clear();
            self.set_screen(Screen::AddServer);
            self.focused_field = Some(0);
            self.edit_name.set_focused(true);
        }

        let row2_y = footer_y + btn_h + gap;
        let row2_w = bot_w * 4.0 + gap * 3.0;
        let row2_x = (screen_w - row2_w) / 2.0;

        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row2_x,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Edit",
            has_sel,
        ) && let Some(idx) = self.selected_server
            && let Some(server) = self.server_list.servers.get(idx)
        {
            self.edit_name.set_value(&server.name, form_inner, &wf);
            self.edit_address
                .set_value(&server.address, form_inner, &wf);
            self.set_screen(Screen::EditServer(idx));
            self.focused_field = Some(0);
            self.edit_name.set_focused(true);
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row2_x + bot_w + gap,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Delete",
            has_sel,
        ) && let Some(idx) = self.selected_server
        {
            self.set_screen(Screen::ConfirmDelete(idx));
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row2_x + (bot_w + gap) * 2.0,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Refresh",
            true,
        ) {
            self.refresh_servers();
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            row2_x + (bot_w + gap) * 3.0,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Back",
            true,
        ) {
            self.set_screen(Screen::Main);
        }

        self.finish_focus(&ctx);

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: (input.clicked && any_hovered) || ctx.fired,
        }
    }

    pub(super) fn build_confirm_delete(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Screen::ConfirmDelete(idx) = self.screen else {
            return empty_result(2.0);
        };

        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.set_screen(Screen::ServerList);
            return empty_result(2.0);
        }

        let warning = self
            .server_list
            .servers
            .get(idx)
            .map(|s| format!("'{}' will be lost forever! (A long time!)", s.name))
            .unwrap_or_default();

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let cy = screen_h * 0.3;
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: cy,
            text: "Are you sure?".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        elements.push(MenuElement::Text {
            x: screen_w / 2.0,
            y: cy + fs + 12.0 * gs,
            text: warning,
            scale: fs,
            color: COL_DIM,
            centered: true,
        });

        let btn_x = (screen_w - form_w) / 2.0;
        let btn_y = cy + fs * 2.0 + 44.0 * gs;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            btn_x,
            btn_y,
            form_w,
            btn_h,
            gs,
            "Delete",
            true,
        ) {
            self.server_list.remove(idx);
            self.selected_server = None;
            self.set_screen(Screen::ServerList);
        }
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            cursor,
            clicked,
            btn_x,
            btn_y + btn_h + gap,
            form_w,
            btn_h,
            gs,
            "Cancel",
            true,
        ) {
            self.set_screen(Screen::ServerList);
        }
        self.finish_focus(&ctx);

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: (input.clicked && any_hovered) || ctx.fired,
        }
    }

    pub(super) fn build_direct_connect(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let field_h = FIELD_H * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.set_screen(Screen::ServerList);
            return empty_result(2.0);
        }

        self.cycle_fields(input, 1);

        let mut elements = Vec::new();
        let mut action = MenuAction::None;
        let mut any_hovered = false;

        let cx = screen_w / 2.0;
        let form_x = cx - form_w / 2.0;
        let mut y = 20.0 * gs;

        elements.push(MenuElement::Text {
            x: cx,
            y,
            text: "Direct Connect".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        y += fs + 40.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x,
            y,
            text: "Server Address".into(),
            scale: fs,
            color: COL_DIM,
            centered: false,
        });
        y += fs + 4.0 * gs;

        self.text_field(
            &mut elements,
            TextTarget::EditAddress,
            0,
            input,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        y += field_h + 28.0 * gs;

        let address = self.edit_address.value().to_string();
        let valid = is_valid_address(&address);
        let enter_submit = input.enter && valid;

        if (push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            form_x,
            y,
            form_w,
            btn_h,
            gs,
            "Join Server",
            valid,
        ) && clicked)
            || enter_submit
        {
            self.last_mp_ip = address.clone();
            let persisted = self
                .server_list
                .servers
                .iter()
                .find(|s| s.address == address)
                .and_then(|s| s.protocol);
            action = MenuAction::Connect {
                server: address.clone(),
                username: self.username.clone(),
                protocol: join_protocol(&self.ping_results.read(), &address, persisted),
            };
        }
        y += btn_h + gap;
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            form_x,
            y,
            form_w,
            btn_h,
            gs,
            "Cancel",
            true,
        ) && clicked
        {
            self.set_screen(Screen::ServerList);
        }

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: input.clicked && any_hovered,
        }
    }

    pub(super) fn build_edit_server(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let form_w = FORM_W * gs;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let field_h = FIELD_H * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        if input.escape {
            self.set_screen(Screen::ServerList);
            return empty_result(2.0);
        }

        self.cycle_fields(input, 2);

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let cx = screen_w / 2.0;
        let form_x = cx - form_w / 2.0;
        let mut y = 17.0 * gs;

        elements.push(MenuElement::Text {
            x: cx,
            y,
            text: "Edit Server Info".into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
        y += fs + 20.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x,
            y,
            text: "Server Name".into(),
            scale: fs,
            color: COL_DIM,
            centered: false,
        });
        y += fs + 4.0 * gs;

        self.text_field(
            &mut elements,
            TextTarget::EditName,
            0,
            input,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        y += field_h + 12.0 * gs;

        elements.push(MenuElement::Text {
            x: form_x,
            y,
            text: "Server Address".into(),
            scale: fs,
            color: COL_DIM,
            centered: false,
        });
        y += fs + 4.0 * gs;

        self.text_field(
            &mut elements,
            TextTarget::EditAddress,
            1,
            input,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            text_width_fn,
        );
        y += field_h + 28.0 * gs;

        let name_val = self.edit_name.value().to_string();
        let address = self.edit_address.value().to_string();
        let valid = is_valid_address(&address);
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            form_x,
            y,
            form_w,
            btn_h,
            gs,
            "Done",
            valid,
        ) && clicked
        {
            let name = if name_val.is_empty() {
                "Minecraft Server".to_string()
            } else {
                name_val.clone()
            };
            let mut entry = ServerEntry {
                name,
                address: address.clone(),
                protocol: None,
                extra: Default::default(),
            };
            if let Screen::EditServer(idx) = self.screen {
                if let Some(old) = self.server_list.servers.get(idx) {
                    entry.extra = old.extra.clone();
                    // The pinged protocol stays valid while the address does.
                    if old.address == entry.address {
                        entry.protocol = old.protocol;
                    }
                }
                self.server_list.update(idx, entry);
            } else {
                self.server_list.add(entry);
            }
            // Absent from the results map, so it pings on draw back on the list.
            self.set_screen(Screen::ServerList);
        }
        y += btn_h + gap;
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            form_x,
            y,
            form_w,
            btn_h,
            gs,
            "Cancel",
            true,
        ) && clicked
        {
            self.set_screen(Screen::ServerList);
        }

        push_bottom_text(
            &mut elements,
            screen_w,
            screen_h,
            gs,
            &self.version,
            text_width_fn,
        );
        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: input.clicked && any_hovered,
        }
    }

    fn field_mut(&mut self, target: TextTarget) -> &mut TextFieldState {
        match target {
            TextTarget::EditName => &mut self.edit_name,
            TextTarget::EditAddress => &mut self.edit_address,
            TextTarget::PackSearch => &mut self.pack_search,
            TextTarget::AddFriend => &mut self.add_friend_name,
        }
    }

    fn field_ref(&self, target: TextTarget) -> &TextFieldState {
        match target {
            TextTarget::EditName => &self.edit_name,
            TextTarget::EditAddress => &self.edit_address,
            TextTarget::PackSearch => &self.pack_search,
            TextTarget::AddFriend => &self.add_friend_name,
        }
    }

    /// The backing text field for a form field index on the current screen.
    fn focus_target(&self, field_idx: u8) -> Option<TextTarget> {
        match (&self.screen, field_idx) {
            (Screen::AddServer | Screen::EditServer(_), 0) => Some(TextTarget::EditName),
            (Screen::AddServer | Screen::EditServer(_), 1) => Some(TextTarget::EditAddress),
            (Screen::DirectConnect, 0) => Some(TextTarget::EditAddress),
            (Screen::OptionsResourcePacks, 0) => Some(TextTarget::PackSearch),
            (Screen::Friends, 0) => Some(TextTarget::AddFriend),
            _ => None,
        }
    }

    /// Tab / Shift+Tab cycle keyboard focus between a form's `field_count` text
    /// fields (vanilla `changeFocus`). Sub-1000 forms don't unify buttons into
    /// this ring yet.
    // TODO: fold form buttons into the focus ring so Tab moves field->button
    // like vanilla TabNavigation.
    pub(super) fn cycle_fields(&mut self, input: &MenuInput, field_count: u8) {
        if !input.tab {
            return;
        }
        self.unfocus_current_field();
        let next = helpers::step_ring(
            self.focused_field.map(usize::from),
            field_count as usize,
            input.shift,
        ) as u8;
        self.focused_field = Some(next);
        self.last_field_click = None;
        if let Some(t) = self.focus_target(next) {
            self.field_mut(t).set_focused(true);
        }
    }

    fn unfocus_current_field(&mut self) {
        if let Some(t) = self.focused_field.and_then(|i| self.focus_target(i)) {
            self.field_mut(t).set_focused(false);
        }
    }

    /// Handle the pointer, feed the keyboard event stream, then render one text
    /// field. Input and rendering share the same `inner_w`, so the horizontal
    /// scroll (`display_pos`) stays consistent.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn text_field(
        &mut self,
        elements: &mut Vec<MenuElement>,
        target: TextTarget,
        field_idx: u8,
        input: &MenuInput,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fs: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) {
        let pad = 4.0 * gs;
        let inner_w = w - pad * 2.0;
        let text_x = x + pad;
        let wf = |s: &str| text_width_fn(s, fs);
        let hit = common::hit_test(input.cursor, [x, y, w, h]);
        // Vanilla `findClickedPositionInText` floors the mouse x first.
        let rel_x = input.cursor.0.floor() - text_x;

        if input.clicked && hit {
            let now = Instant::now();
            let is_double = self.last_field_click == Some(field_idx)
                && now.duration_since(self.last_field_click_time).as_millis() < DOUBLE_CLICK_MS;
            let was_focused = self.focused_field == Some(field_idx);
            if !was_focused {
                self.unfocus_current_field();
            }
            self.focused_field = Some(field_idx);
            self.last_field_click = Some(field_idx);
            self.last_field_click_time = now;
            let f = self.field_mut(target);
            if !was_focused {
                f.set_focused(true);
            }
            let pos = f.pos_from_click(rel_x, inner_w, &wf);
            if is_double {
                // Vanilla double-click selects the word (not the whole field).
                f.select_word_at(pos, inner_w, &wf);
            } else {
                f.on_click(pos, input.shift, inner_w, &wf);
            }
        } else if input.clicked && self.focused_field == Some(field_idx) {
            // Vanilla clears widget focus on a click that lands elsewhere.
            self.unfocus_current_field();
            self.focused_field = None;
        } else if self.focused_field == Some(field_idx) && input.mouse_held && !input.clicked {
            let f = self.field_mut(target);
            let pos = f.pos_from_click(rel_x, inner_w, &wf);
            f.on_drag(pos, inner_w, &wf);
        }

        if self.focused_field == Some(field_idx) {
            self.apply_text_events(target, field_idx, input, inner_w, &wf);
        }

        let focused = self.focused_field == Some(field_idx);
        push_text_field(
            elements,
            x,
            y,
            w,
            h,
            fs,
            gs,
            self.field_ref(target),
            focused,
            text_width_fn,
        );
    }

    /// Feed the frame's key/char events to the focused field, snapshotting for
    /// the pomme-only Ctrl+Z undo stack.
    fn apply_text_events(
        &mut self,
        target: TextTarget,
        field_idx: u8,
        input: &MenuInput,
        inner_w: f32,
        wf: &dyn Fn(&str) -> f32,
    ) {
        // Ctrl+Z: pomme extra (vanilla EditBox has no undo). Intercept before
        // the field sees the stream.
        let undo = input.events.iter().any(|e| {
            matches!(
                e,
                TextInputEvent::Key { code: KeyCode::KeyZ, mods }
                    if mods.edit_shortcut() && !mods.shift
            )
        });
        if undo {
            if let Some(pos) = self
                .field_undo_stack
                .iter()
                .rposition(|(f, _)| *f == field_idx)
            {
                let (_, prev) = self.field_undo_stack.remove(pos);
                self.field_mut(target).set_value(&prev, inner_w, wf);
            }
            return;
        }
        if input.events.is_empty() {
            return;
        }
        let before = self.field_mut(target).value().to_string();
        {
            let mut clipboard = SystemClipboard;
            let f = self.field_mut(target);
            for ev in &input.events {
                f.handle(ev, &mut clipboard, inner_w, wf);
            }
        }
        if self.field_mut(target).value() != before {
            push_undo(&mut self.field_undo_stack, field_idx, before);
        }
    }

    pub(super) fn build_disconnected(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        _text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let reason = match &self.screen {
            Screen::Disconnected(r) => r.clone(),
            _ => return empty_result(2.0),
        };

        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let title_size = 18.0 * gs;
        let body_size = 11.0 * gs;
        let btn_w = 160.0 * gs;
        let btn_h = 30.0 * gs;
        let gap = 12.0 * gs;

        let cx = screen_w / 2.0;
        let total_h = title_size + gap + body_size + gap * 2.0 + btn_h;
        let top_y = (screen_h - total_h) / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        elements.push(MenuElement::Text {
            x: cx,
            y: top_y,
            text: "Disconnected".into(),
            scale: title_size,
            color: [1.0, 0.4, 0.4, 1.0],
            centered: true,
        });

        elements.push(MenuElement::Text {
            x: cx,
            y: top_y + title_size + gap,
            text: reason,
            scale: body_size,
            color: [0.85, 0.85, 0.85, 0.9],
            centered: true,
        });

        let btn_y = top_y + title_size + gap + body_size + gap * 2.0;
        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);
        if push_button_f(
            &mut elements,
            &mut ctx,
            &mut any_hovered,
            input.cursor,
            input.clicked,
            cx - btn_w / 2.0,
            btn_y,
            btn_w,
            btn_h,
            gs,
            "Back to Menu",
            true,
        ) {
            self.set_screen(Screen::Main);
        }
        self.finish_focus(&ctx);

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: (input.clicked && any_hovered) || ctx.fired,
        }
    }
}

const UNDO_STACK_LIMIT: usize = 50;

#[derive(Clone, Copy)]
pub(super) enum TextTarget {
    EditName,
    EditAddress,
    PackSearch,
    AddFriend,
}

fn push_undo(stack: &mut Vec<(u8, String)>, field_idx: u8, prev: String) {
    if stack.len() >= UNDO_STACK_LIMIT {
        stack.remove(0);
    }
    stack.push((field_idx, prev));
}

pub(super) fn write_clipboard(text: &str) -> bool {
    crate::ui::common::set_clipboard(text)
}

/// The protocol to join `address` with: the completed ping's, else the
/// persisted one, so the join skips the wire-version probe when possible.
fn join_protocol(
    ping_results: &std::collections::HashMap<String, PingState>,
    address: &str,
    persisted: Option<i32>,
) -> Option<i32> {
    match ping_results.get(address) {
        Some(PingState::Success { protocol, .. }) => Some(*protocol),
        _ => persisted,
    }
}
