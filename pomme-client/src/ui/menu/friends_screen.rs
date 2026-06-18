use super::*;
use crate::ui::friends::{FriendAction, FriendLists, FriendStatus, FriendsState, UpdateType};

// Vanilla friends-list status colors: green for any online/playing state, gray
// for offline (gui.friends.presence.status.* uses -16711936 / -6250336).
const STATUS_ONLINE: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const STATUS_OFFLINE: [f32; 4] = [0.627, 0.627, 0.627, 1.0]; // -6250336 (0xFFA0A0A0)
const MSG_DIM: [f32; 4] = [0.667, 0.667, 0.667, 1.0]; // ChatFormatting.GRAY (0xAAAAAA)
const TAB_DIM: [f32; 4] = [0.627, 0.627, 0.627, 1.0]; // -6250336 (0xFFA0A0A0)
const ERR_COL: [f32; 4] = [0.88, 0.45, 0.45, 1.0];
const BTN: f32 = 20.0; // vanilla per-row button size

impl MainMenu {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_friends(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let fs = common::FONT_SIZE * gs;
        let cursor = input.cursor;
        let clicked = input.clicked;

        // Vanilla FriendsOverlayScreen geometry: a 220px-wide content block,
        // height screen/gs - 80, CENTERED in the screen. An 8px background
        // border is drawn around it; the 220x20 tab bar sits 7px above the
        // content's top. Rows are 28px with a 24px face.
        let content_w = 220.0 * gs; // OVERLAY_WIDTH
        let border = 8.0 * gs; // BG_BORDER_WIDTH
        let tab_h = 20.0 * gs; // TAB_BUTTON_HEIGHT
        let tab_w = content_w / 2.0; // TAB_BUTTON_WIDTH = 110
        let row_h = 28.0 * gs;
        let face = 24.0 * gs;
        let btn = BTN * gs;

        // Cap height so the panel stays a dialog (not a tall strip) on big
        // windows with a fixed GUI scale. 220 = content width; 280 keeps it ~1.27:1.
        let content_h = (screen_h - 80.0 * gs).clamp(60.0 * gs, 280.0 * gs);
        let content_x = (screen_w - content_w) / 2.0;
        let content_y = (screen_h - content_h) / 2.0; // = 40*gs -> vertical centering

        // Background frame = content plus the 8px border on every side.
        let panel_x = content_x - border;
        let panel_y = content_y - border;
        let panel_w = content_w + border * 2.0;
        let panel_h = content_h + border * 2.0 + gs; // vanilla draws bg height +1

        // Tab bar: 7px above the content top.
        let tab_y = content_y - tab_h - 7.0 * gs;

        let on_friends = self.friend_tab == FriendTab::Friends;
        if on_friends {
            self.handle_text_input(input, 1);
        }
        let popup_open = self.pending_remove.is_some();

        // Esc: close the popup, else blur a field, else leave the screen.
        if input.escape {
            if self.pending_remove.take().is_some() {
                return empty_result(2.0);
            }
            if self.focused_field.take().is_none() {
                self.set_screen(Screen::Main);
                return empty_result(2.0);
            }
        }
        // Click outside the panel closes (unless a popup is capturing input).
        if !popup_open
            && clicked
            && !common::hit_test(
                cursor,
                [panel_x, tab_y, panel_w, (panel_y + panel_h) - tab_y],
            )
        {
            self.set_screen(Screen::Main);
            return empty_result(2.0);
        }
        if input.f5 {
            self.refresh_friends_now();
        }

        let state = self.friends_data.read().clone();
        let lists = match &state {
            FriendsState::Loaded(l) => Some(l.clone()),
            _ => None,
        };
        let incoming_count = lists.as_ref().map(|l| l.incoming.len()).unwrap_or(0);

        // Vanilla renders Friends as a dialog over the previous screen. Re-draw
        // the title screen as a static backdrop (neutral input → visuals only,
        // no hover/click/actions); the renderer blurs it behind the panel.
        let backdrop_input = MenuInput {
            cursor: (-1.0, -1.0),
            clicked: false,
            mouse_held: false,
            typed_chars: Vec::new(),
            backspace: false,
            enter: false,
            escape: false,
            tab: false,
            f5: false,
            select_all: false,
            copy: false,
            cut: false,
            undo: false,
            scroll_delta: 0.0,
        };
        let mut elements = self
            .build_main(screen_w, screen_h, &backdrop_input, text_width_fn)
            .elements;
        // Everything above is the title-screen backdrop; the marker tells the
        // renderer to draw it into the scene and blur it. A full-screen frosted
        // quad then paints that blurred title screen, with the dialog on top.
        elements.push(MenuElement::BlurBackdrop);
        elements.push(MenuElement::FrostedRect {
            x: 0.0,
            y: 0.0,
            w: screen_w,
            h: screen_h,
            corner_radius: 0.0,
            tint: [0.6, 0.6, 0.65, 1.0],
        });

        // --- Tab bar (both clickable) ---
        let tabs = [
            (content_x, FriendTab::Friends, "Friends".to_string()),
            (
                content_x + tab_w,
                FriendTab::Requests,
                format!("Requests ({incoming_count})"),
            ),
        ];
        for (tx, tab, label) in &tabs {
            let active = self.friend_tab == *tab;
            let hovered = !popup_open && common::hit_test(cursor, [*tx, tab_y, tab_w, tab_h]);
            // Vanilla FriendsOverlayTabButton sprite + its .mcmeta nine-slice border.
            let (sprite, tab_border) = if active {
                (SpriteId::FriendsTab, 3.0)
            } else if hovered {
                (SpriteId::FriendsTabHighlighted, 3.0)
            } else {
                (SpriteId::FriendsTabDisabled, 1.0)
            };
            nine_slice(
                &mut elements,
                *tx,
                tab_y,
                tab_w,
                tab_h,
                sprite,
                tab_border * gs,
            );
            elements.push(MenuElement::Text {
                x: tx + tab_w / 2.0,
                y: tab_y + (tab_h - fs) / 2.0,
                text: label.clone(),
                scale: fs,
                color: if active { WHITE } else { TAB_DIM },
                centered: true,
            });
            if active {
                // Vanilla renderFocusUnderline: 1px tall, label width (capped at
                // tab_w-4), centered, 2px above the tab bottom.
                let uw = text_width_fn(label, fs).min(tab_w - 4.0 * gs);
                elements.push(MenuElement::Rect {
                    x: tx + (tab_w - uw) / 2.0,
                    y: tab_y + tab_h - 2.0 * gs,
                    w: uw,
                    h: gs,
                    corner_radius: 0.0,
                    color: WHITE,
                });
            }
            if !active && clicked && hovered {
                self.friend_tab = *tab;
                self.scroll_offset = 0.0;
                self.focused_field = None;
            }
        }

        nine_slice(
            &mut elements,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            SpriteId::FriendsBackground,
            border,
        );

        // --- Body ---
        match (lists, self.friend_tab) {
            (Some(lists), FriendTab::Friends) => self.friends_body(
                &mut elements,
                input,
                text_width_fn,
                &lists,
                popup_open,
                screen_w,
                screen_h,
                (content_x, content_y, content_w, content_h),
                (gs, fs, row_h, face, btn),
            ),
            (Some(lists), FriendTab::Requests) => self.requests_body(
                &mut elements,
                input,
                text_width_fn,
                &lists,
                popup_open,
                screen_w,
                screen_h,
                (content_x, content_y, content_w, content_h),
                (gs, fs, row_h, face, btn),
            ),
            (None, _) => {
                let msg = match &state {
                    FriendsState::Failed(e) => (e.as_str(), ERR_COL),
                    _ => ("Loading friends…", MSG_DIM),
                };
                push_centered(
                    &mut elements,
                    panel_x + panel_w / 2.0,
                    content_y + (content_h - fs) / 2.0,
                    fs,
                    msg.0,
                    msg.1,
                );
            }
        }

        // --- Remove-confirm popup (drawn over everything) ---
        if let Some((uuid, name)) = self.pending_remove.clone() {
            let pw = 240.0 * gs;
            let ph = 90.0 * gs;
            let px = (screen_w - pw) / 2.0;
            let py = (screen_h - ph) / 2.0;
            elements.push(MenuElement::FrostedRect {
                x: px,
                y: py,
                w: pw,
                h: ph,
                corner_radius: 6.0 * gs,
                tint: [0.05, 0.05, 0.1, 0.95],
            });
            push_centered(
                &mut elements,
                screen_w / 2.0,
                py + 12.0 * gs,
                fs,
                &format!("Remove {name}?"),
                WHITE,
            );
            // Vanilla FriendsListConfirmScreen has a message body under the title.
            push_centered(
                &mut elements,
                screen_w / 2.0,
                py + 12.0 * gs + fs + 6.0 * gs,
                fs,
                "Are you sure you want to remove them?",
                MSG_DIM,
            );
            let bw = 84.0 * gs;
            let bh = common::BTN_H * gs;
            let by = py + ph - bh - 10.0 * gs;
            if common::push_button(
                &mut elements,
                cursor,
                px + 12.0 * gs,
                by,
                bw,
                bh,
                gs,
                fs,
                "Remove",
                true,
            ) && clicked
            {
                self.pending_remove = None;
                self.friend_mutate(FriendAction::ById(uuid, UpdateType::Remove));
            }
            if common::push_button(
                &mut elements,
                cursor,
                px + pw - bw - 12.0 * gs,
                by,
                bw,
                bh,
                gs,
                fs,
                "Cancel",
                true,
            ) && clicked
            {
                self.pending_remove = None;
            }
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: false,
            blur: 1.0,
            clicked_button: false,
        }
    }

