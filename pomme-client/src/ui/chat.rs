use std::collections::VecDeque;
use std::time::Instant;

use super::common;
use super::common::WHITE;
use crate::net::commands::CommandTree;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::text::TextSpan;

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

struct ChatLine {
    spans: Vec<TextSpan>,
    received: Instant,
}

pub struct ChatState {
    messages: VecDeque<ChatLine>,
    input: String,
    open: bool,
    cursor_blink: Instant,
    suggestions: Vec<String>,
    suggest_index: usize,
    suggest_anchor: String,
    suggest_applied: bool,
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
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            input: String::new(),
            open: false,
            cursor_blink: Instant::now(),
            suggestions: Vec::new(),
            suggest_index: 0,
            suggest_anchor: String::new(),
            suggest_applied: false,
            last_computed: String::new(),
            next_suggest_id: 0,
            awaiting: None,
            outgoing_request: None,
        }
    }

    pub fn push_message(&mut self, spans: Vec<TextSpan>) {
        self.messages.push_back(ChatLine {
            spans,
            received: Instant::now(),
        });
        if self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.input.clear();
        self.cursor_blink = Instant::now();
        self.clear_suggestions();
    }

    pub fn open_with_slash(&mut self) {
        self.open = true;
        self.input = "/".into();
        self.cursor_blink = Instant::now();
        self.clear_suggestions();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.clear_suggestions();
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
        self.last_computed = self.input.clone();
        if let Some(cmd) = self.input.strip_prefix('/')
            && let Some(tree) = tree
        {
            let sug = tree.suggestions(cmd);
            let cut = self.input.len() - sug.partial_len;
            self.suggest_anchor = self.input[..cut].to_string();
            self.suggestions = sug.options;
            if sug.needs_server {
                self.next_suggest_id = self.next_suggest_id.wrapping_add(1);
                let request = (self.next_suggest_id, self.input.clone());
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
        if id != *want_id || !self.open || self.input != *want_input {
            return;
        }
        self.awaiting = None;
        if options.is_empty() {
            return;
        }
        // Java's StringRange counts UTF-16 units, not bytes.
        let Some(start) = utf16_offset_to_byte(&self.input, start) else {
            return;
        };
        let partial = self.input[start..].to_ascii_lowercase();
        self.suggest_anchor = self.input[..start].to_string();
        self.suggestions = sort_with_partial_first(options, &partial);
        self.suggest_index = 0;
        self.suggest_applied = false;
    }

    pub fn handle_key_input(
        &mut self,
        typed_chars: &[char],
        backspace: bool,
        enter: bool,
        tab: bool,
        shift: bool,
        tree: Option<&CommandTree>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }

        for ch in typed_chars {
            self.input.push(*ch);
            self.cursor_blink = Instant::now();
        }
        if backspace {
            self.input.pop();
            self.cursor_blink = Instant::now();
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
            self.input = format!(
                "{}{}",
                self.suggest_anchor, self.suggestions[self.suggest_index]
            );
            self.suggest_applied = true;
            self.last_computed = self.input.clone();
            self.cursor_blink = Instant::now();
            return None;
        }

        if self.input != self.last_computed {
            self.recompute_suggestions(tree);
        }

        if enter {
            let normalized = normalize_chat_message(&self.input);
            let msg = if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            };
            self.input.clear();
            self.open = false;
            self.clear_suggestions();
            return msg;
        }

        None
    }

    /// The grey inline completion: the remainder of the selected suggestion
    /// past what is already typed. Mirrors vanilla
    /// `CommandSuggestions.calculateSuggestionSuffix` (case-sensitive; the
    /// cursor is always at the end of pomme's chat input).
    fn ghost_suffix(&self) -> Option<&str> {
        let selected = self.suggestions.get(self.suggest_index)?;
        let rest = self.input.strip_prefix(self.suggest_anchor.as_str())?;
        let suffix = selected.strip_prefix(rest)?;
        (!suffix.is_empty()).then_some(suffix)
    }

    pub fn build(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        text_width_fn: &dyn Fn(&str, f32) -> f32,
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
        let width0 = |s: &str| text_width_fn(s, common::FONT_SIZE);

        // Gather the visible wrapped lines newest-first; index 0 is the
        // bottom-most line. All wrapped lines of a message share its alpha.
        let mut display: Vec<(Vec<TextSpan>, f32)> = Vec::new();
        for msg in self.messages.iter().rev() {
            let alpha = if self.open {
                1.0
            } else {
                line_alpha(now.duration_since(msg.received).as_secs_f32())
            };
            if !self.open && alpha <= 1e-5 {
                continue;
            }
            let wrapped = wrap_spans(&msg.spans, CHAT_WIDTH, &width0);
            for line in wrapped.into_iter().rev() {
                display.push((line, alpha));
                if display.len() >= LINES_PER_PAGE {
                    break;
                }
            }
            if display.len() >= LINES_PER_PAGE {
                break;
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
            elements.push(MenuElement::Text {
                x: origin + indent,
                y: text_y,
                text: self.input.clone(),
                scale: fs,
                color: WHITE,
                centered: false,
            });

            let tw = text_width_fn(&self.input, fs);
            // Vanilla `EditBox` draws the suggestion at `cursorX - 1`.
            if let Some(ghost) = self.ghost_suffix() {
                elements.push(MenuElement::Text {
                    x: origin + indent + tw - gs,
                    y: text_y,
                    text: ghost.to_string(),
                    scale: fs,
                    color: GHOST_TEXT,
                    centered: false,
                });
            }
            common::push_cursor_blink(
                elements,
                &self.cursor_blink,
                origin + indent,
                text_y,
                gs,
                fs,
                tw,
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
    collapsed.chars().take(MAX_MESSAGE_LEN).collect()
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

#[derive(Clone, Copy, PartialEq)]
struct CharStyle {
    color: [f32; 4],
    bold: bool,
    italic: bool,
    strikethrough: bool,
    underline: bool,
}

type StyledLine = Vec<(char, CharStyle)>;

fn styled_text(chars: &[(char, CharStyle)]) -> String {
    chars.iter().map(|(c, _)| *c).collect()
}

/// Greedy word-wrap of styled text to `max_w` gui-space units, preserving each
/// character's color/style and hard-breaking any single word wider than the
/// line. Returns one `Vec<TextSpan>` per display line. Mirrors vanilla
/// `Font.split` over a `FormattedText`. `width0` measures text width at
/// gui-scale 1.
fn wrap_spans(spans: &[TextSpan], max_w: f32, width0: &dyn Fn(&str) -> f32) -> Vec<Vec<TextSpan>> {
    // Split into whitespace-delimited words, keeping each character's style.
    let mut words: Vec<StyledLine> = Vec::new();
    let mut word: StyledLine = Vec::new();
    for s in spans {
        let style = CharStyle {
            color: s.color,
            bold: s.bold,
            italic: s.italic,
            strikethrough: s.strikethrough,
            underline: s.underline,
        };
        for ch in s.text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                word.push((ch, style));
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
            if width0(&format!("{} {}", styled_text(&cur), styled_text(&w))) <= max_w {
                cur.push((' ', w[0].1));
                cur.extend(w);
                continue;
            }
            lines.push(std::mem::take(&mut cur));
        }
        // cur is empty here: start a fresh line, hard-breaking an oversized word.
        if width0(&styled_text(&w)) <= max_w {
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
    width0: &dyn Fn(&str) -> f32,
) -> (Vec<StyledLine>, StyledLine) {
    let mut out: Vec<StyledLine> = Vec::new();
    let mut piece: StyledLine = Vec::new();
    for &(ch, st) in word {
        let mut test = styled_text(&piece);
        test.push(ch);
        if width0(&test) > max_w && !piece.is_empty() {
            out.push(std::mem::take(&mut piece));
        }
        piece.push((ch, st));
    }
    (out, piece)
}

/// Coalesce a run of styled characters into `TextSpan`s, merging neighbours
/// that share the same style.
fn merge_chars(chars: &[(char, CharStyle)]) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut last_style: Option<CharStyle> = None;
    for &(ch, st) in chars {
        if last_style == Some(st) {
            spans.last_mut().unwrap().text.push(ch);
        } else {
            spans.push(TextSpan {
                text: ch.to_string(),
                color: st.color,
                bold: st.bold,
                italic: st.italic,
                strikethrough: st.strikethrough,
                underline: st.underline,
            });
            last_style = Some(st);
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
        // Each char is 10 units wide; lines fit 5 chars.
        let width = |s: &str| s.chars().count() as f32 * 10.0;
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
        let width = |s: &str| s.chars().count() as f32 * 10.0;
        let lines = wrap_spans(&[span("aaaaaaa", [1.0; 4])], 30.0, &width);
        let texts: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
        assert_eq!(texts, vec!["aaa", "aaa", "a"]);
    }

    #[test]
    fn wrap_spans_empty_is_one_blank_line() {
        let width = |s: &str| s.chars().count() as f32 * 10.0;
        let lines = wrap_spans(&[], 50.0, &width);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].is_empty());
    }

    /// A chat awaiting a server response for `input` with request id 1.
    fn awaiting_chat(input: &str) -> ChatState {
        let mut chat = ChatState::new();
        chat.open = true;
        chat.input = input.to_string();
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
        chat.input = "/gamemode cr".to_string();
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
        chat.input = "/gam".to_string();
        chat.suggest_anchor = "/".to_string();
        chat.suggestions = vec!["gamemode".into(), "gamerule".into()];
        assert_eq!(chat.ghost_suffix(), Some("emode"));
        chat.suggest_index = 1;
        assert_eq!(chat.ghost_suffix(), Some("erule"));
        // Case mismatch shows no ghost (vanilla is case-sensitive here).
        chat.input = "/GAM".to_string();
        assert_eq!(chat.ghost_suffix(), None);
        // Fully typed suggestion leaves nothing to show.
        chat.input = "/gamerule".to_string();
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
