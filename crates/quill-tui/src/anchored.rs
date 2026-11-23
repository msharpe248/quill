//! Drawing that does not trust a terminal to agree about a character's
//! width.
//!
//! A frame is sent as runs of cells: the cursor is put at the first cell
//! of a run and the rest follow, each taken to land one cell on from the
//! last. That holds only while the terminal gives every character the
//! width the frame was laid out with, and for some characters terminals do
//! not agree: a soft hyphen is nothing to one and a cell to another, half
//! a flag is one cell or two. Where they differ, the rest of the run lands
//! a cell out, and what was drawn out of place is never cleared, because
//! the frame does not know it is there: letters are left behind when the
//! text they belonged to has gone.
//!
//! So a run ends after any cell whose width is not beyond doubt, and the
//! cursor is put where the next cell belongs. A disagreement then costs
//! the one cell it is about and nothing after it.

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// A backend whose runs end after every cell a terminal might draw wider
/// or narrower than the frame has it.
#[derive(Debug)]
pub struct Anchored<B> {
    inner: B,
}

impl<B> Anchored<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }

    /// The backend the cells are sent to.
    pub fn inner(&self) -> &B {
        &self.inner
    }
}

/// Whether every terminal draws `symbol` in exactly one cell: one
/// character, of the kinds no width table has two minds about. ASCII, the
/// Latin letters, the common punctuation, and the lines and shapes panes
/// are drawn with. The soft hyphen is among the Latin ones and is not
/// plain.
fn is_plain(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => matches!(
            c,
            ' '..='~'
                | '\u{a0}'..='\u{ac}'
                | '\u{ae}'..='\u{24f}'
                | '\u{2010}'..='\u{2027}'
                | '\u{2030}'..='\u{203a}'
                | '\u{2500}'..='\u{25cf}'
        ),
        _ => false,
    }
}

