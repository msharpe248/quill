//! Other spellings of a word in the message being written, to choose one
//! from.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::state::{Env, State};
use crate::text;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(picker) = &state.suggestions else {
        return;
    };
    let styles = &env.styles;
    // The word is the user's own, and is cleaned like anything else that
    // is drawn; a long one is cut so the title still fits the box.
    let room = usize::from(area.width).saturating_sub(20);
    let word = text::truncate(&text::sanitize_line(&picker.word), room.max(4));
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(
            format!(" Spelling of \"{word}\" "),
            styles.border(true),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);
    let visible = picker
        .words
        .iter()
        .enumerate()
        .skip(picker.offset)
        .take(usize::from(inner.height));
    for (row, (index, suggestion)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == picker.selected;
        let line = Line::from(vec![
            Span::raw(if selected { "> " } else { "  " }),
            Span::styled(
                text::truncate(suggestion, width.saturating_sub(2)),
                styles.text(),
            ),
        ]);
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    }
}
