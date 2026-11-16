//! Where each pane goes for a given terminal size and state.
//!
//! A pure function shared by `update`, which needs pane heights to page and
//! to keep selections visible, and by the views, which draw into the result.

use ratatui::layout::Rect;

use crate::compose::{Compose, Field};
use crate::state::{MainPane, Pane, State};

/// At this many columns and above, all three panes are side by side.
pub const WIDE_COLUMNS: u16 = 100;
/// Below this many columns, the sidebar becomes an overlay.
pub const NARROW_COLUMNS: u16 = 60;
/// Below this size nothing useful fits and the views say so instead.
pub const MIN_SIZE: (u16, u16) = (24, 6);

const MEDIUM_SIDEBAR: u16 = 18;
const OVERLAY_SIDEBAR: u16 = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Sidebar, list and reader side by side.
    Wide,
    /// Sidebar beside either the list or the reader.
    Medium,
    /// The list or the reader alone; the sidebar is an overlay.
    Narrow,
}

impl LayoutMode {
    pub fn for_width(columns: u16) -> Self {
        if columns >= WIDE_COLUMNS {
            Self::Wide
        } else if columns >= NARROW_COLUMNS {
            Self::Medium
        } else {
            Self::Narrow
        }
    }
}

/// The rectangle of everything on screen. A pane that is not shown is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub mode: LayoutMode,
    pub sidebar: Option<Rect>,
    /// The sidebar is drawn over the main pane rather than beside it.
    pub sidebar_overlay: bool,
    pub list: Option<Rect>,
    pub reader: Option<Rect>,
    pub status: Rect,
    pub help: Option<Rect>,
    /// The list of links to choose from, drawn over the panes.
    pub links: Option<Rect>,
    /// The add-an-account form or a certificate question, drawn over the
    /// panes.
    pub dialog: Option<Rect>,
}

