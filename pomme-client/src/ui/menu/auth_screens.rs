use super::*;

impl MainMenu {
    pub(super) fn build_auth_prompt(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        _text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Screen::AuthPrompt { pending } = self.screen else {
            return empty_result(2.0);
        };

        if input.escape {
            self.screen = Screen::Main;
            return empty_result(2.0);
        }

        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let title_size = 18.0 * gs;
        let body_size = 10.0 * gs;
        let btn_w = 180.0 * gs;
        let btn_h = 30.0 * gs;
        let gap = 10.0 * gs;
        let cx = screen_w / 2.0;
        let dim: [f32; 4] = [0.7, 0.7, 0.7, 0.8];

        let lines = [
            "You need to sign in with your Microsoft account.",
            "",
            "A browser tab will open where you can sign in.",
            "Once complete, the client will detect it automatically.",
            "",
            "In the future, a launcher will handle authentication.",
            "For now, we use a temporary sign-in method.",
        ];

        let text_h = lines.len() as f32 * (body_size + 3.0 * gs);
        let total_h = title_size + gap * 2.0 + text_h + gap * 2.0 + btn_h + gap + btn_h;
        let mut y = (screen_h - total_h) / 2.0;

        let mut elements = Vec::new();
        let mut any_hovered = false;

        elements.push(MenuElement::Text {
            x: cx,
            y,
            text: "Sign In Required".into(),
            scale: title_size,
            color: WHITE,
            centered: true,
        });
        y += title_size + gap * 2.0;

        for line in &lines {
            if !line.is_empty() {
                elements.push(MenuElement::Text {
                    x: cx,
                    y,
                    text: (*line).into(),
                    scale: body_size,
                    color: dim,
                    centered: true,
                });
            }
            y += body_size + 3.0 * gs;
        }
        y += gap;

        if push_button(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            cx - btn_w / 2.0,
            y,
            btn_w,
            btn_h,
            gs,
            "Sign in with Microsoft",
            true,
        ) && input.clicked
        {
            self.screen = Screen::Auth { pending };
            auth::spawn_auth(
                &self.rt,
                Arc::clone(&self.auth_status),
                self.cache_file.clone(),
            );
        }
        y += btn_h + gap;

        if push_button(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            cx - btn_w / 2.0,
            y,
            btn_w,
            btn_h,
            gs,
            "Back",
            true,
        ) && input.clicked
        {
            self.screen = Screen::Main;
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: false,
        }
    }

    pub(super) fn cancel_auth(&mut self) {
        self.screen = Screen::Main;
        *self.auth_status.lock() = AuthStatus::Idle;
    }

    pub(super) fn build_auth(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        _text_width_fn: &dyn Fn(&str, f32) -> f32,
    ) -> MainMenuResult {
        let Screen::Auth { pending } = self.screen else {
            return empty_result(2.0);
        };

        let gs = crate::ui::hud::gui_scale(screen_w, screen_h, self.gui_scale_setting);
        let title_size = 18.0 * gs;
        let body_size = 11.0 * gs;
        let btn_w = 160.0 * gs;
        let btn_h = 30.0 * gs;
        let gap = 12.0 * gs;
        let cx = screen_w / 2.0;
        let status_color: [f32; 4] = [0.8, 0.8, 0.8, 0.9];

        let mut elements = Vec::new();
        let mut any_hovered = false;

        let status = self.auth_status.lock();
        match &*status {
            AuthStatus::Idle | AuthStatus::OpeningBrowser => {
                elements.push(MenuElement::Text {
                    x: cx,
                    y: (screen_h - body_size) / 2.0,
                    text: "Opening browser...".into(),
                    scale: body_size,
                    color: status_color,
                    centered: true,
                });
            }
            AuthStatus::WaitingForBrowser => {
                drop(status);

                let total_h = title_size + gap + body_size + gap * 2.0 + btn_h;
                let mut y = (screen_h - total_h) / 2.0;

                elements.push(MenuElement::Text {
                    x: cx,
                    y,
                    text: "Sign in with Microsoft".into(),
                    scale: title_size,
                    color: WHITE,
                    centered: true,
                });
                y += title_size + gap;

                elements.push(MenuElement::Text {
                    x: cx,
                    y,
                    text: "Complete sign-in in your browser...".into(),
                    scale: body_size,
                    color: status_color,
                    centered: true,
                });
                y += body_size + gap * 2.0;

                if push_button(
                    &mut elements,
                    &mut any_hovered,
                    input.cursor,
                    cx - btn_w / 2.0,
                    y,
                    btn_w,
                    btn_h,
                    gs,
                    "Cancel",
                    true,
                ) && input.clicked
                {
                    self.cancel_auth();
                    return empty_result(2.0);
                }

                return MainMenuResult {
                    elements,
                    action: MenuAction::None,
                    cursor_pointer: any_hovered,
                    blur: 2.0,
                    clicked_button: false,
                };
            }
            AuthStatus::Exchanging => {
                elements.push(MenuElement::Text {
                    x: cx,
                    y: (screen_h - body_size) / 2.0,
                    text: "Logging in...".into(),
                    scale: body_size,
                    color: status_color,
                    centered: true,
                });
            }
            AuthStatus::Success(_) => {
                drop(status);
                let old = std::mem::replace(&mut *self.auth_status.lock(), AuthStatus::Idle);
                if let AuthStatus::Success(account) = old {
                    self.username = account.username.clone();
                    self.auth_account = Some(account);
                }

                match pending {
                    AuthPending::None | AuthPending::Singleplayer => self.screen = Screen::Main,
                    AuthPending::Multiplayer => {
                        self.screen = Screen::ServerList;
                        self.scroll_offset = 0.0;
                        self.selected_server = None;
                    }
                }

                return empty_result(2.0);
            }
            AuthStatus::Failed(err) => {
                let err = err.clone();
                drop(status);

                let total_h = title_size + gap + body_size + gap * 2.0 + btn_h;
                let mut y = (screen_h - total_h) / 2.0;

                elements.push(MenuElement::Text {
                    x: cx,
                    y,
                    text: "Authentication Failed".into(),
                    scale: title_size,
                    color: [1.0, 0.4, 0.4, 1.0],
                    centered: true,
                });
                y += title_size + gap;

                elements.push(MenuElement::Text {
                    x: cx,
                    y,
                    text: err,
                    scale: body_size,
                    color: [0.85, 0.85, 0.85, 0.9],
                    centered: true,
                });
                y += body_size + gap * 2.0;

                if push_button(
                    &mut elements,
                    &mut any_hovered,
                    input.cursor,
                    cx - btn_w / 2.0,
                    y,
                    btn_w,
                    btn_h,
                    gs,
                    "Back",
                    true,
                ) && input.clicked
                {
                    self.cancel_auth();
                    return empty_result(2.0);
                }

                return MainMenuResult {
                    elements,
                    action: MenuAction::None,
                    cursor_pointer: any_hovered,
                    blur: 2.0,
                    clicked_button: false,
                };
            }
        }
        drop(status);

        let btn_y = screen_h / 2.0 + gap * 2.0;
        if push_button(
            &mut elements,
            &mut any_hovered,
            input.cursor,
            cx - btn_w / 2.0,
            btn_y,
            btn_w,
            btn_h,
            gs,
            "Cancel",
            true,
        ) && input.clicked
        {
            self.cancel_auth();
        }

        MainMenuResult {
            elements,
            action: MenuAction::None,
            cursor_pointer: any_hovered,
            blur: 2.0,
            clicked_button: false,
        }
    }
}
