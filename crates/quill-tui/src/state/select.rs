//! Text chosen in the reader with the mouse, to copy: dragged over, a word
//! by a double click, a line by a triple one, or all of the message.
//!
//! A selection is of the message, not of the screen: it names lines of the
//! text and the cells in them, so it stays with the words when the text is
//! scrolled, and is dropped when the text is laid out again. The fields
//! above the text do not scroll, so a selection lies in them or in the
//! text and never across both: one that did would take in the lines
//! scrolled out of sight between them.
//!
//! What is copied is what is drawn, a row to a line. All of the message is
//! the exception: its fields are copied whole, however little of them the
//! pane had room for.

use quill_core::types::Address;
use quill_mime::html::Line;
use ratatui::layout::Rect;

use super::{Body, Effects, Env, HostRequest, Notice, State};
use crate::command::CommandId;
use crate::invite::CardRow;
use crate::{layout, text};

/// The width of the longest label and its gap: `Subject: `.
pub const LABEL_WIDTH: usize = 9;

/// A cell past the end of any row: a selection that runs to it takes the
/// row to its end.
const END: usize = usize::MAX / 2;

/// A cell of a row of the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub row: usize,
    pub column: usize,
}

/// The two parts of a message on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The rows above the text, which stay where they are: the fields,
    /// and what is said under them.
    Head,
    /// The lines of the text, which scroll.
    Body,
}

/// What is selected of the message on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// All of it: the fields and the text.
    All,
    /// From one cell to another of one part, both taken in. `anchor` is
    /// where it began and `head` where it has been taken to.
    Some {
        part: Part,
        anchor: Spot,
        head: Spot,
    },
}

impl Selection {
    /// The cells of `row` of `part` that are selected: from the first up
    /// to the one after the last.
    pub fn columns(&self, of: Part, row: usize) -> Option<(usize, usize)> {
        match *self {
            Self::All => Some((0, END)),
            Self::Some { part, anchor, head } => {
                let (start, end) = (anchor.min(head), anchor.max(head));
                if part != of || row < start.row || row > end.row {
                    return None;
                }
                let from = if row == start.row { start.column } else { 0 };
                let to = if row == end.row {
                    end.column.saturating_add(1)
                } else {
                    END
                };
                Some((from, to))
            }
        }
    }
}

/// One row above the text of the message on show, cut to the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadRow {
    /// A labelled line: a field of the message, or of the invitation it
    /// carries. `strong`: the one the eye is to find first.
    Field {
        label: &'static str,
        value: String,
        strong: bool,
    },
    Blank,
    /// What is still to come of the message.
    Note(String),
    /// The rule across the top of an invitation's card.
    Rule(String),
    /// What an invitation asks, or what has happened to it.
    Ask(String),
}

impl HeadRow {
    /// The row as it is drawn.
    pub fn text(&self) -> String {
        match self {
            Self::Field { label, value, .. } => {
                format!("{}{value}", text::fit(label, LABEL_WIDTH))
            }
            Self::Blank => String::new(),
            Self::Note(words) | Self::Rule(words) | Self::Ask(words) => words.clone(),
        }
    }
}

/// The main button is down in the reader: from the press to the letting
/// go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    part: Part,
    anchor: Spot,
    /// The cell of the screen it went down on.
    at: (u16, u16),
    /// The pointer has been taken to another cell since: this is a
    /// selection being made, and not a click.
    moved: bool,
}

/// A press in the reader that a press in the same place, soon enough,
/// makes a double or a triple click of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clicked {
    at: (u16, u16),
    when: i64,
    times: u8,
}

/// How many seconds may pass between two presses for them to be a double
/// click. The clock here ticks in whole seconds.
const DOUBLE_CLICK_SECONDS: i64 = 1;

/// Where the reader's rows are on the screen.
struct Geometry {
    /// The column of the first cell of text.
    left: u16,
    /// How many cells of text a row has.
    width: usize,
    head_top: u16,
    head_rows: usize,
    body_top: u16,
    body_rows: usize,
}

