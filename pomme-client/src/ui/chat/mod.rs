use history::*;
use suggestions::*;
mod confirm;
mod render;
use confirm::ConfirmScreen;
mod history;
mod suggestions;
use std::cell::OnceCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use azalea_inventory::components::{AttributeModifiers, MaxDamage, Rarity};
use azalea_inventory::default_components::get_default_component;
use azalea_registry::builtin::ItemKind;
use simdnbt::owned::{NbtCompound, NbtTag};

use super::common;
use super::common::LINE_HEIGHT;
use crate::app::input::KeyBindings;
use crate::chat_component::{
    Argument, ClickEvent, Component, HoverEvent, ResolvedStyle, list_elements, normalize_identifier,
};
use crate::net::commands::{
    CommandParse, CommandTokenKind, CommandTokenRange, CommandTree, SyntaxError,
    UnattendedCommandCheck, UsageLine,
};
use crate::net::sender::ChatMark;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::text::layout::slice_spans;
pub(crate) use crate::ui::text::layout::{
    StyleHitRegion, clip_spans, push_hit_regions, style_at, wrap_spans,
};
use crate::ui::text::{
    TextSpan, format_component_spans, format_component_spans_with_bindings,
    format_component_spans_with_parent_and_bindings, legacy_format_spans, parse_uuid_value,
    with_alpha,
};
use crate::ui::text_edit::{
    SystemClipboard, TextFieldState, TextInputEvent, truncate_to_utf16, utf16_len,
};

const MAX_MESSAGES: usize = 100;
const CHAT_X: f32 = 4.0;
const BOTTOM_MARGIN: f32 = 40.0;
const MESSAGE_LIFETIME_SECS: f32 = 10.0;
const TAG_TOOLTIP_MAX_WIDTH: f32 = 210.0;
const INPUT_HEIGHT: f32 = 12.0;
/// ChatScreen's EditBox x, in GUI units.
const INPUT_X: f32 = 4.0;
const MAX_MESSAGE_LEN: usize = 256;
/// `Options.getBackgroundColor(Integer.MIN_VALUE)` with the default Text
/// Background "Chat Only", which Pomme doesn't expose.
const INPUT_BG_ALPHA: f32 = 128.0 / 255.0;

const SUGGEST_ROW_H: f32 = 12.0;
const MAX_SUGGESTION_ROWS: usize = 10;
const SUGGEST_BG_ALPHA: f32 = 208.0 / 255.0;

/// Black at `alpha`, as vanilla's chat and suggestion backgrounds fill.
fn black(alpha: f32) -> [f32; 4] {
    [0.0, 0.0, 0.0, alpha]
}

fn push_fill(elements: &mut Vec<MenuElement>, [x, y, w, h]: [f32; 4], color: [f32; 4]) {
    elements.push(MenuElement::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
        color,
    });
}

/// `component` with its style's color set to `rgb`.
fn colored(mut component: Component, rgb: u32) -> Component {
    component.style.color = Some(rgb);
    component
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
#[serde(default)]
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

    /// Vanilla `addMessageToDisplayQueue`'s wrap width, floored. Unbounded at
    /// scale 0, where nothing is drawn.
    fn wrap_width_px(self) -> f32 {
        (self.width_px() / self.scale.clamp(0.0, 1.0)).floor()
    }

    /// Vanilla `extractRenderState`'s `maxWidth`, which rounds up instead.
    fn render_width_px(self) -> f32 {
        (self.width_px() / self.scale.clamp(0.0, 1.0)).ceil()
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

    /// Vanilla `OptionInstance.set` on load: an invalid value falls back to
    /// the option's default instead of being clamped.
    pub fn sanitized(self) -> Self {
        let default = Self::default();
        // `UnitDouble.validateValue`.
        let unit = |v: f32, d: f32| if (0.0..=1.0).contains(&v) { v } else { d };
        // `chatDelay` is `IntRange(0, 60)` in tenths, xmapped through `(int)`.
        let tenths = (self.delay_secs * 10.0) as i32;
        Self {
            opacity: unit(self.opacity, default.opacity),
            line_spacing: unit(self.line_spacing, default.line_spacing),
            text_background_opacity: unit(
                self.text_background_opacity,
                default.text_background_opacity,
            ),
            scale: unit(self.scale, default.scale),
            width: unit(self.width, default.width),
            height_focused: unit(self.height_focused, default.height_focused),
            height_unfocused: unit(self.height_unfocused, default.height_unfocused),
            delay_secs: if (0..=60).contains(&tenths) {
                tenths as f32 / 10.0
            } else {
                default.delay_secs
            },
            ..self
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
                component
                    .siblings
                    .push(colored(Component::text(format!("\n{original}")), 0xaaaaaa));
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

/// Vanilla `ChatComponent.ChatMethod`: which key opened chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMethod {
    Message,
    Command,
}

/// What Escape did on the chat screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatEscape {
    /// A modal or the suggestion list took it, or chat wasn't open.
    Handled,
    Closed,
    /// `InBedChatScreen.onClose`: ask to leave the bed; the screen stays up.
    WakeUp,
}

/// Vanilla `ChatScreen.ExitReason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatExitReason {
    /// Another screen replaced chat (death, chat settings).
    Interrupted,
    /// Escape or the close key.
    Intentional,
    /// Submitted.
    Done,
}

/// Completions and the input range they replace, as vanilla `Suggestions`.
#[derive(Clone, Debug)]
struct SuggestionSet {
    range: Range<usize>,
    list: Vec<ChatSuggestion>,
}

/// Vanilla `CommandSuggestions.pendingSuggestions`.
#[derive(Debug)]
enum PendingSuggestions {
    Done(SuggestionSet),
    /// Waiting on `ClientboundCommandSuggestions` for `request`; `local` is
    /// what an empty answer leaves.
    Awaiting {
        request: String,
        local: SuggestionSet,
    },
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
    ShowDialog(crate::chat_component::DialogHolder),
}

pub struct ChatBuildContext<'a> {
    pub screen_w: f32,
    pub screen_h: f32,
    pub gui_scale: f32,
    pub cursor: (f32, f32),
    pub clicked: bool,
    pub shift: bool,
    /// A screen (a server dialog) covers the chat: it draws as the Hud's
    /// unfocused backlog and takes no input.
    pub covered: bool,
    pub command_tree: Option<&'a CommandTree>,
    pub advanced_item_tooltips: bool,
    pub key_bindings: &'a KeyBindings,
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
    /// The shown line's signature; vanilla's validation-error markers have
    /// none.
    signature: Option<[u8; 256]>,
    /// What the last-seen tracker acknowledges once the line is handled,
    /// so a validation-error marker can ack the invalid packet as hidden.
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

impl CommandConfirmationKind {
    /// The confirmation vanilla asks for after `check`; `None` sends the
    /// command as is.
    pub(crate) fn for_check(check: UnattendedCommandCheck) -> Option<Self> {
        match check {
            UnattendedCommandCheck::NoIssues => None,
            UnattendedCommandCheck::SignatureRequired => Some(Self::SignatureRequired),
            UnattendedCommandCheck::PermissionsRequired => Some(Self::PermissionsRequired),
            UnattendedCommandCheck::ParseErrors => Some(Self::ParseErrors),
        }
    }

    fn message_key(self) -> &'static str {
        match self {
            Self::SignatureRequired => "multiplayer.confirm_command.signature_required",
            Self::PermissionsRequired => "multiplayer.confirm_command.permissions_required",
            Self::ParseErrors => "multiplayer.confirm_command.parse_errors",
        }
    }

    fn accept_key(self) -> &'static str {
        match self {
            // `openSignedCommandSendConfirmationWindow` only offers
            // suggest_command without a screen to return to; chat is one.
            Self::SignatureRequired => "chat.copy",
            Self::PermissionsRequired | Self::ParseErrors => {
                "multiplayer.confirm_command.run_command"
            }
        }
    }
}

