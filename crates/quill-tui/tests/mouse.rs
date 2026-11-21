//! The mouse: clicks select and open, the wheel scrolls what is under the
//! pointer, and nothing it does is beyond the keyboard.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_store::Mailbox;
use quill_tui::command::Context;
use quill_tui::compose::Field;
use quill_tui::state::{Action, HostRequest, Mouse, MouseKind, Pane};

/// The cell where `needle` first appears on screen, searching rows from
/// `from_row` down.
fn find_from(harness: &Harness, needle: &str, from_row: usize) -> (u16, u16) {
    for (row, line) in harness.screen().lines().enumerate().skip(from_row) {
        if let Some(at) = line.find(needle) {
            let column = quill_tui::text::width(&line[..at]);
            return (column as u16, row as u16);
        }
    }
    panic!("{needle:?} is not on screen:\n{}", harness.screen());
}

fn find(harness: &Harness, needle: &str) -> (u16, u16) {
    find_from(harness, needle, 0)
}

fn mouse(harness: &mut Harness, kind: MouseKind, (column, row): (u16, u16)) {
    harness.act(Action::Mouse(Mouse { kind, column, row }));
}

fn click(harness: &mut Harness, at: (u16, u16)) {
    mouse(harness, MouseKind::Press, at);
}

fn click_on(harness: &mut Harness, needle: &str) {
    let at = find(harness, needle);
    click(harness, at);
}

#[test]
fn a_click_selects_a_message_and_a_second_opens_it() {
    let mut harness = Harness::demo(200, 60);
    assert_eq!(harness.state.focus, Pane::List);
    click_on(&mut harness, "Sunday dinner");
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    assert_eq!(harness.state.focus, Pane::List);
    // Selecting is not reading: it is still unread.
    assert!(
        !harness.state.list.rows[harness.state.list.selected]
            .flags
            .seen
    );
    // The reader shows it, as when the cursor is moved there with keys.
    assert!(harness.screen().contains("Subject: Sunday dinner"));

    click_on(&mut harness, "Sunday dinner");
    assert_eq!(harness.state.focus, Pane::Reader);
    assert!(
        harness.state.list.rows[harness.state.list.selected]
            .flags
            .seen
    );
}

#[test]
fn a_click_in_another_pane_focuses_it_first() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    // The message under the list's cursor: the click moves the focus and
    // does not open it, since the list did not have the focus.
    click_on(&mut harness, "Q4 roadmap");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(!harness.state.list.rows[0].flags.seen);
    // The empty space below the last message selects nothing.
    let (column, _) = find(&harness, "Q4 roadmap");
    click(&mut harness, (column, 50));
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    // A click in the reader focuses it.
    click_on(&mut harness, "Hi Robin,");
    assert_eq!(harness.state.focus, Pane::Reader);
}

#[test]
fn the_sidebar_is_clicked_through_to_a_mailbox() {
    let mut harness = Harness::demo(200, 60);
    // "Sent" appears twice; the first is Work's.
    click_on(&mut harness, "    Sent");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    assert!(matches!(harness.state.mailbox, Mailbox::Folder(_)));
    assert!(harness.row_with("[2]").contains("Work / Sent"));
    // A click on the mailbox on show goes to its mail.
    click_on(&mut harness, ">   Sent");
    assert_eq!(harness.state.focus, Pane::List);
    // An account's name is somewhere to go too: its inbox.
    click_on(&mut harness, "Personal");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    assert!(harness.row_with("[2]").contains("Inbox (Personal)"));
    assert!(harness.screen().contains("> ▾ P Personal"));
    // Nothing was closed by it.
    assert!(harness.state.sidebar.closed.is_empty());
}

#[test]
fn the_wheel_moves_what_is_under_the_pointer() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::Reader);
    // Over the list: the list's cursor moves three rows, and the focus
    // stays where it was.
    let over_list = find(&harness, "Sunday dinner");
    mouse(&mut harness, MouseKind::WheelDown, over_list);
    assert_eq!(harness.state.list.selected, 3);
    assert_eq!(harness.state.focus, Pane::Reader);
    mouse(&mut harness, MouseKind::WheelUp, over_list);
    mouse(&mut harness, MouseKind::WheelUp, over_list);
    assert_eq!(harness.state.list.selected, 0);

    // Over the reader: the text scrolls, and stops at its ends.
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    let over_reader = find(&harness, "Hi Robin,");
    mouse(&mut harness, MouseKind::WheelDown, over_reader);
    assert_eq!(harness.state.reader.scroll, 3);
    for _ in 0..20 {
        mouse(&mut harness, MouseKind::WheelDown, over_reader);
    }
    assert_eq!(harness.state.reader.scroll, 10);
    for _ in 0..20 {
        mouse(&mut harness, MouseKind::WheelUp, over_reader);
    }
    assert_eq!(harness.state.reader.scroll, 0);
}

