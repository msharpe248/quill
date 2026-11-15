//! What the keys do, end to end: key -> `update` -> store -> `update` -> view.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::config::KeymapPreset;
use quill_core::types::{
    Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_store::Mailbox;
use quill_tui::command::{CommandId, Context, Registry};
use quill_tui::data::{DataEvent, DataRequest};
use quill_tui::keymap::Keymap;
use quill_tui::state::{Action, Body, MainPane, Notice, Pane};
use quill_tui::theme::ColorMode;
use ratatui::style::Modifier;

fn unread_in_store(harness: &Harness) -> usize {
    harness
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .iter()
        .filter(|row| !row.flags.seen)
        .count()
}

#[test]
fn the_demo_mailbox_starts_on_the_unified_inbox() {
    let harness = Harness::demo(200, 60);
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert_eq!(harness.state.list.rows.len(), 17);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert_eq!(harness.state.mailbox_unread(), 12);
    // The reader previews the selected message without marking it read.
    assert!(
        matches!(&harness.state.reader.body, Body::Loaded(body) if body.text.starts_with("Hi Robin,"))
    );
    assert_eq!(unread_in_store(&harness), 12);
    assert!(harness.row_with("Q4 roadmap").contains("> * @"));
}

#[test]
fn moving_in_the_list_moves_the_cursor_and_the_reader_follows() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("j j");
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    assert!(harness.row_with("Sunday dinner").contains("> *"));
    assert!(harness.screen().contains("Your mother wants to know"));
    harness.keys("k");
    assert_eq!(harness.selected_subject(), "[quill] PR #42 passed");
    harness.keys("k k k");
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert_eq!(unread_in_store(&harness), 12);
}

#[test]
fn g_g_and_capital_g_jump_to_the_ends() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("G");
    assert_eq!(harness.selected_subject(), "Welcome to the team");
    harness.keys("g");
    assert_eq!(harness.state.pending.len(), 1);
    assert_eq!(harness.selected_subject(), "Welcome to the team");
    harness.keys("g");
    assert!(harness.state.pending.is_empty());
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
}

#[test]
fn a_key_that_breaks_a_sequence_still_acts_by_itself() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("g j");
    assert!(harness.state.pending.is_empty());
    assert_eq!(harness.selected_subject(), "[quill] PR #42 passed");
    // An unbound key after a prefix is simply dropped.
    harness.keys("g x j");
    assert_eq!(harness.selected_subject(), "Sunday dinner");
}

#[test]
fn paging_moves_by_a_screenful_and_keeps_the_cursor_visible() {
    let mut harness = Harness::demo(80, 12);
    // Nine rows are visible; a page is one fewer so a row of context remains.
    harness.keys("space");
    assert_eq!(harness.state.list.selected, 8);
    assert_eq!(harness.state.list.offset, 0);
    harness.keys("space");
    assert_eq!(harness.state.list.selected, 16);
    assert_eq!(harness.state.list.offset, 8);
    assert!(harness.row_with("Welcome to the team").contains('>'));
    harness.keys("ctrl-u ctrl-u ctrl-u");
    assert_eq!(
        (harness.state.list.selected, harness.state.list.offset),
        (0, 0)
    );
}

#[test]
fn opening_a_message_marks_it_read_everywhere() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::Reader);
    assert!(harness.state.list.rows[0].flags.seen);
    assert_eq!(unread_in_store(&harness), 11);
    assert_eq!(harness.state.mailbox_unread(), 11);
    assert!(harness.screen().contains("Inbox (unified) - 11 unread"));
    assert!(harness.row_with("    Inbox").contains('7'));
    // Opening a message that is already read writes nothing.
    harness.keys("esc");
    let requests = harness
        .state
        .update(&harness.env, Action::Key("enter".parse().unwrap()));
    assert!(requests.is_empty(), "{requests:?}");
}

#[test]
fn unread_and_star_toggle_from_the_list_and_from_the_reader() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("u");
    assert!(harness.state.list.rows[0].flags.seen);
    assert_eq!(unread_in_store(&harness), 11);
    harness.keys("u");
    assert_eq!(unread_in_store(&harness), 12);

    harness.keys("s");
    assert!(harness.state.list.rows[0].flags.flagged);
    assert!(harness.row_with("Q4 roadmap").contains("> *!@"));
    assert!(
        harness.store.messages(Mailbox::Unified).unwrap()[0]
            .flags
            .flagged
    );

    harness.keys("enter s u");
    assert_eq!(harness.state.focus, Pane::Reader);
    let row = &harness.store.messages(Mailbox::Unified).unwrap()[0];
    assert!(!row.flags.flagged && !row.flags.seen);
    // The reader's border says so in words.
    assert!(
        harness
            .row_with("[3] Reader")
            .contains("unread, attachment")
    );
}

