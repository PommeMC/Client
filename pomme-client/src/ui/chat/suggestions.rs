use super::*;

impl ChatState {
    /// The list's visible row count and furthest scroll offset.
    pub(super) fn suggestion_window(&self) -> (usize, usize) {
        let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
        (visible, self.suggestions.len() - visible)
    }

    /// The index of the suggestion row under `cursor`, as last drawn.
    pub(super) fn suggestion_at(&self, cursor: (f32, f32)) -> Option<usize> {
        self.suggestion_regions
            .iter()
            .find(|(_, rect)| common::hit_test(cursor, *rect))
            .map(|&(index, _)| index)
    }

    pub(super) fn keep_suggestion_visible(&mut self) {
        if self.suggestions.is_empty() {
            self.suggest_offset = 0;
            return;
        }
        let (visible, max_offset) = self.suggestion_window();
        if self.suggest_index < self.suggest_offset {
            self.suggest_offset = self.suggest_index.min(max_offset);
        } else if self.suggest_index >= self.suggest_offset + visible {
            self.suggest_offset = (self.suggest_index + 1 - visible).min(max_offset);
        }
    }

    pub(super) fn set_allow_suggestions(&mut self, allow: bool) {
        self.allow_suggestions = allow;
        if !allow {
            self.hide_suggestions();
        }
    }

