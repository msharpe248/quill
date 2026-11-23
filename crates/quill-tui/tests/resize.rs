//! The edges between the panes can be dragged with the mouse, and where
//! they are left is remembered.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_tui::layout;
use quill_tui::setup::{AccountRequest, Preference};
use quill_tui::state::{Action, Mouse, MouseKind, Pane};

fn mouse(harness: &mut Harness, kind: MouseKind, (column, row): (u16, u16)) {
    harness.act(Action::Mouse(Mouse { kind, column, row }));
}

/// Press at one cell, move to another column and let go.
fn drag(harness: &mut Harness, from: (u16, u16), to: u16) {
    mouse(harness, MouseKind::Press, from);
    mouse(harness, MouseKind::Drag, ((from.0 + to) / 2, from.1));
    mouse(harness, MouseKind::Drag, (to, from.1 + 3));
    mouse(harness, MouseKind::Release, (to, from.1 + 3));
}

/// The columns of the accounts, the list and the message, each nothing
/// where it is not drawn.
fn widths(harness: &Harness) -> (u16, u16, u16) {
    let areas = layout::compute(&harness.state, harness.state.size, harness.env.reader_width);
    let width = |rect: Option<ratatui::layout::Rect>| rect.map_or(0, |rect| rect.width);
    (width(areas.sidebar), width(areas.list), width(areas.reader))
}

/// What was asked to be remembered since the last look.
fn remembered(harness: &mut Harness) -> Vec<Preference> {
    std::mem::take(&mut harness.account_requests)
        .into_iter()
        .filter_map(|request| match request {
            AccountRequest::Remember(preference) => Some(preference),
            _ => None,
        })
        .collect()
}

/// A harness started as Quill is the next time, with what was remembered
/// in its config.
fn started_again(width: u16, height: u16, remembered: &[Preference]) -> Harness {
    let mut harness = Harness::demo(width, height);
    for preference in remembered {
        match *preference {
            Preference::SidebarWidth(columns) => harness.env.sidebar_width = columns,
            Preference::ReaderWidth(Some(share)) => {
                harness.env.reader_width = f32::from(share) / 1000.0;
            }
            Preference::ReaderWidth(None) => harness.env.reader_width = 0.5,
            _ => {}
        }
    }
    harness.act(Action::ClockTick(common::NOW));
    harness
}

#[test]
fn the_accounts_edge_is_dragged_and_the_message_keeps_its_columns() {
    let mut harness = Harness::demo(200, 60);
    assert_eq!(widths(&harness), (28, 86, 86));

    // The edge is the accounts' right border and the list's left one:
    // taken by the first, it stays under the pointer.
    drag(&mut harness, (27, 10), 39);
    assert_eq!(widths(&harness), (40, 74, 86));
    assert!(harness.state.resizing.is_none());
    let kept = remembered(&mut harness);
    assert!(
        kept.contains(&Preference::SidebarWidth(Some(40))),
        "{kept:?}"
    );
    assert!(
        kept.iter()
            .any(|kept| matches!(kept, Preference::ReaderWidth(Some(_)))),
        "{kept:?}"
    );
    // The next run is laid out the same.
    assert_eq!(widths(&started_again(200, 60, &kept)), (40, 74, 86));

    // And taken by the list's border, narrower again.
    drag(&mut harness, (40, 30), 20);
    assert_eq!(widths(&harness), (20, 94, 86));
    let kept = remembered(&mut harness);
    assert_eq!(widths(&started_again(200, 60, &kept)), (20, 94, 86));

    // Nothing was selected, opened or moved to by any of it.
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert!(harness.host_requests.is_empty());
}