    /// Friends tab: add-friend box + send, optional error line, then the list
    /// with per-row remove buttons.
    #[allow(clippy::too_many_arguments)]
    fn friends_body(
        &mut self,
        elements: &mut Vec<MenuElement>,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        lists: &FriendLists,
        popup_open: bool,
        screen_w: f32,
        screen_h: f32,
        (content_x, content_y, content_w, content_h): (f32, f32, f32, f32),
        (gs, fs, row_h, face, btn): (f32, f32, f32, f32, f32),
    ) {
        let cursor = input.cursor;
        let clicked = input.clicked && !popup_open;
        let field_h = FIELD_H * gs;

        let (lx, lw) = content_inset(content_x, content_w, gs);

        // Add-friend row — vanilla AddFriendWidget: 3px top pad, editbox
        // `lw - 20(button) - 3(gap)`, 20px send button flush right.
        let field_y = content_y + 3.0 * gs;
        let field_w = lw - 23.0 * gs;
        push_text_field(
            elements,
            lx,
            field_y,
            field_w,
            field_h,
            fs,
            gs,
            &self.add_friend_name,
            self.focused_field == Some(0),
            self.focused_field == Some(0) && self.field_all_selected,
            &self.cursor_blink,
            text_width_fn,
        );
        if self.add_friend_name.is_empty() {
            elements.push(MenuElement::Text {
                x: lx + 4.0 * gs,
                y: field_y + (field_h - fs) / 2.0,
                text: "Enter Profile Name".into(),
                scale: fs,
                color: MSG_DIM,
                centered: false,
            });
        }
        if clicked && common::hit_test(cursor, [lx, field_y, field_w, field_h]) {
            self.on_field_click(0);
        }
        let send_hit = icon_button(
            elements,
            cursor,
            lx + lw - btn,
            field_y,
            btn,
            gs,
            SpriteId::FriendsSend,
            15.0,
            15.0,
            screen_w,
            screen_h,
            "Send request",
        );
        let submit = (clicked && send_hit) || (self.focused_field == Some(0) && input.enter);
        if submit {
            let name = self.add_friend_name.trim().to_string();
            if !name.is_empty() {
                self.add_friend_name.clear();
                self.friend_mutate(FriendAction::AddByName(name));
            }
        }

        // Profile row (input 3+20 + 6px margin) then separator (+ profile 9 + 4).
        let label = "My profile name: ";
        let profile_y = content_y + 29.0 * gs;
        elements.push(MenuElement::Text {
            x: lx,
            y: profile_y,
            text: label.into(),
            scale: fs,
            color: TAB_DIM, // vanilla PROFILE_NAME_LABEL = -6250336
            centered: false,
        });
        let name_x = lx + text_width_fn(label, fs);
        elements.push(MenuElement::Text {
            x: name_x,
            y: profile_y,
            text: self.username.clone(),
            scale: fs,
            color: WHITE,
            centered: false,
        });
        // Vanilla: the profile name is a button that copies to clipboard (with
        // tooltip).
        let name_w = text_width_fn(&self.username, fs);
        if common::hit_test(cursor, [name_x, profile_y, name_w, fs]) {
            common::push_tooltip(
                elements,
                cursor,
                screen_w,
                screen_h,
                gs,
                "Copy to clipboard",
            );
            if clicked {
                super::servers::write_clipboard(&self.username);
            }
        }
        push_separator(
            elements,
            content_x,
            content_y + 42.0 * gs,
            content_w,
            2.0 * gs,
        );

        // List starts just below the separator; an error line (if any) slots in
        // first so the normal layout stays pixel-exact.
        let mut list_top = content_y + 44.0 * gs;
        if let Some(err) = self.action_error.read().clone() {
            elements.push(MenuElement::Text {
                x: lx,
                y: list_top,
                text: err,
                scale: fs,
                color: ERR_COL,
                centered: false,
            });
            list_top += fs + 2.0 * gs;
        }
        let list_h = content_y + content_h - list_top;

        if lists.friends.is_empty() {
            // Vanilla showEmpty: 128x48 illustration above the text, 8px gap, centered.
            let illo_w = 128.0 * gs;
            let illo_h = 48.0 * gs;
            let gap = 8.0 * gs;
            let group_top = list_top + (list_h - (illo_h + gap + fs)) / 2.0;
            elements.push(MenuElement::Image {
                x: content_x + (content_w - illo_w) / 2.0,
                y: group_top,
                w: illo_w,
                h: illo_h,
                sprite: SpriteId::FriendsIllustration,
                tint: WHITE,
            });
            push_centered(
                elements,
                content_x + content_w / 2.0,
                group_top + illo_h + gap,
                fs,
                "No friends yet — add one above",
                MSG_DIM,
            );
            return;
        }

        let n = lists.friends.len() as f32;
        // Vanilla appends a centered "manage account" footer below the list.
        let footer_pad = 8.0 * gs;
        let total = n * row_h + footer_pad + fs;
        self.scroll_region(input, [content_x, list_top, content_w, list_h], total, gs);

        elements.push(MenuElement::ScissorPush {
            x: content_x,
            y: list_top,
            w: content_w,
            h: list_h,
        });
        for (i, friend) in lists.friends.iter().enumerate() {
            let ey = list_top + i as f32 * row_h - self.scroll_offset;
            if ey + row_h < list_top || ey > list_top + list_h {
                continue;
            }
            push_face_name(
                elements,
                gs,
                fs,
                lx,
                ey,
                row_h,
                face,
                &friend.uuid,
                &friend.name,
                Some(&friend.status),
            );
            let bx = lx + lw - btn;
            if icon_button(
                elements,
                cursor,
                bx,
                ey + (row_h - btn) / 2.0,
                btn,
                gs,
                SpriteId::FriendsRemove,
                13.0,
                11.0,
                screen_w,
                screen_h,
                "Unfriend",
            ) && clicked
            {
                self.pending_remove = Some((friend.uuid.clone(), friend.name.clone()));
            }
        }
        let footer_y = list_top + n * row_h - self.scroll_offset + footer_pad;
        if footer_y + fs >= list_top && footer_y <= list_top + list_h {
            push_centered(
                elements,
                content_x + content_w / 2.0,
                footer_y,
                fs,
                "Manage your account at minecraft.net",
                MSG_DIM,
            );
        }
        elements.push(MenuElement::ScissorPop);
        push_scrollbar(
            elements,
            content_x + content_w,
            list_top,
            list_h,
            total,
            self.scroll_offset,
            gs,
        );
    }