    /// Vanilla `CommandSuggestions.hide`.
    pub(super) fn hide_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggest_index = 0;
        self.suggest_offset = 0;
        self.tab_cycles = false;
    }

    /// A fresh `CommandSuggestions`, as each `ChatScreen.init` builds.
    pub(super) fn clear_suggestions(&mut self) {
        self.hide_suggestions();
        self.pending_suggestions = None;
        self.current_parse = None;
        self.command_usage.clear();
        self.command_usage_start = None;
    }

    /// Vanilla `CommandSuggestions.updateCommandInfo`: completes the input up
    /// to the caret on every edit, asking the server where an argument could
    /// follow. Whether the result shows by itself is `update_usage_info`'s
    /// call.
    pub(super) fn update_command_info(&mut self, tree: Option<&CommandTree>) {
        let value = self.input.value().to_owned();
        let command = value.strip_prefix('/');
        if self
            .current_parse
            .as_ref()
            .is_some_and(|parse| Some(parse.input()) != command)
        {
            self.current_parse = None;
        }
        if !self.keep_suggestions {
            self.hide_suggestions();
        }
        self.command_usage.clear();
        self.command_usage_start = None;
        let cursor = self.input.cursor();
        if let Some(command) = command {
            let Some(tree) = tree else {
                self.pending_suggestions = None;
                return;
            };
            let parse = self
                .current_parse
                .get_or_insert_with(|| tree.parse_command(command));
            if cursor < 1 || (!self.suggestions.is_empty() && self.keep_suggestions) {
                return;
            }
            let local = tree.completions(parse, cursor - 1);
            let needs_server = local.needs_server;
            let local = SuggestionSet {
                range: local.start + 1..cursor,
                list: local
                    .options
                    .into_iter()
                    .map(ChatSuggestion::plain)
                    .collect(),
            };
            if needs_server {
                // `ClientSuggestionProvider.customSuggestion`.
                self.pending_suggestions_id += 1;
                let request = value[..cursor].to_owned();
                self.outgoing_request = Some((self.pending_suggestions_id as u32, request.clone()));
                self.pending_suggestions = Some(PendingSuggestions::Awaiting { request, local });
            } else {
                self.pending_suggestions = Some(PendingSuggestions::Done(local));
                self.update_usage_info(Some(tree));
            }
        } else if !java_is_blank(&value) {
            // TODO: vanilla completes non-blank messages from the server's
            // `ClientboundCustomChatCompletions` entries, which Pomme ignores.
            self.pending_suggestions = None;
            if !self.messages_allowed() {
                self.command_usage
                    .push(restricted_line("chat_screen.messages_not_allowed"));
            }
        } else {
            self.pending_suggestions = None;
        }
    }

    /// Vanilla `updateUsageInfo`, run once completions finish: the parse's
    /// errors or usage at the caret, then any restriction lines.
    pub(super) fn update_usage_info(&mut self, tree: Option<&CommandTree>) {
        if let Some(tree) = tree
            && let Some(parse) = &self.current_parse
            && let Some(PendingSuggestions::Done(set)) = &self.pending_suggestions
        {
            let value = self.input.value();
            let usage = tree.usage(
                parse,
                self.input.cursor().saturating_sub(1),
                set.list.is_empty(),
            );
            self.command_usage = usage
                .lines
                .iter()
                .map(|line| match line {
                    UsageLine::Usage(text) => {
                        vec![TextSpan::new(text.clone(), common::rgb(0xaaaaaa))]
                    }
                    UsageLine::Error(error) => {
                        format_component_spans(&syntax_error_component(value, error), common::WHITE)
                    }
                })
                .collect();
            if !self.commands_allowed() {
                self.command_usage
                    .push(restricted_line("chat_screen.commands_not_allowed"));
            }
            if parse.is_message() && !self.messages_allowed() {
                self.command_usage
                    .push(restricted_line("chat_screen.messages_not_allowed"));
            }
            self.command_usage_start = Some(usage.start + 1);
        }
        self.hide_suggestions();
        if self.allow_suggestions && self.options.auto_suggestions {
            self.show_suggestions();
        }
    }

    /// Vanilla `CommandSuggestions.showSuggestions`: list the finished
    /// completions, if any.
    pub(super) fn show_suggestions(&mut self) {
        let Some(PendingSuggestions::Done(set)) = &self.pending_suggestions else {
            return;
        };
        if set.list.is_empty() {
            return;
        }
        let value = self.input.value();
        let partial = &value[..self.input.cursor()];
        let last_word = partial[last_word_index(partial)..].to_lowercase();
        self.suggestions = sort_suggestions_with_partial_first(set.list.clone(), &last_word);
        self.suggest_range = set.range.clone();
        self.suggest_original = value.to_owned();
        self.suggest_offset = 0;
        self.tab_cycles = false;
        self.last_suggestion_cursor = None;
        self.select_suggestion(0);
    }

    /// Vanilla `SuggestionsList.select`, wrapping around either end.
    pub(super) fn select_suggestion(&mut self, index: isize) {
        let n = self.suggestions.len() as isize;
        if n > 0 {
            self.suggest_index = index.rem_euclid(n) as usize;
        }
    }

    /// Vanilla `SuggestionsList.cycle`.
    pub(super) fn cycle_suggestion(&mut self, direction: isize) {
        self.select_suggestion(self.suggest_index as isize + direction);
        self.keep_suggestion_visible();
    }

    /// Vanilla `SuggestionsList.useSuggestion`: replace the completed range of
    /// the list's original input, keeping the list open.
    pub(super) fn use_suggestion(
        &mut self,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
        tree: Option<&CommandTree>,
    ) {
        let Some(text) = self
            .suggestions
            .get(self.suggest_index)
            .map(|s| s.text.clone())
        else {
            return;
        };
        let Some(applied) = apply_suggestion(&self.suggest_original, &self.suggest_range, &text)
        else {
            return;
        };
        self.keep_suggestions = true;
        self.set_input_value(&applied, inner_w, width_fn, tree);
        let end = self.suggest_range.start + text.len();
        self.input.move_cursor_to(end, false, inner_w, width_fn);
        self.keep_suggestions = false;
        self.tab_cycles = true;
    }

    /// The tab-complete request queued by the last edit, if any: the input up
    /// to the caret, which the response's range indexes into.
    pub fn take_suggestion_request(&mut self) -> Option<(u32, String)> {
        self.outgoing_request.take()
    }

    /// Apply a `ClientboundCommandSuggestions` response. `start` is the
    /// UTF-16 offset into the sent request where the completed range begins;
    /// the range runs to the request's end. Only the latest request's id
    /// matches (vanilla `completeCustomSuggestions`), and an empty answer
    /// leaves the local literals.
    pub fn apply_server_suggestions(
        &mut self,
        id: u32,
        start: usize,
        options: Vec<ChatSuggestion>,
        tree: Option<&CommandTree>,
    ) {
        if id as i32 != self.pending_suggestions_id {
            return;
        }
        self.pending_suggestions_id = -1;
        let Some(PendingSuggestions::Awaiting { request, local }) = &self.pending_suggestions
        else {
            return;
        };
        let set = if options.is_empty() {
            local.clone()
        } else {
            let Some(start) = utf16_offset_to_byte(request, start) else {
                return;
            };
            SuggestionSet {
                range: start..request.len(),
                list: options,
            }
        };
        self.pending_suggestions = Some(PendingSuggestions::Done(set));
        self.update_usage_info(tree);
    }

    /// The grey inline completion: the rest of the selected entry applied to
    /// the list's original input, past the current value. Vanilla
    /// `CommandSuggestions.calculateSuggestionSuffix`, drawn only with the
    /// caret at the end.
    pub(super) fn ghost_suffix(&self) -> Option<String> {
        if !self.input.cursor_at_end() {
            return None;
        }
        let selected = self.suggestions.get(self.suggest_index)?;
        let applied =
            apply_suggestion(&self.suggest_original, &self.suggest_range, &selected.text)?;
        let suffix = applied.strip_prefix(self.input.value())?;
        (!suffix.is_empty()).then(|| suffix.to_owned())
    }

    /// The showing list's rectangle in GUI units: vanilla `showSuggestions`
    /// places it at the completed range's screen x, clamped on screen, and
    /// the `SuggestionsList` constructor anchors it above the input.
    pub(super) fn suggestion_rect(
        &self,
        screen_w: f32,
        screen_h: f32,
        gui_w: &dyn Fn(&str) -> f32,
    ) -> [f32; 4] {
        let max_w = self
            .suggestions
            .iter()
            .map(|s| gui_w(&s.text))
            .fold(0.0_f32, f32::max);
        let x = input_screen_x(
            &self.suggest_original,
            self.suggest_range.start,
            max_w,
            screen_w,
            gui_w,
        );
        let height = self.suggestion_window().0 as f32 * SUGGEST_ROW_H;
        // The unbordered input shifts the list one left.
        [x - 1.0, screen_h - 12.0 - 3.0 - height, max_w + 1.0, height]
    }

    /// Vanilla `SuggestionsList.extractRenderState`: hovering selects only
    /// when the mouse moved, and the selected entry's tooltip shows while
    /// any row is hovered.
    pub(super) fn push_suggestion_list(
        &mut self,
        elements: &mut Vec<MenuElement>,
        cursor: (f32, f32),
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        gui_w: &dyn Fn(&str) -> f32,
    ) {
        let (limit, max_offset) = self.suggestion_window();
        self.suggest_offset = self.suggest_offset.min(max_offset);
        let offset = self.suggest_offset;
        let [x, y, w, h] = self.suggestion_rect(screen_w / gs, screen_h / gs, gui_w);
        let fill = |elements: &mut Vec<MenuElement>, rect: [f32; 4], color| {
            push_fill(elements, rect.map(|v| v * gs), color);
        };
        let bg = black(SUGGEST_BG_ALPHA);
        let mouse = ((cursor.0 / gs).floor(), (cursor.1 / gs).floor());
        let mouse_moved = self.last_suggestion_cursor != Some(mouse);
        self.last_suggestion_cursor = Some(mouse);

        // Dotted edges mark entries scrolled out above or below.
        let has_previous = offset > 0;
        let has_next = offset < max_offset;
        if has_previous || has_next {
            fill(elements, [x, y - 1.0, w, 1.0], bg);
            fill(elements, [x, y + h, w, 1.0], bg);
            for (dotted, edge_y) in [(has_previous, y - 1.0), (has_next, y + h)] {
                if dotted {
                    for dot in (0..w as usize).step_by(2) {
                        fill(elements, [x + dot as f32, edge_y, 1.0, 1.0], common::WHITE);
                    }
                }
            }
        }

        let mut hovered = false;
        for row in 0..limit {
            let index = row + offset;
            let row_y = y + row as f32 * SUGGEST_ROW_H;
            let row_rect = [x, row_y, w, SUGGEST_ROW_H];
            fill(elements, row_rect, bg);
            self.suggestion_regions
                .push((index, row_rect.map(|v| v * gs)));
            if mouse.0 > x && mouse.0 < x + w && mouse.1 > row_y && mouse.1 < row_y + SUGGEST_ROW_H
            {
                if mouse_moved {
                    self.select_suggestion(index as isize);
                }
                hovered = true;
            }
            elements.push(MenuElement::Text {
                x: (x + 1.0) * gs,
                y: (row_y + 2.0) * gs,
                text: self.suggestions[index].text.clone(),
                scale: common::FONT_SIZE * gs,
                color: if index == self.suggest_index {
                    SUGGEST_SELECTED
                } else {
                    SUGGEST_TEXT
                },
                centered: false,
            });
        }
        if hovered && let Some(tooltip) = &self.suggestions[self.suggest_index].tooltip {
            let lines = component_tooltip_lines(tooltip);
            common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
        }
    }

    /// Vanilla `CommandSuggestions.extractUsage` over the lines
    /// `updateCommandInfo`/`updateUsageInfo` collected.
    pub(super) fn push_usage(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        gui_w: &dyn Fn(&str) -> f32,
        spans_w: &dyn Fn(&[TextSpan]) -> f32,
    ) {
        let lines = &self.command_usage;
        if lines.is_empty() {
            return;
        }
        let width = lines
            .iter()
            .map(|line| spans_w(line))
            .fold(0.0_f32, f32::max);
        let x = self.command_usage_start.map_or(0.0, |start| {
            input_screen_x(self.input.value(), start, width, screen_w / gs, gui_w)
        });
        for (index, spans) in lines.iter().enumerate() {
            let y = screen_h / gs - 27.0 - SUGGEST_ROW_H * index as f32;
            push_fill(
                elements,
                [x - 1.0, y, width + 2.0, SUGGEST_ROW_H].map(|v| v * gs),
                black(SUGGEST_BG_ALPHA),
            );
            elements.push(MenuElement::McText {
                x: x * gs,
                y: (y + 2.0) * gs,
                spans: spans.clone(),
                scale: common::FONT_SIZE * gs,
                centered: false,
                shadow: true,
            });
        }
    }
}
/// Vanilla `EditBox.getScreenX` of byte `start` of `input`, in gui units,
/// clamped so a `width`-wide box stays on screen.
pub(super) fn input_screen_x(
    input: &str,
    start: usize,
    width: f32,
    screen_w: f32,
    gui_w: &dyn Fn(&str) -> f32,
) -> f32 {
    let screen_x = INPUT_X + input.get(..start).map_or(0.0, gui_w);
    screen_x.max(0.0).min(screen_w - width)
}