fn join(addresses: &[Address]) -> String {
    addresses
        .iter()
        .map(Address::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A line of the text as it is drawn, without its styles.
fn line_text(line: &Line) -> String {
    line.spans.iter().map(|span| span.text.as_str()).collect()
}

impl State {
    /// The fields of the message on show, whole: From, To, Cc when there
    /// is one, Date and Subject.
    fn reader_fields(&self, env: &Env) -> Vec<(&'static str, String)> {
        let Some(row) = &self.reader.row else {
            return Vec::new();
        };
        let mut fields = vec![
            ("From:", join(&row.headers.from)),
            ("To:", join(&row.headers.to)),
        ];
        if !row.headers.cc.is_empty() {
            fields.push(("Cc:", join(&row.headers.cc)));
        }
        fields.push(("Date:", text::long_date(row.headers.date, &env.tz)));
        fields.push(("Subject:", row.headers.subject.clone()));
        fields
    }

    /// The fields of the message on show, whole, with its files named
    /// among them: what a page of the message says above it.
    pub(super) fn message_fields(&self, env: &Env) -> Vec<(String, String)> {
        let mut fields = self.reader_fields(env);
        if let Some(named) = self.reader_files_named() {
            fields.push(("Attach:", named));
        }
        fields
            .into_iter()
            .map(|(label, value)| (label.to_owned(), value))
            .collect()
    }

    /// The files of the message on show, by name and size.
    fn reader_files_named(&self) -> Option<String> {
        let files = self.reader.files();
        (!files.is_empty()).then(|| {
            files
                .iter()
                .map(|file| format!("{} ({})", file.name, text::file_size(file.size)))
                .collect::<Vec<_>>()
                .join(", ")
        })
    }

    /// The rows above the text of the message on show, each cut to
    /// `width` cells: the fields, and under them, each after a blank row,
    /// what is still to come of the message and the invitation it
    /// carries. As many as [`State::reader_header_rows`] says.
    pub fn reader_head(&self, env: &Env, width: usize) -> Vec<HeadRow> {
        let room = width.saturating_sub(LABEL_WIDTH);
        let mut rows: Vec<HeadRow> = self
            .reader_fields(env)
            .into_iter()
            .map(|(label, value)| HeadRow::Field {
                label,
                value: text::truncate(&value, room),
                strong: label == "Subject:",
            })
            .collect();
        // The files, by name, as many as fit; the list has them all.
        if let Some(named) = self.reader_files_named() {
            let key = env
                .hint_key(CommandId::MessageAttachments)
                .map_or_else(String::new, |key| format!("  ({key} lists them)"));
            let names = room.saturating_sub(text::width(&key));
            rows.push(HeadRow::Field {
                label: "Attach:",
                value: format!("{}{key}", text::truncate(&named, names)),
                strong: false,
            });
        }
        if !self.reader.note.is_empty() {
            rows.push(HeadRow::Blank);
            rows.extend(
                self.reader
                    .note
                    .iter()
                    .map(|note| HeadRow::Note(text::truncate(note, width))),
            );
        }
        if !self.reader.card.is_empty() {
            rows.push(HeadRow::Blank);
            rows.extend(self.reader.card.iter().map(|row| match row {
                CardRow::Title(kind) => {
                    let title = text::truncate(&format!("── {kind} "), width);
                    let rule = "─".repeat(width.saturating_sub(text::width(&title)));
                    HeadRow::Rule(format!("{title}{rule}"))
                }
                CardRow::Field { label, text } => HeadRow::Field {
                    label,
                    value: text::truncate(text, room),
                    strong: *label == "What:",
                },
                CardRow::Note(words) => HeadRow::Ask(text::truncate(words, width)),
            }));
        }
        rows
    }

    /// The message on show as text: its fields, whole, then what its
    /// invitation says, then `lines`, which are its text laid out for some
    /// width. What is copied when all of it is, and what a text file of
    /// it holds.
    pub(super) fn message_text(&self, env: &Env, lines: &[Line]) -> String {
        let mut out = String::new();
        for (label, value) in self.message_fields(env) {
            out.push_str(&text::fit(&label, LABEL_WIDTH));
            out.push_str(&value);
            out.push('\n');
        }
        if !self.reader.card.is_empty() {
            out.push('\n');
            for row in &self.reader.card {
                match row {
                    CardRow::Title(kind) => out.push_str(&format!("── {kind} ──")),
                    CardRow::Field { label, text } => {
                        out.push_str(&text::fit(label, LABEL_WIDTH));
                        out.push_str(text);
                    }
                    CardRow::Note(words) => out.push_str(words),
                }
                out.push('\n');
            }
        }
        out.push('\n');
        for line in lines {
            out.push_str(line_text(line).trim_end());
            out.push('\n');
        }
        out
    }

    /// The message on show as text for a file: laid out for a page's
    /// width and not the pane's, so that the file is the same whatever
    /// size the window was. `None` while its text is not here.
    pub(super) fn message_text_for_file(&self, env: &Env) -> Option<String> {
        let (Body::Loaded(body), Some(showing)) = (&self.reader.body, self.reader.showing()) else {
            return None;
        };
        let (lines, _) = super::body_lines(body, showing, super::UNDRAWN_WIDTH);
        Some(self.message_text(env, &lines))
    }

    /// Whether the reader's text is drawn, with a message in it whose
    /// text is here: whether there is anything there to select.
    pub(super) fn reader_text_on_show(&self, env: &Env) -> bool {
        self.compose.is_none()
            && self.reader.row.is_some()
            && matches!(self.reader.body, Body::Loaded(_))
            && self.reader_rect(env).is_some()
    }

    fn reader_rect(&self, env: &Env) -> Option<Rect> {
        layout::compute(self, self.size, env.reader_width).reader
    }

    /// What is selected in the reader, as text. `None` when nothing is,
    /// or when what is has no words in it.
    pub(super) fn selected_text(&self, env: &Env) -> Option<String> {
        if !self.reader_text_on_show(env) {
            return None;
        }
        let selection = self.reader.selection?;
        let copied = match selection {
            Selection::All => self.message_text(env, &self.reader.lines),
            Selection::Some { part, anchor, head } => {
                let rows: Vec<String> = match part {
                    Part::Head => {
                        let width = layout::reader_text_width(self.reader_rect(env)?);
                        self.reader_head(env, width)
                            .iter()
                            .map(HeadRow::text)
                            .collect()
                    }
                    Part::Body => self.reader.lines.iter().map(line_text).collect(),
                };
                let (first, last) = (anchor.min(head).row, anchor.max(head).row);
                rows.iter()
                    .enumerate()
                    .skip(first)
                    .take(last.saturating_sub(first) + 1)
                    .filter_map(|(row, words)| {
                        let (from, to) = selection.columns(part, row)?;
                        let taken = text::cells(words, from, to);
                        // A row taken to its end has no use for the
                        // spaces a row may end in.
                        Some(if to >= END { taken.trim_end() } else { taken })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        (!copied.trim().is_empty()).then_some(copied)
    }

    /// Selects all of the message on show.
    pub(super) fn select_all_of_message(&mut self, env: &Env) {
        if self.reader_text_on_show(env) {
            self.reader.selection = Some(Selection::All);
        }
    }

    /// Copies what is selected in the reader. Returns whether anything
    /// was.
    pub(super) fn copy_selection(&mut self, env: &Env, out: &mut Effects) -> bool {
        let Some(copied) = self.selected_text(env) else {
            return false;
        };
        out.host.push(HostRequest::Copy(copied));
        self.flash = Some(("Copied.".to_owned(), false));
        true
    }

    /// Copies what is selected in the reader, or with nothing selected
    /// the whole message as text: the one key that does for a terminal
    /// what Select All and Copy do in the app.
    pub(super) fn copy_message_text(&mut self, env: &Env, out: &mut Effects) {
        if self.copy_selection(env, out) {
            return;
        }
        // Laid out for the pane where the text is drawn, as a selection
        // of all of it would be; for a page's width where it is not.
        let copied = if self.reader_text_on_show(env) {
            Some(self.message_text(env, &self.reader.lines))
        } else {
            self.message_text_for_file(env)
        };
        match copied {
            Some(copied) => {
                out.host.push(HostRequest::Copy(copied));
                self.flash = Some(("Copied the message.".to_owned(), false));
            }
            None => {
                self.flash = Some((
                    "This message's text is not here yet to copy.".to_owned(),
                    false,
                ));
            }
        }
    }

    fn reader_geometry(&self, env: &Env, rect: Rect) -> Geometry {
        let head_rows = self.reader_header_rows();
        let picture = self.reader_image_rows(env, rect);
        let head_top = rect.y + 1;
        let above = head_rows + 1 + if picture > 0 { picture + 1 } else { 0 };
        Geometry {
            left: rect.x + 2,
            width: layout::reader_text_width(rect),
            head_top,
            head_rows,
            body_top: head_top.saturating_add(above as u16),
            body_rows: layout::reader_body_height(rect, head_rows, picture),
        }
    }

    /// The cell of `part` the pointer is over, or the nearest there is to
    /// it. Taken above or below the text, the text follows it a line at a
    /// time.
    fn spot(&mut self, geometry: &Geometry, part: Part, column: u16, row: u16) -> Spot {
        let across =
            usize::from(column.saturating_sub(geometry.left)).min(geometry.width.saturating_sub(1));
        match part {
            Part::Head => {
                let last = geometry.head_rows.saturating_sub(1);
                match row.checked_sub(geometry.head_top).map(usize::from) {
                    None => Spot { row: 0, column: 0 },
                    Some(down) if down > last => Spot {
                        row: last,
                        column: END,
                    },
                    Some(down) => Spot {
                        row: down,
                        column: across,
                    },
                }
            }
            Part::Body => {
                let lines = self.reader.lines.len();
                let last = lines.saturating_sub(1);
                let Some(down) = row.checked_sub(geometry.body_top).map(usize::from) else {
                    self.reader.scroll = self.reader.scroll.saturating_sub(1);
                    return Spot {
                        row: self.reader.scroll,
                        column: 0,
                    };
                };
                if down >= geometry.body_rows {
                    let furthest = lines.saturating_sub(geometry.body_rows);
                    self.reader.scroll = (self.reader.scroll + 1).min(furthest);
                    return Spot {
                        row: (self.reader.scroll + geometry.body_rows)
                            .saturating_sub(1)
                            .min(last),
                        column: END,
                    };
                }
                match self.reader.scroll + down {
                    line if line > last => Spot {
                        row: last,
                        column: END,
                    },
                    line => Spot {
                        row: line,
                        column: across,
                    },
                }
            }
        }
    }

    /// The main button went down in the reader, on the message. A press
    /// alone selects nothing, and lets go of what was selected: it is the
    /// start of a click or of a drag, and which is known when the button
    /// is let go. A second press in the same place selects the word
    /// there, and a third the line.
    pub(super) fn press_in_reader(&mut self, env: &Env, rect: Rect, column: u16, row: u16) {
        self.held = None;
        self.reader.selection = None;
        let geometry = self.reader_geometry(env, rect);
        let in_head =
            row >= geometry.head_top && usize::from(row - geometry.head_top) < geometry.head_rows;
        let part = if in_head {
            Part::Head
        } else if row >= geometry.body_top && !self.reader.lines.is_empty() {
            Part::Body
        } else {
            // The blank rows between the two, and a text that is not here.
            self.clicked = None;
            return;
        };
        let spot = self.spot(&geometry, part, column, row);
        let times = match self.clicked {
            Some(before)
                if before.at == (column, row)
                    && self.now - before.when <= DOUBLE_CLICK_SECONDS
                    && before.times < 3 =>
            {
                before.times + 1
            }
            _ => 1,
        };
        self.clicked = Some(Clicked {
            at: (column, row),
            when: self.now,
            times,
        });
        if times == 1 {
            self.held = Some(Held {
                part,
                anchor: spot,
                at: (column, row),
                moved: false,
            });
            return;
        }
        let words = match part {
            Part::Head => self
                .reader_head(env, geometry.width)
                .get(spot.row)
                .map(HeadRow::text),
            Part::Body => self.reader.lines.get(spot.row).map(line_text),
        };
        let Some(words) = words else {
            return;
        };
        let cells = if times == 2 {
            text::word_cells(&words, spot.column)
        } else {
            let all = text::width(words.trim_end());
            (all > 0).then_some((0, all))
        };
        if let Some((from, to)) = cells {
            self.reader.selection = Some(Selection::Some {
                part,
                anchor: Spot {
                    row: spot.row,
                    column: from,
                },
                head: Spot {
                    row: spot.row,
                    column: to.saturating_sub(1),
                },
            });
        }
    }

    /// The pointer moved with the button held since a press in the
    /// reader: what lies between the press and the pointer is selected.
    pub(super) fn drag_in_reader(&mut self, env: &Env, column: u16, row: u16) {
        let (Some(held), Some(rect)) = (self.held, self.reader_rect(env)) else {
            return;
        };
        if !held.moved && held.at == (column, row) {
            return;
        }
        let geometry = self.reader_geometry(env, rect);
        let head = self.spot(&geometry, held.part, column, row);
        self.reader.selection = Some(Selection::Some {
            part: held.part,
            anchor: held.anchor,
            head,
        });
        self.held = Some(Held {
            moved: true,
            ..held
        });
        // It has been dragged, so it is not half of a double click.
        self.clicked = None;
    }

    /// The button was let go. After a drag what was dragged over stays
    /// selected. With no drag it was a click, and a click on a link opens
    /// it, as it would from the list of links: on its words, on the
    /// number after them, or on its line in the list at the foot of the
    /// message.
    pub(super) fn let_go_in_reader(&mut self, out: &mut Effects) {
        let Some(held) = self.held.take() else {
            return;
        };
        if held.moved || held.part != Part::Body {
            return;
        }
        let Some(line) = self.reader.lines.get(held.anchor.row) else {
            return;
        };
        let mut start = 0;
        let mut link = None;
        for span in &line.spans {
            let end = start + text::width(&span.text);
            if (start..end).contains(&held.anchor.column) {
                link = span
                    .link
                    .filter(|_| super::mouse::opens_on_click(span.style));
                break;
            }
            start = end;
        }
        let Some(url) = link.and_then(|index| self.reader.links.get(index)) else {
            return;
        };
        if quill_mime::links::is_openable(url) {
            out.host.push(HostRequest::OpenUrl(url.clone()));
        } else {
            self.notices.push_back(Notice::error(
                "Quill only opens web (http, https) and mail (mailto) links. This is not one.",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(part: Part, anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection::Some {
            part,
            anchor: Spot {
                row: anchor.0,
                column: anchor.1,
            },
            head: Spot {
                row: head.0,
                column: head.1,
            },
        }
    }

    #[test]
    fn a_selection_takes_in_both_its_ends_whichever_way_it_was_dragged() {
        for selection in [
            some(Part::Body, (2, 5), (4, 1)),
            some(Part::Body, (4, 1), (2, 5)),
        ] {
            assert_eq!(selection.columns(Part::Body, 1), None);
            assert_eq!(selection.columns(Part::Body, 2), Some((5, END)));
            assert_eq!(selection.columns(Part::Body, 3), Some((0, END)));
            assert_eq!(selection.columns(Part::Body, 4), Some((0, 2)));
            assert_eq!(selection.columns(Part::Body, 5), None);
            // It is of the text, and of nothing above it.
            assert_eq!(selection.columns(Part::Head, 3), None);
        }
    }

    #[test]
    fn a_selection_in_one_row_is_the_cells_between_its_ends() {
        let selection = some(Part::Head, (1, 7), (1, 3));
        assert_eq!(selection.columns(Part::Head, 1), Some((3, 8)));
        assert_eq!(selection.columns(Part::Head, 0), None);
    }

    #[test]
    fn all_of_a_message_is_every_row_of_both_parts() {
        assert_eq!(Selection::All.columns(Part::Head, 0), Some((0, END)));
        assert_eq!(Selection::All.columns(Part::Body, 900), Some((0, END)));
    }
}
