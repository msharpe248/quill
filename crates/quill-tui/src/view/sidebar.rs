//! The accounts pane: the unified inbox, then each account and, while it
//! is open, its folders.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{pane_block, placeholder};
use crate::state::{Env, Pane, SidebarEntry, State};
use crate::{layout, text};

/// The fewest cells kept on the right for an unread count and a margin.
const COUNT_WIDTH: usize = 5;

/// What stands before an account's name: whether its folders are on show.
/// A click on it, or anywhere left of it, opens and closes.
const OPEN: &str = "▾ ";
const CLOSED: &str = "▸ ";

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let styles = &env.styles;
    let focused = state.focus == Pane::Sidebar;
    let block = pane_block(1, "Accounts", focused, styles);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if state.sidebar.entries.is_empty() {
        placeholder(frame, inner, "No accounts", styles);
        return;
    }

    let width = usize::from(inner.width);
    // The column is as wide as the largest count needs. A number is never
    // cut short: "150…" for 15,055 would be a wrong number, so a long count
    // takes its room from the names beside it.
    let widest = state
        .sidebar
        .entries
        .iter()
        .map(|entry| match entry {
            SidebarEntry::Unified { unread }
            | SidebarEntry::Account { unread, .. }
            | SidebarEntry::Folder { unread, .. } => *unread,
            SidebarEntry::Local { count, .. } => *count,
            SidebarEntry::Searches | SidebarEntry::Search { .. } | SidebarEntry::OnThisMac => 0,
        })
        .max()
        .unwrap_or(0);
    let count_width = COUNT_WIDTH.max(widest.to_string().len() + 2);
    let label_width = width.saturating_sub(count_width);
    // The folders of a closed account are not among the rows shown.
    let shown = state.sidebar.shown();
    let visible = shown
        .iter()
        .skip(state.sidebar.offset)
        .take(usize::from(inner.height))
        .filter_map(|index| Some((*index, state.sidebar.entries.get(*index)?)));
    for (row, (index, entry)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == state.sidebar.selected;
        // The cursor is a marker as well as a bar, so it shows without colour.
        let cursor = if selected { "> " } else { "  " };
        let line = match entry {
            SidebarEntry::Unified { unread } => {
                counted_row(cursor, "Unified", *unread, label_width, count_width, env)
            }
            SidebarEntry::Local { name, count, .. } => {
                counted_row(cursor, name, *count, label_width, count_width, env)
            }
            SidebarEntry::Account {
                index: position,
                id,
                name,
                unread,
            } => {
                let open = state.sidebar.is_open(*id);
                let marker = Span::styled(if open { OPEN } else { CLOSED }, styles.dim());
                // Open, its unread mail is counted on its inbox, the row
                // below. Closed, this row is where the count is: in the
                // column of the others, and the name has what is left.
                let count = if open || *unread == 0 {
                    String::new()
                } else {
                    unread.to_string()
                };
                // The letter that marks the account's mail where every
                // account's is listed together, before its name: which
                // letter in that list is which account.
                let initial = state
                    .accounts
                    .get(*position)
                    .and_then(|account| account.initial())
                    .filter(|letter| !letter.is_control());
                let mut letter = initial.map_or_else(String::new, |letter| format!("{letter} "));
                // What the name and the count may take: the row, less
                // the cursor, the marker and the margin on the right;
                // and of that, what the name may.
                let room = width.saturating_sub(usize::from(layout::SIDEBAR_MARKER_CELLS) + 1);
                let beside = |letter: &str| {
                    let taken = if count.is_empty() { 0 } else { count.len() + 1 };
                    room.saturating_sub(text::width(letter) + taken)
                };
                // A letter that is only the first of the name says nothing
                // the name does not: where it would cut a name short that
                // has room without it, the name has the room. A letter the
                // user chose is always shown.
                let named = text::width(name);
                if initial == name.chars().next() && named > beside(&letter) && named <= beside("")
                {
                    letter.clear();
                }
                let room = room.saturating_sub(text::width(&letter));
                let name_room = beside(&letter);
                Line::from(vec![
                    Span::raw(cursor),
                    marker,
                    Span::styled(letter, styles.account(*position)),
                    Span::styled(text::fit(name, name_room), styles.account(*position)),
                    Span::styled(text::fit_right(&count, room - name_room), styles.unread()),
                ])
            }
            // A folder that is only on this Mac sits a step further in,
            // under the heading that says so, and a folder inside another
            // a step further in than that one.
            SidebarEntry::Folder {
                label,
                unread,
                local,
                depth,
                ..
            } => counted_row(
                cursor,
                &format!(
                    "{}{}{label}",
                    if *local { "    " } else { "  " },
                    "  ".repeat(usize::from(*depth))
                ),
                *unread,
                label_width,
                count_width,
                env,
            ),
            SidebarEntry::OnThisMac => Line::from(vec![
                Span::raw(cursor),
                Span::styled("  On This Mac", styles.dim()),
            ]),
            SidebarEntry::Searches => Line::from(vec![
                Span::raw(cursor),
                Span::styled("Searches", styles.dim()),
            ]),
            SidebarEntry::Search { name, .. } => counted_row(
                cursor,
                &format!("  {name}"),
                0,
                label_width,
                count_width,
                env,
            ),
        };
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(focused));
        }
    }
}

/// A row with its unread count right-aligned; bold when anything is unread.
fn counted_row<'a>(
    cursor: &'a str,
    label: &str,
    unread: u32,
    label_width: usize,
    count_width: usize,
    env: &Env,
) -> Line<'a> {
    let style = if unread > 0 {
        env.styles.unread()
    } else {
        env.styles.text()
    };
    let count = if unread > 0 {
        unread.to_string()
    } else {
        String::new()
    };
    Line::from(vec![
        Span::raw(cursor),
        Span::styled(text::fit(label, label_width.saturating_sub(2)), style),
        Span::styled(text::fit_right(&count, count_width - 1), style),
    ])
}