/// Byte offset for a UTF-16 code-unit offset (Java's `StringRange` counts
/// UTF-16 units). `None` if it lands mid-char or past the end.
pub(super) fn utf16_offset_to_byte(s: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units == utf16 {
            return Some(i);
        }
        units += c.len_utf16();
    }
    (units == utf16).then_some(s.len())
}

/// Brigadier `Suggestion.apply`: `text` in place of `range` of `input`.
pub(super) fn apply_suggestion(input: &str, range: &Range<usize>, text: &str) -> Option<String> {
    Some(format!(
        "{}{text}{}",
        input.get(..range.start)?,
        input.get(range.end..)?
    ))
}

/// Vanilla `CommandSuggestions.getLastWordIndex`: just past the last `\s+`
/// run.
pub(super) fn last_word_index(text: &str) -> usize {
    text.rfind([' ', '\t', '\n', '\u{b}', '\u{c}', '\r'])
        .map_or(0, |i| i + 1)
}

/// ChatScreen's red commands/messages-not-allowed usage line.
pub(super) fn restricted_line(key: &str) -> Vec<TextSpan> {
    format_component_spans(
        &colored(Component::translate(key, Vec::new()), 0xff5555),
        common::WHITE,
    )
}

/// Vanilla `CommandSuggestions.getExceptionMessage` for a parse of the
/// command in `input` (which keeps its `/`).
pub(super) fn syntax_error_component(input: &str, error: &SyntaxError) -> Component {
    let message = Component::translate(
        error.key,
        error.args.iter().cloned().map(Argument::String).collect(),
    );
    match error.cursor {
        Some(cursor) => parse_error_component(input, cursor + 1, message),
        None => message,
    }
}

