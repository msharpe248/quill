//! The folder picker: where to move a message. A line to type on narrows
//! the list beneath it.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::state::{Env, State};
use crate::text;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(picker) = &state.folders else {
        return;
    };
    let styles = &env.styles;
    let choices = state.folder_choices(picker);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(
            if picker.copy {
                " Copy to "
            } else {
                " Move to "
            },
            styles.border(true),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // The line being typed, with the cursor in it.
    let width = usize::from(inner.width);
    let (typed, column) = picker.filter.view(width.saturating_sub(2));
    let line = Line::from(vec![
        Span::styled("/ ", styles.dim()),
        Span::styled(
            text::fit(&typed, width.saturating_sub(2)),
            styles.text().add_modifier(Modifier::UNDERLINED),
        ),
    ]);
    frame.render_widget(line, Rect::new(inner.x, inner.y, inner.width, 1));
    frame.set_cursor_position(Position::new(inner.x + 2 + column as u16, inner.y));

    let rows = usize::from(inner.height - 1);
    if choices.is_empty() {
        if rows > 0 {
            let none = Line::styled("  No folder matches.", styles.dim());
            frame.render_widget(none, Rect::new(inner.x, inner.y + 1, inner.width, 1));
        }
        return;
    }
    let visible = choices.iter().enumerate().skip(picker.offset).take(rows);
    for (row, (index, choice)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1);
        let selected = index == picker.selected;
        // A folder that is only on this Mac is said to be, where it is
        // about to be chosen: mail moved there leaves its server.
        let name = if choice.local {
            format!("{}{}", choice.name, crate::state::ON_THIS_MAC_SUFFIX)
        } else {
            choice.name.clone()
        };
        let line = Line::from(vec![
            Span::raw(if selected { "> " } else { "  " }),
            Span::styled(
                text::truncate(&name, width.saturating_sub(2)),
                styles.text(),
            ),
        ]);
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    }
}
