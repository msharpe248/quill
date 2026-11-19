//! The message list: one row per message, newest first.
//!
//! Each row starts with a cursor cell, a selection cell and three marker
//! cells, so neither the cursor nor a message's state depends on colour: `>`
//! is the cursor, `+` selected to be acted on with others, `*` unread, `!`
//! starred, `@` has attachments.

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
    // While searching, the title is the search: what is typed, and how
    // much it finds.
    let title = match &state.search {
        // A rule being tried: the list is what it would change, and the
        // title says what it would do with it.
        Some(_) if state.rule_trial().is_some() => state.trial_title().unwrap_or_default(),
        Some(search) => {
            let found = match (search.found, search.more) {
                (Some(count), true) => format!(" - {count}+ found"),
                (Some(1), false) => " - 1 found".to_owned(),
                (Some(count), false) => format!(" - {count} found"),
                (None, _) => String::new(),
            };
            // The box follows its cursor within the room the border has.
            let room = usize::from(area.width).saturating_sub(16 + text::width(&found));
            let (typed, _) = search.input.view(room.max(4));
            format!("Search: {typed}{found}")
        }
        None => match state.mailbox_unread() {
            0 => state.mailbox_title(),
            unread => format!("{} - {unread} unread", state.mailbox_title()),
        },
    };
    let range = state.list.range_rows();
    let is_marked = |index: usize, message: &MessageRow| {
        state.list.marked.contains(&message.id)
            || range.is_some_and(|(first, last)| (first..=last).contains(&index))
    };
    let title = if state.list.has_marks() {
        let count = state
            .list
            .rows
            .iter()
            .enumerate()
            .filter(|(index, message)| is_marked(*index, message))
            .count();
        format!("{title} - {count} selected")
    } else {
        title
    };
    let block = pane_block(2, &title, focused, styles);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(search) = &state.search
        && search.typing
    {
        // The cursor sits in the title, after what has been typed: past
        // the corner, the space, "[2] " and "Search: ".
        let room = usize::from(area.width).saturating_sub(16);
        let (_, column) = search.input.view(room.max(4));
        let x = area.x + 2 + 4 + 8 + column as u16;
        if x < area.x + area.width.saturating_sub(1) {
            frame.set_cursor_position(ratatui::layout::Position::new(x, area.y));
        }
    }
    if state.list.rows.is_empty() {
        if state.list.loaded {
            let words = match &state.search {
                Some(_) if state.rule_trial().is_some() => {
                    "Nothing in the inbox, as far back as this looks, would be changed"
                }
                Some(search) if search.input.is_empty() => {
                    "Words, \"a phrase\", from:ana, is:unread, in:work/sent"
                }
                Some(_) => "Nothing found",
                None => "No messages",
            };
            placeholder(frame, inner, words, styles);
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
            line(
                message,
                Row {
                    selected,
                    marked: is_marked(index, message),
                    follows: state.list.follows.get(index).copied().unwrap_or(false),
                },
                &columns,
                recipients,
                state,
                env,
            ),
            rect,
        );
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(focused));
        }
    }
}

/// Where a row stands in the list, beyond what its message says.
#[derive(Clone, Copy)]
struct Row {
    /// The cursor is on it.
    selected: bool,
    /// It is selected to be acted on with others.
    marked: bool,
    /// It carries on the conversation of the row above.
    follows: bool,
}

fn line<'a>(
    message: &MessageRow,
    row: Row,
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
        Span::raw(if row.selected { ">" } else { " " }),
        Span::styled(if row.marked { "+" } else { " " }, styles.accent()),
        // One column says where the message stands: not yet read, or
        // read and answered.
        if unread {
            Span::styled("*", styles.unread())
        } else {
            Span::styled(mark(message.flags.answered, "r"), styles.dim())
        },
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
    // A snoozed message says when it comes back, not when it came.
    let date = text::short_date(
        message.snoozed_until.unwrap_or(message.headers.date),
        state.now,
        &env.tz,
    );
    spans.extend([
        Span::styled(text::fit(&who, columns.from), row_style),
        Span::raw(" "),
        // A message that carries on the conversation above hangs from it.
        Span::styled(
            if row.follows {
                text::fit(&format!("└ {}", message.headers.subject), columns.subject)
            } else {
                text::fit(&message.headers.subject, columns.subject)
            },
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