/// `command.context.parse_error` around `message` for an exception at byte
/// `cursor` of `input`, with Brigadier's `CommandSyntaxException.getContext`.
pub(super) fn parse_error_component(input: &str, cursor: usize, message: Component) -> Component {
    const CONTEXT_AMOUNT: usize = 10;
    let before = input.get(..cursor).unwrap_or(input);
    let position = utf16_len(before);
    let mut units = 0;
    let mut tail = before.len();
    for (i, c) in before.char_indices().rev() {
        units += c.len_utf16();
        if units > CONTEXT_AMOUNT {
            break;
        }
        tail = i;
    }
    let ellipsis = if position > CONTEXT_AMOUNT { "..." } else { "" };
    Component::translate(
        "command.context.parse_error",
        vec![
            Argument::Component(Box::new(message)),
            Argument::Number(position.to_string()),
            Argument::String(format!("{ellipsis}{}<--[HERE]", &before[tail..])),
        ],
    )
}

pub(super) fn sort_suggestions_with_partial_first(
    options: Vec<ChatSuggestion>,
    partial: &str,
) -> Vec<ChatSuggestion> {
    let namespaced = format!("minecraft:{partial}");
    let (mut hits, misses): (Vec<ChatSuggestion>, Vec<ChatSuggestion>) = options
        .into_iter()
        .partition(|s| s.text.starts_with(partial) || s.text.starts_with(&namespaced));
    hits.extend(misses);
    hits
}