    /// Requests tab: "Received" (accept/decline) then "Sent" (cancel).
    #[allow(clippy::too_many_arguments)]
    fn requests_body(
        &mut self,
        elements: &mut Vec<MenuElement>,
        input: &MenuInput,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        lists: &FriendLists,
        popup_open: bool,
        screen_w: f32,
        screen_h: f32,
        (content_x, content_y, content_w, content_h): (f32, f32, f32, f32),
        (gs, fs, row_h, face, btn): (f32, f32, f32, f32, f32),
    ) {
        let cursor = input.cursor;
        let clicked = input.clicked && !popup_open;
        let (lx, lw) = content_inset(content_x, content_w, gs);

        if lists.incoming.is_empty() && lists.outgoing.is_empty() {
            push_centered(
                elements,
                content_x + content_w / 2.0,
                content_y + (content_h - fs) / 2.0,
                fs,
                "No pending requests",
                MSG_DIM,
            );
            return;
        }

        let header_h = fs + 6.0 * gs;
        let section_h = |list: &[_]| {
            if list.is_empty() {
                0.0
            } else {
                header_h + list.len() as f32 * row_h
            }
        };
        let total = section_h(&lists.incoming) + section_h(&lists.outgoing);
        self.scroll_region(
            input,
            [content_x, content_y, content_w, content_h],
            total,
            gs,
        );

        elements.push(MenuElement::ScissorPush {
            x: content_x,
            y: content_y,
            w: content_w,
            h: content_h,
        });
        let mut y = content_y - self.scroll_offset;
        let visible = |ey: f32| ey + row_h >= content_y && ey <= content_y + content_h;

        // Incoming: accept (left) + reject (right).
        if !lists.incoming.is_empty() {
            push_section_header(
                elements,
                text_width_fn,
                content_x,
                content_w,
                y,
                fs,
                gs,
                "Received",
            );
            y += header_h;
            for req in &lists.incoming {
                if visible(y) {
                    push_face_name(
                        elements, gs, fs, lx, y, row_h, face, &req.uuid, &req.name, None,
                    );
                    let reject_x = lx + lw - btn;
                    let accept_x = reject_x - btn - 4.0 * gs;
                    let by = y + (row_h - btn) / 2.0;
                    if icon_button(
                        elements,
                        cursor,
                        accept_x,
                        by,
                        btn,
                        gs,
                        SpriteId::FriendsAccept,
                        18.0,
                        18.0,
                        screen_w,
                        screen_h,
                        "Accept",
                    ) && clicked
                    {
                        self.friend_mutate(FriendAction::ById(req.uuid.clone(), UpdateType::Add));
                    }
                    if icon_button(
                        elements,
                        cursor,
                        reject_x,
                        by,
                        btn,
                        gs,
                        SpriteId::FriendsReject,
                        18.0,
                        18.0,
                        screen_w,
                        screen_h,
                        "Decline",
                    ) && clicked
                    {
                        self.friend_mutate(FriendAction::ById(
                            req.uuid.clone(),
                            UpdateType::Remove,
                        ));
                    }
                }
                y += row_h;
            }
        }
        // Outgoing: cancel (right).
        if !lists.outgoing.is_empty() {
            push_section_header(
                elements,
                text_width_fn,
                content_x,
                content_w,
                y,
                fs,
                gs,
                "Sent",
            );
            y += header_h;
            for req in &lists.outgoing {
                if visible(y) {
                    push_face_name(
                        elements, gs, fs, lx, y, row_h, face, &req.uuid, &req.name, None,
                    );
                    let by = y + (row_h - btn) / 2.0;
                    if icon_button(
                        elements,
                        cursor,
                        lx + lw - btn,
                        by,
                        btn,
                        gs,
                        SpriteId::FriendsCancel,
                        12.0,
                        12.0,
                        screen_w,
                        screen_h,
                        "Cancel request",
                    ) && clicked
                    {
                        self.friend_mutate(FriendAction::ById(
                            req.uuid.clone(),
                            UpdateType::Remove,
                        ));
                    }
                }
                y += row_h;
            }
        }
        elements.push(MenuElement::ScissorPop);
        push_scrollbar(
            elements,
            content_x + content_w,
            content_y,
            content_h,
            total,
            self.scroll_offset,
            gs,
        );
    }

