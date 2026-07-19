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

        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row1_x,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Join Server",
            has_sel,
        ) && clicked
            && let Some(idx) = self.selected_server
            && let Some(server) = self.server_list.servers.get(idx)
        {
            action = MenuAction::Connect {
                server: server.address.clone(),
                username: self.username.clone(),
                protocol: join_protocol(&ping_results, &server.address, server.protocol),
            };
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row1_x + top_w + gap,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Direct Connect",
            true,
        ) && clicked
        {
            self.edit_address = self.last_mp_ip.clone();
            self.set_screen(Screen::DirectConnect);
            self.focused_field = Some(0);
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row1_x + (top_w + gap) * 2.0,
            footer_y,
            top_w,
            btn_h,
            gs,
            "Add Server",
            true,
        ) && clicked
        {
            self.edit_name.clear();
            self.edit_address.clear();
            self.set_screen(Screen::AddServer);
            self.focused_field = Some(0);
        }

        let row2_y = footer_y + btn_h + gap;
        let row2_w = bot_w * 4.0 + gap * 3.0;
        let row2_x = (screen_w - row2_w) / 2.0;

        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row2_x,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Edit",
            has_sel,
        ) && clicked
            && let Some(idx) = self.selected_server
            && let Some(server) = self.server_list.servers.get(idx)
        {
            self.edit_name = server.name.clone();
            self.edit_address = server.address.clone();
            self.set_screen(Screen::EditServer(idx));
            self.focused_field = Some(0);
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row2_x + bot_w + gap,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Delete",
            has_sel,
        ) && clicked
            && let Some(idx) = self.selected_server
        {
            self.set_screen(Screen::ConfirmDelete(idx));
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row2_x + (bot_w + gap) * 2.0,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Refresh",
            true,
        ) && clicked
        {
            self.refresh_servers();
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            row2_x + (bot_w + gap) * 3.0,
            row2_y,
            bot_w,
            btn_h,
            gs,
            "Back",
            true,
        ) && clicked
        {
            self.set_screen(Screen::Main);
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

        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            btn_x,
            btn_y,
            form_w,
            btn_h,
            gs,
            "Delete",
            true,
        ) && clicked
        {
            self.server_list.remove(idx);
            self.selected_server = None;
            self.set_screen(Screen::ServerList);
        }
        if push_button(
            &mut elements,
            &mut any_hovered,
            cursor,
            btn_x,
            btn_y + btn_h + gap,
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

        self.handle_text_input(input, 1);

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

        push_text_field(
            &mut elements,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            &self.edit_address,
            self.focused_field == Some(0),
            self.focused_field == Some(0) && self.field_all_selected,
            &self.cursor_blink,
            text_width_fn,
        );
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.on_field_click(0);
        }
        y += field_h + 28.0 * gs;

        let valid = is_valid_address(&self.edit_address);
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
            self.last_mp_ip = self.edit_address.clone();
            let persisted = self
                .server_list
                .servers
                .iter()
                .find(|s| s.address == self.edit_address)
                .and_then(|s| s.protocol);
            action = MenuAction::Connect {
                server: self.edit_address.clone(),
                username: self.username.clone(),
                protocol: join_protocol(&self.ping_results.read(), &self.edit_address, persisted),
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

        self.handle_text_input(input, 2);

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

        push_text_field(
            &mut elements,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            &self.edit_name,
            self.focused_field == Some(0),
            self.focused_field == Some(0) && self.field_all_selected,
            &self.cursor_blink,
            text_width_fn,
        );
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.on_field_click(0);
        }
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

        push_text_field(
            &mut elements,
            form_x,
            y,
            form_w,
            field_h,
            fs,
            gs,
            &self.edit_address,
            self.focused_field == Some(1),
            self.focused_field == Some(1) && self.field_all_selected,
            &self.cursor_blink,
            text_width_fn,
        );
        if clicked && common::hit_test(cursor, [form_x, y, form_w, field_h]) {
            self.on_field_click(1);
        }
        y += field_h + 28.0 * gs;

        let valid = is_valid_address(&self.edit_address);
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
            let name = if self.edit_name.is_empty() {
                "Minecraft Server".to_string()
            } else {
                self.edit_name.clone()
            };
            let mut entry = ServerEntry {
                name,
                address: self.edit_address.clone(),
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

    pub(super) fn on_field_click(&mut self, field_idx: u8) {
        let now = Instant::now();
        let is_double = self.last_field_click == Some(field_idx)
            && now.duration_since(self.last_field_click_time).as_millis() < DOUBLE_CLICK_MS;
        self.focused_field = Some(field_idx);
        self.cursor_blink = now;
        self.field_all_selected = is_double;
        self.last_field_click = Some(field_idx);
        self.last_field_click_time = now;
    }

    pub(super) fn handle_text_input(&mut self, input: &MenuInput, field_count: u8) {
        if input.tab {
            self.focused_field = Some(match self.focused_field {
                Some(f) => (f + 1) % field_count,
                None => 0,
            });
            self.field_all_selected = false;
            self.cursor_blink = Instant::now();
        }

        let Some(field_idx) = self.focused_field else {
            return;
        };
        let target = match (&self.screen, field_idx) {
            (Screen::AddServer | Screen::EditServer(_), 0) => TextTarget::EditName,
            (Screen::AddServer | Screen::EditServer(_), 1) => TextTarget::EditAddress,
            (Screen::DirectConnect, 0) => TextTarget::EditAddress,
            (Screen::OptionsResourcePacks, 0) => TextTarget::PackSearch,
            (Screen::Friends, 0) => TextTarget::AddFriend,
            _ => return,
        };
        let text: &mut String = match target {
            TextTarget::EditName => &mut self.edit_name,
            TextTarget::EditAddress => &mut self.edit_address,
            TextTarget::PackSearch => &mut self.pack_search,
            TextTarget::AddFriend => &mut self.add_friend_name,
        };

        if input.copy && !text.is_empty() {
            write_clipboard(text);
        }

        if input.cut && !text.is_empty() && write_clipboard(text) {
            text.clear();
            self.field_all_selected = false;
        }

        if input.undo
            && let Some(pos) = self
                .field_undo_stack
                .iter()
                .rposition(|(f, _)| *f == field_idx)
        {
            let (_, prev) = self.field_undo_stack.remove(pos);
            *text = prev;
            self.field_all_selected = false;
            self.cursor_blink = Instant::now();
            return;
        }

        if input.select_all {
            self.field_all_selected = !text.is_empty();
        }

        let old_text = text.clone();

        if !input.typed_chars.is_empty() {
            if self.field_all_selected {
                text.clear();
                self.field_all_selected = false;
            }
            for ch in &input.typed_chars {
                text.push(*ch);
            }
        }
        if input.backspace {
            if self.field_all_selected {
                text.clear();
                self.field_all_selected = false;
            } else {
                text.pop();
            }
        }

        if *text != old_text {
            push_undo(&mut self.field_undo_stack, field_idx, old_text);
            self.cursor_blink = Instant::now();
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
        if push_button(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            cx - btn_w / 2.0,
            btn_y,
            btn_w,
            btn_h,
            gs,
            "Back to Menu",
            true,
        ) && input.clicked
        {
            self.set_screen(Screen::Main);
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: input.clicked && any_hovered,
        }
    }
}

const UNDO_STACK_LIMIT: usize = 50;

enum TextTarget {
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