#[test]
fn message_keys_do_nothing_in_the_sidebar() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 u s");
    assert_eq!(unread_in_store(&harness), 12);
    assert!(!harness.state.list.rows[0].flags.flagged);
}

#[test]
fn the_sidebar_cursor_skips_account_headings_and_loads_each_mailbox() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 j");
    assert_eq!(harness.state.mailbox_title(), "Work / Inbox");
    assert_eq!(harness.state.list.rows.len(), 11);
    assert_eq!(harness.state.mailbox_unread(), 8);
    harness.keys("j j j j j");
    // Past Work's Trash and over the "Personal" heading.
    assert_eq!(harness.state.mailbox_title(), "Personal / Inbox");
    assert_eq!(harness.state.list.rows.len(), 6);
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    assert!(harness.screen().contains("Your mother wants to know"));
    harness.keys("k");
    assert_eq!(harness.state.mailbox_title(), "Work / Trash");
    assert!(harness.state.list.rows.is_empty());
    assert!(harness.state.reader.row.is_none());
    assert!(harness.screen().contains("No messages"));
    assert!(harness.screen().contains("No message selected"));
    harness.keys("g g");
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    harness.keys("G");
    assert_eq!(harness.state.mailbox_title(), "Personal / Trash");
}

#[test]
fn enter_in_the_sidebar_moves_to_the_list() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 j enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.mailbox_title(), "Work / Inbox");
}

#[test]
fn an_answer_for_a_mailbox_already_left_is_ignored() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 j");
    let work_inbox = harness.state.mailbox;
    let stale = harness.store.messages(Mailbox::Unified).unwrap();
    harness.act(Action::Data(DataEvent::MessagesLoaded {
        mailbox: Mailbox::Unified,
        rows: stale,
    }));
    assert_eq!(harness.state.mailbox, work_inbox);
    assert_eq!(harness.state.list.rows.len(), 11);
}

#[test]
fn the_selection_survives_a_reload_by_message_not_by_position() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("j j j");
    let selected = harness.selected_subject().to_owned();
    // A new message arrives at the top of the Work inbox.
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap();
    add_message(&mut harness, inbox.id, "Just arrived", "Hello.");
    assert_eq!(harness.state.list.rows[0].headers.subject, "Just arrived");
    assert_eq!(harness.selected_subject(), selected);
    assert_eq!(harness.state.list.selected, 4);
    assert_eq!(harness.state.mailbox_unread(), 13);
}

fn add_message(
    harness: &mut Harness,
    folder: quill_core::types::LocalFolderId,
    subject: &str,
    body: &str,
) {
    let id = RemoteId(format!("added-{subject}"));
    let delta = SyncDelta {
        added: vec![RemoteMessage {
            id: id.clone(),
            headers: Headers {
                message_id: None,
                subject: subject.to_owned(),
                from: vec![Address::new("Mallory \u{1b}[5mVane", "mallory@example.com")],
                to: vec![Address::new("Robin Vale", "robin@example.com")],
                cc: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                date: NOW - 30,
            },
            flags: Flags::default(),
            size: 0,
            has_attachments: false,
        }],
        changed: Vec::new(),
        removed: Vec::new(),
        reset: false,
        has_more: false,
        next_cursor: SyncCursor("after-add".to_owned()),
    };
    harness.store.apply_delta(folder, &delta).unwrap();
    let local = harness
        .store
        .local_message_id(folder, &id)
        .unwrap()
        .unwrap();
    harness.store.put_body(local, body).unwrap();
    // What the sync engine's change event will do from M2.
    harness.act(Action::Data(DataEvent::StoreChanged));
}

#[test]
fn hostile_escape_sequences_never_reach_the_screen() {
    let mut harness = Harness::demo(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap();
    add_message(
        &mut harness,
        inbox.id,
        "\u{1b}]0;pwned\u{07}Free \u{1b}[31mmoney\u{1b}[0m\r\nBcc: everyone",
        "Click \u{1b}[2J\u{1b}[Hhere\u{9b}31m now\u{1b}]52;c;ZXZpbA==\u{1b}\\.\u{07}\u{08}\n\tindented",
    );
    harness.keys("g g");
    assert_eq!(harness.selected_subject(), "Free money Bcc: everyone");
    let screen = harness.screen();
    assert!(screen.contains("Free money Bcc: everyone"));
    assert!(screen.contains("Mallory Vane <mallory@example.com>"));
    assert!(screen.contains("Click here now."));
    assert!(screen.contains("    indented"));
    assert!(!screen.contains("pwned") && !screen.contains("ZXZpbA"));
    for cell in harness.buffer().content {
        assert!(
            !cell.symbol().chars().any(char::is_control),
            "control character on screen: {:?}",
            cell.symbol()
        );
    }
    // A message with a Cc gets a Cc line, and the body still fits its pane.
    assert!(
        harness
            .row_with("Cc:")
            .contains("Ana Ruiz <ana.ruiz@example.com>")
    );
}

#[test]
fn tab_cycles_the_panes_both_ways_and_numbers_jump() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("tab");
    assert_eq!(harness.state.focus, Pane::Reader);
    harness.keys("tab");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    harness.keys("tab");
    assert_eq!(harness.state.focus, Pane::List);
    harness.keys("shift-tab");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    harness.keys("3");
    assert_eq!(harness.state.focus, Pane::Reader);
    harness.keys("2");
    assert_eq!(harness.state.focus, Pane::List);
    harness.keys("1");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    // The focused pane is the one with the thick border.
    assert!(harness.screen().starts_with("┏ [1] Accounts"));
}