    /// Apply mouse-wheel scrolling within `area`, clamp the offset, and return
    /// the right-edge gutter to reserve for the scrollbar (0 when it all fits).
    fn scroll_region(&mut self, input: &MenuInput, area: [f32; 4], total: f32, gs: f32) -> f32 {
        let max_scroll = (total - area[3]).max(0.0);
        if common::hit_test(input.cursor, area) {
            self.scroll_offset -= input.scroll_delta * 20.0 * gs;
        }
        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);
        if max_scroll > 0.0 { 8.0 * gs } else { 0.0 }
    }

    fn friend_mutate(&mut self, action: FriendAction) {
        if let Some(token) = self.access_token.clone() {
            friends::friend_action(
                std::sync::Arc::clone(&self.rt),
                token,
                action,
                &self.friends_data,
                &self.face_cache,
                &self.action_error,
            );
        }
    }

    /// Re-fetch the friends list + faces (no-op without a signed-in account).
    pub(super) fn refresh_friends_now(&mut self) {
        if let Some(token) = self.access_token.clone() {
            friends::refresh_friends(
                std::sync::Arc::clone(&self.rt),
                token,
                &self.friends_data,
                &self.face_cache,
            );
        }
    }
}

fn nine_slice(
    elements: &mut Vec<MenuElement>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    sprite: SpriteId,
    border: f32,
) {
    elements.push(MenuElement::NineSlice {
        x,
        y,
        w,
        h,
        sprite,
        border,
        tint: WHITE,
    });
}

