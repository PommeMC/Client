use std::cell::OnceCell;
use std::collections::VecDeque;
use std::time::Instant;

use super::common;
use crate::net::commands::CommandTree;
use crate::net::sender::ChatMark;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::text::TextSpan;
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

/// Vanilla caps messages, wrapped lines and sent history at 100 each.
const MAX_MESSAGES: usize = 100;
const CHAT_X: f32 = 4.0;
const CHAT_WIDTH: f32 = 320.0;
const MESSAGE_INDENT: f32 = 4.0;
const BOTTOM_MARGIN: f32 = 40.0;
const LINE_HEIGHT: f32 = 9.0;
const LINES_PER_PAGE: usize = 10;
const MESSAGE_LIFETIME_SECS: f32 = 10.0;
const INPUT_HEIGHT: f32 = 12.0;
const MAX_MESSAGE_LEN: usize = 256;

const TEXT_OPACITY: f32 = 1.0;
const BACKGROUND_OPACITY: f32 = 0.5;
const INPUT_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

const SUGGEST_ROW_H: f32 = 12.0;
const MAX_SUGGESTION_ROWS: usize = 10;
const SUGGEST_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.816];
const SUGGEST_TEXT: [f32; 4] = [0.667, 0.667, 0.667, 1.0];
const SUGGEST_SELECTED: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
// Vanilla EditBox suggestion color, 0xFF808080.
const GHOST_TEXT: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
// Vanilla EditBox caret color, 0xFFD0D0D0.
const CARET_COLOR: [f32; 4] = [0.816, 0.816, 0.816, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChatVisibilitySetting {
    Full,
    System,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChatOptions {
    pub visibility: ChatVisibilitySetting,
    pub opacity: f32,
    pub line_spacing: f32,
    pub text_background_opacity: f32,
    pub scale: f32,
    pub width: f32,
    pub height_focused: f32,
    pub height_unfocused: f32,
    pub delay_secs: f32,
    pub colors: bool,
    pub links: bool,
    pub links_prompt: bool,
    pub auto_suggestions: bool,
    pub only_secure: bool,
    pub save_drafts: bool,
}

// TODO: connections send these defaults until the chat settings are
// configurable.
impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            visibility: ChatVisibilitySetting::Full,
            opacity: 1.0,
            line_spacing: 0.0,
            text_background_opacity: 0.5,
            scale: 1.0,
            width: 1.0,
            height_focused: 1.0,
            height_unfocused: 70.0 / 160.0,
            delay_secs: 0.0,
            colors: true,
            links: true,
            links_prompt: true,
            auto_suggestions: true,
            only_secure: false,
            save_drafts: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMessageSource {
    Player,
    SystemServer,
    SystemClient,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatMessageTag {
    System,
    SystemSinglePlayer,
    NotSecure,
    Modified { original: String },
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatSuggestion {
    pub text: String,
    pub tooltip: Option<crate::chat_component::Component>,
}

struct ChatLine {
    spans: Vec<TextSpan>,
    received: Instant,
    signature: Option<[u8; 256]>,
    // TODO: draw the tag indicator bar and tooltip (vanilla `handleTag`).
    tag: Option<ChatMessageTag>,
    /// Lazily wrapped display lines (vanilla `trimmedMessages`), kept like
    /// vanilla's, which only re-wraps when a chat option changes.
    wrapped: OnceCell<Vec<Vec<TextSpan>>>,
}

impl ChatLine {
    fn wrapped(&self, width0: &dyn Fn(&[TextSpan]) -> f32) -> &[Vec<TextSpan>] {
        self.wrapped
            .get_or_init(|| wrap_chat_spans(&self.spans, CHAT_WIDTH, width0))
    }
}

pub struct ChatState {
    options: ChatOptions,
    messages: VecDeque<ChatLine>,
    input: TextFieldState,
    open: bool,
    /// Sent messages for Up/Down recall (vanilla `recentChat`): consecutive
    /// duplicates collapse, capped at 100.
    sent_history: VecDeque<String>,
    /// Index into `sent_history`; `len()` means "the draft" (vanilla
    /// `historyPos`).
    history_pos: usize,
    /// The in-progress draft, saved while browsing history (vanilla
    /// `historyBuffer`).
    history_buffer: String,
    /// Wrapped lines scrolled up from the bottom (vanilla `chatScrollbarPos`);
    /// clamped against the wrapped total in `build`.
    scroll_pos: usize,
    /// Newest messages added while the chat was open and scrolled, whose
    /// wrapped lines `build` still has to add to `scroll_pos` (vanilla scrolls
    /// one line per wrapped line).
    scroll_anchor_pending: usize,
    suggestions: Vec<String>,
    suggest_index: usize,
    suggest_anchor: String,
    suggest_applied: bool,
    /// Vanilla `setAllowSuggestions(false)` after a history recall: the popup
    /// stays hidden until the next real edit.
    allow_suggestions: bool,
    last_computed: String,
    /// Monotonic tab-complete transaction id (vanilla `pendingSuggestionsId`).
    /// Never reset, so a response from a previous chat session can't match.
    next_suggest_id: u32,
    /// Id and input snapshot of the in-flight server request; a response is
    /// applied only if both still match.
    awaiting: Option<(u32, String)>,
    /// Request produced by the last recompute, drained once per frame by the
    /// game loop and sent as `ServerboundCommandSuggestion`.
    outgoing_request: Option<(u32, String)>,
    delayed_deletions: Vec<([u8; 256], Instant)>,
    /// Last-seen updates in the order they happened, for the network loop.
    chat_marks: Vec<ChatMark>,
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            options: ChatOptions::default(),
            messages: VecDeque::new(),
            input: TextFieldState::new(MAX_MESSAGE_LEN),
            open: false,
            sent_history: VecDeque::new(),
            history_pos: 0,
            history_buffer: String::new(),
            scroll_pos: 0,
            scroll_anchor_pending: 0,
            suggestions: Vec::new(),
            suggest_index: 0,
            suggest_anchor: String::new(),
            suggest_applied: false,
            allow_suggestions: true,
            last_computed: String::new(),
            next_suggest_id: 0,
            awaiting: None,
            outgoing_request: None,
            delayed_deletions: Vec::new(),
            chat_marks: Vec::new(),
        }
    }

    pub fn only_secure(&self) -> bool {
        self.options.only_secure
    }

    /// Vanilla `addClientSystemMessage`.
    pub fn push_message(&mut self, spans: Vec<TextSpan>) {
        self.push_message_with_source(
            spans,
            None,
            ChatMessageSource::SystemClient,
            Some(ChatMessageTag::SystemSinglePlayer),
        );
    }

    pub fn push_message_with_source(
        &mut self,
        spans: Vec<TextSpan>,
        signature: Option<[u8; 256]>,
        source: ChatMessageSource,
        tag: Option<ChatMessageTag>,
    ) {
        let visible = match source {
            ChatMessageSource::SystemClient => true,
            ChatMessageSource::SystemServer => {
                self.options.visibility != ChatVisibilitySetting::Hidden
            }
            ChatMessageSource::Player => self.options.visibility == ChatVisibilitySetting::Full,
        } && !(self.options.only_secure
            && source == ChatMessageSource::Player
            && matches!(tag, Some(ChatMessageTag::NotSecure)));
        if !visible {
            self.mark_processed(signature, false);
            return;
        }
        self.messages.push_back(ChatLine {
            spans,
            received: Instant::now(),
            signature,
            tag,
            wrapped: OnceCell::new(),
        });
        if self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        if self.open && self.scroll_pos > 0 {
            self.scroll_anchor_pending += 1;
        }
        self.mark_processed(signature, true);
    }

    pub fn push_validation_error(
        &mut self,
        spans: Vec<TextSpan>,
        invalid_signature: Option<[u8; 256]>,
    ) {
        self.push_message_with_source(
            spans,
            None,
            ChatMessageSource::Player,
            Some(ChatMessageTag::Error),
        );
        self.mark_processed(invalid_signature, false);
    }

    /// Vanilla `markMessageAsProcessed`.
    pub fn mark_processed(&mut self, signature: Option<[u8; 256]>, shown: bool) {
        if let Some(signature) = signature {
            self.chat_marks
                .push(ChatMark::Processed { signature, shown });
        }
    }

    /// Vanilla `LastSeenMessagesTracker.ignorePending` on a deletion.
    pub fn ignore_pending(&mut self, signature: [u8; 256]) {
        self.chat_marks.push(ChatMark::Deleted { signature });
    }

    pub fn take_chat_marks(&mut self) -> Vec<ChatMark> {
        std::mem::take(&mut self.chat_marks)
    }

    pub fn delete_message(&mut self, signature: [u8; 256]) {
        if let Some(deletable_after) = self.delete_message_or_delay(signature, Instant::now()) {
            self.delayed_deletions.push((signature, deletable_after));
        }
    }

    /// Vanilla `deleteMessageOrDelay`: when the message is too new to delete,
    /// the time it becomes deletable (60 ticks after it was added).
    fn delete_message_or_delay(&mut self, signature: [u8; 256], now: Instant) -> Option<Instant> {
        let line = self
            .messages
            .iter_mut()
            .find(|line| line.signature.as_ref() == Some(&signature))?;
        let deletable_after = line.received + std::time::Duration::from_secs(3);
        if now < deletable_after {
            return Some(deletable_after);
        }
        let mut marker = TextSpan::new(
            crate::lang::translate("chat.deleted_marker")
                .unwrap_or("<message deleted>")
                .to_owned(),
            common::rgb(0xaaaaaa),
        );
        marker.italic = true;
        line.spans = vec![marker];
        line.signature = None;
        line.tag = Some(ChatMessageTag::System);
        line.wrapped = OnceCell::new();
        None
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let mut queue = std::mem::take(&mut self.delayed_deletions);
        queue.retain(|&(signature, deletable_after)| {
            now < deletable_after || self.delete_message_or_delay(signature, now).is_some()
        });
        self.delayed_deletions = queue;
    }

    /// F3+D; vanilla `clearMessages(false)` keeps the sent-message history.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.scroll_anchor_pending = 0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.input.set_value("", f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
    }

    pub fn open_with_slash(&mut self) {
        self.open = true;
        self.input.set_value("/", f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.input.set_focused(false);
        self.clear_suggestions();
        // Vanilla resets the chat scroll when the screen closes.
        self.scroll_pos = 0;
        self.scroll_anchor_pending = 0;
    }

    /// Scroll the message backlog by wrapped lines; positive is up (vanilla
    /// `scrollChat`). The upper clamp happens in `build`, where wrap counts
    /// are known.
    pub fn scroll_chat(&mut self, delta: i32) {
        if self.open {
            self.scroll_pos = self.scroll_pos.saturating_add_signed(delta as isize);
        }
    }

    /// Up/Down sent-message recall, vanilla `ChatScreen.moveInHistory`.
    fn move_in_history(&mut self, delta: i32, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        let end = self.sent_history.len();
        let target = self
            .history_pos
            .saturating_add_signed(delta as isize)
            .min(end);
        if target == self.history_pos {
            return;
        }
        if target == end {
            let draft = std::mem::take(&mut self.history_buffer);
            self.input.set_value(&draft, inner_w, width_fn);
        } else {
            if self.history_pos == end {
                self.history_buffer = self.input.value().to_string();
            }
            let entry = self.sent_history[target].clone();
            self.input.set_value(&entry, inner_w, width_fn);
        }
        self.history_pos = target;
        self.allow_suggestions = false;
    }

    /// Vanilla `ChatComponent.addRecentChat`: consecutive duplicates collapse.
    fn add_recent_chat(&mut self, msg: &str) {
        if self.sent_history.back().map(String::as_str) != Some(msg) {
            self.sent_history.push_back(msg.to_string());
            if self.sent_history.len() > MAX_MESSAGES {
                self.sent_history.pop_front();
            }
        }
    }

    fn clear_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggest_anchor.clear();
        self.suggest_index = 0;
        self.suggest_applied = false;
        self.last_computed.clear();
        self.awaiting = None;
        self.outgoing_request = None;
    }

    /// Recompute command completions from the current input. Only command input
    /// (leading `/`) yields suggestions; anything else clears them. Local
    /// literals show immediately; argument positions also queue a server
    /// request whose response replaces them (vanilla requests per keystroke
    /// with latest-id-wins, no debounce).
    fn recompute_suggestions(&mut self, tree: Option<&CommandTree>) {
        self.clear_suggestions();
        let input = self.input.value().to_string();
        self.last_computed = input.clone();
        if let Some(cmd) = input.strip_prefix('/')
            && let Some(tree) = tree
        {
            let sug = tree.suggestions(cmd);
            let cut = input.len() - sug.partial_len;
            self.suggest_anchor = input[..cut].to_string();
            self.suggestions = sug.options;
            if sug.needs_server {
                self.next_suggest_id = self.next_suggest_id.wrapping_add(1);
                let request = (self.next_suggest_id, input);
                self.awaiting = Some(request.clone());
                self.outgoing_request = Some(request);
            }
        }
    }

    /// The tab-complete request queued by the last recompute, if any. The
    /// command is the full input including the leading `/`, matching what
    /// vanilla sends (the response range indexes into that exact string).
    pub fn take_suggestion_request(&mut self) -> Option<(u32, String)> {
        self.outgoing_request.take()
    }

    /// Apply a `ClientboundCommandSuggestions` response. `start` is the offset
    /// into the sent command string where the completed range begins. Stale
    /// responses (id or input no longer matching) are dropped; an empty
    /// response keeps the local literal suggestions.
    pub fn apply_server_suggestions(&mut self, id: u32, start: usize, options: Vec<String>) {
        let Some((want_id, want_input)) = &self.awaiting else {
            return;
        };
        if id != *want_id || !self.open || self.input.value() != *want_input {
            return;
        }
        self.awaiting = None;
        if options.is_empty() {
            return;
        }
        // Java's StringRange counts UTF-16 units, not bytes.
        let Some(start) = utf16_offset_to_byte(self.input.value(), start) else {
            return;
        };
        let partial = self.input.value()[start..].to_ascii_lowercase();
        self.suggest_anchor = self.input.value()[..start].to_string();
        self.suggestions = sort_with_partial_first(options, &partial);
        self.suggest_index = 0;
        self.suggest_applied = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_key_input(
        &mut self,
        events: &[TextInputEvent],
        enter: bool,
        tab: bool,
        shift: bool,
        up: bool,
        down: bool,
        page_up: bool,
        page_down: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
        tree: Option<&CommandTree>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }

        // Up/Down cycle the suggestion popup when it's showing, else recall
        // sent-message history (vanilla CommandSuggestions gets keys first).
        if up || down {
            if !self.suggestions.is_empty() {
                let n = self.suggestions.len();
                self.suggest_index = if up {
                    (self.suggest_index + n - 1) % n
                } else {
                    (self.suggest_index + 1) % n
                };
            } else if up {
                self.move_in_history(-1, inner_w, width_fn);
            } else {
                self.move_in_history(1, inner_w, width_fn);
            }
        }
        if page_up {
            self.scroll_chat(LINES_PER_PAGE as i32 - 1);
        }
        if page_down {
            self.scroll_chat(-(LINES_PER_PAGE as i32 - 1));
        }

        let mut clipboard = SystemClipboard;
        let before_edits = self.input.value().to_string();
        for ev in events {
            self.input.handle(ev, &mut clipboard, inner_w, width_fn);
        }
        // A real edit re-enables the popup (vanilla `onEdited`).
        if self.input.value() != before_edits {
            self.allow_suggestions = true;
        }

        // Tab applies the highlighted completion; further Tabs cycle the list.
        if tab && !self.suggestions.is_empty() {
            let n = self.suggestions.len();
            if self.suggest_applied {
                self.suggest_index = if shift {
                    (self.suggest_index + n - 1) % n
                } else {
                    (self.suggest_index + 1) % n
                };
            }
            let applied = format!(
                "{}{}",
                self.suggest_anchor, self.suggestions[self.suggest_index]
            );
            self.input.set_value(&applied, inner_w, width_fn);
            self.suggest_applied = true;
            self.last_computed = applied;
            return None;
        }

        if self.input.value() != self.last_computed {
            // While suppressed, recompute without the tree: clears the popup.
            self.recompute_suggestions(tree.filter(|_| self.allow_suggestions));
        }

        if enter {
            let normalized = normalize_chat_message(self.input.value());
            let msg = if normalized.is_empty() {
                None
            } else {
                self.add_recent_chat(&normalized);
                Some(normalized)
            };
            self.input.set_value("", inner_w, width_fn);
            self.close();
            return msg;
        }

        None
    }

    /// The grey inline completion: the remainder of the selected suggestion
    /// past what is already typed. Mirrors vanilla
    /// `CommandSuggestions.calculateSuggestionSuffix` (case-sensitive; the
    /// cursor is always at the end of pomme's chat input).
    fn ghost_suffix(&self) -> Option<&str> {
        // Vanilla only shows the inline suffix with the cursor at the end.
        if !self.input.cursor_at_end() {
            return None;
        }
        let selected = self.suggestions.get(self.suggest_index)?;
        let rest = self
            .input
            .value()
            .strip_prefix(self.suggest_anchor.as_str())?;
        let suffix = selected.strip_prefix(rest)?;
        (!suffix.is_empty()).then_some(suffix)
    }

    /// Wrapped lines newest-first, capped at 100 like vanilla
    /// `trimmedMessages`, each with its message.
    fn trimmed_lines<'a>(
        &'a self,
        width0: &'a dyn Fn(&[TextSpan]) -> f32,
    ) -> impl Iterator<Item = (&'a ChatLine, &'a Vec<TextSpan>)> {
        self.messages
            .iter()
            .rev()
            .flat_map(move |msg| {
                msg.wrapped(width0)
                    .iter()
                    .rev()
                    .map(move |line| (msg, line))
            })
            .take(MAX_MESSAGES)
    }

    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
        spans_width_fn: common::SpansWidthFn<'_>,
    ) {
        let now = Instant::now();
        let fs = common::FONT_SIZE * gs;
        let lh = LINE_HEIGHT * gs;
        let chat_w = CHAT_WIDTH * gs;
        let origin = CHAT_X * gs;
        let indent = MESSAGE_INDENT * gs;
        let bg_w = chat_w + 2.0 * indent;
        let chat_bottom = screen_h - BOTTOM_MARGIN * gs;
        // Measure wrapping at gui-scale 1 so wrap points stay fixed when the
        // gui scale changes (vanilla wraps in gui-space, then scales).
        let width0 = |spans: &[TextSpan]| spans_width_fn(spans, common::FONT_SIZE);

        let added: usize = self
            .messages
            .iter()
            .rev()
            .take(std::mem::take(&mut self.scroll_anchor_pending))
            .map(|m| m.wrapped(&width0).len())
            .sum();
        self.scroll_pos += added;

        // Clamp the scroll to the wrapped backlog (vanilla scrollChat clamps
        // against `trimmedMessages`).
        // TODO: vanilla clamps per added line and never re-clamps after
        // trimming, so a multi-line message on a full backlog can overshoot.
        if self.open && self.scroll_pos > 0 {
            let total = self.trimmed_lines(&width0).count();
            self.scroll_pos = self.scroll_pos.min(total.saturating_sub(LINES_PER_PAGE));
        }

        // Gather the visible wrapped lines newest-first; index 0 is the
        // bottom-most line, `scroll_pos` lines skipped below it. All wrapped
        // lines of a message share its alpha.
        let mut display: Vec<(Vec<TextSpan>, f32)> = Vec::new();
        for (msg, line) in self
            .trimmed_lines(&width0)
            .skip(self.scroll_pos)
            .take(LINES_PER_PAGE)
        {
            let alpha = if self.open {
                1.0
            } else {
                line_alpha(now.duration_since(msg.received).as_secs_f32())
            };
            if alpha > 1e-5 {
                display.push((line.clone(), alpha));
            }
        }

        for (i, (line_spans, alpha)) in display.iter().enumerate() {
            let entry_bottom = chat_bottom - (i as f32) * lh;
            let entry_top = entry_bottom - lh;
            let bg_a = alpha * BACKGROUND_OPACITY;
            if bg_a > 1e-5 {
                elements.push(MenuElement::Rect {
                    x: origin,
                    y: entry_top,
                    w: bg_w,
                    h: lh,
                    corner_radius: 0.0,
                    color: [0.0, 0.0, 0.0, bg_a],
                });
            }
            let text_a = alpha * TEXT_OPACITY;
            let faded: Vec<TextSpan> = line_spans
                .iter()
                .map(|s| {
                    let mut s = s.clone();
                    s.color[3] *= text_a;
                    s
                })
                .collect();
            elements.push(MenuElement::McText {
                x: origin + indent,
                y: entry_top + (lh - fs) / 2.0,
                spans: faded,
                scale: fs,
                centered: false,
                shadow: true,
            });
        }

        if self.open {
            let input_h = INPUT_HEIGHT * gs;
            // Vanilla pins the input as a full-width bar at the very bottom of
            // the screen: fill(2, height-14, width-2, height-2).
            let bar_y = screen_h - 14.0 * gs;
            let text_y = bar_y + (input_h - fs) / 2.0;

            elements.push(MenuElement::Rect {
                x: 2.0 * gs,
                y: bar_y,
                w: screen_w - 4.0 * gs,
                h: input_h,
                corner_radius: 0.0,
                color: INPUT_BG,
            });

            let text_x = origin + indent;
            let inner_w = screen_w - text_x - 4.0 * gs;
            let wf = |s: &str| text_width_fn(s, fs);
            let info = self.input.render_info(inner_w, true, &wf);
            let shown = &self.input.value()[info.display_start..info.display_end];

            // The ghost is the inline suggestion suffix, shown only while the
            // caret sits at the end of the input.
            common::push_field_text(
                elements,
                &info,
                shown,
                text_x,
                text_y,
                fs,
                gs,
                gs,
                CARET_COLOR,
                self.ghost_suffix().map(|g| (g, GHOST_TEXT)),
                &wf,
            );

            if !self.suggestions.is_empty() {
                let row_h = SUGGEST_ROW_H * gs;
                let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
                let max_offset = self.suggestions.len() - visible;
                let offset = self
                    .suggest_index
                    .saturating_sub(visible - 1)
                    .min(max_offset);

                let pad = gs;
                let max_w = self.suggestions[offset..offset + visible]
                    .iter()
                    .map(|s| text_width_fn(s, fs))
                    .fold(0.0_f32, f32::max);
                let popup_w = max_w + 2.0 * pad;
                let anchor_w = text_width_fn(&self.suggest_anchor, fs);
                let popup_x =
                    (origin + indent + anchor_w).min((screen_w - 2.0 * gs - popup_w).max(2.0 * gs));
                let popup_top = bar_y - gs - visible as f32 * row_h;

                for i in 0..visible {
                    let idx = offset + i;
                    let row_y = popup_top + i as f32 * row_h;
                    elements.push(MenuElement::Rect {
                        x: popup_x,
                        y: row_y,
                        w: popup_w,
                        h: row_h,
                        corner_radius: 0.0,
                        color: SUGGEST_BG,
                    });
                    elements.push(MenuElement::Text {
                        x: popup_x + pad,
                        y: row_y + (row_h - fs) / 2.0,
                        text: self.suggestions[idx].clone(),
                        scale: fs,
                        color: if idx == self.suggest_index {
                            SUGGEST_SELECTED
                        } else {
                            SUGGEST_TEXT
                        },
                        centered: false,
                    });
                }
            }
        }
    }
}

