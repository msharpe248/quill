//! `view(State)`: draws the state and changes nothing.

mod attachments;
pub mod compose;
mod folders;
pub mod help;
mod hooks;
mod links;
mod list;
mod palette;
mod reader;
mod setup;
mod sidebar;
mod status;
mod suggestions;
mod templates;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::layout;
use crate::state::{Env, State};
use crate::theme::Styles;

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

/// A run of link text in the frame just rendered, for the run loop to mark
/// as a hyperlink so the terminal can open it on a click.
///
/// Like images, hyperlinks are not something a cell can hold. The view draws
/// the text as usual and says where it is; the run loop writes the same text
/// again, marked. Only underlined link text is ever marked, and every piece
/// of it is written again on every frame, so a cell cannot keep a link it
/// no longer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPlacement {
    pub x: u16,
    pub y: u16,
    /// The text exactly as drawn, already clipped to the pane.
    pub text: String,
    /// The style it was drawn in.
    pub style: Style,
    /// Where it leads, when that is an address Quill opens. `None` writes
    /// the text with no link.
    pub url: Option<String>,
}

/// What the terminal should draw over the frame just rendered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overlays {
    pub images: Vec<ImagePlacement>,
    pub links: Vec<LinkPlacement>,
}

/// Draws the state into `frame`. Returns what to place over it.
pub fn render(frame: &mut Frame, state: &State, env: &Env) -> Overlays {
    let area = frame.area();
    frame.buffer_mut().set_style(area, env.styles.base());
    if area.width < layout::MIN_SIZE.0 || area.height < layout::MIN_SIZE.1 {
        frame.render_widget(
            Paragraph::new("Quill needs a larger window").style(env.styles.dim()),
            area,
        );
        return Overlays::default();
    }

    // Laid out for the frame's own size, which is ahead of `state.size` for
    // one draw while a resize event is still in the queue.
    let areas = layout::compute(state, (area.width, area.height), env.reader_width);
    if let Some(rect) = areas.list {
        list::render(frame, rect, state, env);
    }
    let mut images = Vec::new();
    let mut links = Vec::new();
    if let Some(rect) = areas.reader {
        // A message being written takes the reader's place.
        if state.compose.is_some() {
            compose::render(frame, rect, state, env);
        } else {
            images.extend(reader::render(frame, rect, state, env, &mut links));
        }
    }
    if let Some(rect) = areas.sidebar {
        if areas.sidebar_overlay {
            clear(frame, rect, &env.styles);
        }
        sidebar::render(frame, rect, state, env);
    }
    status::render(frame, areas.status, state, env);
    if let Some(rect) = areas.links {
        clear(frame, rect, &env.styles);
        if state.palette.is_some() {
            palette::render(frame, rect, state, env);
        } else if state.folders.is_some() {
            folders::render(frame, rect, state, env);
        } else if state.templates.is_some() {
            templates::render(frame, rect, state, env);
        } else if state.suggestions.is_some() {
            suggestions::render(frame, rect, state, env);
        } else if state.hooks.is_some() {
            hooks::render(frame, rect, state, env);
        } else if state.attachments.is_some() {
            attachments::render(frame, rect, state, env);
        } else {
            links::render(frame, rect, state, env);
        }
    }
    if let Some(rect) = areas.dialog {
        clear(frame, rect, &env.styles);
        setup::render(frame, rect, state, env);
    }
    if let Some(rect) = areas.help {
        clear(frame, rect, &env.styles);
        help::render(frame, rect, state, env);
    }
    // The terminal draws images above text, so one under an overlay would
    // show through it. While anything covers an image, it is not placed.
    // Link text is written over the frame too, and must not be written over
    // an overlay.
    let overlays = [
        areas.help,
        areas.dialog,
        areas.links,
        areas.sidebar.filter(|_| areas.sidebar_overlay),
    ];
    let clear_of_overlays = |area: Rect| {
        overlays
            .iter()
            .flatten()
            .all(|overlay| !overlay.intersects(area))
    };
    images.retain(|image| clear_of_overlays(image.area));
    links.retain(|link| {
        let width = u16::try_from(crate::text::width(&link.text)).unwrap_or(u16::MAX);
        clear_of_overlays(Rect::new(link.x, link.y, width.max(1), 1))
    });
    Overlays { images, links }
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