#[test]
fn a_link_in_the_text_and_the_line_of_files_can_be_clicked() {
    let mut harness = Harness::demo(200, 60);
    // The newsletter has links in its HTML.
    harness.keys("G k k enter");
    let link = harness.state.reader.links[0].clone();
    // Its first link's text, somewhere in the body.
    let (index, text) = harness
        .state
        .reader
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find_map(|span| {
            span.link
                .filter(|_| span.style.link)
                .map(|index| (index, span.text.clone()))
        })
        .unwrap();
    assert_eq!(index, 0);
    let at = find_from(&harness, text.trim(), 6);
    click(&mut harness, at);
    assert_eq!(harness.host_requests, [HostRequest::OpenUrl(link)]);
    // Text that is not a link opens nothing.
    harness.host_requests.clear();
    click(&mut harness, (at.0, 58));
    assert!(harness.host_requests.is_empty());

    // The number after a link opens it too, and so does its line in the
    // list of links under the message: on the number or on the address.
    let second = harness.state.reader.links[1].clone();
    let number = find_from(&harness, "issue[2]", 6);
    click(&mut harness, (number.0 + 6, number.1));
    assert_eq!(
        harness.host_requests,
        [HostRequest::OpenUrl(second.clone())]
    );
    harness.host_requests.clear();
    let listed = find_from(&harness, &format!("[2] {second}"), 6);
    click(&mut harness, listed);
    click(&mut harness, (listed.0 + 10, listed.1));
    assert_eq!(
        harness.host_requests,
        [
            HostRequest::OpenUrl(second.clone()),
            HostRequest::OpenUrl(second)
        ]
    );
    harness.host_requests.clear();
    // The space past the end of the address is not the link.
    click(&mut harness, (listed.0 + 60, listed.1));
    assert!(harness.host_requests.is_empty());

    // The line naming a message's files opens their list.
    let mut harness = Harness::demo(200, 60);
    click_on(&mut harness, "Attach:");
    assert_eq!(harness.state.context(), Context::Attachments);
    // A click on the file chosen is `enter`; one beside the box closes it.
    let chosen = find(&harness, "> roadmap.pdf");
    click(&mut harness, (chosen.0 + 4, chosen.1));
    assert!(matches!(
        harness.host_requests.as_slice(),
        [HostRequest::Preview { .. }]
    ));
    click(&mut harness, (1, 1));
    assert_eq!(harness.state.context(), Context::Reader);
}

#[test]
fn lists_drawn_over_the_panes_take_the_mouse() {
    let mut harness = Harness::demo(200, 60);
    harness.keys(":");
    harness.type_text("re");
    let offered: Vec<String> = harness
        .state
        .palette_items(&harness.env)
        .into_iter()
        .map(|item| item.title)
        .collect();
    // A click on another row chooses it; the wheel moves the choice.
    let third = find(&harness, &format!("  {}", offered[2]));
    click(&mut harness, (third.0 + 3, third.1));
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 2);
    mouse(&mut harness, MouseKind::WheelUp, third);
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 1);
    // The line being typed on is not a row of the list.
    let typed = find(&harness, ": re");
    click(&mut harness, typed);
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 1);
    // Messages behind the box are not reached through it, and a click
    // beside it closes it without running anything.
    click(&mut harness, (2, 30));
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert!(harness.state.compose.is_none());

    // Help, on a screen too short to show all of it: the wheel scrolls it
    // and a click beside it puts it away.
    let mut harness = Harness::demo(80, 24);
    harness.keys("?");
    mouse(&mut harness, MouseKind::WheelDown, (40, 12));
    assert!(harness.state.help.as_ref().unwrap().scroll > 0);
    click(&mut harness, (1, 1));
    assert!(harness.state.help.is_none());
}

#[test]
fn a_form_is_not_answered_or_dismissed_by_a_stray_click() {
    let mut harness = Harness::with_accounts(200, 60);
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::Setup);
    for at in [(1, 1), (100, 30), (199, 58)] {
        click(&mut harness, at);
        mouse(&mut harness, MouseKind::WheelDown, at);
    }
    assert_eq!(harness.state.context(), Context::Setup);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
}

#[test]
fn the_message_being_written_takes_clicks_drags_and_the_wheel() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("c tab tab tab");
    harness.type_text("Hello there, Priya.");
    harness.keys("enter");
    harness.type_text("Second line.");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Body);

    // A click on a header row puts the cursor in that field.
    click_on(&mut harness, "Subject:");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().field,
        Field::Subject
    );
    harness.type_text("Hi");
    click_on(&mut harness, "To:");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::To);

    // A click in the text puts the cursor there: typing lands at it.
    let there = find(&harness, "there, Priya.");
    click(&mut harness, there);
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Body);
    harness.type_text("out ");
    assert!(
        harness
            .state
            .compose
            .as_ref()
            .unwrap()
            .content()
            .body
            .starts_with("Hello out there, Priya.")
    );
    // A drag selects, and what is selected is copied.
    let from = find(&harness, "Second line.");
    click(&mut harness, from);
    mouse(&mut harness, MouseKind::Drag, (from.0 + 6, from.1));
    harness.keys("cmd-c");
    assert_eq!(
        harness.host_requests,
        [HostRequest::Copy("Second".to_owned())]
    );
    // The list beside it still takes the mouse; the message stays open.
    click_on(&mut harness, "Sunday dinner");
    assert!(harness.state.compose.is_some());
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    // The wheel over the message does not close or change it.
    mouse(&mut harness, MouseKind::WheelDown, from);
    assert_eq!(
        harness.state.compose.as_ref().unwrap().content().subject,
        "Hi"
    );
}

#[test]
fn under_60_columns_a_click_beside_the_sidebar_closes_it() {
    let mut harness = Harness::demo(50, 20);
    harness.keys("1");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    click(&mut harness, (45, 10));
    assert_eq!(harness.state.focus, Pane::List);
    // And a click in it chooses a mailbox.
    harness.keys("1");
    click_on(&mut harness, "    Sent");
    assert!(matches!(harness.state.mailbox, Mailbox::Folder(_)));
}

#[test]
fn clicks_outside_everything_do_nothing() {
    let mut harness = Harness::demo(200, 60);
    // The status bar, a border, and a cell past the edge of the screen.
    for at in [(10, 59), (0, 0), (199, 0), (500, 500), (u16::MAX, u16::MAX)] {
        click(&mut harness, at);
        mouse(&mut harness, MouseKind::Drag, at);
        mouse(&mut harness, MouseKind::WheelDown, at);
        mouse(&mut harness, MouseKind::WheelUp, at);
    }
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert!(harness.host_requests.is_empty());
}
