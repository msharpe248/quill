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
/// The cells before an account's name in the accounts pane, inside its
/// border: the cursor's two, then the marker that says whether its folders
/// are on show and a space. A click there opens and closes the account.
pub const SIDEBAR_MARKER_CELLS: u16 = 4;

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

/// The fewest columns the list or the reader is given beside the other.
const MIN_PANE: u16 = 20;

/// The accounts pane's columns where the window is wide enough for all
/// three panes.
fn wide_sidebar(width: u16) -> u16 {
    (width / 6).clamp(MEDIUM_SIDEBAR, 28)
}

/// How `width` columns, at least [`WIDE_COLUMNS`] of them, are divided
/// among the accounts, the list and the reader. With the accounts hidden
/// (`accounts` false) they have no columns, and the other two have them
/// all.
fn wide_split(width: u16, reader_width: f32, accounts: bool) -> (u16, u16, u16) {
    let sidebar = if accounts { wide_sidebar(width) } else { 0 };
    let rest = width - sidebar;
    let reader = (f32::from(rest) * reader_width).round() as u16;
    let reader = reader.clamp(MIN_PANE, rest.saturating_sub(MIN_PANE).max(MIN_PANE));
    (sidebar, rest.saturating_sub(reader), reader)
}

/// How many columns the terminal keeps while the host's reading pane
/// stands beside it, in a window with room for `window` columns: the
/// accounts' and the list's, so that the pane has exactly the reader's
/// place and neither of the others moves when it comes or goes.
///
/// With the accounts hidden (`accounts` false) the terminal keeps the
/// list's columns alone: the list stays as wide as it was and the pane
/// has the room the accounts gave up.
///
/// A window too narrow for three panes has no reader beside its list. The
/// pane is given the reader's share of the whole window there, and the
/// terminal draws in what is left.
pub fn beside_reading(window: u16, reader_width: f32, accounts: bool) -> u16 {
    if window >= WIDE_COLUMNS {
        let (sidebar, list, _) = wide_split(window, reader_width, true);
        if accounts { sidebar + list } else { list }
    } else {
        let reader = (f32::from(window) * reader_width).round() as u16;
        window
            .saturating_sub(reader)
            .max(MIN_SIZE.0)
            .min(window)
            .max(1)
    }
}

/// Where the accounts and the list go while the message is in the host's
/// reading pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WebColumns {
    /// The columns they are drawn in, from the left.
    drawn: u16,
    /// The accounts' columns, when they are beside the list.
    sidebar: Option<u16>,
}

impl WebColumns {
    /// `window`: the columns the host's whole window has room for, once
    /// it has said. `width`: the terminal's own.
    ///
    /// The widths are the window's, not the terminal's: the terminal is
    /// narrower by the reader's columns, and its own width would give
    /// another split. A terminal that still has the whole window, because
    /// the pane is not up yet, is drawn in as it will be once it is, so
    /// that nothing moves when the pane arrives. One of any other width
    /// (the user has moved the divider) gives the difference to the list.
    ///
    /// `accounts`: whether the accounts are on show at all. Hidden, they
    /// have no columns at any width.
    fn of(window: Option<u16>, width: u16, reader_width: f32, accounts: bool) -> Self {
        let Some(window) = window else {
            // A host that has not said: by the terminal's own width.
            let sidebar = match LayoutMode::for_width(width) {
                LayoutMode::Wide => Some(wide_sidebar(width)),
                LayoutMode::Medium => Some(MEDIUM_SIDEBAR),
                LayoutMode::Narrow => None,
            };
            return Self {
                drawn: width,
                sidebar: sidebar.filter(|_| accounts),
            };
        };
        let drawn = if width >= window {
            beside_reading(window, reader_width, accounts).min(width)
        } else {
            width
        };
        let sidebar = if window >= WIDE_COLUMNS {
            Some(wide_sidebar(window)).filter(|sidebar| drawn >= sidebar + MIN_PANE)
        } else {
            None
        };
        Self {
            drawn,
            sidebar: sidebar
                .or_else(|| (drawn >= NARROW_COLUMNS).then_some(MEDIUM_SIDEBAR))
                .filter(|_| accounts),
        }
    }

