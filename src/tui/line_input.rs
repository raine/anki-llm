use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Single-line text input with cursor, supporting key events and paste.
#[derive(Default, Debug, Clone)]
pub struct LineInput {
    value: String,
    /// Cursor position in chars (not bytes).
    cursor: usize,
}

impl LineInput {
    pub fn new(value: String) -> Self {
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.value.chars().count());
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn reset(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Visual cursor position accounting for wide characters.
    pub fn visual_cursor(&self) -> usize {
        if self.cursor == 0 {
            return 0;
        }
        let byte_end = self
            .value
            .char_indices()
            .nth(self.cursor)
            .map_or(self.value.len(), |(i, _)| i);
        unicode_width::UnicodeWidthStr::width(&self.value[..byte_end])
    }

    /// Scroll offset for rendering within a given width.
    pub fn visual_scroll(&self, width: usize) -> usize {
        let vc = self.visual_cursor();
        if vc <= width {
            return 0;
        }
        let target = vc - width;
        let mut w = 0;
        for c in self.value.chars() {
            if w >= target {
                break;
            }
            w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        w
    }

    /// Insert text at cursor position (used for paste / IME).
    pub fn insert_str(&mut self, text: &str) {
        let byte_idx = self.cursor_byte_idx();
        self.value.insert_str(byte_idx, text);
        self.cursor += text.chars().count();
    }

    /// Handle a crossterm event. Returns true if the value changed.
    pub fn handle_event(&mut self, evt: &Event) -> bool {
        match evt {
            Event::Key(key) if is_press_or_repeat(key) => self.handle_key(key),
            Event::Paste(text) => {
                let cleaned = text.replace(['\r', '\n'], " ");
                self.insert_str(&cleaned);
                true
            }
            _ => false,
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> bool {
        use KeyCode::*;
        let mods = key.modifiers;
        match (key.code, mods) {
            // Insert
            (Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.insert_char(c);
                true
            }

            // Delete
            (Backspace, KeyModifiers::NONE) | (Char('h'), KeyModifiers::CONTROL) => {
                self.delete_prev_char()
            }
            (Delete, KeyModifiers::NONE) => self.delete_next_char(),
            (Char('u'), KeyModifiers::CONTROL) => self.delete_line(),
            (Char('k'), KeyModifiers::CONTROL) => self.delete_till_end(),
            (Char('w'), KeyModifiers::CONTROL) => self.delete_prev_word(),
            (Backspace, KeyModifiers::META | KeyModifiers::ALT) => self.delete_prev_word(),
            (Char('d'), KeyModifiers::META) => self.delete_prev_word(),
            (Delete, KeyModifiers::CONTROL) => self.delete_next_word(),

            // Move
            (Left, KeyModifiers::NONE) | (Char('b'), KeyModifiers::CONTROL) => self.move_cursor(-1),
            (Right, KeyModifiers::NONE) | (Char('f'), KeyModifiers::CONTROL) => self.move_cursor(1),
            (Left, KeyModifiers::CONTROL) | (Char('b'), KeyModifiers::META) => {
                self.goto_prev_word()
            }
            (Right, KeyModifiers::CONTROL) | (Char('f'), KeyModifiers::META) => {
                self.goto_next_word()
            }
            (Char('a'), KeyModifiers::CONTROL) | (Home, KeyModifiers::NONE) => self.goto_start(),
            (Char('e'), KeyModifiers::CONTROL) | (End, KeyModifiers::NONE) => self.goto_end(),

            _ => false,
        }
    }

    // -- helpers --

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    fn cursor_byte_idx(&self) -> usize {
        self.value
            .char_indices()
            .nth(self.cursor)
            .map_or(self.value.len(), |(i, _)| i)
    }

    fn insert_char(&mut self, c: char) {
        let byte_idx = self.cursor_byte_idx();
        self.value.insert(byte_idx, c);
        self.cursor += 1;
    }

    fn delete_prev_char(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        let idx = self.cursor_byte_idx();
        let end = idx + self.value[idx..].chars().next().unwrap().len_utf8();
        self.value.replace_range(idx..end, "");
        true
    }

    fn delete_next_char(&mut self) -> bool {
        if self.cursor == self.char_count() {
            return false;
        }
        let idx = self.cursor_byte_idx();
        let end = idx + self.value[idx..].chars().next().unwrap().len_utf8();
        self.value.replace_range(idx..end, "");
        true
    }

    fn delete_line(&mut self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value.clear();
        self.cursor = 0;
        true
    }

    fn delete_till_end(&mut self) -> bool {
        let idx = self.cursor_byte_idx();
        if idx == self.value.len() {
            return false;
        }
        self.value.truncate(idx);
        true
    }

    fn delete_prev_word(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        // skip non-alphanumeric
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        // skip alphanumeric
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        let byte_start: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        let byte_end = self.cursor_byte_idx();
        self.value.replace_range(byte_start..byte_end, "");
        self.cursor = i;
        true
    }

    fn delete_next_word(&mut self) -> bool {
        let len = self.char_count();
        if self.cursor == len {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        // skip alphanumeric
        while i < len && chars[i].is_alphanumeric() {
            i += 1;
        }
        // skip non-alphanumeric
        while i < len && !chars[i].is_alphanumeric() {
            i += 1;
        }
        let byte_start = self.cursor_byte_idx();
        let byte_end: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        self.value.replace_range(byte_start..byte_end, "");
        true
    }

    fn move_cursor(&mut self, delta: isize) -> bool {
        let new = self.cursor as isize + delta;
        let new = new.clamp(0, self.char_count() as isize) as usize;
        if new == self.cursor {
            return false;
        }
        self.cursor = new;
        false // cursor-only move, value unchanged
    }

    fn goto_prev_word(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        self.cursor = i;
        false
    }

    fn goto_next_word(&mut self) -> bool {
        let len = self.char_count();
        if self.cursor == len {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i < len && chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < len && !chars[i].is_alphanumeric() {
            i += 1;
        }
        // find start of next word
        if i < len && chars[i].is_alphanumeric() {
            self.cursor = i;
        } else {
            self.cursor = len;
        }
        false
    }

    fn goto_start(&mut self) -> bool {
        self.cursor = 0;
        false
    }

    fn goto_end(&mut self) -> bool {
        self.cursor = self.char_count();
        false
    }
}

fn is_press_or_repeat(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_typing() {
        let mut input = LineInput::default();
        input.insert_char('h');
        input.insert_char('i');
        assert_eq!(input.value(), "hi");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn paste() {
        let mut input = LineInput::new("ab".into()).with_cursor(1);
        input.handle_event(&Event::Paste("XY".into()));
        assert_eq!(input.value(), "aXYb");
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn paste_normalizes_newlines() {
        let mut input = LineInput::default();
        input.handle_event(&Event::Paste("a\nb\rc".into()));
        assert_eq!(input.value(), "a b c");
    }

    #[test]
    fn delete_prev_char_unicode() {
        let mut input = LineInput::new("café".into());
        assert_eq!(input.cursor(), 4);
        input.delete_prev_char();
        assert_eq!(input.value(), "caf");
    }

    #[test]
    fn delete_next_char() {
        let mut input = LineInput::new("abc".into()).with_cursor(1);
        input.delete_next_char();
        assert_eq!(input.value(), "ac");
        assert_eq!(input.cursor(), 1);
    }

    #[test]
    fn delete_prev_word() {
        let mut input = LineInput::new("hello world".into());
        input.delete_prev_word();
        assert_eq!(input.value(), "hello ");
    }

    #[test]
    fn visual_cursor_wide_chars() {
        let input = LineInput::new("Ｈｅ".into()); // fullwidth
        assert_eq!(input.cursor(), 2);
        assert_eq!(input.visual_cursor(), 4); // each fullwidth char is 2 columns
    }

    #[test]
    fn insert_str_at_cursor() {
        let mut input = LineInput::new("ac".into()).with_cursor(1);
        input.insert_str("b");
        assert_eq!(input.value(), "abc");
        assert_eq!(input.cursor(), 2);
    }
}
