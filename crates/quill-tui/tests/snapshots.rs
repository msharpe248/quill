//! Snapshots of whole screens at the two sizes the spec names, 80x24 and
//! 200x60, plus a narrow one that exercises the sidebar overlay.
//!
//! Review a changed snapshot with `cargo insta review`, or by reading the
//! `.snap.new` file beside the old one.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_tui::state::{Action, Notice};

const SIZES: [(u16, u16); 2] = [(80, 24), (200, 60)];

fn snapshot(name: &str, harness: &Harness) {
    let (width, height) = harness.state.size;
    insta::assert_snapshot!(format!("{name}_{width}x{height}"), harness.screen());
}

/// Runs `keys` on the demo mailbox at each standard size and snapshots it.
fn snapshot_after(name: &str, keys: &str) {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys(keys);
        snapshot(name, &harness);
    }
}

#[test]
fn unified_inbox() {
    snapshot_after("unified_inbox", "");
}

#[test]
fn open_message() {
    snapshot_after("open_message", "enter");
}

#[test]
fn open_message_scrolled() {
    snapshot_after("open_message_scrolled", "enter j j j j");
}

#[test]
fn html_message() {
    // The newsletter, third from the end of the unified inbox, is HTML.
    snapshot_after("html_message", "G k k enter");
}

#[test]
fn html_message_as_plain_text() {
    snapshot_after("html_message_as_plain_text", "G k k enter t");
}

#[test]
fn link_picker() {
    snapshot_after("link_picker", "G k k enter o j");
}

#[test]
fn second_message_selected() {
    snapshot_after("second_message_selected", "j");
}

#[test]
fn help_for_the_list() {
    snapshot_after("help_list", "?");
}

#[test]
fn help_for_the_sidebar() {
    snapshot_after("help_sidebar", "1 ?");
}

#[test]
fn sent_folder_shows_recipients() {
    snapshot_after("work_sent", "1 j j j");
}

#[test]
fn empty_folder() {
    snapshot_after("work_trash", "1 j j j j j");
}

#[test]
fn personal_inbox() {
    snapshot_after("personal_inbox", "1 G g g j j j j j j");
}

#[test]
fn zoomed_list() {
    snapshot_after("zoomed_list", "z");
}

#[test]
fn zoomed_reader() {
    snapshot_after("zoomed_reader", "enter z");
}

#[test]
fn pending_key_sequence() {
    snapshot_after("pending_keys", "j j g");
}

#[test]
fn startup_notice() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.act(Action::Notify(Notice::error(
            "config line 3: invalid type: string \"wide\", expected f32",
        )));
        harness.act(Action::Notify(Notice::info("Using the default theme.")));
        snapshot("startup_notice", &harness);
    }
}

#[test]
fn narrow_layouts() {
    let mut harness = Harness::demo(50, 20);
    snapshot("narrow_list", &harness);
    harness.keys("1");
    snapshot("narrow_sidebar_overlay", &harness);
    harness.keys("j enter j enter");
    snapshot("narrow_reader", &harness);
    harness.keys("?");
    snapshot("narrow_help", &harness);
}
