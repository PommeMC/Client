use std::cell::OnceCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use azalea_inventory::components::{MaxDamage, Rarity};
use azalea_inventory::default_components::get_default_component;
use azalea_registry::builtin::ItemKind;
use simdnbt::owned::{NbtCompound, NbtTag};

use super::common;
use crate::chat_component::{
    Argument, ClickEvent, Component, HoverEvent, ResolvedStyle, normalize_identifier,
};
use crate::net::commands::{
    CommandParse, CommandTokenKind, CommandTokenRange, CommandTree, SyntaxError,
    UnattendedCommandCheck, UsageLine,
};
use crate::net::sender::ChatMark;
use crate::renderer::pipelines::menu_overlay::{MenuElement, SpriteId, TooltipLine};
use crate::ui::text::{
    TextSpan, format_component_spans, format_component_spans_with_parent, parse_uuid_value,
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

/// Vanilla blends its black GUI fills in a gamma-space framebuffer; Pomme's
/// sRGB target blends in linear space, where the same alpha looks lighter.
/// Converts to the equivalent linear alpha (the vignette's 2.2 approximation).
fn vanilla_black_fill(alpha: f32) -> [f32; 4] {
    let alpha = alpha.clamp(0.0, 1.0);
    [0.0, 0.0, 0.0, 1.0 - (1.0 - alpha).powf(2.2)]
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

pub(crate) struct StyleHitRegion {
    rect: [f32; 4],
    style: Arc<ResolvedStyle>,
}

/// Records a hit region for each styled span of a line drawn from `x`.
pub(crate) fn push_hit_regions(
    regions: &mut Vec<StyleHitRegion>,
    spans: &[TextSpan],
    mut x: f32,
    y: f32,
    h: f32,
    span_w: &dyn Fn(&TextSpan) -> f32,
) {
    for span in spans {
        let w = span_w(span);
        if let Some(style) = &span.component_style
            && w > 0.0
        {
            regions.push(StyleHitRegion {
                rect: [x, y, w, h],
                style: style.clone(),
            });
        }
        x += w;
    }
}

pub(crate) fn style_at(
    regions: &[StyleHitRegion],
    cursor: (f32, f32),
) -> Option<Arc<ResolvedStyle>> {
    regions
        .iter()
        .find(|region| common::hit_test(cursor, region.rect))
        .map(|region| region.style.clone())
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
    /// Input and draft flag of a chat screen that Chat Settings replaced,
    /// re-opened when settings close (vanilla re-inits the parent screen).
    settings_parent: Option<(String, bool)>,
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

    pub fn request_command_confirmation(&mut self, command: String, kind: CommandConfirmationKind) {
        self.open_modal(ChatModal::Command(PendingCommand { command, kind }));
    }

    fn open_modal(&mut self, modal: ChatModal) {
        self.modal = Some(modal);
        self.modal_buttons.clear();
        // Swapping the screen runs `ChatScreen.removed`.
        self.reset_chat_scroll();
    }

    fn close_modal(&mut self) {
        self.modal = None;
        self.modal_buttons.clear();
    }

    /// Vanilla `ChatComponent.resetChatScroll`.
    pub fn reset_chat_scroll(&mut self) {
        self.scroll_pos = 0;
        self.new_message_since_scroll = false;
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
        // Vanilla `handlePlayerChatMessage` reads Only Show Secure Chat when
        // the packet arrives, so toggling it later doesn't hide shown lines.
        let suppress_display = self.options.only_secure
            && source == ChatMessageSource::Player
            && matches!(tag, Some(ChatMessageTag::NotSecure));
        self.enqueue_or_accept(PendingChatLine {
            spans,
            signature,
            ack_signature: signature,
            force_hidden_ack: false,
            suppress_display,
            source,
            tag,
        });
    }

    pub fn push_validation_error(
        &mut self,
        spans: Vec<TextSpan>,
        invalid_signature: Option<[u8; 256]>,
    ) {
        self.enqueue_or_accept(PendingChatLine {
            spans,
            signature: None,
            ack_signature: invalid_signature,
            force_hidden_ack: true,
            suppress_display: false,
            source: ChatMessageSource::Player,
            tag: Some(ChatMessageTag::Error),
        });
    }

    pub fn push_fully_filtered(&mut self, signature: Option<[u8; 256]>) {
        self.enqueue_or_accept(PendingChatLine {
            spans: Vec::new(),
            signature: None,
            ack_signature: signature,
            force_hidden_ack: true,
            suppress_display: true,
            source: ChatMessageSource::Player,
            tag: None,
        });
    }

    /// Vanilla `ChatListener.handleMessage`: player chat waits in the delay
    /// queue while the chat delay is running; system messages never do.
    fn enqueue_or_accept(&mut self, pending: PendingChatLine) {
        let now = Instant::now();
        if pending.source == ChatMessageSource::Player && self.will_delay_messages(now) {
            self.delayed_messages.push_back(pending);
        } else {
            self.accept_pending_message(pending, now);
        }
    }

    fn source_visible(&self, source: ChatMessageSource) -> bool {
        match source {
            ChatMessageSource::SystemClient => true,
            ChatMessageSource::SystemServer => {
                self.options.visibility != ChatVisibilitySetting::Hidden
            }
            ChatMessageSource::Player => self.options.visibility == ChatVisibilitySetting::Full,
        }
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
        if pending.suppress_display || !self.source_visible(pending.source) {
            self.mark_processed(pending.ack_signature, false);
            return false;
        }
        let PendingChatLine {
            spans,
            signature,
            ack_signature,
            force_hidden_ack,
            source,
            tag,
            ..
        } = pending;
        self.messages.push_back(ChatLine {
            spans,
            received: now,
            signature,
            source,
            tag,
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
        line.invalidate_wrap();
        None
    }

    // TODO: vanilla times deletions on `Hud.tickCount` and ChatListener.tick
    // holds the delay queue (shifting `previousMessageTime`) while the game is
    // paused. Pomme has no paused tick clock, so both run on wall time here.
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
        self.is_restored_draft = is_draft;
        self.history_pos = self.sent_history.len();
        self.history_buffer.clear();
        self.input.set_value(initial, f32::MAX, &|_| 0.0);
        self.input.set_focused(true);
        self.clear_suggestions();
        self.allow_suggestions = false;
        self.update_command_info(tree);
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

    /// Chat Settings opened from chat replace it, with chat as the parent
    /// screen they return to.
    pub fn close_for_settings(&mut self) {
        let parent = (self.input.value().to_owned(), self.is_restored_draft);
        self.close(ChatExitReason::Interrupted);
        self.settings_parent = Some(parent);
    }

    /// Re-open the chat that Chat Settings replaced, if any.
    pub fn return_from_settings(&mut self, tree: Option<&CommandTree>) -> bool {
        let Some((initial, is_draft)) = self.settings_parent.take() else {
            return false;
        };
        self.open_with(&initial, is_draft, tree);
        true
    }

    fn finish_close(&mut self) {
        self.open = false;
        self.is_restored_draft = false;
        self.input.set_focused(false);
        self.clear_suggestions();
        self.close_modal();
        self.reset_chat_scroll();
    }

    /// Vanilla key priority while ChatScreen is open: a child/overlay consumes
    /// Escape before the screen itself closes. Returns true only when this call
    /// actually closed chat and the game should recapture the cursor.
    pub fn handle_escape(&mut self) -> bool {
        // ConfirmScreen answers Escape with `callback.accept(false)`.
        if self.modal.is_some() {
            self.close_modal();
            return false;
        }
        if !self.open {
            return false;
        }
        if !self.suggestions.is_empty() {
            self.hide_suggestions();
            return false;
        }
        self.close(ChatExitReason::Intentional);
        true
    }

    fn lines_per_page(&self) -> usize {
        let height = self.options.height_px(self.is_focused());
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

    /// The list's visible row count and furthest scroll offset.
    fn suggestion_window(&self) -> (usize, usize) {
        let visible = self.suggestions.len().min(MAX_SUGGESTION_ROWS);
        (visible, self.suggestions.len() - visible)
    }

    /// The index of the suggestion row under `cursor`, as last drawn.
    fn suggestion_at(&self, cursor: (f32, f32)) -> Option<usize> {
        self.suggestion_regions
            .iter()
            .find(|(_, rect)| common::hit_test(cursor, *rect))
            .map(|&(index, _)| index)
    }

    fn keep_suggestion_visible(&mut self) {
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

    /// Up/Down sent-message recall, vanilla `ChatScreen.moveInHistory`.
    fn move_in_history(
        &mut self,
        delta: i32,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
        tree: Option<&CommandTree>,
    ) {
        let end = self.sent_history.len();
        let target = self
            .history_pos
            .saturating_add_signed(delta as isize)
            .min(end);
        if target == self.history_pos {
            return;
        }
        if target == end {
            self.history_pos = end;
            let draft = self.history_buffer.clone();
            self.set_input_value(&draft, inner_w, width_fn, tree);
            return;
        }
        if self.history_pos == end {
            self.history_buffer = self.input.value().to_string();
        }
        let entry = self.sent_history[target].clone();
        self.set_input_value(&entry, inner_w, width_fn, tree);
        self.set_allow_suggestions(false);
        self.history_pos = target;
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

    fn set_allow_suggestions(&mut self, allow: bool) {
        self.allow_suggestions = allow;
        if !allow {
            self.hide_suggestions();
        }
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

    /// Vanilla `CommandSuggestions.hide`.
    fn hide_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggest_index = 0;
        self.suggest_offset = 0;
        self.tab_cycles = false;
    }

    /// A fresh `CommandSuggestions`, as each `ChatScreen.init` builds.
    fn clear_suggestions(&mut self) {
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
    fn update_command_info(&mut self, tree: Option<&CommandTree>) {
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
    fn update_usage_info(&mut self, tree: Option<&CommandTree>) {
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
    fn show_suggestions(&mut self) {
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
    fn select_suggestion(&mut self, index: isize) {
        let n = self.suggestions.len() as isize;
        if n > 0 {
            self.suggest_index = index.rem_euclid(n) as usize;
        }
    }

    /// Vanilla `SuggestionsList.cycle`.
    fn cycle_suggestion(&mut self, direction: isize) {
        self.select_suggestion(self.suggest_index as isize + direction);
        self.keep_suggestion_visible();
    }

    /// Vanilla `SuggestionsList.useSuggestion`: replace the completed range of
    /// the list's original input, keeping the list open.
    fn use_suggestion(
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
            self.close(ChatExitReason::Done);
            return message;
        }

        None
    }

    /// The grey inline completion: the rest of the selected entry applied to
    /// the list's original input, past the current value. Vanilla
    /// `CommandSuggestions.calculateSuggestionSuffix`, drawn only with the
    /// caret at the end.
    fn ghost_suffix(&self) -> Option<String> {
        if !self.input.cursor_at_end() {
            return None;
        }
        let selected = self.suggestions.get(self.suggest_index)?;
        let applied =
            apply_suggestion(&self.suggest_original, &self.suggest_range, &selected.text)?;
        let suffix = applied.strip_prefix(self.input.value())?;
        (!suffix.is_empty()).then(|| suffix.to_owned())
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
            covered,
            command_tree,
            advanced_item_tooltips,
            text_width_fn,
            spans_width_fn,
        } = context;
        self.tick();
        self.hit_regions.clear();
        self.suggestion_regions.clear();
        self.queue_region = None;
        // Under a modal (or a dialog) the chat draws as Hud's unfocused
        // background layer.
        let focused = self.is_focused() && !covered;
        let chat_scale = self.options.scale.clamp(0.0, 1.0);
        // Vanilla `pose.scale(0, 0)` collapses every message, tag, queue and
        // restricted-prompt draw (and their click targets) at Chat Text
        // Size 0; the input box is drawn outside that pose.
        let hud_visible = chat_scale > 0.0;
        let chat_width = self.options.wrap_width_px();
        let render_width = self.options.render_width_px();
        let chat_fs = common::FONT_SIZE * gs * chat_scale;
        let unit = gs * chat_scale;
        let entry_height = self.options.line_height();
        let lh = entry_height * unit;
        let text_opacity = self.options.effective_text_opacity();
        let spacing = self.options.line_spacing.clamp(0.0, 1.0);
        let text_baseline_offset = (8.0 * (spacing + 1.0) - 4.0 * spacing).round();
        // Vanilla translates x by 4 inside the scaled pose, so the -4
        // background start lands on screen x=0 at every chat scale.
        let origin = CHAT_X * unit;
        let bg_w = (render_width + 12.0) * unit;
        let screen_gui_h = screen_h / gs;
        let chat_bottom = ((screen_gui_h - BOTTOM_MARGIN) / chat_scale).floor() * unit;
        let now = Instant::now();
        // Wrap in gui units (gui scale 1), measuring styled runs: vanilla
        // wraps before scaling, and bold/fonts move its wrap points.
        let width0 = |spans: &[TextSpan]| spans_width_fn(spans, common::FONT_SIZE);
        let span_w = |span: &TextSpan| spans_width_fn(std::slice::from_ref(span), chat_fs);

        let lines_per_page = self.lines_per_page();
        let total_lines: usize = self
            .messages
            .iter()
            .filter(|m| self.source_visible(m.source))
            .map(|m| m.wrapped(chat_width, self.options.colors, &width0).len())
            .sum();
        // Clamp the scroll to the wrapped backlog (vanilla scrollChat clamps
        // against `trimmedMessages`).
        if focused && self.scroll_pos > 0 {
            self.scroll_pos = self
                .scroll_pos
                .min(total_lines.saturating_sub(lines_per_page));
        }

        // Gather the visible wrapped lines newest-first; index 0 is the
        // bottom-most line, `scroll_pos` lines skipped below it. All wrapped
        // lines of a message share its alpha.
        let mut display: Vec<DisplayLine> = Vec::new();
        let mut skipped = 0usize;
        let messages = if hud_visible {
            &self.messages
        } else {
            &VecDeque::new()
        };
        'gather: for (message_id, msg) in messages.iter().rev().enumerate() {
            if !self.source_visible(msg.source) {
                continue;
            }
            let alpha = if focused {
                1.0
            } else {
                line_alpha(now.duration_since(msg.received).as_secs_f32())
            };
            if !focused && alpha <= 1e-5 {
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

        // Vanilla submits every row background before the text, so glyphs
        // reaching outside their 8px line box (accents, obfuscated) aren't
        // darkened by a neighbouring row's background.
        let mut chat_text_elements = Vec::with_capacity(display.len());
        for (i, (line_spans, alpha, tag, end_of_entry, message_id)) in display.iter().enumerate() {
            let entry_bottom = chat_bottom - (i as f32) * lh;
            let entry_top = entry_bottom - lh;
            let bg_a = alpha * self.options.text_background_opacity.clamp(0.0, 1.0);
            if bg_a > 1e-5 {
                push_fill(
                    elements,
                    [0.0, entry_top, bg_w, lh],
                    vanilla_black_fill(bg_a),
                );
            }
            let text_a = alpha * text_opacity;
            let text_top = entry_bottom - text_baseline_offset * unit;
            if let Some(tag) = tag {
                let mut indicator = tag.indicator_color();
                indicator[3] *= text_a;
                let indicator_rect = [0.0, entry_top, 2.0 * unit, lh];
                push_fill(elements, indicator_rect, indicator);
                // Only the focused (chat open) access shows tag tooltips.
                if focused && common::hit_test(cursor, indicator_rect) {
                    push_tag_tooltip(elements, tag, cursor, screen_w, screen_h, gs, &width0);
                }
            }

            push_hit_regions(
                &mut self.hit_regions,
                line_spans,
                origin,
                entry_top,
                lh,
                &span_w,
            );

            // Vanilla `handleTagIcon`: the background access draws no icon.
            if focused && let Some(tag @ ChatMessageTag::Modified { .. }) = tag {
                // The 9x9 icon's top sits one pixel above the text top; its
                // hover box runs down to the text bottom (9x10).
                let line_width = spans_width_fn(line_spans, chat_fs);
                let icon_rect = [
                    origin + line_width + 4.0 * unit,
                    text_top - unit,
                    9.0 * unit,
                    9.0 * unit,
                ];
                let hover_rect = [icon_rect[0], icon_rect[1], icon_rect[2], 10.0 * unit];
                let icon_hovered = common::hit_test(cursor, hover_rect);
                let message_hovered = *end_of_entry
                    && display.iter().enumerate().any(
                        |(line_idx, (other_spans, _, _, _, other_message_id))| {
                            if other_message_id != message_id {
                                return false;
                            }
                            let top = chat_bottom - line_idx as f32 * lh - lh;
                            let width = spans_width_fn(other_spans, chat_fs);
                            common::hit_test(cursor, [origin, top, width, lh])
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
                if icon_hovered {
                    push_tag_tooltip(elements, tag, cursor, screen_w, screen_h, gs, &width0);
                }
            }

            chat_text_elements.push(MenuElement::McText {
                x: origin,
                y: text_top,
                spans: with_alpha(line_spans, text_a),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }
        elements.extend(chat_text_elements);

        if hud_visible && focused && self.options.visibility != ChatVisibilitySetting::Full {
            let restricted_y = chat_bottom - (display.len() as f32 + 1.0) * lh;
            push_fill(
                elements,
                [2.0 * unit, restricted_y, (render_width + 10.0) * unit, lh],
                vanilla_black_fill(self.options.text_background_opacity),
            );
            let mut restricted = colored(
                Component::translate("chat_screen.restricted", Vec::new()),
                0xff5555,
            );
            restricted.style.underlined = Some(true);
            restricted.style.click_event = Some(ClickEvent::Custom {
                id: GO_TO_RESTRICTIONS_SCREEN.to_owned(),
                payload: None,
            });
            let mut restricted_spans = format_component_spans(&restricted, common::WHITE);
            // Vanilla clips an overlong prompt and adds the full text as a
            // hover tooltip (`RESTRICTED_CHAT_MESSAGE_WITH_HOVER`).
            if width0(&restricted_spans) > render_width {
                restricted.style.hover_event = Some(HoverEvent::Text(Box::new(
                    Component::translate("chat_screen.restricted", Vec::new()),
                )));
                restricted_spans = clip_spans(
                    &format_component_spans(&restricted, common::WHITE),
                    render_width,
                    &width0,
                );
            }
            push_hit_regions(
                &mut self.hit_regions,
                &restricted_spans,
                origin,
                restricted_y,
                lh,
                &span_w,
            );
            elements.push(MenuElement::McText {
                x: origin,
                y: restricted_y + (entry_height - text_baseline_offset - 1.0) * unit,
                spans: with_alpha(&restricted_spans, text_opacity),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
        }

        if hud_visible && !self.delayed_messages.is_empty() {
            let queue_count = self.delayed_messages.len();
            push_fill(
                elements,
                [
                    2.0 * unit,
                    chat_bottom,
                    (render_width + 6.0) * unit,
                    9.0 * unit,
                ],
                vanilla_black_fill(self.options.text_background_opacity),
            );
            let queue_component = Component::translate(
                "chat.queue",
                vec![Argument::Number(queue_count.to_string())],
            );
            let queue_spans = format_component_spans(&queue_component, common::WHITE);
            let text_y = chat_bottom + unit;
            // Only the queue text's glyphs carry the expand click and tooltip
            // (vanilla `QUEUE_EXPAND_TEXT_STYLE`), not the rest of the bar.
            let queue_rect = [
                origin,
                text_y,
                spans_width_fn(&queue_spans, chat_fs),
                common::FONT_SIZE * unit,
            ];
            self.queue_region = Some(queue_rect);
            elements.push(MenuElement::McText {
                x: origin,
                y: text_y,
                spans: with_alpha(&queue_spans, 0.5 * text_opacity),
                scale: chat_fs,
                centered: false,
                shadow: true,
            });
            if focused && common::hit_test(cursor, queue_rect) {
                let tooltip = Component::translate("chat.queue.tooltip", Vec::new());
                push_hover_tooltip(
                    elements,
                    &HoverEvent::Text(Box::new(tooltip)),
                    cursor,
                    screen_w,
                    screen_h,
                    gs,
                    advanced_item_tooltips,
                    &width0,
                );
            }
        }

        if focused && total_lines > lines_per_page && !display.is_empty() {
            // Vanilla sizes the bar in whole gui units with integer division,
            // so a long backlog can round its height down to nothing.
            let entry_units = entry_height as usize;
            let chat_height = display.len() * entry_units;
            let virtual_height = total_lines * entry_units;
            let offset = (self.scroll_pos * chat_height / total_lines) as f32 * unit;
            let bar_h = (chat_height * chat_height / virtual_height) as f32 * unit;
            let bar_bottom = chat_bottom - offset;
            let bar_top = bar_bottom - bar_h;
            let x = origin + (render_width + 4.0) * unit;
            let color = if self.new_message_since_scroll {
                common::rgb(0xcc3333)
            } else {
                common::rgb(0x3333aa)
            };
            let alpha = if offset > chat_bottom { 170.0 } else { 96.0 } / 255.0;
            push_fill(
                elements,
                [x, bar_top, 2.0 * unit, bar_h],
                [color[0], color[1], color[2], alpha],
            );
            push_fill(
                elements,
                [x + unit, bar_top, unit, bar_h],
                [0.8, 0.8, 0.8, alpha],
            );
        }

        if focused {
            let ui_fs = common::FONT_SIZE * gs;
            let input_h = INPUT_HEIGHT * gs;
            // Vanilla pins the input as a full-width bar at the very bottom of
            // the screen: fill(2, height-14, width-2, height-2).
            let bar_y = screen_h - 14.0 * gs;
            let text_y = bar_y + (input_h - ui_fs) / 2.0;
            push_fill(
                elements,
                [2.0 * gs, bar_y, screen_w - 4.0 * gs, input_h],
                vanilla_black_fill(INPUT_BG_ALPHA),
            );

            // ChatScreen's EditBox is independent of ChatComponent scale:
            // x=4, y=height-12, width=screenWidth-4 in GUI coordinates.
            let text_x = INPUT_X * gs;
            let inner_w = screen_w - text_x;
            let wf = |s: &str| text_width_fn(s, ui_fs);
            let info = self.input.render_info(inner_w, true, &wf);
            let shown = &self.input.value()[info.display_start..info.display_end];

            // EditBox formatters, first match wins: ChatScreen's restored
            // draft (grey italic), then CommandSuggestions' Brigadier colours;
            // ordinary messages stay unformatted.
            let all_spans = if self.is_restored_draft {
                let mut draft = TextSpan::new(self.input.value().to_owned(), common::rgb(0xaaaaaa));
                draft.italic = true;
                Some(vec![draft])
            } else {
                self.current_parse
                    .as_ref()
                    .filter(|parse| Some(parse.input()) == self.input.value().strip_prefix('/'))
                    .map(|parse| command_input_spans(self.input.value(), parse.tokens()))
            };
            let visible_spans =
                all_spans.map(|spans| slice_spans(&spans, info.display_start, info.display_end));
            let ghost = self.ghost_suffix();
            common::push_field_text(
                elements,
                &info,
                shown,
                visible_spans.as_deref(),
                text_x,
                text_y,
                ui_fs,
                gs,
                gs,
                CARET_COLOR,
                ghost.as_deref().map(|g| (g, GHOST_TEXT)),
                &wf,
            );

            let gui_w = |s: &str| text_width_fn(s, ui_fs) / gs;
            if !self.suggestions.is_empty() {
                self.push_suggestion_list(elements, cursor, screen_w, screen_h, gs, &gui_w);
            } else {
                self.push_usage(elements, screen_w, screen_h, gs, &gui_w, &|spans| {
                    spans_width_fn(spans, ui_fs) / gs
                });
            }

            if let Some(style) = self.style_at(cursor)
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
                    &width0,
                );
            }

            if clicked {
                return self.handle_click(cursor, shift, inner_w, &wf, command_tree);
            }
        }

        None
    }

    /// The showing list's rectangle in GUI units: vanilla `showSuggestions`
    /// places it at the completed range's screen x, clamped on screen, and
    /// the `SuggestionsList` constructor anchors it above the input.
    fn suggestion_rect(
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
    fn push_suggestion_list(
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
        let bg = vanilla_black_fill(SUGGEST_BG_ALPHA);
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
    fn push_usage(
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
                vanilla_black_fill(SUGGEST_BG_ALPHA),
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

/// Vanilla `ChatComponent.QUEUE_EXPAND_ID`.
const QUEUE_EXPAND_ID: &str = "minecraft:internal/expand_chat_queue";
/// Vanilla `ChatComponent.GO_TO_RESTRICTIONS_SCREEN`.
const GO_TO_RESTRICTIONS_SCREEN: &str = "minecraft:internal/go_to_restrictions_screen";

/// `Button.DEFAULT_WIDTH`.
const CONFIRM_BUTTON_WIDTH: i32 = 150;
/// `ConfirmLinkScreen.BUTTON_WIDTH`.
const CONFIRM_LINK_BUTTON_WIDTH: i32 = 100;
/// `ConfirmScreen.addMessage`'s `setMaxRows`.
const CONFIRM_MESSAGE_MAX_ROWS: usize = 15;
const LINE_HEIGHT: i32 = 9;

/// Vanilla `ConfirmScreen`: title, wrapped message, optional extra line and a
/// button row in a vertical `LinearLayout` centred on the screen.
struct ConfirmScreen {
    title: Component,
    message: Component,
    /// `ConfirmLinkScreen.addAdditionalText`'s warning.
    warning: Option<Component>,
    /// Lang key and gui-unit width of each button, left to right.
    buttons: Vec<(&'static str, i32)>,
}

/// A text line's top-left in gui units.
struct PlacedLine {
    x: i32,
    y: i32,
    spans: Vec<TextSpan>,
}

/// [`ConfirmScreen`] positions in gui units; buttons as `[x, y, w, h]`.
struct ConfirmLayout {
    lines: Vec<PlacedLine>,
    buttons: Vec<[i32; 4]>,
}

impl ConfirmScreen {
    /// `ConfirmScreen.init` + `repositionElements` on a `width` x `height`
    /// gui-unit screen. Vertical spacing 8, children centred; the button row
    /// has spacing 4 and 16 padding above each button.
    fn layout(
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
    fn push(
        &self,
        elements: &mut Vec<MenuElement>,
        cursor: (f32, f32),
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        width0: &dyn Fn(&[TextSpan]) -> f32,
    ) -> Vec<[f32; 4]> {
        // TODO: vanilla also blurs the world behind in-world screens
        // (`Screen.extractBlurredBackground`); this is only the flat
        // inworld_menu_background.png tint (black at 64/255).
        common::push_overlay(
            elements,
            screen_w,
            screen_h,
            vanilla_black_fill(64.0 / 255.0)[3],
        );
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
    width0: &dyn Fn(&[TextSpan]) -> f32,
) {
    let lines = match hover {
        HoverEvent::Text(component) => {
            let max_width = ((screen_w / gs).ceil() / 2.0).floor().max(200.0);
            wrapped_tooltip_lines(component, max_width, width0)
        }
        // TODO: a `show_item` hover keeps only its JSON shape
        // (`HoverEvent::Item`), so its components lose payload types and Java
        // number text; the dialog's item body passes its raw tag.
        HoverEvent::Item(value) => item_tooltip_lines(value, None, advanced_item_tooltips),
        HoverEvent::Entity(value) if advanced_item_tooltips => entity_tooltip_lines(value),
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
    let spans = format_component_spans(component, common::WHITE);
    wrap_spans(&spans, max_width, width0)
        .into_iter()
        .map(|spans| TooltipLine {
            spans,
            right_align: false,
        })
        .collect()
}

pub(crate) fn component_tooltip_lines(component: &Component) -> Vec<TooltipLine> {
    span_tooltip_lines(format_component_spans(component, common::WHITE))
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
        // `EntityType.getDescription`: `entity.<namespace>.<path>`.
        let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let description = Component::translate(
            format!("entity.{namespace}.{}", path.replace('/', ".")),
            Vec::new(),
        );
        lines.extend(component_tooltip_lines(&Component::translate(
            "gui.entity_tooltip.type",
            vec![Argument::Component(Box::new(description))],
        )));
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
    let default_name = crate::lang::translate(&item_key)
        .or_else(|| crate::lang::translate(&block_key))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::lang::title_case_snake(path));

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
        span_tooltip_lines(format_component_spans_with_parent(
            name,
            &parent,
            common::WHITE,
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

    if tooltip_component_visible(tooltip_display, "lore") {
        let lore: Vec<Component> = match raw_component(raw_components, "lore") {
            Some(NbtTag::List(lore)) => lore
                .as_nbt_tags()
                .into_iter()
                .filter_map(|line| Component::from_nbt_tag(&line).ok())
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
            lines.extend(component_tooltip_lines(line));
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

/// Vanilla `EditBox.getScreenX` of byte `start` of `input`, in gui units,
/// clamped so a `width`-wide box stays on screen.
fn input_screen_x(
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

/// Brigadier `Suggestion.apply`: `text` in place of `range` of `input`.
fn apply_suggestion(input: &str, range: &Range<usize>, text: &str) -> Option<String> {
    Some(format!(
        "{}{text}{}",
        input.get(..range.start)?,
        input.get(range.end..)?
    ))
}

/// Vanilla `CommandSuggestions.getLastWordIndex`: just past the last `\s+`
/// run.
fn last_word_index(text: &str) -> usize {
    text.rfind([' ', '\t', '\n', '\u{b}', '\u{c}', '\r'])
        .map_or(0, |i| i + 1)
}

/// ChatScreen's red commands/messages-not-allowed usage line.
fn restricted_line(key: &str) -> Vec<TextSpan> {
    format_component_spans(
        &colored(Component::translate(key, Vec::new()), 0xff5555),
        common::WHITE,
    )
}

/// Vanilla `CommandSuggestions.getExceptionMessage` for a parse of the
/// command in `input` (which keeps its `/`).
fn syntax_error_component(input: &str, error: &SyntaxError) -> Component {
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
fn parse_error_component(input: &str, cursor: usize, message: Component) -> Component {
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

/// Java `Character.isWhitespace`: Unicode separators except the no-break
/// spaces, plus the ASCII controls Java counts.
fn java_is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{1680}'
            | '\u{2000}'..='\u{2006}'
            | '\u{2008}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Java `String.isBlank` / commons-lang3 `StringUtils.isBlank`.
fn java_is_blank(s: &str) -> bool {
    s.chars().all(java_is_whitespace)
}

/// Java `String.trim`: strips everything up to U+0020 from both ends.
fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
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

/// Vanilla `ChatScreen.normalizeChatMessage`:
/// `trimChatMessage(StringUtils.normalizeSpace(message.trim()))`.
fn normalize_chat_message(s: &str) -> String {
    // commons-lang3 `normalizeSpace`: each `Character.isWhitespace` run
    // becomes one space and U+00A0 a plain one, then a Java trim.
    let mut spaced = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in java_trim(s).chars() {
        if java_is_whitespace(c) {
            pending_space = !spaced.is_empty();
        } else {
            if pending_space {
                spaced.push(' ');
                pending_space = false;
            }
            spaced.push(if c == '\u{a0}' { ' ' } else { c });
        }
    }
    let collapsed = java_trim(&spaced);
    let mut out = truncate_to_utf16(collapsed, MAX_MESSAGE_LEN);
    // Java's `substring` keeps a split pair's high surrogate, which encodes
    // as `?`.
    if utf16_len(&out) == MAX_MESSAGE_LEN - 1
        && collapsed[out.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.len_utf16() == 2)
    {
        out.push('?');
    }
    out
}

/// Time-based fade for a closed-chat line. Matches vanilla
/// `ChatComponent.AlphaCalculator.timeBased`: full opacity until ~90% of the
/// lifetime, then a squared fade over the final ~10%.
fn line_alpha(age_secs: f32) -> f32 {
    // TODO: vanilla ages lines on `Hud.tickCount`, which freezes while
    // paused; wall time keeps fading here as Pomme has no paused tick clock.
    let mut t = 1.0 - age_secs / MESSAGE_LIFETIME_SECS;
    t *= 10.0;
    t = t.clamp(0.0, 1.0);
    t * t
}

/// A span's formatting, carried per character with its text left empty.
#[derive(Clone, PartialEq)]
struct CharStyle(TextSpan);

type StyledLine = Vec<(char, CharStyle)>;

fn command_input_spans(input: &str, tokens: &[CommandTokenRange]) -> Vec<TextSpan> {
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
            out.push(span.with_text(span.text[local_start..local_end].to_owned()));
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

/// Vanilla `StringDecomposer.iterateFormatted` over each span: legacy codes
/// never carry into the next span, whose own style is both the current and
/// the reset style.
fn legacy_format_spans(spans: &[TextSpan], colors_enabled: bool) -> Vec<TextSpan> {
    let mut out = Vec::new();
    for base in spans {
        let mut current = base.with_text(String::new());
        let mut buffer = String::new();
        let flush = |out: &mut Vec<TextSpan>, current: &TextSpan, buffer: &mut String| {
            if !buffer.is_empty() {
                out.push(current.with_text(std::mem::take(buffer)));
            }
        };

        let mut chars = base.text.chars();
        while let Some(ch) = chars.next() {
            if ch != '\u{00a7}' {
                buffer.push(ch);
                continue;
            }
            let Some(code) = chars.next() else {
                // A trailing section sign ends the iteration, unemitted.
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
                // Chat Colors off strips the code but keeps the base style.
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
                'r' => current = base.with_text(String::new()),
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
    // Per-character styles: vanilla `StringSplitter` keeps styled spaces in
    // place and drops only the space chosen as a line break.
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
        // Each character is measured with its own style, as the splitter's
        // sink does.
        let widths = chars[start..]
            .iter()
            .enumerate()
            .map(|(offset, (ch, style))| {
                (
                    start + offset,
                    *ch,
                    width0(&merge_chars(&[(*ch, style.clone())])),
                )
            });
        match crate::ui::text::find_line_break(widths, max_w) {
            Some((end, next)) => {
                lines.push(chars[start..end].to_vec());
                start = next;
            }
            None => {
                lines.push(chars[start..].to_vec());
                break;
            }
        }
    }

    lines.iter().map(|line| merge_chars(line)).collect()
}

/// Vanilla `ComponentRenderUtils.clipText`: the longest prefix whose width
/// fits `max_w` less the ellipsis (`Font.substrByWidth`), then an unstyled
/// "...". Widths are gui units, measured by `width0` as in [`wrap_spans`].
fn clip_spans(
    spans: &[TextSpan],
    max_w: f32,
    width0: &dyn Fn(&[TextSpan]) -> f32,
) -> Vec<TextSpan> {
    let ellipsis = TextSpan::new("...".to_owned(), common::WHITE);
    let mut remaining = max_w - width0(std::slice::from_ref(&ellipsis));
    let mut out = Vec::new();
    'spans: for span in spans {
        for (i, ch) in span.text.char_indices() {
            remaining -= width0(&[span.with_text(ch.to_string())]);
            if remaining < 0.0 {
                if i > 0 {
                    out.push(span.with_text(span.text[..i].to_owned()));
                }
                break 'spans;
            }
        }
        out.push(span.clone());
    }
    out.push(ellipsis);
    out
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
    use azalea_protocol::packets::game::c_commands::{
        BrigadierNodeStub, BrigadierParser, ClientboundCommands, EntityParser, NodeType,
    };

    use super::*;

    fn span(text: &str, color: [f32; 4]) -> TextSpan {
        TextSpan::new(text.to_string(), color)
    }

    fn line_text(line: &[TextSpan]) -> String {
        line.iter().map(|s| s.text.clone()).collect()
    }

    fn uniform_width(spans: &[TextSpan], per_char: f32) -> f32 {
        spans
            .iter()
            .map(|span| span.text.chars().count() as f32 * per_char)
            .sum()
    }

    /// 10 units per char, 20 when bold.
    fn width(spans: &[TextSpan]) -> f32 {
        spans
            .iter()
            .map(|s| s.text.chars().count() as f32 * if s.bold { 20.0 } else { 10.0 })
            .sum()
    }

    fn open_chat(method: ChatMethod, tree: Option<&CommandTree>) -> ChatState {
        let mut chat = ChatState::new();
        chat.open(method, tree);
        chat
    }

    fn chat_with(options: ChatOptions) -> ChatState {
        let mut chat = ChatState::new();
        chat.set_options(options);
        chat
    }

    /// A chat with a 5s delay whose last message just arrived.
    fn delayed_chat() -> ChatState {
        let mut chat = chat_with(ChatOptions {
            delay_secs: 5.0,
            ..Default::default()
        });
        chat.previous_message_time = Some(Instant::now());
        chat
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

    /// One modal frame on an 800x600 screen at gui scale 1, glyphs 6 units
    /// per char.
    fn build_modal(
        chat: &mut ChatState,
        cursor: (f32, f32),
        clicked: bool,
    ) -> Option<ChatUiAction> {
        let spans_width = |spans: &[TextSpan], _: f32| uniform_width(spans, 6.0);
        let mut elements = Vec::new();
        chat.build_modal_prompt(
            &mut elements,
            800.0,
            600.0,
            1.0,
            cursor,
            clicked,
            &spans_width,
        )
    }

    fn rect_center(rect: [f32; 4]) -> (f32, f32) {
        (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
    }

    fn hit_region(style: ResolvedStyle) -> StyleHitRegion {
        StyleHitRegion {
            rect: [0.0, 0.0, 10.0, 10.0],
            style: Arc::new(style),
        }
    }

    #[test]
    fn command_confirmation_is_modal_and_requires_explicit_acceptance() {
        let mut chat = open_chat(ChatMethod::Message, None);
        chat.request_command_confirmation(
            "unknown command".to_owned(),
            CommandConfirmationKind::ParseErrors,
        );
        assert!(chat.has_pending_modal_prompt());

        // The first frame only lays the buttons out; a click needs them drawn.
        assert!(build_modal(&mut chat, (400.0, 300.0), true).is_none());
        let accept = chat.modal_buttons[0];
        assert!(matches!(
            build_modal(&mut chat, rect_center(accept), true),
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
        assert!(chat.is_open());
    }

    #[test]
    fn link_confirm_layout_matches_vanilla_positions() {
        let screen = ChatModal::Link {
            url: "https://example.com".to_owned(),
        }
        .confirm_screen();
        // 400x300 gui units, 10 units per char. Title and warning are their
        // lang keys (17 chars), the URL 19; the button row is 3x100 + 2x4.
        let layout = screen.layout(400, 300, &width);
        // Layout 308 wide, 9+8+9+8+9+8+36 = 87 tall: origin (46, 106).
        let positions: Vec<(i32, i32)> = layout.lines.iter().map(|l| (l.x, l.y)).collect();
        assert_eq!(positions, vec![(115, 106), (105, 123), (115, 140)]);
        assert_eq!(
            layout.buttons,
            vec![[46, 173, 100, 20], [150, 173, 100, 20], [254, 173, 100, 20]]
        );
        assert_eq!(layout.lines[2].spans[0].color, common::rgb(0xffcccc));
    }

    #[test]
    fn command_confirm_layout_uses_default_button_width() {
        let screen = ChatModal::Command(PendingCommand {
            command: "say".to_owned(),
            kind: CommandConfirmationKind::PermissionsRequired,
        })
        .confirm_screen();
        let layout = screen.layout(400, 300, &width);
        // Without a lang table the message is its 48-char key (480 > 350)
        // wrapped to 2 rows, so the layout is 350 wide from x 25 and the
        // 304-wide row starts at 48. 9+8+18+8+36 = 79 tall, so the
        // top is (300 - 79) / 2 = 110 and the buttons sit at 110+43+16.
        assert_eq!(layout.lines[0].y, 110);
        assert_eq!(layout.lines[1].y, 127);
        assert_eq!(layout.lines[2].y, 136);
        assert_eq!(
            layout.buttons,
            vec![[48, 169, 150, 20], [202, 169, 150, 20]]
        );
    }

    #[test]
    fn long_link_wraps_at_screen_width_less_50() {
        let url = format!("https://example.com/{}", "a".repeat(80));
        let screen = ChatModal::Link { url }.confirm_screen();
        let layout = screen.layout(400, 300, &width);
        // Title, the 100-char URL in rows of at most 35 chars, the warning.
        let message_rows = layout.lines.len() - 2;
        assert_eq!(message_rows, 3);
        for line in &layout.lines[1..=message_rows] {
            assert!(width(&line.spans) <= 350.0);
        }
    }

    #[test]
    fn command_message_argument_is_yellow_without_placeholders() {
        let mut message = ChatModal::Command(PendingCommand {
            command: "give @s dirt".to_owned(),
            kind: CommandConfirmationKind::ParseErrors,
        })
        .confirm_screen()
        .message;
        // The lang table isn't loaded in tests; supply the en_us template.
        let crate::chat_component::Content::Translate { fallback, .. } = &mut message.content
        else {
            panic!("translatable message");
        };
        *fallback = Some(
            "You are trying to execute an unrecognized or invalid command.\nAre you sure?\nCommand: %s"
                .to_owned(),
        );
        let spans = format_component_spans(&message, common::WHITE);
        assert!(spans.iter().all(|span| !span.text.contains("%s")));
        assert!(
            spans
                .iter()
                .any(|span| span.text == "give @s dirt" && span.color == common::rgb(0xffff55))
        );
        let lines = wrap_spans(&spans, 1000.0, &width);
        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[2]), "Command: give @s dirt");
    }

    #[test]
    fn modal_blocks_chat_scroll_and_resets_it_on_open() {
        let mut chat = open_chat(ChatMethod::Message, None);
        chat.scroll_chat(5);
        chat.new_message_since_scroll = true;
        assert_eq!(chat.scroll_pos, 5);
        chat.request_open_url("https://example.com".to_owned());
        assert!(chat.has_pending_modal_prompt());
        assert_eq!(chat.scroll_pos, 0);
        assert!(!chat.new_message_since_scroll);

        chat.handle_scroll((0.0, 0.0), 1.0, false);
        assert_eq!(chat.scroll_pos, 0);
        assert!(!chat.is_focused());
    }

    #[test]
    fn every_modal_button_shows_the_pointer() {
        let mut chat = open_chat(ChatMethod::Message, None);
        chat.request_command_confirmation(
            "say hi".to_owned(),
            CommandConfirmationKind::ParseErrors,
        );
        build_modal(&mut chat, (0.0, 0.0), false);
        let buttons = chat.modal_buttons.clone();
        assert_eq!(buttons.len(), 2);
        for rect in buttons {
            assert!(chat.hovering_clickable(rect_center(rect), false));
        }
        assert!(!chat.hovering_clickable((0.0, 0.0), false));
    }

    #[test]
    fn shift_hover_over_insertion_shows_the_pointer() {
        let mut chat = open_chat(ChatMethod::Message, None);
        chat.hit_regions.push(hit_region(ResolvedStyle {
            insertion: Some("Steve".to_owned()),
            ..ResolvedStyle::default()
        }));
        assert!(!chat.hovering_clickable((5.0, 5.0), false));
        assert!(chat.hovering_clickable((5.0, 5.0), true));
    }

    #[test]
    fn expand_chat_queue_click_is_handled_locally() {
        let mut chat = open_chat(ChatMethod::Message, None);
        chat.delayed_messages.push_back(PendingChatLine {
            spans: vec![span("queued", common::WHITE)],
            signature: None,
            ack_signature: None,
            force_hidden_ack: false,
            suppress_display: false,
            source: ChatMessageSource::Player,
            tag: None,
        });
        chat.hit_regions.push(hit_region(ResolvedStyle {
            click_event: Some(ClickEvent::Custom {
                id: "internal/expand_chat_queue".to_owned(),
                payload: None,
            }),
            ..ResolvedStyle::default()
        }));
        assert!(
            chat.handle_click((5.0, 5.0), false, 100.0, &|_| 0.0, None)
                .is_none()
        );
        assert!(chat.delayed_messages.is_empty());
        assert_eq!(chat.messages.len(), 1);
    }

    #[test]
    fn normalize_collapses_and_trims() {
        assert_eq!(normalize_chat_message("  hello   world  "), "hello world");
        assert_eq!(normalize_chat_message("/say   hi   there"), "/say hi there");
        assert_eq!(normalize_chat_message("   "), "");
    }

    #[test]
    fn normalize_follows_java_whitespace() {
        // U+2007/U+202F aren't Java whitespace, U+00A0 becomes a plain space,
        // U+001F is whitespace, and String.trim strips ASCII controls.
        assert_eq!(
            normalize_chat_message("a\u{202f}b\u{2007}c"),
            "a\u{202f}b\u{2007}c"
        );
        assert_eq!(normalize_chat_message("a\u{a0}\u{a0}b"), "a  b");
        assert_eq!(normalize_chat_message("a\u{1f}\t b"), "a b");
        assert_eq!(normalize_chat_message("\u{1}hi\u{a0}"), "hi");
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
            uniform_width(spans, 1.0)
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
        chat.open(ChatMethod::Message, None);

        let elements = build_elements(&mut chat);
        let first_history_text = elements
            .iter()
            .position(|element| {
                mc_text(element).is_some_and(|text| text == "newer" || text == "older")
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

    /// A frame of `chat` at gui scale 1, glyphs one unit per char per font px.
    fn build_elements(chat: &mut ChatState) -> Vec<MenuElement> {
        let text_width = |text: &str, scale: f32| text.chars().count() as f32 * scale;
        let spans_width = |spans: &[TextSpan], scale: f32| uniform_width(spans, scale);
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
                covered: false,
                command_tree: None,
                advanced_item_tooltips: false,
                text_width_fn: &text_width,
                spans_width_fn: &spans_width,
            },
        );
        assert!(action.is_none());
        elements
    }

    fn mc_text(element: &MenuElement) -> Option<String> {
        let MenuElement::McText { spans, .. } = element else {
            return None;
        };
        Some(line_text(spans))
    }

    fn mc_texts(elements: &[MenuElement]) -> Vec<String> {
        elements.iter().filter_map(mc_text).collect()
    }

    #[test]
    fn chat_scale_zero_draws_no_messages_or_hit_regions() {
        let mut chat = chat_with(ChatOptions {
            scale: 0.0,
            ..Default::default()
        });
        let mut message = span("hello", common::WHITE);
        message.component_style = Some(Arc::new(ResolvedStyle::default()));
        chat.push_message(vec![message]);
        chat.open(ChatMethod::Message, None);
        let elements = build_elements(&mut chat);
        assert!(!mc_texts(&elements).contains(&"hello".to_owned()));
        assert!(chat.hit_regions.is_empty());
    }

    #[test]
    fn only_secure_applies_at_arrival_not_render() {
        let not_secure = |chat: &mut ChatState, text: &str| {
            chat.push_message_with_source(
                vec![span(text, common::WHITE)],
                None,
                ChatMessageSource::Player,
                Some(ChatMessageTag::NotSecure),
            );
        };
        let mut chat = ChatState::new();
        not_secure(&mut chat, "shown");
        chat.set_options(ChatOptions {
            only_secure: true,
            ..Default::default()
        });
        not_secure(&mut chat, "hidden");
        assert_eq!(chat.messages.len(), 1);
        chat.open(ChatMethod::Message, None);
        let texts = mc_texts(&build_elements(&mut chat));
        assert!(texts.contains(&"shown".to_owned()));
        assert!(!texts.contains(&"hidden".to_owned()));
    }

    #[test]
    fn only_secure_hidden_arrival_acknowledges_as_not_shown() {
        let mut chat = chat_with(ChatOptions {
            only_secure: true,
            ..Default::default()
        });
        let signature = [0x42u8; 256];
        chat.push_message_with_source(
            vec![span("unsigned", common::WHITE)],
            Some(signature),
            ChatMessageSource::Player,
            Some(ChatMessageTag::NotSecure),
        );
        assert!(chat.messages.is_empty());
        assert_eq!(chat.previous_message_time, None);
        assert_eq!(
            chat.take_chat_marks(),
            vec![ChatMark::Processed {
                signature,
                shown: false
            }]
        );
    }

    #[test]
    fn clip_spans_keeps_fitting_prefix_and_appends_plain_ellipsis() {
        let mut styled = span("abcdef", common::rgb(0xff5555));
        styled.underline = true;
        let clipped = clip_spans(&[styled], 60.0, &width);
        assert_eq!(line_text(&clipped), "abc...");
        assert!(clipped[0].underline);
        assert!(!clipped[1].underline);
        assert_eq!(clipped[1].color, common::WHITE);
    }

    #[test]
    fn tag_tooltip_wraps_at_vanilla_width() {
        let tag = ChatMessageTag::Modified {
            original: "word ".repeat(10),
        };
        // 10 units per char: 210 units hold 21 chars of "word word ...".
        let lines = wrapped_tooltip_lines(&tag.tooltip_component(), TAG_TOOLTIP_MAX_WIDTH, &width);
        assert!(lines.len() > 2);
        assert!(
            lines
                .iter()
                .all(|line| width(&line.spans) <= TAG_TOOLTIP_MAX_WIDTH)
        );
    }

    #[test]
    fn show_entity_reads_int_array_uuid() {
        let value = serde_json::json!({
            "id": "minecraft:pig",
            "uuid": [0x00112233_i64, 0x44556677, -2003195205, -857870593]
        });
        let lines = entity_tooltip_lines(&value);
        let uuid = lines.last().unwrap();
        assert_eq!(
            line_text(&uuid.spans),
            "00112233-4455-6677-8899-aabbccddeeff"
        );
        assert_eq!(uuid.spans[0].color, common::WHITE);
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| span.color == common::WHITE)
        );
    }

    #[test]
    fn show_item_name_style_overrides_rarity_parent() {
        let item = |name: serde_json::Value| {
            serde_json::json!({
                "id": "minecraft:diamond_sword",
                "components": {
                    "minecraft:custom_name": name,
                    "minecraft:rarity": "epic"
                }
            })
        };
        let explicit = item_tooltip_lines(
            &item(serde_json::json!({"text":"Sword","italic":false,"color":"white"})),
            None,
            false,
        );
        assert!(!explicit[0].spans[0].italic);
        assert_eq!(explicit[0].spans[0].color, common::WHITE);

        let bare = item_tooltip_lines(&item(serde_json::json!({"text":"Sword"})), None, false);
        assert!(bare[0].spans[0].italic);
        assert_eq!(bare[0].spans[0].color, common::rgb(0xff55ff));
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

    fn plain(texts: &[&str]) -> Vec<ChatSuggestion> {
        texts
            .iter()
            .map(|text| ChatSuggestion::plain((*text).to_owned()))
            .collect()
    }

    fn texts(suggestions: &[ChatSuggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.text.as_str()).collect()
    }

    /// An open chat whose latest request, id 0, was for `input`.
    fn awaiting_chat(input: &str) -> ChatState {
        let mut chat = ChatState::new();
        chat.open = true;
        chat.allow_suggestions = true;
        set_input(&mut chat, input);
        chat.pending_suggestions_id = 0;
        chat.pending_suggestions = Some(PendingSuggestions::Awaiting {
            request: input.to_owned(),
            local: SuggestionSet {
                range: input.len()..input.len(),
                list: Vec::new(),
            },
        });
        chat
    }

    #[test]
    fn server_suggestions_replace_and_select_first() {
        let mut chat = awaiting_chat("/gamemode c");
        chat.apply_server_suggestions(0, 10, plain(&["creative"]), None);
        assert_eq!(texts(&chat.suggestions), vec!["creative"]);
        assert_eq!(chat.suggest_range, 10..11);
        assert_eq!(chat.suggest_index, 0);
        assert!(!chat.tab_cycles);
        assert_eq!(chat.pending_suggestions_id, -1);
    }

    #[test]
    fn server_suggestions_stale_dropped() {
        // Not the latest request's id.
        let mut chat = awaiting_chat("/gamemode c");
        chat.apply_server_suggestions(1, 10, plain(&["creative"]), None);
        assert!(chat.suggestions.is_empty());
        assert_eq!(chat.pending_suggestions_id, 0);

        // The input moved on to something that needed no request.
        let mut chat = awaiting_chat("/gamemode c");
        chat.pending_suggestions = None;
        chat.apply_server_suggestions(0, 10, plain(&["creative"]), None);
        assert!(chat.suggestions.is_empty());
        assert_eq!(chat.pending_suggestions_id, -1);
    }

    #[test]
    fn server_suggestions_empty_keeps_local() {
        let mut chat = awaiting_chat("/time set d");
        chat.pending_suggestions = Some(PendingSuggestions::Awaiting {
            request: "/time set d".to_owned(),
            local: SuggestionSet {
                range: 10..11,
                list: plain(&["day"]),
            },
        });
        chat.apply_server_suggestions(0, 10, Vec::new(), None);
        assert_eq!(texts(&chat.suggestions), vec!["day"]);
        assert_eq!(chat.suggest_range, 10..11);
    }

    #[test]
    fn server_suggestions_reset_offset() {
        let mut chat = awaiting_chat("/give @p ");
        chat.suggestions = (0..15)
            .map(|i| ChatSuggestion::plain(format!("old{i}")))
            .collect();
        chat.suggest_offset = 5;
        chat.suggest_index = 12;
        let options = (0..15)
            .map(|i| ChatSuggestion::plain(format!("item{i}")))
            .collect();
        chat.apply_server_suggestions(0, 9, options, None);
        assert_eq!(chat.suggestions.len(), 15);
        assert_eq!(chat.suggest_offset, 0);
        assert_eq!(chat.suggest_index, 0);
    }

    #[test]
    fn ghost_is_selected_suggestion_remainder() {
        let mut chat = ChatState::new();
        set_input(&mut chat, "/gam");
        chat.suggest_original = "/gam".to_owned();
        chat.suggest_range = 1..4;
        chat.suggestions = plain(&["gamemode", "gamerule"]);
        assert_eq!(chat.ghost_suffix().as_deref(), Some("emode"));
        chat.suggest_index = 1;
        assert_eq!(chat.ghost_suffix().as_deref(), Some("erule"));
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
            plain(&["apple", "creative", "minecraft:cow"]),
            "c",
        );
        assert_eq!(texts(&sorted), vec!["creative", "minecraft:cow", "apple"]);
    }

    #[test]
    fn server_suggestions_non_ascii_start() {
        // "/msg héllo " is 11 UTF-16 units but 12 bytes ('é' is 2 bytes).
        let mut chat = awaiting_chat("/msg héllo w");
        chat.apply_server_suggestions(0, 11, plain(&["world"]), None);
        assert_eq!(chat.suggest_range, 12..13);
        assert_eq!(texts(&chat.suggestions), vec!["world"]);

        // Out-of-range start is dropped.
        let mut chat = awaiting_chat("/msg héllo w");
        chat.apply_server_suggestions(0, 99, plain(&["world"]), None);
        assert!(chat.suggestions.is_empty());
    }

    fn node(node_type: NodeType, children: Vec<u32>, executable: bool) -> BrigadierNodeStub {
        BrigadierNodeStub {
            is_executable: executable,
            children,
            redirect_node: None,
            node_type,
            is_restricted: false,
        }
    }

    fn literal(name: &str, children: Vec<u32>, executable: bool) -> BrigadierNodeStub {
        node(
            NodeType::Literal {
                name: name.to_owned(),
            },
            children,
            executable,
        )
    }

    fn argument(
        name: &str,
        parser: BrigadierParser,
        children: Vec<u32>,
        executable: bool,
    ) -> BrigadierNodeStub {
        node(
            NodeType::Argument {
                name: name.to_owned(),
                parser,
                suggestions_type: None,
            },
            children,
            executable,
        )
    }

    /// time set (day|night), msg <targets> <message>, gamemode <mode>,
    /// gamerule.
    fn test_tree() -> CommandTree {
        CommandTree::from_packet(&ClientboundCommands {
            entries: vec![
                node(NodeType::Root, vec![1, 5, 8, 10], false),
                literal("time", vec![2], false),
                literal("set", vec![3, 4], false),
                literal("day", vec![], true),
                literal("night", vec![], true),
                literal("msg", vec![6], false),
                argument(
                    "targets",
                    BrigadierParser::Entity(EntityParser {
                        single: false,
                        players_only: true,
                    }),
                    vec![7],
                    false,
                ),
                argument("message", BrigadierParser::Message, vec![], true),
                literal("gamemode", vec![9], false),
                argument("gamemode", BrigadierParser::Bool, vec![], true),
                literal("gamerule", vec![], true),
            ],
            root_index: 0,
        })
    }

    #[derive(Clone, Copy)]
    struct Keys {
        enter: bool,
        tab: bool,
        up: bool,
        down: bool,
    }

    const NO_KEYS: Keys = Keys {
        enter: false,
        tab: false,
        up: false,
        down: false,
    };
    const TAB: Keys = Keys {
        tab: true,
        ..NO_KEYS
    };
    const ENTER: Keys = Keys {
        enter: true,
        ..NO_KEYS
    };
    const UP: Keys = Keys {
        up: true,
        ..NO_KEYS
    };
    const DOWN: Keys = Keys {
        down: true,
        ..NO_KEYS
    };

    fn input(
        chat: &mut ChatState,
        events: &[TextInputEvent],
        keys: Keys,
        tree: &CommandTree,
    ) -> Option<String> {
        chat.handle_key_input(
            events,
            keys.enter,
            keys.tab,
            false,
            keys.up,
            keys.down,
            false,
            false,
            f32::MAX,
            &|_| 0.0,
            Some(tree),
        )
    }

    fn press(chat: &mut ChatState, keys: Keys, tree: &CommandTree) -> Option<String> {
        input(chat, &[], keys, tree)
    }

    fn type_text(chat: &mut ChatState, text: &str, tree: &CommandTree) {
        let events: Vec<_> = text.chars().map(TextInputEvent::Char).collect();
        input(chat, &events, NO_KEYS, tree);
    }

    /// An unmodified press of `code`.
    fn press_key(chat: &mut ChatState, code: winit::keyboard::KeyCode, tree: &CommandTree) {
        let mods = crate::ui::text_edit::KeyMods {
            shift: false,
            ctrl: false,
            alt: false,
            super_key: false,
        };
        input(chat, &[TextInputEvent::Key { code, mods }], NO_KEYS, tree);
    }

    /// A chat reopened on the saved draft "draft".
    fn restored_draft_chat(tree: &CommandTree) -> ChatState {
        let mut chat = chat_with(ChatOptions {
            save_drafts: true,
            ..Default::default()
        });
        chat.open(ChatMethod::Message, Some(tree));
        type_text(&mut chat, "draft", tree);
        chat.close(ChatExitReason::Intentional);
        chat.open(ChatMethod::Message, Some(tree));
        assert_eq!(chat.input.value(), "draft");
        assert!(chat.is_restored_draft);
        chat
    }

    #[test]
    fn restored_draft_first_backspace_clears() {
        let tree = test_tree();
        let mut chat = restored_draft_chat(&tree);
        press_key(&mut chat, winit::keyboard::KeyCode::Backspace, &tree);
        assert_eq!(chat.input.value(), "");
        assert!(!chat.is_restored_draft);
    }

    #[test]
    fn programmatic_edits_clear_the_restored_draft() {
        let tree = test_tree();
        let wf = |_: &str| 0.0;

        let mut chat = restored_draft_chat(&tree);
        chat.hit_regions.push(hit_region(ResolvedStyle {
            insertion: Some(" more".to_owned()),
            click_event: Some(ClickEvent::SuggestCommand("/time set day".to_owned())),
            ..ResolvedStyle::default()
        }));
        chat.handle_click((5.0, 5.0), true, f32::MAX, &wf, Some(&tree));
        assert_eq!(chat.input.value(), "draft more");
        assert!(!chat.is_restored_draft);

        chat.is_restored_draft = true;
        chat.handle_click((5.0, 5.0), false, f32::MAX, &wf, Some(&tree));
        assert_eq!(chat.input.value(), "/time set day");
        assert!(!chat.is_restored_draft);

        // Backspace after a history recall edits the recalled line instead of
        // wiping it as a draft.
        let mut chat = restored_draft_chat(&tree);
        chat.sent_history.push_back("hello".to_owned());
        chat.history_pos = 1;
        press(&mut chat, UP, &tree);
        assert_eq!(chat.input.value(), "hello");
        assert!(!chat.is_restored_draft);
        press_key(&mut chat, winit::keyboard::KeyCode::Backspace, &tree);
        assert_eq!(chat.input.value(), "hell");
    }

    #[test]
    fn commands_key_restores_only_command_drafts() {
        let tree = test_tree();
        let mut chat = restored_draft_chat(&tree);
        chat.close(ChatExitReason::Interrupted);
        chat.open(ChatMethod::Command, Some(&tree));
        assert_eq!(chat.input.value(), "/");
        assert!(!chat.is_restored_draft);
    }

    #[test]
    fn exit_reasons_decide_the_draft() {
        let tree = test_tree();
        let closed_with = |reason, save_drafts, text: &str| {
            let mut chat = chat_with(ChatOptions {
                save_drafts,
                ..Default::default()
            });
            chat.open(ChatMethod::Message, Some(&tree));
            type_text(&mut chat, text, &tree);
            chat.close(reason);
            chat.latest_draft
        };
        let draft = Some("hi".to_owned());
        assert_eq!(closed_with(ChatExitReason::Interrupted, false, "hi"), draft);
        assert_eq!(closed_with(ChatExitReason::Intentional, true, "hi"), draft);
        assert_eq!(closed_with(ChatExitReason::Intentional, false, "hi"), None);
        assert_eq!(closed_with(ChatExitReason::Done, true, "hi"), None);
        assert_eq!(closed_with(ChatExitReason::Interrupted, true, "  "), None);
    }

    #[test]
    fn submit_closes_as_done() {
        let tree = test_tree();
        let mut chat = chat_with(ChatOptions {
            save_drafts: true,
            ..Default::default()
        });
        chat.open(ChatMethod::Message, Some(&tree));
        type_text(&mut chat, "hi  there", &tree);
        assert_eq!(press(&mut chat, ENTER, &tree).as_deref(), Some("hi there"));
        assert!(!chat.is_open());
        assert_eq!(chat.latest_draft, None);
    }

    #[test]
    fn chat_settings_return_to_the_same_chat() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Message, Some(&tree));
        type_text(&mut chat, "unsent", &tree);
        chat.close_for_settings();
        assert!(!chat.is_open());
        assert!(chat.return_from_settings(Some(&tree)));
        assert!(chat.is_open());
        assert_eq!(chat.input.value(), "unsent");
        assert!(!chat.return_from_settings(Some(&tree)));

        // Something else replacing the settings drops the parent chat.
        chat.close_for_settings();
        chat.close(ChatExitReason::Interrupted);
        assert!(!chat.return_from_settings(Some(&tree)));
    }

    #[test]
    fn open_computes_but_hides_suggestions() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        assert!(chat.suggestions.is_empty());
        assert!(matches!(
            chat.pending_suggestions,
            Some(PendingSuggestions::Done(_))
        ));

        // Tab only shows the list; the input is untouched.
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/");
        assert_eq!(
            texts(&chat.suggestions),
            vec!["gamemode", "gamerule", "msg", "time"]
        );
    }

    #[test]
    fn history_back_to_draft_keeps_suggestions_allowed() {
        let tree = test_tree();
        let mut chat = ChatState::new();
        chat.sent_history.push_back("hello".to_owned());
        chat.open(ChatMethod::Message, Some(&tree));
        type_text(&mut chat, "/ti", &tree);
        assert_eq!(texts(&chat.suggestions), vec!["time"]);
        assert!(!chat.handle_escape());
        assert!(chat.suggestions.is_empty());

        press(&mut chat, UP, &tree);
        assert_eq!(chat.input.value(), "hello");
        assert!(!chat.allow_suggestions);
        press(&mut chat, DOWN, &tree);
        assert_eq!(chat.input.value(), "/ti");
        assert!(chat.allow_suggestions);
        assert_eq!(texts(&chat.suggestions), vec!["time"]);
    }

    #[test]
    fn tab_applies_then_cycles() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "g", &tree);
        assert_eq!(texts(&chat.suggestions), vec!["gamemode", "gamerule"]);
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/gamemode");
        assert_eq!(chat.suggestions.len(), 2);
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/gamerule");
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/gamemode");
        // Applying never asks the server again.
        assert!(chat.take_suggestion_request().is_none());
    }

    #[test]
    fn arrow_selection_is_applied_by_the_next_tab() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "g", &tree);
        press(&mut chat, UP, &tree);
        assert_eq!(chat.suggest_index, 1);
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/gamerule");
    }

    #[test]
    fn suggestions_complete_and_apply_at_the_caret() {
        let tree = test_tree();
        let wf = |_: &str| 0.0;
        let mut chat = open_chat(ChatMethod::Message, Some(&tree));
        set_input(&mut chat, "/ti day");
        chat.input.move_cursor_to(3, false, f32::MAX, &wf);
        chat.allow_suggestions = true;
        chat.update_command_info(Some(&tree));
        assert_eq!(texts(&chat.suggestions), vec!["time"]);
        assert_eq!(chat.suggest_range, 1..3);
        press(&mut chat, TAB, &tree);
        assert_eq!(chat.input.value(), "/time day");
        assert_eq!(chat.input.cursor(), 5);
    }

    #[test]
    fn server_request_is_the_input_up_to_the_caret() {
        let tree = test_tree();
        let wf = |_: &str| 0.0;
        let mut chat = open_chat(ChatMethod::Message, Some(&tree));
        set_input(&mut chat, "/gamemode cx");
        chat.input.move_cursor_to(11, false, f32::MAX, &wf);
        chat.update_command_info(Some(&tree));
        assert_eq!(
            chat.take_suggestion_request(),
            Some((0, "/gamemode c".to_owned()))
        );
    }

    #[test]
    fn request_ids_count_up_and_reset_on_answer() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "gamemode ", &tree);
        assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(0));
        type_text(&mut chat, "t", &tree);
        assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(1));
        chat.apply_server_suggestions(1, 10, plain(&["true"]), None);
        assert_eq!(texts(&chat.suggestions), vec!["true"]);
        type_text(&mut chat, "r", &tree);
        assert_eq!(chat.take_suggestion_request().map(|r| r.0), Some(0));
    }

    fn usage_texts(chat: &ChatState) -> Vec<String> {
        chat.command_usage.iter().map(|l| line_text(l)).collect()
    }

    #[test]
    fn usage_waits_for_completions_and_ignores_the_caret() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "gamemode ", &tree);
        // A server completion is pending: nothing shows yet.
        assert!(chat.command_usage.is_empty());
        chat.apply_server_suggestions(0, 10, Vec::new(), Some(&tree));
        assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
        assert_eq!(chat.command_usage_start, Some(10));

        // Moving the caret isn't an edit, so the lines stay.
        press_key(&mut chat, winit::keyboard::KeyCode::ArrowLeft, &tree);
        assert_eq!(chat.input.cursor(), 9);
        assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
    }

    #[test]
    fn usage_lists_parse_errors_when_nothing_completes() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "gamemode x", &tree);
        let (id, _) = chat.take_suggestion_request().unwrap();
        chat.apply_server_suggestions(id, 10, Vec::new(), Some(&tree));
        let error = SyntaxError {
            key: "parsing.bool.invalid",
            args: vec!["x".to_owned()],
            cursor: Some(9),
        };
        let expected = syntax_error_component("/gamemode x", &error);
        assert_eq!(
            chat.command_usage,
            vec![format_component_spans(&expected, common::WHITE)]
        );

        // With a completion on offer, the usage shows instead.
        type_text(&mut chat, "y", &tree);
        let (id, _) = chat.take_suggestion_request().unwrap();
        chat.apply_server_suggestions(id, 10, plain(&["xyz"]), Some(&tree));
        assert_eq!(usage_texts(&chat), vec!["<gamemode>"]);
    }

    #[test]
    fn escape_hides_the_list_then_closes() {
        let tree = test_tree();
        let mut chat = open_chat(ChatMethod::Command, Some(&tree));
        type_text(&mut chat, "t", &tree);
        assert!(!chat.suggestions.is_empty());
        assert!(!chat.handle_escape());
        assert!(chat.suggestions.is_empty());
        assert!(chat.is_open());
        assert!(chat.handle_escape());
        assert!(!chat.is_open());
    }

    #[test]
    fn commands_only_blocks_messages_and_message_commands() {
        let tree = test_tree();
        let submit = |text: &str| {
            let mut chat = chat_with(ChatOptions {
                visibility: ChatVisibilitySetting::System,
                ..Default::default()
            });
            chat.open(ChatMethod::Message, Some(&tree));
            type_text(&mut chat, text, &tree);
            let sent = press(&mut chat, ENTER, &tree);
            (sent, chat.is_open())
        };
        assert_eq!(submit("hello"), (None, true));
        assert_eq!(submit("/msg x hi"), (None, true));
        assert_eq!(
            submit("/time set day"),
            (Some("/time set day".to_owned()), false)
        );
    }

    #[test]
    fn suggestion_rect_matches_vanilla_geometry() {
        let mut chat = ChatState::new();
        chat.suggest_original = "/give @p ".to_owned();
        chat.suggest_range = 9..9;
        chat.suggestions = plain(&["apple", "stick"]);
        let gui_w = |s: &str| s.chars().count() as f32 * 6.0;
        // getScreenX(9) = 4 + 54, shifted one left; width 30 + 1; two rows
        // ending 15 above the bottom.
        assert_eq!(
            chat.suggestion_rect(320.0, 240.0, &gui_w),
            [57.0, 201.0, 31.0, 24.0]
        );
        // Clamped so the widest entry stays on screen.
        assert_eq!(chat.suggestion_rect(80.0, 240.0, &gui_w)[0], 49.0);
        // The width counts every entry, not just the visible rows.
        chat.suggestions = (0..11)
            .map(|i| ChatSuggestion::plain("a".repeat(i + 1)))
            .collect();
        let rect = chat.suggestion_rect(320.0, 240.0, &gui_w);
        assert_eq!(rect[2], 67.0);
        assert_eq!(rect[3], 120.0);
    }

    #[test]
    fn parse_error_carries_brigadier_context() {
        let expected = |key: &str, position: &str, context: &str| {
            Component::translate(
                "command.context.parse_error",
                vec![
                    Argument::Component(Box::new(Component::translate(key, Vec::new()))),
                    Argument::Number(position.to_owned()),
                    Argument::String(context.to_owned()),
                ],
            )
        };
        assert_eq!(
            parse_error_component(
                "/foo",
                1,
                Component::translate("command.unknown.command", Vec::new())
            ),
            expected("command.unknown.command", "1", "/<--[HERE]")
        );
        // Past ten characters the context keeps the last ten behind "...".
        assert_eq!(
            parse_error_component(
                "/time set abcdefg",
                17,
                Component::translate("command.unknown.argument", Vec::new())
            ),
            expected("command.unknown.argument", "17", "...et abcdefg<--[HERE]")
        );
        // Positions count UTF-16 units.
        assert_eq!(
            parse_error_component(
                "/é",
                3,
                Component::translate("command.unknown.argument", Vec::new())
            ),
            expected("command.unknown.argument", "2", "/é<--[HERE]")
        );
        // A context-free `create()` is the bare message.
        let too_long = SyntaxError {
            key: "argument.message.too_long",
            args: vec!["257".to_owned(), "256".to_owned()],
            cursor: None,
        };
        assert_eq!(
            syntax_error_component("/say x", &too_long),
            Component::translate(
                "argument.message.too_long",
                vec![
                    Argument::String("257".to_owned()),
                    Argument::String("256".to_owned()),
                ],
            )
        );
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
        assert_eq!(options.render_width_px(), 320.0);
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
    fn scaled_chat_wraps_floored_and_renders_ceiled() {
        let options = ChatOptions {
            scale: 0.7,
            ..Default::default()
        };
        assert_eq!(options.wrap_width_px(), 457.0);
        assert_eq!(options.render_width_px(), 458.0);
    }

    #[test]
    fn legacy_formatting_matches_vanilla_segment_reset_rules() {
        let white = common::WHITE;
        let spans = legacy_format_spans(&[span("a§cb§lc§rd", white)], true);
        assert_eq!(line_text(&spans), "abcd");
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
        assert_eq!(line_text(&spans), "abcd");
        assert!(spans.iter().all(|span| span.color == common::rgb(0x55ffff)));
        assert!(spans.iter().all(|span| span.italic));
        assert!(spans.iter().all(|span| !span.bold));
    }

    #[test]
    fn visibility_matches_vanilla_chat_abilities() {
        let mut chat = ChatState::new();
        assert!(chat.source_visible(ChatMessageSource::Player));
        assert!(chat.source_visible(ChatMessageSource::SystemServer));
        assert!(chat.source_visible(ChatMessageSource::SystemClient));

        let mut options = ChatOptions {
            visibility: ChatVisibilitySetting::System,
            ..Default::default()
        };
        chat.set_options(options);
        assert!(!chat.source_visible(ChatMessageSource::Player));
        assert!(chat.source_visible(ChatMessageSource::SystemServer));
        assert!(chat.source_visible(ChatMessageSource::SystemClient));

        options.visibility = ChatVisibilitySetting::Hidden;
        chat.set_options(options);
        assert!(!chat.source_visible(ChatMessageSource::Player));
        assert!(!chat.source_visible(ChatMessageSource::SystemServer));
        // Client-local system messages remain visible in Vanilla.
        assert!(chat.source_visible(ChatMessageSource::SystemClient));
    }

    #[test]
    fn delayed_player_chat_queues_and_accepts_one() {
        let mut chat = delayed_chat();
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
        let mut chat = delayed_chat();
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
        let mut chat = delayed_chat();
        let previous = chat.previous_message_time;
        let signature = [0x33u8; 256];
        chat.push_fully_filtered(Some(signature));
        assert_eq!(chat.delayed_messages.len(), 1);
        assert!(chat.messages.is_empty());

        chat.accept_next_delayed_message();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.previous_message_time, previous);
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
        assert!(item_tooltip_lines(&value, None, false).is_empty());
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
        let lines = item_tooltip_lines(&value, None, true);
        let text = lines
            .iter()
            .map(|line| line_text(&line.spans))
            .collect::<Vec<_>>();
        assert_eq!(text.first().map(String::as_str), Some("Blade"));
        assert!(text.iter().any(|line| line.contains("Sharpness")));
        assert!(text.iter().any(|line| line == "Lore line"));
        assert!(text.iter().any(|line| line.contains("Unbreakable")));
        assert!(text.iter().any(|line| line.contains("1551")));
        assert!(text.iter().any(|line| line == "minecraft:diamond_sword"));
    }

    /// The dialog's item body hands over its raw components, so a lore line's
    /// numbers keep Java's text instead of the JSON detour's widened float.
    #[test]
    fn raw_components_keep_lore_number_formatting() {
        let mut lore_line = NbtCompound::new();
        lore_line.insert("translate", "pomme.unknown");
        lore_line.insert("fallback", "%s");
        lore_line.insert(
            "with",
            NbtTag::List(simdnbt::owned::NbtList::from(vec![NbtTag::Float(0.1)])),
        );
        let mut components = NbtCompound::new();
        components.insert(
            "minecraft:lore",
            NbtTag::List(simdnbt::owned::NbtList::from(vec![NbtTag::Compound(
                lore_line,
            )])),
        );
        let mut stack = NbtCompound::new();
        stack.insert("id", "minecraft:stone");
        stack.insert("components", NbtTag::Compound(components.clone()));
        let value = crate::chat_component::nbt_to_value(&NbtTag::Compound(stack));

        let text = |raw| {
            item_tooltip_lines(&value, raw, false)
                .iter()
                .map(|line| line_text(&line.spans))
                .collect::<Vec<_>>()
        };
        // The JSON shape widens the float to a double.
        assert!(text(None).iter().any(|line| line.contains("0.1000000")));
        assert!(text(Some(&components)).iter().any(|line| line == "0.1"));
    }

    #[test]
    fn server_suggestion_tooltip_is_preserved() {
        let mut chat = awaiting_chat("/example v");
        let tooltip = Component::text("server tooltip");
        chat.apply_server_suggestions(
            0,
            9,
            vec![ChatSuggestion {
                text: "value".to_owned(),
                tooltip: Some(tooltip.clone()),
            }],
            None,
        );
        assert_eq!(chat.suggestions.len(), 1);
        assert_eq!(chat.suggestions[0].text, "value");
        assert_eq!(chat.suggestions[0].tooltip, Some(tooltip));
    }
}
