//! The built-in editor: the multi-line text area a message body is written
//! in. It is [`LineInput`](crate::input::LineInput)'s bigger sibling, with
//! the same key conventions and the same grapheme handling.
//!
//! The editor knows nothing about ratatui. [`BuiltinEditor::view`] returns
//! rows of plain spans and a cursor position, and a view function elsewhere
//! turns them into cells. Compose logic talks to the [`Editor`] trait, so a
//! second implementation (a bundled Vim, later) can stand in.
//!
//! How it is put together:
//!
//! - The text is a list of hard lines (paragraphs). Each caches where it
//!   soft-wraps, so an edit re-wraps one line and a megabyte paste is wrapped
//!   once, not on every keystroke.
//! - Text wraps one cell short of the view's width. The last column is kept
//!   for the cursor, which needs somewhere to sit after a full row.
//! - A space at a wrap stays on the upper row and may hang past the wrap
//!   width, as it does in a Mac text view. Rows are cut to the view's width
//!   when drawn, and a cursor among hanging spaces is drawn in the last
//!   column.
//! - Undo records the text each step removed and inserted, never a copy of
//!   the document.
//! - Markdown highlighting is worked out when a row is drawn and never
//!   touches the text.

use std::borrow::Cow;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::keymap::{Key, KeyChord, Mods};
use crate::text;

/// How far `tab` indents: to the next multiple of this many cells.
const TAB_STOP: usize = 4;

/// What compose logic needs from an editor, so a second implementation (a
/// bundled Vim, later) can stand in.
pub trait Editor {
    /// The whole text, lines joined by `\n`.
    fn text(&self) -> String;

    /// Loads a new text. The cursor goes to the start, the undo history is
    /// dropped, and the editor counts as unmodified.
    fn set_text(&mut self, text: &str);

    /// Puts the cursor at a byte offset into [`Editor::text`], moved back to
    /// the nearest grapheme boundary. An offset past the end means the end.
    fn set_cursor_offset(&mut self, byte_offset: usize);

    /// Typed or pasted text at the cursor, replacing the selection. Line
    /// ends of every kind become `\n`, a tab becomes spaces, and other
    /// control characters are dropped.
    fn insert(&mut self, text: &str);

    /// Applies a key. [`Outcome::Ignored`] means the key is not the
    /// editor's business and the caller may give it to a command.
    fn key(&mut self, chord: KeyChord) -> Outcome;

    /// Whether the text differs from what it was at the last
    /// [`Editor::mark_saved`] (or the last [`Editor::set_text`]).
    fn is_modified(&self) -> bool;

    /// Records that the text as it is now has been saved. The undo history
    /// is kept.
    fn mark_saved(&mut self);
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not an editing key. Nothing changed.
    Ignored,
    Handled,
    /// Handled, and this text belongs on the system clipboard.
    Copy(String),
}

/// What to draw: the visible rows, top to bottom.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorView {
    /// Exactly the visible rows. Fewer than the height when the text is
    /// shorter than the area.
    pub lines: Vec<ViewLine>,
    /// Column and row of the cursor within the area. `None` when the view
    /// has been scrolled away from it, or the area has no cells.
    pub cursor: Option<(u16, u16)>,
    /// How many rows the whole text takes at this width, for a scroll
    /// indicator.
    pub total_rows: usize,
    /// Which of those rows is the first one shown.
    pub first_row: usize,
}

/// One row of the view. Its spans never add up to more than the view's
/// width.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewLine {
    pub spans: Vec<ViewSpan>,
}

/// A run of text drawn one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSpan {
    /// The text to draw. One span on a row may be a single space that is not
    /// in the document: it stands for a selected line break, so that a
    /// selection across empty lines can be seen.
    pub text: String,
    pub selected: bool,
    /// Flagged by the spell checker. This is a flag, not a [`SpanKind`], so
    /// that a misspelled word in a quotation keeps the quotation's colour
    /// under its underline.
    pub misspelled: bool,
    pub kind: SpanKind,
}

/// What a span is, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Text,
    /// A quoted line, with its nesting level counted from 1.
    Quote(u8),
    Heading,
    Emphasis,
    Strong,
    /// A code span, or a line of a fenced code block.
    Code,
    /// A Markdown link or a bare URL.
    Link,
}

/// A place in the text: a hard line and a byte offset into it, always on a
/// grapheme boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Pos {
    line: usize,
    col: usize,
}

/// One hard line of the text, with what is cached about it.
#[derive(Debug, Clone, Default)]
struct Line {
    text: String,
    /// The byte offsets where the second and later visual rows start. Empty
    /// (and so not allocated) for a line that fits on one row.
    breaks: Vec<usize>,
    /// The wrap width `breaks` was worked out for. Zero means not yet.
    wrapped_for: usize,
    /// The character of the code fence this line opens or closes, or zero.
    fence: u8,
    /// Misspelled byte ranges within this line: sorted, never overlapping.
    misspelled: Vec<Range<usize>>,
}

#[cfg(test)]
thread_local! {
    /// How many bytes of text have been wrapped on this thread, so a test
    /// can prove that typing does not re-wrap the document.
    static WRAPPED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Line {
    fn new(text: String) -> Self {
        Self {
            fence: fence_of(&text),
            text,
            ..Self::default()
        }
    }

    /// Call after every change to `text`: forgets the cached wrapping.
    fn changed(&mut self) {
        self.breaks.clear();
        self.wrapped_for = 0;
        self.fence = fence_of(&self.text);
    }

    /// Makes sure `breaks` is right for rows of `max` cells.
    fn wrap(&mut self, max: usize) {
        if self.wrapped_for != max {
            #[cfg(test)]
            WRAPPED_BYTES.with(|bytes| bytes.set(bytes.get() + self.text.len()));
            self.breaks = wrap_breaks(&self.text, max);
            self.wrapped_for = max;
        }
    }

    fn rows(&self) -> usize {
        self.breaks.len() + 1
    }

    /// The bytes of one visual row.
    fn row_range(&self, row: usize) -> Range<usize> {
        let len = self.text.len();
        let start = match row {
            0 => 0,
            _ => self.breaks.get(row - 1).copied().unwrap_or(len),
        };
        let end = self.breaks.get(row).copied().unwrap_or(len);
        start..end
    }

    /// The visual row and the cell within it where a cursor at `col` is
    /// drawn. An offset where one row ends and the next begins is drawn at
    /// the start of the lower row, unless `upstream` asks for the end of the
    /// upper one.
    fn locate(&self, col: usize, upstream: bool) -> (usize, usize) {
        let mut row = self.breaks.partition_point(|start| *start <= col);
        if upstream && row > 0 && self.breaks.get(row - 1) == Some(&col) {
            row -= 1;
        }
        let start = self.row_range(row).start;
        let x = self.text.get(start..col).map_or(0, text::width);
        (row, x)
    }

    /// The offset of the grapheme drawn at cell `x` of a row: the cursor
    /// goes in front of the grapheme under the pointer. Past the end of the
    /// row it is the row's end, and the flag says the cursor belongs at the
    /// end of this row, not the start of the next.
    fn col_at(&self, row: usize, x: usize) -> (usize, bool) {
        let row = row.min(self.rows() - 1);
        let range = self.row_range(row);
        let mut used = 0;
        let shown = self.text.get(range.clone()).unwrap_or("");
        for (offset, grapheme) in shown.grapheme_indices(true) {
            let w = text::width(grapheme);
            // The second test is for graphemes with no width (a stray
            // combining mark): the cursor goes in front of them too.
            if used + w > x || used == x {
                return (range.start + offset, false);
            }
            used += w;
        }
        (range.end, row + 1 < self.rows())
    }
}

/// Where the second and later visual rows of a hard line start when it is
/// soft-wrapped to `max` cells.
///
/// Rows break after spaces, and inside a word only when the word alone is
/// wider than a row. A space never starts a new row: spaces at a wrap stay
/// on the upper row, hanging past `max` if they must. Every byte of the line
/// belongs to exactly one row, and a grapheme cluster is never split.
fn wrap_breaks(line: &str, max: usize) -> Vec<usize> {
    let mut breaks = Vec::new();
    // Nothing but letters, digits and punctuation one cell wide, and few
    // enough of them: the common case, and a megabyte paste is mostly this.
    if line.len() <= max && line.is_ascii() {
        return breaks;
    }
    let mut row_start = 0;
    let mut used = 0;
    // Where the word being read began, and how many cells of it are on this
    // row. A row can break before a word only if the word did not start it.
    let mut word_start = 0;
    let mut word_used = 0;
    for (index, grapheme) in line.grapheme_indices(true) {
        if grapheme == " " {
            used += 1;
            word_start = index + 1;
            word_used = 0;
            continue;
        }
        // A single byte is printable ASCII: control characters never get in.
        let w = if grapheme.len() == 1 {
            1
        } else {
            text::width(grapheme)
        };
        if used + w > max && index > row_start {
            if word_start > row_start {
                breaks.push(word_start);
                row_start = word_start;
                used = word_used;
            }
            // No space to break at, or the word alone is still too wide.
            if used + w > max && index > row_start {
                breaks.push(index);
                row_start = index;
                used = 0;
                word_start = index;
                word_used = 0;
            }
        }
        used += w;
        word_used += w;
    }
    breaks
}

/// Which fence character a line opens or closes a code block with, or zero.
fn fence_of(line: &str) -> u8 {
    let line = line.trim_start_matches(' ');
    if line.starts_with("```") {
        b'`'
    } else if line.starts_with("~~~") {
        b'~'
    } else {
        0
    }
}

/// The fence state after a line: which fence character an open code block
/// is waiting for, or zero outside one.
fn fence_after(open: u8, line: u8) -> u8 {
    if line == 0 {
        open
    } else if open == 0 {
        line
    } else if open == line {
        0
    } else {
        open
    }
}

/// Makes typed or pasted text fit to enter the document. Every kind of line
/// ending becomes `\n`, a tab becomes spaces, and every other control
/// character is dropped.
fn clean(text: &str) -> Cow<'_, str> {
    let dirty = |c: char| (c.is_control() && c != '\n') || matches!(c, '\u{2028}' | '\u{2029}');
    if !text.contains(dirty) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Windows line ends, and the lone carriage returns some terminals
            // send for the line breaks of a paste.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// The nearest grapheme boundary at or before `at`.
fn floor_boundary(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut at = at;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    if GraphemeCursor::new(at, text.len(), true).is_boundary(text, 0) == Ok(true) {
        return at;
    }
    GraphemeCursor::new(at, text.len(), true)
        .prev_boundary(text, 0)
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// The nearest grapheme boundary at or after `at`.
fn ceil_boundary(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut at = at;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    if GraphemeCursor::new(at, text.len(), true).is_boundary(text, 0) == Ok(true) {
        return at;
    }
    GraphemeCursor::new(at, text.len(), true)
        .next_boundary(text, 0)
        .ok()
        .flatten()
        .unwrap_or(text.len())
}

/// The largest char boundary at or before `at`.
fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Where text that starts at `at` ends.
fn end_of(at: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos {
            line: at.line,
            col: at.col + text.len(),
        },
        Some(last) => Pos {
            line: at.line + text.bytes().filter(|byte| *byte == b'\n').count(),
            col: text.len() - last - 1,
        },
    }
}

fn is_one_grapheme(text: &str) -> bool {
    let mut graphemes = text.graphemes(true);
    graphemes.next().is_some() && graphemes.next().is_none()
}

/// The quote prefix of a line: how many `>` it starts with, and where its
/// own text begins. `> > text` and `>> text` are both level 2.
fn quote_prefix(line: &str) -> (u8, usize) {
    let bytes = line.as_bytes();
    let mut level: u8 = 0;
    let mut at = 0;
    while bytes.get(at) == Some(&b'>') {
        level = level.saturating_add(1);
        at += 1;
        if bytes.get(at) == Some(&b' ') {
            at += 1;
        }
    }
    (level, at)
}

/// A line's own text: what follows its quote prefix.
fn own_text(line: &str) -> &str {
    line.get(quote_prefix(line).1..).unwrap_or("")
}

/// A stretch of a line to draw as something other than plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mark {
    range: Range<usize>,
    kind: SpanKind,
}

/// The Markdown in one hard line: sorted marks that do not overlap. `fenced`
/// says the line is inside a code block or is one of its fences.
///
/// This is deliberately not a CommonMark parser. A quotation, a heading and
/// a fenced line are one mark each; anything else is scanned once, left to
/// right, for code spans, strong, emphasis, links and bare URLs, and marks
/// do not nest.
fn marks(line: &str, fenced: bool) -> Vec<Mark> {
    let whole = |kind| {
        vec![Mark {
            range: 0..line.len(),
            kind,
        }]
    };
    if fenced {
        return whole(SpanKind::Code);
    }
    let (level, _) = quote_prefix(line);
    if level > 0 {
        return whole(SpanKind::Quote(level));
    }
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) && matches!(line.as_bytes().get(hashes), None | Some(b' ')) {
        return whole(SpanKind::Heading);
    }
    inline_marks(line)
}

/// Finds the next closing delimiter, remembering the last answer. A line of
/// a thousand unmatched asterisks is then still read once, not a thousand
/// times.
#[derive(Default)]
struct Seek {
    from: usize,
    found: Option<usize>,
    known: bool,
}

impl Seek {
    /// The first index in `at..len` that passes `test`.
    fn find(&mut self, at: usize, len: usize, test: impl Fn(usize) -> bool) -> Option<usize> {
        if self.known && at >= self.from && self.found.is_none_or(|found| found >= at) {
            return self.found;
        }
        let found = (at..len).find(|index| test(*index));
        *self = Self {
            from: at,
            found,
            known: true,
        };
        found
    }
}

fn inline_marks(line: &str) -> Vec<Mark> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let is = |index: usize, byte: u8| bytes.get(index) == Some(&byte);
    // Every delimiter is ASCII, so each index compared here is a char
    // boundary and the slices below are safe.
    let alphanumeric_before = |index: usize| {
        line[..index]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    };
    let alphanumeric_after = |index: usize| {
        line[index..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
    };
    let mut found = Vec::new();
    let (mut ticks, mut strongs, mut stars) = (Seek::default(), Seek::default(), Seek::default());
    let (mut unders, mut brackets, mut parens) =
        (Seek::default(), Seek::default(), Seek::default());
    let mut at = 0;
    while at < len {
        // The opener must be followed by text, and the closer preceded by it,
        // so "2 * 3 * 4" and a "* " bullet are left alone.
        let opens = at + 1 < len && !is(at + 1, b' ');
        let mark = match bytes[at] {
            b'`' => ticks
                .find(at + 2, len, |index| is(index, b'`'))
                .map(|close| (close + 1, SpanKind::Code)),
            b'*' if is(at + 1, b'*') => {
                if at + 2 < len && !is(at + 2, b' ') {
                    strongs
                        .find(at + 3, len, |index| {
                            is(index, b'*') && is(index + 1, b'*') && !is(index - 1, b' ')
                        })
                        .map(|close| (close + 2, SpanKind::Strong))
                } else {
                    None
                }
            }
            b'*' if opens => stars
                .find(at + 2, len, |index| {
                    is(index, b'*')
                        && !is(index - 1, b' ')
                        && !is(index - 1, b'*')
                        && !is(index + 1, b'*')
                })
                .map(|close| (close + 1, SpanKind::Emphasis)),
            // An underscore inside a word is part of a name, not emphasis.
            b'_' if opens && !is(at + 1, b'_') && !alphanumeric_before(at) => unders
                .find(at + 2, len, |index| {
                    is(index, b'_') && !is(index - 1, b' ') && !alphanumeric_after(index + 1)
                })
                .map(|close| (close + 1, SpanKind::Emphasis)),
            b'[' => brackets
                .find(at + 1, len, |index| is(index, b']'))
                .filter(|close| is(close + 1, b'('))
                .and_then(|close| parens.find(close + 2, len, |index| is(index, b')')))
                .map(|close| (close + 1, SpanKind::Link)),
            b'h' | b'm' if !alphanumeric_before(at) => {
                url_end(line, at).map(|end| (end, SpanKind::Link))
            }
            _ => None,
        };
        match mark {
            Some((end, kind)) => {
                found.push(Mark {
                    range: at..end,
                    kind,
                });
                at = end;
            }
            None => at += 1,
        }
    }
    found
}

/// Where a bare URL starting at `start` ends, if one starts there.
fn url_end(line: &str, start: usize) -> Option<usize> {
    let rest = line.get(start..)?;
    let scheme = ["https://", "http://", "mailto:"]
        .into_iter()
        .find(|scheme| rest.starts_with(scheme))?;
    let stop = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
        .unwrap_or(rest.len());
    // Punctuation after a URL belongs to the sentence around it.
    let url = rest[..stop].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'', '*']);
    (url.len() > scheme.len()).then_some(start + url.len())
}

