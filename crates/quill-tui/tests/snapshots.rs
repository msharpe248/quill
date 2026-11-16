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

#[test]
fn compose_new_message() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys("c");
        harness.type_text("pri");
        harness.keys("enter tab tab");
        harness.type_text("Lunch on Thursday");
        harness.keys("tab");
        harness.type_text("Hi Priya,");
        harness.keys("enter enter");
        harness.type_text("Are you *free* on Thursday? The usual place, `12:30`.");
        harness.keys("enter enter");
        harness.type_text("> Tuesday is out, I am afraid.");
        snapshot("compose_new_message", &harness);
    }
}

#[test]
fn compose_people_on_offer() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys("c");
        harness.type_text("r");
        harness.keys("down");
        snapshot("compose_people_on_offer", &harness);
    }
}

#[test]
fn compose_reply() {
    snapshot_after("compose_reply", "r");
}

#[test]
fn compose_forward() {
    snapshot_after("compose_forward", "f");
}

#[test]
fn compose_doubts_before_sending() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys("c");
        harness.type_text("pri");
        harness.keys("enter tab tab tab");
        harness.type_text("The agenda is attached.");
        harness.keys("cmd-enter");
        snapshot("compose_doubts_before_sending", &harness);
    }
}

#[test]
fn compose_zoomed() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys("r ctrl-z");
        snapshot("compose_zoomed", &harness);
    }
}

#[test]
fn compose_in_vim() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.keys("c");
        harness.type_text("ana.ruiz@example.com");
        harness.keys("ctrl-v");
        snapshot("compose_in_vim", &harness);
    }
}

#[test]
fn hook_picker() {
    for (width, height) in SIZES {
        let mut harness = Harness::demo(width, height);
        harness.env.hooks = quill_tui::hook::parse(
            "[[hook]]\nname = \"Save to notes\"\ncommand = [\"/usr/local/bin/save-note\", \"--from-mail\"]\n\
             [[hook]]\nname = \"Word count\"\ncommand = [\"/usr/bin/wc\", \"-w\"]\ninput = \"text\"\n",
        )
        .unwrap();
        harness.keys("| down");
        snapshot("hook_picker", &harness);
    }
}
