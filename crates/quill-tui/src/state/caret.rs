//! Selecting the words of the message on show with the keys.
//!
//! A cursor is put on the text ([`Caret`], `text.select`) and moved with
//! the keys that move a cursor anywhere: the arrows, and `h` `j` `k` `l`,
//! `w` and `b` for a word, `0` and `$` for the ends of a row, `g` and `G`
//! for the ends of the text, and a page at a time. Until it is told where
//! the selection begins it is one cell that goes where it is taken;
//! `text.select` again begins the selection there, and from then on the
//! cursor takes the selection's other end with it. It is the selection
//! the mouse makes ([`Selection`]), copied by the same commands, and it
//! reaches from the rows above the text into the text as that does.
//!
//! The keys that move it are keys and not commands, as in a field that is
//! typed in: they go to the cursor first, and only what it has no use for
//! is looked up as a command.

use super::select::{HeadRow, Part, Selection, Spot};
use super::{Effects, Env, State};
use crate::keymap::{Key, KeyChord, Mods};
use crate::{layout, text};

/// The cursor that selects with the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    /// Where the selection begins has been said: the cursor now takes
    /// its other end. Before that the cursor is all there is of it.
    pub anchored: bool,
    /// The cell it keeps to as it goes up and down: where it was last
    /// taken along a row, however short the rows it has crossed since.
    pub wanted: usize,
}

/// One move of the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Left,
    Right,
    Up,
    Down,
    WordBack,
    WordOn,
    RowStart,
    RowEnd,
    Top,
    Bottom,
    PageUp,
    PageDown,
}

/// The move a key makes, if it makes one.
fn step_of(chord: KeyChord) -> Option<Step> {
    // With shift an arrow still moves: on a Mac that is how a selection
    // is stretched.
    let shift_only = Mods {
        shift: true,
        ..Mods::default()
    };
    if chord.mods != Mods::default() && chord.mods != shift_only {
        return None;
    }
    Some(match chord.key {
        Key::Left | Key::Char('h') => Step::Left,
        Key::Right | Key::Char('l') => Step::Right,
        Key::Up | Key::Char('k') => Step::Up,
        Key::Down | Key::Char('j') => Step::Down,
        Key::Char('b') => Step::WordBack,
        Key::Char('w') => Step::WordOn,
        Key::Home | Key::Char('0') => Step::RowStart,
        Key::End | Key::Char('$') => Step::RowEnd,
        Key::Char('g') => Step::Top,
        Key::Char('G') => Step::Bottom,
        Key::PageUp => Step::PageUp,
        Key::PageDown | Key::Char(' ') => Step::PageDown,
        _ => return None,
    })
}

/// The cells of a row that hold something: where each begins, and
/// whether it is a space.
fn cells_of(words: &str) -> Vec<(usize, bool)> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut cells = Vec::new();
    let mut used = 0;
    for grapheme in words.graphemes(true) {
        let wide = text::width(grapheme);
        if wide > 0 {
            cells.push((used, grapheme.chars().all(char::is_whitespace)));
        }
        used += wide;
    }
    cells
}

impl State {
    /// Whether the cursor that selects with the keys is on the text.
    pub fn selecting_with_keys(&self) -> bool {
        self.reader.caret.is_some() && matches!(self.reader.selection, Some(Selection::Some { .. }))
    }

    /// Whether the selection's beginning has been said.
    pub fn selection_begun(&self) -> bool {
        self.reader.caret.is_some_and(|caret| caret.anchored)
    }

    /// Whether there is text on show for a cursor to go on.
    pub fn can_select_with_keys(&self, env: &Env) -> bool {
        self.reader_text_on_show(env) && !self.reader.lines.is_empty()
    }

    /// `text.select`. With no cursor on the text it puts one there, at
    /// the first line on show. With one, it begins the selection where
    /// the cursor is, or, if it has begun, lets go of it and leaves the
    /// cursor.
    pub(super) fn select_with_keys(&mut self, env: &Env) {
        if !self.can_select_with_keys(env) {
            return;
        }
        match (self.reader.caret, self.reader.selection) {
            (Some(caret), Some(Selection::Some { head, .. })) => {
                self.reader.caret = Some(Caret {
                    anchored: !caret.anchored,
                    ..caret
                });
                if caret.anchored {
                    self.reader.selection = Some(Selection::Some { anchor: head, head });
                }
            }
            _ => {
                let at = Spot {
                    part: Part::Body,
                    row: self.reader.scroll.min(self.reader.lines.len() - 1),
                    column: 0,
                };
                self.reader.selection = Some(Selection::Some {
                    anchor: at,
                    head: at,
                });
                self.reader.caret = Some(Caret {
                    anchored: false,
                    wanted: 0,
                });
            }
        }
    }

    /// Takes the cursor off the text. What was selected goes with it
    /// unless it is `kept`, as it is when it has just been copied.
    pub(super) fn stop_selecting_with_keys(&mut self, kept: bool) {
        self.reader.caret = None;
        if !kept {
            self.reader.selection = None;
        }
    }

    /// Copies what the cursor has selected and takes the cursor off.
    pub(super) fn copy_with_keys(&mut self, env: &Env, out: &mut Effects) {
        if self.copy_selection(env, out) {
            self.stop_selecting_with_keys(true);
        }
    }

