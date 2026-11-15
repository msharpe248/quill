//! A question on screen is all there is until it is answered: no key and
//! no turn of the wheel moves what lies behind it. The lists drawn over
//! the panes keep their own moving about.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Flags, FolderRole, Headers, LocalMessageId, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{CertificateDetails, CertificateProblem};
use quill_store::Mailbox;
use quill_sync::SyncEvent;
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataEvent;
use quill_tui::state::{Action, Mouse, MouseKind};
use quill_tui::template::Template;

/// Every key that moves without typing a letter, and then some of them
/// again in case the first only reached an end.
const MOVES: &str = "down up pagedown pageup end home down down pagedown end up";

/// The six commands that move.
const MOVING: [CommandId; 6] = [
    CommandId::NavNext,
    CommandId::NavPrev,
    CommandId::NavTop,
    CommandId::NavBottom,
    CommandId::NavPageDown,
    CommandId::NavPageUp,
];

/// Where everything behind a dialog is: the cursors, how far the list
/// and the reader are scrolled, and which mailbox and message are shown.
#[derive(Debug, PartialEq, Eq)]
struct Places {
    sidebar: (usize, usize),
    list: (usize, usize),
    reader: usize,
    mailbox: Mailbox,
    reading: Option<LocalMessageId>,
}

fn places(harness: &Harness) -> Places {
    let state = &harness.state;
    Places {
        sidebar: (state.sidebar.selected, state.sidebar.offset),
        list: (state.list.selected, state.list.offset),
        reader: state.reader.scroll,
        mailbox: state.mailbox,
        reading: state.reader.row.as_ref().map(|row| row.id),
    }
}

/// The demo mailbox in a short, wide window: the fourth message open and
/// scrolled, so that there is room to move every way, in the list and in
/// the reader.
fn reading() -> Harness {
    let mut harness = Harness::with_accounts(120, 14);
    // The first message after the top two that is long enough to scroll.
    harness.keys("j j");
    while harness.state.reader.lines.len() < 14 {
        harness.keys("j");
    }
    harness.keys("enter j j");
    assert_eq!(harness.state.context(), Context::Reader);
    assert_eq!(harness.state.list.selected, SELECTED);
    assert_eq!(harness.state.reader.scroll, 2);
    harness
}

/// The row of the list [`reading`] leaves the cursor on.
const SELECTED: usize = 14;

/// Turns the wheel both ways over each pane, at its top edge, which no
/// dialog covers.
fn wheel_everywhere(harness: &mut Harness) {
    let areas = quill_tui::layout::compute(&harness.state, harness.state.size, 0.5);
    for rect in [areas.sidebar, areas.list, areas.reader]
        .into_iter()
        .flatten()
    {
        for kind in [
            MouseKind::WheelDown,
            MouseKind::WheelDown,
            MouseKind::WheelUp,
            MouseKind::WheelDown,
        ] {
            harness.act(Action::Mouse(Mouse {
                kind,
                column: rect.x + 1,
                row: rect.y,
            }));
        }
    }
}

/// Presses every moving key and turns the wheel, and checks that nothing
/// behind the dialog in `context` moved and the dialog is still there.
fn nothing_moves_behind(harness: &mut Harness, context: Context) {
    assert_eq!(harness.state.context(), context);
    let before = places(harness);
    let asked = (
        harness.host_requests.len(),
        harness.sync_requests.len(),
        harness.account_requests.len(),
    );
    harness.keys(MOVES);
    assert_eq!(places(harness), before, "a key moved something");
    wheel_everywhere(harness);
    assert_eq!(places(harness), before, "the wheel moved something");
    // By name too, as a menu or the palette would run them.
    for id in MOVING {
        harness.act(Action::Command(id));
    }
    assert_eq!(places(harness), before, "a command moved something");
    assert_eq!(harness.state.context(), context);
    assert_eq!(
        asked,
        (
            harness.host_requests.len(),
            harness.sync_requests.len(),
            harness.account_requests.len(),
        ),
        "something was asked for"
    );
}

/// Without the dialog, the same keys and the same wheel do move things:
/// the checks above are not passing because nothing ever moves.
#[test]
fn the_same_keys_and_wheel_move_things_when_nothing_is_in_the_way() {
    let mut harness = reading();
    let before = places(&harness);
    harness.keys("pagedown");
    assert_ne!(places(&harness), before);
    let mut harness = reading();
    wheel_everywhere(&mut harness);
    assert_ne!(places(&harness), before);
    let mut harness = reading();
    harness.keys("2 down");
    assert_eq!(harness.state.list.selected, SELECTED + 1);
}

