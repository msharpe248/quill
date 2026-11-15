//! Searching: `/`, typing, and the list that shows what is found.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_store::Mailbox;
use quill_tui::command::Context;
use quill_tui::state::{Action, Pane};

fn title(harness: &Harness) -> String {
    harness.row_with("[2]")
}

fn subjects(harness: &Harness) -> Vec<&str> {
    harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.as_str())
        .collect()
}

#[test]
fn slash_opens_a_box_and_the_list_follows_what_is_typed() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    assert_eq!(harness.state.context(), Context::Search);
    assert!(
        title(&harness).contains("[2] Search: "),
        "{}",
        title(&harness)
    );
    // Nothing typed, nothing found, and a line saying what can be typed.
    assert!(harness.state.list.rows.is_empty());
    assert!(
        harness.screen().contains("from:ana"),
        "{}",
        harness.screen()
    );
    // The cursor waits in the title.
    let (x, y) = harness.cursor().unwrap();
    let row = harness
        .screen()
        .lines()
        .nth(usize::from(y))
        .unwrap()
        .to_owned();
    assert!(row.contains("Search:"), "{row}");
    let before: String = row.chars().take(usize::from(x)).collect();
    assert!(before.ends_with("Search: "), "{before:?}");

    // Letters are typed, not run: every one of these is a command in the
    // list. Each narrows what is found, across every account and folder.
    harness.type_text("road");
    assert_eq!(harness.state.context(), Context::Search);
    assert!(
        title(&harness).contains("Search: road - "),
        "{}",
        title(&harness)
    );
    assert!(subjects(&harness).contains(&"Q4 roadmap"));
    let wide = subjects(&harness).len();
    harness.type_text("map from:ana");
    assert_eq!(subjects(&harness), ["Q4 roadmap"]);
    assert!(wide >= 1);
    assert!(title(&harness).contains("- 1 found"), "{}", title(&harness));
    // The reader shows what is under the cursor, as in any list.
    assert!(harness.screen().contains("Hi Robin,"));
    // Typed back out, the list empties again.
    harness.keys("ctrl-u");
    assert!(harness.state.list.rows.is_empty());
    assert!(!title(&harness).contains("found"), "{}", title(&harness));
}

#[test]
fn enter_goes_to_the_results_and_escape_leaves_the_search() {
    let mut harness = Harness::demo(200, 60);
    let inbox = subjects(&harness).len();
    harness.keys("/");
    harness.type_text("is:unread from:sam");
    let found = subjects(&harness).len();
    assert!(found >= 1 && found < inbox);
    harness.keys("enter");
    // The results are a list like any other.
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.cursor().is_none());
    assert!(title(&harness).contains("Search: is:unread from:sam"));
    harness.keys("j k enter");
    assert_eq!(harness.state.focus, Pane::Reader);
    // Out of the reader, then out of the search: back where it began.
    harness.keys("esc");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(harness.state.search.is_some());
    harness.keys("esc");
    assert!(harness.state.search.is_none());
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert_eq!(subjects(&harness).len(), inbox);
    assert!(title(&harness).contains("Inbox (unified)"));
}

#[test]
fn slash_again_takes_up_what_was_typed() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    harness.type_text("roadmap");
    harness.keys("enter /");
    assert_eq!(harness.state.context(), Context::Search);
    harness.type_text(" is:unread");
    assert!(title(&harness).contains("Search: roadmap is:unread"));
    // Escape while typing leaves the search altogether.
    harness.keys("esc");
    assert!(harness.state.search.is_none());
    assert!(title(&harness).contains("Inbox (unified)"));
}

#[test]
fn enter_with_nothing_found_stays_in_the_box() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    harness.type_text("zebra");
    assert!(harness.screen().contains("Nothing found"));
    assert!(title(&harness).contains("- 0 found"), "{}", title(&harness));
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Search);
}

#[test]
fn choosing_a_mailbox_leaves_the_search() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    harness.type_text("roadmap");
    harness.keys("enter 1 j enter");
    assert!(harness.state.search.is_none());
    assert!(
        title(&harness).contains("Work / Inbox"),
        "{}",
        title(&harness)
    );
    // Even the mailbox the search was started from.
    harness.keys("/");
    harness.type_text("roadmap");
    harness.keys("enter 1 enter");
    assert!(harness.state.search.is_none());
    assert!(
        title(&harness).contains("Work / Inbox"),
        "{}",
        title(&harness)
    );
    assert!(!harness.state.list.rows.is_empty());
}

#[test]
fn what_is_found_follows_changes_to_the_mail() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    harness.type_text("is:unread");
    let unread = subjects(&harness).len();
    harness.keys("enter");
    // Marking one read is a change to what an unread search finds.
    harness.keys("u");
    assert_eq!(subjects(&harness).len(), unread - 1);
    assert!(
        title(&harness).contains(&format!("- {} found", unread - 1)),
        "{}",
        title(&harness)
    );
    // Archiving from the results works on the message. It is still unread
    // mail, so this search still finds it, in Archive now; a search of the
    // inbox no longer does.
    let first = subjects(&harness)[0].to_owned();
    harness.keys("e");
    assert!(subjects(&harness).contains(&first.as_str()));
    harness.keys("/");
    harness.type_text(" in:inbox");
    assert!(!subjects(&harness).contains(&first.as_str()));
    harness.keys("ctrl-u");
    harness.type_text("in:archive is:unread");
    assert!(subjects(&harness).contains(&first.as_str()));
}

#[test]
fn an_answer_for_what_was_typed_before_is_dropped() {
    use quill_tui::data::DataEvent;
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    harness.type_text("roadmap");
    let found = subjects(&harness).len();
    harness.act(Action::Data(DataEvent::SearchLoaded {
        text: "road".to_owned(),
        rows: Vec::new(),
        more: false,
    }));
    assert_eq!(subjects(&harness).len(), found);
    // And one that arrives after the search is over changes nothing.
    harness.keys("esc");
    let inbox = subjects(&harness).len();
    harness.act(Action::Data(DataEvent::SearchLoaded {
        text: "roadmap".to_owned(),
        rows: Vec::new(),
        more: false,
    }));
    assert_eq!(subjects(&harness).len(), inbox);
}

#[test]
fn pasting_searches_and_dates_are_days_where_the_user_is() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("/");
    // The demo's "now" is Saturday 14 November 2026.
    harness.act(Action::Paste("after:2026-11-14\n".to_owned()));
    let today = subjects(&harness).len();
    assert!(today >= 1);
    harness.keys("ctrl-u");
    harness.act(Action::Paste("before:2026-11-14".to_owned()));
    let earlier = subjects(&harness).len();
    harness.keys("ctrl-u");
    harness.type_text("after:2020");
    assert_eq!(subjects(&harness).len(), today + earlier);
}

#[test]
fn the_search_box_at_the_two_standard_sizes() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let mut harness = Harness::demo(width, height);
        harness.keys("/");
        harness.type_text("from:ana road");
        insta::assert_snapshot!(format!("search_typing_{width}x{height}"), harness.screen());
    }
}
