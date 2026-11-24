//! The reader: a message's headers, then its body, wrapped to the pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;

use super::{ImagePlacement, LinkPlacement, pane_block, placeholder};
use crate::layout;
use crate::state::{Body, Env, HeadRow, LABEL_WIDTH, Pane, Part, Selection, Showing, State};
use crate::text;
use crate::theme::Styles;

/// Draws the reader. Returns the image to place over it, if the message
/// shows one and the terminal can draw it, and adds the link text it drew
/// to `links`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &State,
    env: &Env,
    links: &mut Vec<LinkPlacement>,
) -> Option<ImagePlacement> {
    let styles = &env.styles;
    let focused = state.focus == Pane::Reader;
    let mut block = pane_block(3, "Reader", focused, styles).padding(Padding::horizontal(1));
    let Some(row) = &state.reader.row else {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        placeholder(frame, inner, "No message selected", styles);
        return None;
    };

    // The message's state, in words, on the top border.
    // And, for mail from a list that says how to leave it, the key that
    // does.
    let unsubscribe = env
        .hint_key(crate::command::CommandId::MessageUnsubscribe)
        .filter(|_| state.reader_can_unsubscribe(env))
        .map(|key| format!("{key} unsubscribes"));
    let tags: Vec<&str> = [
        (!row.flags.seen, "unread"),
        (row.flags.flagged, "starred"),
        (row.has_attachments, "attachment"),
        (unsubscribe.is_some(), unsubscribe.as_deref().unwrap_or("")),
    ]
    .into_iter()
    .filter_map(|(on, tag)| on.then_some(tag))
    .collect();
    if !tags.is_empty() {
        block = block.title_top(
            Line::styled(format!(" {} ", tags.join(", ")), styles.dim()).right_aligned(),
        );
    }

    let image_rows = state.reader_image_rows(env, area);
    let viewport = layout::reader_body_height(area, state.reader_header_rows(), image_rows);
    let total = state.reader.lines.len();
    if total > viewport && viewport > 0 {
        let first = state.reader.scroll + 1;
        let last = (state.reader.scroll + viewport).min(total);
        block = block.title_bottom(
            Line::styled(format!(" {first}-{last} of {total} "), styles.dim()).right_aligned(),
        );
    }
    // Which version is on show, when there is another to switch to.
    if state.reader.has_two_versions() {
        let version = match state.reader.showing() {
            Some(Showing::Html) => " html ",
            Some(Showing::Plain) => " plain text ",
            None => "",
        };
        block = block.title_bottom(Line::styled(version, styles.dim()).left_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = usize::from(inner.width);
    let selection = state.reader.selection;
    // What is selected of a row is drawn as selected, as far as the row
    // has words.
    let mark = |frame: &mut Frame, part: Part, row: usize, y: u16, words: usize| {
        let Some((from, to)) = selection.and_then(|selection| selection.columns(part, row)) else {
            return;
        };
        let to = to.min(words).min(width);
        if from < to {
            let rect = Rect::new(inner.x + from as u16, y, (to - from) as u16, 1);
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
        // The cursor that selects with the keys is drawn wherever it is:
        // on a row with no words too, where nothing else would show it.
        if let (Some(_), Some(Selection::Some { head, .. })) = (state.reader.caret, selection)
            && (head.part, head.row) == (part, row)
            && width > 0
        {
            let at = head.column.min(width - 1);
            let rect = Rect::new(inner.x + at as u16, y, 1, 1);
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    };

    // The rows above the text: the fields, and under them, each after a
    // blank line, what is still to come of the message and the
    // invitation it carries.
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    for (index, head) in state.reader_head(env, width).iter().enumerate() {
        if y >= bottom {
            return None;
        }
        let line = match head {
            HeadRow::Field {
                label,
                value,
                strong,
            } => Line::from(vec![
                Span::styled(text::fit(label, LABEL_WIDTH), styles.dim()),
                Span::styled(
                    value.as_str(),
                    if *strong {
                        styles.text().add_modifier(Modifier::BOLD)
                    } else {
                        styles.text()
                    },
                ),
            ]),
            HeadRow::Blank => Line::default(),
            HeadRow::Note(words) | HeadRow::Rule(words) => {
                Line::styled(words.as_str(), styles.dim())
            }
            HeadRow::Ask(words) => Line::styled(words.as_str(), styles.accent()),
        };
        frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        mark(frame, Part::Head, index, y, text::width(&head.text()));
        y += 1;
    }
    // A blank line between the headers and what follows.
    y += 1;
    if y >= bottom {
        return None;
    }

    // The message's picture: cells left empty for the terminal to draw
    // over. Where the terminal draws no pictures there is none here, and
    // the file is in the list like any other.
    let mut image = None;
    let (columns, rows) = state.reader_picture_cells(env, area);
    if let (Some(picture), true) = (&state.reader.picture, rows > 0) {
        image = Some(ImagePlacement {
            id: picture.id,
            area: Rect::new(inner.x, y, columns.min(inner.width), rows),
        });
        y += rows + 1;
        if y >= bottom {
            return image;
        }
    }

    match &state.reader.body {
        Body::None => {}
        Body::Loading => {
            let line = Line::styled("Loading…", styles.dim());
            frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        }
        Body::Downloading => {
            let line = Line::styled("Downloading…", styles.dim());
            frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        }
        Body::Unavailable(reason) => {
            for (offset, text) in text::wrap(reason, width)
                .iter()
                .take(usize::from(bottom - y))
                .enumerate()
            {
                let rect = Rect::new(inner.x, y + offset as u16, inner.width, 1);
                frame.render_widget(Line::styled(text.as_str(), styles.dim()), rect);
            }
        }
        Body::Loaded(_) => {
            let rows = usize::from(bottom - y);
            for (offset, line) in state
                .reader
                .lines
                .iter()
                .skip(state.reader.scroll)
                .take(rows)
                .enumerate()
            {
                let rect = Rect::new(inner.x, y + offset as u16, inner.width, 1);
                let spans: Vec<Span> = line
                    .spans
                    .iter()
                    .map(|span| Span::styled(span.text.as_str(), body_style(span.style, styles)))
                    .collect();
                // A line wider than the pane, such as a long address in the
                // list of links, is clipped at its edge.
                frame.render_widget(Line::from(spans), rect);
                let words: usize = line.spans.iter().map(|span| text::width(&span.text)).sum();
                mark(
                    frame,
                    Part::Body,
                    state.reader.scroll + offset,
                    rect.y,
                    words,
                );

                // Where the link text landed, for the run loop to mark.
                let mut column = 0;
                for span in &line.spans {
                    let room = width.saturating_sub(column);
                    if room == 0 {
                        break;
                    }
                    if span.style.link {
                        let url = span
                            .link
                            .and_then(|index| state.reader.links.get(index))
                            .filter(|url| quill_mime::links::is_openable(url));
                        // A link partly selected is marked in pieces: each
                        // is written again in the one style its first cell
                        // was drawn in.
                        let chosen = selection
                            .and_then(|selection| {
                                selection.columns(Part::Body, state.reader.scroll + offset)
                            })
                            .map_or((0, 0), |(from, to)| {
                                (from.saturating_sub(column), to.saturating_sub(column))
                            });
                        let shown = text::clip(&span.text, room);
                        let mut at = column;
                        for piece in text::pieces(shown, chosen.0, chosen.1) {
                            let x = inner.x + at as u16;
                            let drawn = frame
                                .buffer_mut()
                                .cell((x, rect.y))
                                .map(ratatui::buffer::Cell::style);
                            if let Some(style) = drawn {
                                links.push(LinkPlacement {
                                    x,
                                    y: rect.y,
                                    text: piece.to_owned(),
                                    style,
                                    url: url.cloned(),
                                });
                            }
                            at += text::width(piece);
                        }
                    }
                    column += text::width(&span.text);
                }
            }
        }
    }
    image
}

/// How a piece of a message body is drawn. Every distinction is carried by
/// a text attribute, so it survives a terminal without colour.
fn body_style(style: quill_mime::html::Style, styles: &Styles) -> Style {
    // Quoted text steps back; its `>` says so without colour.
    let mut drawn = if style.dim || style.quote > 0 {
        styles.dim()
    } else if style.code {
        styles.accent()
    } else {
        styles.text()
    };
    if style.bold {
        drawn = drawn.add_modifier(Modifier::BOLD);
    }
    if style.italic {
        drawn = drawn.add_modifier(Modifier::ITALIC);
    }
    if style.strike {
        drawn = drawn.add_modifier(Modifier::CROSSED_OUT);
    }
    if style.link {
        drawn = drawn.add_modifier(Modifier::UNDERLINED);
    }
    drawn
}
