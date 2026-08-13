//! A one-line text input with a cursor: the editing primitive behind the cue
//! editor, the chapter-title editor and the export-name prompt.
//!
//! The cursor is a byte offset kept on a `char` boundary, so inserting into the
//! middle of a multi-byte sentence — which subtitles usually are — is safe. For
//! display the input wraps to a given number of columns; wrapping is measured in
//! terminal cells, so CJK text (two cells per character) lands where it looks
//! like it should.

use std::ops::Range;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

use crate::util::display_width;

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset of the cursor: always on a `char` boundary, never past the end.
    cursor: usize,
}

/// A [`TextInput`] laid out over a fixed number of columns.
pub struct Wrapped {
    /// Byte ranges of the visual lines. Contiguous and covering the whole text,
    /// so every byte belongs to exactly one line; never empty.
    pub lines: Vec<Range<usize>>,
    /// Index into `lines` of the line the cursor sits on.
    pub cursor_line: usize,
}

impl TextInput {
    /// Start editing `text`, cursor at its end (where typing continues it).
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self { text, cursor }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Take the text out, leaving the input empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Apply one keypress. Enter and Esc are the caller's (they end the edit),
    /// so they are ignored here.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char('b') if ctrl => self.move_left(),
            KeyCode::Char('f') if ctrl => self.move_right(),
            KeyCode::Char('b') if alt => self.cursor = self.word_start(),
            KeyCode::Char('f') if alt => self.cursor = self.word_end(),
            KeyCode::Char('h') if ctrl => self.backspace(),
            KeyCode::Char('d') if ctrl => self.delete(),
            KeyCode::Char('k') if ctrl => self.text.truncate(self.cursor),
            KeyCode::Char('u') if ctrl => self.clear(),
            KeyCode::Char('w') if ctrl => self.delete_word_before(),
            // An unmapped chord must not type its letter into the text.
            KeyCode::Char(_) if ctrl || alt => {}
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Left if ctrl || alt => self.cursor = self.word_start(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_end(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            _ => {}
        }
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.text.replace_range(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    fn delete_word_before(&mut self) {
        let start = self.word_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn move_left(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.cursor = prev;
        }
    }

    fn move_right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.cursor = next;
        }
    }

    fn prev_boundary(&self) -> Option<usize> {
        let c = self.text[..self.cursor].chars().next_back()?;
        Some(self.cursor - c.len_utf8())
    }

    fn next_boundary(&self) -> Option<usize> {
        let c = self.text[self.cursor..].chars().next()?;
        Some(self.cursor + c.len_utf8())
    }

    /// Start of the word before the cursor: skip any blanks, then the word.
    /// Scripts without spaces (Japanese) have one word per run, which is what
    /// makes this a "jump to the beginning" key there.
    fn word_start(&self) -> usize {
        let mut i = self.cursor;
        while let Some(c) = self.text[..i].chars().next_back() {
            if !c.is_whitespace() {
                break;
            }
            i -= c.len_utf8();
        }
        while let Some(c) = self.text[..i].chars().next_back() {
            if c.is_whitespace() {
                break;
            }
            i -= c.len_utf8();
        }
        i
    }

    /// End of the word after the cursor (mirror of [`Self::word_start`]).
    fn word_end(&self) -> usize {
        let mut i = self.cursor;
        while let Some(c) = self.text[i..].chars().next() {
            if !c.is_whitespace() {
                break;
            }
            i += c.len_utf8();
        }
        while let Some(c) = self.text[i..].chars().next() {
            if c.is_whitespace() {
                break;
            }
            i += c.len_utf8();
        }
        i
    }

    /// Lay the text out over `width` columns and say which line holds the cursor.
    pub fn wrap(&self, width: usize) -> Wrapped {
        let width = width.max(1);
        let mut lines = wrap(&self.text, width);
        // A cursor at the very end of a full line needs a line of its own,
        // otherwise there is no cell left to draw it in.
        if self.cursor == self.text.len() {
            let last = lines.last().cloned().unwrap_or(0..0);
            if display_width(&self.text[last]) >= width {
                lines.push(self.text.len()..self.text.len());
            }
        }
        let cursor_line = lines
            .iter()
            .position(|r| r.contains(&self.cursor))
            .unwrap_or(lines.len() - 1);
        Wrapped { lines, cursor_line }
    }
}