#[test]
fn under_100_columns_the_reader_replaces_the_list_and_esc_brings_it_back() {
    let mut harness = Harness::demo(80, 24);
    assert!(harness.screen().contains("[2] Inbox (unified)"));
    assert!(!harness.screen().contains("[3] Reader"));
    harness.keys("enter");
    assert!(harness.screen().contains("[3] Reader"));
    assert!(!harness.screen().contains("[2] Inbox (unified)"));
    assert!(harness.screen().contains("[1] Accounts"));
    // Visiting the sidebar leaves the reader showing.
    harness.keys("1");
    assert_eq!(harness.state.main_pane, MainPane::Reader);
    assert!(harness.screen().contains("[3] Reader"));
    harness.keys("3 esc");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(harness.screen().contains("[2] Inbox (unified)"));
}

#[test]
fn under_60_columns_the_sidebar_is_an_overlay_that_esc_closes() {
    let mut harness = Harness::demo(50, 20);
    assert!(!harness.screen().contains("[1] Accounts"));
    harness.keys("enter");
    assert!(harness.screen().contains("[3] Reader"));
    harness.keys("1");
    let screen = harness.screen();
    assert!(screen.contains("[1] Accounts"));
    // The reader is still there to the right of the overlay.
    assert!(screen.lines().next().unwrap().contains("attachment"));
    harness.keys("esc");
    assert_eq!(harness.state.focus, Pane::Reader);
    assert!(!harness.screen().contains("[1] Accounts"));
    // Choosing a mailbox in the overlay and pressing enter lands on its list.
    harness.keys("1 j j enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.mailbox_title(), "Work / Drafts");
    assert!(harness.screen().contains("To: Ana Ruiz"));
}

#[test]
fn zoom_fills_the_screen_with_the_focused_pane_and_toggles_off() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("z");
    let screen = harness.screen();
    assert!(screen.contains("[2] Inbox (unified)"));
    assert!(!screen.contains("[1] Accounts") && !screen.contains("[3] Reader"));
    assert_eq!(screen.lines().next().unwrap().chars().count(), 200);
    // Zoom follows the focus.
    harness.keys("3");
    assert!(harness.screen().starts_with("┏ [3] Reader"));
    harness.keys("z");
    assert!(harness.screen().contains("[1] Accounts"));
}

#[test]
fn the_reader_scrolls_and_stops_at_the_end_of_the_body() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    assert!(harness.row_with("of 25").contains("1-16 of 25"));
    harness.keys("j");
    assert_eq!(harness.state.reader.scroll, 1);
    harness.keys("G");
    assert_eq!(harness.state.reader.scroll, 9);
    assert!(harness.row_with("of 25").contains("10-25 of 25"));
    assert!(harness.screen().contains("Product, Example Corp"));
    harness.keys("j j space");
    assert_eq!(harness.state.reader.scroll, 9);
    harness.keys("g g");
    assert_eq!(harness.state.reader.scroll, 0);
    // A page is fifteen lines, but only nine are left below the first screen.
    harness.keys("space");
    assert_eq!(harness.state.reader.scroll, 9);
    harness.keys("k k ctrl-u");
    assert_eq!(harness.state.reader.scroll, 0);
}

#[test]
fn resizing_rewraps_the_body_and_keeps_the_scroll_in_range() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter G");
    let narrow_lines = harness.state.reader.lines.len();
    assert!(harness.state.reader.scroll > 0);
    harness.resize(200, 60);
    // Wider lines, fewer of them, and the whole body now fits.
    assert!(harness.state.reader.lines.len() < narrow_lines);
    assert_eq!(harness.state.reader.scroll, 0);
    for line in &harness.state.reader.lines {
        let line = line.text();
        assert!(quill_tui::text::width(&line) <= 82, "{line}");
    }
    harness.resize(50, 12);
    assert!(harness.state.reader.lines.len() > narrow_lines);
    for line in &harness.state.reader.lines {
        let line = line.text();
        assert!(quill_tui::text::width(&line) <= 46, "{line}");
    }
}