#[test]
fn the_edge_between_the_list_and_the_message_is_dragged() {
    let mut harness = Harness::demo(200, 60);
    assert_eq!(widths(&harness), (28, 86, 86));
    // The message starts at column 114: its left border is the edge.
    drag(&mut harness, (114, 20), 134);
    assert_eq!(widths(&harness), (28, 106, 66));
    let kept = remembered(&mut harness);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(
        matches!(kept[0], Preference::ReaderWidth(Some(share)) if (380..=390).contains(&share))
    );
    assert_eq!(widths(&started_again(200, 60, &kept)), (28, 106, 66));

    // By the list's right border, the other way.
    drag(&mut harness, (133, 5), 93);
    assert_eq!(widths(&harness), (28, 66, 106));
    let kept = remembered(&mut harness);
    assert_eq!(widths(&started_again(200, 60, &kept)), (28, 66, 106));
}

#[test]
fn no_pane_is_dragged_narrower_than_it_can_be_used() {
    let mut harness = Harness::demo(200, 60);
    // The accounts: no narrower than a name needs, and no wider than
    // leaves the list its least beside a message that keeps its columns.
    drag(&mut harness, (27, 10), 0);
    assert_eq!(widths(&harness), (12, 102, 86));
    drag(&mut harness, (12, 10), 199);
    assert_eq!(widths(&harness), (60, 54, 86));

    // The message: a share the config can hold, either way.
    let mut harness = Harness::demo(200, 60);
    drag(&mut harness, (114, 10), 199);
    let (accounts, list, message) = widths(&harness);
    assert_eq!((accounts, list + message), (28, 172));
    assert_eq!(message, 34, "a fifth of what is right of the accounts");
    drag(&mut harness, (28 + list, 10), 0);
    assert_eq!(widths(&harness), (28, 34, 138));
    for kept in remembered(&mut harness) {
        match kept {
            Preference::ReaderWidth(Some(share)) => assert!((200..=800).contains(&share)),
            other => panic!("{other:?}"),
        }
    }

    // In a window just wide enough for three panes each keeps its least.
    let mut harness = Harness::demo(100, 30);
    assert_eq!(widths(&harness), (18, 41, 41));
    drag(&mut harness, (18, 10), 99);
    let (accounts, list, message) = widths(&harness);
    assert_eq!(accounts + list + message, 100);
    assert!(list >= 20 && message == 41, "{accounts} {list} {message}");
}

#[test]
fn a_press_that_moves_nothing_changes_and_remembers_nothing() {
    let mut harness = Harness::demo(200, 60);
    mouse(&mut harness, MouseKind::Press, (27, 10));
    assert!(harness.state.resizing.is_some());
    // The edge in hand is drawn as a focused pane's border is.
    let buffer = harness.buffer();
    let style = harness.env.styles.border(true);
    assert_eq!(buffer[(27, 10)].fg, style.fg.unwrap());
    assert_eq!(buffer[(28, 10)].fg, style.fg.unwrap());
    mouse(&mut harness, MouseKind::Drag, (27, 14));
    mouse(&mut harness, MouseKind::Release, (27, 14));
    assert!(harness.state.resizing.is_none());
    assert_eq!(widths(&harness), (28, 86, 86));
    assert!(remembered(&mut harness).is_empty());
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
}

#[test]
fn an_edge_whose_letting_go_never_came_is_kept_at_the_next_press() {
    let mut harness = Harness::demo(200, 60);
    mouse(&mut harness, MouseKind::Press, (27, 10));
    mouse(&mut harness, MouseKind::Drag, (35, 10));
    assert_eq!(widths(&harness).0, 36);
    assert!(remembered(&mut harness).is_empty());
    // The pointer left the window with the button down. The next press
    // is a click like any other, and the edge is where it was left.
    mouse(&mut harness, MouseKind::Press, (60, 10));
    assert!(harness.state.resizing.is_none());
    assert!(remembered(&mut harness).contains(&Preference::SidebarWidth(Some(36))));
    assert_eq!(widths(&harness).0, 36);
    // A drag with nothing in hand moves no edge.
    mouse(&mut harness, MouseKind::Drag, (90, 10));
    assert_eq!(widths(&harness).0, 36);
}