/// Byte offset for a UTF-16 code-unit offset (Java's `StringRange` counts
/// UTF-16 units). `None` if it lands mid-char or past the end.
fn utf16_offset_to_byte(s: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units == utf16 {
            return Some(i);
        }
        units += c.len_utf16();
    }
    (units == utf16).then_some(s.len())
}

/// Float suggestions matching the typed partial (or its `minecraft:`-prefixed
/// form) to the front, keeping order otherwise. Mirrors vanilla
/// `CommandSuggestions.sortSuggestions`; `partial` must be lowercased.
fn sort_with_partial_first(options: Vec<String>, partial: &str) -> Vec<String> {
    let namespaced = format!("minecraft:{partial}");
    let (mut hits, misses): (Vec<String>, Vec<String>) = options
        .into_iter()
        .partition(|s| s.starts_with(partial) || s.starts_with(&namespaced));
    hits.extend(misses);
    hits
}

/// Trim ends, collapse internal whitespace runs to single spaces, and clamp to
/// the max message length. Mirrors vanilla `ChatScreen.normalizeChatMessage`.
/// A leading `/` is preserved so commands still route correctly downstream.
fn normalize_chat_message(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut units = 0;
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        units += c.len_utf16();
        if units > MAX_MESSAGE_LEN {
            // Java's `substring` keeps a split pair's high surrogate, which
            // encodes as `?`.
            if units == MAX_MESSAGE_LEN + 1 && c.len_utf16() == 2 {
                out.push('?');
            }
            break;
        }
        out.push(c);
    }
    out
}

