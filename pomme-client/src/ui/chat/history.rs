use super::*;

impl ChatState {
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
    pub(super) fn enqueue_or_accept(&mut self, pending: PendingChatLine) {
        let now = Instant::now();
        if pending.source == ChatMessageSource::Player && self.will_delay_messages(now) {
            self.delayed_messages.push_back(pending);
        } else {
            self.accept_pending_message(pending, now);
        }
    }

    pub(super) fn source_visible(&self, source: ChatMessageSource) -> bool {
        match source {
            ChatMessageSource::SystemClient => true,
            ChatMessageSource::SystemServer => {
                self.options.visibility != ChatVisibilitySetting::Hidden
            }
            ChatMessageSource::Player => self.options.visibility == ChatVisibilitySetting::Full,
        }
    }

    pub(super) fn will_delay_messages(&self, now: Instant) -> bool {
        if self.options.delay_secs <= 0.0 {
            return false;
        }
        self.previous_message_time.is_some_and(|previous| {
            now.duration_since(previous).as_secs_f32() < self.options.delay_secs
        })
    }

    pub(super) fn accept_pending_message(
        &mut self,
        pending: PendingChatLine,
        now: Instant,
    ) -> bool {
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
    pub(super) fn mark_processed(&mut self, signature: Option<[u8; 256]>, shown: bool) {
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

    pub(super) fn flush_delayed_messages(&mut self, now: Instant) {
        while let Some(message) = self.delayed_messages.pop_front() {
            self.accept_pending_message(message, now);
        }
        self.previous_message_time = None;
    }

    pub(super) fn process_delayed_messages(&mut self, now: Instant) {
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

    pub(super) fn accept_next_delayed_message(&mut self) {
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
    pub(super) fn delete_message_or_delay(
        &mut self,
        signature: [u8; 256],
        now: Instant,
    ) -> Option<Instant> {
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

    /// Vanilla `ChatComponent.resetChatScroll`.
    pub fn reset_chat_scroll(&mut self) {
        self.scroll_pos = 0;
        self.new_message_since_scroll = false;
    }

    pub(super) fn lines_per_page(&self) -> usize {
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

    /// Up/Down sent-message recall, vanilla `ChatScreen.moveInHistory`.
    pub(super) fn move_in_history(
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

    /// Vanilla `ChatComponent.addRecentChat`: consecutive duplicates collapse.
    pub(super) fn add_recent_chat(&mut self, msg: &str) {
        if self.sent_history.back().map(String::as_str) != Some(msg) {
            self.sent_history.push_back(msg.to_string());
            if self.sent_history.len() > MAX_MESSAGES {
                self.sent_history.pop_front();
            }
        }
    }
}
/// Java `Character.isWhitespace`: Unicode separators except the no-break
/// spaces, plus the ASCII controls Java counts.
pub(super) fn java_is_whitespace(c: char) -> bool {
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
pub(super) fn java_is_blank(s: &str) -> bool {
    s.chars().all(java_is_whitespace)
}

/// Java `String.trim`: strips everything up to U+0020 from both ends.
pub(super) fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// Vanilla `ChatScreen.normalizeChatMessage`:
/// `trimChatMessage(StringUtils.normalizeSpace(message.trim()))`.
pub(super) fn normalize_chat_message(s: &str) -> String {
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
pub(super) fn line_alpha(age_secs: f32) -> f32 {
    // TODO: vanilla ages lines on `Hud.tickCount`, which freezes while
    // paused; wall time keeps fading here as Pomme has no paused tick clock.
    let mut t = 1.0 - age_secs / MESSAGE_LIFETIME_SECS;
    t *= 10.0;
    t = t.clamp(0.0, 1.0);
    t * t
}
