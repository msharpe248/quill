//! The reader: a message's headers, then its body, wrapped to the pane.

use quill_core::types::Address;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;

use super::{ImagePlacement, LinkPlacement, pane_block, placeholder};
use crate::invite::CardRow;
use crate::layout;
use crate::state::{Body, Env, Pane, Showing, State};
use crate::text;
use crate::theme::Styles;

/// The width of the longest label and its gap: `Subject: `.
const LABEL_WIDTH: usize = 9;

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

    let has_cc = !row.headers.cc.is_empty();
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
    let mut headers = vec![
        ("From:", join(&row.headers.from)),
        ("To:", join(&row.headers.to)),
    ];
    if has_cc {
        headers.push(("Cc:", join(&row.headers.cc)));
    }
    headers.push(("Date:", text::long_date(row.headers.date, &env.tz)));
    headers.push(("Subject:", row.headers.subject.clone()));
    // The files, by name, as many as fit; the list has them all.
    let files = state.reader.files();
    if !files.is_empty() {
        let key = env
            .hint_key(crate::command::CommandId::MessageAttachments)
            .map_or_else(String::new, |key| format!("  ({key} lists them)"));
        let named = files
            .iter()
            .map(|file| format!("{} ({})", file.name, text::file_size(file.size)))
            .collect::<Vec<_>>()
            .join(", ");
        let room = width.saturating_sub(LABEL_WIDTH + text::width(&key));
        headers.push(("Attach:", format!("{}{key}", text::truncate(&named, room))));
    }

    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    for (label, value) in headers {
        if y >= bottom {
            return None;
        }
        let value_style = if label == "Subject:" {
            styles.text().add_modifier(Modifier::BOLD)
        } else {
            styles.text()
        };
        let line = Line::from(vec![
            Span::styled(text::fit(label, LABEL_WIDTH), styles.dim()),
            Span::styled(
                text::truncate(&value, width.saturating_sub(LABEL_WIDTH)),
                value_style,
            ),
        ]);
        frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        y += 1;
    }
    // Under the fields, each after a blank line: what is still to come of
    // the message, and the invitation it carries.
    if !state.reader.note.is_empty() {
        y += 1;
        for note in &state.reader.note {
            if y >= bottom {
                return None;
            }
            let line = Line::styled(text::truncate(note, width), styles.dim());
            frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
            y += 1;
        }
    }
    if !state.reader.card.is_empty() {
        y += 1;
        for row in &state.reader.card {
            if y >= bottom {
                return None;
            }
            frame.render_widget(
                card_line(row, width, styles),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
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

                // Where the link text landed, for the run loop to mark.
                let mut column = 0;
                for span in &line.spans {
                    let room = width.saturating_sub(column);
                    if room == 0 {
                        break;
                    }
                    if span.style.link {
                        let x = inner.x + column as u16;
                        let drawn = frame
                            .buffer_mut()
                            .cell((x, rect.y))
                            .map(ratatui::buffer::Cell::style);
                        if let Some(style) = drawn {
                            links.push(LinkPlacement {
                                x,
                                y: rect.y,
                                text: text::clip(&span.text, room).to_owned(),
                                style,
                                url: span
                                    .link
                                    .and_then(|index| state.reader.links.get(index))
                                    .filter(|url| quill_mime::links::is_openable(url))
                                    .cloned(),
                            });
                        }
                    }
                    column += text::width(&span.text);
                }
            }
        }
    }
    image
}

/// One row of an invitation's card, cut to the pane. Its words were made
/// safe to draw when the card was made.
fn card_line(row: &CardRow, width: usize, styles: &Styles) -> Line<'static> {
    match row {
        CardRow::Title(kind) => {
            let title = text::truncate(&format!("── {kind} "), width);
            let rule = "─".repeat(width.saturating_sub(text::width(&title)));
            Line::styled(format!("{title}{rule}"), styles.dim())
        }
        CardRow::Field { label, text } => {
            let value = if *label == "What:" {
                styles.text().add_modifier(Modifier::BOLD)
            } else {
                styles.text()
            };
            Line::from(vec![
                Span::styled(text::fit(label, LABEL_WIDTH), styles.dim()),
                Span::styled(
                    text::truncate(text, width.saturating_sub(LABEL_WIDTH)),
                    value,
                ),
            ])
        }
        CardRow::Note(words) => Line::styled(text::truncate(words, width), styles.accent()),
    }
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

fn join(addresses: &[Address]) -> String {
    addresses
        .iter()
        .map(Address::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
