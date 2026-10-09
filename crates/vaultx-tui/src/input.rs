//! A small text editor for single-line and multi-line input fields.
//!
//! The buffer is a [`Zeroizing`] string, so master passwords and secrets
//! typed into a field are wiped when the field is dropped.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;
use zeroize::Zeroizing;

/// Character shown instead of each character of a masked value.
pub const MASK_CHAR: char = '•';

/// What a key press did to the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResult {
    /// The value changed.
    Edited,
    /// Only the cursor moved.
    Moved,
    /// The key is not handled by the input (the caller may use it).
    Ignored,
}

/// The printable character of a key event, if it should be inserted as
/// text. Ctrl or Alt alone mark a shortcut; Ctrl+Alt is AltGr on Windows
/// (e.g. `@` on a German keyboard) and counts as text.
pub fn text_char(key: &KeyEvent) -> Option<char> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl != alt || c.is_control() {
        None
    } else {
        Some(c)
    }
}

/// Editable text with a cursor.
#[derive(Debug)]
pub struct TextInput {
    value: Zeroizing<String>,
    /// Byte index into `value`, always on a char boundary.
    cursor: usize,
    multiline: bool,
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Rendered view of an input: visible lines and the cursor position
/// relative to the view.
#[derive(Debug, PartialEq, Eq)]
pub struct InputView {
    pub lines: Vec<String>,
    pub cursor_x: u16,
    pub cursor_y: u16,
}

impl TextInput {
    /// Empty single-line input.
    pub fn new() -> Self {
        TextInput {
            // Reserve upfront so typing rarely reallocates (a reallocation
            // would leave an unwiped copy of the old buffer behind).
            value: Zeroizing::new(String::with_capacity(128)),
            cursor: 0,
            multiline: false,
        }
    }

    /// Single-line input with an initial value (cursor at the end).
    pub fn with_value(value: &str) -> Self {
        let mut input = Self::new();
        input.set_value(value);
        input
    }