#[derive(Clone, Debug)]
struct PendingCommand {
    command: String,
    kind: CommandConfirmationKind,
}

/// The ConfirmScreen vanilla swaps in for ChatScreen; answering it returns to
/// the same live chat input.
#[derive(Clone, Debug)]
enum ChatModal {
    /// `Screen.clickUrlAction`'s untrusted ConfirmLinkScreen.
    Link { url: String },
    /// `ClientPacketListener.openSendConfirmationWindow`.
    Command(PendingCommand),
}

impl ChatModal {
    fn confirm_screen(&self) -> ConfirmScreen {
        match self {
            Self::Link { url } => ConfirmScreen {
                title: Component::translate("chat.link.confirm", Vec::new()),
                message: Component::text(url.clone()),
                warning: Some(colored(
                    Component::translate("chat.link.warning", Vec::new()),
                    0xffcccc,
                )),
                buttons: vec![
                    ("gui.yes", CONFIRM_LINK_BUTTON_WIDTH),
                    ("chat.copy", CONFIRM_LINK_BUTTON_WIDTH),
                    ("gui.no", CONFIRM_LINK_BUTTON_WIDTH),
                ],
            },
            Self::Command(pending) => ConfirmScreen {
                title: Component::translate("multiplayer.confirm_command.title", Vec::new()),
                message: Component::translate(
                    pending.kind.message_key(),
                    vec![Argument::Component(Box::new(colored(
                        Component::text(pending.command.clone()),
                        0xffff55,
                    )))],
                ),
                warning: None,
                buttons: vec![
                    (pending.kind.accept_key(), CONFIRM_BUTTON_WIDTH),
                    ("gui.back", CONFIRM_BUTTON_WIDTH),
                ],
            },
        }
    }
}

pub struct ChatState {
    options: ChatOptions,
    messages: VecDeque<ChatLine>,
    input: TextFieldState,
    open: bool,
    /// Vanilla `InBedChatScreen`: Enter submits without closing.
    in_bed: bool,
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
    /// The showing `SuggestionsList` (empty when hidden): its entries,
    /// `current` and `offset`.
    suggestions: Vec<ChatSuggestion>,
    suggest_index: usize,
    suggest_offset: usize,
    /// The list's `originalContents` and the range its entries replace.
    suggest_original: String,
    suggest_range: Range<usize>,
    tab_cycles: bool,
    /// Vanilla `CommandSuggestions.keepSuggestions`, set while applying one.
    keep_suggestions: bool,
    /// Whether finished completions may show the list by themselves; off
    /// on open and after a history recall, until the next edit.
    allow_suggestions: bool,
    pending_suggestions: Option<PendingSuggestions>,
    /// Vanilla `ClientSuggestionProvider.pendingSuggestionsId`.
    pending_suggestions_id: i32,
    /// Vanilla `CommandSuggestions.currentParse`, kept until the input
    /// changes.
    current_parse: Option<CommandParse>,
    /// Vanilla `commandUsage`: built on edits and when completions finish,
    /// not per frame.
    command_usage: Vec<Vec<TextSpan>>,
    /// The input byte the usage box is drawn from; `None` pins it to x = 0.
    command_usage_start: Option<usize>,
    /// Drained once per frame by the game loop and sent as
    /// `ServerboundCommandSuggestion`.
    outgoing_request: Option<(u32, String)>,
    /// Physical rectangles of styled chat spans as last drawn, for hover and
    /// click lookup (vanilla's active-text collector).
    hit_regions: Vec<StyleHitRegion>,
    /// Physical rectangles of the visible command-suggestion rows.
    suggestion_regions: Vec<(usize, [f32; 4])>,
    queue_region: Option<[f32; 4]>,
    last_suggestion_cursor: Option<(f32, f32)>,
    modal: Option<ChatModal>,
    /// The modal's button rectangles as last drawn; a click only counts once
    /// they exist.
    modal_buttons: Vec<[f32; 4]>,
    delayed_deletions: Vec<([u8; 256], Instant)>,
    delayed_messages: VecDeque<PendingChatLine>,
    /// Last-seen updates in the order they happened, for the network loop.
    chat_marks: Vec<ChatMark>,
    previous_message_time: Option<Instant>,
    latest_draft: Option<String>,
    is_restored_draft: bool,
    /// Input, draft flag, and bed-screen mode of a chat screen that Chat
    /// Settings replaced, re-opened when settings close (vanilla re-inits
    /// the parent screen).
    settings_parent: Option<(String, bool, bool)>,
}