    /// The rows above the text as they are drawn, and how wide the pane
    /// is for text.
    fn head_rows_drawn(&self, env: &Env) -> Vec<String> {
        let Some(rect) = layout::compute(self, self.size, env.reader_width).reader else {
            return Vec::new();
        };
        self.reader_head(env, layout::reader_text_width(rect))
            .iter()
            .map(HeadRow::text)
            .collect()
    }

    /// Gives a key to the cursor. Returns whether it was a key that moves
    /// it.
    pub(super) fn caret_key(&mut self, env: &Env, chord: KeyChord) -> bool {
        let (Some(caret), Some(Selection::Some { anchor, head })) =
            (self.reader.caret, self.reader.selection)
        else {
            return false;
        };
        let Some(step) = step_of(chord) else {
            return false;
        };
        let head_rows = self.head_rows_drawn(env);
        let lines = self.reader.lines.len();
        if lines == 0 {
            return false;
        }
        let page = layout::compute(self, self.size, env.reader_width)
            .reader
            .map_or(1, |rect| {
                layout::reader_body_height(
                    rect,
                    self.reader_header_rows(),
                    self.reader_image_rows(env, rect),
                )
            })
            .max(1);
        let words = |spot: Spot| -> String {
            match spot.part {
                Part::Head => head_rows.get(spot.row).cloned().unwrap_or_default(),
                Part::Body => self
                    .reader
                    .lines
                    .get(spot.row)
                    .map(|line| line.spans.iter().map(|span| span.text.as_str()).collect())
                    .unwrap_or_default(),
            }
        };
        // The rows in the order they are read, and a row's place in it.
        let count = head_rows.len() + lines;
        let place = |spot: Spot| match spot.part {
            Part::Head => spot.row.min(head_rows.len().saturating_sub(1)),
            Part::Body => head_rows.len() + spot.row,
        };
        let at = |place: usize, column: usize| {
            if place < head_rows.len() {
                Spot {
                    part: Part::Head,
                    row: place,
                    column,
                }
            } else {
                Spot {
                    part: Part::Body,
                    row: place - head_rows.len(),
                    column,
                }
            }
        };
        // The cell of a row nearest to `column` that holds something, or
        // its first cell when it holds nothing.
        let settle = |spot: Spot| {
            let cells = cells_of(&words(spot));
            let column = cells
                .iter()
                .map(|(start, _)| *start)
                .take_while(|start| *start <= spot.column)
                .last()
                .unwrap_or(0);
            Spot { column, ..spot }
        };
        let here = place(head);
        let first_line = head_rows.len();
        let moved = match step {
            Step::Up => settle(at(here.saturating_sub(1), caret.wanted)),
            Step::Down => settle(at((here + 1).min(count - 1), caret.wanted)),
            Step::PageUp => settle(at(
                here.saturating_sub(page).max(first_line.min(here)),
                caret.wanted,
            )),
            Step::PageDown => settle(at((here + page).min(count - 1), caret.wanted)),
            Step::Top => at(first_line, 0),
            Step::Bottom => at(count - 1, 0),
            Step::RowStart => Spot { column: 0, ..head },
            Step::RowEnd => settle(Spot {
                column: usize::MAX / 4,
                ..head
            }),
            Step::Left | Step::Right | Step::WordBack | Step::WordOn => {
                let forward = matches!(step, Step::Right | Step::WordOn);
                let by_word = matches!(step, Step::WordBack | Step::WordOn);
                let mut row = here;
                let mut cells = cells_of(&words(head));
                // Where the cursor is among the cells of its row.
                let mut index = cells
                    .iter()
                    .rposition(|(start, _)| *start <= head.column)
                    .map_or(0, |index| index as isize);
                let mut found = None;
                // The cell before or after; for a word, the nearest cell
                // that way that begins one. Past the end of a row, on to
                // the next row; an empty row is a place to stop.
                for _ in 0..10_000 {
                    index += if forward { 1 } else { -1 };
                    if index < 0 || index as usize >= cells.len() {
                        let next = if forward {
                            row + 1
                        } else {
                            row.wrapping_sub(1)
                        };
                        if next >= count {
                            break;
                        }
                        row = next;
                        cells = cells_of(&words(at(row, 0)));
                        if cells.is_empty() {
                            found = Some((row, 0));
                            break;
                        }
                        index = if forward { -1 } else { cells.len() as isize };
                        continue;
                    }
                    let (start, space) = cells[index as usize];
                    let begins = !space && (index == 0 || cells[index as usize - 1].1);
                    if !by_word || begins {
                        found = Some((row, start));
                        break;
                    }
                }
                found.map_or(head, |(row, column)| at(row, column))
            }
        };
        self.reader.selection = Some(Selection::Some {
            anchor: if caret.anchored { anchor } else { moved },
            head: moved,
        });
        // Up and down keep to the cell the cursor was last taken to; the
        // end of a row is kept to as the end of every row.
        let wanted = match step {
            Step::Up | Step::Down | Step::PageUp | Step::PageDown => caret.wanted,
            Step::RowEnd => usize::MAX / 4,
            _ => moved.column,
        };
        self.reader.caret = Some(Caret { wanted, ..caret });
        // The text follows the cursor; the rows above it are always on
        // show.
        if moved.part == Part::Body {
            if moved.row < self.reader.scroll {
                self.reader.scroll = moved.row;
            } else if moved.row >= self.reader.scroll + page {
                self.reader.scroll = moved.row + 1 - page;
            }
        }
        true
    }
}