/// Wrap `text` to `width` columns, breaking after a blank when the line has one
/// (so words stay whole) and mid-run otherwise. The blank stays on the line it
/// ended, and no byte is ever dropped: the ranges tile the whole string, which
/// is what lets the caller map a cursor offset onto a line.
pub fn wrap(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    let mut col = 0;
    // Byte offset just past the last blank on the current line, if any.
    let mut after_blank: Option<usize> = None;

    for (i, c) in text.char_indices() {
        let cw = c.width().unwrap_or(0);
        // `i > start` keeps at least one character per line, so a character
        // wider than the pane can't spin here forever.
        if col + cw > width && i > start {
            let brk = match after_blank {
                Some(b) if b > start && b < i => b,
                _ => i,
            };
            lines.push(start..brk);
            start = brk;
            after_blank = None;
            col = display_width(&text[start..i]);
        }
        col += cw;
        if c.is_whitespace() {
            after_blank = Some(i + c.len_utf8());
        }
    }
    lines.push(start..text.len());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn press(input: &mut TextInput, keys: &[KeyEvent]) {
        for k in keys {
            input.handle_key(*k);
        }
    }

    #[test]
    fn typing_lands_where_the_cursor_is() {
        let mut input = TextInput::with_text("これはテストです");
        // Left four times: between "これは" and "テスト".
        press(&mut input, &[key(KeyCode::Left); 5]);
        press(&mut input, &[key(KeyCode::Char('大'))]);
        assert_eq!(input.text(), "これは大テストです");

        // And the cursor followed the insertion, so the next one abuts it.
        press(&mut input, &[key(KeyCode::Char('小'))]);
        assert_eq!(input.text(), "これは大小テストです");
    }

    #[test]
    fn backspace_and_delete_straddle_the_cursor() {
        let mut input = TextInput::with_text("日本語");
        press(&mut input, &[key(KeyCode::Left)]);
        press(&mut input, &[key(KeyCode::Backspace)]);
        assert_eq!(input.text(), "日語", "deletes the char before the cursor");
        press(&mut input, &[key(KeyCode::Delete)]);
        assert_eq!(input.text(), "日", "deletes the char under the cursor");
        // Both are no-ops at their respective ends.
        press(&mut input, &[key(KeyCode::Delete), key(KeyCode::Home), key(KeyCode::Backspace)]);
        assert_eq!(input.text(), "日");
    }

    #[test]
    fn motion_keys_clamp_at_the_ends() {
        let mut input = TextInput::with_text("あい");
        press(&mut input, &[key(KeyCode::Left); 5]);
        assert_eq!(input.cursor(), 0);
        press(&mut input, &[key(KeyCode::Right); 5]);
        assert_eq!(input.cursor(), "あい".len());
        press(&mut input, &[key(KeyCode::Home)]);
        assert_eq!(input.cursor(), 0);
        press(&mut input, &[ctrl('e')]);
        assert_eq!(input.cursor(), "あい".len());
        press(&mut input, &[ctrl('a')]);
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn word_motion_and_kills() {
        let mut input = TextInput::with_text("one two three");
        press(&mut input, &[KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)]);
        assert_eq!(input.cursor(), "one two ".len());
        press(&mut input, &[ctrl('w')]);
        assert_eq!(input.text(), "one three");
        press(&mut input, &[ctrl('k')]);
        assert_eq!(input.text(), "one ");
        press(&mut input, &[ctrl('u')]);
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn unmapped_chords_do_not_type_their_letter() {
        let mut input = TextInput::with_text("abc");
        press(&mut input, &[ctrl('c'), ctrl('z')]);
        assert_eq!(input.text(), "abc");
    }

    #[test]
    fn wrapping_measures_cells_not_chars() {
        // Each kana is two cells wide, so four fit in eight columns.
        let text = "あいうえおか";
        let lines = wrap(text, 8);
        assert_eq!(
            lines.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(),
            vec!["あいうえ", "おか"]
        );
    }

    #[test]
    fn wrapping_keeps_words_whole_when_it_can() {
        let text = "the quick brown fox";
        let lines = wrap(text, 10);
        assert_eq!(
            lines.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(),
            vec!["the quick ", "brown fox"]
        );
    }

    #[test]
    fn wrapping_breaks_a_word_too_long_to_fit() {
        let text = "ab wwwwwwwwwwww";
        let lines = wrap(text, 6);
        // Contiguous, and no line is empty, however long the word is.
        assert!(lines.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(lines.first().unwrap().start, 0);
        assert_eq!(lines.last().unwrap().end, text.len());
        assert!(lines.iter().all(|r| !r.is_empty()));
    }

    #[test]
    fn wrapping_tiles_the_whole_text() {
        let text = "字幕は長くなることがあります。 with some latin too";
        for width in [1, 2, 3, 7, 40] {
            let lines = wrap(text, width);
            assert_eq!(lines.first().unwrap().start, 0, "width {width}");
            assert_eq!(lines.last().unwrap().end, text.len(), "width {width}");
            assert!(
                lines.windows(2).all(|w| w[0].end == w[1].start),
                "width {width} left a gap"
            );
        }
    }

    #[test]
    fn cursor_line_follows_the_cursor() {
        let mut input = TextInput::with_text("あいうえおか");
        // Cursor at the end: last line.
        assert_eq!(input.wrap(8).cursor_line, 1);
        press(&mut input, &[key(KeyCode::Home)]);
        assert_eq!(input.wrap(8).cursor_line, 0);
    }

    #[test]
    fn a_cursor_past_a_full_line_gets_a_line_of_its_own() {
        let input = TextInput::with_text("あいうえ");
        let w = input.wrap(8);
        assert_eq!(w.lines.len(), 2, "the trailing cursor needs somewhere to sit");
        assert_eq!(w.cursor_line, 1);
        assert!(w.lines[1].is_empty());
    }

    #[test]
    fn an_empty_input_still_has_a_line() {
        let input = TextInput::default();
        let w = input.wrap(10);
        assert_eq!(w.lines.len(), 1);
        assert_eq!(w.cursor_line, 0);
    }
}