/// A 20×20 sprite button with a faint hover highlight; returns whether hovered.
/// Vanilla `SpriteIconButton`: a widget-button background with the icon
/// centered at its native size (`iw`×`ih` source pixels), not stretched to the
/// button.
#[allow(clippy::too_many_arguments)]
fn icon_button(
    elements: &mut Vec<MenuElement>,
    cursor: (f32, f32),
    x: f32,
    y: f32,
    size: f32,
    gs: f32,
    sprite: SpriteId,
    iw: f32,
    ih: f32,
    screen_w: f32,
    screen_h: f32,
    tooltip: &str,
) -> bool {
    let hovered = common::hit_test(cursor, [x, y, size, size]);
    nine_slice(
        elements,
        x,
        y,
        size,
        size,
        if hovered {
            SpriteId::ButtonHover
        } else {
            SpriteId::ButtonNormal
        },
        3.0 * gs,
    );
    let (icon_w, icon_h) = (iw * gs, ih * gs);
    elements.push(MenuElement::Image {
        x: x + (size - icon_w) / 2.0,
        y: y + (size - icon_h) / 2.0,
        w: icon_w,
        h: icon_h,
        sprite,
        tint: WHITE,
    });
    if hovered && !tooltip.is_empty() {
        common::push_tooltip(elements, cursor, screen_w, screen_h, gs, tooltip);
    }
    hovered
}