impl ChatState {
    /// Each received line's plain text, oldest first.
    #[cfg(feature = "test-harness")]
    pub fn message_texts(&self) -> impl Iterator<Item = String> + '_ {
        self.messages
            .iter()
            .map(|line| line.spans.iter().map(|span| span.text.as_str()).collect())
    }

    pub fn new() -> Self {
        Self {
            options: ChatOptions::default(),
            messages: VecDeque::new(),
            input: TextFieldState::new(MAX_MESSAGE_LEN),
            open: false,
            in_bed: false,
            sent_history: VecDeque::new(),
            history_pos: 0,
            history_buffer: String::new(),
            scroll_pos: 0,
            new_message_since_scroll: false,
            suggestions: Vec::new(),
            suggest_index: 0,
            suggest_offset: 0,
            suggest_original: String::new(),
            suggest_range: 0..0,
            tab_cycles: false,
            keep_suggestions: false,
            allow_suggestions: false,
            pending_suggestions: None,
            pending_suggestions_id: -1,
            current_parse: None,
            command_usage: Vec::new(),
            command_usage_start: None,
            outgoing_request: None,
            hit_regions: Vec::new(),
            suggestion_regions: Vec::new(),
            queue_region: None,
            last_suggestion_cursor: None,
            modal: None,
            modal_buttons: Vec::new(),
            delayed_deletions: Vec::new(),
            delayed_messages: VecDeque::new(),
            chat_marks: Vec::new(),
            previous_message_time: None,
            latest_draft: None,
            is_restored_draft: false,
            settings_parent: None,
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
    }

    pub fn has_pending_modal_prompt(&self) -> bool {
        self.modal.is_some()
    }

    /// Vanilla `ChatComponent.isChatFocused`: ChatScreen is the current
    /// screen, which a modal replaces.
    pub fn is_focused(&self) -> bool {
        self.open && self.modal.is_none()
    }

    pub fn only_secure(&self) -> bool {
        self.options.only_secure
    }

    // TODO: vanilla times deletions on `Hud.tickCount` and ChatListener.tick
    // holds the delay queue (shifting `previousMessageTime`) while the game is
    // paused. Pomme has no paused tick clock, so both run on wall time here.

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_in_bed(&self) -> bool {
        self.open && self.in_bed
    }

    /// Vanilla `ChatComponent.openScreen`: restores the saved draft when the
    /// method allows it, else starts from the method's prefix.
    pub fn open(&mut self, method: ChatMethod, tree: Option<&CommandTree>) {
        let draft = self.latest_draft.clone().filter(|draft| match method {
            ChatMethod::Message => true,
            // A draft's method is fixed by its `/` when it was saved.
            ChatMethod::Command => draft.starts_with('/'),
        });
        match draft {
            Some(draft) => self.open_with(&draft, true, tree),
            None => {
                let prefix = match method {
                    ChatMethod::Message => "",
                    ChatMethod::Command => "/",
                };
                self.open_with(prefix, false, tree);
            }
        }
    }

    /// Vanilla `ChatScreen.init`. The initial `setValue` runs before the
    /// responder is attached, so it doesn't count as an edit.
    fn open_with(&mut self, initial: &str, is_draft: bool, tree: Option<&CommandTree>) {
        self.open = true;
        self.in_bed = false;
        self.is_restored_draft = is_draft;
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
        self.input.set_value(initial, f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.allow_suggestions = false;
        self.update_command_info(tree);
    }

    /// Opens `InBedChatScreen` the way `ChatComponent.openScreen` does, so a
    /// saved draft comes back.
    pub fn open_in_bed(&mut self, tree: Option<&CommandTree>) {
        self.open(ChatMethod::Message, tree);
        self.in_bed = true;
    }

    /// Vanilla `InBedChatScreen.onPlayerWokeUp`. Returns true when the screen
    /// closes and the mouse should be recaptured.
    pub fn on_player_woke_up(&mut self, tree: Option<&CommandTree>) -> bool {
        if !self.is_in_bed() {
            return false;
        }

        if self.is_restored_draft || self.input.value().is_empty() {
            self.close(ChatExitReason::Interrupted);
            true
        } else {
            // Vanilla replaces the bed screen with `ChatScreen(text, false)`.
            let text = self.input.value().to_owned();
            self.close(ChatExitReason::Done);
            self.open_with(&text, false, tree);
            false
        }
    }

    /// Vanilla `ChatScreen.removed`: what happens to the draft depends on why
    /// the screen went away.
    pub fn close(&mut self, reason: ChatExitReason) {
        if self.open {
            let value = self.input.value();
            let discard = match reason {
                ChatExitReason::Interrupted => false,
                ChatExitReason::Intentional => !self.options.save_drafts,
                ChatExitReason::Done => true,
            };
            if discard || java_is_blank(value) {
                self.latest_draft = None;
            } else if !self.is_restored_draft {
                self.latest_draft = Some(value.to_owned());
            }
        }
        self.settings_parent = None;
        self.finish_close();
    }

    /// Chat Settings replace chat with the current screen as their parent.
    pub fn close_for_settings(&mut self) {
        let parent = (
            self.input.value().to_owned(),
            self.is_restored_draft,
            self.in_bed,
        );
        self.close(ChatExitReason::Interrupted);
        self.settings_parent = Some(parent);
    }

    /// Re-open the chat that Chat Settings replaced, preserving whether that
    /// parent was the sleeping chat screen.
    pub fn return_from_settings(&mut self, tree: Option<&CommandTree>) -> bool {
        let Some((initial, is_draft, in_bed)) = self.settings_parent.take() else {
            return false;
        };
        self.open_with(&initial, is_draft, tree);
        self.in_bed = in_bed;
        true
    }

    fn finish_close(&mut self) {
        self.open = false;
        self.in_bed = false;
        self.is_restored_draft = false;
        self.input.set_focused(false);
        self.clear_suggestions();
        self.close_modal();
        self.reset_chat_scroll();
    }

    /// Vanilla key priority while ChatScreen is open: a child/overlay consumes
    /// Escape before the screen itself closes.
    pub fn handle_escape(&mut self) -> ChatEscape {
        // ConfirmScreen answers Escape with `callback.accept(false)`.
        if self.modal.is_some() {
            self.close_modal();
            return ChatEscape::Handled;
        }
        if !self.open {
            return ChatEscape::Handled;
        }
        if !self.suggestions.is_empty() {
            self.hide_suggestions();
            return ChatEscape::Handled;
        }
        if self.in_bed {
            return ChatEscape::WakeUp;
        }
        self.close(ChatExitReason::Intentional);
        ChatEscape::Closed
    }

    /// Vanilla `SuggestionsList.mouseScrolled` gets first refusal when the
    /// pointer is over the completion rectangle; otherwise ChatScreen scrolls
    /// the message backlog by one line with Shift or seven lines normally.
    pub fn handle_scroll(&mut self, cursor: (f32, f32), delta: f32, shift: bool) {
        if !self.is_focused() || delta == 0.0 {
            return;
        }
        let step = delta.clamp(-1.0, 1.0);
        if !self.suggestions.is_empty() && self.suggestion_at(cursor).is_some() {
            let (_, max_offset) = self.suggestion_window();
            let next = (self.suggest_offset as f32 - step).trunc() as isize;
            self.suggest_offset = next.clamp(0, max_offset as isize) as usize;
            return;
        }
        let multiplier = if shift { 1.0 } else { 7.0 };
        self.scroll_chat((step * multiplier) as i32);
    }

    /// Vanilla `EditBox.setValue` (or `insertText`) followed by its responder,
    /// `ChatScreen.onEdited`: every programmatic value change goes through
    /// here.
    fn set_input_value(
        &mut self,
        value: &str,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
        tree: Option<&CommandTree>,
    ) {
        self.input.set_value(value, inner_w, width_fn);
        self.on_edited(tree);
    }

    fn on_edited(&mut self, tree: Option<&CommandTree>) {
        self.set_allow_suggestions(true);
        self.update_command_info(tree);
        self.is_restored_draft = false;
    }

    /// Vanilla `CommandSuggestions.hasAllowedInput`: what Enter may send under
    /// the chat visibility's restrictions.
    fn has_allowed_input(&self) -> bool {
        let value = self.input.value();
        let (is_command, is_message) = if value.starts_with('/') {
            (
                true,
                self.current_parse
                    .as_ref()
                    .is_some_and(CommandParse::is_message),
            )
        } else {
            (false, !java_is_blank(value))
        };
        !(is_message && !self.messages_allowed()) && (!is_command || self.commands_allowed())
    }

    /// Vanilla `ChatAbilities.canSendMessages`.
    fn messages_allowed(&self) -> bool {
        self.options.visibility == ChatVisibilitySetting::Full
    }

    /// Vanilla `ChatAbilities.canSendCommands`.
    fn commands_allowed(&self) -> bool {
        self.options.visibility != ChatVisibilitySetting::Hidden
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

        // Up/Down cycle the suggestion list when it's showing, else recall
        // sent-message history (vanilla CommandSuggestions gets keys first).
        if up || down {
            let direction = if up { -1 } else { 1 };
            if !self.suggestions.is_empty() {
                self.cycle_suggestion(direction);
                self.tab_cycles = false;
            } else {
                self.move_in_history(direction as i32, inner_w, width_fn, tree);
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
                self.set_input_value("", inner_w, width_fn, tree);
                continue;
            }
            let before = self.input.value().to_owned();
            self.input.handle(ev, &mut clipboard, inner_w, width_fn);
            if self.input.value() != before {
                self.on_edited(tree);
            }
        }

        // Tab uses the highlighted entry of a showing list, cycling first
        // once a Tab has applied one; without a list it only shows it.
        if tab {
            if self.suggestions.is_empty() {
                self.show_suggestions();
            } else {
                if self.tab_cycles {
                    self.cycle_suggestion(if shift { -1 } else { 1 });
                }
                self.use_suggestion(inner_w, width_fn, tree);
            }
            return None;
        }

        if enter {
            if !self.has_allowed_input() {
                return None;
            }
            let normalized = normalize_chat_message(self.input.value());
            let message = (!normalized.is_empty()).then(|| {
                self.add_recent_chat(&normalized);
                normalized
            });
            if self.in_bed {
                // `closeOnSubmit=false`: `setValue("")`, then `resetChatScroll`.
                self.set_input_value("", inner_w, width_fn, tree);
                self.reset_chat_scroll();
            } else {
                self.close(ChatExitReason::Done);
            }
            return message;
        }

        None
    }

    fn style_at(&self, cursor: (f32, f32)) -> Option<Arc<ResolvedStyle>> {
        style_at(&self.hit_regions, cursor)
    }

    /// Whether the pointing-hand cursor applies: a hovered button
    /// (`AbstractWidget`), click-event text, or insertion text while Shift
    /// is held (`DrawingFocusedGraphicsAccess.changeCursorOnInsertions`).
    pub fn hovering_clickable(&self, cursor: (f32, f32), shift: bool) -> bool {
        if !self.open {
            return false;
        }
        if self.modal.is_some() {
            return self
                .modal_buttons
                .iter()
                .any(|rect| common::hit_test(cursor, *rect));
        }
        if self
            .queue_region
            .is_some_and(|rect| common::hit_test(cursor, rect))
            || self.suggestion_at(cursor).is_some()
        {
            return true;
        }
        self.style_at(cursor).is_some_and(|style| {
            style.click_event.is_some() || (shift && style.insertion.is_some())
        })
    }

    fn handle_click(
        &mut self,
        cursor: (f32, f32),
        shift: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
        tree: Option<&CommandTree>,
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

        if let Some(index) = self.suggestion_at(cursor) {
            self.select_suggestion(index as isize);
            self.use_suggestion(inner_w, width_fn, tree);
            return None;
        }

        let style = self.style_at(cursor)?;
        if shift {
            if let Some(insertion) = &style.insertion {
                self.input.insert_text(insertion, inner_w, width_fn);
                self.on_edited(tree);
            }
            return None;
        }

        match style.click_event.as_ref()? {
            ClickEvent::OpenUrl(url) => self.request_open_url(url.clone()),
            ClickEvent::RunCommand(command) => Some(ChatUiAction::RunCommand(command.clone())),
            ClickEvent::SuggestCommand(command) => {
                self.set_input_value(command, inner_w, width_fn, tree);
                None
            }
            ClickEvent::CopyToClipboard(value) => {
                common::set_clipboard(value);
                None
            }
            ClickEvent::ShowDialog(dialog) => Some(ChatUiAction::ShowDialog(dialog.clone())),
            ClickEvent::Custom { id, payload } => match normalize_identifier(id).as_str() {
                QUEUE_EXPAND_ID => {
                    self.accept_next_delayed_message();
                    None
                }
                // TODO: vanilla opens RestrictionsScreen (ChatScreen.java:250).
                GO_TO_RESTRICTIONS_SCREEN => Some(ChatUiAction::OpenChatSettings),
                _ => Some(ChatUiAction::Custom {
                    id: id.clone(),
                    payload: payload.clone(),
                }),
            },
            ClickEvent::ChangePage(_) => None,
        }
    }
}

/// Vanilla `ChatComponent.QUEUE_EXPAND_ID`.
const QUEUE_EXPAND_ID: &str = "minecraft:internal/expand_chat_queue";
/// Vanilla `ChatComponent.GO_TO_RESTRICTIONS_SCREEN`.
const GO_TO_RESTRICTIONS_SCREEN: &str = "minecraft:internal/go_to_restrictions_screen";

/// `Button.DEFAULT_WIDTH`.
const CONFIRM_BUTTON_WIDTH: i32 = 150;
/// `ConfirmLinkScreen.BUTTON_WIDTH`.
const CONFIRM_LINK_BUTTON_WIDTH: i32 = 100;
/// Vanilla `GuiGraphicsExtractor.componentHoverEffect`. `width0` measures
/// styled runs in gui units, as for chat wrapping.
#[allow(clippy::too_many_arguments)]
fn push_hover_tooltip(
    elements: &mut Vec<MenuElement>,
    hover: &HoverEvent,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    advanced_item_tooltips: bool,
    bindings: &KeyBindings,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) {
    let lines = match hover {
        HoverEvent::Text(component) => {
            let max_width = ((screen_w / gs).ceil() / 2.0).floor().max(200.0);
            wrapped_tooltip_lines_with_bindings(component, max_width, width0, bindings)
        }
        // TODO: a `show_item` hover keeps only its JSON shape
        // (`HoverEvent::Item`), so its components lose payload types and Java
        // number text; the dialog's item body passes its raw tag.
        HoverEvent::Item(value) => {
            item_tooltip_lines_with_bindings(value, None, None, advanced_item_tooltips, bindings)
        }
        HoverEvent::Entity(value) if advanced_item_tooltips => {
            entity_tooltip_lines(value, bindings)
        }
        HoverEvent::Entity(_) => Vec::new(),
    };
    if !lines.is_empty() {
        common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
    }
}

/// Vanilla `DrawingFocusedGraphicsAccess.showTooltip`.
fn push_tag_tooltip(
    elements: &mut Vec<MenuElement>,
    tag: &ChatMessageTag,
    cursor: (f32, f32),
    screen_w: f32,
    screen_h: f32,
    gs: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) {
    let lines = wrapped_tooltip_lines(&tag.tooltip_component(), TAG_TOOLTIP_MAX_WIDTH, width0);
    common::push_tooltip_lines(elements, cursor, screen_w, screen_h, gs, lines);
}

/// Vanilla `Font.split(component, max_width)` as tooltip lines, `max_width`
/// in gui units.
pub(crate) fn wrapped_tooltip_lines(
    component: &Component,
    max_width: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<TooltipLine> {
    wrapped_tooltip_lines_with_bindings(component, max_width, width0, &KeyBindings::default())
}

fn wrapped_tooltip_lines_with_bindings(
    component: &Component,
    max_width: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
    bindings: &KeyBindings,
) -> Vec<TooltipLine> {
    let spans = format_component_spans_with_bindings(component, common::WHITE, bindings);
    wrap_spans(&spans, max_width, width0)
        .into_iter()
        .map(|spans| TooltipLine {
            spans,
            right_align: false,
        })
        .collect()
}

pub(crate) fn component_tooltip_lines(component: &Component) -> Vec<TooltipLine> {
    component_tooltip_lines_with_bindings(component, &KeyBindings::default())
}

fn component_tooltip_lines_with_bindings(
    component: &Component,
    bindings: &KeyBindings,
) -> Vec<TooltipLine> {
    span_tooltip_lines(format_component_spans_with_bindings(
        component,
        common::WHITE,
        bindings,
    ))
}

/// One tooltip line per `\n`-separated run, unwrapped.
fn span_tooltip_lines(spans: Vec<TextSpan>) -> Vec<TooltipLine> {
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
                lines
                    .last_mut()
                    .unwrap()
                    .spans
                    .push(span.with_text(part.to_owned()));
            }
        }
    }
    if lines.last().is_some_and(|line| line.spans.is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    lines
}