#[test]
fn switching_message_resets_the_reader_scroll() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter G esc j");
    assert_eq!(harness.selected_subject(), "[quill] PR #42 passed");
    assert_eq!(harness.state.reader.scroll, 0);
    // Column-aligned text keeps its spacing.
    harness.keys("enter");
    assert!(harness.screen().contains("fmt      ok"));
}

#[test]
fn help_lists_the_commands_of_the_context_it_was_opened_from() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("?");
    assert_eq!(harness.state.context(), Context::Help);
    let screen = harness.screen();
    assert!(screen.contains("Help: Message list"));
    assert!(screen.contains("Star or unstar"));
    assert!(screen.contains("g g, home"));
    harness.keys("?");
    assert!(harness.state.help.is_none());

    harness.keys("1 ?");
    let screen = harness.screen();
    assert!(screen.contains("Help: Accounts"));
    assert!(!screen.contains("Star or unstar"));
}

#[test]
fn keys_scroll_the_help_overlay_not_the_pane_under_it() {
    let mut harness = Harness::demo(80, 14);
    harness.keys("?");
    let before = harness.screen();
    harness.keys("j j");
    assert_eq!(harness.state.help.unwrap().scroll, 2);
    assert_eq!(harness.state.list.selected, 0);
    assert_ne!(harness.screen(), before);
    harness.keys("G");
    let scroll = harness.state.help.unwrap().scroll;
    harness.keys("j");
    assert_eq!(harness.state.help.unwrap().scroll, scroll);
    assert!(harness.screen().contains("Help for this context"));
    // Message keys are inert while help is open.
    harness.keys("u s enter");
    assert_eq!(unread_in_store(&harness), 12);
    assert_eq!(harness.state.focus, Pane::List);
}

#[test]
fn q_closes_help_first_and_quits_only_from_a_pane() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("? q");
    assert!(harness.state.help.is_none());
    assert!(!harness.state.should_quit);
    harness.keys("? esc");
    assert!(harness.state.help.is_none());
    harness.keys("q");
    assert!(harness.state.should_quit);
}

#[test]
fn ctrl_c_quits() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("ctrl-c");
    assert!(harness.state.should_quit);
}

#[test]
fn notices_are_dismissed_one_at_a_time_before_esc_does_anything_else() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    harness.act(Action::Notify(Notice::error("first problem")));
    harness.act(Action::Notify(Notice::info("second thing")));
    assert!(
        harness
            .screen()
            .contains("first problem (+1 more)  [esc to dismiss]")
    );
    harness.keys("esc");
    assert!(harness.screen().contains("second thing  [esc to dismiss]"));
    assert_eq!(harness.state.focus, Pane::Reader);
    harness.keys("esc");
    assert!(harness.state.notices.is_empty());
    assert_eq!(harness.state.focus, Pane::Reader);
    harness.keys("esc");
    assert_eq!(harness.state.focus, Pane::List);
}

#[test]
fn a_store_error_becomes_a_notice() {
    let mut harness = Harness::demo(80, 24);
    harness.act(Action::Data(DataEvent::Error(
        "database error: disk full".to_owned(),
    )));
    assert!(harness.screen().contains("database error: disk full"));
}

#[test]
fn a_body_not_yet_downloaded_is_fetched_and_shown_when_it_lands() {
    use quill_sync::{SyncEvent, SyncRequest};
    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    let text = harness.store.body(id).unwrap().unwrap();

    // The store has no body for it: the reader asks the engine and waits.
    harness.act(Action::Data(DataEvent::BodyLoaded { id, body: None }));
    assert_eq!(harness.state.reader.body, Body::Downloading);
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    assert!(harness.screen().contains("Downloading…"));

    // News of some other message's body changes nothing here.
    let other = harness.state.list.rows[1].id;
    harness.act(Action::Sync(SyncEvent::BodyReady(other)));
    assert_eq!(harness.state.reader.body, Body::Downloading);

    // It lands: the reader reads it from the store.
    harness.act(Action::Sync(SyncEvent::BodyReady(id)));
    assert!(
        matches!(&harness.state.reader.body, Body::Loaded(body) if text.starts_with(body.text.as_str()))
    );
    assert!(harness.screen().contains("Hi Robin,"));
}

#[test]
fn a_body_that_cannot_be_downloaded_says_why() {
    use quill_sync::SyncEvent;
    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded { id, body: None }));
    harness.act(Action::Sync(SyncEvent::BodyFailed {
        id,
        reason: "cannot reach the server: no route to host\u{1b}[2J".to_owned(),
    }));
    assert!(matches!(harness.state.reader.body, Body::Unavailable(_)));
    assert!(
        harness
            .screen()
            .contains("cannot reach the server: no route to host")
    );
    // Moving away and back asks again.
    harness.keys("j k");
    assert!(matches!(&harness.state.reader.body, Body::Loaded(_)));
}