pub fn compute(state: &State, size: (u16, u16), reader_width: f32) -> Areas {
    let (width, height) = size;
    let body_height = height.saturating_sub(1);
    let body = Rect::new(0, 0, width, body_height);
    let mode = LayoutMode::for_width(width);
    let mut areas = Areas {
        mode,
        sidebar: None,
        sidebar_overlay: false,
        list: None,
        reader: None,
        status: Rect::new(0, body_height, width, height.min(1)),
        help: None,
        links: None,
        dialog: None,
    };

    // With web reading on the message is in the host's pane, not here:
    // the accounts and the list are what is drawn, at any width, and only
    // a message being written takes the list's place.
    let web = state.web_reading;
    let main_pane = if web && state.compose.is_none() {
        MainPane::List
    } else {
        state.main_pane
    };
    let show_main = |areas: &mut Areas, rect: Rect| match main_pane {
        MainPane::List => areas.list = Some(rect),
        MainPane::Reader => areas.reader = Some(rect),
    };

    if state.zoomed {
        match state.focus {
            Pane::Sidebar => areas.sidebar = Some(body),
            Pane::List => areas.list = Some(body),
            Pane::Reader => areas.reader = Some(body),
        }
    } else {
        match mode {
            LayoutMode::Wide if web => {
                let sidebar = (width / 6).clamp(MEDIUM_SIDEBAR, 28);
                areas.sidebar = Some(Rect::new(0, 0, sidebar, body_height));
                show_main(
                    &mut areas,
                    Rect::new(sidebar, 0, width - sidebar, body_height),
                );
            }
            LayoutMode::Wide => {
                let sidebar = (width / 6).clamp(MEDIUM_SIDEBAR, 28);
                let rest = width - sidebar;
                let reader = (f32::from(rest) * reader_width).round() as u16;
                let reader = reader.clamp(20, rest.saturating_sub(20).max(20));
                let list = rest.saturating_sub(reader);
                areas.sidebar = Some(Rect::new(0, 0, sidebar, body_height));
                areas.list = Some(Rect::new(sidebar, 0, list, body_height));
                areas.reader = Some(Rect::new(sidebar + list, 0, reader, body_height));
            }
            LayoutMode::Medium => {
                areas.sidebar = Some(Rect::new(0, 0, MEDIUM_SIDEBAR, body_height));
                show_main(
                    &mut areas,
                    Rect::new(MEDIUM_SIDEBAR, 0, width - MEDIUM_SIDEBAR, body_height),
                );
            }
            LayoutMode::Narrow => {
                show_main(&mut areas, body);
                if state.focus == Pane::Sidebar {
                    areas.sidebar = Some(Rect::new(0, 0, width.min(OVERLAY_SIDEBAR), body_height));
                    areas.sidebar_overlay = true;
                }
            }
        }
    }

    if state.palette.is_some() {
        // The palette keeps its size as typing narrows it, so the box does
        // not jump about: a line to type on and up to fifteen below it.
        let palette_width = width.saturating_sub(4).min(64);
        let palette_height = 18
            .min(body_height.saturating_sub(2))
            .max(3)
            .min(body_height);
        areas.links = Some(Rect::new(
            (width - palette_width) / 2,
            (body_height - palette_height) / 3,
            palette_width,
            palette_height,
        ));
    } else if state.links.is_some()
        || state.folders.is_some()
        || state.templates.is_some()
        || state.suggestions.is_some()
        || state.hooks.is_some()
        || state.attachments.is_some()
    {
        // As tall as what it lists, as far as the screen allows. The folder
        // picker has a line to type on above its list, and keeps its
        // height as typing narrows it, so the box does not jump about.
        let links_width =
            width
                .saturating_sub(4)
                .min(if state.folders.is_some() || state.templates.is_some() {
                    60
                } else if state.suggestions.is_some() {
                    44
                } else if state.attachments.is_some() || state.hooks.is_some() {
                    // Room for a name and the program it runs.
                    76
                } else {
                    100
                });
        let wanted = match &state.folders {
            Some(picker) => {
                let all = crate::state::FolderPicker::for_message(picker.message);
                state.folder_choices(&all).len() + 1
            }
            // The same: a line to type on, and every template under it.
            None if state.templates.is_some() => {
                state.templates.as_ref().map_or(0, |picker| picker.total) + 1
            }
            None if state.suggestions.is_some() => state
                .suggestions
                .as_ref()
                .map_or(0, |picker| picker.words.len()),
            // A line to type on, and every hook under it.
            None if state.hooks.is_some() => {
                state.hooks.as_ref().map_or(0, |picker| picker.total) + 1
            }
            None if state.attachments.is_some() => state.reader.files().len(),
            None => state.reader.links.len(),
        };
        let wanted = u16::try_from(wanted).unwrap_or(u16::MAX);
        let links_height = wanted
            .saturating_add(2)
            .min(body_height.saturating_sub(2))
            .max(3)
            .min(body_height);
        areas.links = Some(Rect::new(
            (width - links_width) / 2,
            (body_height - links_height) / 2,
            links_width,
            links_height,
        ));
    }
    // The form is not drawn here when the host shows it, in a sheet.
    if (state.setup.is_some() && !state.form_hosted)
        || !state.certificates.is_empty()
        || state.sign_in.is_some()
        || state.when.is_some()
        || state.unsubscribe.is_some()
    {
        let dialog_width = width.saturating_sub(4).min(76);
        let dialog_height = body_height.saturating_sub(2).min(18);
        areas.dialog = Some(Rect::new(
            (width - dialog_width) / 2,
            (body_height - dialog_height) / 2,
            dialog_width,
            dialog_height,
        ));
    }
    if state.help.is_some() {
        let help_width = width.saturating_sub(4).min(64);
        let help_height = body_height.saturating_sub(2);
        areas.help = Some(Rect::new(
            (width - help_width) / 2,
            (body_height - help_height) / 2,
            help_width,
            help_height,
        ));
    }
    areas
}

/// How many rows of the compose pane the header fields take, with the rule
/// under them: From, To, Cc, Subject always; Bcc when it is in use; the
/// attachments when there are any.
pub fn compose_header_rows(compose: &Compose) -> u16 {
    let bcc = u16::from(compose.field == Field::Bcc || !compose.bcc.is_empty());
    let attachments = u16::from(!compose.attachments.is_empty());
    4 + bcc + attachments + 1
}