/// The kind of the mark covering byte `at`.
fn kind_at(marks: &[Mark], at: usize) -> SpanKind {
    let index = marks.partition_point(|mark| mark.range.end <= at);
    match marks.get(index) {
        Some(mark) if mark.range.start <= at => mark.kind,
        _ => SpanKind::Text,
    }
}

/// Whether one of `ranges` (sorted, not overlapping) covers byte `at`.
fn covers(ranges: &[Range<usize>], at: usize) -> bool {
    let index = ranges.partition_point(|range| range.end <= at);
    ranges.get(index).is_some_and(|range| range.start <= at)
}

/// One visual row as spans, cut to `width` cells. `chosen` is the selected
/// part of the hard line and whether the selection runs on past its end.
fn view_row(
    line: &Line,
    row: usize,
    width: usize,
    marks: &[Mark],
    chosen: Option<&(Range<usize>, bool)>,
) -> ViewLine {
    let range = line.row_range(row);
    let shown = text::clip(line.text.get(range.clone()).unwrap_or(""), width);
    let (start, end) = (range.start, range.start + shown.len());

    // Every place within the row where the way of drawing may change.
    let mut cuts = vec![start, end];
    let mut cut = |at: usize| {
        if at > start && at < end {
            cuts.push(at);
        }
    };
    let first = marks.partition_point(|mark| mark.range.end <= start);
    for mark in marks.iter().skip(first) {
        if mark.range.start >= end {
            break;
        }
        cut(mark.range.start);
        cut(mark.range.end);
    }
    let first = line.misspelled.partition_point(|word| word.end <= start);
    for word in line.misspelled.iter().skip(first) {
        if word.start >= end {
            break;
        }
        cut(word.start);
        cut(word.end);
    }
    if let Some((selected, _)) = chosen {
        cut(selected.start);
        cut(selected.end);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut spans: Vec<ViewSpan> = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        // Marks start and end at ASCII delimiters and everything else is on
        // a grapheme boundary, so this never fails. It must not panic if
        // that is ever wrong.
        let Some(text) = line.text.get(from..to) else {
            continue;
        };
        let kind = kind_at(marks, from);
        let misspelled = covers(&line.misspelled, from);
        let selected = chosen.is_some_and(|(range, _)| range.start <= from && from < range.end);
        match spans.last_mut() {
            Some(last)
                if last.kind == kind
                    && last.misspelled == misspelled
                    && last.selected == selected =>
            {
                last.text.push_str(text);
            }
            _ => spans.push(ViewSpan {
                text: text.to_owned(),
                selected,
                misspelled,
                kind,
            }),
        }
    }
    // A selected line break is drawn as one selected cell after the line,
    // where there is room for it.
    let runs_on = chosen.is_some_and(|(_, runs_on)| *runs_on);
    if runs_on && row + 1 == line.rows() && text::width(shown) < width {
        spans.push(ViewSpan {
            text: " ".to_owned(),
            selected: true,
            misspelled: false,
            kind: SpanKind::Text,
        });
    }
    ViewLine { spans }
}

/// The end of a case-insensitive match of `needle` (already lowercased)
/// that starts at byte `start` of `text`.
fn match_at(text: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut wanted = needle.iter();
    for (offset, c) in text.get(start..)?.char_indices() {
        for lower in c.to_lowercase() {
            if wanted.next() != Some(&lower) {
                return None;
            }
        }
        if wanted.len() == 0 {
            return Some(start + offset + c.len_utf8());
        }
    }
    None
}

/// Where the cursor belongs in a reflowed paragraph: after as many
/// non-space bytes of the paragraph's own text as were before it, and at
/// the start of the next word if that is where it was. Returns the line
/// within the paragraph and the byte offset in it.
fn reflowed_cursor(
    lines: &[String],
    prefix: usize,
    solid: usize,
    at_word_start: bool,
) -> (usize, usize) {
    let mut left = solid;
    for (index, line) in lines.iter().enumerate() {
        let own = line.as_bytes().get(prefix..).unwrap_or(&[]);
        let here = own.iter().filter(|byte| **byte != b' ').count();
        let more = index + 1 < lines.len();
        if left > here || (left == here && at_word_start && more) {
            left -= here;
            continue;
        }
        let mut at = 0;
        let mut passed = 0;
        while passed < left && at < own.len() {
            if own[at] != b' ' {
                passed += 1;
            }
            at += 1;
        }
        if at_word_start {
            while own.get(at) == Some(&b' ') {
                at += 1;
            }
        }
        return (index, prefix + at);
    }
    let last = lines.len().saturating_sub(1);
    (last, lines.get(last).map_or(0, String::len))
}

/// A cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    /// The ends of the visual, soft-wrapped row.
    RowStart,
    RowEnd,
    /// The ends of the hard line.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    ParagraphUp,
    ParagraphDown,
    Up,
    Down,
    PageUp,
    PageDown,
}

impl Motion {
    fn of(chord: KeyChord) -> Option<Self> {
        let mods = chord.mods;
        let none = !mods.ctrl && !mods.alt && !mods.cmd;
        let alt = mods.alt && !mods.ctrl && !mods.cmd;
        let cmd = mods.cmd && !mods.ctrl && !mods.alt;
        let ctrl = mods.ctrl && !mods.alt && !mods.cmd;
        Some(match chord.key {
            Key::Left if none => Self::Left,
            Key::Left if alt => Self::WordLeft,
            Key::Left if cmd => Self::RowStart,
            Key::Right if none => Self::Right,
            Key::Right if alt => Self::WordRight,
            Key::Right if cmd => Self::RowEnd,
            Key::Up if none => Self::Up,
            Key::Up if alt => Self::ParagraphUp,
            Key::Up if cmd => Self::DocStart,
            Key::Down if none => Self::Down,
            Key::Down if alt => Self::ParagraphDown,
            Key::Down if cmd => Self::DocEnd,
            Key::Home if none => Self::RowStart,
            Key::End if none => Self::RowEnd,
            Key::PageUp if none => Self::PageUp,
            Key::PageDown if none => Self::PageDown,
            // The readline keys, which mean what they mean in every Mac text
            // field: ctrl-a and ctrl-e go to the ends of the paragraph.
            Key::Char('a') if ctrl => Self::LineStart,
            Key::Char('e') if ctrl => Self::LineEnd,
            Key::Char('f') if ctrl => Self::Right,
            Key::Char('b') if ctrl => Self::Left,
            Key::Char('n') if ctrl => Self::Down,
            Key::Char('p') if ctrl => Self::Up,
            _ => return None,
        })
    }

    fn is_vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::PageUp | Self::PageDown)
    }
}

/// What kind of edit an undo step is. Steps of the same kind made one after
/// another join into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// One typed character.
    Typing,
    /// One grapheme deleted backwards.
    Backspace,
    /// One grapheme deleted forwards.
    Delete,
    /// Anything else: a paste, a line break, a cut, a command. Never joined.
    Other,
}

/// The cursor and selection, as an undo step restores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Caret {
    cursor: Pos,
    anchor: Option<Pos>,
}

/// One undo step: at `at`, `removed` was replaced by `inserted`.
#[derive(Debug, Clone)]
struct Step {
    /// Names the state of the text after this step, for `is_modified`.
    id: u64,
    kind: Kind,
    at: Pos,
    removed: String,
    inserted: String,
    /// Where `inserted` ends.
    end: Pos,
    before: Caret,
    after: Caret,
}

/// The built-in editor. See the module documentation.
#[derive(Debug, Clone)]
pub struct BuiltinEditor {
    /// The hard lines. Never empty: an empty text is one empty line.
    lines: Vec<Line>,
    cursor: Pos,
    /// The other end of the selection. A selection is from here to the
    /// cursor, in either direction.
    anchor: Option<Pos>,
    /// The cursor sits where a soft-wrapped row ends and the next begins,
    /// and belongs at the end of the upper row. `cmd-right` leaves it so.
    upstream: bool,
    /// The cell column `up` and `down` aim for, kept across short lines.
    goal_x: Option<usize>,
    /// Whether `tab` belongs to the editor. Off, the default, `tab` is
    /// [`Outcome::Ignored`] so compose can use it to change fields. On, it
    /// inserts spaces to the next multiple of four. `shift-tab` is always
    /// ignored.
    pub tab_inserts: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
    /// The id of the step that was last when the text was saved. Zero is
    /// the text as loaded.
    saved_id: u64,
    /// The kind of the last edit, while the next one may still join it.
    /// Anything but another edit of that kind clears it.
    last: Option<Kind>,
    /// The size of the last view, which movement by row and page goes by.
    width: usize,
    height: usize,
    /// The first visible row, counted over the whole text.
    top: usize,
    /// Whether the next view scrolls to show the cursor. `scroll` turns it
    /// off; using the cursor turns it back on.
    follow: bool,
}

impl Default for BuiltinEditor {
    fn default() -> Self {
        Self::new("")
    }
}

impl Editor for BuiltinEditor {
    fn text(&self) -> String {
        let bytes: usize = self.lines.iter().map(|line| line.text.len() + 1).sum();
        let mut out = String::with_capacity(bytes);
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&line.text);
        }
        out
    }

    fn set_text(&mut self, text: &str) {
        self.lines = clean(text)
            .split('\n')
            .map(|line| Line::new(line.to_owned()))
            .collect();
        self.cursor = Pos::default();
        self.anchor = None;
        self.upstream = false;
        self.goal_x = None;
        self.undo.clear();
        self.redo.clear();
        self.saved_id = 0;
        self.last = None;
        self.top = 0;
        self.follow = true;
    }

    fn set_cursor_offset(&mut self, byte_offset: usize) {
        let at = self.pos_of(byte_offset);
        self.place(Pos {
            line: at.line,
            col: floor_boundary(self.text_of(at.line), at.col),
        });
    }

    fn insert(&mut self, text: &str) {
        let last = self.last.take();
        self.put(text, last);
        self.follow = true;
    }

    fn key(&mut self, chord: KeyChord) -> Outcome {
        let last = self.last.take();
        let mods = chord.mods;
        if let Some(motion) = Motion::of(chord) {
            self.move_cursor(motion, mods.shift);
            self.follow = true;
            return Outcome::Handled;
        }
        let plain = !mods.ctrl && !mods.alt && !mods.cmd;
        let ctrl = mods.ctrl && !mods.alt && !mods.cmd;
        let cmd = mods.cmd && !mods.ctrl && !mods.alt;
        match chord.key {
            Key::Char(c) if plain => {
                let mut buffer = [0; 4];
                self.put(c.encode_utf8(&mut buffer), last);
            }
            Key::Enter if !mods.ctrl && !mods.cmd => self.put("\n", None),
            Key::Tab if plain && !mods.shift && self.tab_inserts => {
                let start = self.span().map_or(self.cursor, |(start, _)| start);
                let before = self.text_of(start.line).get(..start.col).unwrap_or("");
                let spaces = TAB_STOP - text::width(before) % TAB_STOP;
                self.put(&" ".repeat(spaces), None);
            }
            Key::Backspace => self.backspace(mods, last),
            Key::Delete => self.delete(mods, last),
            Key::Char('d') if ctrl => self.delete(Mods::default(), last),
            Key::Char('k') if ctrl => self.kill_line(),
            Key::Char('w') if ctrl => self.backspace(
                Mods {
                    alt: true,
                    ..Mods::default()
                },
                None,
            ),
            Key::Char('z') if cmd && !mods.shift => {
                self.undo();
            }
            // Shift is folded into the letter, so cmd-shift-z arrives as Z.
            Key::Char('Z' | 'z') if cmd => {
                self.redo();
            }
            Key::Char('a') if cmd => {
                self.select_all();
                return Outcome::Handled;
            }
            Key::Char('c') if cmd => {
                return self.copy().map_or(Outcome::Handled, Outcome::Copy);
            }
            Key::Char('x') if cmd => {
                return self.cut().map_or(Outcome::Handled, Outcome::Copy);
            }
            _ => {
                // A key that is not the editor's does not end a run of typing.
                self.last = last;
                return Outcome::Ignored;
            }
        }
        self.follow = true;
        Outcome::Handled
    }

    fn is_modified(&self) -> bool {
        self.state_id() != self.saved_id
    }

    fn mark_saved(&mut self) {
        self.saved_id = self.state_id();
        // The next character must start a new undo step, or undoing it
        // would go back past the saved text without ever landing on it.
        self.last = None;
    }
}

impl BuiltinEditor {
    /// An editor holding `text`, with the cursor at the start. Until the
    /// first [`BuiltinEditor::view`] it takes its area to be 80 x 24.
    pub fn new(text: &str) -> Self {
        let mut editor = Self {
            lines: vec![Line::default()],
            cursor: Pos::default(),
            anchor: None,
            upstream: false,
            goal_x: None,
            tab_inserts: false,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            saved_id: 0,
            last: None,
            width: 80,
            height: 24,
            top: 0,
            follow: true,
        };
        editor.set_text(text);
        editor
    }

    /// The cursor as a byte offset into the text.
    pub fn cursor_offset(&self) -> usize {
        self.offset_of(self.cursor)
    }