    /// Multi-line input with an initial value (Enter inserts a newline).
    pub fn multiline(value: &str) -> Self {
        let mut input = Self::new();
        input.multiline = true;
        input.set_value(value);
        input
    }

    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Replaces the value and moves the cursor to the end.
    pub fn set_value(&mut self, value: &str) {
        self.value.clear();
        if self.multiline {
            self.value.push_str(&value.replace("\r\n", "\n"));
        } else {
            self.value.push_str(&single_line(value));
        }
        self.cursor = self.value.len();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Inserts pasted text at the cursor. Single-line inputs drop a
    /// trailing line break and turn inner line breaks/tabs into spaces.
    pub fn insert_str(&mut self, text: &str) {
        let text = if self.multiline {
            text.replace("\r\n", "\n")
        } else {
            single_line(text)
        };
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' && !self.multiline {
            return;
        }
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.value[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.value[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.value[self.cursor..]
            .find('\n')
            .map_or(self.value.len(), |i| self.cursor + i)
    }

    /// Moves to the previous line (multi-line only). False on the first line.
    fn line_up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = self.value[start..self.cursor].chars().count();
        let prev_start = self.value[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = advance_chars(&self.value, prev_start, start - 1, col);
        true
    }

    /// Moves to the next line (multi-line only). False on the last line.
    fn line_down(&mut self) -> bool {
        let end = self.line_end();
        if end >= self.value.len() {
            return false;
        }
        let col = self.value[self.line_start()..self.cursor].chars().count();
        let next_start = end + 1;
        let next_end = self.value[next_start..]
            .find('\n')
            .map_or(self.value.len(), |i| next_start + i);
        self.cursor = advance_chars(&self.value, next_start, next_end, col);
        true
    }

    /// Applies an editing key. Enter, Tab, Esc and shortcuts are left to
    /// the caller (Enter only inserts a newline in multi-line inputs; Up
    /// and Down are only handled inside multi-line text).
    pub fn handle_key(&mut self, key: &KeyEvent) -> InputResult {
        if let Some(c) = text_char(key) {
            self.insert_char(c);
            return InputResult::Edited;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter if self.multiline && !ctrl => {
                self.insert_char('\n');
                InputResult::Edited
            }
            KeyCode::Backspace => match self.prev_boundary() {
                Some(prev) => {
                    self.value.replace_range(prev..self.cursor, "");
                    self.cursor = prev;
                    InputResult::Edited
                }
                None => InputResult::Moved,
            },
            KeyCode::Delete => match self.next_boundary() {
                Some(next) => {
                    self.value.replace_range(self.cursor..next, "");
                    InputResult::Edited
                }
                None => InputResult::Moved,
            },
            KeyCode::Left => {
                if let Some(prev) = self.prev_boundary() {
                    self.cursor = prev;
                }
                InputResult::Moved
            }
            KeyCode::Right => {
                if let Some(next) = self.next_boundary() {
                    self.cursor = next;
                }
                InputResult::Moved
            }
            KeyCode::Home => {
                self.cursor = if ctrl { 0 } else { self.line_start() };
                InputResult::Moved
            }
            KeyCode::End => {
                self.cursor = if ctrl {
                    self.value.len()
                } else {
                    self.line_end()
                };
                InputResult::Moved
            }
            KeyCode::Up if self.multiline => {
                if self.line_up() {
                    InputResult::Moved
                } else {
                    InputResult::Ignored
                }
            }
            KeyCode::Down if self.multiline => {
                if self.line_down() {
                    InputResult::Moved
                } else {
                    InputResult::Ignored
                }
            }
            // Ctrl+U clears the field (as in most terminals).
            KeyCode::Char('u') if ctrl => {
                self.clear();
                InputResult::Edited
            }
            _ => InputResult::Ignored,
        }
    }

    /// Visible part of the input for a viewport of `width` × `height`
    /// cells, scrolled so that the cursor is visible. `masked` replaces
    /// every character with [`MASK_CHAR`].
    pub fn view(&self, width: u16, height: u16, masked: bool) -> InputView {
        let width = usize::from(width.max(1));
        let height = usize::from(height.max(1));
        let display = |s: &str| -> String {
            if masked {
                s.chars().map(|_| MASK_CHAR).collect()
            } else {
                s.chars().map(|c| if c == '\t' { ' ' } else { c }).collect()
            }
        };
        let before = &self.value[..self.cursor];
        let cursor_line = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let cursor_col = str_width(&display(&self.value[line_start..self.cursor]));

        let top = (cursor_line + 1).saturating_sub(height);
        let left = (cursor_col + 1).saturating_sub(width);
        let lines = self
            .value
            .split('\n')
            .skip(top)
            .take(height)
            .map(|line| slice_columns(&display(line), left, width))
            .collect();
        InputView {
            lines,
            cursor_x: u16::try_from(cursor_col - left).unwrap_or(u16::MAX),
            cursor_y: u16::try_from(cursor_line - top).unwrap_or(u16::MAX),
        }
    }
}

/// Display width of a string in terminal cells.
pub fn str_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The part of `s` between display columns `left` and `left + width`.
pub fn slice_columns(s: &str, left: usize, width: usize) -> String {
    let mut col = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if col >= left && col + w <= left + width {
            out.push(c);
        }
        col += w;
        if col >= left + width {
            break;
        }
    }
    out
}

/// Truncates `s` to `width` display columns, ending with "…" if cut.
pub fn truncate(s: &str, width: usize) -> String {
    if str_width(s) <= width {
        return s.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = slice_columns(s, 0, width - 1);
    out.push('…');
    out
}

fn single_line(text: &str) -> String {
    text.trim_end_matches(['\r', '\n'])
        .chars()
        .map(|c| {
            if matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Byte index `col` characters after `start`, but not beyond `end`.
fn advance_chars(s: &str, start: usize, end: usize, col: usize) -> usize {
    s[start..end]
        .char_indices()
        .nth(col)
        .map_or(end, |(i, _)| start + i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(input: &mut TextInput, s: &str) {
        for c in s.chars() {
            input.handle_key(&key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_and_editing() {
        let mut i = TextInput::new();
        type_str(&mut i, "hällo");
        assert_eq!(i.value(), "hällo");
        i.handle_key(&key(KeyCode::Left));
        i.handle_key(&key(KeyCode::Left));
        i.handle_key(&key(KeyCode::Backspace));
        assert_eq!(i.value(), "hälo");
        i.handle_key(&key(KeyCode::Delete));
        assert_eq!(i.value(), "häo");
        i.handle_key(&key(KeyCode::Home));
        type_str(&mut i, "X");
        assert_eq!(i.value(), "Xhäo");
        i.handle_key(&key(KeyCode::End));
        assert_eq!(i.handle_key(&key(KeyCode::Delete)), InputResult::Moved);
        assert_eq!(i.handle_key(&key(KeyCode::Enter)), InputResult::Ignored);
        assert_eq!(i.handle_key(&key(KeyCode::Up)), InputResult::Ignored);
        i.handle_key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(i.is_empty());
    }

    #[test]
    fn shortcuts_are_not_text_but_altgr_is() {
        let ctrl_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(text_char(&ctrl_g), None);
        let altgr_at = KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(text_char(&altgr_at), Some('@'));
        let shift_a = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(text_char(&shift_a), Some('A'));
    }

    #[test]
    fn paste_into_single_line() {
        let mut i = TextInput::new();
        i.insert_str("pass\tword\r\n");
        assert_eq!(i.value(), "pass word");
        let mut m = TextInput::multiline("");
        m.insert_str("a\r\nb");
        assert_eq!(m.value(), "a\nb");
    }

    #[test]
    fn multiline_navigation() {
        let mut m = TextInput::multiline("first line\nab\nthird");
        // Cursor at the end of "third" (col 5) -> up to "ab" (clamped to 2).
        assert_eq!(m.handle_key(&key(KeyCode::Up)), InputResult::Moved);
        type_str(&mut m, "!");
        assert_eq!(m.value(), "first line\nab!\nthird");
        assert_eq!(m.handle_key(&key(KeyCode::Up)), InputResult::Moved);
        assert_eq!(m.handle_key(&key(KeyCode::Up)), InputResult::Ignored);
        m.handle_key(&key(KeyCode::Home));
        m.handle_key(&key(KeyCode::Enter));
        assert_eq!(m.value(), "\nfirst line\nab!\nthird");
        m.handle_key(&KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(m.handle_key(&key(KeyCode::Down)), InputResult::Ignored);
    }

    #[test]
    fn view_scrolls_to_cursor_and_masks() {
        let i = TextInput::with_value("abcdefghij");
        let v = i.view(5, 1, false);
        assert_eq!(v.lines, vec!["ghij".to_owned()]);
        assert_eq!((v.cursor_x, v.cursor_y), (4, 0));
        let v = i.view(20, 1, true);
        assert_eq!(v.lines[0], "••••••••••");
        assert_eq!(v.cursor_x, 10);

        let m = TextInput::multiline("1\n2\n3\n4");
        let v = m.view(10, 2, false);
        assert_eq!(v.lines, vec!["3".to_owned(), "4".to_owned()]);
        assert_eq!((v.cursor_x, v.cursor_y), (1, 1));
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("GitHub", 10), "GitHub");
        assert_eq!(truncate("GitHub Enterprise", 8), "GitHub …");
        assert_eq!(truncate("abc", 0), "");
    }
}
