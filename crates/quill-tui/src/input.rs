//! A one-line text field: what the setup flow, search and the compose
//! header fields type into.
//!
//! The cursor moves by grapheme cluster, so an emoji with a skin tone or a
//! letter with a combining accent is one step and one deletion, as it is in
//! every other Mac text field.

use unicode_segmentation::UnicodeSegmentation;

use crate::keymap::{Key, KeyChord};
use crate::text;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// A byte offset into `text`, always on a grapheme boundary.
    cursor: usize,
    /// Draw every character as a bullet: a password.
    pub masked: bool,
}

/// A masked field holds a password, which is not printed here either: the
/// form it is part of prints whole.
impl std::fmt::Debug for LineInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut shown = f.debug_struct("LineInput");
        if self.masked {
            shown.field("text", &"…");
        } else {
            shown.field("text", &self.text);
        }
        shown
            .field("cursor", &self.cursor)
            .field("masked", &self.masked)
            .finish()
    }
}

impl LineInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            masked: false,
        }
    }

    pub fn masked() -> Self {
        Self {
            masked: true,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replaces the text and puts the cursor at its end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.set(String::new());
    }

    /// The cursor's position in grapheme clusters from the start.
    pub fn cursor(&self) -> usize {
        self.text[..self.cursor].graphemes(true).count()
    }

    /// Types `text` at the cursor. Line breaks and control characters never
    /// enter a one-line field: a pasted line break becomes a space.
    pub fn insert(&mut self, text: &str) {
        let clean: String = text
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.text.len(), |grapheme| self.cursor + grapheme.len())
    }

    /// The start of the word before the cursor, as `opt-left` finds it.
    fn previous_word(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_alphanumeric())
            .map_or(0, |(index, c)| index + c.len_utf8())
    }

    /// The end of the word after the cursor, as `opt-right` finds it.
    fn next_word(&self) -> usize {
        let after = &self.text[self.cursor..];
        let skipped = after.len()
            - after
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .len();
        let word = after[skipped..]
            .char_indices()
            .find(|(_, c)| !c.is_alphanumeric())
            .map_or(after.len() - skipped, |(index, _)| index);
        self.cursor + skipped + word
    }

    /// Applies an editing key. Returns `false` for a key that is not this
    /// field's business, so the caller can give it to a command.
    pub fn key(&mut self, chord: KeyChord) -> bool {
        let mods = chord.mods;
        let plain = !mods.ctrl && !mods.alt && !mods.cmd;
        match chord.key {
            Key::Char(c) if plain => {
                let mut buffer = [0; 4];
                self.insert(c.encode_utf8(&mut buffer));
            }
            // The readline keys every terminal user's fingers know.
            Key::Char('a') if mods.ctrl => self.cursor = 0,
            Key::Char('e') if mods.ctrl => self.cursor = self.text.len(),
            Key::Char('k') if mods.ctrl => self.text.truncate(self.cursor),
            Key::Char('u') if mods.ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            Key::Char('w') if mods.ctrl => self.delete_to(self.previous_word()),
            Key::Left if mods.cmd => self.cursor = 0,
            Key::Right if mods.cmd => self.cursor = self.text.len(),
            Key::Left if mods.alt => self.cursor = self.previous_word(),
            Key::Right if mods.alt => self.cursor = self.next_word(),
            Key::Left if plain => self.cursor = self.previous_boundary(),
            Key::Right if plain => self.cursor = self.next_boundary(),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.text.len(),
            Key::Backspace if mods.cmd => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            Key::Backspace if mods.alt => self.delete_to(self.previous_word()),
            Key::Backspace => self.delete_to(self.previous_boundary()),
            Key::Delete => {
                let end = self.next_boundary();
                self.text.drain(self.cursor..end);
            }
            _ => return false,
        }
        true
    }

    /// Deletes from `start` to the cursor.
    fn delete_to(&mut self, start: usize) {
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    /// What to draw in a field `width` cells wide, and the column the cursor
    /// sits in. The text scrolls sideways to keep the cursor in view.
    pub fn view(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let shown = |grapheme: &str| -> String {
            if self.masked {
                "•".to_owned()
            } else {
                grapheme.to_owned()
            }
        };
        let before: Vec<String> = self.text[..self.cursor]
            .graphemes(true)
            .map(shown)
            .collect();
        let after: Vec<String> = self.text[self.cursor..]
            .graphemes(true)
            .map(shown)
            .collect();

        // The cursor needs a cell of its own at the end of the text, so the
        // text before it may fill all but one.
        let room = width - 1;
        let mut start = before.len();
        let mut used = 0;
        while start > 0 {
            let w = text::width(&before[start - 1]);
            if used + w > room {
                break;
            }
            used += w;
            start -= 1;
        }
        let mut out: String = before[start..].concat();
        let column = used;
        let mut total = used;
        for grapheme in &after {
            let w = text::width(grapheme);
            if total + w > width {
                break;
            }
            out.push_str(grapheme);
            total += w;
        }
        (out, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut LineInput, keys: &str) {
        for key in keys.split_whitespace() {
            let chord: KeyChord = key.parse().unwrap();
            assert!(input.key(chord), "{key} was not taken");
        }
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut input = LineInput::default();
        press(&mut input, "a c left b");
        assert_eq!(input.text(), "abc");
        assert_eq!(input.cursor(), 2);
        input.insert("XY");
        assert_eq!(input.text(), "abXYc");
    }

    #[test]
    fn a_grapheme_cluster_is_one_step_and_one_deletion() {
        // A family emoji, a flag, and e with a combining accent.
        let mut input = LineInput::new("a👨‍👩‍👧🇫🇷e\u{301}z");
        assert_eq!(input.cursor(), 5);
        press(&mut input, "left left");
        assert_eq!(input.cursor(), 3);
        press(&mut input, "backspace");
        assert_eq!(input.text(), "a👨‍👩‍👧e\u{301}z");
        press(&mut input, "backspace");
        assert_eq!(input.text(), "ae\u{301}z");
        press(&mut input, "delete");
        assert_eq!(input.text(), "az");
        // Past either end nothing happens.
        press(&mut input, "home backspace left end delete right");
        assert_eq!(input.text(), "az");
    }

    #[test]
    fn words_and_line_ends() {
        let mut input = LineInput::new("ana.ruiz@example.com, sam lee");
        press(&mut input, "alt-left");
        assert_eq!(&input.text()[..input.cursor], "ana.ruiz@example.com, sam ");
        press(&mut input, "alt-left alt-left");
        assert_eq!(&input.text()[..input.cursor], "ana.ruiz@example.");
        press(&mut input, "alt-right");
        assert_eq!(&input.text()[..input.cursor], "ana.ruiz@example.com");
        press(&mut input, "alt-backspace");
        assert_eq!(input.text(), "ana.ruiz@example., sam lee");
        press(&mut input, "ctrl-a");
        assert_eq!(input.cursor(), 0);
        press(&mut input, "ctrl-e ctrl-w");
        assert_eq!(input.text(), "ana.ruiz@example., sam ");
        press(&mut input, "ctrl-u");
        assert_eq!(input.text(), "");
        input.set("keep this, drop that");
        press(&mut input, "home alt-right alt-right ctrl-k");
        assert_eq!(input.text(), "keep this");
        press(&mut input, "cmd-left");
        assert_eq!(input.cursor(), 0);
        press(&mut input, "cmd-right cmd-backspace");
        assert_eq!(input.text(), "");
    }

    #[test]
    fn keys_that_are_not_editing_are_left_for_commands() {
        let mut input = LineInput::new("x");
        for key in ["enter", "esc", "tab", "up", "down", "ctrl-c", "cmd-s", "f1"] {
            let chord: KeyChord = key.parse().unwrap();
            assert!(!input.key(chord), "{key}");
        }
        assert_eq!(input.text(), "x");
    }

    #[test]
    fn pasted_line_breaks_and_control_characters_do_not_get_in() {
        let mut input = LineInput::default();
        input.insert("one\ntwo\tthree\u{1b}[2J\u{7}");
        assert_eq!(input.text(), "one two three[2J");
    }

    #[test]
    fn the_view_scrolls_to_keep_the_cursor_in_sight() {
        let mut input = LineInput::new("abcdefghij");
        // Wide enough: all of it, cursor after the last letter.
        assert_eq!(input.view(20), ("abcdefghij".to_owned(), 10));
        // Too narrow: the end is shown, with a cell left for the cursor.
        assert_eq!(input.view(5), ("ghij".to_owned(), 4));
        press(&mut input, "home");
        assert_eq!(input.view(5), ("abcde".to_owned(), 0));
        press(&mut input, "right right right right right right");
        assert_eq!(input.view(5), ("cdefg".to_owned(), 4));
        assert_eq!(input.view(0), (String::new(), 0));
        // Wide characters are never cut in half.
        let wide = LineInput::new("日本語のメール");
        let (shown, column) = wide.view(7);
        assert_eq!((shown.as_str(), column), ("メール", 6));
    }

    #[test]
    fn a_password_is_drawn_as_bullets() {
        let mut input = LineInput::masked();
        press(&mut input, "s 3 c r é t");
        assert_eq!(input.text(), "s3crét");
        assert_eq!(input.view(20), ("••••••".to_owned(), 6));
        // Nor is it printed with whatever holds the field.
        assert!(!format!("{input:?}").contains("s3crét"));
        assert!(!format!("{input:#?}").contains("s3crét"));
        assert!(format!("{:?}", LineInput::new("robin@example.com")).contains("robin@example.com"));
    }
}