    fn mode(self) -> LayoutMode {
        match self.sidebar {
            Some(_) if self.drawn >= WIDE_COLUMNS => LayoutMode::Wide,
            Some(_) => LayoutMode::Medium,
            None => LayoutMode::Narrow,
        }
    }
}

/// How the panes are arranged in a terminal of `width` columns: what
/// [`compute`] goes by.
pub fn mode(state: &State, width: u16, reader_width: f32) -> LayoutMode {
    if state.web_reading {
        let accounts = !state.sidebar_hidden;
        WebColumns::of(state.window_columns, width, reader_width, accounts).mode()
    } else {
        LayoutMode::for_width(width)
    }
}

/// Whether the accounts have no place of their own beside the other
/// panes, and so are drawn over them while they have the keys and are
/// gone again after: where there is no room for them, and wherever the
/// user has hidden them.
pub fn sidebar_floats(state: &State, width: u16, reader_width: f32) -> bool {
    state.sidebar_hidden || mode(state, width, reader_width) == LayoutMode::Narrow
}

pub fn compute(state: &State, size: (u16, u16), reader_width: f32) -> Areas {
    let (width, height) = size;
    let body_height = height.saturating_sub(1);
    let body = Rect::new(0, 0, width, body_height);
    // With web reading on the message is in the host's pane, not here:
    // the accounts and the list are what is drawn, at any width, and only
    // a message being written takes the list's place.
    let accounts = !state.sidebar_hidden;
    let web = state
        .web_reading
        .then(|| WebColumns::of(state.window_columns, width, reader_width, accounts));
    let mode = web.map_or_else(|| LayoutMode::for_width(width), WebColumns::mode);
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

    let main_pane = if web.is_some() && state.compose.is_none() {
        MainPane::List
    } else {
        state.main_pane
    };
    let show_main = |areas: &mut Areas, rect: Rect| match main_pane {
        MainPane::List => areas.list = Some(rect),
        MainPane::Reader => areas.reader = Some(rect),
    };
    // The accounts where they have no columns of their own: over the
    // left of what is drawn in `across` columns, while they have the keys.
    let float = |areas: &mut Areas, across: u16| {
        if state.focus == Pane::Sidebar {
            areas.sidebar = Some(Rect::new(0, 0, across.min(OVERLAY_SIDEBAR), body_height));
            areas.sidebar_overlay = true;
        }
    };

    if state.zoomed {
        match state.focus {
            Pane::Sidebar => areas.sidebar = Some(body),
            Pane::List => areas.list = Some(body),
            Pane::Reader => areas.reader = Some(body),
        }
    } else if let Some(web) = web {
        match web.sidebar {
            Some(sidebar) => {
                areas.sidebar = Some(Rect::new(0, 0, sidebar, body_height));
                show_main(
                    &mut areas,
                    Rect::new(sidebar, 0, web.drawn - sidebar, body_height),
                );
            }
            None => {
                show_main(&mut areas, Rect::new(0, 0, web.drawn, body_height));
                float(&mut areas, web.drawn);
            }
        }
    } else {
        match mode {
            LayoutMode::Wide => {
                let (sidebar, list, reader) = wide_split(width, reader_width, accounts);
                areas.list = Some(Rect::new(sidebar, 0, list, body_height));
                areas.reader = Some(Rect::new(sidebar + list, 0, reader, body_height));
                if accounts {
                    areas.sidebar = Some(Rect::new(0, 0, sidebar, body_height));
                } else {
                    float(&mut areas, width);
                }
            }
            LayoutMode::Medium if !accounts => {
                show_main(&mut areas, body);
                float(&mut areas, width);
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
                float(&mut areas, width);
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
    // A form or a question is not drawn here when the host shows it, in a
    // sheet.
    if !state.form_hosted
        && (state.setup.is_some()
            || !state.certificates.is_empty()
            || state.sign_in.is_some()
            || state.when.is_some()
            || state.unsubscribe.is_some()
            || state.remove.is_some())
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
    fn with_the_accounts_hidden_the_others_have_the_width_and_the_accounts_float() {
        for width in [60, 80, 100, 120, 200, 317] {
            let mut hidden = state(Pane::List, MainPane::List);
            hidden.sidebar_hidden = true;
            let areas = compute(&hidden, (width, 40), 0.5);
            assert_eq!(areas.sidebar, None, "{width}");
            let list = areas.list.unwrap();
            assert_eq!(list.x, 0, "{width}");
            match areas.reader {
                Some(reader) => {
                    assert!(width >= WIDE_COLUMNS);
                    assert_eq!(right(list), reader.x, "{width}");
                    assert_eq!(right(reader), width, "{width}");
                    assert!(list.width >= MIN_PANE && reader.width >= MIN_PANE);
                }
                None => assert_eq!(list.width, width, "{width}"),
            }
            assert!(sidebar_floats(&hidden, width, 0.5));

            // With the keys, the accounts are over the left of the rest,
            // which stays where it was.
            hidden.focus = Pane::Sidebar;
            let over = compute(&hidden, (width, 40), 0.5);
            assert_eq!(over.sidebar, Some(Rect::new(0, 0, OVERLAY_SIDEBAR, 39)));
            assert!(over.sidebar_overlay);
            assert_eq!((over.list, over.reader), (areas.list, areas.reader));
        }
        // Shown, they float only where there is no room beside the rest.
        let shown = state(Pane::List, MainPane::List);
        assert!(!sidebar_floats(&shown, 100, 0.5) && !sidebar_floats(&shown, 60, 0.5));
        assert!(sidebar_floats(&shown, 59, 0.5));
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

    /// A state with web reading on in a window with room for `window`
    /// columns.
    fn reading_in(window: u16) -> State {
        let mut state = state(Pane::List, MainPane::List);
        state.web_reading = true;
        state.window_columns = Some(window);
        state
    }

    const SHARES: [f32; 6] = [0.2, 0.33, 0.5, 0.58, 0.62, 0.8];

    #[test]
    fn the_reading_pane_has_the_readers_place_to_the_column() {
        // At every width with three panes and every share the config
        // allows: with the pane beside it the terminal draws the accounts
        // and the list exactly where they are with the reader drawn.
        for share in SHARES {
            for window in WIDE_COLUMNS..=420 {
                let off = compute(&state(Pane::List, MainPane::List), (window, 40), share);
                let (sidebar, list, reader) =
                    (off.sidebar.unwrap(), off.list.unwrap(), off.reader.unwrap());
                let kept = beside_reading(window, share, true);
                assert_eq!(kept, sidebar.width + list.width, "{window} {share}");
                assert_eq!(kept + reader.width, window, "{window} {share}");

                let on = compute(&reading_in(window), (kept, 40), share);
                assert_eq!(on.sidebar, Some(sidebar), "{window} {share}");
                assert_eq!(on.list, Some(list), "{window} {share}");
                assert_eq!(on.reader, None);
                assert!(!on.sidebar_overlay);
            }
        }
    }

    #[test]
    fn the_place_is_the_same_however_often_it_is_asked_for_and_whatever_came_before() {
        // Nothing is remembered: the place is worked out from the window
        // alone, so switching back and forth, or resizing in between,
        // cannot make it creep.
        let widths = [147_u16, 100, 233, 147, 99, 147, 180, 147];
        let first = beside_reading(147, 0.5, true);
        for (index, window) in widths.into_iter().enumerate() {
            let kept = beside_reading(window, 0.5, true);
            if window == 147 {
                assert_eq!(kept, first, "at step {index}");
            }
            // The terminal narrowed to it is still told the same.
            let on = compute(&reading_in(window), (kept, 40), 0.5);
            let again = compute(&reading_in(window), (kept, 40), 0.5);
            assert_eq!(on, again);
            assert_eq!(
                on.list.map(|list| list.x + list.width),
                Some(kept),
                "{window}"
            );
        }
    }

    #[test]
    fn before_the_reading_pane_is_up_the_panes_are_already_where_they_will_be() {
        // The terminal still has the whole window: the columns the pane
        // is about to cover are left empty, and nothing moves when it
        // comes.
        for window in [100_u16, 147, 200, 317] {
            let kept = beside_reading(window, 0.5, true);
            let waiting = compute(&reading_in(window), (window, 40), 0.5);
            let beside = compute(&reading_in(window), (kept, 40), 0.5);
            assert_eq!(waiting.sidebar, beside.sidebar, "{window}");
            assert_eq!(waiting.list, beside.list, "{window}");
            assert_eq!(waiting.reader, None);
            // The status line still runs the width of the terminal.
            assert_eq!(waiting.status, Rect::new(0, 39, window, 1));
        }
    }

    #[test]
    fn a_divider_the_user_moved_gives_the_difference_to_the_list() {
        let window = 147;
        let sidebar = compute(&state(Pane::List, MainPane::List), (window, 40), 0.5)
            .sidebar
            .unwrap();
        for width in [sidebar.width + MIN_PANE, 60, 85, 99, 100, 130, 146] {
            let areas = compute(&reading_in(window), (width, 40), 0.5);
            assert_eq!(areas.sidebar, Some(sidebar), "{width}");
            assert_eq!(
                areas.list,
                Some(Rect::new(sidebar.width, 0, width - sidebar.width, 39)),
                "{width}"
            );
            assert_ne!(mode(&reading_in(window), width, 0.5), LayoutMode::Narrow);
        }
        // With no room for a list beside the accounts, the list alone,
        // and the accounts over it when they are wanted.
        let width = sidebar.width + MIN_PANE - 1;
        let areas = compute(&reading_in(window), (width, 40), 0.5);
        assert_eq!(areas.list, Some(Rect::new(0, 0, width, 39)));
        assert_eq!(areas.sidebar, None);
        assert_eq!(mode(&reading_in(window), width, 0.5), LayoutMode::Narrow);
        let mut state = reading_in(window);
        state.focus = Pane::Sidebar;
        let areas = compute(&state, (width, 40), 0.5);
        assert!(areas.sidebar_overlay);
        assert_eq!(areas.sidebar, Some(Rect::new(0, 0, OVERLAY_SIDEBAR, 39)));
    }

    #[test]
    fn a_window_too_narrow_for_three_panes_gives_the_pane_the_readers_share() {
        for share in SHARES {
            for window in 1..WIDE_COLUMNS {
                let kept = beside_reading(window, share, true);
                assert!(kept >= 1 && kept <= window, "{window} {share}: {kept}");
                // The terminal is never asked to be too narrow to draw
                // in while the window has the room.
                assert!(kept >= MIN_SIZE.0.min(window), "{window} {share}: {kept}");
                let areas = compute(&reading_in(window), (kept, 40), share);
                let list = areas.list.unwrap();
                assert_eq!(list.x + list.width, kept, "{window} {share}");
                assert_eq!(areas.reader, None);
                // Beside the accounts where there is room for them, as
                // in a terminal of that width.
                assert_eq!(
                    areas.sidebar.map(|sidebar| sidebar.width),
                    (kept >= NARROW_COLUMNS).then_some(MEDIUM_SIDEBAR),
                    "{window} {share}"
                );
            }
        }
        // Half of eighty columns each, by default.
        assert_eq!(beside_reading(80, 0.5, true), 40);
        assert_eq!(beside_reading(99, 0.5, true), 49);
    }

    #[test]
    fn the_mode_says_whether_the_accounts_are_beside_the_list() {
        for window in [40_u16, 80, 100, 147, 250] {
            for width in 1..=window {
                let mut state = reading_in(window);
                let beside = compute(&state, (width, 40), 0.5).sidebar.is_some();
                assert_eq!(
                    mode(&state, width, 0.5) != LayoutMode::Narrow,
                    beside,
                    "{window} {width}"
                );
                state.focus = Pane::Sidebar;
                assert_eq!(
                    compute(&state, (width, 40), 0.5).sidebar_overlay,
                    !beside,
                    "{window} {width}"
                );
            }
        }
        // With the reader drawn here it is the terminal's width alone.
        let off = state(Pane::List, MainPane::List);
        assert_eq!(mode(&off, 59, 0.5), LayoutMode::Narrow);
        assert_eq!(mode(&off, 99, 0.5), LayoutMode::Medium);
        assert_eq!(mode(&off, 100, 0.5), LayoutMode::Wide);
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
            all: false,
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
                    all: false,
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
