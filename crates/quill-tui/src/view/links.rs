//! The link picker: every link in the message being read, by number, to
//! choose one to open. Each is shown in full as far as the width allows, so
//! where a link really leads is in view before it is opened.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::state::{Env, State};
use crate::text;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(picker) = &state.links else {
        return;
    };
    let styles = &env.styles;
    let links = &state.reader.links;
    let mut block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(" Links ", styles.border(true)));
    let viewport = usize::from(area.height.saturating_sub(2));
    if links.len() > viewport && viewport > 0 {
        let first = picker.offset + 1;
        let last = (picker.offset + viewport).min(links.len());
        block = block.title_bottom(
            Line::styled(format!(" {first}-{last} of {} ", links.len()), styles.dim())
                .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = usize::from(inner.width);
    // Numbers line up on the right of their column.
    let number_width = links.len().to_string().len() + 2;
    let visible = links
        .iter()
        .enumerate()
        .skip(picker.offset)
        .take(usize::from(inner.height));
    for (row, (index, url)) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let selected = index == picker.selected;
        let number = format!("[{}]", index + 1);
        let line = Line::from(vec![
            Span::raw(if selected { "> " } else { "  " }),
            Span::styled(format!("{number:>number_width$} "), styles.dim()),
            Span::styled(
                text::truncate(url, width.saturating_sub(number_width + 3)),
                // What Quill will not open is shown, but set back.
                if quill_mime::links::is_openable(url) {
                    styles.text()
                } else {
                    styles.dim()
                },
            ),
        ]);
        frame.render_widget(line, rect);
        if selected {
            frame.buffer_mut().set_style(rect, styles.selected(true));
        }
    }
}