fn entity_tooltip_lines(value: &serde_json::Value, bindings: &KeyBindings) -> Vec<TooltipLine> {
    let Some(map) = value.as_object() else {
        return vec![TooltipLine::new(value.to_string(), common::WHITE)];
    };
    let mut lines = Vec::new();
    if let Some(name) = map.get("name")
        && let Ok(component) = Component::from_value(name)
    {
        lines.extend(component_tooltip_lines_with_bindings(&component, bindings));
    }

    if let Some(id) = map.get("id").and_then(serde_json::Value::as_str) {
        // `EntityType.getDescription`: `entity.<namespace>.<path>`.
        let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let description = Component::translate(
            format!("entity.{namespace}.{}", path.replace('/', ".")),
            Vec::new(),
        );
        lines.extend(component_tooltip_lines_with_bindings(
            &Component::translate(
                "gui.entity_tooltip.type",
                vec![Argument::Component(Box::new(description))],
            ),
            bindings,
        ));
    }
    if let Some(uuid) = map.get("uuid").and_then(parse_uuid_value) {
        lines.push(TooltipLine::new(uuid.to_string(), common::WHITE));
    }
    lines
}

/// The stack's tooltip. `raw_components` is the item's `components` tag where
/// the stack came as NBT, so its components keep the payloads and Java number
/// text the JSON shape loses.
pub(crate) fn item_tooltip_lines(
    value: &serde_json::Value,
    raw_components: Option<&NbtCompound>,
    advanced: bool,
) -> Vec<TooltipLine> {
    item_tooltip_lines_with_bindings(
        value,
        raw_components,
        None,
        advanced,
        &KeyBindings::default(),
    )
}

