//! The reader: a message's headers, then its body, wrapped to the pane.

use quill_core::types::Address;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;

use super::{ImagePlacement, SAMPLE_IMAGE_ID, pane_block, placeholder};
use crate::layout;
use crate::state::{Body, Env, Pane, State};
use crate::text;
use crate::theme::Styles;

/// The width of the longest label and its gap: `Subject: `.
const LABEL_WIDTH: usize = 9;

/// Draws the reader. Returns the image to place over it, if the message
/// shows one and the terminal can draw it.
pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) -> Option<ImagePlacement> {
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
    let tags: Vec<&str> = [
        (!row.flags.seen, "unread"),
        (row.flags.flagged, "starred"),
        (row.has_attachments, "attachment"),
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
    let viewport = layout::reader_body_height(area, has_cc, image_rows);
    let total = state.reader.lines.len();
    if total > viewport && viewport > 0 {
        let first = state.reader.scroll + 1;
        let last = (state.reader.scroll + viewport).min(total);
        block = block.title_bottom(
            Line::styled(format!(" {first}-{last} of {total} "), styles.dim()).right_aligned(),
        );
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
    // A blank line between the headers and what follows.
    y += 1;
    if y >= bottom {
        return None;
    }

    // An inline image: cells left empty for the terminal to draw over, or
    // one row naming the image where the terminal cannot draw it.
    let mut image = None;
    if image_rows > 0 {
        let sample = (quill_media::sample::WIDTH, quill_media::sample::HEIGHT);
        if env.images {
            let rows = image_rows as u16;
            let columns = (u32::from(rows) * 2 * sample.0 / sample.1).min(u32::from(inner.width));
            image = Some(ImagePlacement {
                id: SAMPLE_IMAGE_ID,
                area: Rect::new(inner.x, y, columns as u16, rows),
            });
        } else {
            let name = format!("[image: sample, {}x{}]", sample.0, sample.1);
            let line = Line::styled(text::truncate(&name, width), styles.dim());
            frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        }
        y += image_rows as u16 + 1;
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
            }
        }
    }
    image
}

/// How a piece of a message body is drawn. Every distinction is carried by
/// a text attribute, so it survives a terminal without colour.
fn body_style(style: quill_mime::html::Style, styles: &Styles) -> Style {
    let mut drawn = if style.dim {
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