#[test]
fn nothing_moves_behind_the_sign_in_dialog() {
    for focus in ["", "2", "1"] {
        let mut harness = reading();
        harness.keys(focus);
        harness.keys("S");
        nothing_moves_behind(&mut harness, Context::SignIn);
        for id in MOVING {
            assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
        }
        // Giving up still works, and then the keys move again.
        harness.keys("esc");
        assert_ne!(harness.state.context(), Context::SignIn);
        let before = places(&harness);
        harness.keys("down");
        assert_ne!(places(&harness), before);
    }
}

fn certificate(harness: &mut Harness) {
    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: Box::new(CertificateProblem {
            host: "imap.example.com".to_owned(),
            reason: "UnknownIssuer".to_owned(),
            fingerprint: Some("0123456789abcdef".repeat(4)),
            certificate: None,
            details: Some(CertificateDetails {
                subject: "CN=imap.example.com".to_owned(),
                issuer: "CN=Example Home Server".to_owned(),
                not_before: NOW - 86_400,
                not_after: NOW + 30 * 86_400,
            }),
        }),
        sending: false,
    }));
}

#[test]
fn nothing_moves_behind_the_certificate_question() {
    for focus in ["", "2", "1"] {
        let mut harness = reading();
        harness.keys(focus);
        certificate(&mut harness);
        nothing_moves_behind(&mut harness, Context::Certificate);
        for id in MOVING {
            assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
        }
        // The question is still there to answer, and was not answered by
        // any of it.
        assert_eq!(harness.state.certificates.len(), 1);
        assert!(!harness.state.certificates[0].busy);
        harness.keys("r");
        assert_ne!(harness.state.context(), Context::Certificate);
    }
}

#[test]
fn nothing_moves_behind_the_question_of_when() {
    for focus in ["", "2"] {
        let mut harness = reading();
        harness.keys(focus);
        harness.keys("b");
        nothing_moves_behind(&mut harness, Context::When);
        for id in MOVING {
            assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
        }
        // What is typed still goes in the box, where home and end move
        // the cursor.
        harness.type_text("2h");
        harness.keys("home");
        harness.type_text("1");
        assert_eq!(harness.state.when.as_ref().unwrap().input.text(), "12h");
        harness.keys("esc");
        assert_ne!(harness.state.context(), Context::When);
        // Nothing was snoozed.
        assert!(harness.store.messages(Mailbox::Snoozed).unwrap().is_empty());
    }
    // And when it is a message being written whose time is asked for.
    let mut harness = reading();
    harness.keys("c ctrl-l");
    nothing_moves_behind(&mut harness, Context::When);
}

/// Puts a message from a mailing list at the top of the Work inbox.
fn add_list_mail(harness: &mut Harness) {
    let raw = "From: Harbour Books <news@harbourbooks.example.com>\r\n\
               To: Robin Vale <robin@example.com>\r\n\
               Subject: New this November\r\n\
               List-Unsubscribe: <https://lists.harbourbooks.example.com/u/r0b1n>\r\n\
               \r\n\
               Three new arrivals this month.\r\n";
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("list-1".to_owned());
    let parsed = quill_mime::parse(raw.as_bytes(), NOW).unwrap();
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![RemoteMessage {
                    id: remote.clone(),
                    headers: Headers {
                        date: NOW + 60,
                        ..parsed.headers.clone()
                    },
                    flags: Flags::default(),
                    size: raw.len() as u64,
                    has_attachments: false,
                    thread: Vec::new(),
                }],
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
    let id = harness
        .store
        .local_message_id(inbox, &remote)
        .unwrap()
        .unwrap();
    harness
        .store
        .put_message(id, &parsed.body, raw.as_bytes())
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
}

#[test]
fn nothing_moves_behind_the_unsubscribe_question() {
    for focus in ["3", "2"] {
        let mut harness = Harness::with_accounts(120, 14);
        add_list_mail(&mut harness);
        // The list's mail is second from the top once the cursor's own
        // message has kept its place; go to it by name.
        harness.keys("g g");
        while harness.selected_subject() != "New this November" {
            harness.keys("j");
        }
        harness.keys("enter");
        harness.keys(focus);
        harness.keys("X");
        nothing_moves_behind(&mut harness, Context::Unsubscribe);
        for id in MOVING {
            assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
        }
        // Still unanswered: nothing was opened or sent.
        assert!(harness.state.unsubscribe.is_some());
        harness.keys("esc");
        assert_ne!(harness.state.context(), Context::Unsubscribe);
    }
}