/// A held stack's tooltip, vanilla `ItemStack.getTooltipLines`: what a
/// container slot and the creative inventory show.
pub(crate) fn stack_tooltip_lines(
    data: &azalea_inventory::ItemStackData,
    registries: &azalea_core::registry_holder::RegistryHolder,
    advanced: bool,
) -> Vec<TooltipLine> {
    item_tooltip_lines_with_bindings(
        &stack_json(data, registries),
        None,
        Some(super::creative_inventory::total_component_count(data)),
        advanced,
        &KeyBindings::default(),
    )
}

/// The stack in `show_item`'s JSON shape, for [`item_tooltip_lines`].
/// azalea's serde writes an enchantment as an enum variant, which can't key a
/// JSON map, so enchantments go in by their registry names.
// TODO: pomme's item grammar has no JSON output yet, so the rest still goes
// through azalea's serde shape.
fn stack_json(
    data: &azalea_inventory::ItemStackData,
    registries: &azalea_core::registry_holder::RegistryHolder,
) -> serde_json::Value {
    use azalea_inventory::components::{Enchantments, StoredEnchantments};
    use azalea_registry::DataRegistry;

    let stack = azalea_inventory::ItemStack::Present(data.clone())
        .with_component::<Enchantments>(None)
        .with_component::<StoredEnchantments>(None);
    let mut value = serde_json::to_value(&stack).unwrap_or_default();
    let names = |levels: &std::collections::HashMap<azalea_registry::data::Enchantment, i32>| {
        levels
            .iter()
            .filter_map(|(enchantment, level)| {
                let (id, _) = registries
                    .enchantment
                    .map
                    .get_index(enchantment.protocol_id() as usize)?;
                Some((id.to_string(), serde_json::Value::from(*level)))
            })
            .collect::<serde_json::Map<_, _>>()
    };
    let enchantments = [
        (
            "minecraft:enchantments",
            data.get_component::<Enchantments>()
                .map(|e| names(&e.levels)),
        ),
        (
            "minecraft:stored_enchantments",
            data.get_component::<StoredEnchantments>()
                .map(|e| names(&e.enchantments)),
        ),
    ];
    if let Some(map) = value.as_object_mut() {
        let components = map
            .entry("components")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(components) = components.as_object_mut() {
            for (key, levels) in enchantments {
                if let Some(levels) = levels {
                    components.insert(key.to_owned(), serde_json::Value::Object(levels));
                }
            }
        }
    }
    value
}