/// The most rows a complaint at the foot of the compose pane may take.
pub const COMPOSE_ERROR_ROWS: usize = 3;

/// How many rows at the foot of the compose pane are taken by a complaint,
/// doubts about sending, or the line a file name is typed on. `width` is
/// the pane's text width: a long complaint wraps rather than being cut.
pub fn compose_footer_rows(compose: &Compose, width: u16) -> u16 {
    if compose.attaching.is_some() || compose.sending {
        1
    } else if let Some(error) = &compose.error {
        let rows = crate::text::wrap(error, usize::from(width))
            .len()
            .clamp(1, COMPOSE_ERROR_ROWS);
        u16::try_from(rows).unwrap_or(1)
    } else {
        u16::try_from(compose.doubts.len())
            .unwrap_or(u16::MAX)
            .min(3)
            + u16::from(!compose.doubts.is_empty())
    }
}

/// Where the body of the message being written is drawn, inside the pane
/// at `rect`.
pub fn compose_body(rect: Rect, compose: &Compose) -> Rect {
    // The pane's border and one column of padding each side.
    let inner = Rect::new(
        rect.x.saturating_add(2),
        rect.y.saturating_add(1),
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    let header = compose_header_rows(compose).min(inner.height);
    let footer = compose_footer_rows(compose, inner.width).min(inner.height - header);
    Rect::new(
        inner.x,
        inner.y + header,
        inner.width,
        inner.height - header - footer,
    )
}

/// The rows inside a bordered pane.
pub fn inner_height(rect: Rect) -> usize {
    usize::from(rect.height.saturating_sub(2))
}

/// The columns the reader's text gets: the pane less its borders and one
/// column of padding on each side.
pub fn reader_text_width(rect: Rect) -> usize {
    usize::from(rect.width.saturating_sub(4))
}

/// The reader's header lines: From, To, Date and Subject, plus Cc when the
/// message has one and a line naming its files when it has any.
pub fn reader_header_height(has_cc: bool, has_files: bool) -> usize {
    4 + usize::from(has_cc) + usize::from(has_files)
}

/// How tall a cell is taken to be, in pixels of a picture, when deciding
/// how many rows a picture is worth: a picture is never drawn at more than
/// twice its own size.
const PIXELS_PER_ROW: u32 = 20;

/// The cells the reader gives a picture of `pixels` (width, height),
/// between the header and the body: as wide as the pane allows and never
/// more than half its height, since the text matters more, with the
/// picture's own shape kept for cells twice as tall as they are wide.
/// `(0, 0)` when there is no room worth using.
pub fn reader_picture_cells(rect: Rect, header: usize, pixels: (u32, u32)) -> (u16, u16) {
    let (width, height) = (u64::from(pixels.0.max(1)), u64::from(pixels.1.max(1)));
    let columns_available = reader_text_width(rect) as u64;
    let rows_available = inner_height(rect).saturating_sub(header + 1) as u64;
    if rows_available < 4 || columns_available < 4 {
        return (0, 0);
    }
    let natural = u64::from(pixels.1.div_ceil(PIXELS_PER_ROW)).max(1);
    let mut rows = (rows_available / 2).min(natural * 2).max(1);
    let mut columns = rows * 2 * width / height;
    if columns > columns_available {
        // Too wide at that height: as wide as fits, and shorter.
        columns = columns_available;
        rows = (columns * height / (2 * width)).max(1);
    }
    (
        u16::try_from(columns.max(1)).unwrap_or(u16::MAX),
        u16::try_from(rows).unwrap_or(u16::MAX),
    )
}

/// The rows the reader's body gets, below the header and a blank line, and
/// below the image and another blank line when there is one.
pub fn reader_body_height(rect: Rect, header: usize, image_rows: usize) -> usize {
    let image = if image_rows > 0 { image_rows + 1 } else { 0 };
    inner_height(rect).saturating_sub(header + 1 + image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(focus: Pane, main_pane: MainPane) -> State {
        let mut state = State::new((0, 0), 0);
        state.focus = focus;
        state.main_pane = main_pane;
        state
    }

    fn right(rect: Rect) -> u16 {
        rect.x + rect.width
    }

    #[test]
    fn the_breakpoints_are_one_hundred_and_sixty_columns() {
        assert_eq!(LayoutMode::for_width(200), LayoutMode::Wide);
        assert_eq!(LayoutMode::for_width(100), LayoutMode::Wide);
        assert_eq!(LayoutMode::for_width(99), LayoutMode::Medium);
        assert_eq!(LayoutMode::for_width(60), LayoutMode::Medium);
        assert_eq!(LayoutMode::for_width(59), LayoutMode::Narrow);
    }

    #[test]
    fn wide_shows_three_panes_that_tile_the_width() {
        for width in [100, 120, 200, 317] {
            let areas = compute(&state(Pane::List, MainPane::List), (width, 40), 0.5);
            let (sidebar, list, reader) = (
                areas.sidebar.unwrap(),
                areas.list.unwrap(),
                areas.reader.unwrap(),
            );
            assert_eq!(sidebar.x, 0);
            assert_eq!(right(sidebar), list.x);
            assert_eq!(right(list), reader.x);
            assert_eq!(right(reader), width);
            assert_eq!(areas.status, Rect::new(0, 39, width, 1));
            assert_eq!(sidebar.height, 39);
            assert!(!areas.sidebar_overlay);
        }
    }

    #[test]
    fn reader_width_sets_the_split_right_of_the_sidebar() {
        let split = |fraction| {
            let areas = compute(&state(Pane::List, MainPane::List), (200, 40), fraction);
            (areas.list.unwrap().width, areas.reader.unwrap().width)
        };
        assert_eq!(split(0.5), (86, 86));
        let (list, reader) = split(0.25);
        assert!(reader < list);
        assert_eq!(list + reader, 172);
    }

    #[test]
    fn with_web_reading_on_no_reader_is_drawn_at_any_width() {
        for (width, sidebar) in [(200, 28), (120, 20), (100, 18), (80, 18)] {
            let mut state = state(Pane::List, MainPane::List);
            state.web_reading = true;
            let areas = compute(&state, (width, 40), 0.5);
            assert_eq!(areas.sidebar, Some(Rect::new(0, 0, sidebar, 39)));
            assert_eq!(areas.list, Some(Rect::new(sidebar, 0, width - sidebar, 39)));
            assert_eq!(areas.reader, None);
            // Whichever main pane was showing before it was switched on.
            state.main_pane = MainPane::Reader;
            state.focus = Pane::Sidebar;
            let areas = compute(&state, (width, 40), 0.5);
            assert!(areas.list.is_some() && areas.reader.is_none());
        }
        // Too narrow for the sidebar beside it: the list alone.
        let mut state = state(Pane::List, MainPane::List);
        state.web_reading = true;
        let areas = compute(&state, (50, 20), 0.5);
        assert_eq!(areas.list, Some(Rect::new(0, 0, 50, 19)));
        assert_eq!((areas.sidebar, areas.reader), (None, None));
    }

    #[test]
    fn medium_shows_the_sidebar_and_one_main_pane() {
        let areas = compute(&state(Pane::List, MainPane::List), (80, 24), 0.5);
        assert_eq!(areas.sidebar, Some(Rect::new(0, 0, 18, 23)));
        assert_eq!(areas.list, Some(Rect::new(18, 0, 62, 23)));
        assert_eq!(areas.reader, None);

        let areas = compute(&state(Pane::Reader, MainPane::Reader), (80, 24), 0.5);
        assert_eq!(areas.list, None);
        assert_eq!(areas.reader, Some(Rect::new(18, 0, 62, 23)));

        // Going to the sidebar keeps whichever main pane was showing.
        let areas = compute(&state(Pane::Sidebar, MainPane::Reader), (80, 24), 0.5);
        assert!(areas.reader.is_some() && areas.list.is_none());
    }

    #[test]
    fn narrow_overlays_the_sidebar_only_while_it_has_focus() {
        let areas = compute(&state(Pane::List, MainPane::List), (50, 20), 0.5);
        assert_eq!(areas.sidebar, None);
        assert_eq!(areas.list, Some(Rect::new(0, 0, 50, 19)));

        let areas = compute(&state(Pane::Sidebar, MainPane::List), (50, 20), 0.5);
        assert!(areas.sidebar_overlay);
        assert_eq!(areas.sidebar, Some(Rect::new(0, 0, 26, 19)));
        assert_eq!(areas.list, Some(Rect::new(0, 0, 50, 19)));
    }

    #[test]
    fn zoom_gives_the_focused_pane_everything_but_the_status_bar() {
        for (focus, width) in [(Pane::Sidebar, 200), (Pane::List, 80), (Pane::Reader, 50)] {
            let mut state = state(focus, MainPane::List);
            state.zoomed = true;
            let areas = compute(&state, (width, 30), 0.5);
            let shown = [areas.sidebar, areas.list, areas.reader];
            assert_eq!(shown.iter().flatten().count(), 1);
            assert_eq!(
                shown.iter().flatten().next(),
                Some(&Rect::new(0, 0, width, 29))
            );
        }
    }

    #[test]
    fn help_is_centred_inside_the_body() {
        let mut state = state(Pane::List, MainPane::List);
        state.help = Some(crate::state::Help {
            context: crate::command::Context::List,
            scroll: 0,
        });
        let areas = compute(&state, (80, 24), 0.5);
        assert_eq!(areas.help, Some(Rect::new(8, 1, 64, 21)));
        let areas = compute(&state, (40, 12), 0.5);
        assert_eq!(areas.help, Some(Rect::new(2, 1, 36, 9)));
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        for size in [(0, 0), (1, 1), (5, 2), (59, 1), (100, 0)] {
            for focus in [Pane::Sidebar, Pane::List, Pane::Reader] {
                let mut state = state(focus, MainPane::Reader);
                state.help = Some(crate::state::Help {
                    context: crate::command::Context::List,
                    scroll: 0,
                });
                compute(&state, size, 0.5);
                state.zoomed = true;
                compute(&state, size, 0.5);
            }
        }
    }

    #[test]
    fn reader_geometry_accounts_for_borders_padding_and_headers() {
        let rect = Rect::new(0, 0, 62, 23);
        assert_eq!(inner_height(rect), 21);
        assert_eq!(reader_text_width(rect), 58);
        assert_eq!(reader_body_height(rect, 4, 0), 16);
        assert_eq!(reader_body_height(rect, 5, 0), 15);
        assert_eq!(reader_body_height(Rect::new(0, 0, 10, 3), 5, 0), 0);
        // A line of files in the header is a line less of body.
        assert_eq!(reader_header_height(false, false), 4);
        assert_eq!(reader_header_height(true, true), 6);
        assert_eq!(
            reader_body_height(rect, reader_header_height(false, true), 0),
            15
        );
    }

    #[test]
    fn a_picture_takes_at_most_half_the_reader_and_keeps_its_shape() {
        // 58 columns and 55 rows below the header to work with.
        let tall = Rect::new(0, 0, 62, 62);
        // 320 x 200 pixels is ten rows at its own size: drawn at up to
        // twice that, 20 rows, which is 64 columns; cut to the 58 there
        // are, and shorter to match.
        assert_eq!(reader_picture_cells(tall, 4, (320, 200)), (58, 18));
        // A picture taller than it is wide is limited by height.
        assert_eq!(reader_picture_cells(tall, 4, (200, 800)), (13, 27));
        // A small one is not blown up past twice its size.
        assert_eq!(reader_picture_cells(tall, 4, (60, 40)), (12, 4));
        // 16 rows below the header: the picture gets 8, the body the rest.
        let short = Rect::new(0, 0, 62, 23);
        assert_eq!(reader_picture_cells(short, 4, (320, 200)), (25, 8));
        assert_eq!(reader_body_height(short, 4, 8), 7);
        // Too small to be worth it.
        assert_eq!(
            reader_picture_cells(Rect::new(0, 0, 62, 9), 4, (320, 200)),
            (0, 0)
        );
        assert_eq!(
            reader_picture_cells(Rect::new(0, 0, 6, 40), 4, (320, 200)),
            (0, 0)
        );
        // Nonsense sizes do not divide by zero.
        let (columns, rows) = reader_picture_cells(tall, 4, (0, 0));
        assert!(columns >= 1 && rows >= 1);
    }
}