#[test]
fn flag_changes_are_sent_to_the_server_with_what_to_restore() {
    use quill_core::types::Flags;
    use quill_sync::SyncRequest;
    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    let unread = Flags::default();
    harness.keys("s");
    assert_eq!(
        harness.sync_requests,
        [SyncRequest::SetFlags {
            id,
            flags: Flags {
                flagged: true,
                ..unread
            },
            previous: unread,
        }]
    );
    harness.sync_requests.clear();
    // Opening marks it read: one more change, from the starred state.
    harness.keys("enter");
    assert_eq!(
        harness.sync_requests,
        [SyncRequest::SetFlags {
            id,
            flags: Flags {
                seen: true,
                flagged: true,
                ..unread
            },
            previous: Flags {
                flagged: true,
                ..unread
            },
        }]
    );
    // Opening it again changes nothing, so nothing is sent.
    harness.sync_requests.clear();
    harness.keys("esc enter");
    assert!(harness.sync_requests.is_empty());
}

#[test]
fn sync_events_reload_report_and_show_progress() {
    use quill_sync::SyncEvent;
    let mut harness = Harness::demo(200, 60);
    let account = harness.state.accounts[0].id;
    assert!(!harness.screen().contains("syncing"));
    harness.act(Action::Sync(SyncEvent::Syncing {
        account,
        active: true,
    }));
    assert!(
        harness
            .screen()
            .lines()
            .last()
            .unwrap()
            .ends_with("syncing…")
    );
    harness.act(Action::Sync(SyncEvent::Syncing {
        account,
        active: false,
    }));
    assert!(!harness.screen().contains("syncing"));

    // A change in the store reloads what is on screen.
    let id = harness.state.list.rows[0].id;
    harness.store.set_seen(id, true).unwrap();
    harness.act(Action::Sync(SyncEvent::Changed));
    assert!(harness.state.list.rows[0].flags.seen);
    assert_eq!(harness.state.mailbox_unread(), 11);

    harness.act(Action::Sync(SyncEvent::Problem {
        account: "Work".to_owned(),
        message: "sign-in failed: wrong password".to_owned(),
    }));
    assert!(
        harness
            .screen()
            .contains("Work: sign-in failed: wrong password")
    );
}

#[test]
fn the_status_bar_shows_the_keys_of_the_focused_context() {
    let mut harness = Harness::demo(200, 60);
    assert!(
        harness
            .screen()
            .lines()
            .last()
            .unwrap()
            .starts_with(" j/k move  enter open  u unread")
    );
    harness.keys("1");
    let bar = harness.screen().lines().last().unwrap().to_owned();
    assert!(bar.starts_with(" j/k move  enter open  tab next pane"));
    assert!(!bar.contains("star"));
    harness.keys("3");
    assert!(
        harness
            .screen()
            .lines()
            .last()
            .unwrap()
            .starts_with(" j/k scroll  space page  esc back")
    );
}

#[test]
fn the_mutt_preset_drives_the_same_commands_with_mutt_keys() {
    let mut harness = Harness::build(
        80,
        24,
        Keymap::preset(KeymapPreset::Mutt),
        ColorMode::TrueColor,
    );
    harness.keys("N");
    assert_eq!(unread_in_store(&harness), 11);
    harness.keys("u");
    assert_eq!(unread_in_store(&harness), 11);
    harness.keys("F * =");
    assert!(harness.state.list.rows[0].flags.flagged);
    assert_eq!(harness.state.list.selected, 0);
    harness.keys("enter i");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(
        harness
            .screen()
            .lines()
            .last()
            .unwrap()
            .contains("N unread  F star")
    );
}

#[test]
fn overridden_keys_show_up_in_the_hints_and_in_help() {
    let registry = Registry::builtin();
    let keymap = Keymap::preset(KeymapPreset::Vim)
        .with_overrides(
            "\"nav.next\" = \"n\"\n\"nav.prev\" = \"p\"\n\"message.toggle_star\" = []\n",
            &registry,
        )
        .unwrap();
    let mut harness = Harness::build(200, 60, keymap, ColorMode::TrueColor);
    let bar = harness.screen().lines().last().unwrap().to_owned();
    assert!(
        bar.starts_with(" n/p move  enter open  u unread  tab next pane"),
        "{bar}"
    );
    harness.keys("n n p");
    assert_eq!(harness.state.list.selected, 1);
    harness.keys("j s");
    assert_eq!(harness.state.list.selected, 1);
    assert!(!harness.state.list.rows[1].flags.flagged);
    harness.keys("?");
    assert!(harness.row_with("Next item or line").contains(" n "));
    assert!(harness.row_with("Star or unstar").contains("(unbound)"));
}

#[test]
fn commands_can_be_run_by_id_and_respect_their_contexts() {
    let mut harness = Harness::demo(200, 60);
    harness.act(Action::Command(CommandId::NavBottom));
    assert_eq!(harness.selected_subject(), "Welcome to the team");
    harness.act(Action::Command(CommandId::FocusSidebar));
    harness.act(Action::Command(CommandId::MessageToggleStar));
    assert!(
        harness
            .state
            .list
            .rows
            .iter()
            .all(|row| { row.headers.subject != "Welcome to the team" || !row.flags.flagged })
    );
}

