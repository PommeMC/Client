use super::*;

impl ChatState {
    pub fn request_command_confirmation(&mut self, command: String, kind: CommandConfirmationKind) {
        self.open_modal(ChatModal::Command(PendingCommand { command, kind }));
    }

    pub(super) fn open_modal(&mut self, modal: ChatModal) {
        self.modal = Some(modal);
        self.modal_buttons.clear();
        // Swapping the screen runs `ChatScreen.removed`.
        self.reset_chat_scroll();
    }

    pub(super) fn close_modal(&mut self) {
        self.modal = None;
        self.modal_buttons.clear();
    }

    pub fn request_open_url(&mut self, url: String) -> Option<ChatUiAction> {
        let Ok(url) = crate::chat_component::parse_untrusted_url(url) else {
            return None;
        };
        if !self.options.links {
            return None;
        }
        if self.options.links_prompt {
            self.open_modal(ChatModal::Link { url });
            None
        } else {
            Some(ChatUiAction::OpenUrl(url))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_modal_prompt(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
        spans_width_fn: &dyn Fn(&[TextSpan], f32) -> f32,
    ) -> Option<ChatUiAction> {
        let modal = self.modal.clone()?;
        let was_drawn = !self.modal_buttons.is_empty();
        let width0 = |spans: &[TextSpan]| spans_width_fn(spans, common::FONT_SIZE);
        self.modal_buttons = modal
            .confirm_screen()
            .push(elements, cursor, screen_w, screen_h, gs, &width0);
        if !clicked || !was_drawn {
            return None;
        }
        let pressed = self
            .modal_buttons
            .iter()
            .position(|rect| common::hit_test(cursor, *rect))?;
        self.close_modal();
        match modal {
            ChatModal::Link { url } => match pressed {
                0 => Some(ChatUiAction::OpenUrl(url)),
                1 => {
                    common::set_clipboard(&url);
                    None
                }
                _ => None,
            },
            ChatModal::Command(pending) if pressed == 0 => match pending.kind {
                CommandConfirmationKind::SignatureRequired => {
                    common::set_clipboard(&format!("/{}", pending.command));
                    None
                }
                CommandConfirmationKind::PermissionsRequired
                | CommandConfirmationKind::ParseErrors => {
                    Some(ChatUiAction::RunCommandUnsigned(pending.command))
                }
            },
            ChatModal::Command(_) => None,
        }
    }
}
/// `ConfirmScreen.addMessage`'s `setMaxRows`.
const CONFIRM_MESSAGE_MAX_ROWS: usize = 15;

/// Vanilla `ConfirmScreen`: title, wrapped message, optional extra line and a
/// button row in a vertical `LinearLayout` centred on the screen.
pub(super) struct ConfirmScreen {
    pub(super) title: Component,
    pub(super) message: Component,
    /// `ConfirmLinkScreen.addAdditionalText`'s warning.
    pub(super) warning: Option<Component>,
    /// Lang key and gui-unit width of each button, left to right.
    pub(super) buttons: Vec<(&'static str, i32)>,
}

/// A text line's top-left in gui units.
pub(super) struct PlacedLine {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) spans: Vec<TextSpan>,
}

/// [`ConfirmScreen`] positions in gui units; buttons as `[x, y, w, h]`.
pub(super) struct ConfirmLayout {
    pub(super) lines: Vec<PlacedLine>,
    pub(super) buttons: Vec<[i32; 4]>,
}

impl ConfirmScreen {
    /// `ConfirmScreen.init` + `repositionElements` on a `width` x `height`
    /// gui-unit screen. Vertical spacing 8, children centred; the button row
    /// has spacing 4 and 16 padding above each button.
    pub(super) fn layout(
        &self,
        width: i32,
        height: i32,
        width0: &dyn Fn(&[TextSpan]) -> f32,
    ) -> ConfirmLayout {
        let text_width = |spans: &[TextSpan]| width0(spans).round() as i32;

        let title = format_component_spans(&self.title, common::WHITE);
        // MultiLineTextWidget(maxWidth = width - 50, maxRows = 15, centred);
        // MultiLineLabel ends a cut-off last row with an ellipsis.
        let max_width = (width - 50).max(1);
        let mut message = wrap_spans(
            &format_component_spans(&self.message, common::WHITE),
            max_width as f32,
            width0,
        );
        if message.len() > CONFIRM_MESSAGE_MAX_ROWS {
            message.truncate(CONFIRM_MESSAGE_MAX_ROWS);
            let last = message.last_mut().expect("15 rows");
            *last = clip_spans(last, width0(last), width0);
        }
        let message_width = message
            .iter()
            .map(|line| text_width(line))
            .max()
            .unwrap_or(0)
            .min(max_width);
        let warning = self
            .warning
            .as_ref()
            .map(|warning| format_component_spans(warning, common::WHITE));
        let row_width =
            self.buttons.iter().map(|(_, w)| w).sum::<i32>() + 4 * (self.buttons.len() as i32 - 1);

        let title_width = text_width(&title);
        let mut children = vec![
            (title_width, LINE_HEIGHT),
            (message_width, message.len() as i32 * LINE_HEIGHT),
        ];
        let warning_width = warning.as_deref().map(text_width);
        if let Some(warning_width) = warning_width {
            children.push((warning_width, LINE_HEIGHT));
        }
        children.push((row_width, 16 + 20));
        let layout_width = children.iter().map(|&(w, _)| w).max().unwrap_or(0);
        let layout_height =
            children.iter().map(|&(_, h)| h).sum::<i32>() + 8 * (children.len() as i32 - 1);
        // `FrameLayout.centerInRectangle` truncates the half offsets.
        let left = (width - layout_width) / 2;
        let centred = |child_width: i32| left + (layout_width - child_width) / 2;
        let mut y = (height - layout_height) / 2;

        let mut lines = vec![PlacedLine {
            x: centred(title_width),
            y,
            spans: title,
        }];
        y += LINE_HEIGHT + 8;
        let mid_x = centred(message_width) + message_width / 2;
        for line in message {
            lines.push(PlacedLine {
                x: mid_x - text_width(&line) / 2,
                y,
                spans: line,
            });
            y += LINE_HEIGHT;
        }
        y += 8;
        if let (Some(spans), Some(warning_width)) = (warning, warning_width) {
            lines.push(PlacedLine {
                x: centred(warning_width),
                y,
                spans,
            });
            y += LINE_HEIGHT + 8;
        }
        let mut x = centred(row_width);
        let buttons = self
            .buttons
            .iter()
            .map(|&(_, w)| {
                let rect = [x, y + 16, w, 20];
                x += w + 4;
                rect
            })
            .collect();
        ConfirmLayout { lines, buttons }
    }

    /// Draws the screen and returns its button rectangles in pixels.
    pub(super) fn push(
        &self,
        elements: &mut Vec<MenuElement>,
        cursor: (f32, f32),
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        width0: &dyn Fn(&[TextSpan]) -> f32,
    ) -> Vec<[f32; 4]> {
        // TODO: vanilla also blurs the world behind in-world screens
        // (`Screen.extractBlurredBackground`).
        common::push_menu_background(elements, screen_w, screen_h);
        let layout = self.layout(
            (screen_w / gs).ceil() as i32,
            (screen_h / gs).ceil() as i32,
            width0,
        );
        let fs = common::FONT_SIZE * gs;
        for line in layout.lines {
            elements.push(MenuElement::McText {
                x: line.x as f32 * gs,
                y: line.y as f32 * gs,
                spans: line.spans,
                scale: fs,
                centered: false,
                shadow: true,
            });
        }
        layout
            .buttons
            .iter()
            .zip(&self.buttons)
            .map(|(rect, (key, _))| {
                let [x, y, w, h] = rect.map(|v| v as f32 * gs);
                let label = crate::lang::translate(key).unwrap_or(key);
                common::push_button(elements, cursor, x, y, w, h, gs, fs, label, true);
                [x, y, w, h]
            })
            .collect()
    }
}
