use std::cell::OnceCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use azalea_inventory::components::{MaxDamage, Rarity};
use azalea_inventory::default_components::get_default_component;
use azalea_registry::builtin::ItemKind;

use super::common;
use crate::chat_component::{Argument, ClickEvent, Component, HoverEvent, ResolvedStyle};
use crate::net::commands::{CommandPresentation, CommandTokenKind, CommandTree};
use crate::net::sender::ChatMark;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::text::{TextSpan, format_component_spans};
use crate::ui::text_edit::{SystemClipboard, TextFieldState, TextInputEvent};

const MAX_MESSAGES: usize = 100;
const CHAT_X: f32 = 4.0;
const BOTTOM_MARGIN: f32 = 40.0;
const MESSAGE_LIFETIME_SECS: f32 = 10.0;
const INPUT_HEIGHT: f32 = 12.0;
const MAX_MESSAGE_LEN: usize = 256;

const SUGGEST_ROW_H: f32 = 12.0;
const MAX_SUGGESTION_ROWS: usize = 10;
const SUGGEST_BG_ALPHA: f32 = 208.0 / 255.0;

/// Vanilla composites these black GUI fills in its gamma-space framebuffer.
/// Pomme's Vulkan UI target is sRGB, whose fixed-function blending occurs in
/// linear space, so the same numeric alpha would look noticeably lighter.
/// Convert the gamma-space darkening factor to the equivalent linear-space
/// alpha (the renderer uses the same 2.2 approximation for Vanilla's vignette).
fn vanilla_black_fill(alpha: f32) -> [f32; 4] {
    let alpha = alpha.clamp(0.0, 1.0);
    [0.0, 0.0, 0.0, 1.0 - (1.0 - alpha).powf(2.2)]
}
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

impl ChatVisibilitySetting {
    pub fn cycle(self) -> Self {
        match self {
            Self::Full => Self::System,
            Self::System => Self::Hidden,
            Self::Hidden => Self::Full,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "Shown",
            Self::System => "Commands Only",
            Self::Hidden => "Hidden",
        }
    }
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

impl ChatOptions {
    pub fn width_px(self) -> f32 {
        (self.width.clamp(0.0, 1.0) * 280.0 + 40.0).floor()
    }

    fn wrap_width_px(self) -> f32 {
        (self.width_px() / self.scale.clamp(0.01, 1.0)).ceil()
    }

    pub fn height_px(self, focused: bool) -> f32 {
        let pct = if focused {
            self.height_focused
        } else {
            self.height_unfocused
        };
        (pct.clamp(0.0, 1.0) * 160.0 + 20.0).floor()
    }

    pub fn effective_text_opacity(self) -> f32 {
        self.opacity.clamp(0.0, 1.0) * 0.9 + 0.1
    }

    fn line_height(self) -> f32 {
        (9.0 * (self.line_spacing.clamp(0.0, 1.0) + 1.0))
            .floor()
            .max(1.0)
    }
}

#[derive(Clone)]
struct ChatHitRegion {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    style: Arc<ResolvedStyle>,
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

impl ChatMessageTag {
    fn indicator_color(&self) -> [f32; 4] {
        match self {
            Self::System | Self::SystemSinglePlayer | Self::NotSecure => common::rgb(0xd0d0d0),
            Self::Modified { .. } => common::rgb(0x606060),
            Self::Error => common::rgb(0xff5555),
        }
    }

