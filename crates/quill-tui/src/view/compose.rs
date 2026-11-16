//! The message being written, in the reader's place: the header fields, a
//! rule, the body, and at the foot anything that needs saying.

use quill_store::DraftAttachment;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Padding;

use super::pane_block;
use crate::compose::{Away, Compose, Field};
use crate::editor::{SpanKind, ViewSpan};
use crate::input::LineInput;
use crate::layout;
use crate::state::{Env, State};
use crate::text;
use crate::theme::Styles;
use crate::vim::ExternalEditor;

/// The width of the longest label and its gap: `Subject: `.
const LABEL_WIDTH: usize = 9;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(compose) = &state.compose else {
        return;
    };
    let styles = &env.styles;
    let mut block = pane_block(3, compose.title, true, styles).padding(Padding::horizontal(1));
    let view = &compose.body_view;
    if compose.away.is_none() && view.total_rows > view.lines.len() && !view.lines.is_empty() {
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
    // In an editor that has all of it, there is nothing of the message
    // here. One that has the text alone leaves the fields where they are.
    if let Some(away) = compose.away.filter(|_| !compose.embedded) {
        in_vim(frame, inner, away, env);
        return;
    }
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

    // The body, as the editor laid it out for exactly this space. With
    // the text in an editor outside Quill, the space is that editor's:
    // the host lays it over these cells, and what is drawn here is only
    // seen while it is not there.
    let body = layout::compose_body(area, compose);
    if let Some(away) = compose.away {
        text_away(frame, area, state, env, away);
    }
    let lines: &[_] = if compose.away.is_some() {
        &[]
    } else {
        &compose.body_view.lines
    };
    for (offset, line) in lines.iter().enumerate() {
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
        && compose.away.is_none()
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

/// The fewest rows and columns an editor is run in. In less, the text's
/// place says so and the editor is not on show.
const EDITOR_LEAST: (u16, u16) = (20, 3);

/// What is said above an editor that runs in the text's place: what its
/// own keys do with the message, and the keys of Quill's that still work.
/// Each a line, wrapped to `width`.
pub fn tips(env: &Env, width: usize) -> Vec<String> {
    let editor = env.external;
    let key = |id, otherwise: &str| {
        env.hint_key(id)
            .map_or_else(|| otherwise.to_owned(), |key| key.to_string())
    };
    let its_own = if editor.is_vim() {
        ":QuillSend sends the message, :QuillPostpone saves it as a draft and closes it, \
         :QuillDiscard throws it away. :wq brings it into Quill's own editor."
            .to_owned()
    } else if editor == ExternalEditor::Mg {
        "ctrl-x ctrl-s saves the message, and ctrl-x ctrl-c then brings it into Quill's own \
         editor, to send."
            .to_owned()
    } else {
        "ctrl-x saves the message and brings it into Quill's own editor, to send.".to_owned()
    };
    let quills = format!(
        "{} goes to the fields above and {} comes back. {} is Quill's own editor. {} hides this.",
        key(crate::command::CommandId::ComposeFields, "A click"),
        key(crate::command::CommandId::ComposeBody, "a click"),
        key(
            crate::command::CommandId::ComposeVim,
            "Write in Another Editor"
        ),
        key(crate::command::CommandId::ComposeTips, "The Help menu"),
    );
    [its_own, quills]
        .iter()
        .flat_map(|words| text::wrap(words, width.max(1)))
        .collect()
}

/// The rows the tips take above the editor, with the rule under them:
/// none when they are not shown or would leave the editor no room.
fn tips_rows(body: Rect, env: &Env, tips_on: bool) -> u16 {
    if !tips_on {
        return 0;
    }
    let rows = u16::try_from(tips(env, usize::from(body.width)).len() + 1).unwrap_or(u16::MAX);
    if body.height >= rows.saturating_add(EDITOR_LEAST.1 + 2) {
        rows
    } else {
        0
    }
}

/// The cells an editor that has the text is given, in the message's pane
/// at `area`: the text's place, below the tips. `None` when there is not
/// room for an editor in it.
pub fn editor_rect(area: Rect, compose: &Compose, env: &Env, tips_on: bool) -> Option<Rect> {
    let body = layout::compose_body(area, compose);
    let tips = tips_rows(body, env, tips_on);
    let rect = Rect::new(body.x, body.y + tips, body.width, body.height - tips);
    (rect.width >= EDITOR_LEAST.0 && rect.height >= EDITOR_LEAST.1).then_some(rect)
}

/// The row of the pane a header field is drawn on, counting from the
/// first inside the border.
fn field_row(compose: &Compose, field: Field) -> Option<u16> {
    let bcc = compose.field == Field::Bcc || !compose.bcc.is_empty();
    match field {
        Field::From => Some(0),
        Field::To => Some(1),
        Field::Cc => Some(2),
        Field::Bcc => bcc.then_some(3),
        Field::Subject => Some(3 + u16::from(bcc)),
        Field::Body => None,
    }
}

/// How many of the rows of `place`, from its top, the people on offer
/// under an address being typed are drawn over.
pub fn rows_over(area: Rect, compose: &Compose, place: Rect) -> u16 {
    let (Some(completion), Some(row)) = (&compose.completion, field_row(compose, compose.field))
    else {
        return 0;
    };
    let people = u16::try_from(completion.people.len()).unwrap_or(u16::MAX);
    // Inside the border, under the field.
    let end = area.y.saturating_add(1 + row + 1).saturating_add(people);
    end.saturating_sub(place.y).min(place.height)
}

/// What is drawn in the text's place while an editor outside Quill has
/// the text: the tips, when they are on, and under them a line for while
/// the editor is not there, on its way out or back or with no room to be
/// in. Where the editor goes nothing is drawn: a part of it that is
/// covered for a moment shows what is under it.
fn text_away(frame: &mut Frame, area: Rect, state: &State, env: &Env, away: Away) {
    let Some(compose) = &state.compose else {
        return;
    };
    let styles = &env.styles;
    let body = layout::compose_body(area, compose);
    let name = env.external.name();
    let width = usize::from(body.width);
    let rows = tips_rows(body, env, state.editor_tips);
    if rows > 0 {
        for (offset, words) in tips(env, width).iter().enumerate() {
            frame.render_widget(
                Line::styled(text::truncate(words, width), styles.dim()),
                Rect::new(body.x, body.y + offset as u16, body.width, 1),
            );
        }
        frame.render_widget(
            Line::styled("─".repeat(width), styles.dim()),
            Rect::new(body.x, body.y + rows - 1, body.width, 1),
        );
    }
    if body.height <= rows {
        return;
    }
    let said = match away {
        Away::Leaving => format!("Handing the text to {name}…"),
        Away::Returning => format!("Reading back what was written in {name}…"),
        Away::InVim if editor_rect(area, compose, env, state.editor_tips).is_none() => {
            format!("There is no room here for {name}. Make the window larger, or zoom this pane.")
        }
        Away::InVim => return,
    };
    frame.render_widget(
        Line::styled(text::truncate(&said, width), styles.dim()),
        Rect::new(body.x, body.y + rows, body.width, 1),
    );
}

/// What the pane says while the message is in Vim: where it is, and how
/// it comes back. The fields are not drawn: they are what the message was
/// when it left, and nothing typed here changes them.
fn in_vim(frame: &mut Frame, inner: Rect, away: Away, env: &Env) {
    let styles = &env.styles;
    let width = usize::from(inner.width);
    let editor = env.external;
    let name = editor.name();
    let said: Vec<(String, bool)> = match away {
        Away::Leaving => vec![(format!("Handing this message to {name}…"), false)],
        Away::Returning => vec![(format!("Reading back what was written in {name}…"), false)],
        Away::InVim if editor.is_vim() => [
            ("This message is being written in Vim.", false),
            ("", true),
            (":QuillSend sends it.", true),
            (":QuillPostpone saves it as a draft and closes it.", true),
            (":QuillDiscard throws it away.", true),
            (
                ":wq brings it back here, to send, to save or to close.",
                true,
            ),
            (":cq leaves it as it was before Vim.", true),
        ]
        .into_iter()
        .map(|(words, dim)| (words.to_owned(), dim))
        .collect(),
        // An editor with none of Quill's commands in it: the message
        // comes back when it has been saved and the editor has gone.
        Away::InVim => vec![
            (format!("This message is being written in {name}."), false),
            (String::new(), true),
            (
                match editor {
                    ExternalEditor::Mg => {
                        "ctrl-x ctrl-s saves it, and ctrl-x ctrl-c then brings it back here, \
                         to send, to save or to close."
                    }
                    _ => "ctrl-x saves it and brings it back here, to send, to save or to close.",
                }
                .to_owned(),
                true,
            ),
        ],
    };
    let mut y = inner.y;
    for (words, dim) in &said {
        let lines = if words.is_empty() {
            vec![String::new()]
        } else {
            text::wrap(words, width)
        };
        for line in lines {
            if y >= inner.y + inner.height {
                return;
            }
            let style = if *dim { styles.dim() } else { styles.text() };
            frame.render_widget(
                Line::styled(text::truncate(&line, width), style),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
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
    format!("{} ({})", attachment.name, text::file_size(attachment.size))
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