impl<B: Backend> Backend for Anchored<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Each call to the backend under this one starts by putting the
        // cursor at its first cell, so a run that must end is a call that
        // ends.
        let cells: Vec<(u16, u16, &Cell)> = content.collect();
        let mut rest = cells.as_slice();
        while !rest.is_empty() {
            let end = rest
                .iter()
                .position(|(_, _, cell)| !is_plain(cell.symbol()))
                .map_or(rest.len(), |at| at + 1);
            let (run, after) = rest.split_at(end);
            self.inner.draw(run.iter().copied())?;
            rest = after;
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use ratatui::Terminal;
    use ratatui::text::Line;
    use unicode_width::UnicodeWidthChar;

    use super::*;

    const COLUMNS: u16 = 24;

    /// A terminal with widths of its own, as real ones have: a soft hyphen
    /// takes a cell and half a flag takes two, whatever the frame was laid
    /// out with. Cells are sent to it as the real backend sends them: the
    /// cursor is moved for the first of a call and for any that does not
    /// follow the one before, and is otherwise wherever the last
    /// character left it.
    #[derive(Debug, Default)]
    struct Opinionated {
        /// One row: what each cell shows. The second cell of a wide
        /// character is `None`.
        row: Vec<Option<String>>,
    }

    impl Opinionated {
        fn new() -> Self {
            Self {
                row: vec![Some(" ".to_owned()); usize::from(COLUMNS)],
            }
        }

        fn width(c: char) -> usize {
            match c {
                '\u{ad}' => 1,
                '\u{1f1e6}'..='\u{1f1ff}' => 2,
                c => c.width().unwrap_or(0),
            }
        }

        /// A character over either cell of a wide one takes the whole of
        /// it away.
        fn clear_wide_at(&mut self, at: usize) {
            if self.row.get(at).is_some_and(Option::is_none) && at > 0 {
                self.row[at - 1] = Some(" ".to_owned());
                self.row[at] = Some(" ".to_owned());
            }
            if self.row.get(at + 1).is_some_and(Option::is_none) {
                self.row[at + 1] = Some(" ".to_owned());
            }
        }

        fn print(&mut self, cursor: &mut usize, c: char) {
            let width = Self::width(c);
            if width == 0 {
                if let Some(Some(cell)) = cursor.checked_sub(1).and_then(|at| self.row.get_mut(at))
                {
                    cell.push(c);
                }
                return;
            }
            if *cursor + width > self.row.len() {
                return;
            }
            self.clear_wide_at(*cursor);
            self.row[*cursor] = Some(c.to_string());
            if width == 2 {
                self.clear_wide_at(*cursor + 1);
                self.row[*cursor + 1] = None;
            }
            *cursor += width;
        }

        /// What the row shows, with nothing for a cell that shows nothing.
        fn shown(&self) -> String {
            let text: String = self.row.iter().flatten().map(String::as_str).collect();
            text.trim_end().to_owned()
        }
    }

    impl Backend for Opinionated {
        type Error = Infallible;

        fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            let mut last: Option<u16> = None;
            let mut cursor = 0;
            for (x, _, cell) in content {
                if last.is_none_or(|last| x != last + 1) {
                    cursor = usize::from(x);
                }
                last = Some(x);
                for c in cell.symbol().chars() {
                    self.print(&mut cursor, c);
                }
            }
            Ok(())
        }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
            Ok(Position::ORIGIN)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
            Ok(())
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            *self = Self::new();
            Ok(())
        }

        fn clear_region(&mut self, _: ClearType) -> Result<(), Self::Error> {
            self.clear()
        }

        fn size(&self) -> Result<Size, Self::Error> {
            Ok(Size::new(COLUMNS, 1))
        }

        fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
            Ok(WindowSize {
                columns_rows: Size::new(COLUMNS, 1),
                pixels: Size::new(0, 0),
            })
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Draws `first` and then `second` on one row, and says what the
    /// terminal shows after each.
    fn drawn<B: Backend>(
        backend: B,
        first: &str,
        second: &str,
        shown: impl Fn(&B) -> String,
    ) -> (String, String) {
        let mut terminal = Terminal::new(backend).unwrap();
        let mut after = Vec::new();
        for text in [first, second] {
            terminal
                .draw(|frame| frame.render_widget(Line::raw(text), frame.area()))
                .unwrap();
            after.push(shown(terminal.backend()));
        }
        (after.remove(0), after.remove(0))
    }

    const SOFT_HYPHEN: &str = "ab a\u{ad}b, cd ef";
    const HALF_A_FLAG: &str = "ab \u{1f1fa}, cd ef";

    #[test]
    fn a_terminal_with_its_own_widths_keeps_letters_of_text_that_has_gone() {
        // What this module is for, shown without it: the comma is left
        // where the terminal put it, a cell on from where the frame has it.
        for text in [SOFT_HYPHEN, HALF_A_FLAG] {
            let (_, after) = drawn(Opinionated::new(), text, "", Opinionated::shown);
            assert!(after.contains(','), "{text:?} left {after:?}");
        }
    }

    #[test]
    fn nothing_is_left_of_text_that_has_gone_whatever_widths_the_terminal_has() {
        for text in [SOFT_HYPHEN, HALF_A_FLAG] {
            let (_, after) = drawn(Anchored::new(Opinionated::new()), text, "", |backend| {
                backend.inner().shown()
            });
            assert_eq!(after, "", "{text:?}");
        }
    }

    #[test]
    fn the_text_after_a_doubtful_character_is_where_the_frame_has_it() {
        let (first, _) = drawn(
            Anchored::new(Opinionated::new()),
            SOFT_HYPHEN,
            "",
            |backend| backend.inner().shown(),
        );
        // The soft hyphen is with the letter before it, where the frame put
        // it, and every letter after is in its own cell.
        assert_eq!(first.replace('\u{ad}', ""), "ab ab, cd ef");

        let (first, second) = drawn(
            Anchored::new(Opinionated::new()),
            "ab \u{1f1fa} cd",
            "ab xy cd",
            |backend| backend.inner().shown(),
        );
        assert_eq!(first, "ab \u{1f1fa}cd");
        assert_eq!(second, "ab xy cd");
    }

    #[test]
    fn plain_text_is_sent_as_it_always_was() {
        /// Counts the calls made to it: each is a run.
        #[derive(Debug, Default)]
        struct Counting {
            runs: usize,
            under: Opinionated,
        }
        impl Backend for Counting {
            type Error = Infallible;
            fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
            where
                I: Iterator<Item = (u16, u16, &'a Cell)>,
            {
                self.runs += 1;
                self.under.draw(content)
            }
            fn hide_cursor(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn show_cursor(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
                Ok(Position::ORIGIN)
            }
            fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
                Ok(())
            }
            fn clear(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
            fn clear_region(&mut self, _: ClearType) -> Result<(), Self::Error> {
                Ok(())
            }
            fn size(&self) -> Result<Size, Self::Error> {
                Ok(Size::new(COLUMNS, 1))
            }
            fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
                self.under.window_size()
            }
            fn flush(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        let runs = |text: &str| {
            let under = Counting {
                runs: 0,
                under: Opinionated::new(),
            };
            let mut terminal = Terminal::new(Anchored::new(under)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(Line::raw(text), frame.area()))
                .unwrap();
            terminal.backend().inner().runs
        };
        assert_eq!(runs("│ Caf\u{e9} \u{2026} \u{25be} ok │"), 1);
        // One run up to and with the doubtful cell, one for what follows.
        assert_eq!(runs("ab \u{2764} cd"), 2);
    }

    #[test]
    fn only_single_characters_everyone_agrees_about_are_plain() {
        for plain in ["a", " ", "~", "\u{e9}", "\u{2026}", "\u{2502}", "\u{25be}"] {
            assert!(is_plain(plain), "{plain:?}");
        }
        for doubtful in [
            "",
            "\u{ad}",
            "a\u{ad}",
            "e\u{301}",
            "\u{2764}",
            "\u{1f1fa}",
            "\u{65e5}",
            "\u{203c}",
            "\u{25fe}",
        ] {
            assert!(!is_plain(doubtful), "{doubtful:?}");
        }
    }
}