    fn tooltip_component(&self) -> Component {
        match self {
            Self::System => Component::translate("chat.tag.system", Vec::new()),
            Self::SystemSinglePlayer => {
                Component::translate("chat.tag.system_single_player", Vec::new())
            }
            Self::NotSecure => Component::translate("chat.tag.not_secure", Vec::new()),
            Self::Modified { original } => {
                let mut component = Component::translate("chat.tag.modified", Vec::new());
                let mut old = Component::text(format!("\n{original}"));
                old.style.color = Some(0xaaaaaa);
                component.siblings.push(old);
                component
            }
            Self::Error => Component::translate("chat.tag.error", Vec::new()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatSuggestion {
    pub text: String,
    pub tooltip: Option<Component>,
}

impl ChatSuggestion {
    fn plain(text: String) -> Self {
        Self {
            text,
            tooltip: None,
        }
    }
}

impl From<String> for ChatSuggestion {
    fn from(text: String) -> Self {
        Self::plain(text)
    }
}

impl From<&str> for ChatSuggestion {
    fn from(text: &str) -> Self {
        Self::plain(text.to_owned())
    }
}

impl PartialEq<&str> for ChatSuggestion {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatUiAction {
    OpenUrl(String),
    OpenChatSettings,
    RunCommand(String),
    RunCommandUnsigned(String),
    Custom {
        id: String,
        payload: Option<simdnbt::owned::NbtTag>,
    },
    ShowDialog(serde_json::Value),
}

pub struct ChatBuildContext<'a> {
    pub screen_w: f32,
    pub screen_h: f32,
    pub gui_scale: f32,
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub shift: bool,
    pub command_tree: Option<&'a CommandTree>,
    pub advanced_item_tooltips: bool,
    pub text_width_fn: &'a dyn Fn(&str, f32) -> f32,
    pub spans_width_fn: &'a dyn Fn(&[TextSpan], f32) -> f32,
}

type DisplayLine = (Vec<TextSpan>, f32, Option<ChatMessageTag>, bool, usize);

struct ChatLine {
    spans: Vec<TextSpan>,
    received: Instant,
    signature: Option<[u8; 256]>,
    source: ChatMessageSource,
    tag: Option<ChatMessageTag>,
    /// Lazily wrapped display lines (vanilla `trimmedMessages`).
    wrapped: OnceCell<Vec<Vec<TextSpan>>>,
}

struct PendingChatLine {
    spans: Vec<TextSpan>,
    /// Signature attached to the rendered message itself. Validation-error
    /// markers intentionally have no display signature in Vanilla.
    signature: Option<[u8; 256]>,
    /// Signature receipt to feed LastSeenMessagesTracker once this delayed
    /// handler actually runs. This is separate from `signature` so a red
    /// validation-error marker can acknowledge the invalid packet as hidden.
    ack_signature: Option<[u8; 256]>,
    force_hidden_ack: bool,
    suppress_display: bool,
    source: ChatMessageSource,
    tag: Option<ChatMessageTag>,
}

impl ChatLine {
    fn wrapped(
        &self,
        chat_width: f32,
        chat_colors: bool,
        width0: &dyn Fn(&[TextSpan]) -> f32,
    ) -> &[Vec<TextSpan>] {
        self.wrapped.get_or_init(|| {
            let max_width = if matches!(self.tag, Some(ChatMessageTag::Modified { .. })) {
                // Vanilla reserves icon.width + margin-left + 2 = 9 + 4 + 2.
                chat_width - 15.0
            } else {
                chat_width
            };
            let spans = legacy_format_spans(&self.spans, chat_colors);
            wrap_spans(&spans, max_width.max(1.0), width0)
        })
    }

    fn invalidate_wrap(&mut self) {
        self.wrapped = OnceCell::new();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandConfirmationKind {
    SignatureRequired,
    PermissionsRequired,
    ParseErrors,
}

#[derive(Clone, Debug)]
struct PendingCommandConfirmation {
    command: String,
    kind: CommandConfirmationKind,
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
    /// Vanilla `newMessageSinceScroll`: changes the open-chat scrollbar color
    /// when new text arrives while the user is reading older lines.
    new_message_since_scroll: bool,
    suggestions: Vec<ChatSuggestion>,
    suggest_index: usize,
    suggest_offset: usize,
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
    /// Exact physical-screen rectangles occupied by styled chat spans on the
    /// previous/current rendered frame. Vanilla's active-text collector does
    /// the same job for ChatScreen hover/click lookup.
    hit_regions: Vec<ChatHitRegion>,
    /// Physical rectangles of the visible command-suggestion rows.
    suggestion_regions: Vec<(usize, [f32; 4])>,
    queue_region: Option<[f32; 4]>,
    last_suggestion_cursor: Option<(f32, f32)>,
    /// Vanilla's chatLinksPrompt confirmation state. Kept inside ChatScreen so
    /// accepting/cancelling returns to the same live chat input.
    pending_link: Option<String>,
    link_buttons: Option<([f32; 4], [f32; 4], [f32; 4])>,
    pending_command: Option<PendingCommandConfirmation>,
    command_buttons: Option<([f32; 4], [f32; 4])>,
    delayed_deletions: Vec<([u8; 256], Instant)>,
    delayed_messages: VecDeque<PendingChatLine>,
    /// Last-seen updates in the order they happened, for the network loop.
    chat_marks: Vec<ChatMark>,
    previous_message_time: Option<Instant>,
    latest_draft: Option<String>,
    is_restored_draft: bool,
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
            new_message_since_scroll: false,
            suggestions: Vec::new(),
            suggest_index: 0,
            suggest_offset: 0,
            suggest_anchor: String::new(),
            suggest_applied: false,
            allow_suggestions: true,
            last_computed: String::new(),
            next_suggest_id: 0,
            awaiting: None,
            outgoing_request: None,
            hit_regions: Vec::new(),
            suggestion_regions: Vec::new(),
            queue_region: None,
            last_suggestion_cursor: None,
            pending_link: None,
            link_buttons: None,
            pending_command: None,
            command_buttons: None,
            delayed_deletions: Vec::new(),
            delayed_messages: VecDeque::new(),
            chat_marks: Vec::new(),
            previous_message_time: None,
            latest_draft: None,
            is_restored_draft: false,
        }
    }

    pub fn set_options(&mut self, options: ChatOptions) {
        let previous = self.options;
        let wrap_changed = (previous.width - options.width).abs() > f32::EPSILON
            || (previous.scale - options.scale).abs() > f32::EPSILON
            || previous.colors != options.colors;
        self.options = options;
        if wrap_changed {
            for message in &mut self.messages {
                message.invalidate_wrap();
            }
            self.scroll_pos = 0;
        }
        if previous.delay_secs > 0.0 && self.options.delay_secs <= 0.0 {
            self.flush_delayed_messages(Instant::now());
        }
        if !self.options.auto_suggestions {
            self.clear_suggestions();
            self.allow_suggestions = false;
        } else if self.open && self.input.value() != self.last_computed {
            self.allow_suggestions = true;
        }
    }

    pub fn has_pending_modal_prompt(&self) -> bool {
        self.pending_link.is_some() || self.pending_command.is_some()
    }

    pub fn only_secure(&self) -> bool {
        self.options.only_secure
    }

    pub fn request_command_confirmation(&mut self, command: String, kind: CommandConfirmationKind) {
        self.pending_command = Some(PendingCommandConfirmation { command, kind });
        self.command_buttons = None;
    }

    pub fn request_open_url(&mut self, url: String) -> Option<ChatUiAction> {
        let Ok(url) = crate::chat_component::parse_untrusted_url(url) else {
            return None;
        };
        if !self.options.links {
            return None;
        }
        if self.options.links_prompt {
            self.pending_link = Some(url);
            None
        } else {
            Some(ChatUiAction::OpenUrl(url))
        }
    }

    pub fn push_message(&mut self, spans: Vec<TextSpan>) {
        self.push_message_with_source(spans, None, ChatMessageSource::SystemClient, None);
    }

    pub fn push_message_with_source(
        &mut self,
        spans: Vec<TextSpan>,
        signature: Option<[u8; 256]>,
        source: ChatMessageSource,
        tag: Option<ChatMessageTag>,
    ) {
        let pending = PendingChatLine {
            spans,
            signature,
            ack_signature: signature,
            force_hidden_ack: false,
            suppress_display: false,
            source,
            tag,
        };
        let now = Instant::now();
        if source == ChatMessageSource::Player && self.will_delay_messages(now) {
            self.delayed_messages.push_back(pending);
            return;
        }
        self.accept_pending_message(pending, now);
    }

    pub fn push_validation_error(
        &mut self,
        spans: Vec<TextSpan>,
        invalid_signature: Option<[u8; 256]>,
    ) {
        let pending = PendingChatLine {
            spans,
            signature: None,
            ack_signature: invalid_signature,
            force_hidden_ack: true,
            suppress_display: false,
            source: ChatMessageSource::Player,
            tag: Some(ChatMessageTag::Error),
        };
        let now = Instant::now();
        if self.will_delay_messages(now) {
            self.delayed_messages.push_back(pending);
        } else {
            self.accept_pending_message(pending, now);
        }
    }

    pub fn push_fully_filtered(&mut self, signature: Option<[u8; 256]>) {
        let pending = PendingChatLine {
            spans: Vec::new(),
            signature: None,
            ack_signature: signature,
            force_hidden_ack: true,
            suppress_display: true,
            source: ChatMessageSource::Player,
            tag: None,
        };
        let now = Instant::now();
        if self.will_delay_messages(now) {
            self.delayed_messages.push_back(pending);
        } else {
            self.accept_pending_message(pending, now);
        }
    }

    fn source_visible(&self, source: ChatMessageSource, tag: Option<&ChatMessageTag>) -> bool {
        let visibility_ok = match source {
            ChatMessageSource::SystemClient => true,
            ChatMessageSource::SystemServer => {
                self.options.visibility != ChatVisibilitySetting::Hidden
            }
            ChatMessageSource::Player => self.options.visibility == ChatVisibilitySetting::Full,
        };
        if !visibility_ok {
            return false;
        }
        !(self.options.only_secure
            && source == ChatMessageSource::Player
            && matches!(tag, Some(ChatMessageTag::NotSecure)))
    }

    fn will_delay_messages(&self, now: Instant) -> bool {
        if self.options.delay_secs <= 0.0 {
            return false;
        }
        self.previous_message_time.is_some_and(|previous| {
            now.duration_since(previous).as_secs_f32() < self.options.delay_secs
        })
    }

    fn accept_pending_message(&mut self, pending: PendingChatLine, now: Instant) -> bool {
        if pending.suppress_display || !self.source_visible(pending.source, pending.tag.as_ref()) {
            self.mark_processed(pending.ack_signature, false);
            return false;
        }
        let source = pending.source;
        let signature = pending.signature;
        let ack_signature = pending.ack_signature;
        let force_hidden_ack = pending.force_hidden_ack;
        self.messages.push_back(ChatLine {
            spans: pending.spans,
            received: now,
            signature,
            source,
            tag: pending.tag,
            wrapped: OnceCell::new(),
        });
        if self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        // A new line while scrolled keeps the view anchored (vanilla
        // ChatComponent.addMessage shifts the scrollbar by one).
        if self.scroll_pos > 0 {
            self.new_message_since_scroll = true;
            self.scroll_pos += 1;
        }
        self.mark_processed(ack_signature, !force_hidden_ack);
        if source == ChatMessageSource::Player {
            self.previous_message_time = Some(now);
        }
        true
    }

    /// Vanilla `markMessageAsProcessed`.
    fn mark_processed(&mut self, signature: Option<[u8; 256]>, shown: bool) {
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

    fn flush_delayed_messages(&mut self, now: Instant) {
        while let Some(message) = self.delayed_messages.pop_front() {
            self.accept_pending_message(message, now);
        }
        self.previous_message_time = None;
    }

    fn process_delayed_messages(&mut self, now: Instant) {
        if self.delayed_messages.is_empty() {
            return;
        }
        if self.options.delay_secs <= 0.0 {
            self.flush_delayed_messages(now);
            return;
        }
        if self.will_delay_messages(now) {
            return;
        }
        while let Some(message) = self.delayed_messages.pop_front() {
            if self.accept_pending_message(message, now) {
                break;
            }
        }
    }

    fn accept_next_delayed_message(&mut self) {
        if let Some(message) = self.delayed_messages.pop_front() {
            self.accept_pending_message(message, Instant::now());
        }
    }

    pub fn delete_message(&mut self, signature: [u8; 256]) {
        if let Some(index) = self
            .delayed_messages
            .iter()
            .position(|message| message.signature.as_ref() == Some(&signature))
        {
            self.delayed_messages.remove(index);
            return;
        }
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
        line.source = ChatMessageSource::SystemServer;
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
        self.process_delayed_messages(now);
    }

    /// F3+D; vanilla `clearMessages(false)` keeps the sent-message history.
    pub fn clear_messages(&mut self) {
        self.delayed_messages.clear();
        self.previous_message_time = None;
        self.delayed_deletions.clear();
        self.messages.clear();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        let restored = self.latest_draft.clone();
        self.is_restored_draft = restored.is_some();
        self.input
            .set_value(restored.as_deref().unwrap_or(""), f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.allow_suggestions = self.options.auto_suggestions;
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
    }

    pub fn open_with_slash(&mut self) {
        self.open = true;
        let restored = self
            .latest_draft
            .clone()
            .filter(|draft| draft.starts_with('/'));
        self.is_restored_draft = restored.is_some();
        self.input
            .set_value(restored.as_deref().unwrap_or("/"), f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.allow_suggestions = self.options.auto_suggestions;
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
    }

    pub fn close(&mut self) {
        if self.open {
            let value = self.input.value().trim().to_owned();
            if self.options.save_drafts && !value.is_empty() {
                self.latest_draft = Some(self.input.value().to_owned());
            } else {
                self.latest_draft = None;
            }
        }
        self.finish_close();
    }

    fn close_after_submit(&mut self) {
        self.latest_draft = None;
        self.finish_close();
    }

    fn finish_close(&mut self) {
        self.open = false;
        self.is_restored_draft = false;
        self.input.set_focused(false);
        self.clear_suggestions();
        self.pending_link = None;
        self.link_buttons = None;
        self.pending_command = None;
        self.command_buttons = None;
        // Vanilla resets the chat scroll when the screen closes.
        self.scroll_pos = 0;
        self.new_message_since_scroll = false;
    }

    /// Vanilla key priority while ChatScreen is open: a child/overlay consumes
    /// Escape before the screen itself closes. Returns true only when this call
    /// actually closed chat and the game should recapture the cursor.
    pub fn handle_escape(&mut self) -> bool {
        if self.pending_link.take().is_some() {
            self.link_buttons = None;
            return false;
        }
        if self.pending_command.take().is_some() {
            self.command_buttons = None;
            return false;
        }
        if !self.open {
            return false;
        }
        if !self.suggestions.is_empty() {
            self.suggestions.clear();
            self.suggest_anchor.clear();
            self.suggest_index = 0;
            self.suggest_applied = false;
            self.allow_suggestions = false;
            self.last_computed = self.input.value().to_owned();
            return false;
        }
        self.close();
        true
    }

    fn lines_per_page(&self) -> usize {
        let height = self.options.height_px(self.open);
        (height / self.options.line_height()).floor().max(1.0) as usize
    }

    /// Scroll the message backlog by wrapped lines; positive is up (vanilla
    /// `scrollChat`). The upper clamp happens in `build`, where wrap counts
    /// are known.
    pub fn scroll_chat(&mut self, delta: i32) {
        if self.open {
            self.scroll_pos = self.scroll_pos.saturating_add_signed(delta as isize);
            if self.scroll_pos == 0 {
                self.new_message_since_scroll = false;
            }
        }
    }

    fn keep_suggestion_visible(&mut self) {
        if self.suggestions.is_empty() {
            self.suggest_offset = 0;
            return;
        }
        let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
        let max_offset = self.suggestions.len().saturating_sub(visible);
        if self.suggest_index < self.suggest_offset {
            self.suggest_offset = self.suggest_index.min(max_offset);
        } else if self.suggest_index >= self.suggest_offset + visible {
            self.suggest_offset = (self.suggest_index + 1 - visible).min(max_offset);
        }
    }

    /// Vanilla `SuggestionsList.mouseScrolled` gets first refusal when the
    /// pointer is over the completion rectangle; otherwise ChatScreen scrolls
    /// the message backlog by one line with Shift or seven lines normally.
    pub fn handle_scroll(&mut self, cursor: (f32, f32), delta: f32, shift: bool) {
        if !self.open || delta == 0.0 {
            return;
        }
        let step = delta.clamp(-1.0, 1.0);
        if !self.suggestions.is_empty()
            && self
                .suggestion_regions
                .iter()
                .any(|(_, rect)| common::hit_test(cursor, *rect))
        {
            let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
            let max_offset = self.suggestions.len().saturating_sub(visible);
            let next = (self.suggest_offset as f32 - step).trunc() as isize;
            self.suggest_offset = next.clamp(0, max_offset as isize) as usize;
            return;
        }
        let multiplier = if shift { 1.0 } else { 7.0 };
        self.scroll_chat((step * multiplier) as i32);
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
        self.suggest_offset = 0;
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
        if self.options.visibility == ChatVisibilitySetting::Hidden {
            return;
        }
        if let Some(cmd) = input.strip_prefix('/')
            && let Some(tree) = tree
        {
            let sug = tree.suggestions(cmd);
            let cut = input.len() - sug.partial_len;
            self.suggest_anchor = input[..cut].to_string();
            self.suggestions = sug.options.into_iter().map(ChatSuggestion::plain).collect();
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
    pub fn apply_server_suggestions(
        &mut self,
        id: u32,
        start: usize,
        options: Vec<ChatSuggestion>,
    ) {
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
        self.suggestions = sort_suggestions_with_partial_first(options, &partial);
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
                self.keep_suggestion_visible();
            } else if up {
                self.move_in_history(-1, inner_w, width_fn);
            } else {
                self.move_in_history(1, inner_w, width_fn);
            }
        }
        let lines_per_page = self.lines_per_page();
        if page_up {
            self.scroll_chat(lines_per_page as i32 - 1);
        }
        if page_down {
            self.scroll_chat(-(lines_per_page as i32 - 1));
        }

        let mut clipboard = SystemClipboard;
        let before_edits = self.input.value().to_string();
        for ev in events {
            if self.is_restored_draft
                && matches!(
                    ev,
                    TextInputEvent::Key {
                        code: winit::keyboard::KeyCode::Backspace,
                        ..
                    }
                )
            {
                self.input.set_value("", inner_w, width_fn);
                self.is_restored_draft = false;
                self.latest_draft = None;
                continue;
            }
            self.input.handle(ev, &mut clipboard, inner_w, width_fn);
        }
        // A real edit re-enables the popup and turns a restored draft into a
        // normal live edit (vanilla `ChatScreen.onEdited`).
        if self.input.value() != before_edits {
            self.is_restored_draft = false;
            self.allow_suggestions = self.options.auto_suggestions;
        }

        // Tab opens a hidden completion list again; once visible it applies the
        // highlighted completion, and further Tabs cycle it.
        if tab && self.suggestions.is_empty() && !self.allow_suggestions {
            self.allow_suggestions = true;
            self.last_computed.clear();
            self.recompute_suggestions(tree);
        }
        if tab && !self.suggestions.is_empty() {
            let n = self.suggestions.len();
            if self.suggest_applied {
                self.suggest_index = if shift {
                    (self.suggest_index + n - 1) % n
                } else {
                    (self.suggest_index + 1) % n
                };
                self.keep_suggestion_visible();
            }
            let applied = format!(
                "{}{}",
                self.suggest_anchor, self.suggestions[self.suggest_index].text
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
            if normalized.is_empty() {
                self.input.set_value("", inner_w, width_fn);
                self.close_after_submit();
                return None;
            }
            let is_command = normalized.starts_with('/');
            let allowed = match self.options.visibility {
                ChatVisibilitySetting::Full => true,
                ChatVisibilitySetting::System => is_command,
                ChatVisibilitySetting::Hidden => false,
            };
            if !allowed {
                return None;
            }
            self.add_recent_chat(&normalized);
            self.input.set_value("", inner_w, width_fn);
            self.close_after_submit();
            return Some(normalized);
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
        let suffix = selected.text.strip_prefix(rest)?;
        (!suffix.is_empty()).then_some(suffix)
    }

    fn style_at(&self, cursor: (f32, f32)) -> Option<Arc<ResolvedStyle>> {
        self.hit_regions
            .iter()
            .find(|r| cursor.0 >= r.x0 && cursor.0 < r.x1 && cursor.1 >= r.y0 && cursor.1 < r.y1)
            .map(|r| r.style.clone())
    }

    pub fn hovering_clickable(&self, cursor: (f32, f32)) -> bool {
        if !self.open {
            return false;
        }
        if let Some((yes, copy, no)) = self.link_buttons {
            return common::hit_test(cursor, yes)
                || common::hit_test(cursor, copy)
                || common::hit_test(cursor, no);
        }
        if self
            .queue_region
            .is_some_and(|rect| common::hit_test(cursor, rect))
        {
            return true;
        }
        if self
            .suggestion_regions
            .iter()
            .any(|(_, rect)| common::hit_test(cursor, *rect))
        {
            return true;
        }
        !self.has_pending_modal_prompt()
            && self
                .style_at(cursor)
                .is_some_and(|style| style.click_event.is_some())
    }

    fn apply_suggestion(&mut self, idx: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        let Some(suggestion) = self.suggestions.get(idx).cloned() else {
            return;
        };
        self.suggest_index = idx;
        let applied = format!("{}{}", self.suggest_anchor, suggestion.text);
        self.input.set_value(&applied, inner_w, width_fn);
        self.suggest_applied = true;
        self.last_computed = applied;
    }

    fn handle_link_prompt_click(&mut self, cursor: (f32, f32)) -> Option<ChatUiAction> {
        let url = self.pending_link.clone()?;
        let (yes, copy, no) = self.link_buttons?;
        if common::hit_test(cursor, yes) {
            self.pending_link = None;
            self.link_buttons = None;
            return Some(ChatUiAction::OpenUrl(url));
        }
        if common::hit_test(cursor, copy) {
            common::set_clipboard(&url);
            self.pending_link = None;
            self.link_buttons = None;
            return None;
        }
        if common::hit_test(cursor, no) {
            self.pending_link = None;
            self.link_buttons = None;
        }
        None
    }

    fn handle_click(
        &mut self,
        cursor: (f32, f32),
        shift: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> Option<ChatUiAction> {
        if self.has_pending_modal_prompt() {
            return None;
        }

        if self
            .queue_region
            .is_some_and(|rect| common::hit_test(cursor, rect))
        {
            self.accept_next_delayed_message();
            return None;
        }

        if let Some((idx, _)) = self
            .suggestion_regions
            .iter()
            .find(|(_, rect)| common::hit_test(cursor, *rect))
            .copied()
        {
            self.apply_suggestion(idx, inner_w, width_fn);
            return None;
        }

        let style = self.style_at(cursor)?;
        if shift {
            if let Some(insertion) = &style.insertion {
                self.input.insert_text(insertion, inner_w, width_fn);
                self.allow_suggestions = self.options.auto_suggestions;
                self.last_computed.clear();
            }
            return None;
        }

        match style.click_event.as_ref()? {
            ClickEvent::OpenUrl(url) => self.request_open_url(url.clone()),
            ClickEvent::RunCommand(command) => Some(ChatUiAction::RunCommand(command.clone())),
            ClickEvent::SuggestCommand(command) => {
                self.input.set_value(command, inner_w, width_fn);
                self.allow_suggestions = self.options.auto_suggestions;
                self.last_computed.clear();
                None
            }
            ClickEvent::CopyToClipboard(value) => {
                common::set_clipboard(value);
                None
            }
            ClickEvent::ShowDialog(dialog) => Some(ChatUiAction::ShowDialog(dialog.clone())),
            ClickEvent::Custom { id, payload } => {
                if id == "minecraft:internal/go_to_restrictions_screen"
                    || id == "internal/go_to_restrictions_screen"
                {
                    Some(ChatUiAction::OpenChatSettings)
                } else {
                    Some(ChatUiAction::Custom {
                        id: id.clone(),
                        payload: payload.clone(),
                    })
                }
            }
            ClickEvent::ChangePage(_) => None,
        }
    }

    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        context: ChatBuildContext<'_>,
    ) -> Option<ChatUiAction> {
        let ChatBuildContext {
            screen_w,
            screen_h,
            gui_scale: gs,
            cursor,
            clicked,
            shift,
            command_tree,
            advanced_item_tooltips,
            text_width_fn,
            spans_width_fn,
        } = context;
        self.tick();
        self.hit_regions.clear();
        self.suggestion_regions.clear();
        self.queue_region = None;
        let chat_scale = self.options.scale.clamp(0.01, 1.0);
        let chat_width = self.options.wrap_width_px();
        let chat_fs = common::FONT_SIZE * gs * chat_scale;
        let entry_height = self.options.line_height();
        let lh = entry_height * gs * chat_scale;
        let spacing = self.options.line_spacing.clamp(0.0, 1.0);
        let text_baseline_offset = (8.0 * (spacing + 1.0) - 4.0 * spacing).round();
        // Vanilla scales the ChatComponent pose, then translates x by 4 in
        // that local coordinate space. The -4 background start therefore
        // lands exactly on screen x=0 at every chat scale.
        let origin = CHAT_X * gs * chat_scale;
        let bg_x = 0.0;
        let bg_w = (chat_width + 12.0) * gs * chat_scale;
        let screen_gui_h = screen_h / gs;
        let chat_bottom = ((screen_gui_h - BOTTOM_MARGIN) / chat_scale).floor() * chat_scale * gs;
        let now = Instant::now();
        // Measure wrapping at gui-scale 1 so wrap points stay fixed when the
        // gui scale changes (vanilla wraps in gui-space, then scales). Use the
        // actual styled-span width: bold/font changes affect Vanilla wrap
        // points and must not be flattened to plain-string metrics.
        let width0 = |spans: &[TextSpan]| spans_width_fn(spans, common::FONT_SIZE);

        let lines_per_page = self.lines_per_page();
        let total_lines: usize = self
            .messages
            .iter()
            .filter(|m| self.source_visible(m.source, m.tag.as_ref()))
            .map(|m| m.wrapped(chat_width, self.options.colors, &width0).len())
            .sum();
        // Clamp the scroll to the wrapped backlog (vanilla scrollChat clamps
        // against `trimmedMessages`).
        if self.open && self.scroll_pos > 0 {
            self.scroll_pos = self
                .scroll_pos
                .min(total_lines.saturating_sub(lines_per_page));
        }

        // Gather the visible wrapped lines newest-first; index 0 is the
        // bottom-most line, `scroll_pos` lines skipped below it. All wrapped
        // lines of a message share its alpha.
        let mut display: Vec<DisplayLine> = Vec::new();
        let mut skipped = 0usize;
        'gather: for (message_id, msg) in self.messages.iter().rev().enumerate() {
            if !self.source_visible(msg.source, msg.tag.as_ref()) {
                continue;
            }
            let alpha = if self.open {
                1.0
            } else {
                line_alpha(now.duration_since(msg.received).as_secs_f32())
            };
            if !self.open && alpha <= 1e-5 {
                continue;
            }
            let wrapped = msg.wrapped(chat_width, self.options.colors, &width0);
            for (line_index, line) in wrapped.iter().enumerate().rev() {
                if skipped < self.scroll_pos {
                    skipped += 1;
                    continue;
                }
                display.push((
                    line.clone(),
                    alpha,
                    msg.tag.clone(),
                    line_index + 1 == wrapped.len(),
                    message_id,
                ));
                if display.len() >= lines_per_page {
                    break 'gather;
                }
            }
        }

        // Vanilla submits the visible chat-row backgrounds before the text
        // render states. Keep text deferred until every row background has
        // been emitted so glyphs whose provider geometry extends outside the
        // normal 8px line box (notably accented/obfuscated glyphs) are not
        // darkened by a neighboring row's translucent background.
        let mut chat_text_elements = Vec::with_capacity(display.len());
        for (i, (line_spans, alpha, tag, end_of_entry, message_id)) in display.iter().enumerate() {
            let entry_bottom = chat_bottom - (i as f32) * lh;
            let entry_top = entry_bottom - lh;
            let bg_a = alpha * self.options.text_background_opacity.clamp(0.0, 1.0);
            if bg_a > 1e-5 {
                elements.push(MenuElement::Rect {
                    x: bg_x,
                    y: entry_top,
                    w: bg_w,
                    h: lh,
                    corner_radius: 0.0,
                    color: vanilla_black_fill(bg_a),
                });
            }
            let text_a = alpha * self.options.effective_text_opacity();
            if let Some(tag) = tag {
                let mut indicator = tag.indicator_color();
                indicator[3] *= text_a;
                elements.push(MenuElement::Rect {
                    x: 0.0,
                    y: entry_top,
                    w: 2.0 * gs * chat_scale,
                    h: lh,
                    corner_radius: 0.0,
                    color: indicator,
                });
                let indicator_rect = [0.0, entry_top, 2.0 * gs * chat_scale, lh];
                if common::hit_test(cursor, indicator_rect) {
                    let lines = component_tooltip_lines(&tag.tooltip_component());
                    common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
                }
            }

            let mut span_x = origin;
            for span in line_spans {
                let span_w = spans_width_fn(std::slice::from_ref(span), chat_fs);
                if let Some(style) = &span.component_style
                    && span_w > 0.0
                {
                    self.hit_regions.push(ChatHitRegion {
                        x0: span_x,
                        y0: entry_top,
                        x1: span_x + span_w,
                        y1: entry_bottom,
                        style: style.clone(),
                    });
                }
                span_x += span_w;
            }

            if matches!(tag, Some(ChatMessageTag::Modified { .. })) {
                let line_width = spans_width_fn(line_spans, chat_fs);
                let icon_rect = [
                    origin + line_width + 4.0 * gs * chat_scale,
                    entry_top,
                    9.0 * gs * chat_scale,
                    9.0 * gs * chat_scale,
                ];
                let icon_hovered = common::hit_test(cursor, icon_rect);
                let message_hovered = *end_of_entry
                    && display.iter().enumerate().any(
                        |(line_idx, (other_spans, _, _, _, other_message_id))| {
                            if other_message_id != message_id {
                                return false;
                            }
                            let bottom = chat_bottom - line_idx as f32 * lh;
                            let top = bottom - lh;
                            let width = spans_width_fn(other_spans, chat_fs);
                            cursor.0 >= origin
                                && cursor.0 < origin + width
                                && cursor.1 >= top
                                && cursor.1 < bottom
                        },
                    );
                if icon_hovered || message_hovered {
                    elements.push(MenuElement::Image {
                        x: icon_rect[0],
                        y: icon_rect[1],
                        w: icon_rect[2],
                        h: icon_rect[3],
                        sprite: SpriteId::ChatModified,
                        tint: common::WHITE,
                    });
                }
                if icon_hovered && let Some(tag) = tag {
                    let lines = component_tooltip_lines(&tag.tooltip_component());
                    common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
                }
            }

            let faded: Vec<TextSpan> = line_spans
                .iter()
                .map(|s| {
                    let mut s = s.clone();
                    s.color[3] *= text_a;
                    s
                })
                .collect();
            chat_text_elements.push(MenuElement::McText {
                x: origin,
                y: entry_bottom - text_baseline_offset * gs * chat_scale,
                spans: faded,
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }
        elements.extend(chat_text_elements);

        if self.open && self.options.visibility != ChatVisibilitySetting::Full {
            let restricted_y = chat_bottom - (display.len() as f32 + 1.0) * lh;
            elements.push(MenuElement::Rect {
                x: 2.0 * gs * chat_scale,
                y: restricted_y,
                w: (chat_width + 10.0) * gs * chat_scale,
                h: lh,
                corner_radius: 0.0,
                color: vanilla_black_fill(self.options.text_background_opacity),
            });
            let mut restricted = Component::translate("chat_screen.restricted", Vec::new());
            restricted.style.color = Some(0xff5555);
            restricted.style.underlined = Some(true);
            restricted.style.click_event = Some(ClickEvent::Custom {
                id: "minecraft:internal/go_to_restrictions_screen".to_owned(),
                payload: None,
            });
            let restricted_spans = format_component_spans(&restricted, common::rgb(0xff5555));
            let mut x = origin;
            for span in &restricted_spans {
                let width = spans_width_fn(std::slice::from_ref(span), chat_fs);
                if let Some(style) = &span.component_style {
                    self.hit_regions.push(ChatHitRegion {
                        x0: x,
                        y0: restricted_y,
                        x1: x + width,
                        y1: restricted_y + lh,
                        style: style.clone(),
                    });
                }
                x += width;
            }
            elements.push(MenuElement::McText {
                x: origin,
                y: restricted_y + (entry_height - text_baseline_offset - 1.0) * gs * chat_scale,
                spans: restricted_spans,
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }

        if !self.delayed_messages.is_empty() {
            let queue_count = self.delayed_messages.len();
            let queue_h = common::FONT_SIZE * gs * chat_scale;
            let queue_rect = [
                2.0 * gs * chat_scale,
                chat_bottom,
                (chat_width + 6.0) * gs * chat_scale,
                queue_h,
            ];
            self.queue_region = Some(queue_rect);
            elements.push(MenuElement::Rect {
                x: queue_rect[0],
                y: queue_rect[1],
                w: queue_rect[2],
                h: queue_rect[3],
                corner_radius: 0.0,
                color: vanilla_black_fill(self.options.text_background_opacity),
            });
            let queue_component = Component::translate(
                "chat.queue",
                vec![Argument::Number(queue_count.to_string())],
            );
            let mut queue_spans = format_component_spans(&queue_component, common::WHITE);
            let queue_alpha = 0.5 * self.options.effective_text_opacity();
            for span in &mut queue_spans {
                span.color[3] *= queue_alpha;
            }
            elements.push(MenuElement::McText {
                x: origin,
                y: chat_bottom + gs * chat_scale,
                spans: queue_spans,
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
            if self.open && common::hit_test(cursor, queue_rect) {
                let tooltip = Component::translate("chat.queue.tooltip", Vec::new());
                common::push_tooltip_lines(
                    elements,
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    component_tooltip_lines(&tooltip),
                );
            }
        }

        if self.open && total_lines > lines_per_page && !display.is_empty() {
            let count = display.len();
            let chat_height = count as f32 * lh;
            let virtual_height = total_lines as f32 * lh;
            let bar_h = (chat_height * chat_height / virtual_height).max(gs * chat_scale);
            let offset = self.scroll_pos as f32 * chat_height / total_lines as f32;
            let bar_bottom = chat_bottom - offset;
            let bar_top = bar_bottom - bar_h;
            let x = origin + (chat_width + 4.0) * gs * chat_scale;
            let color = if self.new_message_since_scroll {
                common::rgb(0xcc3333)
            } else {
                common::rgb(0x3333aa)
            };
            let alpha = if offset > chat_bottom { 170.0 } else { 96.0 } / 255.0;
            elements.push(MenuElement::Rect {
                x,
                y: bar_top,
                w: 2.0 * gs * chat_scale,
                h: bar_h,
                corner_radius: 0.0,
                color: [color[0], color[1], color[2], alpha],
            });
            elements.push(MenuElement::Rect {
                x: x + gs * chat_scale,
                y: bar_top,
                w: gs * chat_scale,
                h: bar_h,
                corner_radius: 0.0,
                color: [0.8, 0.8, 0.8, alpha],
            });
        }

        if self.open {
            let ui_fs = common::FONT_SIZE * gs;
            let input_h = INPUT_HEIGHT * gs;
            // Vanilla pins the input as a full-width bar at the very bottom of
            // the screen: fill(2, height-14, width-2, height-2).
            let bar_y = screen_h - 14.0 * gs;
            let text_y = bar_y + (input_h - ui_fs) / 2.0;

            elements.push(MenuElement::Rect {
                x: 2.0 * gs,
                y: bar_y,
                w: screen_w - 4.0 * gs,
                h: input_h,
                corner_radius: 0.0,
                color: vanilla_black_fill(self.options.text_background_opacity),
            });

            // ChatScreen's EditBox is independent of ChatComponent scale:
            // x=4, y=height-12, width=screenWidth-4 in GUI coordinates.
            let text_x = 4.0 * gs;
            let inner_w = screen_w - text_x;
            let wf = |s: &str| text_width_fn(s, ui_fs);
            let info = self.input.render_info(inner_w, true, &wf);
            let shown = &self.input.value()[info.display_start..info.display_end];

            // The ghost is the inline suggestion suffix, shown only while the
            // caret sits at the end of the input. Commands use Vanilla's
            // Brigadier syntax colors while ordinary messages remain white.
            let presentation = self
                .input
                .value()
                .strip_prefix('/')
                .and_then(|command| command_tree.map(|tree| tree.presentation(command)));
            if let Some(presentation) = presentation.as_ref() {
                let all_spans = command_input_spans(self.input.value(), presentation);
                let visible_spans = slice_spans(&all_spans, info.display_start, info.display_end);
                common::push_field_spans(
                    elements,
                    &info,
                    shown,
                    &visible_spans,
                    text_x,
                    text_y,
                    ui_fs,
                    gs,
                    gs,
                    CARET_COLOR,
                    self.ghost_suffix().map(|g| (g, GHOST_TEXT)),
                    &wf,
                );
            } else {
                common::push_field_text(
                    elements,
                    &info,
                    shown,
                    text_x,
                    text_y,
                    ui_fs,
                    gs,
                    gs,
                    CARET_COLOR,
                    self.ghost_suffix().map(|g| (g, GHOST_TEXT)),
                    &wf,
                );
            }

            if !self.suggestions.is_empty() {
                let row_h = SUGGEST_ROW_H * gs;
                let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
                let max_offset = self.suggestions.len() - visible;
                self.suggest_offset = self.suggest_offset.min(max_offset);
                let offset = self.suggest_offset;

                let pad = gs;
                let max_w = self.suggestions[offset..offset + visible]
                    .iter()
                    .map(|s| text_width_fn(&s.text, ui_fs))
                    .fold(0.0_f32, f32::max);
                // Vanilla showSuggestions computes x from the width of the
                // input prefix (not EditBox.x), then the unbordered list moves
                // one pixel left and adds one pixel to the supplied width.
                let supplied_w = max_w + gs;
                let popup_w = supplied_w + gs;
                let anchor_w = text_width_fn(&self.suggest_anchor, ui_fs);
                let popup_x = anchor_w.clamp(0.0, (screen_w - supplied_w).max(0.0)) - gs;
                let popup_top = bar_y - gs - visible as f32 * row_h;
                let mouse_moved = self.last_suggestion_cursor != Some(cursor);
                self.last_suggestion_cursor = Some(cursor);
                let mut hovered_idx = None;

                for i in 0..visible {
                    let idx = offset + i;
                    let row_y = popup_top + i as f32 * row_h;
                    let rect = [popup_x, row_y, popup_w, row_h];
                    self.suggestion_regions.push((idx, rect));
                    if mouse_moved && common::hit_test(cursor, rect) {
                        hovered_idx = Some(idx);
                    }
                    elements.push(MenuElement::Rect {
                        x: popup_x,
                        y: row_y,
                        w: popup_w,
                        h: row_h,
                        corner_radius: 0.0,
                        color: vanilla_black_fill(SUGGEST_BG_ALPHA),
                    });
                    elements.push(MenuElement::Text {
                        x: popup_x + pad,
                        y: row_y + (row_h - ui_fs) / 2.0,
                        text: self.suggestions[idx].text.clone(),
                        scale: ui_fs,
                        color: if idx == self.suggest_index {
                            SUGGEST_SELECTED
                        } else {
                            SUGGEST_TEXT
                        },
                        centered: false,
                    });
                }
                if let Some(idx) = hovered_idx {
                    self.suggest_index = idx;
                    if let Some(tooltip) = self.suggestions[idx].tooltip.as_ref() {
                        let lines = component_tooltip_lines(tooltip);
                        common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
                    }
                }
            } else {
                let mut usage: Vec<(String, [f32; 4])> = Vec::new();
                let mut usage_start = 0usize;
                if let Some(presentation) = presentation.as_ref() {
                    usage_start = presentation.usage_start.saturating_add(1);
                    usage.extend(
                        presentation
                            .usage
                            .iter()
                            .cloned()
                            .map(|line| (line, common::rgb(0xaaaaaa))),
                    );
                    if presentation
                        .tokens
                        .iter()
                        .any(|token| token.kind == CommandTokenKind::Unparsed)
                        && usage.is_empty()
                    {
                        let key = if presentation.tokens.len() <= 1 {
                            "command.unknown.command"
                        } else {
                            "command.unknown.argument"
                        };
                        usage.push((
                            crate::lang::translate(key).unwrap_or(key).to_owned(),
                            common::rgb(0xff5555),
                        ));
                    }
                    if self.options.visibility == ChatVisibilitySetting::Hidden {
                        let key = "chat_screen.commands_not_allowed";
                        usage.push((
                            crate::lang::translate(key).unwrap_or(key).to_owned(),
                            common::rgb(0xff5555),
                        ));
                    }
                } else if !self.input.value().trim().is_empty()
                    && self.options.visibility != ChatVisibilitySetting::Full
                {
                    let key = "chat_screen.messages_not_allowed";
                    usage.push((
                        crate::lang::translate(key).unwrap_or(key).to_owned(),
                        common::rgb(0xff5555),
                    ));
                }

                if !usage.is_empty() {
                    let usage_w = usage
                        .iter()
                        .map(|(line, _)| text_width_fn(line, ui_fs))
                        .fold(0.0_f32, f32::max);
                    let prefix_end = usage_start.min(self.input.value().len());
                    let usage_x = (text_x
                        + text_width_fn(&self.input.value()[..prefix_end], ui_fs))
                    .clamp(text_x, (screen_w - usage_w - gs).max(text_x));
                    for (index, (line, color)) in usage.into_iter().enumerate() {
                        let y = screen_h - 27.0 * gs - 12.0 * gs * index as f32;
                        elements.push(MenuElement::Rect {
                            x: usage_x - gs,
                            y,
                            w: usage_w + 2.0 * gs,
                            h: 12.0 * gs,
                            corner_radius: 0.0,
                            color: vanilla_black_fill(SUGGEST_BG_ALPHA),
                        });
                        elements.push(MenuElement::Text {
                            x: usage_x,
                            y: y + 2.0 * gs,
                            text: line,
                            scale: ui_fs,
                            color,
                            centered: false,
                        });
                    }
                }
            }

            if !self.has_pending_modal_prompt()
                && let Some(style) = self.style_at(cursor)
                && let Some(hover) = &style.hover_event
            {
                push_hover_tooltip(
                    elements,
                    hover,
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    advanced_item_tooltips,
                );
            }

            if clicked && !self.has_pending_modal_prompt() {
                return self.handle_click(cursor, shift, inner_w, &wf);
            }
        }

        None
    }

    pub fn build_modal_prompt(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
    ) -> Option<ChatUiAction> {
        if let Some(pending) = self.pending_command.clone() {
            let was_visible = self.command_buttons.is_some();
            self.command_buttons = Some(push_command_prompt(
                elements,
                &pending.command,
                pending.kind,
                cursor,
                screen_w,
                screen_h,
                gs,
            ));
            if clicked && was_visible {
                let (accept, cancel) = self.command_buttons?;
                if common::hit_test(cursor, accept) {
                    self.pending_command = None;
                    self.command_buttons = None;
                    match pending.kind {
                        CommandConfirmationKind::SignatureRequired => {
                            common::set_clipboard(&format!("/{}", pending.command));
                        }
                        CommandConfirmationKind::PermissionsRequired
                        | CommandConfirmationKind::ParseErrors => {
                            return Some(ChatUiAction::RunCommandUnsigned(pending.command));
                        }
                    }
                } else if common::hit_test(cursor, cancel) {
                    self.pending_command = None;
                    self.command_buttons = None;
                }
            }
            return None;
        }

        let url = self.pending_link.clone()?;
        let was_visible = self.link_buttons.is_some();
        self.link_buttons = Some(push_link_prompt(
            elements, &url, cursor, screen_w, screen_h, gs,
        ));
        if clicked && was_visible {
            return self.handle_link_prompt_click(cursor);
        }
        None
    }
}

fn push_command_prompt(
    elements: &mut Vec<MenuElement>,
    command: &str,
    kind: CommandConfirmationKind,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
) -> ([f32; 4], [f32; 4]) {
    common::push_overlay(elements, screen_w, screen_h, 0.5);
    let ui_fs = common::FONT_SIZE * gs;
    let title =
        crate::lang::translate("multiplayer.confirm_command.title").unwrap_or("Confirm Command");
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 44.0 * gs,
        text: title.to_owned(),
        scale: ui_fs,
        color: common::WHITE,
        centered: true,
    });
    let (message_key, message_fallback, accept_key, accept_fallback) = match kind {
        CommandConfirmationKind::SignatureRequired => (
            "multiplayer.confirm_command.signature_required",
            "This command requires a signed message argument.",
            "chat.copy",
            "Copy to Clipboard",
        ),
        CommandConfirmationKind::PermissionsRequired => (
            "multiplayer.confirm_command.permissions_required",
            "This command requires elevated permissions.",
            "multiplayer.confirm_command.run_command",
            "Run Command",
        ),
        CommandConfirmationKind::ParseErrors => (
            "multiplayer.confirm_command.parse_errors",
            "This command could not be parsed safely.",
            "multiplayer.confirm_command.run_command",
            "Run Command",
        ),
    };
    let message = crate::lang::translate(message_key).unwrap_or(message_fallback);
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 24.0 * gs,
        text: message.to_owned(),
        scale: ui_fs,
        color: common::WHITE,
        centered: true,
    });
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 6.0 * gs,
        text: format!("/{command}"),
        scale: ui_fs,
        color: common::rgb(0xffff55),
        centered: true,
    });
    let bw = 150.0 * gs;
    let bh = 20.0 * gs;
    let gap = 8.0 * gs;
    let bx = (screen_w - (2.0 * bw + gap)) / 2.0;
    let by = screen_h / 2.0 + 22.0 * gs;
    let accept = [bx, by, bw, bh];
    let cancel = [bx + bw + gap, by, bw, bh];
    let accept_label = crate::lang::translate(accept_key).unwrap_or(accept_fallback);
    let cancel_label = crate::lang::translate("gui.back").unwrap_or("Back");
    common::push_button(
        elements,
        cursor,
        accept[0],
        accept[1],
        accept[2],
        accept[3],
        gs,
        ui_fs,
        accept_label,
        true,
    );
    common::push_button(
        elements,
        cursor,
        cancel[0],
        cancel[1],
        cancel[2],
        cancel[3],
        gs,
        ui_fs,
        cancel_label,
        true,
    );
    (accept, cancel)
}

fn push_link_prompt(
    elements: &mut Vec<MenuElement>,
    url: &str,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
) -> ([f32; 4], [f32; 4], [f32; 4]) {
    // Vanilla's untrusted ConfirmLinkScreen uses Yes / Copy to Clipboard / No.
    common::push_overlay(elements, screen_w, screen_h, 0.5);
    let ui_fs = common::FONT_SIZE * gs;
    let title = crate::lang::translate("chat.link.confirm")
        .unwrap_or("Are you sure you want to open the following website?");
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 54.0 * gs,
        text: title.to_owned(),
        scale: ui_fs,
        color: common::WHITE,
        centered: true,
    });
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 34.0 * gs,
        text: url.to_owned(),
        scale: ui_fs,
        color: common::WHITE,
        centered: true,
    });
    let warning = crate::lang::translate("chat.link.warning")
        .unwrap_or("Never open links from people that you don't trust!");
    elements.push(MenuElement::Text {
        x: screen_w / 2.0,
        y: screen_h / 2.0 - 16.0 * gs,
        text: warning.to_owned(),
        scale: ui_fs,
        color: common::rgb(0xffcccc),
        centered: true,
    });
    let bw = 100.0 * gs;
    let bh = 20.0 * gs;
    let gap = 4.0 * gs;
    let total = 3.0 * bw + 2.0 * gap;
    let bx = (screen_w - total) / 2.0;
    let by = screen_h / 2.0 + 12.0 * gs;
    let yes = [bx, by, bw, bh];
    let copy = [bx + bw + gap, by, bw, bh];
    let no = [bx + 2.0 * (bw + gap), by, bw, bh];
    let yes_label = crate::lang::translate("gui.yes").unwrap_or("Yes");
    let copy_label = crate::lang::translate("chat.copy").unwrap_or("Copy to Clipboard");
    let no_label = crate::lang::translate("gui.no").unwrap_or("No");
    common::push_button(
        elements, cursor, yes[0], yes[1], yes[2], yes[3], gs, ui_fs, yes_label, true,
    );
    common::push_button(
        elements, cursor, copy[0], copy[1], copy[2], copy[3], gs, ui_fs, copy_label, true,
    );
    common::push_button(
        elements, cursor, no[0], no[1], no[2], no[3], gs, ui_fs, no_label, true,
    );
    (yes, copy, no)
}

fn push_hover_tooltip(
    elements: &mut Vec<MenuElement>,
    hover: &HoverEvent,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    advanced_item_tooltips: bool,
) {
    let lines = match hover {
        HoverEvent::Text(component) => component_tooltip_lines(component),
        HoverEvent::Item(value) => item_tooltip_lines(value, advanced_item_tooltips),
        HoverEvent::Entity(value) if advanced_item_tooltips => entity_tooltip_lines(value),
        HoverEvent::Entity(_) => Vec::new(),
    };
    if !lines.is_empty() {
        common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
    }
}

fn component_tooltip_lines(component: &Component) -> Vec<TooltipLine> {
    let spans = format_component_spans(component, common::WHITE);
    let mut lines = vec![TooltipLine {
        spans: Vec::new(),
        right_align: false,
    }];
    for span in spans {
        let mut first = true;
        for part in span.text.split('\n') {
            if !first {
                lines.push(TooltipLine {
                    spans: Vec::new(),
                    right_align: false,
                });
            }
            first = false;
            if !part.is_empty() {
                let mut piece = span.clone();
                piece.text = part.to_owned();
                lines.last_mut().unwrap().spans.push(piece);
            }
        }
    }
    if lines.last().is_some_and(|line| line.spans.is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    lines
}

fn entity_tooltip_lines(value: &serde_json::Value) -> Vec<TooltipLine> {
    let Some(map) = value.as_object() else {
        return vec![TooltipLine::new(value.to_string(), common::WHITE)];
    };
    let mut lines = Vec::new();
    if let Some(name) = map.get("name")
        && let Ok(component) = Component::from_value(name)
    {
        lines.extend(component_tooltip_lines(&component));
    }

    if let Some(id) = map.get("id").and_then(serde_json::Value::as_str) {
        let path = id.split(':').next_back().unwrap_or(id);
        let key = format!("entity.minecraft.{path}");
        let translated = crate::lang::translate(&key).unwrap_or(id);
        let type_component = Component::translate(
            "gui.entity_tooltip.type",
            vec![Argument::String(translated.to_owned())],
        );
        let mut type_lines = component_tooltip_lines(&type_component);
        for line in &mut type_lines {
            for span in &mut line.spans {
                span.color = common::rgb(0xaaaaaa);
            }
        }
        lines.extend(type_lines);
    }
    if let Some(uuid) = map.get("uuid").and_then(serde_json::Value::as_str) {
        lines.push(TooltipLine::new(uuid.to_owned(), common::rgb(0xaaaaaa)));
    }
    lines
}

fn item_tooltip_lines(value: &serde_json::Value, advanced: bool) -> Vec<TooltipLine> {
    let Some(map) = value.as_object() else {
        return vec![TooltipLine::new(value.to_string(), common::WHITE)];
    };
    let id = map
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("minecraft:air");
    let components = map.get("components").and_then(serde_json::Value::as_object);
    let tooltip_display =
        component_value(components, "tooltip_display").and_then(serde_json::Value::as_object);
    if tooltip_display
        .and_then(|display| display.get("hide_tooltip"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let kind = id.parse::<ItemKind>().ok();
    let enchanted = component_value(components, "enchantments")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|entries| !entries.is_empty());
    let rarity = component_value(components, "rarity")
        .and_then(serde_json::Value::as_str)
        .and_then(item_rarity_from_name)
        .or_else(|| kind.and_then(get_default_component::<Rarity>))
        .unwrap_or(Rarity::Common);
    let rarity = if enchanted {
        match rarity {
            Rarity::Common | Rarity::Uncommon => Rarity::Rare,
            Rarity::Rare => Rarity::Epic,
            Rarity::Epic => Rarity::Epic,
        }
    } else {
        rarity
    };
    let rarity_color = item_rarity_color(rarity);

    let path = id.split(':').next_back().unwrap_or(id);
    let item_key = format!("item.minecraft.{path}");
    let block_key = format!("block.minecraft.{path}");
    let default_name = crate::lang::translate(&item_key)
        .or_else(|| crate::lang::translate(&block_key))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::lang::title_case_snake(path));

    let custom_name = component_value(components, "custom_name")
        .and_then(|value| Component::from_value(value).ok());
    let item_name = component_value(components, "item_name")
        .and_then(|value| Component::from_value(value).ok());
    let mut lines = if let Some(name) = custom_name.as_ref().or(item_name.as_ref()) {
        let mut lines = component_tooltip_lines(name);
        for line in &mut lines {
            for span in &mut line.spans {
                // Parent rarity/italic styles in ItemStack#getStyledHoverName
                // inherit only where the supplied component does not override
                // them. The native component decoder has already resolved
                // explicit colors; preserve non-white explicit colors here.
                if span.color == common::WHITE {
                    span.color = rarity_color;
                }
                if custom_name.is_some() {
                    span.italic = true;
                }
            }
        }
        lines
    } else {
        vec![TooltipLine::new(default_name.to_owned(), rarity_color)]
    };

    for component_name in ["enchantments", "stored_enchantments"] {
        if !tooltip_component_visible(tooltip_display, component_name) {
            continue;
        }
        if let Some(entries) =
            component_value(components, component_name).and_then(serde_json::Value::as_object)
        {
            for (enchantment, level) in entries {
                let Some(level) = level.as_i64() else {
                    continue;
                };
                if level <= 0 {
                    continue;
                }
                lines.push(enchantment_tooltip_line(enchantment, level as i32));
            }
        }
    }

    if tooltip_component_visible(tooltip_display, "lore")
        && let Some(lore) =
            component_value(components, "lore").and_then(serde_json::Value::as_array)
    {
        for line in lore {
            if let Ok(component) = Component::from_value(line) {
                lines.extend(component_tooltip_lines(&component));
            }
        }
    }

    if tooltip_component_visible(tooltip_display, "unbreakable")
        && component_value(components, "unbreakable").is_some()
    {
        lines.push(TooltipLine::new(
            crate::lang::translate("item.unbreakable")
                .unwrap_or("Unbreakable")
                .to_owned(),
            common::rgb(0x5555ff),
        ));
    }

    if advanced {
        let damage = component_value(components, "damage")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0) as i32;
        let max_damage = component_value(components, "max_damage")
            .and_then(serde_json::Value::as_i64)
            .map(|value| value as i32)
            .or_else(|| {
                kind.and_then(get_default_component::<MaxDamage>)
                    .map(|value| value.amount)
            })
            .unwrap_or(0);
        if damage > 0 && max_damage > 0 && tooltip_component_visible(tooltip_display, "damage") {
            let remaining = (max_damage - damage).max(0);
            if crate::lang::translate("item.durability").is_some() {
                let component = Component::translate(
                    "item.durability",
                    vec![
                        Argument::Number(remaining.to_string()),
                        Argument::Number(max_damage.to_string()),
                    ],
                );
                lines.extend(component_tooltip_lines(&component));
            } else {
                lines.push(TooltipLine::new(
                    format!("Durability: {remaining} / {max_damage}"),
                    common::WHITE,
                ));
            }
        }
        lines.push(TooltipLine::new(id.to_owned(), common::rgb(0x555555)));
        let component_count = components.map_or(0, serde_json::Map::len);
        if component_count > 0 {
            if crate::lang::translate("item.components").is_some() {
                let component = Component::translate(
                    "item.components",
                    vec![Argument::Number(component_count.to_string())],
                );
                let mut tooltip = component_tooltip_lines(&component);
                for line in &mut tooltip {
                    for span in &mut line.spans {
                        span.color = common::rgb(0x555555);
                    }
                }
                lines.extend(tooltip);
            } else {
                lines.push(TooltipLine::new(
                    format!("{component_count} component(s)"),
                    common::rgb(0x555555),
                ));
            }
        }
    }

    lines
}

fn component_value<'a>(
    components: Option<&'a serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> Option<&'a serde_json::Value> {
    components.and_then(|components| {
        components
            .get(name)
            .or_else(|| components.get(&format!("minecraft:{name}")))
    })
}

fn tooltip_component_visible(
    display: Option<&serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> bool {
    let Some(hidden) = display
        .and_then(|display| display.get("hidden_components"))
        .and_then(serde_json::Value::as_array)
    else {
        return true;
    };
    !hidden.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|hidden| hidden == name || hidden == format!("minecraft:{name}"))
    })
}

fn item_rarity_from_name(name: &str) -> Option<Rarity> {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "common" => Some(Rarity::Common),
        "uncommon" => Some(Rarity::Uncommon),
        "rare" => Some(Rarity::Rare),
        "epic" => Some(Rarity::Epic),
        _ => None,
    }
}

fn item_rarity_color(rarity: Rarity) -> [f32; 4] {
    match rarity {
        Rarity::Common => common::WHITE,
        Rarity::Uncommon => common::rgb(0xffff55),
        Rarity::Rare => common::rgb(0x55ffff),
        Rarity::Epic => common::rgb(0xff55ff),
    }
}

fn enchantment_level_fallback(level: i32) -> String {
    match level {
        1 => "I".to_owned(),
        2 => "II".to_owned(),
        3 => "III".to_owned(),
        4 => "IV".to_owned(),
        5 => "V".to_owned(),
        6 => "VI".to_owned(),
        7 => "VII".to_owned(),
        8 => "VIII".to_owned(),
        9 => "IX".to_owned(),
        10 => "X".to_owned(),
        _ => level.to_string(),
    }
}

fn enchantment_tooltip_line(id: &str, level: i32) -> TooltipLine {
    let path = id.split(':').next_back().unwrap_or(id);
    let key = format!("enchantment.minecraft.{path}");
    let name = crate::lang::translate(&key)
        .map(str::to_owned)
        .unwrap_or_else(|| crate::lang::title_case_snake(path));
    let curse = matches!(path, "binding_curse" | "vanishing_curse");
    let single_level = matches!(
        path,
        "aqua_affinity"
            | "binding_curse"
            | "channeling"
            | "flame"
            | "infinity"
            | "mending"
            | "multishot"
            | "silk_touch"
            | "vanishing_curse"
    );
    let text = if level == 1 && single_level {
        name.to_owned()
    } else {
        let level_key = format!("enchantment.level.{level}");
        let level_text = crate::lang::translate(&level_key)
            .map(str::to_owned)
            .unwrap_or_else(|| enchantment_level_fallback(level));
        format!("{name} {level_text}")
    };
    TooltipLine::new(
        text,
        if curse {
            common::rgb(0xff5555)
        } else {
            common::rgb(0xaaaaaa)
        },
    )
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

fn sort_suggestions_with_partial_first(
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

fn command_input_spans(input: &str, presentation: &CommandPresentation) -> Vec<TextSpan> {
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
    for token in &presentation.tokens {
        let start = token.range.start.saturating_add(1).min(input.len());
        let end = token.range.end.saturating_add(1).min(input.len());
        if cursor < start {
            out.push(TextSpan::new(input[cursor..start].to_owned(), literal));
        }
        if start < end {
            let color = match token.kind {
                CommandTokenKind::Literal => literal,
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

fn slice_spans(spans: &[TextSpan], start: usize, end: usize) -> Vec<TextSpan> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for span in spans {
        let span_start = offset;
        let span_end = offset + span.text.len();
        offset = span_end;
        if span_end <= start || span_start >= end {
            continue;
        }
        let local_start = start.saturating_sub(span_start).min(span.text.len());
        let local_end = end.saturating_sub(span_start).min(span.text.len());
        if local_start < local_end {
            let mut piece = span.clone();
            piece.text = span.text[local_start..local_end].to_owned();
            out.push(piece);
        }
    }
    out
}

fn legacy_color(code: char) -> Option<[f32; 4]> {
    let rgb = match code.to_ascii_lowercase() {
        '0' => 0x000000,
        '1' => 0x0000aa,
        '2' => 0x00aa00,
        '3' => 0x00aaaa,
        '4' => 0xaa0000,
        '5' => 0xaa00aa,
        '6' => 0xffaa00,
        '7' => 0xaaaaaa,
        '8' => 0x555555,
        '9' => 0x5555ff,
        'a' => 0x55ff55,
        'b' => 0x55ffff,
        'c' => 0xff5555,
        'd' => 0xff55ff,
        'e' => 0xffff55,
        'f' => 0xffffff,
        _ => return None,
    };
    Some(common::rgb(rgb))
}

/// Vanilla's `StringDecomposer.iterateFormatted` pass over each component text
/// segment. Legacy formatting never carries into the next TextSpan because
/// each segment supplies its own style as both currentStyle and resetStyle.
fn legacy_format_spans(spans: &[TextSpan], colors_enabled: bool) -> Vec<TextSpan> {
    let mut out = Vec::new();
    for base in spans {
        let mut current = base.clone();
        current.text.clear();
        let mut buffer = String::new();
        let flush = |out: &mut Vec<TextSpan>, current: &TextSpan, buffer: &mut String| {
            if !buffer.is_empty() {
                let mut span = current.clone();
                span.text = std::mem::take(buffer);
                out.push(span);
            }
        };

        let mut chars = base.text.chars();
        while let Some(ch) = chars.next() {
            if ch != '\u{00a7}' {
                buffer.push(ch);
                continue;
            }
            let Some(code) = chars.next() else {
                // A trailing section sign terminates Vanilla's formatted
                // iterator without emitting the section sign itself.
                break;
            };
            let lower = code.to_ascii_lowercase();
            let recognized =
                legacy_color(lower).is_some() || matches!(lower, 'k' | 'l' | 'm' | 'n' | 'o' | 'r');
            if !recognized {
                // StringDecomposer consumes even an unknown code pair.
                continue;
            }
            flush(&mut out, &current, &mut buffer);
            if !colors_enabled {
                // Chat Colors OFF strips only the legacy code. The component's
                // ordinary Style is preserved and the stripped text is later
                // rendered normally.
                continue;
            }
            if let Some(color) = legacy_color(lower) {
                current.color = color;
                current.bold = false;
                current.italic = false;
                current.strikethrough = false;
                current.underline = false;
                current.obfuscated = false;
                continue;
            }
            match lower {
                'k' => current.obfuscated = true,
                'l' => current.bold = true,
                'm' => current.strikethrough = true,
                'n' => current.underline = true,
                'o' => current.italic = true,
                'r' => {
                    current = base.clone();
                    current.text.clear();
                }
                _ => unreachable!(),
            }
        }
        flush(&mut out, &current, &mut buffer);
    }
    out
}

/// Word-wraps styled spans to `max_w` gui-space units like vanilla
/// `Font.split`: `width0` measures styled runs at gui-scale 1, so fonts, bold
/// and inline objects count as in `StringSplitter`. Returns one
/// `Vec<TextSpan>` per display line.
pub(crate) fn wrap_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<Vec<TextSpan>> {
    // Preserve every character's original style. Vanilla StringSplitter keeps
    // styled spaces in-place and drops only the particular space selected as
    // a line-break delimiter. Reconstructing separators from the following
    // word moves styles across component boundaries (e.g. the C04 trailing
    // underline/strike space appeared before the word instead of after it).
    let mut chars: StyledLine = Vec::new();
    for s in spans {
        let style = CharStyle(s.with_text(String::new()));
        chars.extend(s.text.chars().map(|ch| (ch, style.clone())));
    }
    if chars.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines: Vec<StyledLine> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut width = 0.0f32;
        let mut had_non_zero = false;
        let mut last_space: Option<usize> = None;
        let mut i = start;
        let mut split = false;

        while i < chars.len() {
            let (ch, style) = &chars[i];
            if *ch == '\n' {
                lines.push(chars[start..i].to_vec());
                start = i + 1;
                split = true;
                break;
            }
            if *ch == ' ' {
                last_space = Some(i);
            }

            let char_span = merge_chars(&[(*ch, style.clone())]);
            let char_width = width0(&char_span);
            width += char_width;
            if had_non_zero && width > max_w {
                if let Some(space) = last_space {
                    // `FlatComponents.splitAt(lineBreak, 1, ...)`: the chosen
                    // delimiter space is omitted from both display lines.
                    lines.push(chars[start..space].to_vec());
                    start = space + 1;
                } else {
                    lines.push(chars[start..i].to_vec());
                    start = i;
                }
                split = true;
                break;
            }
            had_non_zero |= char_width != 0.0;
            i += 1;
        }

        if !split {
            lines.push(chars[start..].to_vec());
            break;
        }
    }

    lines.iter().map(|line| merge_chars(line)).collect()
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

    /// 10 units per char, 20 when bold.
    fn width(spans: &[TextSpan]) -> f32 {
        spans
            .iter()
            .map(|s| s.text.chars().count() as f32 * if s.bold { 20.0 } else { 10.0 })
            .sum()
    }

    #[test]
    fn open_url_respects_validation_link_toggle_and_confirmation_option() {
        let mut chat = ChatState::new();

        assert!(
            chat.request_open_url("file:///tmp/not-allowed".to_owned())
                .is_none()
        );
        assert!(!chat.has_pending_modal_prompt());

        let mut options = ChatOptions {
            links: false,
            links_prompt: false,
            ..ChatOptions::default()
        };
        chat.set_options(options);
        assert!(
            chat.request_open_url("https://example.com".to_owned())
                .is_none()
        );
        assert!(!chat.has_pending_modal_prompt());

        options.links = true;
        options.links_prompt = true;
        chat.set_options(options);
        assert!(
            chat.request_open_url("https://example.com/path".to_owned())
                .is_none()
        );
        assert!(chat.has_pending_modal_prompt());
        chat.handle_escape();
        assert!(!chat.has_pending_modal_prompt());

        options.links_prompt = false;
        chat.set_options(options);
        assert!(matches!(
            chat.request_open_url("http://example.com".to_owned()),
            Some(ChatUiAction::OpenUrl(ref url)) if url == "http://example.com"
        ));
    }

    #[test]
    fn command_confirmation_is_modal_and_requires_explicit_acceptance() {
        let mut chat = ChatState::new();
        chat.request_command_confirmation(
            "unknown command".to_owned(),
            CommandConfirmationKind::ParseErrors,
        );
        assert!(chat.has_pending_modal_prompt());

        let mut elements = Vec::new();
        assert!(
            chat.build_modal_prompt(&mut elements, 800.0, 600.0, 1.0, (0.0, 0.0), false)
                .is_none()
        );
        let (accept, _) = chat.command_buttons.expect("command buttons");
        let cursor = (accept[0] + accept[2] / 2.0, accept[1] + accept[3] / 2.0);
        assert!(matches!(
            chat.build_modal_prompt(&mut elements, 800.0, 600.0, 1.0, cursor, true),
            Some(ChatUiAction::RunCommandUnsigned(ref command)) if command == "unknown command"
        ));
        assert!(!chat.has_pending_modal_prompt());

        chat.request_command_confirmation(
            "msg Steve hello".to_owned(),
            CommandConfirmationKind::SignatureRequired,
        );
        assert!(chat.has_pending_modal_prompt());
        chat.handle_escape();
        assert!(!chat.has_pending_modal_prompt());
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
    fn wrap_preserves_trailing_space_style_across_component_runs() {
        let white = common::WHITE;
        let mut italic = span("ITALIC ", white);
        italic.italic = true;
        let mut underline = span("UNDERLINE ", white);
        underline.underline = true;
        let mut strike = span("STRIKE ", white);
        strike.strikethrough = true;

        let lines = wrap_spans(&[italic, underline, strike], 10_000.0, &|spans| {
            spans
                .iter()
                .map(|span| span.text.chars().count() as f32)
                .sum()
        });
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][0].text, "ITALIC ");
        assert!(lines[0][0].italic);
        assert!(!lines[0][0].underline);
        assert_eq!(lines[0][1].text, "UNDERLINE ");
        assert!(lines[0][1].underline);
        assert!(!lines[0][1].italic);
        assert_eq!(lines[0][2].text, "STRIKE ");
        assert!(lines[0][2].strikethrough);
        assert!(!lines[0][2].underline);
    }

    #[test]
    fn chat_row_backgrounds_render_before_history_text() {
        let mut chat = ChatState::new();
        chat.push_message(vec![span("older", common::WHITE)]);
        chat.push_message(vec![span("newer", common::WHITE)]);
        chat.open();

        let text_width = |text: &str, scale: f32| text.chars().count() as f32 * scale;
        let spans_width = |spans: &[TextSpan], scale: f32| {
            spans
                .iter()
                .map(|span| span.text.chars().count() as f32 * scale)
                .sum()
        };
        let mut elements = Vec::new();
        let action = chat.build(
            &mut elements,
            ChatBuildContext {
                screen_w: 640.0,
                screen_h: 360.0,
                gui_scale: 1.0,
                cursor: (0.0, 0.0),
                clicked: false,
                shift: false,
                command_tree: None,
                advanced_item_tooltips: false,
                text_width_fn: &text_width,
                spans_width_fn: &spans_width,
            },
        );
        assert!(action.is_none());

        let first_history_text = elements
            .iter()
            .position(|element| {
                let MenuElement::McText { spans, .. } = element else {
                    return false;
                };
                let text: String = spans.iter().map(|span| span.text.as_str()).collect();
                text == "newer" || text == "older"
            })
            .expect("history text element");
        let backgrounds_before_text = elements[..first_history_text]
            .iter()
            .filter(|element| matches!(element, MenuElement::Rect { .. }))
            .count();
        assert!(
            backgrounds_before_text >= 2,
            "all visible row backgrounds must be emitted before history text"
        );
    }

    #[test]
    fn wrap_spans_hard_breaks_long_word() {
        let lines = wrap_spans(&[span("aaaaaaa", [1.0; 4])], 30.0, &width);
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        assert_eq!(texts, vec!["aaa", "aaa", "a"]);
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
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        assert_eq!(texts, vec!["aa", "bb"]);
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
        let sorted = sort_suggestions_with_partial_first(
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

    #[test]
    fn vanilla_chat_option_defaults_match_26_2_geometry() {
        let options = ChatOptions::default();
        assert_eq!(options.visibility, ChatVisibilitySetting::Full);
        assert_eq!(options.width_px(), 320.0);
        assert_eq!(options.wrap_width_px(), 320.0);
        assert_eq!(options.height_px(true), 180.0);
        assert_eq!(options.height_px(false), 90.0);
        assert_eq!(options.line_height(), 9.0);
        assert_eq!(options.effective_text_opacity(), 1.0);
        assert_eq!(options.text_background_opacity, 0.5);
        assert!(options.colors);
        assert!(options.links);
        assert!(options.links_prompt);
        assert!(options.auto_suggestions);
        assert!(!options.only_secure);
        assert!(!options.save_drafts);
    }

    #[test]
    fn legacy_formatting_matches_vanilla_segment_reset_rules() {
        let white = common::WHITE;
        let spans = legacy_format_spans(&[span("a§cb§lc§rd", white)], true);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "abcd"
        );
        assert_eq!(spans.len(), 4);
        assert_eq!(spans[0].color, white);
        assert_eq!(spans[1].color, common::rgb(0xff5555));
        assert!(!spans[1].bold);
        assert_eq!(spans[2].color, common::rgb(0xff5555));
        assert!(spans[2].bold);
        assert_eq!(spans[3].color, white);
        assert!(!spans[3].bold);
    }

    #[test]
    fn chat_colors_off_strips_only_legacy_codes() {
        let mut base = span("a§cb§lc§rd", common::rgb(0x55ffff));
        base.italic = true;
        let spans = legacy_format_spans(&[base], false);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "abcd"
        );
        assert!(spans.iter().all(|span| span.color == common::rgb(0x55ffff)));
        assert!(spans.iter().all(|span| span.italic));
        assert!(spans.iter().all(|span| !span.bold));
    }

    #[test]
    fn visibility_matches_vanilla_chat_abilities() {
        let mut chat = ChatState::new();
        assert!(chat.source_visible(ChatMessageSource::Player, None));
        assert!(chat.source_visible(ChatMessageSource::SystemServer, None));
        assert!(chat.source_visible(ChatMessageSource::SystemClient, None));

        let mut options = ChatOptions {
            visibility: ChatVisibilitySetting::System,
            ..Default::default()
        };
        chat.set_options(options);
        assert!(!chat.source_visible(ChatMessageSource::Player, None));
        assert!(chat.source_visible(ChatMessageSource::SystemServer, None));
        assert!(chat.source_visible(ChatMessageSource::SystemClient, None));

        options.visibility = ChatVisibilitySetting::Hidden;
        chat.set_options(options);
        assert!(!chat.source_visible(ChatMessageSource::Player, None));
        assert!(!chat.source_visible(ChatMessageSource::SystemServer, None));
        // Client-local system messages remain visible in Vanilla.
        assert!(chat.source_visible(ChatMessageSource::SystemClient, None));
    }

    #[test]
    fn save_chat_drafts_restores_and_first_backspace_clears() {
        let mut chat = ChatState::new();
        let options = ChatOptions {
            save_drafts: true,
            ..Default::default()
        };
        chat.set_options(options);
        chat.open();
        set_input(&mut chat, "draft text");
        chat.close();
        chat.open();
        assert_eq!(chat.input.value(), "draft text");
        assert!(chat.is_restored_draft);

        let event = TextInputEvent::Key {
            code: winit::keyboard::KeyCode::Backspace,
            mods: crate::ui::text_edit::KeyMods {
                shift: false,
                ctrl: false,
                alt: false,
                super_key: false,
            },
        };
        chat.handle_key_input(
            &[event],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            f32::MAX,
            &|_| 0.0,
            None,
        );
        assert_eq!(chat.input.value(), "");
        assert!(!chat.is_restored_draft);
        assert!(chat.latest_draft.is_none());
    }

    #[test]
    fn delayed_player_chat_queues_and_accepts_one() {
        let mut chat = ChatState::new();
        let options = ChatOptions {
            delay_secs: 5.0,
            ..Default::default()
        };
        chat.set_options(options);
        chat.previous_message_time = Some(Instant::now());
        chat.push_message_with_source(
            vec![span("queued", common::WHITE)],
            None,
            ChatMessageSource::Player,
            None,
        );
        assert!(chat.messages.is_empty());
        assert_eq!(chat.delayed_messages.len(), 1);
        chat.accept_next_delayed_message();
        assert_eq!(chat.messages.len(), 1);
        assert!(chat.delayed_messages.is_empty());
        assert_eq!(chat.messages.back().unwrap().spans[0].text, "queued");
    }

    #[test]
    fn delayed_validation_error_acknowledges_invalid_signature_as_hidden() {
        let mut chat = ChatState::new();
        let options = ChatOptions {
            delay_secs: 5.0,
            ..Default::default()
        };
        chat.set_options(options);
        chat.previous_message_time = Some(Instant::now());
        let signature = [0x7au8; 256];
        chat.push_validation_error(
            vec![span("validation error", common::rgb(0xff5555))],
            Some(signature),
        );
        assert_eq!(chat.delayed_messages.len(), 1);
        assert!(chat.take_chat_marks().is_empty());

        chat.accept_next_delayed_message();
        assert_eq!(chat.messages.len(), 1);
        assert!(chat.messages.back().unwrap().signature.is_none());
        assert_eq!(
            chat.take_chat_marks(),
            vec![ChatMark::Processed {
                signature,
                shown: false
            }]
        );
    }

    #[test]
    fn fully_filtered_signed_message_never_renders_and_acknowledges_hidden() {
        let mut chat = ChatState::new();
        let options = ChatOptions {
            delay_secs: 5.0,
            ..Default::default()
        };
        chat.set_options(options);
        let previous = Instant::now();
        chat.previous_message_time = Some(previous);
        let signature = [0x33u8; 256];
        chat.push_fully_filtered(Some(signature));
        assert_eq!(chat.delayed_messages.len(), 1);
        assert!(chat.messages.is_empty());

        chat.accept_next_delayed_message();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.previous_message_time, Some(previous));
        assert_eq!(
            chat.take_chat_marks(),
            vec![ChatMark::Processed {
                signature,
                shown: false
            }]
        );
    }

    #[test]
    fn show_item_respects_tooltip_display_hide() {
        let value = serde_json::json!({
            "id": "minecraft:diamond_sword",
            "components": {
                "minecraft:tooltip_display": {
                    "hide_tooltip": true,
                    "hidden_components": []
                }
            }
        });
        assert!(item_tooltip_lines(&value, false).is_empty());
    }

    #[test]
    fn show_item_builds_vanilla_ordered_component_lines() {
        let value = serde_json::json!({
            "id": "minecraft:diamond_sword",
            "components": {
                "minecraft:custom_name": {"text":"Blade"},
                "minecraft:rarity": "epic",
                "minecraft:enchantments": {"minecraft:sharpness": 5},
                "minecraft:lore": [{"text":"Lore line","color":"gray"}],
                "minecraft:unbreakable": {},
                "minecraft:damage": 10,
                "minecraft:max_damage": 1561
            }
        });
        let lines = item_tooltip_lines(&value, true);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(text.first().map(String::as_str), Some("Blade"));
        assert!(text.iter().any(|line| line.contains("Sharpness")));
        assert!(text.iter().any(|line| line == "Lore line"));
        assert!(text.iter().any(|line| line.contains("Unbreakable")));
        assert!(text.iter().any(|line| line.contains("1551")));
        assert!(text.iter().any(|line| line == "minecraft:diamond_sword"));
    }

    #[test]
    fn server_suggestion_tooltip_is_preserved() {
        let mut chat = awaiting_chat("/example v");
        let tooltip = Component::text("server tooltip");
        chat.apply_server_suggestions(
            1,
            9,
            vec![ChatSuggestion {
                text: "value".to_owned(),
                tooltip: Some(tooltip.clone()),
            }],
        );
        assert_eq!(chat.suggestions.len(), 1);
        assert_eq!(chat.suggestions[0].text, "value");
        assert_eq!(chat.suggestions[0].tooltip, Some(tooltip));
    }
}