#[test]
fn a_double_click_on_an_edge_puts_it_back() {
    let mut harness = Harness::demo(200, 60);
    drag(&mut harness, (27, 10), 39);
    drag(&mut harness, (126, 10), 150);
    let moved = widths(&harness);
    assert_ne!(moved, (28, 86, 86));
    remembered(&mut harness);

    let click = |harness: &mut Harness, at| {
        mouse(harness, MouseKind::Press, at);
        mouse(harness, MouseKind::Release, at);
    };
    // One click does nothing.
    click(&mut harness, (40, 10));
    assert_eq!(widths(&harness), moved);
    // Nor do two with a key between them, or with time between them.
    harness.keys("j");
    click(&mut harness, (40, 10));
    assert_eq!(widths(&harness), moved);
    harness.act(Action::ClockTick(common::NOW + 30));
    click(&mut harness, (40, 10));
    assert_eq!(widths(&harness), moved);
    assert!(remembered(&mut harness).is_empty());

    // Two together: the accounts go by the window's width again, and the
    // setting is taken out.
    click(&mut harness, (40, 10));
    assert_eq!(widths(&harness).0, 28);
    assert_eq!(remembered(&mut harness), [Preference::SidebarWidth(None)]);

    // And the message has half of what is beside the accounts again.
    let edge = 28 + widths(&harness).1;
    click(&mut harness, (edge, 10));
    click(&mut harness, (edge, 10));
    assert_eq!(widths(&harness), (28, 86, 86));
    assert_eq!(remembered(&mut harness), [Preference::ReaderWidth(None)]);
    assert_eq!(widths(&started_again(200, 60, &[])), (28, 86, 86));
}

#[test]
fn beside_one_pane_the_accounts_edge_moves_and_the_share_is_left_alone() {
    let mut harness = Harness::demo(80, 24);
    assert_eq!(widths(&harness), (18, 62, 0));
    drag(&mut harness, (18, 10), 30);
    assert_eq!(widths(&harness), (30, 50, 0));
    assert_eq!(
        remembered(&mut harness),
        [Preference::SidebarWidth(Some(30))]
    );
    // The pane beside them keeps its least.
    drag(&mut harness, (30, 10), 79);
    assert_eq!(widths(&harness), (60, 20, 0));
    remembered(&mut harness);

    // The same columns in a wider window, with the message beside.
    harness.resize(200, 60);
    assert_eq!(widths(&harness), (60, 70, 70));
    // And held to what a narrower one has room for.
    harness.resize(70, 24);
    assert_eq!(widths(&harness), (50, 20, 0));
}

#[test]
fn the_config_says_how_wide_the_accounts_are() {
    let mut harness = started_again(200, 60, &[Preference::SidebarWidth(Some(44))]);
    assert_eq!(widths(&harness), (44, 78, 78));
    harness.resize(80, 24);
    assert_eq!(widths(&harness), (44, 36, 0));
    // Under sixty columns they are drawn over the list, as ever.
    harness.resize(50, 24);
    assert_eq!(widths(&harness), (0, 50, 0));
}

#[test]
fn there_is_no_edge_to_take_where_a_pane_has_the_screen_or_lies_over_another() {
    // Zoomed: one pane, and a press where the edge was is a click in it.
    let mut harness = Harness::demo(200, 60);
    harness.keys("z");
    mouse(&mut harness, MouseKind::Press, (27, 10));
    assert!(harness.state.resizing.is_none());
    mouse(&mut harness, MouseKind::Drag, (60, 10));
    mouse(&mut harness, MouseKind::Release, (60, 10));
    harness.keys("z");
    assert_eq!(widths(&harness), (28, 86, 86));

    // The accounts over the list in a narrow terminal.
    let mut harness = Harness::demo(50, 24);
    harness.keys("1");
    assert_eq!(widths(&harness), (26, 50, 0));
    drag(&mut harness, (25, 10), 40);
    assert_eq!(harness.state.sidebar_width, None);
    assert!(remembered(&mut harness).is_empty());

    // And nothing behind the help or a list to choose from is moved.
    let mut harness = Harness::demo(200, 60);
    harness.keys("?");
    drag(&mut harness, (27, 10), 39);
    assert_eq!(harness.state.sidebar_width, None);
    assert!(remembered(&mut harness).is_empty());
}