pub(super) fn command_input_spans(input: &str, tokens: &[CommandTokenRange]) -> Vec<TextSpan> {
    const ARGUMENT_COLORS: [[f32; 4]; 5] = [
        [0x55 as f32 / 255.0, 1.0, 1.0, 1.0],
        [1.0, 1.0, 0x55 as f32 / 255.0, 1.0],
        [0x55 as f32 / 255.0, 1.0, 0x55 as f32 / 255.0, 1.0],
        [1.0, 0x55 as f32 / 255.0, 1.0, 1.0],
        [1.0, 0xaa as f32 / 255.0, 0.0, 1.0],
    ];
    let literal = common::rgb(0xaaaaaa);
    let unparsed = common::rgb(0xff5555);
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for token in tokens {
        let start = token.range.start.saturating_add(1).min(input.len());
        let end = token.range.end.saturating_add(1).min(input.len());
        if cursor < start {
            out.push(TextSpan::new(input[cursor..start].to_owned(), literal));
        }
        if start < end {
            let color = match token.kind {
                CommandTokenKind::Argument(index) => ARGUMENT_COLORS[index % ARGUMENT_COLORS.len()],
                CommandTokenKind::Unparsed => unparsed,
            };
            out.push(TextSpan::new(input[start..end].to_owned(), color));
        }
        cursor = end;
    }
    if cursor < input.len() {
        out.push(TextSpan::new(input[cursor..].to_owned(), literal));
    }
    if out.is_empty() {
        out.push(TextSpan::new(input.to_owned(), literal));
    }
    out
}