#[test]
fn nothing_moves_behind_the_account_form_but_its_own_fields() {
    let mut harness = reading();
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::Setup);
    let before = places(&harness);
    harness.keys(MOVES);
    wheel_everywhere(&mut harness);
    for id in MOVING {
        harness.act(Action::Command(id));
    }
    assert_eq!(places(&harness), before);
    assert_eq!(harness.state.context(), Context::Setup);
    // The arrows are the form's, to step between its fields; a jump or a
    // page is nothing here.
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavNext));
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavPrev));
    for id in &MOVING[2..] {
        assert!(!harness.state.is_enabled(&harness.env, *id), "{id:?}");
    }
}

#[test]
fn page_keys_in_a_message_being_written_do_not_scroll_the_reader_under_it() {
    let mut harness = reading();
    let before = places(&harness);
    harness.keys("c");
    assert_eq!(harness.state.context(), Context::Compose);
    // In a header field, which has no use for them.
    harness.keys("pagedown pagedown pageup pagedown");
    for id in MOVING {
        harness.act(Action::Command(id));
        assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
    }
    assert_eq!(places(&harness), before);
    // In the body they are the editor's, and still move its cursor.
    harness.keys("tab tab tab");
    harness.act(Action::Paste("one\ntwo\nthree".to_owned()));
    harness.keys("pageup");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().body.cursor_offset(),
        0
    );
    assert_eq!(places(&harness), before);
}

#[test]
fn the_lists_drawn_over_the_panes_keep_their_own_moving_about() {
    // Help scrolls, and the list under it stays.
    let mut harness = reading();
    harness.keys("2");
    let before = places(&harness);
    harness.keys("? down down pagedown");
    assert!(harness.state.help.unwrap().scroll > 2);
    harness.keys("home");
    assert_eq!(harness.state.help.unwrap().scroll, 0);
    assert_eq!(places(&harness), before);
    harness.keys("esc");

    // The palette.
    // The palette. Home and end are for the line being typed on.
    harness.keys(":");
    harness.keys("down down up");
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 1);
    harness.keys("pagedown");
    assert!(harness.state.palette.as_ref().unwrap().selected > 2);
    harness.keys("pageup up up");
    assert_eq!(harness.state.palette.as_ref().unwrap().selected, 0);
    assert_eq!(places(&harness), before);
    harness.keys("esc");

    // The folders a message can be moved to.
    harness.keys("m down down up");
    assert_eq!(harness.state.folders.as_ref().unwrap().selected, 1);
    harness.keys("pagedown");
    assert!(harness.state.folders.as_ref().unwrap().selected > 1);
    assert_eq!(places(&harness), before);
    harness.keys("esc");

    // A message's links: the newsletter, third from the end.
    let mut harness = Harness::demo(120, 14);
    harness.keys("G k k enter");
    let before = places(&harness);
    harness.keys("o down down up");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 1);
    harness.keys("end");
    assert_eq!(
        harness.state.links.as_ref().unwrap().selected,
        harness.state.reader.links.len() - 1
    );
    assert_eq!(places(&harness), before);
    harness.keys("esc");

    // A message's files.
    let mut harness = Harness::demo(120, 14);
    while harness.selected_subject() != "Photos from Sat" {
        harness.keys("j");
    }
    harness.keys("enter");
    let files = harness.state.reader.files().len();
    assert!(files > 1, "{files}");
    let before = places(&harness);
    harness.keys("a down");
    assert_eq!(harness.state.attachments.unwrap().selected, 1);
    harness.keys("end");
    assert_eq!(harness.state.attachments.unwrap().selected, files - 1);
    harness.keys("home");
    assert_eq!(harness.state.attachments.unwrap().selected, 0);
    assert_eq!(places(&harness), before);
    harness.keys("esc");

    // Templates, over a message being written.
    let mut harness = reading();
    harness.env.templates = ["One", "Two", "Three"]
        .map(|name| Template::parse(name, "Some text.\n"))
        .to_vec();
    let before = places(&harness);
    harness.keys("c ctrl-r down down up");
    assert_eq!(harness.state.context(), Context::Templates);
    assert_eq!(harness.state.templates.as_ref().unwrap().selected, 1);
    harness.keys("pagedown");
    assert_eq!(harness.state.templates.as_ref().unwrap().selected, 2);
    assert_eq!(places(&harness), before);

    // A search being typed: the arrows walk what it has found so far.
    let mut harness = Harness::demo(120, 14);
    harness.keys("/");
    harness.type_text("a");
    assert_eq!(harness.state.context(), Context::Search);
    assert!(harness.state.list.rows.len() > 2);
    harness.keys("down down");
    assert_eq!(harness.state.list.selected, 2);
    harness.keys("up");
    assert_eq!(harness.state.list.selected, 1);
}