#[test]
fn only_a_new_day_makes_the_clock_redraw() {
    let mut harness = Harness::demo(80, 24);
    harness.state.dirty = false;
    harness.act(Action::ClockTick(NOW + 30));
    assert!(!harness.state.dirty);
    assert_eq!(harness.state.now, NOW + 30);
    assert!(harness.row_with("Q4 roadmap").contains("09:14"));
    // Past midnight UTC, this morning's mail was yesterday's.
    harness.act(Action::ClockTick(NOW + 15 * 3600));
    assert!(harness.state.dirty);
    assert!(harness.row_with("Q4 roadmap").contains("Sat"));
}

#[test]
fn every_change_asks_for_a_redraw_and_requests_only_what_it_needs() {
    let mut harness = Harness::demo(80, 24);
    harness.state.dirty = false;
    let requests = harness
        .state
        .update(&harness.env, Action::Key("j".parse().unwrap()));
    assert!(harness.state.dirty);
    let id = harness.state.list.rows[1].id;
    assert_eq!(requests.data, [DataRequest::LoadBody(id)]);
    assert!(requests.host.is_empty());
    // Moving within the reader touches no data at all.
    harness.pump(requests.data);
    harness.keys("enter");
    let requests = harness
        .state
        .update(&harness.env, Action::Key("j".parse().unwrap()));
    assert!(requests.is_empty());
}

#[test]
fn without_colour_the_selection_is_reversed_and_nothing_is_coloured() {
    let harness = Harness::build(80, 24, Keymap::preset(KeymapPreset::Vim), ColorMode::None);
    let buffer = harness.buffer();
    for cell in &buffer.content {
        assert_eq!(
            (cell.fg, cell.bg),
            (ratatui::style::Color::Reset, ratatui::style::Color::Reset)
        );
    }
    // Row 1 is the selected message in the focused list; row 2 is not selected.
    let cell = |x: u16, y: u16| buffer.cell((x, y)).unwrap();
    assert!(cell(30, 1).modifier.contains(Modifier::REVERSED));
    assert!(!cell(30, 2).modifier.contains(Modifier::REVERSED));
    // The sidebar's cursor, in an unfocused pane, is underlined instead.
    assert!(cell(5, 1).modifier.contains(Modifier::UNDERLINED));
    // Unread rows are bold; the read one on row 2 is not.
    assert!(cell(30, 3).modifier.contains(Modifier::BOLD));
    assert!(!cell(30, 2).modifier.contains(Modifier::BOLD));
}

#[test]
fn with_colour_the_selection_has_a_background_bar() {
    let harness = Harness::demo(80, 24);
    let buffer = harness.buffer();
    let selected = buffer.cell((30, 1)).unwrap().bg;
    let other = buffer.cell((30, 2)).unwrap().bg;
    assert_ne!(selected, other);
}

#[test]
fn no_size_or_key_makes_the_views_panic() {
    let keys = "j G 1 j j enter 3 ? j esc z tab enter s u 2 g g space z";
    for (width, height) in [
        (0, 0),
        (1, 1),
        (10, 3),
        (23, 6),
        (24, 5),
        (24, 6),
        (30, 8),
        (59, 7),
        (60, 9),
        (99, 10),
        (100, 6),
        (140, 6),
        (300, 100),
    ] {
        let mut harness = Harness::demo(width, height);
        harness.screen();
        for key in keys.split_whitespace() {
            harness.keys(key);
            harness.screen();
        }
    }
    let harness = Harness::demo(20, 5);
    assert!(harness.screen().contains("Quill needs a larger"));
}

#[test]
fn a_frame_larger_or_smaller_than_the_state_expects_still_draws() {
    // For one draw after a resize, the terminal is ahead of `state.size`.
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut harness = Harness::demo(200, 60);
    harness.keys("G enter");
    for (width, height) in [(40, 10), (80, 24), (250, 70), (24, 6)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                quill_tui::view::render(frame, &harness.state, &harness.env);
            })
            .unwrap();
    }
}