/// Face + name; with `status` it's a two-line row, without it the name is
/// vertically centered (used for requests, which carry no presence).
#[allow(clippy::too_many_arguments)]
fn push_face_name(
    elements: &mut Vec<MenuElement>,
    gs: f32,
    fs: f32,
    content_x: f32,
    ey: f32,
    row_h: f32,
    face: f32,
    uuid: &str,
    name: &str,
    status: Option<&FriendStatus>,
) {
    elements.push(MenuElement::SkinFace {
        x: content_x,
        y: ey + (row_h - face) / 2.0,
        size: face,
        uuid: uuid.into(),
    });
    let text_x = content_x + face + 4.0 * gs;
    match status {
        Some(s) => {
            // Vanilla: name at row_h/3 (centered on the glyph), status below it.
            let name_y = ey + row_h / 3.0 - fs / 2.0;
            elements.push(MenuElement::Text {
                x: text_x,
                y: name_y,
                text: name.into(),
                scale: fs,
                color: WHITE,
                centered: false,
            });
            let (label, color) = status_label(s);
            elements.push(MenuElement::Text {
                x: text_x,
                y: name_y + fs + 2.0 * gs,
                text: label.into(),
                scale: fs,
                color,
                centered: false,
            });
        }
        None => elements.push(MenuElement::Text {
            x: text_x,
            y: ey + (row_h - fs) / 2.0,
            text: name.into(),
            scale: fs,
            color: WHITE,
            centered: false,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_section_header(
    elements: &mut Vec<MenuElement>,
    text_width_fn: &dyn Fn(&str, f32) -> f32,
    content_x: f32,
    content_w: f32,
    y: f32,
    fs: f32,
    gs: f32,
    text: &str,
) {
    // Vanilla RECEIVED/SENT headers: white, BOLD + UNDERLINE, centered.
    let cx = content_x + content_w / 2.0;
    let ty = y + 3.0 * gs;
    // Faux-bold: draw twice, second offset 1px in x (MC's bold overdraw).
    for dx in [0.0, gs] {
        elements.push(MenuElement::Text {
            x: cx + dx,
            y: ty,
            text: text.into(),
            scale: fs,
            color: WHITE,
            centered: true,
        });
    }
    let tw = text_width_fn(text, fs) + gs; // +1px for the bold overdraw
    elements.push(MenuElement::Rect {
        x: cx - tw / 2.0,
        y: ty + fs + gs,
        w: tw,
        h: gs,
        corner_radius: 0.0,
        color: WHITE,
    });
}

fn push_scrollbar(
    elements: &mut Vec<MenuElement>,
    right_x: f32,
    top: f32,
    h: f32,
    total: f32,
    scroll: f32,
    gs: f32,
) {
    let max_scroll = (total - h).max(0.0);
    if max_scroll <= 0.0 {
        return;
    }
    // 6px track, inset 2px from the content's right edge (vanilla spacing).
    let track_w = 6.0 * gs;
    let track_x = right_x - track_w - 2.0 * gs;
    let thumb_h = (h * h / total).max(16.0 * gs); // vanilla min thumb is larger
    let thumb_y = top + (scroll / max_scroll) * (h - thumb_h);
    nine_slice(
        elements,
        track_x,
        top,
        track_w,
        h,
        SpriteId::ScrollerBackground,
        gs,
    );
    nine_slice(
        elements,
        track_x,
        thumb_y,
        track_w,
        thumb_h,
        SpriteId::Scroller,
        gs,
    );
}

/// Vanilla `gui.friends.presence.status.*` label + color for a friend.
fn status_label(status: &FriendStatus) -> (&'static str, [f32; 4]) {
    match status {
        FriendStatus::Online => ("Online", STATUS_ONLINE),
        FriendStatus::PlayingOffline => ("Playing offline", STATUS_ONLINE),
        FriendStatus::PlayingServer => ("Playing on a server", STATUS_ONLINE),
        FriendStatus::Offline => ("Offline", STATUS_OFFLINE),
    }
}

/// Vanilla insets the input/profile rows and list entries 8px from the
/// content's left edge (paddingLeft 8; getListContentWidth = w - 16), leaving
/// the right margin for the scrollbar. Returns `(left x, usable width)`.
fn content_inset(content_x: f32, content_w: f32, gs: f32) -> (f32, f32) {
    let inset = 8.0 * gs;
    (content_x + inset, content_w - inset * 2.0)
}

fn push_centered(
    elements: &mut Vec<MenuElement>,
    cx: f32,
    y: f32,
    fs: f32,
    text: &str,
    color: [f32; 4],
) {
    elements.push(MenuElement::Text {
        x: cx,
        y,
        text: text.into(),
        scale: fs,
        color,
        centered: true,
    });
}