/// `component_count` is the stack's components including its item's
/// defaults; a hover's JSON carries only the ones it changes.
fn item_tooltip_lines_with_bindings(
    value: &serde_json::Value,
    raw_components: Option<&NbtCompound>,
    component_count: Option<usize>,
    advanced: bool,
    bindings: &KeyBindings,
) -> Vec<TooltipLine> {
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
    let rarity_rgb = item_rarity_rgb(rarity);

    let path = id.split(':').next_back().unwrap_or(id);
    let item_key = format!("item.minecraft.{path}");
    let block_key = format!("block.minecraft.{path}");
    // A `profile` is a bare name, or `{name, id, properties}`.
    let profile = component_value(components, "profile");
    let profile_name = profile.and_then(|profile| {
        profile
            .as_str()
            .or_else(|| profile.get("name").and_then(serde_json::Value::as_str))
    });
    let default_name = crate::lang::translate(&item_key)
        .or_else(|| crate::lang::translate(&block_key))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::lang::title_case_snake(path));
    // `PlayerHeadItem.getName`: the head of the profile's name.
    let default_name = match profile_name {
        Some(name) if path == "player_head" => {
            crate::lang::translate("block.minecraft.player_head.named").map_or_else(
                || format!("{name}'s Head"),
                |text| text.replacen("%s", name, 1),
            )
        }
        _ => default_name,
    };

    let custom_name = item_component(components, raw_components, "custom_name");
    let item_name = item_component(components, raw_components, "item_name");
    let mut lines = if let Some(name) = custom_name.as_ref().or(item_name.as_ref()) {
        // `ItemStack.getStyledHoverName` wraps the name in a parent carrying
        // the rarity color (and italic for a custom name), so the name's own
        // explicit style wins.
        let parent = ResolvedStyle {
            color: Some(rarity_rgb),
            italic: custom_name.is_some(),
            ..ResolvedStyle::default()
        };
        span_tooltip_lines(format_component_spans_with_parent_and_bindings(
            name,
            &parent,
            common::WHITE,
            bindings,
        ))
    } else {
        vec![TooltipLine::new(default_name, common::rgb(rarity_rgb))]
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

    // `ResolvableProfile.Dynamic.addToTooltip`: a profile with a name or an
    // id, not both, and no properties resolves when shown.
    if tooltip_component_visible(tooltip_display, "profile")
        && let Some(profile) = profile
    {
        let has = |field| profile.get(field).is_some();
        let dynamic = profile.is_string()
            || (has("name") != has("id")
                && profile
                    .get("properties")
                    .is_none_or(|properties| properties.as_array().is_some_and(Vec::is_empty)));
        if dynamic {
            lines.push(TooltipLine::new(
                crate::lang::translate("component.profile.dynamic")
                    .unwrap_or("Dynamic")
                    .to_owned(),
                common::rgb(0xAAAAAA),
            ));
        }
    }

    if tooltip_component_visible(tooltip_display, "lore") {
        let lore: Vec<Component> = match raw_component(raw_components, "lore") {
            Some(NbtTag::List(lore)) => list_elements(lore)
                .iter()
                .filter_map(|line| Component::from_nbt_tag(line).ok())
                .collect(),
            _ => component_value(components, "lore")
                .and_then(serde_json::Value::as_array)
                .map(|lore| {
                    lore.iter()
                        .filter_map(|line| Component::from_value(line).ok())
                        .collect()
                })
                .unwrap_or_default(),
        };
        for line in &lore {
            lines.extend(component_tooltip_lines_with_bindings(line, bindings));
        }
    }

    if tooltip_component_visible(tooltip_display, "attribute_modifiers") {
        let defaults;
        let modifiers = match component_value(components, "attribute_modifiers") {
            Some(modifiers) => modifiers,
            None => {
                defaults = kind
                    .and_then(get_default_component::<AttributeModifiers>)
                    .and_then(|modifiers| serde_json::to_value(modifiers).ok())
                    .unwrap_or_default();
                &defaults
            }
        };
        if let Some(modifiers) = modifiers.as_array() {
            lines.extend(attribute_tooltip_lines(modifiers, bindings));
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
                lines.extend(component_tooltip_lines_with_bindings(&component, bindings));
            } else {
                lines.push(TooltipLine::new(
                    format!("Durability: {remaining} / {max_damage}"),
                    common::WHITE,
                ));
            }
        }
        lines.push(TooltipLine::new(id.to_owned(), common::rgb(0x555555)));
        let component_count =
            component_count.unwrap_or_else(|| components.map_or(0, serde_json::Map::len));
        if component_count > 0 {
            if crate::lang::translate("item.components").is_some() {
                let component = Component::translate(
                    "item.components",
                    vec![Argument::Number(component_count.to_string())],
                );
                lines.extend(colored_tooltip_lines(&component, 0x555555, bindings));
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

/// `ItemStack.addAttributeTooltips` over an `attribute_modifiers` list, each
/// slot group's modifiers under its header, as the local player reads them
/// (`ItemAttributeModifiers.Display.Default`).
// TODO: the modifiers enchantments add (`EnchantmentHelper.forEachModifier`),
// which come from the enchantment registry's effects; and the player's own
// base attack damage and speed, where a server changed them.
fn attribute_tooltip_lines(
    modifiers: &[serde_json::Value],
    bindings: &KeyBindings,
) -> Vec<TooltipLine> {
    use crate::attribute::AttributeKind;

    /// `EquipmentSlotGroup`, in order.
    const SLOT_GROUPS: [&str; 11] = [
        "any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body",
        "saddle",
    ];
    let colored = |component: &Component, rgb| colored_tooltip_lines(component, rgb, bindings);
    let path = |value: Option<&serde_json::Value>| {
        let id = value
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        id.split(':').next_back().unwrap_or(id).to_owned()
    };
    let mut lines = Vec::new();
    for group in SLOT_GROUPS {
        let mut first = true;
        for entry in modifiers {
            if entry
                .get("slot")
                .and_then(serde_json::Value::as_str)
                .map_or("any", |slot| slot)
                != group
            {
                continue;
            }
            // `display` is the codec's `{type, value}`, azalea's
            // `{"override": {text}}`, or a bare type name.
            let (display_type, override_text) = match entry.get("display") {
                Some(serde_json::Value::String(kind)) => (kind.as_str(), None),
                Some(serde_json::Value::Object(display)) => {
                    match display.get("type").and_then(serde_json::Value::as_str) {
                        Some(kind) => (kind, display.get("value")),
                        None => display
                            .iter()
                            .next()
                            .map_or(("default", None), |(kind, value)| {
                                (kind.as_str(), value.get("text"))
                            }),
                    }
                }
                _ => ("default", None),
            };
            if display_type == "hidden" {
                continue;
            }
            if first {
                lines.push(TooltipLine::new(String::new(), common::WHITE));
                lines.extend(colored(
                    &Component::translate(format!("item.modifiers.{group}"), Vec::new()),
                    0xAAAAAA,
                ));
                first = false;
            }
            if display_type == "override" {
                if let Some(text) =
                    override_text.and_then(|value| Component::from_value(value).ok())
                {
                    lines.extend(component_tooltip_lines_with_bindings(&text, bindings));
                }
                continue;
            }
            let attribute = path(entry.get("type"));
            let modifier_id = path(entry.get("id"));
            let operation = match entry.get("operation").and_then(serde_json::Value::as_str) {
                Some("add_multiplied_base") => 1,
                Some("add_multiplied_total") => 2,
                _ => 0,
            };
            let mut amount = entry
                .get("amount")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let base = match modifier_id.as_str() {
                "base_attack_damage" => AttributeKind::AttackDamage.player_base_value(),
                "base_attack_speed" => AttributeKind::AttackSpeed.player_base_value(),
                _ => None,
            };
            if let Some(base) = base {
                amount += base;
            }
            let display_amount = if operation != 0 {
                amount * 100.0
            } else if attribute == "knockback_resistance" {
                amount * 10.0
            } else {
                amount
            };
            let name = Argument::Component(Box::new(Component::translate(
                format!("attribute.name.{attribute}"),
                Vec::new(),
            )));
            let line = |kind: &str, value: f64| {
                Component::translate(
                    format!("attribute.modifier.{kind}.{operation}"),
                    vec![Argument::String(modifier_amount(value)), name.clone()],
                )
            };
            // `Attribute.Sentiment.getStyle`: the colours of an increase and
            // a decrease.
            let (increase, decrease) = match attribute.as_str() {
                "burning_time" | "fall_damage_multiplier" => (0xFF5555, 0x5555FF),
                "gravity" | "scale" | "waypoint_transmit_range" | "waypoint_receive_range" => {
                    (0xAAAAAA, 0xAAAAAA)
                }
                _ => (0x5555FF, 0xFF5555),
            };
            if base.is_some() {
                let mut spaced = colored(&line("equals", display_amount), 0x00AA00);
                if let Some(first) = spaced.first_mut().and_then(|line| line.spans.first_mut()) {
                    first.text.insert(0, ' ');
                }
                lines.extend(spaced);
            } else if amount > 0.0 {
                lines.extend(colored(&line("plus", display_amount), increase));
            } else if amount < 0.0 {
                lines.extend(colored(&line("take", -display_amount), decrease));
            }
        }
    }
    lines
}

/// `component`'s lines, all in one colour.
fn colored_tooltip_lines(
    component: &Component,
    rgb: u32,
    bindings: &KeyBindings,
) -> Vec<TooltipLine> {
    let mut lines = component_tooltip_lines_with_bindings(component, bindings);
    for span in lines.iter_mut().flat_map(|line| &mut line.spans) {
        span.color = common::rgb(rgb);
    }
    lines
}

/// `ItemAttributeModifiers.ATTRIBUTE_MODIFIER_FORMAT`: at most two decimals,
/// without trailing zeros.
fn modifier_amount(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// A component of the stack that is itself a text component, read from the
/// raw tag where there is one.
fn item_component(
    components: Option<&serde_json::Map<String, serde_json::Value>>,
    raw_components: Option<&NbtCompound>,
    name: &str,
) -> Option<Component> {
    if let Some(tag) = raw_component(raw_components, name) {
        return Component::from_nbt_tag(tag).ok();
    }
    component_value(components, name).and_then(|value| Component::from_value(value).ok())
}

/// A component of the raw `components` tag, under either spelling of its id.
fn raw_component<'a>(raw_components: Option<&'a NbtCompound>, name: &str) -> Option<&'a NbtTag> {
    raw_components.and_then(|components| {
        components
            .get(name)
            .or_else(|| components.get(&format!("minecraft:{name}")))
    })
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

fn item_rarity_rgb(rarity: Rarity) -> u32 {
    match rarity {
        Rarity::Common => 0xffffff,
        Rarity::Uncommon => 0xffff55,
        Rarity::Rare => 0x55ffff,
        Rarity::Epic => 0xff55ff,
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
        name
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

#[cfg(test)]
mod tests;