#[test]
fn the_host_is_told_what_applies_where_the_focus_is() {
    use quill_tui::state::HostState;
    let mut harness = Harness::demo(200, 60);
    let HostState {
        context,
        enabled,
        unread,
        title,
    } = harness.state.host_state(&harness.env);
    assert_eq!(context, Context::List);
    assert_eq!((unread, title.as_str()), (12, "Inbox (unified)"));
    assert!(enabled.contains(&CommandId::MessageToggleStar));
    assert!(enabled.contains(&CommandId::NavOpen));
    // Nothing to go back to from the list.
    assert!(!enabled.contains(&CommandId::NavBack));

    harness.keys("1 j j j j j");
    let state = harness.state.host_state(&harness.env);
    assert_eq!(state.context, Context::Sidebar);
    assert_eq!(state.title, "Work / Trash");
    // The unread count is the whole mailbox's, whatever folder is showing.
    assert_eq!(state.unread, 12);
    assert!(!state.enabled.contains(&CommandId::MessageToggleStar));

    // An empty folder has nothing to open or flag.
    harness.keys("2");
    let state = harness.state.host_state(&harness.env);
    assert!(!state.enabled.contains(&CommandId::NavOpen));
    assert!(!state.enabled.contains(&CommandId::MessageToggleUnread));

    harness.keys("?");
    let state = harness.state.host_state(&harness.env);
    assert_eq!(state.context, Context::Help);
    assert!(state.enabled.contains(&CommandId::NavBack));
    assert!(!state.enabled.contains(&CommandId::AppQuit));
}

#[test]
fn opening_a_message_changes_what_the_host_mirrors() {
    let mut harness = Harness::demo(200, 60);
    let before = harness.state.host_state(&harness.env);
    harness.keys("enter");
    let after = harness.state.host_state(&harness.env);
    assert_ne!(before, after);
    assert_eq!(after.unread, 11);
    assert_eq!(after.context, Context::Reader);
    assert!(after.enabled.contains(&CommandId::NavBack));
    // Moving within the reader changes nothing the host cares about.
    harness.keys("j");
    assert_eq!(harness.state.host_state(&harness.env), after);
}

#[test]
fn the_preview_command_asks_the_host_and_touches_no_data() {
    use quill_tui::state::HostRequest;
    let mut harness = Harness::demo(200, 60);
    let effects = harness
        .state
        .update(&harness.env, Action::Key("P".parse().unwrap()));
    assert!(effects.data.is_empty());
    assert_eq!(effects.host, [HostRequest::PreviewSample]);
    // Not while help is open.
    harness.keys("? P");
    assert!(harness.host_requests.is_empty());
    harness.act(Action::Command(CommandId::DebugPreviewSample));
    assert!(harness.host_requests.is_empty());
    harness.keys("esc");
    harness.act(Action::Command(CommandId::DebugPreviewSample));
    assert_eq!(harness.host_requests, [HostRequest::PreviewSample]);
}

/// Renders the harness and returns what the view asks the terminal to draw.
fn placements(harness: &Harness) -> Vec<quill_tui::view::ImagePlacement> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let (width, height) = harness.state.size;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut images = Vec::new();
    terminal
        .draw(|frame| images = quill_tui::view::render(frame, &harness.state, &harness.env))
        .unwrap();
    images
}

#[test]
fn the_sample_image_is_placed_in_the_reader_above_the_body() {
    let mut harness = Harness::demo(200, 60);
    harness.env.images = true;
    assert!(placements(&harness).is_empty());

    harness.keys("I");
    let images = placements(&harness);
    assert_eq!(images.len(), 1);
    let image = images[0];
    assert_eq!(image.id, quill_tui::view::SAMPLE_IMAGE_ID);
    // Ten rows, forty columns: the 2:1 picture keeps its shape in 1:2 cells.
    assert_eq!((image.area.width, image.area.height), (40, 10));

    // The cells under it are left empty, and the body starts below it.
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    let top = usize::from(image.area.y);
    assert!(rows[top - 2].contains("Subject: Q4 roadmap"));
    for row in &rows[top..top + 10] {
        let inside: String = row
            .chars()
            .skip(usize::from(image.area.x))
            .take(40)
            .collect();
        assert!(inside.trim().is_empty(), "{row}");
    }
    assert!(rows[top + 11].contains("Hi Robin,"));

    harness.keys("I");
    assert!(placements(&harness).is_empty());
    assert!(
        harness
            .screen()
            .lines()
            .nth(top)
            .unwrap()
            .contains("Hi Robin,")
    );
}

#[test]
fn an_image_is_not_placed_under_an_overlay() {
    let mut harness = Harness::demo(200, 60);
    harness.env.images = true;
    harness.keys("I");
    assert_eq!(placements(&harness).len(), 1);
    // Help sits in the middle of the screen, clear of the reader's image,
    // at this size; at 100 columns it covers it.
    harness.resize(100, 30);
    assert_eq!(placements(&harness).len(), 1);
    harness.keys("?");
    assert!(placements(&harness).is_empty());
    harness.keys("?");
    assert_eq!(placements(&harness).len(), 1);

    // The narrow layout's sidebar overlay covers the reader's left edge.
    harness.resize(50, 24);
    harness.keys("3");
    assert_eq!(placements(&harness).len(), 1);
    harness.keys("1");
    assert!(placements(&harness).is_empty());
}