/// Time-based fade for a closed-chat line. Matches vanilla
/// `ChatComponent.AlphaCalculator.timeBased`: full opacity until ~90% of the
/// lifetime, then a squared fade over the final ~10%.
fn line_alpha(age_secs: f32) -> f32 {
    let mut t = 1.0 - age_secs / MESSAGE_LIFETIME_SECS;
    t *= 10.0;
    t = t.clamp(0.0, 1.0);
    t * t
}

/// A span's formatting, carried per character with its text left empty.
#[derive(Clone, PartialEq)]
struct CharStyle(TextSpan);

type StyledLine = Vec<(char, CharStyle)>;

/// Styled text wrapped to `max_w` gui-space units, one `Vec<TextSpan>` per
/// display line. Mirrors vanilla `Font.split` over a `FormattedText`.
pub(crate) fn wrap_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    split_lines(spans, max_w, width0)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

/// `wrap_spans` with a one-space indent before every width-wrapped line.
/// Mirrors vanilla `ComponentRenderUtils.wrapComponents`.
fn wrap_chat_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    split_lines(spans, max_w, width0)
        .into_iter()
        .map(|(mut line, wrapped)| {
            if wrapped {
                line.insert(0, TextSpan::new(" ".into(), common::WHITE));
            }
            line
        })
        .collect()
}