    /// The selection as byte offsets into the text, if anything is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.span()
            .map(|(start, end)| self.offset_of(start)..self.offset_of(end))
    }

    /// The selected text, as `cmd-c` copies it.
    pub fn copy(&self) -> Option<String> {
        self.span().map(|(start, end)| self.slice(start, end))
    }

    /// Deletes the selection and returns it, as `cmd-x` does.
    pub fn cut(&mut self) -> Option<String> {
        self.last = None;
        let (start, end) = self.span()?;
        let text = self.slice(start, end);
        self.edit(start, end, "", Kind::Other, None);
        self.follow = true;
        Some(text)
    }

    /// Selects the whole text, as `cmd-a` does. The cursor goes to the end,
    /// but the view stays where it is, as on a Mac: the next key that uses
    /// the cursor scrolls to it.
    pub fn select_all(&mut self) {
        self.last = None;
        self.anchor = Some(Pos::default());
        self.cursor = self.end();
        self.upstream = false;
        self.goal_x = None;
        self.follow = false;
    }

    /// Takes back the last step, putting the cursor and selection back where
    /// they were before it. Returns `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.last = None;
        let Some(step) = self.undo.pop() else {
            return false;
        };
        self.replace(step.at, step.end, &step.removed);
        self.restore(step.before);
        self.redo.push(step);
        true
    }

    /// Does again the step last undone. Returns `false` when there is none.
    pub fn redo(&mut self) -> bool {
        self.last = None;
        let Some(step) = self.redo.pop() else {
            return false;
        };
        self.replace(step.at, end_of(step.at, &step.removed), &step.inserted);
        self.restore(step.after);
        self.undo.push(step);
        true
    }

    /// Inserts `text` at the cursor as a quotation, each line prefixed with
    /// `> ` (an empty line gets a bare `>`, so no line ends in a space). The
    /// quotation starts and ends on lines of its own.
    pub fn paste_as_quotation(&mut self, text: &str) {
        self.last = None;
        let text = clean(text);
        let text = text.strip_suffix('\n').unwrap_or(&text);
        if text.is_empty() {
            return;
        }
        let (start, end) = self.span().unwrap_or((self.cursor, self.cursor));
        let mut block = String::with_capacity(text.len() + text.len() / 8 + 4);
        if start.col > 0 {
            block.push('\n');
        }
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                block.push('\n');
            }
            block.push('>');
            if !line.is_empty() {
                block.push(' ');
                block.push_str(line);
            }
        }
        if end.col < self.text_of(end.line).len() {
            block.push('\n');
        }
        self.edit(start, end, &block, Kind::Other, None);
        self.follow = true;
    }

    /// Re-wraps the paragraph under the cursor with hard line breaks, so no
    /// line is wider than `width` cells unless one word is. A paragraph is a
    /// run of lines with text on them and the same quote level; its first
    /// line's quote prefix is repeated on every line. A word is never
    /// broken, and a `-- ` signature separator is never joined to anything.
    pub fn reflow_paragraph(&mut self, width: usize) {
        self.last = None;
        let width = width.max(1);
        let here = self.cursor.line;
        let (level, _) = quote_prefix(self.text_of(here));
        let member = |line: &str| {
            let (its_level, own) = quote_prefix(line);
            its_level == level && line != "-- " && !line[own..].trim().is_empty()
        };
        if !member(self.text_of(here)) {
            return;
        }
        let mut first = here;
        while first > 0 && member(self.text_of(first - 1)) {
            first -= 1;
        }
        let mut last = here;
        while last + 1 < self.lines.len() && member(self.text_of(last + 1)) {
            last += 1;
        }
        let solid = |text: &str| text.bytes().filter(|byte| *byte != b' ').count();

        // Fill rows with the paragraph's words.
        let first_line = self.text_of(first);
        let prefix = first_line[..quote_prefix(first_line).1].to_owned();
        let prefix_width = text::width(&prefix);
        let mut rows: Vec<String> = Vec::new();
        let mut row = prefix.clone();
        let mut used = prefix_width;
        let mut empty = true;
        for index in first..=last {
            for word in own_text(self.text_of(index)).split(' ') {
                if word.is_empty() {
                    continue;
                }
                let w = text::width(word);
                if !empty && used + 1 + w > width {
                    rows.push(std::mem::replace(&mut row, prefix.clone()));
                    used = prefix_width;
                    empty = true;
                }
                if !empty {
                    row.push(' ');
                    used += 1;
                }
                row.push_str(word);
                used += w;
                empty = false;
            }
        }
        rows.push(row);

        // The cursor stays with the text it was in.
        let cursor = self.cursor;
        let cursor_line = self.text_of(cursor.line);
        let own_start = quote_prefix(cursor_line).1;
        let before = cursor_line.get(own_start..cursor.col).unwrap_or("");
        let after = cursor_line.get(cursor.col.max(own_start)..).unwrap_or("");
        let passed: usize = (first..cursor.line)
            .map(|index| solid(own_text(self.text_of(index))))
            .sum::<usize>()
            + solid(before);
        let at_word_start = !after.is_empty()
            && !after.starts_with(' ')
            && (before.is_empty() || before.ends_with(' '));
        let (row, col) = reflowed_cursor(&rows, prefix.len(), passed, at_word_start);

        let start = Pos {
            line: first,
            col: 0,
        };
        let end = Pos {
            line: last,
            col: self.text_of(last).len(),
        };
        let filled = rows.join("\n");
        self.follow = true;
        if self.slice(start, end) == filled {
            return;
        }
        self.edit(start, end, &filled, Kind::Other, None);
        self.cursor = self.clamp(Pos {
            line: first + row,
            col,
        });
        self.settle();
    }

    /// Adds `-- `, a line break and `signature` at the end of the text,
    /// after a blank line, unless that signature is already in the text. The
    /// cursor, the selection and the scroll position stay where they were.
    pub fn insert_signature(&mut self, signature: &str) {
        self.last = None;
        let signature = clean(signature);
        let signature = signature.trim_matches('\n').trim_end();
        if signature.is_empty() {
            return;
        }
        let block = format!("-- \n{signature}");
        let text = self.text();
        let present = text.match_indices(&block).any(|(at, _)| {
            let end = at + block.len();
            (at == 0 || text.as_bytes().get(at - 1) == Some(&b'\n'))
                && matches!(text.as_bytes().get(end), None | Some(b'\n'))
        });
        if present {
            return;
        }
        // One blank line between the body and the separator. An empty body
        // gets a line to write on as well.
        let gap = if text.is_empty() || !text.ends_with('\n') {
            "\n\n"
        } else if text.ends_with("\n\n") {
            ""
        } else {
            "\n"
        };
        let caret = self.caret();
        let upstream = self.upstream;
        let end = self.end();
        self.edit(end, end, &format!("{gap}{block}"), Kind::Other, None);
        self.cursor = caret.cursor;
        self.anchor = caret.anchor;
        self.upstream = upstream;
        self.settle();
    }

    /// Selects the next match of `needle` after the cursor (before it, when
    /// `forward` is false), wrapping around the ends of the text. Case is
    /// ignored. Returns `false`, changing nothing, when there is no match.
    ///
    /// The search starts after the selection, so calling this again finds
    /// the match after the one just selected.
    pub fn find(&mut self, needle: &str, forward: bool) -> bool {
        let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
        if needle.is_empty() {
            return false;
        }
        let text = self.text();
        let cursor = self.cursor_offset();
        let selection = self.selection().unwrap_or(cursor..cursor);
        let matched = |start: usize| match_at(&text, start, &needle).map(|end| (start, end));
        let starts = |range: Range<usize>| {
            let base = range.start;
            text.get(range)
                .unwrap_or("")
                .char_indices()
                .map(move |(index, _)| base + index)
        };
        let found = if forward {
            let from = selection.end;
            starts(from..text.len())
                .find_map(matched)
                .or_else(|| starts(0..from).find_map(matched))
        } else {
            let from = selection.start;
            starts(0..from)
                .rev()
                .find_map(matched)
                .or_else(|| starts(from..text.len()).rev().find_map(matched))
        };
        let Some((start, end)) = found else {
            return false;
        };
        // A match may begin or end inside a grapheme cluster (a letter
        // without its accent). The selection takes the whole cluster.
        let start = self.pos_of(start);
        let end = self.pos_of(end);
        let start = Pos {
            line: start.line,
            col: floor_boundary(self.text_of(start.line), start.col),
        };
        let end = Pos {
            line: end.line,
            col: ceil_boundary(self.text_of(end.line), end.col),
        };
        self.place(end);
        self.anchor = Some(start);
        true
    }

    /// Puts the cursor at a cell of the last view, as a click does: in
    /// front of the grapheme under the pointer, or at the end of the row or
    /// of the text when the pointer is past it. Clears the selection.
    pub fn click(&mut self, column: u16, row: u16) {
        let (at, upstream) = self.pos_at_cell(column, row);
        self.place(at);
        self.upstream = upstream;
    }

    /// Extends the selection to a cell of the last view, as dragging does
    /// after a click, or as a shift-click does. A row below the view is
    /// allowed: the view then scrolls to follow the cursor.
    pub fn drag(&mut self, column: u16, row: u16) {
        let anchor = self.anchor.unwrap_or(self.cursor);
        let (at, upstream) = self.pos_at_cell(column, row);
        self.place(at);
        self.upstream = upstream;
        self.anchor = Some(anchor);
    }

    /// Scrolls the view by `lines` rows (down when positive) without moving
    /// the cursor, as the wheel does on a Mac. The cursor may end up out of
    /// sight, and [`EditorView::cursor`] is then `None`. The next key, edit,
    /// click or find brings the view back to the cursor.
    pub fn scroll(&mut self, lines: isize) {
        let max = self.wrap_width();
        let mut total = 0;
        for line in &mut self.lines {
            line.wrap(max);
            total += line.rows();
        }
        self.top = self
            .top
            .saturating_add_signed(lines)
            .min(total.saturating_sub(self.height));
        self.follow = false;
    }

    /// Replaces the misspelled ranges: byte ranges into the text, from a
    /// spell checker that runs elsewhere. They are drawn with
    /// [`ViewSpan::misspelled`] set.
    ///
    /// When the text changes, a range the change touches or overlaps is
    /// dropped (the word is no longer the word that was checked), ranges
    /// after it on the same line move with the text, and all others stay.
    pub fn set_misspellings(&mut self, mut ranges: Vec<Range<usize>>) {
        for line in &mut self.lines {
            line.misspelled.clear();
        }
        ranges.retain(|range| range.start < range.end);
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        let mut index = 0;
        // The offset in the whole text at which line `index` starts.
        let mut base = 0;
        for range in ranges {
            while let Some(line) = self.lines.get(index) {
                if range.start <= base + line.text.len() {
                    break;
                }
                base += line.text.len() + 1;
                index += 1;
            }
            let Some(line) = self.lines.get_mut(index) else {
                break;
            };
            // A word does not cross a line break: cut the range at the line's
            // end, and keep it off the middle of a character.
            let start = floor_char(&line.text, range.start.saturating_sub(base));
            let end = floor_char(&line.text, range.end.saturating_sub(base));
            if start >= end {
                continue;
            }
            match line.misspelled.last_mut() {
                Some(previous) if previous.end >= start => previous.end = previous.end.max(end),
                _ => line.misspelled.push(start..end),
            }
        }
    }

    /// The misspelled ranges as they stand, as byte ranges into the text.
    pub fn misspellings(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut base = 0;
        for line in &self.lines {
            out.extend(
                line.misspelled
                    .iter()
                    .map(|range| base + range.start..base + range.end),
            );
            base += line.text.len() + 1;
        }
        out
    }

    /// What to draw in an area `width` x `height`, scrolled to keep the
    /// cursor visible unless [`BuiltinEditor::scroll`] has moved the view
    /// away. Movement by row and by page goes by the size given here.
    pub fn view(&mut self, width: usize, height: usize) -> EditorView {
        if width == 0 || height == 0 {
            return EditorView::default();
        }
        self.width = width;
        self.height = height;
        let max = self.wrap_width();

        // One pass over every line. For all but the lines that changed this
        // is a comparison and an addition.
        let (cursor, upstream) = (self.cursor, self.upstream);
        let mut total = 0;
        let mut cursor_at = (0, 0);
        for (index, line) in self.lines.iter_mut().enumerate() {
            line.wrap(max);
            if index == cursor.line {
                let (row, x) = line.locate(cursor.col, upstream);
                cursor_at = (total + row, x);
            }
            total += line.rows();
        }
        if self.follow {
            if cursor_at.0 < self.top {
                self.top = cursor_at.0;
            } else if cursor_at.0 >= self.top + height {
                self.top = cursor_at.0 + 1 - height;
            }
        }
        self.top = self.top.min(total.saturating_sub(height));

        // Find the line the first visible row belongs to, and whether a code
        // block is open above it.
        let mut index = 0;
        let mut above = 0;
        let mut fence = 0;
        while let Some(line) = self.lines.get(index) {
            if above + line.rows() > self.top {
                break;
            }
            above += line.rows();
            fence = fence_after(fence, line.fence);
            index += 1;
        }
        let mut skip = self.top.saturating_sub(above);

        let selection = self.span();
        let mut lines = Vec::with_capacity(height.min(total));
        while lines.len() < height {
            let Some(line) = self.lines.get(index) else {
                break;
            };
            let fenced = fence != 0 || line.fence != 0;
            fence = fence_after(fence, line.fence);
            let marks = marks(&line.text, fenced);
            let chosen = selection.and_then(|(start, end)| {
                if index < start.line || index > end.line {
                    return None;
                }
                let from = if index == start.line { start.col } else { 0 };
                let to = if index == end.line {
                    end.col
                } else {
                    line.text.len()
                };
                Some((from..to, index < end.line))
            });
            for row in skip..line.rows() {
                if lines.len() == height {
                    break;
                }
                lines.push(view_row(line, row, width, &marks, chosen.as_ref()));
            }
            skip = 0;
            index += 1;
        }

        let visible = cursor_at.0 >= self.top && cursor_at.0 < self.top + height;
        let cursor = visible.then(|| {
            let column = cursor_at.1.min(width - 1);
            let row = cursor_at.0 - self.top;
            (
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(row).unwrap_or(u16::MAX),
            )
        });
        EditorView {
            lines,
            cursor,
            total_rows: total,
            first_row: self.top,
        }
    }

    // Everything below is private.

    fn text_of(&self, line: usize) -> &str {
        self.lines.get(line).map_or("", |line| line.text.as_str())
    }

    /// The end of the text.
    fn end(&self) -> Pos {
        let line = self.lines.len().saturating_sub(1);
        Pos {
            line,
            col: self.text_of(line).len(),
        }
    }

    /// The nearest real position to `at`.
    fn clamp(&self, at: Pos) -> Pos {
        let line = at.line.min(self.lines.len().saturating_sub(1));
        Pos {
            line,
            col: floor_boundary(self.text_of(line), at.col),
        }
    }

    fn offset_of(&self, at: Pos) -> usize {
        let before: usize = self
            .lines
            .iter()
            .take(at.line)
            .map(|line| line.text.len() + 1)
            .sum();
        before + at.col
    }

    /// The line and column of a byte offset into the whole text. The column
    /// may be inside a character; callers round it.
    fn pos_of(&self, offset: usize) -> Pos {
        let mut left = offset;
        for (index, line) in self.lines.iter().enumerate() {
            if left <= line.text.len() {
                return Pos {
                    line: index,
                    col: left,
                };
            }
            left -= line.text.len() + 1;
        }
        self.end()
    }

    /// The selection, start first, if anything is selected.
    fn span(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn caret(&self) -> Caret {
        Caret {
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    /// Puts the cursor somewhere, dropping the selection and everything
    /// that goes with the old place.
    fn place(&mut self, at: Pos) {
        self.last = None;
        self.cursor = at;
        self.anchor = None;
        self.upstream = false;
        self.goal_x = None;
        self.follow = true;
    }

    fn restore(&mut self, caret: Caret) {
        self.place(self.clamp(caret.cursor));
        self.anchor = caret.anchor.map(|anchor| self.clamp(anchor));
    }

    /// Names the text as it is now: the id of the last step applied.
    fn state_id(&self) -> u64 {
        self.undo.last().map_or(0, |step| step.id)
    }

    /// How many cells of text a row holds: one fewer than the view is wide,
    /// which leaves the last column for the cursor.
    fn wrap_width(&self) -> usize {
        self.width.saturating_sub(1).max(1)
    }

    /// The text between two positions.
    fn slice(&self, start: Pos, end: Pos) -> String {
        if start.line == end.line {
            let line = self.text_of(start.line);
            return line.get(start.col..end.col).unwrap_or("").to_owned();
        }
        let mut out = String::new();
        out.push_str(self.text_of(start.line).get(start.col..).unwrap_or(""));
        for line in start.line + 1..end.line {
            out.push('\n');
            out.push_str(self.text_of(line));
        }
        out.push('\n');
        out.push_str(self.text_of(end.line).get(..end.col).unwrap_or(""));
        out
    }

    /// Replaces `start..end` with `text`. This is the only place the text
    /// changes. It records nothing and does not move the cursor.
    fn replace(&mut self, start: Pos, end: Pos, text: &str) {
        if start > end {
            return;
        }
        if start.line == end.line && !text.contains('\n') {
            // The common case, a change within one line, moves no lines.
            let Some(line) = self.lines.get_mut(start.line) else {
                return;
            };
            if !line.text.is_char_boundary(start.col) || !line.text.is_char_boundary(end.col) {
                return;
            }
            line.text.replace_range(start.col..end.col, text);
            let (cut, added) = (end.col - start.col, text.len());
            line.misspelled.retain_mut(|word| {
                if word.end < start.col {
                    return true;
                }
                if word.start <= end.col {
                    return false;
                }
                *word = word.start - cut + added..word.end - cut + added;
                true
            });
            line.changed();
            return;
        }
        let (Some(first), Some(last)) = (self.lines.get(start.line), self.lines.get(end.line))
        else {
            return;
        };
        let (Some(head), Some(tail)) = (first.text.get(..start.col), last.text.get(end.col..))
        else {
            return;
        };
        let mut pieces = text.split('\n');
        let mut current = String::with_capacity(head.len() + text.len().min(256));
        current.push_str(head);
        current.push_str(pieces.next().unwrap_or(""));
        let kept: Vec<Range<usize>> = first
            .misspelled
            .iter()
            .filter(|word| word.end < start.col)
            .cloned()
            .collect();
        let mut fresh: Vec<Line> = Vec::new();
        for piece in pieces {
            fresh.push(Line::new(std::mem::replace(&mut current, piece.to_owned())));
        }
        let base = current.len();
        let carried: Vec<Range<usize>> = last
            .misspelled
            .iter()
            .filter(|word| word.start > end.col)
            .map(|word| word.start - end.col + base..word.end - end.col + base)
            .collect();
        current.push_str(tail);
        fresh.push(Line::new(current));
        if let Some(line) = fresh.first_mut() {
            line.misspelled = kept;
        }
        if let Some(line) = fresh.last_mut() {
            line.misspelled.extend(carried);
        }
        self.lines.splice(start.line..=end.line, fresh);
    }

    /// Replaces `start..end` with `text` as an undoable step, and leaves the
    /// cursor after the new text with nothing selected. `last` is the kind
    /// of the edit before this one, if this one may join it.
    fn edit(&mut self, start: Pos, end: Pos, text: &str, kind: Kind, last: Option<Kind>) {
        if start == end && text.is_empty() {
            return;
        }
        let before = self.caret();
        let removed = self.slice(start, end);
        self.replace(start, end, text);
        let end = end_of(start, text);
        // New text can join the cluster after it (a letter typed in front of
        // a combining accent), so the end may need moving past that.
        self.cursor = Pos {
            line: end.line,
            col: ceil_boundary(self.text_of(end.line), end.col),
        };
        self.anchor = None;
        self.upstream = false;
        self.goal_x = None;
        let step = Step {
            id: self.next_id,
            kind,
            at: start,
            removed,
            inserted: text.to_owned(),
            end,
            before,
            after: self.caret(),
        };
        self.next_id += 1;
        self.record(step, last);
    }

    /// Adds a step to the undo history, joining it to the step before when
    /// both are part of one run of typing or deleting.
    fn record(&mut self, step: Step, last: Option<Kind>) {
        self.redo.clear();
        self.last = (step.kind != Kind::Other).then_some(step.kind);
        if last == Some(step.kind)
            && let Some(top) = self.undo.last_mut()
            && top.kind == step.kind
        {
            match step.kind {
                Kind::Typing if top.end == step.at => {
                    top.inserted.push_str(&step.inserted);
                    top.end = step.end;
                    top.after = step.after;
                    return;
                }
                Kind::Backspace if end_of(step.at, &step.removed) == top.at => {
                    top.removed.insert_str(0, &step.removed);
                    top.at = step.at;
                    top.end = step.at;
                    top.after = step.after;
                    return;
                }
                Kind::Delete if step.at == top.at => {
                    top.removed.push_str(&step.removed);
                    top.after = step.after;
                    return;
                }
                _ => {}
            }
        }
        self.undo.push(step);
    }

    /// Makes the last step's "after" the cursor and selection as they are
    /// now, for a command that moves the cursor once its edit is made.
    fn settle(&mut self) {
        let caret = self.caret();
        if let Some(step) = self.undo.last_mut() {
            step.after = caret;
        }
    }

    /// Typed or pasted text: replaces the selection, or goes in at the
    /// cursor. A single character is typing and joins the run before it.
    /// Text with nothing printable in it does nothing, selection or not.
    fn put(&mut self, text: &str, last: Option<Kind>) {
        let text = clean(text);
        if text.is_empty() {
            return;
        }
        let (start, end) = self.span().unwrap_or((self.cursor, self.cursor));
        let typed = start == end && !text.contains('\n') && is_one_grapheme(&text);
        let kind = if typed { Kind::Typing } else { Kind::Other };
        self.edit(start, end, &text, kind, last);
    }

    fn backspace(&mut self, mods: Mods, last: Option<Kind>) {
        if let Some((start, end)) = self.span() {
            self.edit(start, end, "", Kind::Other, None);
            return;
        }
        let at = self.cursor;
        if mods.cmd {
            let (start, _) = self.target(Motion::RowStart);
            self.edit(start, at, "", Kind::Other, None);
        } else if mods.alt {
            self.edit(self.word_left(at), at, "", Kind::Other, None);
        } else {
            self.edit(self.before(at), at, "", Kind::Backspace, last);
        }
    }

    fn delete(&mut self, mods: Mods, last: Option<Kind>) {
        if let Some((start, end)) = self.span() {
            self.edit(start, end, "", Kind::Other, None);
            return;
        }
        let at = self.cursor;
        if mods.cmd {
            let (end, _) = self.target(Motion::RowEnd);
            self.edit(at, end, "", Kind::Other, None);
        } else if mods.alt {
            self.edit(at, self.word_right(at), "", Kind::Other, None);
        } else {
            self.edit(at, self.after(at), "", Kind::Delete, last);
        }
    }

    /// `ctrl-k`: deletes to the end of the hard line, or the line break
    /// itself when the cursor is already there.
    fn kill_line(&mut self) {
        let (start, end) = match self.span() {
            Some(span) => span,
            None => {
                let at = self.cursor;
                let line_end = Pos {
                    line: at.line,
                    col: self.text_of(at.line).len(),
                };
                let end = if at == line_end {
                    self.after(at)
                } else {
                    line_end
                };
                (at, end)
            }
        };
        self.edit(start, end, "", Kind::Other, None);
    }

    fn move_cursor(&mut self, motion: Motion, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            let selection = self.span();
            self.anchor = None;
            // Left and right without shift go to that end of the selection.
            if let (Some((start, end)), Motion::Left | Motion::Right) = (selection, motion) {
                self.cursor = if motion == Motion::Left { start } else { end };
                self.upstream = false;
                self.goal_x = None;
                return;
            }
        }
        if !motion.is_vertical() {
            self.goal_x = None;
        }
        // A page moves the view as far as the cursor, so the cursor keeps
        // its place on the screen.
        let page = self.page();
        match motion {
            Motion::PageUp => self.top = self.top.saturating_sub(page),
            Motion::PageDown => self.top = self.top.saturating_add(page),
            _ => {}
        }
        let (cursor, upstream) = self.target(motion);
        self.cursor = cursor;
        self.upstream = upstream;
    }

    /// How many rows `pageup` and `pagedown` move: a screenful, less one row
    /// kept for context.
    fn page(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// Where a movement takes the cursor, and whether it then belongs at the
    /// end of the upper row (see `upstream`).
    fn target(&mut self, motion: Motion) -> (Pos, bool) {
        let at = self.cursor;
        let line_end = Pos {
            line: at.line,
            col: self.text_of(at.line).len(),
        };
        let line_start = Pos {
            line: at.line,
            col: 0,
        };
        match motion {
            Motion::Left => (self.before(at), false),
            Motion::Right => (self.after(at), false),
            Motion::WordLeft => (self.word_left(at), false),
            Motion::WordRight => (self.word_right(at), false),
            Motion::RowStart | Motion::RowEnd => {
                let (max, upstream) = (self.wrap_width(), self.upstream);
                let Some(line) = self.lines.get_mut(at.line) else {
                    return (at, false);
                };
                line.wrap(max);
                let (row, _) = line.locate(at.col, upstream);
                let range = line.row_range(row);
                if motion == Motion::RowStart {
                    (
                        Pos {
                            line: at.line,
                            col: range.start,
                        },
                        false,
                    )
                } else {
                    (
                        Pos {
                            line: at.line,
                            col: range.end,
                        },
                        row + 1 < line.rows(),
                    )
                }
            }
            Motion::LineStart => (line_start, false),
            Motion::LineEnd => (line_end, false),
            Motion::DocStart => (Pos::default(), false),
            Motion::DocEnd => (self.end(), false),
            // The start of this paragraph, then of the one before it.
            Motion::ParagraphUp if at.col > 0 || at.line == 0 => (line_start, false),
            Motion::ParagraphUp => (
                Pos {
                    line: at.line - 1,
                    col: 0,
                },
                false,
            ),
            Motion::ParagraphDown if at != line_end || at.line + 1 >= self.lines.len() => {
                (line_end, false)
            }
            Motion::ParagraphDown => (
                Pos {
                    line: at.line + 1,
                    col: self.text_of(at.line + 1).len(),
                },
                false,
            ),
            Motion::Up => self.rows_away(1, false),
            Motion::Down => self.rows_away(1, true),
            Motion::PageUp => self.rows_away(self.page(), false),
            Motion::PageDown => self.rows_away(self.page(), true),
        }
    }

    /// The position `count` visual rows above or below the cursor, in the
    /// column the run of vertical movement started from. Past the first row
    /// that is the start of the text; past the last, its end.
    fn rows_away(&mut self, count: usize, down: bool) -> (Pos, bool) {
        let (max, upstream) = (self.wrap_width(), self.upstream);
        let mut index = self.cursor.line.min(self.lines.len().saturating_sub(1));
        let Some(line) = self.lines.get_mut(index) else {
            return (self.cursor, false);
        };
        line.wrap(max);
        let (mut row, x) = line.locate(self.cursor.col, upstream);
        let goal = *self.goal_x.get_or_insert(x);
        for _ in 0..count {
            let rows = self.lines.get(index).map_or(1, Line::rows);
            if down {
                if row + 1 < rows {
                    row += 1;
                } else if let Some(next) = self.lines.get_mut(index + 1) {
                    next.wrap(max);
                    index += 1;
                    row = 0;
                } else {
                    return (self.end(), false);
                }
            } else if row > 0 {
                row -= 1;
            } else if index > 0 {
                index -= 1;
                if let Some(previous) = self.lines.get_mut(index) {
                    previous.wrap(max);
                    row = previous.rows() - 1;
                }
            } else {
                return (Pos::default(), false);
            }
        }
        let (col, upstream) = self
            .lines
            .get(index)
            .map_or((0, false), |line| line.col_at(row, goal));
        (Pos { line: index, col }, upstream)
    }

    /// One grapheme back, across a line break if need be.
    fn before(&self, at: Pos) -> Pos {
        if at.col == 0 {
            if at.line == 0 {
                return at;
            }
            return Pos {
                line: at.line - 1,
                col: self.text_of(at.line - 1).len(),
            };
        }
        let text = self.text_of(at.line);
        let col = text
            .get(..at.col)
            .unwrap_or(text)
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index);
        Pos { line: at.line, col }
    }

    /// One grapheme on, across a line break if need be.
    fn after(&self, at: Pos) -> Pos {
        let text = self.text_of(at.line);
        match text
            .get(at.col..)
            .and_then(|rest| rest.graphemes(true).next())
        {
            Some(grapheme) => Pos {
                line: at.line,
                col: at.col + grapheme.len(),
            },
            None if at.line + 1 < self.lines.len() => Pos {
                line: at.line + 1,
                col: 0,
            },
            None => at,
        }
    }

    /// The start of the word before `at`, as `opt-left` finds it. From the
    /// start of a line that is the last word of the line above.
    fn word_left(&self, at: Pos) -> Pos {
        let mut at = at;
        if at.col == 0 {
            if at.line == 0 {
                return at;
            }
            at = Pos {
                line: at.line - 1,
                col: self.text_of(at.line - 1).len(),
            };
        }
        let text = self.text_of(at.line);
        let before = text.get(..at.col).unwrap_or(text);
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
        let col = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_alphanumeric())
            .map_or(0, |(index, c)| index + c.len_utf8());
        Pos {
            line: at.line,
            col: floor_boundary(text, col),
        }
    }

    /// The end of the word after `at`, as `opt-right` finds it. From the end
    /// of a line that is the first word of the line below.
    fn word_right(&self, at: Pos) -> Pos {
        let mut at = at;
        if at.col >= self.text_of(at.line).len() {
            if at.line + 1 >= self.lines.len() {
                return at;
            }
            at = Pos {
                line: at.line + 1,
                col: 0,
            };
        }
        let text = self.text_of(at.line);
        let after = text.get(at.col..).unwrap_or("");
        let skipped = after.len()
            - after
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .len();
        let word = after[skipped..]
            .char_indices()
            .find(|(_, c)| !c.is_alphanumeric())
            .map_or(after.len() - skipped, |(index, _)| index);
        Pos {
            line: at.line,
            col: ceil_boundary(text, at.col + skipped + word),
        }
    }

    /// The position drawn at a cell of the last view.
    fn pos_at_cell(&mut self, column: u16, row: u16) -> (Pos, bool) {
        let max = self.wrap_width();
        let target = self.top + usize::from(row);
        let mut above = 0;
        for (index, line) in self.lines.iter_mut().enumerate() {
            line.wrap(max);
            if target < above + line.rows() {
                let (col, upstream) = line.col_at(target - above, usize::from(column));
                return (Pos { line: index, col }, upstream);
            }
            above += line.rows();
        }
        (self.end(), false)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn press(editor: &mut BuiltinEditor, keys: &str) {
        for key in keys.split_whitespace() {
            let chord: KeyChord = key.parse().unwrap();
            assert_ne!(editor.key(chord), Outcome::Ignored, "{key} was not taken");
        }
    }

    /// Types `text` one key at a time, as a keyboard would.
    fn type_text(editor: &mut BuiltinEditor, text: &str) {
        for c in text.chars() {
            let chord = match c {
                '\n' => KeyChord::plain(Key::Enter),
                c => KeyChord::plain(Key::Char(c)),
            };
            assert_eq!(editor.key(chord), Outcome::Handled);
        }
    }

    /// The text with `|` at the cursor and `^` at the other end of the
    /// selection.
    fn marked(editor: &BuiltinEditor) -> String {
        let mut text = editor.text();
        let cursor = editor.cursor_offset();
        match editor.selection() {
            Some(range) if range.start == cursor => {
                text.insert(range.end, '^');
                text.insert(cursor, '|');
            }
            Some(range) => {
                text.insert(cursor, '|');
                text.insert(range.start, '^');
            }
            None => text.insert(cursor, '|'),
        }
        text
    }

    /// The rows of a view as plain strings.
    fn rows(view: &EditorView) -> Vec<String> {
        view.lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.text.as_str()).collect())
            .collect()
    }

    /// The spans of one row as text and kind.
    fn kinds(view: &EditorView, row: usize) -> Vec<(&str, SpanKind)> {
        view.lines[row]
            .spans
            .iter()
            .map(|span| (span.text.as_str(), span.kind))
            .collect()
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut editor = BuiltinEditor::default();
        press(&mut editor, "a c left b");
        assert_eq!(marked(&editor), "ab|c");
        editor.insert("XY");
        assert_eq!(marked(&editor), "abXY|c");
        // Text with nothing in it changes nothing, not even a selection.
        press(&mut editor, "shift-left");
        editor.insert("");
        editor.insert("\u{7}");
        assert_eq!(marked(&editor), "abX|Y^c");
        press(&mut editor, "right");
        press(&mut editor, "enter d space e");
        assert_eq!(marked(&editor), "abXY\nd e|c");
        // Shift or option held with enter is still a line break.
        press(&mut editor, "shift-enter alt-enter");
        assert_eq!(editor.text(), "abXY\nd e\n\nc");
    }

    #[test]
    fn it_works_through_the_trait() {
        let mut editor: Box<dyn Editor> = Box::new(BuiltinEditor::new("one"));
        editor.set_cursor_offset(3);
        editor.insert(" two");
        assert_eq!(editor.text(), "one two");
        assert!(editor.is_modified());
        editor.mark_saved();
        assert!(!editor.is_modified());
        assert_eq!(editor.key("cmd-z".parse().unwrap()), Outcome::Handled);
        assert_eq!(editor.text(), "one");
    }

    #[test]
    fn set_text_puts_the_cursor_at_the_start_and_offsets_are_clamped() {
        let mut editor = BuiltinEditor::new("héllo\n👨‍👩‍👧 x");
        assert_eq!(editor.cursor_offset(), 0);
        // Inside the two-byte é: back to its start.
        editor.set_cursor_offset(2);
        assert_eq!(editor.cursor_offset(), 1);
        // Inside the family emoji, on a char boundary but not a grapheme one.
        let family = "héllo\n".len();
        editor.set_cursor_offset(family + 4);
        assert_eq!(editor.cursor_offset(), family);
        editor.set_cursor_offset(family + "👨‍👩‍👧".len());
        assert_eq!(editor.cursor_offset(), family + "👨‍👩‍👧".len());
        editor.set_cursor_offset(usize::MAX);
        assert_eq!(editor.cursor_offset(), editor.text().len());
        // The line break is a position of its own.
        editor.set_cursor_offset(6);
        assert_eq!(marked(&editor), "héllo|\n👨‍👩‍👧 x");
        editor.set_text("new");
        assert_eq!(marked(&editor), "|new");
    }

    #[test]
    fn a_grapheme_cluster_is_one_step_one_deletion_and_one_unit_of_selection() {
        // A family emoji, a thumbs-up with a skin tone, a flag, and e with a
        // combining accent.
        let mut editor = BuiltinEditor::new("a👨‍👩‍👧👍🏽🇫🇷e\u{301}z");
        press(&mut editor, "cmd-down left left");
        assert_eq!(marked(&editor), "a👨‍👩‍👧👍🏽🇫🇷|e\u{301}z");
        press(&mut editor, "backspace");
        assert_eq!(marked(&editor), "a👨‍👩‍👧👍🏽|e\u{301}z");
        press(&mut editor, "backspace");
        assert_eq!(marked(&editor), "a👨‍👩‍👧|e\u{301}z");
        press(&mut editor, "delete");
        assert_eq!(marked(&editor), "a👨‍👩‍👧|z");
        press(&mut editor, "shift-left");
        assert_eq!(editor.copy().as_deref(), Some("👨‍👩‍👧"));
        press(&mut editor, "left right right right right");
        assert_eq!(marked(&editor), "a👨‍👩‍👧z|");
        // Past either end nothing happens.
        press(&mut editor, "delete right cmd-up backspace left");
        assert_eq!(marked(&editor), "|a👨‍👩‍👧z");
    }

    #[test]
    fn text_typed_in_front_of_a_combining_mark_keeps_the_cursor_on_a_boundary() {
        let mut editor = BuiltinEditor::new("\u{301}x");
        press(&mut editor, "e");
        // The e and the accent are now one cluster; the cursor is after it.
        assert_eq!(marked(&editor), "e\u{301}|x");
        press(&mut editor, "backspace");
        assert_eq!(marked(&editor), "|x");
        // Joining lines can make a cluster too.
        let mut editor = BuiltinEditor::new("a\n\u{301}b");
        press(&mut editor, "down backspace");
        assert_eq!(marked(&editor), "a\u{301}|b");
    }

    #[test]
    fn left_and_right_cross_line_breaks() {
        let mut editor = BuiltinEditor::new("ab\n\ncd");
        press(&mut editor, "right right right");
        assert_eq!(marked(&editor), "ab\n|\ncd");
        press(&mut editor, "right ctrl-f");
        assert_eq!(marked(&editor), "ab\n\nc|d");
        press(&mut editor, "left left ctrl-b");
        assert_eq!(marked(&editor), "ab|\n\ncd");
    }

    #[test]
    fn words() {
        let mut editor = BuiltinEditor::new("ana.ruiz@example.com, sam lee\n  next line");
        press(&mut editor, "alt-right");
        assert_eq!(
            marked(&editor),
            "ana|.ruiz@example.com, sam lee\n  next line"
        );
        press(&mut editor, "alt-right alt-right alt-right alt-right");
        assert_eq!(
            marked(&editor),
            "ana.ruiz@example.com, sam| lee\n  next line"
        );
        press(&mut editor, "alt-left alt-left");
        assert_eq!(
            marked(&editor),
            "ana.ruiz@example.|com, sam lee\n  next line"
        );
        // From the end of a line, the first word of the next.
        press(&mut editor, "ctrl-e alt-right");
        assert_eq!(
            marked(&editor),
            "ana.ruiz@example.com, sam lee\n  next| line"
        );
        // From the start of a line, the last word of the one above.
        press(&mut editor, "ctrl-a alt-left");
        assert_eq!(
            marked(&editor),
            "ana.ruiz@example.com, sam |lee\n  next line"
        );
        press(&mut editor, "cmd-down alt-right cmd-up alt-left");
        assert_eq!(editor.cursor_offset(), 0);
    }

    #[test]
    fn paragraph_and_document_ends() {
        let mut editor = BuiltinEditor::new("one two\nthree four\nfive");
        press(&mut editor, "down right right");
        assert_eq!(marked(&editor), "one two\nth|ree four\nfive");
        press(&mut editor, "alt-up");
        assert_eq!(marked(&editor), "one two\n|three four\nfive");
        press(&mut editor, "alt-up");
        assert_eq!(marked(&editor), "|one two\nthree four\nfive");
        press(&mut editor, "alt-up alt-down");
        assert_eq!(marked(&editor), "one two|\nthree four\nfive");
        press(&mut editor, "alt-down");
        assert_eq!(marked(&editor), "one two\nthree four|\nfive");
        press(&mut editor, "alt-down alt-down");
        assert_eq!(marked(&editor), "one two\nthree four\nfive|");
        press(&mut editor, "cmd-up");
        assert_eq!(editor.cursor_offset(), 0);
        press(&mut editor, "cmd-down");
        assert_eq!(editor.cursor_offset(), editor.text().len());
        press(&mut editor, "up ctrl-a");
        assert_eq!(marked(&editor), "one two\n|three four\nfive");
        press(&mut editor, "ctrl-e");
        assert_eq!(marked(&editor), "one two\nthree four|\nfive");
    }

    #[test]
    fn soft_wrap_breaks_at_spaces() {
        let mut editor = BuiltinEditor::new("the quick brown fox jumps");
        // Eleven cells wide: ten for text, one kept for the cursor. The
        // space at each wrap stays on the upper row.
        let view = editor.view(11, 5);
        assert_eq!(rows(&view), ["the quick ", "brown fox ", "jumps"]);
        assert_eq!(view.total_rows, 3);
        assert_eq!(view.first_row, 0);
        assert_eq!(view.cursor, Some((0, 0)));
        // One cell narrower, the space at the wrap sits in the cursor's
        // column.
        let view = editor.view(10, 5);
        assert_eq!(rows(&view), ["the quick ", "brown fox ", "jumps"]);
        assert_eq!(rows(&editor.view(9, 5))[0], "the ");
        // More spaces than fit hang off the row and are not drawn, but they
        // are still in the text.
        let mut editor = BuiltinEditor::new("the quick    brown fox");
        assert_eq!(rows(&editor.view(10, 5)), ["the quick ", "brown fox"]);
        assert_eq!(editor.text(), "the quick    brown fox");
    }

    #[test]
    fn soft_wrap_splits_only_a_word_wider_than_the_row() {
        let mut editor = BuiltinEditor::new("see abcdefghij now");
        assert_eq!(
            rows(&editor.view(5, 9)),
            ["see ", "abcd", "efgh", "ij ", "now"]
        );
        let mut editor = BuiltinEditor::new("    indented\n\nfmt      ok");
        assert_eq!(
            rows(&editor.view(20, 9)),
            ["    indented", "", "fmt      ok"]
        );
        for width in 1..30 {
            let mut editor = BuiltinEditor::new(
                "Legal accepted everything except clause 7.2. Their counter-proposal is attached.",
            );
            let view = editor.view(width, 100);
            let all = rows(&view);
            for row in &all {
                assert!(text::width(row) <= width, "{width}: {row:?}");
            }
            // Only spaces hanging off a row are left undrawn.
            let drawn: String = all.concat();
            assert_eq!(
                drawn.replace(' ', ""),
                editor.text().replace(' ', ""),
                "{width}"
            );
        }
    }

    #[test]
    fn wide_characters_are_never_split_at_a_wrap() {
        let mut editor = BuiltinEditor::new("ab日本語");
        // Five cells of text per row: 日 fits after "ab", 本 would not.
        let view = editor.view(6, 4);
        assert_eq!(rows(&view), ["ab日", "本語"]);
        press(&mut editor, "cmd-down");
        assert_eq!(editor.view(6, 4).cursor, Some((4, 1)));
        press(&mut editor, "left");
        assert_eq!(editor.view(6, 4).cursor, Some((2, 1)));
        // An emoji sequence is one cluster, two cells, and stays whole.
        let mut editor = BuiltinEditor::new("abc👨‍👩‍👧👨‍👩‍👧d");
        assert_eq!(rows(&editor.view(5, 4)), ["abc", "👨‍👩‍👧👨‍👩‍👧", "d"]);
        // Too narrow for a wide character at all: nothing of it is drawn.
        let mut editor = BuiltinEditor::new("日本");
        for width in 1..4 {
            for row in rows(&editor.view(width, 4)) {
                assert!(text::width(&row) <= width);
            }
        }
        assert_eq!(rows(&editor.view(1, 4)), ["", ""]);
    }

    #[test]
    fn the_cursor_column_counts_cells_not_bytes() {
        let mut editor = BuiltinEditor::new("日本e\u{301}👍🏽x");
        press(&mut editor, "right");
        assert_eq!(editor.view(40, 2).cursor, Some((2, 0)));
        press(&mut editor, "right right");
        assert_eq!(editor.view(40, 2).cursor, Some((5, 0)));
        press(&mut editor, "right");
        assert_eq!(editor.view(40, 2).cursor, Some((7, 0)));
    }

    #[test]
    fn cmd_arrows_go_to_the_ends_of_the_visual_row() {
        let mut editor = BuiltinEditor::new("the quick brown fox jumps\nnext");
        editor.view(11, 5);
        press(&mut editor, "down cmd-left");
        assert_eq!(marked(&editor), "the quick |brown fox jumps\nnext");
        assert_eq!(editor.view(11, 5).cursor, Some((0, 1)));
        // The end of a wrapped row is the same offset as the start of the
        // next. After cmd-right the cursor is drawn at the end of this row.
        press(&mut editor, "cmd-right");
        assert_eq!(marked(&editor), "the quick brown fox |jumps\nnext");
        assert_eq!(editor.view(11, 5).cursor, Some((10, 1)));
        // Again changes nothing.
        press(&mut editor, "cmd-right end");
        assert_eq!(editor.view(11, 5).cursor, Some((10, 1)));
        press(&mut editor, "home");
        assert_eq!(marked(&editor), "the quick |brown fox jumps\nnext");
        // Moving on from the row's end is by offset, onto the next row.
        press(&mut editor, "end right");
        assert_eq!(marked(&editor), "the quick brown fox j|umps\nnext");
        assert_eq!(editor.view(11, 5).cursor, Some((1, 2)));
        // The last row of a paragraph ends at the line break.
        press(&mut editor, "cmd-right");
        assert_eq!(marked(&editor), "the quick brown fox jumps|\nnext");
        assert_eq!(editor.view(11, 5).cursor, Some((5, 2)));
        // Selecting to the row's start and end.
        press(&mut editor, "up cmd-left shift-cmd-right");
        assert_eq!(editor.copy().as_deref(), Some("brown fox "));
        press(&mut editor, "right shift-end");
        assert_eq!(marked(&editor), "the quick brown fox ^jumps|\nnext");
        press(&mut editor, "shift-home");
        assert_eq!(marked(&editor), "the quick brown fox |jumps\nnext");
    }

    #[test]
    fn a_word_split_across_rows_has_two_ends_at_one_offset() {
        let mut editor = BuiltinEditor::new("abcdefghij");
        assert_eq!(rows(&editor.view(5, 5)), ["abcd", "efgh", "ij"]);
        press(&mut editor, "cmd-right");
        assert_eq!(editor.cursor_offset(), 4);
        assert_eq!(editor.view(5, 5).cursor, Some((4, 0)));
        // Left then right comes back to the same offset, now drawn at the
        // start of the lower row.
        press(&mut editor, "left right");
        assert_eq!(editor.cursor_offset(), 4);
        assert_eq!(editor.view(5, 5).cursor, Some((0, 1)));
        press(&mut editor, "cmd-right");
        assert_eq!(editor.cursor_offset(), 8);
        press(&mut editor, "cmd-left");
        assert_eq!(editor.cursor_offset(), 4);
        // Typing at the upper row's end puts the character at that offset.
        press(&mut editor, "cmd-up cmd-right X");
        assert_eq!(editor.text(), "abcdXefghij");
    }

    #[test]
    fn up_and_down_keep_the_preferred_column_across_short_lines() {
        let mut editor = BuiltinEditor::new("abcdefgh\nab\n\nabcdefgh");
        editor.view(40, 10);
        press(&mut editor, "right right right right right right down");
        assert_eq!(marked(&editor), "abcdefgh\nab|\n\nabcdefgh");
        press(&mut editor, "down");
        assert_eq!(marked(&editor), "abcdefgh\nab\n|\nabcdefgh");
        press(&mut editor, "ctrl-n");
        assert_eq!(marked(&editor), "abcdefgh\nab\n\nabcdef|gh");
        press(&mut editor, "up up ctrl-p");
        assert_eq!(marked(&editor), "abcdef|gh\nab\n\nabcdefgh");
        // A sideways movement forgets the column.
        press(&mut editor, "down left down down");
        assert_eq!(marked(&editor), "abcdefgh\nab\n\na|bcdefgh");
        // Off the top is the start of the text; off the bottom, its end.
        press(&mut editor, "down");
        assert_eq!(marked(&editor), "abcdefgh\nab\n\nabcdefgh|");
        press(&mut editor, "up up up up up");
        assert_eq!(marked(&editor), "|abcdefgh\nab\n\nabcdefgh");
    }

    #[test]
    fn up_and_down_move_through_wrapped_rows() {
        let mut editor = BuiltinEditor::new("aaaa bbbb cccc dddd eeee\nxy\n日本語日本");
        assert_eq!(
            rows(&editor.view(11, 9)),
            ["aaaa bbbb ", "cccc dddd ", "eeee", "xy", "日本語日本"]
        );
        editor.set_cursor_offset(7);
        press(&mut editor, "down");
        assert_eq!(editor.cursor_offset(), 17);
        assert_eq!(editor.view(11, 9).cursor, Some((7, 1)));
        // "eeee" is short: the cursor goes to its end and remembers 7.
        press(&mut editor, "down");
        assert_eq!(editor.cursor_offset(), 24);
        press(&mut editor, "down");
        assert_eq!(editor.view(11, 9).cursor, Some((2, 3)));
        // Cell 7 is the right half of the fourth wide character: the cursor
        // goes in front of it.
        press(&mut editor, "down");
        assert_eq!(editor.view(11, 9).cursor, Some((6, 4)));
        press(&mut editor, "up up up");
        assert_eq!(editor.cursor_offset(), 17);
        press(&mut editor, "up");
        assert_eq!(editor.cursor_offset(), 7);
        // A column past the end of a wrapped row lands at that row's end.
        press(&mut editor, "down down cmd-right up");
        assert_eq!(editor.view(11, 9).cursor, Some((4, 1)));
        press(&mut editor, "cmd-down cmd-up cmd-right down");
        assert_eq!(editor.view(11, 9).cursor, Some((10, 1)));
        assert_eq!(editor.cursor_offset(), 20);
    }

    #[test]
    fn page_keys_move_by_a_screenful() {
        let text: Vec<String> = (0..100).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        let view = editor.view(20, 10);
        assert_eq!((view.first_row, view.total_rows), (0, 100));
        press(&mut editor, "right right pagedown");
        let view = editor.view(20, 10);
        // Nine rows down, and the view with it: the cursor keeps its row.
        assert_eq!(view.first_row, 9);
        assert_eq!(view.cursor, Some((2, 0)));
        assert_eq!(rows(&view)[0], "line 9");
        press(&mut editor, "pagedown pagedown");
        assert_eq!(editor.view(20, 10).first_row, 27);
        press(&mut editor, "pageup");
        let view = editor.view(20, 10);
        assert_eq!((view.first_row, view.cursor), (18, Some((2, 0))));
        press(&mut editor, "shift-pageup");
        assert_eq!(editor.copy().unwrap().lines().count(), 10);
        press(&mut editor, "pageup pageup pageup");
        assert_eq!(editor.cursor_offset(), 0);
        press(&mut editor, "cmd-down pagedown");
        assert_eq!(editor.cursor_offset(), editor.text().len());
        assert_eq!(editor.view(20, 10).first_row, 90);
    }

    #[test]
    fn shift_extends_the_selection_in_both_directions() {
        let mut editor = BuiltinEditor::new("one two three");
        press(&mut editor, "alt-right shift-right shift-alt-right");
        assert_eq!(marked(&editor), "one^ two| three");
        assert_eq!(editor.selection(), Some(3..7));
        // Back past the anchor: the selection turns around.
        press(&mut editor, "shift-alt-left shift-alt-left");
        assert_eq!(marked(&editor), "|one^ two three");
        press(&mut editor, "shift-right");
        assert_eq!(marked(&editor), "o|ne^ two three");
        // Shrunk to nothing, nothing is selected.
        press(&mut editor, "shift-right shift-right");
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.copy(), None);
        // Left and right without shift go to that end of the selection.
        press(&mut editor, "shift-cmd-right left");
        assert_eq!(marked(&editor), "one| two three");
        press(&mut editor, "shift-alt-right shift-alt-right right");
        assert_eq!(marked(&editor), "one two three|");
        // Any other movement without shift drops the selection and moves.
        press(&mut editor, "shift-alt-left alt-left");
        assert_eq!(marked(&editor), "one |two three");
        press(&mut editor, "shift-down");
        assert_eq!(marked(&editor), "one ^two three|");
        press(&mut editor, "cmd-a");
        assert_eq!(editor.selection(), Some(0..13));
    }

    #[test]
    fn typing_deleting_and_cutting_replace_the_selection() {
        // Selected forwards, then typed over.
        let mut editor = BuiltinEditor::new("one two three");
        press(&mut editor, "alt-right right shift-alt-right x");
        assert_eq!(marked(&editor), "one x| three");
        // Selected backwards, then pasted over.
        press(&mut editor, "cmd-right shift-alt-left");
        editor.insert("3\n4");
        assert_eq!(marked(&editor), "one x 3\n4|");
        // Backspace and delete take the selection and nothing more.
        press(&mut editor, "shift-left shift-left backspace");
        assert_eq!(marked(&editor), "one x 3|");
        press(&mut editor, "shift-alt-left shift-left delete");
        assert_eq!(marked(&editor), "one x|");
        press(&mut editor, "shift-cmd-left alt-backspace");
        assert_eq!(marked(&editor), "|");

        // Copy leaves the text alone; cut takes it.
        let mut editor = BuiltinEditor::new("one two\nthree");
        press(&mut editor, "alt-right shift-down");
        assert_eq!(
            editor.key("cmd-c".parse().unwrap()),
            Outcome::Copy(" two\nthr".to_owned())
        );
        assert_eq!(marked(&editor), "one^ two\nthr|ee");
        assert_eq!(
            editor.key("cmd-x".parse().unwrap()),
            Outcome::Copy(" two\nthr".to_owned())
        );
        assert_eq!(marked(&editor), "one|ee");
        // With nothing selected both keys are taken and do nothing.
        assert_eq!(editor.key("cmd-c".parse().unwrap()), Outcome::Handled);
        assert_eq!(editor.key("cmd-x".parse().unwrap()), Outcome::Handled);
        assert_eq!(editor.text(), "oneee");
        // Enter replaces the selection as well.
        press(&mut editor, "cmd-a enter");
        assert_eq!(editor.text(), "\n");
    }

    #[test]
    fn deleting_by_word_and_to_line_ends() {
        let mut editor = BuiltinEditor::new("keep this, drop that\nnext line");
        press(&mut editor, "alt-right alt-right alt-delete");
        assert_eq!(marked(&editor), "keep this| that\nnext line");
        press(&mut editor, "alt-backspace");
        assert_eq!(marked(&editor), "keep | that\nnext line");
        press(&mut editor, "ctrl-e ctrl-w");
        assert_eq!(marked(&editor), "keep  |\nnext line");
        press(&mut editor, "ctrl-a ctrl-d ctrl-d");
        assert_eq!(marked(&editor), "|ep  \nnext line");
        // ctrl-k takes the rest of the line, then the line break.
        press(&mut editor, "right ctrl-k");
        assert_eq!(marked(&editor), "e|\nnext line");
        press(&mut editor, "ctrl-k");
        assert_eq!(marked(&editor), "e|next line");
        press(&mut editor, "alt-right cmd-backspace");
        assert_eq!(marked(&editor), "| line");
        press(&mut editor, "cmd-backspace right cmd-delete");
        assert_eq!(marked(&editor), " |");
        // Backspace at the start of a line joins it to the one above, and
        // word deletion there takes the line break and the word before it.
        let mut editor = BuiltinEditor::new("one two\nthree");
        press(&mut editor, "down backspace");
        assert_eq!(marked(&editor), "one two|three");
        press(&mut editor, "enter alt-backspace");
        assert_eq!(marked(&editor), "one |three");
        press(&mut editor, "cmd-down delete");
        assert_eq!(editor.text(), "one three");
    }

    #[test]
    fn cmd_backspace_deletes_to_the_start_of_the_visual_row() {
        let mut editor = BuiltinEditor::new("the quick brown fox jumps");
        editor.view(11, 5);
        press(&mut editor, "down alt-right cmd-backspace");
        assert_eq!(marked(&editor), "the quick | fox jumps");
    }

    #[test]
    fn tab_is_the_editors_only_when_the_caller_says_so() {
        let mut editor = BuiltinEditor::new("ab");
        let tab: KeyChord = "tab".parse().unwrap();
        let back_tab: KeyChord = "shift-tab".parse().unwrap();
        assert_eq!(editor.key(tab), Outcome::Ignored);
        assert_eq!(editor.key(back_tab), Outcome::Ignored);
        assert_eq!(editor.text(), "ab");
        editor.tab_inserts = true;
        assert_eq!(editor.key(back_tab), Outcome::Ignored);
        // To the next multiple of four, in cells.
        assert_eq!(editor.key(tab), Outcome::Handled);
        assert_eq!(marked(&editor), "    |ab");
        press(&mut editor, "right tab");
        assert_eq!(marked(&editor), "    a   |b");
        press(&mut editor, "end 日 tab");
        assert_eq!(marked(&editor), "    a   b日 |");
        press(&mut editor, "enter x y z w tab");
        assert_eq!(editor.text(), "    a   b日 \nxyzw    ");
        // One undo step per tab.
        press(&mut editor, "cmd-z");
        assert_eq!(editor.text(), "    a   b日 \nxyzw");
    }

    #[test]
    fn keys_that_are_not_editing_are_left_for_commands() {
        let mut editor = BuiltinEditor::new("x");
        press(&mut editor, "y");
        for key in [
            "esc",
            "tab",
            "shift-tab",
            "cmd-v",
            "cmd-f",
            "cmd-s",
            "cmd-enter",
            "ctrl-enter",
            "ctrl-c",
            "ctrl-u",
            "alt-x",
            "cmd-alt-left",
            "ctrl-left",
            "cmd-b",
            "f1",
        ] {
            let chord: KeyChord = key.parse().unwrap();
            assert_eq!(editor.key(chord), Outcome::Ignored, "{key}");
        }
        assert_eq!(marked(&editor), "y|x");
        // An ignored key does not end a run of typing either.
        press(&mut editor, "z cmd-z");
        assert_eq!(editor.text(), "x");
    }

    #[test]
    fn a_run_of_typing_is_one_undo_step() {
        let mut editor = BuiltinEditor::default();
        type_text(&mut editor, "hello world");
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "|");
        press(&mut editor, "cmd-shift-z");
        assert_eq!(marked(&editor), "hello world|");
        // Nothing more to redo, and nothing happens.
        press(&mut editor, "cmd-shift-z");
        assert_eq!(marked(&editor), "hello world|");
        // Shift is normally folded into the letter. A chord that still
        // carries it means the same.
        let mut redo: KeyChord = "cmd-z".parse().unwrap();
        redo.mods.shift = true;
        press(&mut editor, "cmd-z");
        assert_eq!(editor.key(redo), Outcome::Handled);
        assert_eq!(marked(&editor), "hello world|");

        // A movement starts a new step, even one that ends where it began.
        type_text(&mut editor, "!!");
        press(&mut editor, "left right");
        type_text(&mut editor, "??");
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "hello world!!|");
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "hello world|");

        // A line break is a step of its own, and so is a paste.
        type_text(&mut editor, "\nab");
        editor.insert("PASTED");
        type_text(&mut editor, "cd");
        for expected in [
            "hello world\nabPASTED|",
            "hello world\nab|",
            "hello world\n|",
            "hello world|",
            "|",
            "|",
        ] {
            press(&mut editor, "cmd-z");
            assert_eq!(marked(&editor), expected);
        }
        assert!(!editor.undo());
        for expected in [
            "hello world|",
            "hello world\n|",
            "hello world\nab|",
            "hello world\nabPASTED|",
            "hello world\nabPASTEDcd|",
        ] {
            assert!(editor.redo());
            assert_eq!(marked(&editor), expected);
        }
        assert!(!editor.redo());
    }

    #[test]
    fn runs_of_deletion_are_one_undo_step_each() {
        let mut editor = BuiltinEditor::new("abcdefgh");
        press(&mut editor, "cmd-right backspace backspace backspace");
        assert_eq!(marked(&editor), "abcde|");
        press(&mut editor, "cmd-left delete delete");
        assert_eq!(marked(&editor), "|cde");
        // A deletion after typing is a new step, and typing after it too.
        type_text(&mut editor, "xy");
        press(&mut editor, "backspace");
        type_text(&mut editor, "z");
        for expected in ["x|cde", "xy|cde", "|cde", "|abcde", "abcdefgh|"] {
            press(&mut editor, "cmd-z");
            assert_eq!(marked(&editor), expected);
        }
        for expected in ["abcde|", "|cde", "xy|cde", "x|cde", "xz|cde"] {
            press(&mut editor, "cmd-shift-z");
            assert_eq!(marked(&editor), expected);
        }
        // Word deletions are never joined.
        let mut editor = BuiltinEditor::new("one two three");
        press(&mut editor, "cmd-right alt-backspace alt-backspace cmd-z");
        assert_eq!(marked(&editor), "one two |");
    }

    #[test]
    fn undo_restores_the_cursor_and_the_selection() {
        let mut editor = BuiltinEditor::new("hello world");
        press(&mut editor, "cmd-right shift-alt-left x");
        assert_eq!(marked(&editor), "hello x|");
        press(&mut editor, "cmd-up cmd-z");
        assert_eq!(marked(&editor), "hello |world^");
        press(&mut editor, "cmd-shift-z");
        assert_eq!(marked(&editor), "hello x|");
        // A forward selection comes back forwards.
        press(&mut editor, "cmd-up shift-alt-right cmd-x cmd-down cmd-z");
        assert_eq!(marked(&editor), "^hello| x");
        // A new edit after an undo throws the undone steps away.
        press(&mut editor, "right y cmd-shift-z");
        assert_eq!(marked(&editor), "helloy| x");
    }

    #[test]
    fn modified_follows_undo_back_to_the_saved_text() {
        let mut editor = BuiltinEditor::new("draft");
        assert!(!editor.is_modified());
        press(&mut editor, "cmd-right");
        assert!(!editor.is_modified());
        type_text(&mut editor, " one");
        assert!(editor.is_modified());
        editor.mark_saved();
        assert!(!editor.is_modified());
        // Typing straight on after a save is a new step, so that undo can
        // stop at the saved text.
        type_text(&mut editor, " two");
        assert!(editor.is_modified());
        press(&mut editor, "cmd-z");
        assert_eq!(editor.text(), "draft one");
        assert!(!editor.is_modified());
        // The history survives the save: undo goes back past it.
        press(&mut editor, "cmd-z");
        assert_eq!(editor.text(), "draft");
        assert!(editor.is_modified());
        press(&mut editor, "cmd-shift-z");
        assert!(!editor.is_modified());
        press(&mut editor, "cmd-shift-z");
        assert!(editor.is_modified());
        // Undone to the saved text and then changed some other way: the
        // saved text is gone from the history, and it stays modified.
        press(&mut editor, "cmd-z cmd-z x");
        assert!(editor.is_modified());
        press(&mut editor, "cmd-z");
        assert_eq!(editor.text(), "draft");
        assert!(editor.is_modified());
        // Loading a text is a fresh start.
        editor.set_text("other");
        assert!(!editor.is_modified());
        assert!(!editor.undo());
    }

    #[test]
    fn control_characters_never_get_in() {
        let mut editor = BuiltinEditor::default();
        editor.insert("one\r\ntwo\rthree\tfour\u{1b}[2J\u{7}\u{0}\u{9b}\u{2028}five");
        assert_eq!(editor.text(), "one\ntwo\nthree    four[2J\nfive");
        editor.set_text("a\r\nb\u{8}\tc");
        assert_eq!(editor.text(), "a\nb    c");
        editor.paste_as_quotation("x\r\ny\u{1b}");
        editor.insert_signature("Sam\u{7}\r\nLee");
        assert!(
            !editor
                .text()
                .contains(|c: char| c.is_control() && c != '\n')
        );
        // A control character as a key is not text either.
        let mut editor = BuiltinEditor::default();
        editor.key(KeyChord::plain(Key::Char('\u{1b}')));
        editor.key(KeyChord::plain(Key::Char('\t')));
        assert_eq!(editor.text(), "    ");
    }

    #[test]
    fn the_view_scrolls_to_keep_the_cursor_in_sight() {
        let text: Vec<String> = (0..50).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        for (width, height) in [(20, 5), (8, 3), (3, 2), (2, 7), (1, 1)] {
            press(&mut editor, "cmd-up");
            let mut last_first = 0;
            loop {
                let view = editor.view(width, height);
                assert!(view.lines.len() <= height);
                assert_eq!(view.lines.len(), height.min(view.total_rows));
                for row in rows(&view) {
                    assert!(text::width(&row) <= width, "{width}x{height}: {row:?}");
                }
                let (column, row) = view.cursor.expect("the cursor is always in sight");
                assert!(usize::from(column) < width && usize::from(row) < height);
                // Going down, the view only ever moves down, a row at a time.
                assert!(view.first_row >= last_first && view.first_row <= last_first + 1);
                last_first = view.first_row;
                if editor.cursor_offset() == editor.text().len() {
                    assert_eq!(view.first_row + height, view.total_rows);
                    break;
                }
                press(&mut editor, "down");
            }
            // And back up: the cursor leads the view to the top.
            while editor.cursor_offset() > 0 {
                press(&mut editor, "up");
                assert!(editor.view(width, height).cursor.is_some());
            }
            assert_eq!(editor.view(width, height).first_row, 0);
        }
        // Wide enough for a whole line, the rows are the lines.
        press(&mut editor, "cmd-down");
        let view = editor.view(20, 3);
        assert_eq!(rows(&view), ["line 47", "line 48", "line 49"]);
        assert_eq!(view.cursor, Some((7, 2)));
        assert_eq!((view.first_row, view.total_rows), (47, 50));
    }

    #[test]
    fn an_area_with_no_cells_draws_nothing() {
        let mut editor = BuiltinEditor::new("some text\nand more");
        for (width, height) in [(0, 0), (0, 5), (5, 0)] {
            assert_eq!(editor.view(width, height), EditorView::default());
        }
        // The editor still works, by the size it last had (or its default).
        press(&mut editor, "down end x");
        assert_eq!(editor.text(), "some text\nand morex");
        // An empty text is one empty row with the cursor on it.
        let mut editor = BuiltinEditor::default();
        let view = editor.view(1, 1);
        assert_eq!(rows(&view), [""]);
        assert_eq!(view.cursor, Some((0, 0)));
        assert_eq!((view.first_row, view.total_rows), (0, 1));
        // In one cell the cursor has nowhere to go but that cell.
        press(&mut editor, "a b");
        let view = editor.view(1, 1);
        assert_eq!(view.cursor, Some((0, 0)));
        assert_eq!(view.total_rows, 2);
    }

    #[test]
    fn a_full_row_leaves_the_last_column_for_the_cursor() {
        let mut editor = BuiltinEditor::new("abcd");
        press(&mut editor, "cmd-right");
        let view = editor.view(5, 2);
        assert_eq!(rows(&view), ["abcd"]);
        assert_eq!(view.cursor, Some((4, 0)));
        // One more letter and the word no longer fits on a row.
        press(&mut editor, "e");
        let view = editor.view(5, 2);
        assert_eq!(rows(&view), ["abcd", "e"]);
        assert_eq!(view.cursor, Some((1, 1)));
        // In spaces hanging off the row, the cursor waits in the last column.
        let mut editor = BuiltinEditor::new("abcd      x");
        press(&mut editor, "alt-right right right right right");
        let view = editor.view(5, 3);
        assert_eq!(rows(&view), ["abcd ", "x"]);
        assert_eq!(view.cursor, Some((4, 0)));
    }

    #[test]
    fn the_wheel_scrolls_the_view_and_leaves_the_cursor() {
        let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        editor.view(20, 5);
        editor.scroll(3);
        let view = editor.view(20, 5);
        assert_eq!(rows(&view)[0], "line 3");
        assert_eq!(view.first_row, 3);
        // The cursor is where it was, which is now out of sight.
        assert_eq!(view.cursor, None);
        assert_eq!(editor.cursor_offset(), 0);
        editor.scroll(-1);
        editor.scroll(-1);
        assert_eq!(editor.view(20, 5).first_row, 1);
        // It stops at both ends.
        editor.scroll(-10);
        let view = editor.view(20, 5);
        assert_eq!((view.first_row, view.cursor), (0, Some((0, 0))));
        editor.scroll(1000);
        assert_eq!(editor.view(20, 5).first_row, 25);
        editor.scroll(isize::MAX);
        editor.scroll(isize::MIN);
        assert_eq!(editor.view(20, 5).first_row, 0);
        // Using the cursor brings the view back to it.
        editor.scroll(12);
        press(&mut editor, "x");
        let view = editor.view(20, 5);
        assert_eq!((view.first_row, view.cursor), (0, Some((1, 0))));
        editor.scroll(12);
        press(&mut editor, "down");
        let view = editor.view(20, 5);
        assert_eq!((view.first_row, view.cursor), (1, Some((1, 0))));
        // Selecting everything does not scroll.
        press(&mut editor, "cmd-a");
        assert_eq!(editor.view(20, 5).first_row, 1);
    }

    #[test]
    fn quote_levels_headings_and_fences_colour_whole_lines() {
        let source = "# Plan\n\
                      #not a heading\n\
                      > quoted *text*\n\
                      >> deeper\n\
                      > > > deepest\n\
                      ```rust\n\
                      let x = **1**;\n\
                      > not a quote\n\
                      ```\n\
                      after\n\
                      ~~~\n\
                      ```\n\
                      ~~~\n\
                      done";
        let mut editor = BuiltinEditor::new(source);
        let view = editor.view(40, 20);
        let whole = |row: usize| {
            let spans = kinds(&view, row);
            assert_eq!(spans.len(), 1, "row {row}: {spans:?}");
            spans[0].1
        };
        assert_eq!(whole(0), SpanKind::Heading);
        assert_eq!(whole(1), SpanKind::Text);
        assert_eq!(whole(2), SpanKind::Quote(1));
        assert_eq!(whole(3), SpanKind::Quote(2));
        assert_eq!(whole(4), SpanKind::Quote(3));
        for row in 5..9 {
            assert_eq!(whole(row), SpanKind::Code, "row {row}");
        }
        assert_eq!(whole(9), SpanKind::Text);
        // A tilde fence is closed by tildes, not by backticks inside it.
        for row in 10..13 {
            assert_eq!(whole(row), SpanKind::Code, "row {row}");
        }
        assert_eq!(whole(13), SpanKind::Text);
        // Highlighting is display only.
        assert_eq!(rows(&view).join("\n"), source);
        assert_eq!(editor.text(), source);

        // A fence opened above the view still colours what is in view.
        press(&mut editor, "cmd-down up up up up up");
        let view = editor.view(40, 3);
        assert_eq!(rows(&view)[0], "let x = **1**;");
        assert_eq!(kinds(&view, 0), [("let x = **1**;", SpanKind::Code)]);
    }

    #[test]
    fn inline_markdown_is_marked() {
        let spans = |line: &str| -> Vec<(String, SpanKind)> {
            let mut editor = BuiltinEditor::new(line);
            let view = editor.view(200, 1);
            assert_eq!(rows(&view), [line]);
            kinds(&view, 0)
                .into_iter()
                .filter(|(_, kind)| *kind != SpanKind::Text)
                .map(|(text, kind)| (text.to_owned(), kind))
                .collect()
        };
        let one = |text: &str, kind: SpanKind| vec![(text.to_owned(), kind)];
        assert_eq!(
            spans("a **strong** word"),
            one("**strong**", SpanKind::Strong)
        );
        assert_eq!(
            spans("an *emphatic* word"),
            one("*emphatic*", SpanKind::Emphasis)
        );
        assert_eq!(
            spans("an _emphatic_ word"),
            one("_emphatic_", SpanKind::Emphasis)
        );
        assert_eq!(
            spans("run `cargo test` now"),
            one("`cargo test`", SpanKind::Code)
        );
        assert_eq!(
            spans("see [the spec](https://example.com/spec) first"),
            one("[the spec](https://example.com/spec)", SpanKind::Link)
        );
        assert_eq!(
            spans("at https://example.com/a?b=1, or mailto:ana@example.com."),
            [
                ("https://example.com/a?b=1".to_owned(), SpanKind::Link),
                ("mailto:ana@example.com".to_owned(), SpanKind::Link)
            ]
        );
        assert_eq!(
            spans("**one** *two* `three` <http://example.com>"),
            [
                ("**one**".to_owned(), SpanKind::Strong),
                ("*two*".to_owned(), SpanKind::Emphasis),
                ("`three`".to_owned(), SpanKind::Code),
                ("http://example.com".to_owned(), SpanKind::Link)
            ]
        );
        // Code wins over what is inside it, and non-ASCII text is fine.
        assert_eq!(spans("`a *b* c`"), one("`a *b* c`", SpanKind::Code));
        assert_eq!(
            spans("très **важно** 日本"),
            one("**важно**", SpanKind::Strong)
        );
        // Things that only look like markup.
        for plain in [
            "2 * 3 * 4",
            "* a list item",
            "snake_case_name and another_one",
            "a * b* c",
            "unclosed *emphasis",
            "unclosed **strong",
            "unclosed `code",
            "[text] (not a link)",
            "[unclosed](link",
            "https://",
            "xhttps://example.com",
            "**",
            "``",
            "_",
            "",
        ] {
            assert_eq!(spans(plain), [], "{plain:?}");
        }
    }

    #[test]
    fn markdown_marks_carry_across_soft_wraps() {
        let mut editor = BuiltinEditor::new("say **very strong words** now");
        let view = editor.view(11, 5);
        assert_eq!(rows(&view), ["say **very ", "strong ", "words** ", "now"]);
        assert_eq!(
            kinds(&view, 0),
            [("say ", SpanKind::Text), ("**very ", SpanKind::Strong)]
        );
        assert_eq!(kinds(&view, 1), [("strong ", SpanKind::Strong)]);
        assert_eq!(
            kinds(&view, 2),
            [("words**", SpanKind::Strong), (" ", SpanKind::Text)]
        );
    }

    #[test]
    fn a_long_line_of_unmatched_markup_is_scanned_once() {
        // Each of these would take minutes if every opener searched the rest
        // of the line for its closer.
        for unit in ["* ", "*a ", "[x] ", "[x](y ", "_a ", "**b "] {
            let line = unit.repeat(100_000);
            let started = Instant::now();
            let found = inline_marks(&line);
            assert_eq!(found, [], "{unit:?}");
            assert!(started.elapsed() < Duration::from_secs(5), "{unit:?}");
        }
    }

    #[test]
    fn the_selection_is_marked_in_the_view() {
        let selected = |view: &EditorView| -> Vec<String> {
            view.lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .filter(|span| span.selected)
                        .map(|span| span.text.as_str())
                        .collect()
                })
                .collect()
        };
        let mut editor = BuiltinEditor::new("one **two** three\n\nfour");
        press(&mut editor, "alt-right shift-alt-right shift-right");
        let view = editor.view(40, 5);
        assert_eq!(selected(&view), [" **two*", "", ""]);
        // The selection cuts the strong span without changing its kind.
        assert_eq!(
            kinds(&view, 0),
            [
                ("one", SpanKind::Text),
                (" ", SpanKind::Text),
                ("**two*", SpanKind::Strong),
                ("*", SpanKind::Strong),
                (" three", SpanKind::Text)
            ]
        );
        // A selected line break shows as a selected cell, so the empty line
        // in the middle can be seen to be selected.
        press(&mut editor, "shift-cmd-down");
        let view = editor.view(40, 5);
        assert_eq!(selected(&view), [" **two** three ", " ", "four"]);
        assert_eq!(editor.text(), "one **two** three\n\nfour");
        // Across a soft wrap there is no line break to show.
        let mut editor = BuiltinEditor::new("the quick brown fox");
        press(&mut editor, "cmd-a");
        assert_eq!(selected(&editor.view(11, 5)), ["the quick ", "brown fox"]);
        let mut editor = BuiltinEditor::new("abcd\nabcd");
        press(&mut editor, "cmd-a");
        assert_eq!(selected(&editor.view(4, 5)), ["abc", "d ", "abc", "d"]);
        assert_eq!(selected(&editor.view(5, 5)), ["abcd ", "abcd"]);
        // On a row that is full there is no cell to show the line break in.
        let mut editor = BuiltinEditor::new("abc \nx");
        press(&mut editor, "cmd-a");
        assert_eq!(selected(&editor.view(4, 5)), ["abc ", "x"]);
    }

    #[test]
    fn misspellings_are_marked_and_follow_the_text() {
        let misspelled = |editor: &mut BuiltinEditor| -> Vec<String> {
            editor
                .view(80, 10)
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .filter(|span| span.misspelled)
                .map(|span| span.text.clone())
                .collect()
        };
        let source = "> teh quick\nthe brwon fox\nso jumsp over";
        let mut editor = BuiltinEditor::new(source);
        // Out of order, one empty, one past the end, one overlapping.
        editor.set_misspellings(vec![29..34, 2..5, 16..21, 9..9, 17..19, 400..410]);
        assert_eq!(editor.misspellings(), [2..5, 16..21, 29..34]);
        assert_eq!(misspelled(&mut editor), ["teh", "brwon", "jumsp"]);
        // A misspelled word in a quotation is still part of the quotation.
        let view = editor.view(80, 10);
        assert_eq!(
            view.lines[0].spans[1],
            ViewSpan {
                text: "teh".to_owned(),
                selected: false,
                misspelled: true,
                kind: SpanKind::Quote(1)
            }
        );

        // Typing before a word on its line moves the mark with the word.
        editor.set_cursor_offset(12);
        type_text(&mut editor, "a ");
        assert_eq!(editor.misspellings(), [2..5, 18..23, 31..36]);
        assert_eq!(misspelled(&mut editor), ["teh", "brwon", "jumsp"]);
        // A new line above moves everything below it.
        press(&mut editor, "cmd-up enter");
        assert_eq!(misspelled(&mut editor), ["teh", "brwon", "jumsp"]);
        // Editing a word, or typing against it, drops its mark and no other.
        press(&mut editor, "alt-right x");
        assert_eq!(
            editor.text(),
            "\n> tehx quick\na the brwon fox\nso jumsp over"
        );
        assert_eq!(misspelled(&mut editor), ["brwon", "jumsp"]);
        // Joining lines carries the marks of the second onto the first, and
        // splitting them again carries them back.
        press(&mut editor, "down down cmd-left backspace");
        assert_eq!(
            editor.text(),
            "\n> tehx quick\na the brwon foxso jumsp over"
        );
        assert_eq!(misspelled(&mut editor), ["brwon", "jumsp"]);
        press(&mut editor, "cmd-z");
        assert_eq!(misspelled(&mut editor), ["brwon", "jumsp"]);
        assert_eq!(editor.misspellings(), [20..25, 33..38]);
        // Deleting text that holds a mark removes it.
        press(&mut editor, "cmd-a backspace");
        assert_eq!(editor.misspellings(), []);
        // A new list replaces the old one, and odd ranges do no harm: this
        // one starts inside a character and runs across a line break.
        editor.set_text("日本語 wrod\nnext");
        editor.set_misspellings(vec![1..6, 10..40]);
        assert_eq!(misspelled(&mut editor), ["日本", "wrod"]);
        editor.set_misspellings(Vec::new());
        assert_eq!(misspelled(&mut editor), Vec::<String>::new());
    }

    #[test]
    fn reflow_fills_a_paragraph_to_the_width() {
        let mut editor = BuiltinEditor::new(
            "First paragraph.\n\nLegal accepted everything\nexcept clause 7.2. Their counter-proposal is attached.\nSee it.\n\nLast.",
        );
        press(&mut editor, "down down down");
        editor.reflow_paragraph(30);
        assert_eq!(
            editor.text(),
            "First paragraph.\n\nLegal accepted everything\nexcept clause 7.2. Their\ncounter-proposal is attached.\nSee it.\n\nLast."
        );
        for line in editor.text().lines() {
            assert!(text::width(line) <= 30, "{line}");
        }
        // One undo step, and reflowing what is already filled is not a step.
        editor.reflow_paragraph(30);
        press(&mut editor, "cmd-z");
        assert!(editor.text().contains("attached.\nSee it."));
        assert!(editor.text().contains("Their counter-proposal"));
        assert!(!editor.undo());
        // A blank line is not a paragraph.
        press(&mut editor, "cmd-up down");
        editor.reflow_paragraph(10);
        assert!(!editor.is_modified());
        // A word wider than the width gets a line to itself, unbroken.
        let mut editor = BuiltinEditor::new("see https://example.com/a/long/path now  please");
        editor.reflow_paragraph(10);
        assert_eq!(
            editor.text(),
            "see\nhttps://example.com/a/long/path\nnow please"
        );
        editor.reflow_paragraph(0);
        assert_eq!(
            editor.text(),
            "see\nhttps://example.com/a/long/path\nnow\nplease"
        );
    }

    #[test]
    fn reflow_keeps_the_quote_prefix_on_every_line() {
        let source = "On Monday Ana wrote:\n\
                      > Legal accepted everything except clause 7.2.\n\
                      > Their counter-proposal\n\
                      > is attached.\n\
                      >\n\
                      > Thanks\n\
                      >> an older message that is quoted twice and is long\n\
                      My reply.";
        let mut editor = BuiltinEditor::new(source);
        press(&mut editor, "down down");
        editor.reflow_paragraph(24);
        assert_eq!(
            editor.text(),
            "On Monday Ana wrote:\n\
             > Legal accepted\n\
             > everything except\n\
             > clause 7.2. Their\n\
             > counter-proposal is\n\
             > attached.\n\
             >\n\
             > Thanks\n\
             >> an older message that is quoted twice and is long\n\
             My reply."
        );
        // The deeper level is a paragraph of its own, with its own prefix.
        press(&mut editor, "cmd-down up");
        editor.reflow_paragraph(24);
        assert!(editor.text().ends_with(
            "> Thanks\n\
             >> an older message that\n\
             >> is quoted twice and\n\
             >> is long\n\
             My reply."
        ));
        for line in editor.text().lines() {
            assert!(text::width(line) <= 24, "{line}");
        }
        // Reflowing wider joins the lines again.
        press(&mut editor, "cmd-up down");
        editor.reflow_paragraph(72);
        assert!(editor.text().starts_with(
            "On Monday Ana wrote:\n\
             > Legal accepted everything except clause 7.2. Their counter-proposal is\n\
             > attached.\n\
             >\n"
        ));
    }

    #[test]
    fn reflow_keeps_the_cursor_with_its_text() {
        // In the middle of a word.
        let mut editor = BuiltinEditor::new("> one two three four\n> five six");
        editor.set_cursor_offset("> one two three fo".len());
        editor.reflow_paragraph(12);
        assert_eq!(marked(&editor), "> one two\n> three fo|ur\n> five six");
        // At the start of a word that moves to the next line.
        press(&mut editor, "cmd-z");
        editor.set_cursor_offset("> one two ".len());
        editor.reflow_paragraph(12);
        assert_eq!(marked(&editor), "> one two\n> |three four\n> five six");
        // At the end of a line.
        press(&mut editor, "cmd-z ctrl-e");
        editor.reflow_paragraph(12);
        assert_eq!(marked(&editor), "> one two\n> three four|\n> five six");
        // On a later line, and undo puts it back exactly.
        press(&mut editor, "cmd-z cmd-down alt-left");
        assert_eq!(marked(&editor), "> one two three four\n> five |six");
        editor.reflow_paragraph(40);
        assert_eq!(marked(&editor), "> one two three four five |six");
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "> one two three four\n> five |six");
        press(&mut editor, "cmd-shift-z");
        assert_eq!(marked(&editor), "> one two three four five |six");
        // A signature separator is not part of the paragraph above it.
        let mut editor = BuiltinEditor::new("Thanks,\nSam\n-- \nSam Lee");
        editor.reflow_paragraph(72);
        assert_eq!(editor.text(), "Thanks, Sam\n-- \nSam Lee");
    }

    #[test]
    fn paste_as_quotation_prefixes_every_line() {
        let mut editor = BuiltinEditor::default();
        editor.paste_as_quotation("one\r\n\n> two\nthree\n");
        assert_eq!(marked(&editor), "> one\n>\n> > two\n> three|");
        // In the middle of a line the quotation gets lines of its own.
        let mut editor = BuiltinEditor::new("before after");
        press(&mut editor, "alt-right");
        editor.paste_as_quotation("quoted");
        assert_eq!(marked(&editor), "before\n> quoted\n| after");
        // One undo step.
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "before| after");
        // It replaces the selection.
        press(&mut editor, "shift-cmd-right");
        editor.paste_as_quotation("a\nb");
        assert_eq!(marked(&editor), "before\n> a\n> b|");
        // Nothing to quote changes nothing.
        editor.mark_saved();
        editor.paste_as_quotation("");
        editor.paste_as_quotation("\n");
        assert!(!editor.is_modified());
        // The quotation is drawn as one.
        let view = editor.view(40, 5);
        assert_eq!(kinds(&view, 1), [("> a", SpanKind::Quote(1))]);
    }

    #[test]
    fn the_signature_is_added_once_and_the_cursor_stays() {
        let mut editor = BuiltinEditor::new("Hi Ana,\n\nSee you there.");
        press(&mut editor, "down down alt-right shift-alt-right");
        editor.insert_signature("Sam Lee\nexample.com\n");
        assert_eq!(
            marked(&editor),
            "Hi Ana,\n\nSee^ you| there.\n\n-- \nSam Lee\nexample.com"
        );
        // Already there: nothing happens.
        editor.mark_saved();
        editor.insert_signature("Sam Lee\nexample.com");
        assert!(!editor.is_modified());
        // A different signature is a different matter.
        editor.insert_signature("Sam");
        assert!(editor.text().ends_with("example.com\n\n-- \nSam"));
        // One undo step, and the cursor does not move on undo or redo.
        press(&mut editor, "cmd-z");
        assert_eq!(
            marked(&editor),
            "Hi Ana,\n\nSee^ you| there.\n\n-- \nSam Lee\nexample.com"
        );
        press(&mut editor, "cmd-z");
        assert_eq!(marked(&editor), "Hi Ana,\n\nSee^ you| there.");
        press(&mut editor, "cmd-shift-z");
        assert_eq!(
            marked(&editor),
            "Hi Ana,\n\nSee^ you| there.\n\n-- \nSam Lee\nexample.com"
        );

        // An empty message gets a line to write on above the signature.
        let mut editor = BuiltinEditor::default();
        editor.insert_signature("Sam");
        assert_eq!(marked(&editor), "|\n\n-- \nSam");
        type_text(&mut editor, "Hello");
        assert_eq!(editor.text(), "Hello\n\n-- \nSam");
        // The blank line before it is not doubled.
        for (body, expected) in [
            ("Hi\n", "Hi\n\n-- \nSam"),
            ("Hi\n\n", "Hi\n\n-- \nSam"),
            ("Hi", "Hi\n\n-- \nSam"),
        ] {
            let mut editor = BuiltinEditor::new(body);
            press(&mut editor, "cmd-down");
            editor.insert_signature("Sam");
            assert_eq!(editor.text(), expected);
            assert_eq!(editor.cursor_offset(), body.len());
        }
        // Text that merely contains the signature's words is not it.
        let mut editor = BuiltinEditor::new("-- \nSammy");
        editor.insert_signature("Sam");
        assert_eq!(editor.text(), "-- \nSammy\n\n-- \nSam");
        editor.insert_signature("");
        editor.insert_signature("\n");
        assert_eq!(editor.text(), "-- \nSammy\n\n-- \nSam");
        // Adding the signature does not scroll the view to it.
        let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        editor.view(20, 5);
        editor.insert_signature("Sam");
        assert_eq!(editor.view(20, 5).first_row, 0);
    }

    #[test]
    fn find_selects_the_next_match_and_wraps_around() {
        let mut editor = BuiltinEditor::new("Alpha beta ALPHA\ngamma alpha");
        // A match at the cursor counts.
        assert!(editor.find("alpha", true));
        assert_eq!(marked(&editor), "^Alpha| beta ALPHA\ngamma alpha");
        assert!(editor.find("alpha", true));
        assert_eq!(marked(&editor), "Alpha beta ^ALPHA|\ngamma alpha");
        assert!(editor.find("ALPHA", true));
        assert_eq!(marked(&editor), "Alpha beta ALPHA\ngamma ^alpha|");
        // Past the last match, round to the first.
        assert!(editor.find("alpha", true));
        assert_eq!(editor.selection(), Some(0..5));
        // Backwards from the first, round to the last.
        assert!(editor.find("alpha", false));
        assert_eq!(editor.selection(), Some(23..28));
        assert!(editor.find("alpha", false));
        assert_eq!(editor.selection(), Some(11..16));
        assert!(editor.find("alpha", false));
        assert_eq!(editor.selection(), Some(0..5));
        // No match: nothing changes.
        assert!(!editor.find("delta", true));
        assert!(!editor.find("", true));
        assert_eq!(editor.selection(), Some(0..5));
        // A needle can span a line break.
        assert!(editor.find("alpha\nGAMMA", true));
        assert_eq!(editor.copy().as_deref(), Some("ALPHA\ngamma"));
        // The only match is found again from inside itself, either way.
        press(&mut editor, "left right");
        assert!(editor.find("beta", true));
        assert!(editor.find("beta", true));
        assert_eq!(editor.selection(), Some(6..10));
        assert!(editor.find("beta", false));
        assert_eq!(editor.selection(), Some(6..10));
        // Typing replaces the match, as it does any selection.
        press(&mut editor, "x");
        assert_eq!(editor.text(), "Alpha x ALPHA\ngamma alpha");
    }

    #[test]
    fn find_ignores_case_beyond_ascii_and_keeps_clusters_whole() {
        let mut editor = BuiltinEditor::new("L'ÉCOLE et l'école, 日本語 STRASSE");
        assert!(editor.find("école", true));
        assert_eq!(editor.copy().as_deref(), Some("ÉCOLE"));
        assert!(editor.find("École", true));
        assert_eq!(editor.copy().as_deref(), Some("école"));
        assert!(editor.find("本語", true));
        assert_eq!(editor.copy().as_deref(), Some("本語"));
        assert!(editor.find("strasse", false));
        assert_eq!(editor.copy().as_deref(), Some("STRASSE"));
        // "e" matches the letter under a combining accent; the selection
        // takes the accent too, since half a cluster cannot be selected.
        let mut editor = BuiltinEditor::new("caf\u{65}\u{301}s");
        assert!(editor.find("E", true));
        assert_eq!(editor.copy().as_deref(), Some("e\u{301}"));
        // The found text is scrolled into view.
        let text: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        editor.view(20, 5);
        assert!(editor.find("LINE 33", true));
        let view = editor.view(20, 5);
        assert_eq!(view.first_row, 29);
        assert_eq!(view.cursor, Some((7, 4)));
    }

    #[test]
    fn a_click_places_the_cursor_on_the_grapheme_under_it() {
        let mut editor = BuiltinEditor::new("the quick brown fox jumps\n\n日本語abc👨‍👩‍👧z");
        assert_eq!(
            rows(&editor.view(11, 6)),
            ["the quick ", "brown fox ", "jumps", "", "日本語abc", "👨‍👩‍👧z"]
        );
        editor.click(2, 1);
        assert_eq!(editor.cursor_offset(), 12);
        assert_eq!(editor.view(11, 6).cursor, Some((2, 1)));
        // Past the end of a wrapped row is that row's end, drawn there.
        editor.click(10, 0);
        assert_eq!(editor.cursor_offset(), 10);
        assert_eq!(editor.view(11, 6).cursor, Some((10, 0)));
        // The first cell of the next row is the same offset, drawn there.
        editor.click(0, 1);
        assert_eq!(editor.cursor_offset(), 10);
        assert_eq!(editor.view(11, 6).cursor, Some((0, 1)));
        // Past the end of a line, and on an empty line.
        editor.click(9, 2);
        assert_eq!(editor.cursor_offset(), 25);
        editor.click(5, 3);
        assert_eq!(editor.cursor_offset(), 26);
        // Either half of a wide character is that character.
        let wide = 27;
        editor.click(2, 4);
        assert_eq!(editor.cursor_offset(), wide + 3);
        editor.click(3, 4);
        assert_eq!(editor.cursor_offset(), wide + 3);
        editor.click(4, 4);
        assert_eq!(editor.cursor_offset(), wide + 6);
        editor.click(7, 4);
        assert_eq!(editor.cursor_offset(), wide + 10);
        // Either cell of an emoji sequence is the start of the sequence.
        editor.click(1, 5);
        assert_eq!(editor.cursor_offset(), wide + 12);
        editor.click(2, 5);
        assert_eq!(editor.cursor_offset(), wide + 12 + "👨‍👩‍👧".len());
        // Below the text is its end.
        editor.click(0, 40);
        assert_eq!(editor.cursor_offset(), editor.text().len());
        // A click drops the selection.
        press(&mut editor, "cmd-a");
        editor.click(0, 0);
        assert_eq!(marked(&editor).find('|'), Some(0));
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn a_click_in_a_scrolled_view_counts_from_the_first_visible_row() {
        let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        editor.view(20, 5);
        editor.scroll(10);
        assert_eq!(rows(&editor.view(20, 5))[2], "line 12");
        editor.click(5, 2);
        press(&mut editor, "X");
        assert!(editor.text().contains("line X12"));
        assert_eq!(editor.view(20, 5).first_row, 10);
    }

    #[test]
    fn a_drag_extends_the_selection_from_the_click() {
        let mut editor = BuiltinEditor::new("the quick brown fox jumps\n日本語abc");
        editor.view(11, 6);
        editor.click(4, 0);
        editor.drag(5, 1);
        assert_eq!(editor.copy().as_deref(), Some("quick brown"));
        assert_eq!(editor.cursor_offset(), 15);
        // Back past where it started: the selection turns around.
        editor.drag(1, 0);
        assert_eq!(marked(&editor), "t|he ^quick brown fox jumps\n日本語abc");
        // Onto the right half of a wide character: up to that character.
        editor.drag(3, 3);
        assert_eq!(editor.copy().as_deref(), Some("quick brown fox jumps\n日"));
        // Back to where it started selects nothing.
        editor.drag(4, 0);
        assert_eq!(editor.selection(), None);
        // A drag below the view scrolls it.
        let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
        let mut editor = BuiltinEditor::new(&text.join("\n"));
        editor.view(20, 5);
        editor.click(0, 1);
        editor.drag(4, 7);
        assert_eq!(editor.copy().unwrap().lines().count(), 7);
        let view = editor.view(20, 5);
        assert_eq!((view.first_row, view.cursor), (3, Some((4, 4))));
        // A drag with no click before it selects from the cursor.
        let mut editor = BuiltinEditor::new("abcdef");
        editor.view(20, 5);
        editor.drag(3, 0);
        assert_eq!(marked(&editor), "^abc|def");
    }

    #[test]
    fn a_large_paste_is_wrapped_once_and_typing_into_it_stays_fast() {
        let mut big = String::new();
        let mut lines = 0;
        while big.len() < 1_100_000 {
            big.push_str(&format!(
                "Line {lines} of the quarterly report, 日本語 and all, runs past the wrap width.\n"
            ));
            lines += 1;
        }
        assert!(lines > 10_000);
        let started = Instant::now();
        let mut editor = BuiltinEditor::default();
        editor.insert(&big);
        let view = editor.view(40, 30);
        assert!(view.total_rows > 2 * lines);
        assert_eq!(view.first_row + 30, view.total_rows);

        // Type in the middle, drawing after every key as the app does.
        editor.set_cursor_offset(big.len() / 2);
        WRAPPED_BYTES.with(|bytes| bytes.set(0));
        let keys = "w o r d space enter backspace backspace alt-left up down cmd-right";
        for _ in 0..125 {
            for key in keys.split(' ') {
                press(&mut editor, key);
                let view = editor.view(40, 30);
                assert!(view.cursor.is_some());
            }
        }
        // Each key re-wraps the line it changed and nothing else. Wrapping
        // the whole text even once more would be past this.
        let rewrapped = WRAPPED_BYTES.with(std::cell::Cell::get);
        assert!(rewrapped < big.len() / 2, "{rewrapped} bytes re-wrapped");
        assert!(editor.is_modified());
        while editor.undo() {}
        assert_eq!(editor.text(), "");
        assert!(editor.redo());
        assert_eq!(editor.text().len(), big.len());
        // Generous: this takes a fraction of a second. Re-wrapping a megabyte
        // on each of 1,500 keys takes minutes.
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    #[test]
    fn one_enormous_line_does_not_break_anything() {
        let mut editor = BuiltinEditor::default();
        editor.insert(&"word ".repeat(40_000));
        let view = editor.view(21, 10);
        assert_eq!(view.total_rows, 10_000);
        press(&mut editor, "up up cmd-left shift-down");
        assert_eq!(editor.copy().as_deref(), Some("word word word word "));
        press(&mut editor, "x pageup cmd-up");
        assert_eq!(editor.view(21, 10).first_row, 0);
    }

    /// A small deterministic generator, so the random test below needs no
    /// dependency and fails the same way every time.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: usize) -> usize {
            (self.next() % limit as u64) as usize
        }

        fn pick<'a, T: ?Sized>(&mut self, from: &[&'a T]) -> &'a T {
            from[self.below(from.len())]
        }
    }

    /// What must hold after any operation at all.
    fn check_invariants(editor: &BuiltinEditor) {
        assert!(!editor.lines.is_empty());
        for at in [Some(editor.cursor), editor.anchor].into_iter().flatten() {
            let line = &editor.lines[at.line].text;
            assert!(
                at.col == line.len()
                    || line
                        .grapheme_indices(true)
                        .any(|(index, _)| index == at.col),
                "{at:?} is not on a grapheme boundary of {line:?}"
            );
        }
        for line in &editor.lines {
            assert!(!line.text.contains(char::is_control), "{:?}", line.text);
            let mut end = 0;
            for word in &line.misspelled {
                assert!(word.start < word.end && word.end <= line.text.len());
                assert!(word.start >= end);
                assert!(line.text.is_char_boundary(word.start));
                assert!(line.text.is_char_boundary(word.end));
                end = word.end;
            }
        }
        let text = editor.text();
        assert!(editor.cursor_offset() <= text.len());
        assert!(text.is_char_boundary(editor.cursor_offset()));
        if let Some(selection) = editor.selection() {
            assert!(selection.start < selection.end && selection.end <= text.len());
            assert_eq!(editor.copy().as_deref(), text.get(selection));
        }
        // The text survives being loaded into another editor.
        let mut other = BuiltinEditor::default();
        other.set_text(&text);
        assert_eq!(other.text(), text);
        assert_eq!(other.lines.len(), editor.lines.len());
    }

    fn check_view(editor: &mut BuiltinEditor, width: usize, height: usize) {
        let before = editor.text();
        let view = editor.view(width, height);
        assert!(view.lines.len() <= height);
        for line in &view.lines {
            let row: String = line.spans.iter().map(|span| span.text.as_str()).collect();
            assert!(text::width(&row) <= width, "{width}x{height}: {row:?}");
            assert!(line.spans.iter().all(|span| !span.text.is_empty()));
        }
        if let Some((column, row)) = view.cursor {
            assert!(usize::from(column) < width && usize::from(row) < height);
        }
        if width > 0 && height > 0 {
            assert_eq!(view.lines.len(), height.min(view.total_rows));
            assert!(view.first_row + view.lines.len() <= view.total_rows);
        }
        assert_eq!(editor.text(), before, "drawing changed the text");
    }

    #[test]
    fn random_operations_never_break_the_invariants() {
        #[rustfmt::skip]
        const KEYS: &[&str] = &[
            "left", "right", "up", "down", "alt-left", "alt-right", "alt-up", "alt-down",
            "cmd-left", "cmd-right", "cmd-up", "cmd-down", "home", "end", "pageup", "pagedown",
            "shift-left", "shift-right", "shift-up", "shift-down", "shift-alt-left",
            "shift-alt-right", "shift-cmd-left", "shift-cmd-right", "shift-cmd-up",
            "shift-cmd-down", "shift-home", "shift-end", "shift-pageup", "shift-pagedown",
            "shift-alt-up", "shift-alt-down", "cmd-a", "backspace", "delete", "alt-backspace",
            "alt-delete", "cmd-backspace", "cmd-delete", "enter", "tab", "shift-tab", "cmd-z",
            "cmd-shift-z", "cmd-x", "cmd-c", "cmd-v", "ctrl-a", "ctrl-e", "ctrl-k", "ctrl-d",
            "ctrl-n", "ctrl-p", "ctrl-f", "ctrl-b", "ctrl-w", "esc", "f5", "backspace",
            "delete", "backspace", "enter",
        ];
        #[rustfmt::skip]
        const TYPED: &[&str] = &[
            "a", "b", "e", "z", " ", " ", " ", ".", ">", "*", "_", "`", "#", "[", "]",
            "(", ")", "-", "é", "日", "本", "한", "\u{301}", "\u{200d}", "👍", "🏽", "🇫", "🇷",
            "👨‍👩‍👧", "\u{fe0f}", "क", "\u{94d}", "ष",
        ];
        #[rustfmt::skip]
        const PASTED: &[&str] = &[
            "plain words here",
            "two\nlines",
            "\n",
            "windows\r\nline ends\rand old ones\ttab",
            "esc\u{1b}[31m bell\u{7} nul\u{0}",
            "> quoted\n>> deeper\n\n```\ncode *here*\n```\n",
            "**strong** _em_ `code` [link](https://example.com) https://example.com/x.",
            "日本語のメール 👨‍👩‍👧🇫🇷👍🏽 e\u{301}\u{301} क\u{94d}ष",
            "a very long line of words that will certainly wrap at any sensible width and then some",
            "-- \nSam Lee",
            "",
            "\u{301}\u{200d}",
        ];
        const NEEDLES: &[&str] = &[
            "a", "E", " ", "line", "日", "\n", "e\u{301}", "", "zzz", "*",
        ];

        for seed in [0x9e37_79b9_7f4a_7c15_u64, 0x2545_f491_4f6c_dd1d, 42] {
            let mut rng = Rng(seed);
            let original = "Hi Ana,\n\n> Can we move it a week?\n\nYes, 日本語 works. 👍🏽\n";
            let mut editor = BuiltinEditor::new(original);
            editor.tab_inserts = true;
            let (mut width, mut height) = (24, 6);
            check_invariants(&editor);
            for step in 0..1500 {
                let size = editor.text().len();
                match rng.below(100) {
                    0..30 => {
                        let chord =
                            KeyChord::plain(Key::Char(rng.pick(TYPED).chars().next().unwrap()));
                        editor.key(chord);
                        // A multi-character cluster arrives as text.
                        editor.insert(rng.pick(TYPED));
                    }
                    30..72 => {
                        let chord: KeyChord = rng.pick(KEYS).parse().unwrap();
                        editor.key(chord);
                    }
                    // Keep the text from growing without bound.
                    72..78 if size < 4000 => editor.insert(rng.pick(PASTED)),
                    72..78 => {
                        editor.set_cursor_offset(rng.below(size + 1));
                        for _ in 0..40 {
                            editor.key("shift-down".parse().unwrap());
                        }
                        editor.cut();
                    }
                    78..81 => editor.click(rng.below(40) as u16, rng.below(12) as u16),
                    81..84 => editor.drag(rng.below(40) as u16, rng.below(12) as u16),
                    84..86 => editor.scroll(rng.below(21) as isize - 10),
                    86..88 => {
                        editor.find(rng.pick(NEEDLES), rng.below(2) == 0);
                    }
                    88..90 => editor.reflow_paragraph(rng.below(40)),
                    90..92 if size < 4000 => editor.paste_as_quotation(rng.pick(PASTED)),
                    92 => editor.insert_signature(rng.pick(PASTED)),
                    93..95 => {
                        let ranges = (0..rng.below(6))
                            .map(|_| {
                                let start = rng.below(size + 10);
                                start..start + rng.below(12)
                            })
                            .collect();
                        editor.set_misspellings(ranges);
                    }
                    95 => editor.set_cursor_offset(rng.below(size + 10)),
                    96 => editor.mark_saved(),
                    97 => {
                        width = rng.below(30);
                        height = rng.below(10);
                    }
                    _ => {
                        editor.undo();
                    }
                }
                check_invariants(&editor);
                check_view(&mut editor, width, height);
                if step % 250 == 249 {
                    // Undoing everything gives back the text it started
                    // with, and redoing everything the text it has now.
                    let now = editor.text();
                    let mut copy = editor.clone();
                    let mut undone = 0;
                    while copy.undo() {
                        check_invariants(&copy);
                        undone += 1;
                    }
                    assert_eq!(copy.text(), original, "seed {seed:#x} step {step}");
                    for _ in 0..undone {
                        assert!(copy.redo());
                    }
                    assert_eq!(copy.text(), now, "seed {seed:#x} step {step}");
                    assert_eq!(copy.is_modified(), editor.is_modified());
                }
            }
        }
    }
}
