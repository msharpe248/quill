//! `view(State)`: draws the state and changes nothing.

pub mod help;
mod list;
mod reader;
mod sidebar;
mod status;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::layout;
use crate::state::{Env, State};
use crate::theme::Styles;

/// The id the built-in sample image is known to the terminal by.
pub const SAMPLE_IMAGE_ID: u32 = 1;

/// Where the terminal should draw an image over the frame just rendered.
///
/// Images are not cells, so they are not part of the frame: the views leave
/// the cells under an image empty and say where it goes, and the run loop
/// asks the terminal to draw it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImagePlacement {
    pub id: u32,
    pub area: Rect,
}

/// Draws the state into `frame`. Returns the images to place over it.
pub fn render(frame: &mut Frame, state: &State, env: &Env) -> Vec<ImagePlacement> {
    let area = frame.area();
    frame.buffer_mut().set_style(area, env.styles.base());
    if area.width < layout::MIN_SIZE.0 || area.height < layout::MIN_SIZE.1 {
        frame.render_widget(
            Paragraph::new("Quill needs a larger window").style(env.styles.dim()),
            area,
        );
        return Vec::new();
    }

    // Laid out for the frame's own size, which is ahead of `state.size` for
    // one draw while a resize event is still in the queue.
    let areas = layout::compute(state, (area.width, area.height), env.reader_width);
    if let Some(rect) = areas.list {
        list::render(frame, rect, state, env);
    }
    let mut images = Vec::new();
    if let Some(rect) = areas.reader {
        images.extend(reader::render(frame, rect, state, env));
    }
    if let Some(rect) = areas.sidebar {
        if areas.sidebar_overlay {
            clear(frame, rect, &env.styles);
        }
        sidebar::render(frame, rect, state, env);
    }
    status::render(frame, areas.status, state, env);
    if let Some(rect) = areas.help {
        clear(frame, rect, &env.styles);
        help::render(frame, rect, state, env);
    }
    // The terminal draws images above text, so one under an overlay would
    // show through it. While anything covers an image, it is not placed.
    let overlays = [areas.help, areas.sidebar.filter(|_| areas.sidebar_overlay)];
    images.retain(|image| {
        overlays
            .iter()
            .flatten()
            .all(|overlay| !overlay.intersects(image.area))
    });
    images
}

/// Empties a rectangle so an overlay can be drawn over what was there.
fn clear(frame: &mut Frame, rect: Rect, styles: &Styles) {
    frame.render_widget(Clear, rect);
    frame.buffer_mut().set_style(rect, styles.base());
}

/// The frame of a pane. Focus shows as a thick, bold border as well as a
/// colour, so it survives `NO_COLOR`. The number is the key that focuses it.
fn pane_block<'a>(number: u8, title: &str, focused: bool, styles: &Styles) -> Block<'a> {
    let title_style = if focused {
        styles.border(true)
    } else {
        styles.dim()
    };
    Block::bordered()
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(styles.border(focused))
        .title(Line::from(Span::styled(
            format!(" [{number}] {title} "),
            title_style,
        )))
}

/// A dim, centred line for a pane with nothing to show.
fn placeholder(frame: &mut Frame, inner: Rect, message: &str, styles: &Styles) {
    if inner.height == 0 {
        return;
    }
    let row = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
    frame.render_widget(
        Paragraph::new(message)
            .style(styles.dim())
            .alignment(Alignment::Center),
        row,
    );
}