/// Vanilla `StringSplitter.splitLines`: each display line with whether it
/// continues a width wrap rather than starting the text or following a `\n`.
/// A trailing `\n` leaves a final empty line.
fn split_lines(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<(Vec<TextSpan>, bool)> {
    let mut paragraphs: Vec<Vec<TextSpan>> = vec![Vec::new()];
    for s in spans {
        for (i, part) in s.text.split('\n').enumerate() {
            if i > 0 {
                paragraphs.push(Vec::new());
            }
            if !part.is_empty() {
                paragraphs
                    .last_mut()
                    .unwrap()
                    .push(s.with_text(part.into()));
            }
        }
    }
    paragraphs
        .iter()
        .flat_map(|p| {
            wrap_words(p, max_w, width0)
                .into_iter()
                .enumerate()
                .map(|(i, line)| (line, i > 0))
        })
        .collect()
}

/// Greedy word-wrap of one `\n`-free paragraph, preserving each character's
/// color/style and hard-breaking any single word wider than the line.
/// `width0` measures styled spans at gui-scale 1, so fonts, bold and inline
/// objects count like vanilla `StringSplitter`.
// TODO: vanilla `LineBreakFinder` keeps whitespace runs and breaks at the
// last space before the overflowing char; this collapses whitespace.
fn wrap_words(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    let width = |chars: &[(char, CharStyle)]| width0(&merge_chars(chars));
    // Split into whitespace-delimited words, keeping each character's style.
    let mut words: Vec<StyledLine> = Vec::new();
    let mut word: StyledLine = Vec::new();
    for s in spans {
        let style = CharStyle(s.with_text(String::new()));
        for ch in s.text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                word.push((ch, style.clone()));
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines: Vec<StyledLine> = Vec::new();
    let mut cur: StyledLine = Vec::new();
    for w in words {
        if !cur.is_empty() {
            let mut joined = cur.clone();
            joined.push((' ', w[0].1.clone()));
            joined.extend(w.iter().cloned());
            if width(&joined) <= max_w {
                cur = joined;
                continue;
            }
            lines.push(std::mem::take(&mut cur));
        }
        // cur is empty here: start a fresh line, hard-breaking an oversized word.
        if width(&w) <= max_w {
            cur = w;
        } else {
            let (broken, rem) = hard_break_word(&w, max_w, width0);
            lines.extend(broken);
            cur = rem;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }

    lines.iter().map(|l| merge_chars(l)).collect()
}

/// Split a single word wider than `max_w` into pieces no wider than the line,
/// returning the completed pieces and the trailing remainder.
fn hard_break_word(
    word: &[(char, CharStyle)],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> (Vec<StyledLine>, StyledLine) {
    let mut out: Vec<StyledLine> = Vec::new();
    let mut piece: StyledLine = Vec::new();
    for entry in word {
        piece.push(entry.clone());
        if width0(&merge_chars(&piece)) > max_w && piece.len() > 1 {
            let last = piece.pop().expect("piece has the new char");
            out.push(std::mem::replace(&mut piece, vec![last]));
        }
    }
    (out, piece)
}

/// Coalesce a run of styled characters into `TextSpan`s, merging neighbours
/// that share the same style.
fn merge_chars(chars: &[(char, CharStyle)]) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut last_style: Option<CharStyle> = None;
    for (ch, st) in chars {
        if last_style.as_ref() == Some(st) {
            spans.last_mut().unwrap().text.push(*ch);
        } else {
            spans.push(st.0.with_text(ch.to_string()));
            last_style = Some(st.clone());
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, color: [f32; 4]) -> TextSpan {
        TextSpan::new(text.to_string(), color)
    }

    fn line_text(line: &[TextSpan]) -> String {
        line.iter().map(|s| s.text.clone()).collect()
    }

    fn line_texts(lines: &[Vec<TextSpan>]) -> Vec<String> {
        lines.iter().map(|l| line_text(l)).collect()
    }

    /// 10 units per char, 20 when bold.
    fn width(spans: &[TextSpan]) -> f32 {
        spans
            .iter()
            .map(|s| s.text.chars().count() as f32 * if s.bold { 20.0 } else { 10.0 })
            .sum()
    }

    #[test]
    fn normalize_collapses_and_trims() {
        assert_eq!(normalize_chat_message("  hello   world  "), "hello world");
        assert_eq!(normalize_chat_message("/say   hi   there"), "/say hi there");
        assert_eq!(normalize_chat_message("   "), "");
    }

    #[test]
    fn normalize_clamps_length() {
        let long = "a".repeat(300);
        assert_eq!(
            normalize_chat_message(&long).chars().count(),
            MAX_MESSAGE_LEN
        );
        let emoji = format!("a{}", "😀".repeat(200));
        assert_eq!(
            normalize_chat_message(&emoji),
            format!("a{}?", "😀".repeat(127))
        );
    }

    #[test]
    fn line_alpha_curve() {
        assert!((line_alpha(0.0) - 1.0).abs() < 1e-6);
        assert!((line_alpha(9.0) - 1.0).abs() < 1e-6);
        assert_eq!(line_alpha(10.0), 0.0);
        assert!(line_alpha(9.5) > 0.0 && line_alpha(9.5) < 1.0);
    }

    #[test]
    fn wrap_spans_wraps_on_width_and_keeps_color() {
        // Lines fit 5 plain chars.
        let red = [1.0, 0.0, 0.0, 1.0];
        let green = [0.0, 1.0, 0.0, 1.0];
        let lines = wrap_spans(&[span("aa", red), span(" bb cc", green)], 50.0, &width);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "aa bb");
        assert_eq!(line_text(&lines[1]), "cc");
        // First line stays red "aa" then green " bb".
        assert_eq!(lines[0][0].text, "aa");
        assert_eq!(lines[0][0].color, red);
        assert_eq!(lines[0].last().unwrap().color, green);
        assert_eq!(lines[1][0].color, green);
    }

    #[test]
    fn wrap_spans_hard_breaks_long_word() {
        let lines = wrap_spans(&[span("aaaaaaa", [1.0; 4])], 30.0, &width);
        assert_eq!(line_texts(&lines), vec!["aaa", "aaa", "a"]);
    }

    #[test]
    fn wrap_spans_empty_is_one_blank_line() {
        let lines = wrap_spans(&[], 50.0, &width);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].is_empty());
    }

    #[test]
    fn wrap_spans_measures_styled_width() {
        let mut bold = span(" bb", [1.0; 4]);
        bold.bold = true;
        let lines = wrap_spans(&[span("aa", [1.0; 4]), bold], 50.0, &width);
        assert_eq!(line_texts(&lines), vec!["aa", "bb"]);
    }

    #[test]
    fn chat_wrap_indents_width_wraps_only() {
        let red = [1.0, 0.0, 0.0, 1.0];
        let lines = wrap_chat_spans(
            &[span("aaa bbb\nccc", [1.0; 4]), span(" ddd\n", red)],
            30.0,
            &width,
        );
        assert_eq!(line_texts(&lines), vec!["aaa", " bbb", "ccc", " ddd", ""]);
        assert_eq!(lines[3][1].color, red);
        // `Font.split` breaks the same way without the indent.
        let lines = wrap_spans(&[span("aaa bbb\n\nccc", [1.0; 4])], 30.0, &width);
        assert_eq!(line_texts(&lines), vec!["aaa", "bbb", "", "ccc"]);
    }

    /// Three 30-char words; each fills its own line of a 320-wide chat.
    fn three_line_message() -> Vec<TextSpan> {
        let word = "a".repeat(30);
        vec![span(&format!("{word} {word} {word}"), [1.0; 4])]
    }

    fn build_chat(chat: &mut ChatState) {
        chat.build(
            &mut Vec::new(),
            800.0,
            600.0,
            1.0,
            &|s, _| s.len() as f32 * 10.0,
            &|spans, _| width(spans),
        );
    }

    #[test]
    fn backlog_caps_wrapped_lines() {
        let mut chat = ChatState::new();
        for _ in 0..MAX_MESSAGES {
            chat.push_message(three_line_message());
        }
        assert_eq!(chat.messages.len(), MAX_MESSAGES);
        assert_eq!(chat.trimmed_lines(&width).count(), MAX_MESSAGES);
        chat.open();
        chat.scroll_chat(1000);
        build_chat(&mut chat);
        assert_eq!(chat.scroll_pos, MAX_MESSAGES - LINES_PER_PAGE);
    }

    #[test]
    fn scroll_anchors_per_wrapped_line_while_open() {
        let mut chat = ChatState::new();
        for _ in 0..20 {
            chat.push_message(vec![span("a", [1.0; 4])]);
        }
        chat.open();
        chat.scroll_chat(2);
        chat.push_message(three_line_message());
        build_chat(&mut chat);
        assert_eq!(chat.scroll_pos, 5);
        // Only the open chat anchors.
        chat.close();
        chat.scroll_pos = 2;
        chat.push_message(three_line_message());
        assert_eq!(chat.scroll_anchor_pending, 0);
    }

    #[test]
    fn client_system_message_is_tagged() {
        let mut chat = ChatState::new();
        chat.push_message(vec![span("a", [1.0; 4])]);
        assert_eq!(
            chat.messages[0].tag,
            Some(ChatMessageTag::SystemSinglePlayer)
        );
    }

    fn set_input(chat: &mut ChatState, input: &str) {
        chat.input.set_value(input, f32::MAX, &|_| 0.0);
    }

    /// A chat awaiting a server response for `input` with request id 1.
    fn awaiting_chat(input: &str) -> ChatState {
        let mut chat = ChatState::new();
        chat.open = true;
        set_input(&mut chat, input);
        chat.awaiting = Some((1, input.to_string()));
        chat
    }

    #[test]
    fn server_suggestions_replace_and_select_first() {
        let mut chat = awaiting_chat("/gamemode c");
        chat.suggestions = vec!["stale".into()];
        chat.suggest_index = 3;
        chat.suggest_applied = true;
        chat.apply_server_suggestions(1, 10, vec!["creative".into()]);
        assert_eq!(chat.suggestions, vec!["creative"]);
        assert_eq!(chat.suggest_anchor, "/gamemode ");
        assert_eq!(chat.suggest_index, 0);
        assert!(!chat.suggest_applied);
        assert!(chat.awaiting.is_none());
    }

    #[test]
    fn server_suggestions_stale_dropped() {
        // Wrong id.
        let mut chat = awaiting_chat("/gamemode c");
        chat.apply_server_suggestions(2, 10, vec!["creative".into()]);
        assert!(chat.suggestions.is_empty());
        assert!(chat.awaiting.is_some());

        // Input changed since the request.
        let mut chat = awaiting_chat("/gamemode c");
        set_input(&mut chat, "/gamemode cr");
        chat.apply_server_suggestions(1, 10, vec!["creative".into()]);
        assert!(chat.suggestions.is_empty());
    }

    #[test]
    fn server_suggestions_empty_keeps_local() {
        let mut chat = awaiting_chat("/time set d");
        chat.suggestions = vec!["day".into()];
        chat.suggest_anchor = "/time set ".to_string();
        chat.apply_server_suggestions(1, 10, Vec::new());
        assert_eq!(chat.suggestions, vec!["day"]);
        assert!(chat.awaiting.is_none());
    }

    #[test]
    fn ghost_is_selected_suggestion_remainder() {
        let mut chat = ChatState::new();
        set_input(&mut chat, "/gam");
        chat.suggest_anchor = "/".to_string();
        chat.suggestions = vec!["gamemode".into(), "gamerule".into()];
        assert_eq!(chat.ghost_suffix(), Some("emode"));
        chat.suggest_index = 1;
        assert_eq!(chat.ghost_suffix(), Some("erule"));
        // Case mismatch shows no ghost (vanilla is case-sensitive here).
        set_input(&mut chat, "/GAM");
        assert_eq!(chat.ghost_suffix(), None);
        // Fully typed suggestion leaves nothing to show.
        set_input(&mut chat, "/gamerule");
        assert_eq!(chat.ghost_suffix(), None);
    }

    #[test]
    fn sort_floats_partial_matches() {
        let sorted = sort_with_partial_first(
            vec!["apple".into(), "creative".into(), "minecraft:cow".into()],
            "c",
        );
        assert_eq!(sorted, vec!["creative", "minecraft:cow", "apple"]);
    }

    #[test]
    fn server_suggestions_non_ascii_start() {
        // "/msg héllo " is 11 UTF-16 units but 12 bytes ('é' is 2 bytes).
        let mut chat = awaiting_chat("/msg héllo w");
        chat.apply_server_suggestions(1, 11, vec!["world".into()]);
        assert_eq!(chat.suggest_anchor, "/msg héllo ");
        assert_eq!(chat.suggestions, vec!["world"]);

        // Out-of-range start is dropped.
        let mut chat = awaiting_chat("/msg héllo w");
        chat.apply_server_suggestions(1, 99, vec!["world".into()]);
        assert!(chat.suggestions.is_empty());
    }

    #[test]
    fn utf16_offset_conversion() {
        assert_eq!(utf16_offset_to_byte("abc", 0), Some(0));
        assert_eq!(utf16_offset_to_byte("abc", 3), Some(3));
        // 'é' is 1 UTF-16 unit, 2 bytes.
        assert_eq!(utf16_offset_to_byte("héllo", 2), Some(3));
        // '𝄞' is 2 UTF-16 units, 4 bytes.
        assert_eq!(utf16_offset_to_byte("𝄞x", 2), Some(4));
        assert_eq!(utf16_offset_to_byte("𝄞x", 1), None);
        assert_eq!(utf16_offset_to_byte("abc", 4), None);
    }
}
