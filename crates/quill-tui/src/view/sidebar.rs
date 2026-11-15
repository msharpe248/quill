//! The accounts pane: the unified inbox, then each account and its folders.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{pane_block, placeholder};
use crate::state::{Env, Pane, SidebarEntry, State};
use crate::text;

/// Cells kept on the right for an unread count and a margin.
const COUNT_WIDTH: usize = 5;

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
    let label_width = width.saturating_sub(COUNT_WIDTH);
    let visible = state
        .sidebar
        .entries
        .iter()
        .enumerate()
        .skip(state.sidebar.offset)
        .take(usize::from(inner.height));
    for (row, (index, entry)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == state.sidebar.selected;
        // The cursor is a marker as well as a bar, so it shows without colour.
        let cursor = if selected { "> " } else { "  " };
        let line = match entry {
            SidebarEntry::Unified { unread } => {
                counted_row(cursor, "Unified", *unread, label_width, env)
            }
            SidebarEntry::Account { index, name } => Line::from(vec![
                Span::raw(cursor),
                Span::styled(
                    text::truncate(name, width.saturating_sub(2)),
                    styles.account(*index),
                ),
            ]),
            SidebarEntry::Folder { name, unread, .. } => {
                counted_row(cursor, &format!("  {name}"), *unread, label_width, env)
            }
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
        Span::styled(text::fit_right(&count, COUNT_WIDTH - 1), style),
    ])
}
