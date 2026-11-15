//! The attachment list: every file of the message being read, to choose
//! one to look at, open or save. Each is shown with what it is and how big,
//! so nothing is opened blind.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::state::{Env, State};
use crate::text;

/// The widest the size column gets: `1023 bytes`.
const SIZE_WIDTH: usize = 10;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(picker) = &state.attachments else {
        return;
    };
    let styles = &env.styles;
    let files = state.reader.files();
    let mut block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(" Attachments ", styles.border(true)));
    let viewport = usize::from(area.height.saturating_sub(2));
    if files.len() > viewport && viewport > 0 {
        let first = picker.offset + 1;
        let last = (picker.offset + viewport).min(files.len());
        block = block.title_bottom(
            Line::styled(format!(" {first}-{last} of {} ", files.len()), styles.dim())
                .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = usize::from(inner.width);
    // The kind of file gets a column as wide as the widest kind here, up to
    // a third of the box; the name takes the rest.
    let kind_width = files
        .iter()
        .map(|file| text::width(&file.content_type))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let name_width = width.saturating_sub(2 + kind_width + SIZE_WIDTH + 4);
    let visible = files
        .iter()
        .enumerate()
        .skip(picker.offset)
        .take(usize::from(inner.height));
    for (row, (index, file)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == picker.selected;
        let line = Line::from(vec![
            Span::raw(if selected { "> " } else { "  " }),
            Span::styled(text::fit(&file.name, name_width), styles.text()),
            Span::raw("  "),
            // What it claims to be is the sender's word: cleaned like any
            // other text from a message.
            Span::styled(
                text::fit(&text::sanitize_line(&file.content_type), kind_width),
                styles.dim(),
            ),
            Span::raw("  "),
            Span::styled(
                text::fit_right(&text::file_size(file.size), SIZE_WIDTH),
                styles.dim(),
            ),
        ]);
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    }
}
