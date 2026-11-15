//! The message list: one row per message, newest first.
//!
//! Each row starts with a cursor cell and three marker cells, so neither the
//! selection nor a message's state depends on colour: `>` is the cursor, `*`
//! unread, `!` starred, `@` has attachments.

use quill_store::{Mailbox, MessageRow};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{pane_block, placeholder};
use crate::state::{Env, Pane, State};
use crate::text;

const DATE_WIDTH: usize = 8;

/// Column widths for a pane `width` cells wide.
struct Columns {
    account: bool,
    from: usize,
    subject: usize,
}

impl Columns {
    fn for_width(width: usize, account: bool) -> Self {
        let from = (width / 4).clamp(10, 22);
        // The cursor and a gap, markers and a gap, the account letter and
        // gap, the sender and a gap, then a gap before the date.
        let fixed = 2 + 3 + 1 + if account { 2 } else { 0 } + from + 1 + 1 + DATE_WIDTH;
        Self {
            account,
            from,
            subject: width.saturating_sub(fixed),
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let styles = &env.styles;
    let focused = state.focus == Pane::List;
    let title = match state.mailbox_unread() {
        0 => state.mailbox_title(),
        unread => format!("{} - {unread} unread", state.mailbox_title()),
    };
    let block = pane_block(2, &title, focused, styles);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if state.list.rows.is_empty() {
        if state.list.loaded {
            placeholder(frame, inner, "No messages", styles);
        }
        return;
    }

    let columns = Columns::for_width(usize::from(inner.width), state.mailbox == Mailbox::Unified);
    let recipients = state.shows_recipients();
    let visible = state
        .list
        .rows
        .iter()
        .enumerate()
        .skip(state.list.offset)
        .take(usize::from(inner.height));
    for (row, (index, message)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == state.list.selected;
        frame.render_widget(
            line(message, selected, &columns, recipients, state, env),
            rect,
        );
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(focused));
        }
    }
}

fn line<'a>(
    message: &MessageRow,
    selected: bool,
    columns: &Columns,
    recipients: bool,
    state: &State,
    env: &Env,
) -> Line<'a> {
    let styles = &env.styles;
    let unread = !message.flags.seen;
    let row_style = if unread {
        styles.unread()
    } else {
        styles.text()
    };
    let mark = |on: bool, mark: &'static str| if on { mark } else { " " };

    let mut spans = vec![
        // The cursor is a marker as well as a bar, so it shows without colour.
        Span::raw(if selected { "> " } else { "  " }),
        Span::styled(mark(unread, "*"), styles.unread()),
        Span::styled(mark(message.flags.flagged, "!"), styles.starred()),
        Span::styled(mark(message.has_attachments, "@"), styles.dim()),
        Span::raw(" "),
    ];
    if columns.account {
        // The account's initial, in its colour: a letter, so it reads
        // without colour too.
        let index = state.account_index(message.account);
        let initial = index
            .and_then(|index| state.accounts.get(index))
            .and_then(|account| account.name.chars().next())
            .map_or_else(|| " ".to_owned(), |c| c.to_string());
        spans.push(Span::styled(
            text::fit(&initial, 1),
            styles.account(index.unwrap_or(0)),
        ));
        spans.push(Span::raw(" "));
    }
    let who = if recipients {
        message
            .headers
            .to
            .first()
            .map_or_else(String::new, |to| format!("To: {}", to.display_name()))
    } else {
        message
            .headers
            .from
            .first()
            .map_or_else(String::new, |from| from.display_name().to_owned())
    };
    let date = text::short_date(message.headers.date, state.now, &env.tz);
    spans.extend([
        Span::styled(text::fit(&who, columns.from), row_style),
        Span::raw(" "),
        Span::styled(
            text::fit(&message.headers.subject, columns.subject),
            row_style,
        ),
        Span::raw(" "),
        Span::styled(
            text::fit_right(&date, DATE_WIDTH),
            if unread { row_style } else { styles.dim() },
        ),
    ]);
    Line::from(spans)
}