#[test]
fn beside_the_reading_pane_the_accounts_edge_moves_and_the_pane_stays_where_it_is() {
    let mut harness = Harness::in_the_app(147, 40, true);
    harness.act(Action::WindowColumns(147));
    harness.settle();
    let place = |harness: &Harness| harness.state.reading_place(&harness.env).unwrap();
    let kept = place(&harness);
    harness.resize(kept, 40);
    let (accounts, list, _) = widths(&harness);
    assert_eq!(accounts + list, kept);

    drag(&mut harness, (accounts, 10), accounts + 10);
    assert_eq!(widths(&harness), (accounts + 10, list - 10, 0));
    // The terminal keeps the columns it had: the pane does not move.
    assert_eq!(place(&harness), kept);
    let kept_settings = remembered(&mut harness);
    assert!(kept_settings.contains(&Preference::SidebarWidth(Some(accounts + 10))));

    // Read as text in the same window, the message has the pane's place.
    harness.keys("w");
    harness.resize(147, 40);
    let (_, _, message) = widths(&harness);
    assert_eq!(message, 147 - kept);
}

#[test]
fn the_dividers_beside_the_page_and_beside_the_text_are_one_edge() {
    // The window of the report: 283 columns, the accounts dragged to 39
    // of them, and the message with 128.
    let mut harness = Harness::in_the_app(283, 40, true);
    harness.env.sidebar_width = Some(39);
    harness.env.reader_width = 0.525;
    harness.act(Action::WindowColumns(283));
    harness.settle();
    let place = |harness: &Harness| harness.state.reading_place(&harness.env).unwrap();
    assert_eq!(place(&harness), 155);
    harness.resize(155, 40);
    assert_eq!(widths(&harness), (39, 116, 0));

    // The divider beside the page is dragged ten columns to the right,
    // in the host's window: the terminal is to keep 165.
    harness.act(Action::ReadingDragged(Some(165)));
    assert_eq!(place(&harness), 165);
    harness.resize(165, 40);
    assert_eq!(widths(&harness), (39, 126, 0));
    // Kept for the next run, as the message's share.
    let kept = remembered(&mut harness);
    assert!(
        matches!(kept.as_slice(), [Preference::ReaderWidth(Some(share))] if (480..=487).contains(share)),
        "{kept:?}"
    );

    // Read as text, the message is where the page was: the list did not
    // move, and nor did the edge.
    harness.keys("w");
    harness.resize(283, 40);
    assert_eq!(widths(&harness), (39, 126, 118));
    // And as a page again.
    harness.keys("w");
    harness.settle();
    assert_eq!(place(&harness), 165);

    // The next run starts with it there, either way.
    let mut again = started_again(283, 40, &kept);
    again.env.sidebar_width = Some(39);
    again.act(Action::ClockTick(common::NOW));
    assert_eq!(widths(&again), (39, 126, 118));

    // A double click on the divider beside the page puts the edge back
    // where Quill has it by itself, for both.
    remembered(&mut harness);
    harness.act(Action::ReadingDragged(None));
    assert_eq!(remembered(&mut harness), [Preference::ReaderWidth(None)]);
    let usual = place(&harness);
    assert_eq!(usual, 39 + 122);
    harness.keys("w");
    harness.resize(283, 40);
    assert_eq!(widths(&harness), (39, 122, 122));
}

