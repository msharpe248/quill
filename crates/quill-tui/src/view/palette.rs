//! The command palette: a line to type on, and under it what matches, each
//! command with its key.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::state::{Env, State};
use crate::text;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(palette) = &state.palette else {
        return;
    };
    let styles = &env.styles;
    let items = state.palette_items(env);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(" Commands ", styles.border(true)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // The line being typed, with the cursor in it.
    let width = usize::from(inner.width);
    let (typed, column) = palette.input.view(width.saturating_sub(2));
    let line = Line::from(vec![
        Span::styled(": ", styles.dim()),
        Span::styled(
            text::fit(&typed, width.saturating_sub(2)),
            styles.text().add_modifier(Modifier::UNDERLINED),
        ),
    ]);
    frame.render_widget(line, Rect::new(inner.x, inner.y, inner.width, 1));
    frame.set_cursor_position(Position::new(inner.x + 2 + column as u16, inner.y));

    let rows = usize::from(inner.height - 1);
    if items.is_empty() {
        if rows > 0 {
            let none = Line::styled("  Nothing matches.", styles.dim());
            frame.render_widget(none, Rect::new(inner.x, inner.y + 1, inner.width, 1));
        }
        return;
    }
    let visible = items.iter().enumerate().skip(palette.offset).take(rows);
    for (row, (index, item)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + 1 + row as u16, inner.width, 1);
        let selected = index == palette.selected;
        // The key sits at the right edge; the title has the rest.
        let keys_width = text::width(&item.keys);
        let title_width = width.saturating_sub(2 + keys_width + 2);
        let line = Line::from(vec![
            Span::raw(if selected { "> " } else { "  " }),
            Span::styled(text::fit(&item.title, title_width), styles.text()),
            Span::raw("  "),
            Span::styled(item.keys.as_str(), styles.key()),
        ]);
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    }
}
