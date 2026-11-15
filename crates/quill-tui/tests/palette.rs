//! The command palette: `:` or `ctrl-k`, a few letters, enter.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_store::Mailbox;
use quill_tui::command::Context;
use quill_tui::state::{Action, Pane};

fn offered(harness: &Harness) -> Vec<String> {
    harness
        .state
        .palette_items(&harness.env)
        .into_iter()
        .map(|item| item.title)
        .collect()
}

#[test]
fn the_palette_lists_what_applies_here_with_its_keys() {
    let mut harness = Harness::demo(200, 60);
    harness.keys(":");
    assert_eq!(harness.state.context(), Context::Palette);
    let screen = harness.screen();
    assert!(screen.contains(" Commands "), "{screen}");
    // A command of the list, with its key at the right.
    let items = harness.state.palette_items(&harness.env);
    let archive = items.iter().find(|item| item.title == "Archive").unwrap();
    assert_eq!(archive.keys, "e");
    assert!(
        screen.lines().any(|line| line.contains("  Archive ")
            && line.trim_end_matches(['┃', ' ', '│']).ends_with(" e")),
        "{screen}"
    );
    let offered = offered(&harness);
    assert!(offered.contains(&"Reply".to_owned()));
    assert!(offered.contains(&"Search".to_owned()));
    // Mailboxes to go to, after the commands.
    assert!(offered.contains(&"Go to Work / Sent".to_owned()));
    assert!(offered.contains(&"Go to Inbox (unified)".to_owned()));
    // Not what does not apply here, not plain cursor movement, not itself.
    assert!(!offered.contains(&"Send".to_owned()));
    assert!(!offered.contains(&"Next item or line".to_owned()));
    assert!(!offered.contains(&"Command palette".to_owned()));
    // The cursor is on the line being typed.
    assert!(harness.cursor().is_some());
}

#[test]
fn a_few_letters_find_a_command_and_enter_runs_it() {
    let mut harness = Harness::demo(200, 60);
    let first = harness.selected_subject().to_owned();
    assert!(!harness.state.list.rows[0].flags.flagged);
    harness.keys("ctrl-k");
    // Letters are typed, not run: `s` alone would star, `d` delete.
    harness.type_text("sd");
    assert_eq!(harness.state.context(), Context::Palette);
    assert_eq!(harness.selected_subject(), first);
    harness.keys("ctrl-u");
    harness.type_text("star");
    assert_eq!(offered(&harness)[0], "Star or unstar");
    harness.keys("enter");
    // It ran where the palette was opened, and the palette is gone.
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.list.rows[0].flags.flagged);
}

#[test]
fn a_mailbox_can_be_gone_to_by_name() {
    let mut harness = Harness::demo(200, 60);
    harness.keys(":");
    harness.type_text("persent");
    assert_eq!(offered(&harness)[0], "Go to Personal / Sent");
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(matches!(harness.state.mailbox, Mailbox::Folder(_)));
    assert!(
        harness.row_with("[2]").contains("Personal / Sent"),
        "{}",
        harness.row_with("[2]")
    );
    // The sidebar's cursor went with it.
    assert!(harness.row_with(">   Sent").contains("Sent"));
}

#[test]
fn the_cursor_moves_through_what_matches_and_escape_closes() {
    let mut harness = Harness::demo(200, 60);
    harness.keys(":");
    harness.type_text("re");
    let offered = offered(&harness);
    assert!(offered.len() >= 3, "{offered:?}");
    assert_eq!(offered[0], "Reply");
    harness.keys("down");
    assert!(
        harness
            .row_with(&offered[1])
            .contains(&format!("> {}", offered[1]))
    );
    harness.keys("up up");
    assert!(harness.row_with("> Reply").contains("> Reply"));
    // Typing more starts again from the best match.
    harness.keys("down");
    harness.type_text("p");
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 0);
    // Nothing matches: enter does nothing but close.
    harness.type_text("zzz");
    assert!(harness.screen().contains("Nothing matches."));
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.compose.is_none());
}

#[test]
fn what_is_offered_is_what_applied_where_it_was_opened() {
    let mut harness = Harness::demo(200, 60);
    // In the reader of a message with HTML and links.
    harness.keys("G k k enter :");
    let offered = offered(&harness);
    assert!(offered.contains(&"Show as HTML or as plain text".to_owned()));
    assert!(offered.contains(&"Open a link".to_owned()));
    harness.type_text("links");
    harness.keys("ctrl-u");
    harness.type_text("open a link");
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Links);
    harness.keys("esc");

    // In the sidebar there is no message to reply to.
    harness.keys("esc 1 :");
    assert!(!self::offered(&harness).contains(&"Open a link".to_owned()));
    harness.act(Action::Paste("zoom\n".to_owned()));
    assert_eq!(self::offered(&harness)[0], "Zoom the focused pane");
}

#[test]
fn the_palette_at_the_two_standard_sizes() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let mut harness = Harness::demo(width, height);
        harness.keys(":");
        harness.type_text("re");
        insta::assert_snapshot!(format!("palette_{width}x{height}"), harness.screen());
    }
}
