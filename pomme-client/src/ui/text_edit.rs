//! Port of vanilla `EditBox` text-editing logic
//! (net.minecraft.client.gui.components.EditBox).
//!
//! Positions are byte indices on `char` boundaries; cursor movement steps by
//! `char`, which matches vanilla's per-codepoint stepping. `max_length` is the
//! one place UTF-16 semantics survive: it is counted in UTF-16 code units to
//! match Java `String.length()`, so an astral char (emoji) costs 2.

use std::time::Instant;

use winit::keyboard::KeyCode;

pub const BACKWARDS: i32 = -1;
pub const FORWARDS: i32 = 1;

/// 300ms half-period blink, from `TextCursorUtils.CURSOR_BLINK_INTERVAL_MS`.
const CURSOR_BLINK_INTERVAL_MS: u64 = 300;

pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl KeyMods {
    /// `InputWithModifiers.hasControlDownWithQuirk`: the edit modifier is Cmd
    /// (super) on macOS, Ctrl elsewhere
    /// (`InputQuirks.EDIT_SHORTCUT_KEY_MODIFIER`).
    pub fn edit_shortcut(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.super_key
        } else {
            self.ctrl
        }
    }
}

pub enum TextInputEvent {
    Key { code: KeyCode, mods: KeyMods },
    Char(char),
}

/// Clipboard indirection so tests can mock `Minecraft.keyboardHandler`.
pub trait ClipboardAccess {
    fn get(&mut self) -> String;
    fn set(&mut self, s: &str);
}

/// The real OS clipboard (arboard); errors read as empty / write nothing.
pub struct SystemClipboard;

impl ClipboardAccess for SystemClipboard {
    fn get(&mut self) -> String {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.get_text())
            .unwrap_or_default()
    }

    fn set(&mut self, s: &str) {
        crate::ui::common::set_clipboard(s);
    }
}

/// Everything a renderer needs for one frame. `display_start..display_end` is a
/// byte range into `value`; `caret_byte` and `selection` are byte offsets
/// within that displayed slice.
pub struct TextFieldRenderInfo {
    pub display_start: usize,
    pub display_end: usize,
    pub caret_byte: usize,
    pub caret_visible: bool,
    pub selection: Option<(usize, usize)>,
    pub insert_mode: bool,
}

pub struct TextFieldState {
    value: String,
    cursor_pos: usize,
    highlight_pos: usize,
    display_pos: usize,
    max_length: usize,
    focused_time: Instant,
}

