//! The message being written, in the reader's place: the header fields, a
//! rule, the body, and at the foot anything that needs saying.

use quill_store::DraftAttachment;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;

use super::pane_block;
use crate::compose::Field;
use crate::editor::{SpanKind, ViewSpan};
use crate::input::LineInput;
use crate::layout;
use crate::state::{Env, State};
use crate::text;
use crate::theme::Styles;

/// The width of the longest label and its gap: `Subject: `.
const LABEL_WIDTH: usize = 9;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(compose) = &state.compose else {
        return;
    };
    let styles = &env.styles;
    let mut block = pane_block(3, compose.title, true, styles).padding(Padding::horizontal(1));
    let view = &compose.body_view;
    if view.total_rows > view.lines.len() && !view.lines.is_empty() {
        let first = view.first_row + 1;
        let last = view.first_row + view.lines.len();
        block = block.title_bottom(
            Line::styled(
                format!(" {first}-{last} of {} ", view.total_rows),
                styles.dim(),
            )
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let width = usize::from(inner.width);
    let typing = !compose.sending && compose.attaching.is_none();
    let bottom = inner.y + inner.height;
    let mut y = inner.y;
    let row = |y: &mut u16| -> Option<Rect> {
        (*y < bottom).then(|| {
            let rect = Rect::new(inner.x, *y, inner.width, 1);
            *y += 1;
            rect
        })
    };

    // From: who the message goes from, and which account that is.
    if let Some(rect) = row(&mut y) {
        let account = state
            .accounts
            .iter()
            .find(|account| account.id == compose.account);
        let focused = typing && compose.field == Field::From;
        let who = account.map_or_else(String::new, |account| {
            let name = env
                .identities
                .get(&account.name)
                .and_then(|identity| identity.full_name.as_deref())
                .unwrap_or_default();
            let address = crate::compose::written_address(name, &account.address);
            format!("{address}  ({})", account.name)
        });
        let mut spans = vec![label("From:", focused, styles)];
        if focused && state.accounts.len() > 1 {
            spans.push(Span::styled("‹ ", styles.dim()));
            spans.push(Span::styled(
                text::truncate(
                    &text::sanitize_line(&who),
                    width.saturating_sub(LABEL_WIDTH + 4),
                ),
                styles.text().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ›", styles.dim()));
        } else {
            spans.push(Span::styled(
                text::truncate(
                    &text::sanitize_line(&who),
                    width.saturating_sub(LABEL_WIDTH),
                ),
                styles.text(),
            ));
        }
        frame.render_widget(Line::from(spans), rect);
    }

    // Where the people on offer are drawn: under the field being typed in.
    let mut popup_under = None;
    let fields: [(&str, &LineInput, Field, bool); 4] = [
        ("To:", &compose.to, Field::To, true),
        ("Cc:", &compose.cc, Field::Cc, true),
        (
            "Bcc:",
            &compose.bcc,
            Field::Bcc,
            compose.field == Field::Bcc || !compose.bcc.is_empty(),
        ),
        ("Subject:", &compose.subject, Field::Subject, true),
    ];
    for (name, input, field, shown) in fields {
        if !shown {
            continue;
        }
        let Some(rect) = row(&mut y) else { break };
        let focused = typing && compose.field == field;
        let room = width.saturating_sub(LABEL_WIDTH);
        // The field being typed in follows its cursor; the others show
        // how they start.
        let (shown, column) = if focused {
            input.view(room)
        } else {
            (text::truncate(input.text(), room), 0)
        };
        let mut value = styles.text();
        if field == Field::Subject {
            value = value.add_modifier(Modifier::BOLD);
        }
        let line = Line::from(vec![
            label(name, focused, styles),
            Span::styled(text::fit(&shown, room), value),
        ]);
        frame.render_widget(line, rect);
        if focused {
            frame.set_cursor_position(Position::new(
                rect.x + (LABEL_WIDTH + column) as u16,
                rect.y,
            ));
            popup_under = Some(rect);
        }
    }
    if !compose.attachments.is_empty()
        && let Some(rect) = row(&mut y)
    {
        let files = compose
            .attachments
            .iter()
            .map(described)
            .collect::<Vec<_>>()
            .join(", ");
        let line = Line::from(vec![
            label("Attach:", false, styles),
            Span::styled(
                text::truncate(
                    &text::sanitize_line(&files),
                    width.saturating_sub(LABEL_WIDTH),
                ),
                styles.text(),
            ),
        ]);
        frame.render_widget(line, rect);
    }
    if let Some(rect) = row(&mut y) {
        frame.render_widget(Line::styled("─".repeat(width), styles.dim()), rect);
    }

    // The body, as the editor laid it out for exactly this space.
    let body = layout::compose_body(area, compose);
    for (offset, line) in compose.body_view.lines.iter().enumerate() {
        let Some(row) = u16::try_from(offset).ok().filter(|row| *row < body.height) else {
            break;
        };
        let spans: Vec<Span> = line
            .spans
            .iter()
            .map(|span| Span::styled(span.text.as_str(), body_style(span, styles)))
            .collect();
        frame.render_widget(
            Line::from(spans),
            Rect::new(body.x, body.y + row, body.width, 1),
        );
    }
    if typing
        && compose.field == Field::Body
        && let Some((column, row)) = compose.body_view.cursor
    {
        frame.set_cursor_position(Position::new(body.x + column, body.y + row));
    }

    // The foot: a file name being typed, a complaint, or doubts.
    let foot = body.y + body.height;
    if let Some(path) = &compose.attaching {
        let rect = Rect::new(inner.x, foot.min(bottom - 1), inner.width, 1);
        let prompt = "Attach the file at: ";
        let room = width.saturating_sub(prompt.len());
        let (shown, column) = path.view(room);
        let line = Line::from(vec![
            Span::styled(prompt, styles.key()),
            Span::styled(
                text::fit(&shown, room),
                styles.text().add_modifier(Modifier::UNDERLINED),
            ),
        ]);
        frame.render_widget(line, rect);
        frame.set_cursor_position(Position::new(
            rect.x + (prompt.len() + column) as u16,
            rect.y,
        ));
    } else if compose.sending {
        let rect = Rect::new(inner.x, foot.min(bottom - 1), inner.width, 1);
        frame.render_widget(Line::styled("Checking the message…", styles.dim()), rect);
    } else if let Some(error) = &compose.error {
        let lines = text::wrap(error, width);
        let shown = lines.len().min(layout::COMPOSE_ERROR_ROWS);
        for (offset, words) in lines.iter().take(shown).enumerate() {
            let y = foot + offset as u16;
            if y >= bottom {
                break;
            }
            // What does not fit in the rows there are ends in an ellipsis.
            let words = if offset + 1 == shown && lines.len() > shown {
                text::truncate(&format!("{words}…"), width)
            } else {
                text::truncate(words, width)
            };
            frame.render_widget(
                Line::styled(words, styles.error()),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
    } else if !compose.doubts.is_empty() {
        let send = env
            .hint_key(crate::command::CommandId::ComposeSend)
            .map_or_else(|| "Send".to_owned(), ToString::to_string);
        let mut lines: Vec<(String, Style)> = compose
            .doubts
            .iter()
            .take(3)
            .map(|doubt| (doubt.clone(), styles.error()))
            .collect();
        lines.push((format!("{send} again sends it as it is."), styles.dim()));
        for (offset, (words, style)) in lines.into_iter().enumerate() {
            let y = foot + offset as u16;
            if y >= bottom {
                break;
            }
            frame.render_widget(
                Line::styled(text::truncate(&words, width), style),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
    }

    // The people on offer, over whatever is under the field.
    if let (Some(completion), Some(under)) = (&compose.completion, popup_under) {
        for (offset, person) in completion.people.iter().enumerate() {
            let y = under.y + 1 + offset as u16;
            if y >= bottom {
                break;
            }
            let rect = Rect::new(
                under.x + LABEL_WIDTH as u16,
                y,
                under.width.saturating_sub(LABEL_WIDTH as u16),
                1,
            );
            let selected = offset == completion.selected;
            let who = crate::compose::written_address(&person.name, &person.email);
            let line = Line::from(vec![
                Span::raw(if selected { "> " } else { "  " }),
                Span::styled(
                    text::fit(
                        &text::sanitize_line(&who),
                        usize::from(rect.width).saturating_sub(2),
                    ),
                    styles.text(),
                ),
            ]);
            frame.render_widget(ratatui::widgets::Clear, rect);
            frame.buffer_mut().set_style(rect, styles.base());
            frame.render_widget(line, rect);
            frame.buffer_mut().set_style(
                rect,
                if selected {
                    styles.selected(true)
                } else {
                    styles.dim()
                },
            );
        }
    }
}

fn label(name: &str, focused: bool, styles: &Styles) -> Span<'static> {
    Span::styled(
        text::fit(name, LABEL_WIDTH),
        if focused { styles.key() } else { styles.dim() },
    )
}

/// A file as the header lists it: its name and size.
fn described(attachment: &DraftAttachment) -> String {
    let size = attachment.size;
    let size = if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{} KB", size / 1024)
    } else {
        format!("{size} bytes")
    };
    format!("{} ({size})", attachment.name)
}

/// How a piece of the body is drawn. Every distinction also has a text
/// attribute, so it survives a terminal without colour.
fn body_style(span: &ViewSpan, styles: &Styles) -> Style {
    let mut drawn = match span.kind {
        SpanKind::Text => styles.text(),
        SpanKind::Quote(_) => styles.dim(),
        SpanKind::Heading | SpanKind::Strong => styles.text().add_modifier(Modifier::BOLD),
        SpanKind::Emphasis => styles.text().add_modifier(Modifier::ITALIC),
        SpanKind::Code => styles.accent(),
        SpanKind::Link => styles.text().add_modifier(Modifier::UNDERLINED),
    };
    if span.misspelled {
        drawn = drawn
            .add_modifier(Modifier::UNDERLINED)
            .patch(styles.error());
    }
    if span.selected {
        drawn = drawn.add_modifier(Modifier::REVERSED);
    }
    drawn
}