#[test]
fn a_divider_dragged_beside_the_page_leaves_every_pane_room() {
    let mut harness = Harness::in_the_app(147, 40, true);
    harness.act(Action::WindowColumns(147));
    harness.settle();
    let place = |harness: &Harness| harness.state.reading_place(&harness.env).unwrap();
    let (accounts, _, _) = {
        harness.resize(place(&harness), 40);
        widths(&harness)
    };
    // All the way left, and all the way right: the list and the message
    // each keep their least, and the share is one the config can hold.
    for asked in [1, accounts, 146, 147, u16::MAX] {
        harness.act(Action::ReadingDragged(Some(asked)));
        let kept = place(&harness);
        // Twenty columns is the least a pane is drawn in.
        assert!(kept >= accounts + 20, "{asked}: {kept}");
        assert!(147 - kept >= 20, "{asked}: {kept}");
        for preference in remembered(&mut harness) {
            match preference {
                Preference::ReaderWidth(Some(share)) => assert!((200..=800).contains(&share)),
                other => panic!("{other:?}"),
            }
        }
    }

    // With the accounts hidden the terminal keeps the list alone, and a
    // drag is of the list's columns.
    harness.act(Action::ReadingDragged(None));
    harness.keys("\\");
    let before = place(&harness);
    harness.act(Action::ReadingDragged(Some(before + 7)));
    assert_eq!(place(&harness), before + 7);

    // Nothing is dragged beside a page that is not there.
    let mut harness = in_a_window(147);
    let was = widths(&harness);
    harness.act(Action::ReadingDragged(Some(60)));
    assert_eq!(widths(&harness), was);
    assert!(remembered(&mut harness).is_empty());
}

/// The app with the text reader on show, in a window of as many columns
/// as the terminal has.
fn in_a_window(width: u16) -> Harness {
    let mut harness = Harness::in_the_app(width, 40, false);
    harness.act(Action::WindowColumns(width));
    harness
}

#[test]
fn the_reading_pane_has_the_place_the_message_was_dragged_to() {
    let mut harness = in_a_window(147);
    let (accounts, list, message) = widths(&harness);
    assert_eq!(accounts + list + message, 147);

    // The message is given twenty columns more, read as text.
    let edge = accounts + list;
    drag(&mut harness, (edge, 10), edge - 20);
    assert_eq!(widths(&harness), (accounts, list - 20, message + 20));

    // The page has those columns, in the same run: the terminal keeps the
    // accounts' and the list's as they were left.
    harness.keys("w");
    harness.settle();
    let kept = harness.state.reading_place(&harness.env).unwrap();
    assert_eq!(kept, accounts + list - 20);
    harness.resize(kept, 40);
    assert_eq!(widths(&harness), (accounts, list - 20, 0));

    // And back as text nothing has moved.
    harness.keys("w");
    harness.resize(147, 40);
    assert_eq!(widths(&harness), (accounts, list - 20, message + 20));
}

#[test]
fn the_reading_pane_has_the_messages_place_beside_accounts_of_another_width() {
    // The accounts made wider, read as text.
    let mut harness = in_a_window(147);
    let (accounts, _, message) = widths(&harness);
    drag(&mut harness, (accounts, 10), accounts + 10);
    let (wider, list, kept_message) = widths(&harness);
    assert_eq!((wider, kept_message), (accounts + 10, message));
    let kept_settings = remembered(&mut harness);

    harness.keys("w");
    harness.settle();
    assert_eq!(
        harness.state.reading_place(&harness.env),
        Some(wider + list)
    );

    // The next run, with what was remembered in its config, comes to the
    // same place: the accounts' own columns count there too.
    let mut again = Harness::in_the_app(147, 40, true);
    for preference in &kept_settings {
        match *preference {
            Preference::SidebarWidth(columns) => again.env.sidebar_width = columns,
            Preference::ReaderWidth(Some(share)) => {
                again.env.reader_width = f32::from(share) / 1000.0;
            }
            _ => {}
        }
    }
    again.act(Action::WindowColumns(147));
    again.settle();
    assert_eq!(again.state.reading_place(&again.env), Some(wider + list));
}

#[test]
fn a_narrow_window_gives_the_reading_pane_the_share_the_message_was_dragged_to() {
    // Dragged in a wide window, then the window made too narrow for three
    // panes: the page has the share the message was left with.
    let mut harness = in_a_window(147);
    let (accounts, list, _) = widths(&harness);
    let edge = accounts + list;
    drag(&mut harness, (edge, 10), edge - 20);
    let share = harness.state.reader_share.unwrap();
    assert!(share > 0.5, "{share}");

    harness.keys("w");
    harness.act(Action::WindowColumns(90));
    harness.settle();
    let page = (90.0 * share).round() as u16;
    assert_eq!(harness.state.reading_place(&harness.env), Some(90 - page));
}