impl TextFieldState {
    pub fn new(max_length: usize) -> Self {
        Self {
            value: String::new(),
            cursor_pos: 0,
            highlight_pos: 0,
            display_pos: 0,
            max_length,
            focused_time: Instant::now(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Reset to empty; geometry-free (empty text needs no scroll).
    pub fn clear(&mut self) {
        self.set_value("", 0.0, &|_| 0.0);
    }

    pub fn set_value(&mut self, value: &str, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        // Vanilla truncates by raw UTF-16 index, which could split a surrogate
        // pair into a lone surrogate; Rust strings can't represent that, so we
        // stop on a char boundary instead (differs only for a pathological
        // over-length astral string).
        self.value = if utf16_len(value) > self.max_length {
            truncate_to_utf16(value, self.max_length)
        } else {
            value.to_string()
        };
        self.move_cursor_to_end(false, inner_w, width_fn);
        self.set_highlight_pos(self.cursor_pos, inner_w, width_fn);
    }

    pub fn get_highlighted(&self) -> &str {
        let start = self.cursor_pos.min(self.highlight_pos);
        let end = self.cursor_pos.max(self.highlight_pos);
        &self.value[start..end]
    }

    pub fn cursor_at_end(&self) -> bool {
        self.cursor_pos == self.value.len()
    }

    pub fn insert_text(&mut self, input: &str, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        let start = self.cursor_pos.min(self.highlight_pos);
        let end = self.cursor_pos.max(self.highlight_pos);
        let value_u16 = utf16_len(&self.value);
        let selection_u16 = utf16_len(&self.value[start..end]);
        let budget = self.max_length + selection_u16 - value_u16;
        if budget == 0 {
            return;
        }
        // filterText: keep only allowed chat characters (drops the rest).
        let filtered = input.chars().filter(|&c| is_allowed_chat_character(c));
        // Truncate to the remaining UTF-16 budget without splitting a char; a
        // char needing 2 units when only 1 remains is dropped whole, matching
        // vanilla's `isHighSurrogate` guard.
        let mut taken = 0usize;
        let mut text = String::new();
        for c in filtered {
            let w = c.len_utf16();
            if taken + w > budget {
                break;
            }
            taken += w;
            text.push(c);
        }
        let cursor = start + text.len();
        self.value.replace_range(start..end, &text);
        self.set_cursor_position(cursor, inner_w, width_fn);
        self.set_highlight_pos(self.cursor_pos, inner_w, width_fn);
    }

    fn delete_text(
        &mut self,
        dir: i32,
        whole_word: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        if whole_word {
            self.delete_words(dir, inner_w, width_fn);
        } else {
            self.delete_chars(dir, inner_w, width_fn);
        }
    }

    pub fn delete_words(&mut self, dir: i32, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        if self.value.is_empty() {
            return;
        }
        if self.highlight_pos != self.cursor_pos {
            self.insert_text("", inner_w, width_fn);
            return;
        }
        let pos = self.get_word_position(dir, self.cursor_pos, true);
        self.delete_chars_to_pos(pos, inner_w, width_fn);
    }

    pub fn delete_chars(&mut self, dir: i32, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        let pos = offset_by_chars(&self.value, self.cursor_pos, dir);
        self.delete_chars_to_pos(pos, inner_w, width_fn);
    }

    pub fn delete_chars_to_pos(
        &mut self,
        pos: usize,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        if self.value.is_empty() {
            return;
        }
        if self.highlight_pos != self.cursor_pos {
            self.insert_text("", inner_w, width_fn);
            return;
        }
        let start = pos.min(self.cursor_pos);
        let end = pos.max(self.cursor_pos);
        if start == end {
            return;
        }
        self.value.replace_range(start..end, "");
        self.set_cursor_position(start, inner_w, width_fn);
        self.move_cursor_to(start, false, inner_w, width_fn);
    }

    pub fn get_word_position(&self, dir: i32, from: usize, strip_spaces: bool) -> usize {
        let mut result = from;
        let reverse = dir < 0;
        let abs = dir.unsigned_abs();
        let bytes = self.value.as_bytes();
        for _ in 0..abs {
            if reverse {
                while strip_spaces && result > 0 && bytes[result - 1] == b' ' {
                    result = prev_char_boundary(&self.value, result);
                }
                while result > 0 && bytes[result - 1] != b' ' {
                    result = prev_char_boundary(&self.value, result);
                }
            } else {
                let length = self.value.len();
                match self.value[result..].find(' ') {
                    None => result = length,
                    Some(off) => {
                        result += off;
                        while strip_spaces && result < length && bytes[result] == b' ' {
                            result += 1;
                        }
                    }
                }
            }
        }
        result
    }

    pub fn move_cursor(
        &mut self,
        dir: i32,
        extend: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        let pos = offset_by_chars(&self.value, self.cursor_pos, dir);
        self.move_cursor_to(pos, extend, inner_w, width_fn);
    }

    pub fn move_cursor_to(
        &mut self,
        pos: usize,
        extend: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        self.set_cursor_position(pos, inner_w, width_fn);
        if !extend {
            self.set_highlight_pos(self.cursor_pos, inner_w, width_fn);
        }
    }

    pub fn move_cursor_to_start(
        &mut self,
        extend: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        self.move_cursor_to(0, extend, inner_w, width_fn);
    }

    pub fn move_cursor_to_end(
        &mut self,
        extend: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        self.move_cursor_to(self.value.len(), extend, inner_w, width_fn);
    }

    fn set_cursor_position(&mut self, pos: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        self.cursor_pos = pos.min(self.value.len());
        self.scroll_to(self.cursor_pos, inner_w, width_fn);
    }

    fn set_highlight_pos(&mut self, pos: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        self.highlight_pos = pos.min(self.value.len());
        self.scroll_to(self.highlight_pos, inner_w, width_fn);
    }

    fn scroll_to(&mut self, pos: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        // Callers mutate `value` before scrolling; replacing a selection that
        // started left of the window can leave `display_pos` mid-char in the
        // new string, so snap down before slicing.
        self.display_pos = floor_char_boundary(&self.value, self.display_pos.min(self.value.len()));
        let displayed_len =
            plain_substr_by_width(&self.value[self.display_pos..], inner_w, false, width_fn).len();
        let last_pos = displayed_len + self.display_pos;
        let mut dp = self.display_pos as isize;
        if pos == self.display_pos {
            let suffix_len = plain_substr_by_width(&self.value, inner_w, true, width_fn).len();
            dp -= suffix_len as isize;
        }
        if pos > last_pos {
            dp += (pos - last_pos) as isize;
        } else if (pos as isize) <= dp {
            dp = pos as isize;
        }
        let clamped = dp.clamp(0, self.value.len() as isize) as usize;
        // Byte arithmetic across the whole-value suffix branch can land off a
        // char boundary; snap down so later slicing is safe.
        self.display_pos = floor_char_boundary(&self.value, clamped);
    }

    pub fn key_pressed(
        &mut self,
        code: KeyCode,
        mods: &KeyMods,
        clipboard: &mut dyn ClipboardAccess,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> bool {
        match code {
            KeyCode::ArrowLeft => {
                if mods.edit_shortcut() {
                    let pos = self.get_word_position(BACKWARDS, self.cursor_pos, true);
                    self.move_cursor_to(pos, mods.shift, inner_w, width_fn);
                } else {
                    self.move_cursor(BACKWARDS, mods.shift, inner_w, width_fn);
                }
                true
            }
            KeyCode::ArrowRight => {
                if mods.edit_shortcut() {
                    let pos = self.get_word_position(FORWARDS, self.cursor_pos, true);
                    self.move_cursor_to(pos, mods.shift, inner_w, width_fn);
                } else {
                    self.move_cursor(FORWARDS, mods.shift, inner_w, width_fn);
                }
                true
            }
            KeyCode::Backspace => {
                self.delete_text(BACKWARDS, mods.edit_shortcut(), inner_w, width_fn);
                true
            }
            KeyCode::Delete => {
                self.delete_text(FORWARDS, mods.edit_shortcut(), inner_w, width_fn);
                true
            }
            KeyCode::Home => {
                self.move_cursor_to_start(mods.shift, inner_w, width_fn);
                true
            }
            KeyCode::End => {
                self.move_cursor_to_end(mods.shift, inner_w, width_fn);
                true
            }
            _ => {
                // Clipboard shortcuts require the edit modifier and no shift/alt.
                let clean = mods.edit_shortcut() && !mods.shift && !mods.alt;
                if clean && code == KeyCode::KeyA {
                    self.move_cursor_to_end(false, inner_w, width_fn);
                    self.set_highlight_pos(0, inner_w, width_fn);
                    true
                } else if clean && code == KeyCode::KeyC {
                    clipboard.set(self.get_highlighted());
                    true
                } else if clean && code == KeyCode::KeyV {
                    let text = clipboard.get();
                    self.insert_text(&text, inner_w, width_fn);
                    true
                } else if clean && code == KeyCode::KeyX {
                    clipboard.set(self.get_highlighted());
                    self.insert_text("", inner_w, width_fn);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn char_typed(&mut self, c: char, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) -> bool {
        if is_allowed_chat_character(c) {
            let mut buf = [0u8; 4];
            self.insert_text(c.encode_utf8(&mut buf), inner_w, width_fn);
            true
        } else {
            false
        }
    }

    pub fn handle(
        &mut self,
        ev: &TextInputEvent,
        clipboard: &mut dyn ClipboardAccess,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> bool {
        match ev {
            TextInputEvent::Key { code, mods } => {
                self.key_pressed(*code, mods, clipboard, inner_w, width_fn)
            }
            TextInputEvent::Char(c) => self.char_typed(*c, inner_w, width_fn),
        }
    }

    /// `findClickedPositionInText`: `rel_x` is the click x relative to the text
    /// origin, with the mouse x floored before subtracting like vanilla
    /// (`floor(mouse_x) - text_x`). Honors `display_pos`.
    pub fn pos_from_click(
        &self,
        rel_x: f32,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> usize {
        let position_in_text = rel_x.min(inner_w);
        let displayed = &self.value[self.display_pos..];
        self.display_pos + plain_substr_by_width(displayed, position_in_text, false, width_fn).len()
    }

    pub fn select_word_at(&mut self, pos: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        let word_start = self.get_word_position(BACKWARDS, pos, true);
        let word_end = self.get_word_position(FORWARDS, pos, true);
        self.move_cursor_to(word_start, false, inner_w, width_fn);
        self.move_cursor_to(word_end, true, inner_w, width_fn);
    }

    pub fn on_click(
        &mut self,
        pos: usize,
        shift_held: bool,
        inner_w: f32,
        width_fn: &dyn Fn(&str) -> f32,
    ) {
        self.move_cursor_to(pos, shift_held, inner_w, width_fn);
    }

    pub fn on_drag(&mut self, pos: usize, inner_w: f32, width_fn: &dyn Fn(&str) -> f32) {
        self.move_cursor_to(pos, true, inner_w, width_fn);
    }

    /// Restarts the caret blink on focus gain (vanilla `setFocused`). Focus
    /// itself lives with the owning screen, which passes it to `render_info`.
    pub fn set_focused(&mut self, focused: bool) {
        if focused {
            self.focused_time = Instant::now();
        }
    }

    pub fn render_info(
        &self,
        inner_w: f32,
        focused: bool,
        width_fn: &dyn Fn(&str) -> f32,
    ) -> TextFieldRenderInfo {
        let display_start =
            floor_char_boundary(&self.value, self.display_pos.min(self.value.len()));
        let displayed =
            plain_substr_by_width(&self.value[display_start..], inner_w, false, width_fn);
        let displayed_len = displayed.len();
        let display_end = display_start + displayed_len;

        let rel_cursor = self.cursor_pos as isize - display_start as isize;
        let caret_on_screen = rel_cursor >= 0 && rel_cursor <= displayed_len as isize;
        let caret_byte = rel_cursor.clamp(0, displayed_len as isize) as usize;

        let elapsed = self.focused_time.elapsed().as_millis() as u64;
        let caret_visible =
            focused && (elapsed / CURSOR_BLINK_INTERVAL_MS).is_multiple_of(2) && caret_on_screen;

        let insert_mode =
            self.cursor_pos < self.value.len() || utf16_len(&self.value) >= self.max_length;

        let rel_highlight = (self.highlight_pos as isize - display_start as isize)
            .clamp(0, displayed_len as isize) as usize;
        // Vanilla draws the highlight when the clamped highlight offset differs
        // from the (unclamped) cursor offset.
        let selection = if rel_highlight as isize != rel_cursor {
            let a = caret_byte.min(rel_highlight);
            let b = caret_byte.max(rel_highlight);
            (a != b).then_some((a, b))
        } else {
            None
        };

        TextFieldRenderInfo {
            display_start,
            display_end,
            caret_byte,
            caret_visible,
            selection,
            insert_mode,
        }
    }
}

/// `StringUtil.isAllowedChatCharacter`: not `§`, `>= ' '`, not DEL.
fn is_allowed_chat_character(c: char) -> bool {
    c != '\u{a7}' && c >= ' ' && c != '\u{7f}'
}

/// UTF-16 code-unit length, matching Java `String.length()`.
fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

fn truncate_to_utf16(s: &str, max: usize) -> String {
    let mut acc = 0usize;
    let mut out = String::new();
    for c in s.chars() {
        let w = c.len_utf16();
        if acc + w > max {
            break;
        }
        acc += w;
        out.push(c);
    }
    out
}

/// `Util.offsetByCodepoints`: move `index` by `dir` chars, clamped to the ends.
fn offset_by_chars(s: &str, index: usize, dir: i32) -> usize {
    let mut idx = index;
    if dir >= 0 {
        for _ in 0..dir {
            if idx >= s.len() {
                break;
            }
            idx += s[idx..].chars().next().unwrap().len_utf8();
        }
    } else {
        for _ in 0..(-dir) {
            if idx == 0 {
                break;
            }
            idx = prev_char_boundary(s, idx);
        }
    }
    idx
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    idx - s[..idx].chars().next_back().unwrap().len_utf8()
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// `Font.plainSubstrByWidth`: longest prefix (or suffix, `from_end`) whose
/// accumulated per-char width does not exceed `max_w`. Widths sum per char,
/// matching vanilla's per-codepoint `WidthLimitedCharSink`.
fn plain_substr_by_width<'a>(
    text: &'a str,
    max_w: f32,
    from_end: bool,
    width_fn: &dyn Fn(&str) -> f32,
) -> &'a str {
    let mut width = 0.0f32;
    if !from_end {
        let mut end = 0usize;
        for (i, c) in text.char_indices() {
            width += width_fn(&text[i..i + c.len_utf8()]);
            if width > max_w {
                return &text[..i];
            }
            end = i + c.len_utf8();
        }
        &text[..end]
    } else {
        let mut start = text.len();
        for (i, c) in text.char_indices().rev() {
            width += width_fn(&text[i..i + c.len_utf8()]);
            if width > max_w {
                return &text[start..];
            }
            start = i;
        }
        &text[start..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fake font: 6px per char, so `inner_w / 6` chars fit.
    fn wf(s: &str) -> f32 {
        s.chars().count() as f32 * 6.0
    }
    const W: &dyn Fn(&str) -> f32 = &wf;
    const WIDE: f32 = 100_000.0;

    struct MockClipboard(String);
    impl ClipboardAccess for MockClipboard {
        fn get(&mut self) -> String {
            self.0.clone()
        }
        fn set(&mut self, s: &str) {
            self.0 = s.to_string();
        }
    }

    fn field(value: &str) -> TextFieldState {
        let mut f = TextFieldState::new(256);
        f.set_value(value, WIDE, W);
        f
    }

    fn key(f: &mut TextFieldState, code: KeyCode, mods: KeyMods) -> bool {
        let mut clip = MockClipboard(String::new());
        f.key_pressed(code, &mods, &mut clip, WIDE, W)
    }

    fn plain() -> KeyMods {
        KeyMods {
            shift: false,
            ctrl: false,
            alt: false,
            super_key: false,
        }
    }
    fn shift() -> KeyMods {
        KeyMods {
            shift: true,
            ctrl: false,
            alt: false,
            super_key: false,
        }
    }
    fn ctrl() -> KeyMods {
        KeyMods {
            shift: false,
            ctrl: true,
            alt: false,
            super_key: false,
        }
    }

    #[test]
    fn word_jump_skips_multiple_spaces() {
        let f = field("foo   bar baz");
        // Backwards from end lands at start of "baz".
        assert_eq!(f.get_word_position(BACKWARDS, 13, true), 10);
        // Backwards from start of "bar" strips the run of spaces then the word.
        assert_eq!(f.get_word_position(BACKWARDS, 6, true), 0);
        // Forwards from 0 stops after the space run following "foo".
        assert_eq!(f.get_word_position(FORWARDS, 0, true), 6);
        // Forwards from within the last word runs to the end.
        assert_eq!(f.get_word_position(FORWARDS, 11, true), 13);
    }

    #[test]
    fn insert_replaces_selection() {
        let mut f = field("hello world");
        // Select "hello".
        f.move_cursor_to(0, false, WIDE, W);
        f.move_cursor_to(5, true, WIDE, W);
        assert_eq!(f.get_highlighted(), "hello");
        f.insert_text("hi", WIDE, W);
        assert_eq!(f.value(), "hi world");
        assert_eq!(f.cursor_pos, 2);
        assert_eq!(f.highlight_pos, 2);
    }

    #[test]
    fn insert_filters_disallowed() {
        let mut f = field("");
        f.insert_text("a\u{a7}b\nc", WIDE, W);
        assert_eq!(f.value(), "abc");
    }

    #[test]
    fn max_length_counts_astral_as_two_utf16() {
        let mut f = TextFieldState::new(3);
        f.set_value("", WIDE, W);
        // Emoji is 2 UTF-16 units; only one more unit fits afterwards.
        f.insert_text("\u{1F600}xy", WIDE, W);
        assert_eq!(f.value(), "\u{1F600}x");
        assert_eq!(utf16_len(f.value()), 3);
    }

    #[test]
    fn max_length_does_not_split_astral() {
        let mut f = TextFieldState::new(1);
        f.set_value("", WIDE, W);
        // 1 unit of budget and a leading 2-unit emoji: vanilla truncates the
        // insertion as a prefix, so the emoji can't fit without splitting and
        // nothing is inserted (it does not skip ahead to the `a`).
        f.insert_text("\u{1F600}a", WIDE, W);
        assert_eq!(f.value(), "");
    }

    #[test]
    fn delete_word_backwards() {
        let mut f = field("foo bar baz");
        f.move_cursor_to_end(false, WIDE, W);
        key(&mut f, KeyCode::Backspace, ctrl());
        assert_eq!(f.value(), "foo bar ");
    }

    #[test]
    fn delete_char_forward() {
        let mut f = field("abc");
        f.move_cursor_to(0, false, WIDE, W);
        key(&mut f, KeyCode::Delete, plain());
        assert_eq!(f.value(), "bc");
    }

    #[test]
    fn home_end_with_shift_select() {
        let mut f = field("hello");
        f.move_cursor_to(2, false, WIDE, W);
        key(&mut f, KeyCode::Home, shift());
        assert_eq!(f.get_highlighted(), "he");
        assert_eq!(f.cursor_pos, 0);
        // The anchor stays at the original cursor (2), so shift+End selects [2,5).
        key(&mut f, KeyCode::End, shift());
        assert_eq!(f.get_highlighted(), "llo");
        assert_eq!(f.cursor_pos, 5);
    }

    #[test]
    fn ctrl_a_selects_all_cursor_at_end() {
        let mut f = field("hello");
        f.move_cursor_to(2, false, WIDE, W);
        key(&mut f, KeyCode::KeyA, ctrl());
        assert_eq!(f.cursor_pos, 5);
        assert_eq!(f.highlight_pos, 0);
        assert_eq!(f.get_highlighted(), "hello");
    }

    #[test]
    fn clipboard_copy_paste_cut() {
        let mut clip = MockClipboard(String::new());
        let mut f = field("hello world");
        f.move_cursor_to(0, false, WIDE, W);
        f.move_cursor_to(5, true, WIDE, W);
        // Copy selection.
        f.key_pressed(KeyCode::KeyC, &ctrl(), &mut clip, WIDE, W);
        assert_eq!(clip.0, "hello");
        // Cut selection.
        f.key_pressed(KeyCode::KeyX, &ctrl(), &mut clip, WIDE, W);
        assert_eq!(clip.0, "hello");
        assert_eq!(f.value(), " world");
        // Paste at start.
        f.move_cursor_to(0, false, WIDE, W);
        f.key_pressed(KeyCode::KeyV, &ctrl(), &mut clip, WIDE, W);
        assert_eq!(f.value(), "hello world");
    }

    #[test]
    fn copy_with_no_selection_clears_clipboard() {
        let mut clip = MockClipboard("old".to_string());
        let mut f = field("hello");
        f.move_cursor_to(2, false, WIDE, W);
        f.key_pressed(KeyCode::KeyC, &ctrl(), &mut clip, WIDE, W);
        assert_eq!(clip.0, "");
    }

    #[test]
    fn scroll_to_windows_long_value() {
        // 10 chars fit (60 / 6).
        let inner = 60.0f32;
        let mut f = TextFieldState::new(256);
        f.set_value("0123456789ABCDEF", inner, W);
        // Cursor is at the end after set_value; window shows the tail.
        let info = f.render_info(inner, true, W);
        assert_eq!(
            &f.value()[info.display_start..info.display_end],
            "6789ABCDEF"
        );
        // Move to the start: window scrolls back to the head.
        f.move_cursor_to(0, false, inner, W);
        let info = f.render_info(inner, true, W);
        assert_eq!(info.display_start, 0);
        assert_eq!(
            &f.value()[info.display_start..info.display_end],
            "0123456789"
        );
    }

    #[test]
    fn pos_from_click_honors_display() {
        let inner = 60.0f32;
        let mut f = TextFieldState::new(256);
        f.set_value("0123456789ABCDEF", inner, W);
        f.move_cursor_to(0, false, inner, W);
        // display_pos is 0 here; clicking 3.5 chars in (21px) lands on index 3.
        assert_eq!(f.pos_from_click(21.0, inner, W), 3);
        // Click past the inner width clamps to the last visible char.
        assert_eq!(f.pos_from_click(1000.0, inner, W), 10);
    }

    #[test]
    fn double_click_selects_word() {
        let mut f = field("foo bar baz");
        f.select_word_at(5, WIDE, W);
        // Vanilla's forward word scan strips trailing spaces, so the trailing
        // space is included in the selection.
        assert_eq!(f.get_highlighted(), "bar ");
    }

    #[test]
    fn replace_selection_with_stale_window_does_not_panic() {
        // Narrow window (3 chars) over 20 é. Home puts the caret (and window)
        // at 0; Shift+End extends the selection to the end and scrolls the
        // window right past the anchor. Pasting then replaces the whole value
        // while `display_pos` still points at a byte offset that is mid-char
        // in the pasted string; scroll_to must re-snap before slicing.
        let inner = 18.0f32;
        let pasted = format!("a{}", "\u{e9}".repeat(17));
        let mut clip = MockClipboard(pasted.clone());
        let mut f = TextFieldState::new(256);
        f.set_value(&"\u{e9}".repeat(20), inner, W);
        f.key_pressed(KeyCode::Home, &plain(), &mut clip, inner, W);
        let mut shift = plain();
        shift.shift = true;
        f.key_pressed(KeyCode::End, &shift, &mut clip, inner, W);
        let mut ctrl = plain();
        ctrl.ctrl = true;
        f.key_pressed(KeyCode::KeyV, &ctrl, &mut clip, inner, W);
        assert_eq!(f.value(), pasted);
    }
}
