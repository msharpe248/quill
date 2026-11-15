//! Where each pane goes for a given terminal size and state.
//!
//! A pure function shared by `update`, which needs pane heights to page and
//! to keep selections visible, and by the views, which draw into the result.

use ratatui::layout::Rect;

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
    };

    let show_main = |areas: &mut Areas, rect: Rect| match state.main_pane {
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

    if state.links.is_some() {
        // As tall as its links, as far as the screen allows.
        let links_width = width.saturating_sub(4).min(100);
        let wanted = u16::try_from(state.reader.links.len()).unwrap_or(u16::MAX);
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
/// message has one.
pub fn reader_header_height(has_cc: bool) -> usize {
    if has_cc { 5 } else { 4 }
}

/// The most rows an inline image may take in the reader.
const IMAGE_ROWS: usize = 10;

/// The rows the reader gives an inline image, between the header and the
/// body. `drawn` is whether the terminal can draw it; where it cannot, one
/// row names the image instead.
pub fn reader_image_height(rect: Rect, has_cc: bool, shown: bool, drawn: bool) -> usize {
    let available = inner_height(rect).saturating_sub(reader_header_height(has_cc) + 1);
    match (shown, drawn) {
        (false, _) => 0,
        (true, false) => available.min(1),
        // Never more than half the pane: the text matters more.
        (true, true) if available >= 4 => IMAGE_ROWS.min(available / 2),
        (true, true) => 0,
    }
}

/// The rows the reader's body gets, below the header and a blank line, and
/// below the image and another blank line when there is one.
pub fn reader_body_height(rect: Rect, has_cc: bool, image_rows: usize) -> usize {
    let image = if image_rows > 0 { image_rows + 1 } else { 0 };
    inner_height(rect).saturating_sub(reader_header_height(has_cc) + 1 + image)
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
        assert_eq!(reader_body_height(rect, false, 0), 16);
        assert_eq!(reader_body_height(rect, true, 0), 15);
        assert_eq!(reader_body_height(Rect::new(0, 0, 10, 3), true, 0), 0);
    }

    #[test]
    fn an_inline_image_takes_at_most_half_the_reader() {
        let tall = Rect::new(0, 0, 62, 60);
        assert_eq!(reader_image_height(tall, false, false, true), 0);
        assert_eq!(reader_image_height(tall, false, true, true), 10);
        // Where the terminal cannot draw it, one row names it.
        assert_eq!(reader_image_height(tall, false, true, false), 1);
        // 16 rows below the header: the image gets 8, a gap 1, the body 7.
        let short = Rect::new(0, 0, 62, 23);
        assert_eq!(reader_image_height(short, false, true, true), 8);
        assert_eq!(reader_body_height(short, false, 8), 7);
        // Too small to be worth it.
        assert_eq!(
            reader_image_height(Rect::new(0, 0, 62, 10), false, true, true),
            0
        );
        assert_eq!(
            reader_image_height(Rect::new(0, 0, 62, 3), true, true, false),
            0
        );
    }
}