#[test]
fn a_terminal_without_images_gets_a_row_naming_the_image() {
    let mut harness = Harness::demo(200, 60);
    assert!(!harness.env.images);
    harness.keys("I");
    assert!(placements(&harness).is_empty());
    let screen = harness.screen();
    assert!(screen.contains("[image: sample, 320x160]"));
    assert!(screen.contains("Hi Robin,"));
}

#[test]
fn the_image_takes_room_from_the_body_and_the_scroll_knows() {
    let mut harness = Harness::demo(80, 24);
    harness.env.images = true;
    harness.keys("enter");
    assert!(harness.row_with("of 25").contains("1-16 of 25"));
    harness.keys("I");
    // Eight rows of image and a blank line leave seven for the text.
    assert!(harness.row_with("of 25").contains("1-7 of 25"));
    harness.keys("G");
    assert_eq!(harness.state.reader.scroll, 18);
    assert!(harness.screen().contains("Product, Example Corp"));
    // Hiding the image gives the rows back and the scroll stays in range.
    harness.keys("I");
    assert_eq!(harness.state.reader.scroll, 9);
}

#[test]
fn no_image_without_a_message_or_room() {
    let mut harness = Harness::demo(200, 60);
    harness.env.images = true;
    harness.keys("I 1 j j j j j");
    assert!(harness.state.reader.row.is_none());
    assert!(placements(&harness).is_empty());
    // A reader too short for a picture shows only text.
    let mut harness = Harness::demo(120, 10);
    harness.env.images = true;
    harness.keys("I");
    assert!(placements(&harness).is_empty());
}

#[test]
fn an_html_message_is_drawn_as_styled_text() {
    use ratatui::style::Modifier;

    let mut harness = Harness::demo(200, 60);
    harness.keys("G k k enter");
    assert_eq!(
        harness.selected_subject(),
        "Issue 88: Parsing email is still hard"
    );
    assert!(
        matches!(&harness.state.reader.body, Body::Loaded(body) if body.html.is_some()),
        "the reader was given the message's HTML"
    );
    let screen = harness.screen();
    // Structure survives; markup, hidden preview text and the tracking
    // image do not.
    assert!(screen.contains("# The Weekly Parser"), "{screen}");
    assert!(screen.contains("* A tour of the worst Content-Type headers"));
    assert!(screen.contains("Read the issue[2] or unsubscribe[3]."));
    assert!(screen.contains("[2] https://example.com/issue-88"));
    assert!(!screen.contains("<h1>") && !screen.contains("display:none"));
    assert!(!screen.contains("Soft line breaks, bad headers"));
    assert!(!screen.contains("open.gif"));

    // Emphasis is carried by text attributes, not by colour alone.
    let buffer = harness.buffer();
    let modifier_at = |needle: &str| {
        let row = screen
            .lines()
            .position(|line| line.contains(needle))
            .unwrap();
        let line = screen.lines().nth(row).unwrap();
        let column = quill_tui::text::width(&line[..line.find(needle).unwrap()]);
        buffer[(column as u16, row as u16)].modifier
    };
    assert!(modifier_at("# The Weekly Parser").contains(Modifier::BOLD));
    assert!(modifier_at("worst Content-Type").contains(Modifier::BOLD));
    assert!(modifier_at("Issue 88: parsing").contains(Modifier::ITALIC));
    assert!(modifier_at("Read the issue").contains(Modifier::UNDERLINED));
    assert!(!modifier_at("A tour of").contains(Modifier::BOLD));
}

#[test]
fn an_html_message_is_laid_out_again_for_a_narrower_pane() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("G k k enter");
    let wide = harness.state.reader.lines.len();
    harness.resize(80, 24);
    assert!(harness.state.reader.lines.len() > wide);
    let width = harness.state.reader.wrap_width.unwrap();
    for line in &harness.state.reader.lines {
        let line = line.text();
        // The list of links is clipped by the pane, not wrapped.
        assert!(
            quill_tui::text::width(&line) <= width || line.starts_with('['),
            "{line}"
        );
    }
}

#[test]
fn html_cannot_smuggle_escape_sequences_onto_the_screen() {
    use quill_tui::data::LoadedBody;

    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: "plain".to_owned(),
            html: Some("<p>before&#27;[2J&#7;after <a href=\"https://example.com/\u{1b}]0;x\u{7}\">go</a></p>".to_owned()),
        }),
    }));
    for line in &harness.state.reader.lines {
        assert!(
            !line.text().chars().any(char::is_control),
            "{:?}",
            line.text()
        );
    }
    assert!(harness.screen().contains("before[2Jafter go[1]"));
}

#[test]
fn html_that_renders_to_nothing_falls_back_to_the_plain_text() {
    use quill_tui::data::LoadedBody;

    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: "The plain words.".to_owned(),
            html: Some("<script>nothing()</script>".to_owned()),
        }),
    }));
    assert!(harness.screen().contains("The plain words."));
}
