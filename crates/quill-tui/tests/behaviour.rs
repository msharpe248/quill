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
use quill_tui::state::{Action, Body, HostRequest, MainPane, Notice, Pane};
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
    harness.keys("g Q j");
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
fn the_sidebar_cursor_stops_on_accounts_and_folders_and_loads_each_mailbox() {
    let mut harness = Harness::demo(200, 60);
    // An account's own row is somewhere to be: its inbox, said to be so.
    harness.keys("1 j");
    assert_eq!(harness.state.mailbox_title(), "Inbox (Work)");
    assert_eq!(harness.state.list.rows.len(), 11);
    assert_eq!(harness.state.mailbox_unread(), 8);
    harness.keys("j");
    assert_eq!(harness.state.mailbox_title(), "Work / Inbox");
    assert_eq!(harness.state.list.rows.len(), 11);
    assert_eq!(harness.state.mailbox_unread(), 8);
    harness.keys("j j j j j");
    // Past Work's Trash to Personal's own row, and on to its inbox.
    assert_eq!(harness.state.mailbox_title(), "Inbox (Personal)");
    assert_eq!(harness.state.list.rows.len(), 6);
    harness.keys("j");
    assert_eq!(harness.state.mailbox_title(), "Personal / Inbox");
    assert_eq!(harness.state.list.rows.len(), 6);
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    assert!(harness.screen().contains("Your mother wants to know"));
    harness.keys("k k");
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
    harness.keys("1 j j enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.mailbox_title(), "Work / Inbox");
    // And from an account's own row, to its inbox.
    harness.keys("1 k enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.mailbox_title(), "Inbox (Work)");
}

#[test]
fn an_answer_for_a_mailbox_already_left_is_ignored() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 j j");
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
            thread: Vec::new(),
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
    harness.keys("1 j j j enter");
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
    // Five header rows (the message has a file) leave fifteen for the text.
    assert!(harness.row_with("of 25").contains("1-15 of 25"));
    harness.keys("j");
    assert_eq!(harness.state.reader.scroll, 1);
    harness.keys("G");
    assert_eq!(harness.state.reader.scroll, 10);
    assert!(harness.row_with("of 25").contains("11-25 of 25"));
    assert!(harness.screen().contains("Product, Example Corp"));
    harness.keys("j j space");
    assert_eq!(harness.state.reader.scroll, 10);
    harness.keys("g g");
    assert_eq!(harness.state.reader.scroll, 0);
    // A page is fourteen lines, but only ten are left below the first screen.
    harness.keys("space");
    assert_eq!(harness.state.reader.scroll, 10);
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
fn the_list_of_every_key_has_each_command_once_wherever_it_is_opened_from() {
    use quill_tui::view::help::{HelpLine, all_lines, all_text};

    let mut harness = Harness::demo(200, 60);
    harness.keys("K");
    assert_eq!(harness.state.context(), Context::Help);
    let screen = harness.screen();
    assert!(screen.contains(" Keyboard shortcuts "), "{screen}");
    assert!(!screen.contains("Help: Message list"));
    assert!(screen.contains("Star or unstar"));
    // The same key puts it away.
    harness.keys("K");
    assert!(harness.state.help.is_none());
    // From the accounts pane it is the same list: the message keys too.
    harness.keys("1 f1");
    assert!(harness.screen().contains("Star or unstar"));
    // The key for the help of this place swaps the two, and back.
    harness.keys("?");
    assert!(harness.screen().contains("Help: Accounts"));
    assert!(!harness.screen().contains("Star or unstar"));
    harness.keys("K");
    assert!(harness.screen().contains(" Keyboard shortcuts "));
    // It scrolls to its end, where writing a message and the list of
    // attachments have their own keys.
    harness.keys("G");
    let screen = harness.screen();
    assert!(screen.contains("Writing a message"), "{screen}");
    assert!(screen.contains("Save as a draft"));
    assert!(screen.contains("In the list of attachments"));
    harness.keys("esc");
    assert!(harness.state.help.is_none());

    // Every command is in it, once, with its keys; web reading only
    // where there is a web pane to read in.
    let env = &harness.env;
    let titles = |web_pane: bool| -> Vec<&'static str> {
        all_lines(&env.registry, &env.keymap, web_pane)
            .into_iter()
            .filter_map(|line| match line {
                HelpLine::Entry { title, .. } => Some(title),
                _ => None,
            })
            .collect()
    };
    let with = titles(true);
    assert_eq!(with.len(), env.registry.specs().len());
    for spec in env.registry.specs() {
        assert_eq!(
            with.iter().filter(|title| **title == spec.title).count(),
            1,
            "{}",
            spec.name
        );
    }
    assert_eq!(titles(false).len(), with.len() - 1);
    assert!(!titles(false).contains(&"Web reading pane"));

    // In print: headings at the margin, keys set in and lined up.
    let text = all_text(&env.registry, &env.keymap, false);
    assert!(text.starts_with("Navigation\n  j, down "), "{text}");
    assert!(text.contains("\n\nWriting a message\n"));
    assert!(text.contains("  K, f1 "));
    let columns: Vec<usize> = text
        .lines()
        .filter(|line| line.starts_with("  "))
        .filter_map(|line| {
            line.find("Star or unstar")
                .or_else(|| line.find("Reply to all"))
        })
        .collect();
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0], columns[1]);
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
fn flag_changes_are_queued_for_the_server() {
    use quill_core::types::Flags;
    use quill_sync::SyncRequest;
    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    harness.keys("s");
    // Written to the store's queue first, and only then is the engine told
    // to look.
    assert_eq!(harness.store.queued_operations().unwrap(), 1);
    assert_eq!(harness.sync_requests, [SyncRequest::Queue]);
    assert!(
        harness
            .store
            .message_location(id)
            .unwrap()
            .unwrap()
            .flags
            .flagged
    );

    // Opening marks it read: a second change to the same message, which
    // the server will hear as one.
    harness.sync_requests.clear();
    harness.keys("enter");
    assert_eq!(harness.store.queued_operations().unwrap(), 1);
    assert_eq!(harness.sync_requests, [SyncRequest::Queue]);
    assert_eq!(
        harness.store.message_location(id).unwrap().unwrap().flags,
        Flags {
            seen: true,
            flagged: true,
            ..Flags::default()
        }
    );

    // Opening it again changes nothing, so nothing is queued or said.
    harness.sync_requests.clear();
    harness.keys("esc enter");
    assert!(harness.sync_requests.is_empty());
    assert_eq!(harness.store.queued_operations().unwrap(), 1);
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
            .starts_with(" j/k move  enter open  / search  c write  r reply  e archive  d delete")
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
            .contains("/ search  m write  r reply  e archive  d delete")
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
        bar.starts_with(
            " n/p move  enter open  / search  c write  r reply  e archive  d delete  u unread  x select  tab next pane"
        ),
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
        checked,
        unread,
        title,
        background,
        foreground,
        appearance,
    } = harness.state.host_state(&harness.env);
    // As the system is, until the settings say otherwise.
    assert_eq!(appearance, quill_core::config::Appearance::System);
    assert_eq!(context, Context::List);
    // Nothing that is switched on and off is on.
    assert!(checked.is_empty());
    assert_eq!((unread, title.as_str()), (12, "Inbox (unified)"));
    // The dark theme's own background and text.
    assert_eq!(
        (background, foreground),
        (Some((0x1a, 0x1b, 0x26)), Some((0xc0, 0xca, 0xf5)))
    );
    assert!(enabled.contains(&CommandId::MessageToggleStar));
    assert!(enabled.contains(&CommandId::NavOpen));
    // Nothing to go back to from the list.
    assert!(!enabled.contains(&CommandId::NavBack));

    harness.keys("1 j j j j j j");
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
            html_first: true,
            has_plain: true,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    for line in &harness.state.reader.lines {
        assert!(
            !line.text().chars().any(char::is_control),
            "{:?}",
            line.text()
        );
    }
    // The escape sequence is gone whole, not just its first byte: what was
    // left of it would otherwise read as text.
    let screen = harness.screen();
    assert!(screen.contains("beforeafter go[1]"), "{screen}");
    assert!(!screen.contains("[2J"));
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
            html_first: true,
            has_plain: true,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    assert!(harness.screen().contains("The plain words."));
}

/// Opens the demo's newsletter, which comes as both HTML and plain text.
fn reading_the_newsletter(width: u16, height: u16) -> Harness {
    let mut harness = Harness::demo(width, height);
    harness.keys("G k k enter");
    assert_eq!(
        harness.selected_subject(),
        "Issue 88: Parsing email is still hard"
    );
    harness
}

#[test]
fn a_message_with_both_versions_switches_between_them() {
    use quill_tui::state::Showing;

    let mut harness = reading_the_newsletter(200, 60);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageToggleHtml)
    );
    assert_eq!(harness.state.reader.showing(), Some(Showing::Html));
    assert!(harness.screen().contains("# The Weekly Parser"));
    assert!(harness.screen().contains(" html "));
    let html_links = harness.state.reader.links.clone();

    harness.keys("j j t");
    assert_eq!(harness.state.reader.showing(), Some(Showing::Plain));
    // The other version starts at its top.
    assert_eq!(harness.state.reader.scroll, 0);
    let screen = harness.screen();
    assert!(screen.contains("The Weekly Parser, issue 88: parsing email is still hard."));
    assert!(!screen.contains("# The Weekly Parser"));
    assert!(screen.contains(" plain text "));
    // The plain part's own links are the ones on offer now.
    assert_eq!(
        harness.state.reader.links,
        [
            "https://example.com/issue-88",
            "https://example.com/unsubscribe"
        ]
    );
    assert_ne!(harness.state.reader.links, html_links);

    harness.keys("t");
    assert_eq!(harness.state.reader.showing(), Some(Showing::Html));
    assert!(harness.screen().contains("# The Weekly Parser"));

    // The choice belongs to the message: the next one starts afresh, and so
    // does this one when it is opened again.
    harness.keys("t esc k j");
    assert_eq!(harness.state.reader.showing(), Some(Showing::Html));
}

#[test]
fn a_message_with_one_version_has_nothing_to_switch_to() {
    use quill_tui::data::LoadedBody;
    use quill_tui::state::Showing;

    // Plain text only.
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageToggleHtml)
    );
    let before = harness.screen();
    harness.keys("t");
    assert_eq!(harness.screen(), before);
    assert!(!before.contains(" plain text ") && !before.contains(" html "));

    // HTML only: the text the store holds is a rendering of the same HTML.
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: "Hello.".to_owned(),
            html: Some("<p>Hello.</p>".to_owned()),
            html_first: true,
            has_plain: false,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageToggleHtml)
    );
    harness.keys("t");
    assert_eq!(harness.state.reader.showing(), Some(Showing::Html));
}

#[test]
fn a_fuller_plain_part_is_shown_first_with_the_html_one_key_away() {
    use quill_tui::data::LoadedBody;
    use quill_tui::state::Showing;

    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: "The whole message, in words.".to_owned(),
            html: Some("<p>scan</p>".to_owned()),
            html_first: false,
            has_plain: true,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    assert_eq!(harness.state.reader.showing(), Some(Showing::Plain));
    assert!(harness.screen().contains("The whole message, in words."));
    harness.keys("t");
    assert_eq!(harness.state.reader.showing(), Some(Showing::Html));
    assert!(harness.screen().contains("scan"));
}

#[test]
fn links_are_chosen_from_a_list_and_opened_by_the_host() {
    let mut harness = reading_the_newsletter(200, 60);
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageOpenLink)
    );

    harness.keys("o");
    assert_eq!(harness.state.context(), Context::Links);
    let screen = harness.screen();
    assert!(screen.contains(" Links "));
    assert!(screen.contains("> [1] https://example.com/issue-88/soft-breaks"));
    assert!(screen.contains("  [3] https://example.com/unsubscribe"));
    // The bar says what the keys do here.
    assert!(
        harness
            .screen()
            .lines()
            .last()
            .unwrap()
            .contains("open in browser")
    );

    // Moving does not scroll the reader underneath.
    let scroll = harness.state.reader.scroll;
    harness.keys("j");
    assert_eq!(harness.state.reader.scroll, scroll);
    assert!(
        harness
            .screen()
            .contains("> [2] https://example.com/issue-88")
    );
    assert!(harness.host_requests.is_empty());

    harness.keys("enter");
    assert_eq!(
        harness.host_requests,
        [HostRequest::OpenUrl(
            "https://example.com/issue-88".to_owned()
        )]
    );
    // Opening closes the list and leaves the reader where it was.
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(!harness.screen().contains(" Links "));
}

#[test]
fn a_link_is_reached_by_typing_its_number() {
    use quill_tui::data::LoadedBody;

    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    let id = harness.state.list.rows[0].id;
    let html: String = (1..=14)
        .map(|n| format!("<p><a href=\"https://example.com/{n}\">Link {n}</a></p>"))
        .collect();
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: String::new(),
            html: Some(html),
            html_first: true,
            has_plain: false,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    assert_eq!(harness.state.reader.links.len(), 14);

    harness.keys("o 3");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 2);
    // "1" then "2" is link 12.
    harness.keys("1");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 0);
    harness.keys("2");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 11);
    // There is no link 129, so "9" starts a new number.
    harness.keys("9");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 8);
    // Nor a link 0 or 90: the selection stays put.
    harness.keys("0");
    assert_eq!(harness.state.links.as_ref().unwrap().selected, 8);
    harness.keys("enter");
    assert_eq!(
        harness.host_requests,
        [HostRequest::OpenUrl("https://example.com/9".to_owned())]
    );

    // Digits mean panes again once the list is closed.
    harness.keys("1");
    assert_eq!(harness.state.context(), Context::Sidebar);
}

#[test]
fn the_link_list_closes_without_opening_anything() {
    let mut harness = reading_the_newsletter(200, 60);
    harness.keys("o esc");
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(harness.host_requests.is_empty());

    // It also goes when the message it listed is no longer the one open.
    harness.keys("o");
    assert!(harness.state.links.is_some());
    harness.act(Action::Command(CommandId::NavBack));
    harness.keys("esc k");
    assert!(harness.state.links.is_none());
}

#[test]
fn a_message_without_links_has_no_list_to_open() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert!(harness.state.reader.links.is_empty());
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageOpenLink)
    );
    harness.keys("o");
    assert_eq!(harness.state.context(), Context::Reader);
}

#[test]
fn only_web_and_mail_links_are_ever_opened() {
    use quill_tui::data::LoadedBody;

    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: String::new(),
            html: Some(
                "<p><a href=\"file:///Applications/Calculator.app\">Your invoice</a> \
                 <a href=\"x-evil://run\">Track parcel</a> \
                 <a href=\"mailto:ana.ruiz@example.com\">Write to Ana</a></p>"
                    .to_owned(),
            ),
            html_first: true,
            has_plain: false,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    // All three are listed, so where they lead can be seen.
    harness.keys("o");
    assert!(
        harness
            .screen()
            .contains("[1] file:///Applications/Calculator.app")
    );
    harness.keys("enter");
    assert!(harness.host_requests.is_empty());
    assert!(harness.screen().contains("Quill only opens web"));

    harness.keys("esc o 2 enter");
    assert!(harness.host_requests.is_empty());

    harness.keys("esc o 3 enter");
    assert_eq!(
        harness.host_requests,
        [HostRequest::OpenUrl(
            "mailto:ana.ruiz@example.com".to_owned()
        )]
    );
}

#[test]
fn link_text_is_placed_for_the_terminal_to_make_clickable() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    let overlays = |harness: &Harness| {
        let (width, height) = harness.state.size;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut overlays = quill_tui::view::Overlays::default();
        terminal
            .draw(|frame| {
                overlays = quill_tui::view::render(frame, &harness.state, &harness.env);
            })
            .unwrap();
        overlays
    };

    let mut harness = reading_the_newsletter(200, 60);
    let screen = harness.screen();
    let links = overlays(&harness).links;
    // The three links' text, each where the screen shows it, in the style
    // it was drawn in. Their numbers and the list of addresses are not
    // marked.
    let texts: Vec<&str> = links.iter().map(|link| link.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "quoted-printable soft line break",
            "Read the issue",
            "unsubscribe"
        ]
    );
    for link in &links {
        let row = screen.lines().nth(usize::from(link.y)).unwrap();
        let before: String = row.chars().take(usize::from(link.x)).collect();
        assert_eq!(quill_tui::text::width(&before), usize::from(link.x));
        assert!(row[before.len()..].starts_with(&link.text), "{row}");
        assert!(link.style.add_modifier.contains(Modifier::UNDERLINED));
    }
    assert_eq!(
        links[1].url.as_deref(),
        Some("https://example.com/issue-88")
    );

    // Under the link list or help, nothing is written over the overlay.
    harness.keys("o");
    let covered = harness.state.links.is_some();
    assert!(covered);
    let area = quill_tui::layout::compute(&harness.state, harness.state.size, 0.55)
        .links
        .unwrap();
    for link in overlays(&harness).links {
        assert!(
            !area.intersects(ratatui::layout::Rect::new(link.x, link.y, 1, 1)),
            "{link:?}"
        );
    }

    // Scrolled out of view, a link is not placed at all.
    harness.keys("esc");
    harness.resize(80, 12);
    harness.keys("G");
    let texts: Vec<String> = overlays(&harness)
        .links
        .into_iter()
        .map(|link| link.text)
        .collect();
    assert!(!texts.iter().any(|text| text.contains("quoted-printable")));
}

#[test]
fn link_text_cut_off_by_the_pane_is_placed_only_as_far_as_it_shows() {
    use quill_tui::data::LoadedBody;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    let id = harness.state.list.rows[0].id;
    // A table cell that cannot wrap is wider than the pane.
    let cell = "x".repeat(120);
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: String::new(),
            html: Some(format!(
                "<pre><a href=\"https://example.com/long\">{cell}</a></pre>"
            )),
            html_first: true,
            has_plain: false,
            files: Vec::new(),
            unsubscribe: None,
            ..LoadedBody::default()
        }),
    }));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut overlays = quill_tui::view::Overlays::default();
    terminal
        .draw(|frame| overlays = quill_tui::view::render(frame, &harness.state, &harness.env))
        .unwrap();
    let width = harness.state.reader.wrap_width.unwrap();
    assert!(!overlays.links.is_empty());
    for link in &overlays.links {
        assert!(quill_tui::text::width(&link.text) <= width, "{link:?}");
        assert!(usize::from(link.x) + quill_tui::text::width(&link.text) <= 80);
    }
}

#[test]
fn a_large_unread_count_is_shown_whole() {
    use quill_core::types::{FolderId, FolderRole, RemoteFolder};

    let mut harness = Harness::demo(80, 24);
    // 15,055 unread in a folder of the first account.
    let account = harness.store.accounts().unwrap()[0].id;
    let folder = harness
        .store
        .upsert_folder(
            account,
            &RemoteFolder {
                id: FolderId("bulk".to_owned()),
                name: "Mailing lists".to_owned(),
                role: FolderRole::Other,
            },
        )
        .unwrap();
    let added = (0..15_055)
        .map(|n| RemoteMessage {
            id: RemoteId(format!("bulk-{n}")),
            headers: Headers {
                message_id: None,
                subject: format!("Digest {n}"),
                from: vec![Address::new("List", "list@example.com")],
                to: Vec::new(),
                cc: Vec::new(),
                date: NOW - i64::from(n),
            },
            flags: Flags::default(),
            size: 1,
            has_attachments: false,
            thread: Vec::new(),
        })
        .collect();
    harness
        .store
        .apply_delta(
            folder,
            &SyncDelta {
                reset: false,
                added,
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));

    // The whole number, never "150…": the name gives way instead.
    let row = harness.row_with("15055");
    assert!(row.contains("Mail… 15055 │"), "{row}");
    // Small counts still end at the same column.
    assert!(harness.row_with("Unified").contains(" 12 │"));
}

#[test]
fn a_message_that_was_answered_says_so_once_read() {
    let mut harness = Harness::demo(200, 60);
    // The second message has been read; the first has not.
    for index in [0, 1] {
        let row = harness.state.list.rows[index].clone();
        let answered = quill_core::types::Flags {
            answered: true,
            ..row.flags
        };
        harness
            .store
            .queue_flags(row.id, answered, common::NOW)
            .unwrap();
    }
    harness.act(Action::Data(DataEvent::StoreChanged));
    assert!(
        harness.row_with("Build Bot").contains("  r   W Build Bot"),
        "{}",
        harness.row_with("Build Bot")
    );
    // Unread comes first: it is the more useful thing to know.
    assert!(
        harness.row_with("Q4 roadmap").contains("> * @ W Ana Ruiz"),
        "{}",
        harness.row_with("Q4 roadmap")
    );
}

/// A message in the Work inbox whose text is in the store and whose
/// downloaded copy is not, as one let go to keep within the disk budget
/// is. Returns its id, with the cursor on it.
fn let_go(harness: &mut Harness) -> quill_core::types::LocalMessageId {
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    add_message(harness, inbox, "Old news", "The text that was kept.");
    harness.keys("g g");
    assert_eq!(harness.selected_subject(), "Old news");
    let id = harness.state.list.rows[harness.state.list.selected].id;
    assert!(harness.store.raw(id).unwrap().is_none());
    id
}

/// The messages the engine has been asked to download, in order.
fn fetches(harness: &Harness) -> Vec<quill_core::types::LocalMessageId> {
    harness
        .sync_requests
        .iter()
        .filter_map(|request| match request {
            quill_sync::SyncRequest::FetchBody(id) => Some(*id),
            _ => None,
        })
        .collect()
}

const OLD_NEWS: &str = "From: Mallory Vane <mallory@example.com>\r\n\
    To: Robin Vale <robin@example.com>\r\n\
    Subject: Old news\r\n\
    MIME-Version: 1.0\r\n\
    Content-Type: multipart/mixed; boundary=\"m\"\r\n\
    \r\n\
    --m\r\n\
    Content-Type: text/html; charset=utf-8\r\n\
    \r\n\
    <p>The text that was kept, <b>and its shape</b>.</p>\r\n\
    --m\r\n\
    Content-Type: text/plain; name=\"notes.txt\"\r\n\
    Content-Disposition: attachment; filename=\"notes.txt\"\r\n\
    \r\n\
    Notes.\r\n\
    --m--\r\n";

#[test]
fn a_message_whose_copy_was_let_go_shows_its_text_and_is_fetched_again() {
    use quill_sync::{SyncEvent, SyncRequest};
    use quill_tui::state::Rest;
    let mut harness = Harness::demo(200, 60);
    let id = let_go(&mut harness);

    // The text is shown at once, with a line saying more is coming, and
    // the engine is asked for the rest: once.
    assert!(matches!(&harness.state.reader.body, Body::Loaded(body) if body.copy_missing));
    assert_eq!(harness.state.reader.rest, Rest::Fetching);
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    let screen = harness.screen();
    assert!(screen.contains("The text that was kept."), "{screen}");
    assert!(
        screen.contains("Fetching the rest of this message…"),
        "{screen}"
    );
    // The line is between the headers and the text, and the text is laid
    // out under it.
    let line_of = |needle: &str| {
        screen
            .lines()
            .position(|line| line.contains(needle))
            .unwrap()
    };
    assert_eq!(line_of("Fetching the rest"), line_of("Subject:") + 2);
    assert_eq!(
        line_of("The text that was kept."),
        line_of("Fetching the rest") + 2
    );
    assert!(harness.state.reader.files().is_empty());

    // Opening it, reading on and coming back to the list ask for nothing
    // more.
    harness.keys("enter j k esc");
    harness.act(Action::Data(DataEvent::StoreChanged));
    assert_eq!(fetches(&harness), [id]);

    // The engine downloads it and says so: the reader reads it again, and
    // now has everything.
    harness
        .store
        .put_message(
            id,
            "The text that was kept, and its shape.",
            OLD_NEWS.as_bytes(),
        )
        .unwrap();
    harness.act(Action::Sync(SyncEvent::BodyReady(id)));
    assert_eq!(harness.state.reader.rest, Rest::Here);
    assert!(matches!(
        &harness.state.reader.body,
        Body::Loaded(body) if !body.copy_missing && body.html.is_some()
    ));
    assert_eq!(harness.state.reader.files().len(), 1);
    let screen = harness.screen();
    assert!(!screen.contains("Fetching the rest"), "{screen}");
    assert!(screen.contains("and its shape"), "{screen}");
    assert!(screen.contains("notes.txt"), "{screen}");
    assert_eq!(fetches(&harness), [id]);
}

#[test]
fn a_copy_that_cannot_be_fetched_leaves_the_text_and_says_why() {
    use quill_sync::SyncEvent;
    use quill_tui::state::Rest;
    let mut harness = Harness::demo(200, 60);
    let id = let_go(&mut harness);
    harness.act(Action::Sync(SyncEvent::BodyFailed {
        id,
        reason: "cannot reach the server: no route to host\u{1b}[2J".to_owned(),
    }));
    // The text stays where it was, and the line says what happened.
    assert!(matches!(&harness.state.reader.body, Body::Loaded(_)));
    assert!(matches!(&harness.state.reader.rest, Rest::Failed(_)));
    let screen = harness.screen();
    assert!(screen.contains("The text that was kept."), "{screen}");
    // Too long for one row of this pane, it runs on to a second, and the
    // text starts under it.
    assert!(
        screen.contains(
            "The rest of this message could not be fetched: cannot reach the server: no route"
        ),
        "{screen}"
    );
    assert_eq!(harness.state.reader.note.len(), 2, "{screen}");
    assert!(
        harness.state.reader.note[1].starts_with("to host"),
        "{screen}"
    );
    assert!(!screen.contains("[2J"), "{screen}");
    let line_of = |needle: &str| {
        screen
            .lines()
            .position(|line| line.contains(needle))
            .unwrap()
    };
    assert_eq!(
        line_of("The text that was kept."),
        line_of("The rest of this message") + 3
    );
    // In a narrower pane it is laid out again, and never takes more than
    // three rows however long the reason.
    harness.resize(80, 24);
    harness.keys("enter");
    assert_eq!(harness.state.reader.note.len(), 2);
    assert!(
        harness.screen().contains("no route to host"),
        "{}",
        harness.screen()
    );
    harness.keys("esc");
    harness.resize(200, 60);
    // Nothing is asked again by itself, whatever else happens.
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.act(Action::Sync(SyncEvent::Changed));
    harness.keys("enter esc");
    assert_eq!(fetches(&harness), [id]);
    // A failure reported twice, or for another message, changes nothing.
    let other = harness.state.list.rows[1].id;
    for id in [id, other] {
        harness.act(Action::Sync(SyncEvent::BodyFailed {
            id,
            reason: "again".to_owned(),
        }));
    }
    assert!(
        harness
            .state
            .reader
            .note
            .join(" ")
            .ends_with("no route to host")
    );

    // Leaving the message and opening it again is asking again: once.
    harness.keys("j k");
    assert_eq!(fetches(&harness), [id, id]);
    assert_eq!(harness.state.reader.rest, Rest::Fetching);
    // A reason of any length takes three rows at most, and says it was
    // cut short.
    harness.act(Action::Sync(SyncEvent::BodyFailed {
        id,
        reason: "the server said no, and went on about it. ".repeat(40),
    }));
    assert_eq!(harness.state.reader.note.len(), 3);
    assert!(harness.state.reader.note[2].ends_with('…'));
    assert!(harness.screen().contains("The text that was kept."));
}

#[test]
fn a_copy_that_is_fetched_and_still_not_here_is_not_fetched_in_a_loop() {
    use quill_sync::{SyncEvent, SyncRequest};
    use quill_tui::data::LoadedBody;
    use quill_tui::state::Rest;
    let mut harness = Harness::demo(200, 60);
    let id = let_go(&mut harness);
    // The engine says it is done, and the store still has only the text.
    for _ in 0..3 {
        harness.act(Action::Sync(SyncEvent::BodyReady(id)));
    }
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    assert!(matches!(&harness.state.reader.rest, Rest::Failed(_)));
    let screen = harness.screen();
    assert!(screen.contains("The text that was kept."), "{screen}");
    assert!(
        screen.contains("The rest of this message could not be fetched: it was downloaded and"),
        "{screen}"
    );

    // The same for a message that had nothing here at all when opened:
    // one fetch, whatever comes of it.
    let mut harness = Harness::demo(200, 60);
    let id = harness.state.list.rows[0].id;
    harness.act(Action::Data(DataEvent::BodyLoaded { id, body: None }));
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    harness.act(Action::Data(DataEvent::BodyLoaded {
        id,
        body: Some(LoadedBody {
            text: "Only the text.".to_owned(),
            has_plain: true,
            copy_missing: true,
            ..LoadedBody::default()
        }),
    }));
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    assert!(harness.screen().contains("Only the text."));
}

#[test]
fn mail_that_is_all_here_and_drafts_are_never_fetched() {
    let mut harness = Harness::demo(200, 60);
    // Every message of the demo mailbox, opened in turn.
    for _ in 0..harness.state.list.rows.len() {
        harness.keys("enter esc j");
    }
    assert!(fetches(&harness).is_empty());
    assert!(!harness.screen().contains("Fetching the rest"));
    // A draft has no copy on any server to fetch.
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    harness.keys("esc esc");
    let drafts = harness
        .state
        .sidebar
        .entries
        .iter()
        .position(|entry| entry.mailbox() == Some(Mailbox::Drafts))
        .unwrap();
    harness.keys("1 g g");
    for _ in 0..drafts {
        harness.keys("j");
    }
    assert_eq!(harness.state.mailbox, Mailbox::Drafts);
    assert!(matches!(&harness.state.reader.body, Body::Loaded(body) if !body.copy_missing));
    assert!(fetches(&harness).is_empty());
}

#[test]
fn checking_for_new_mail_asks_every_account_now() {
    // Where there are servers to ask: a mailbox with accounts of its own.
    let mut harness = Harness::with_accounts(120, 30);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MailboxSync)
    );
    assert!(harness.sync_requests.is_empty());
    harness.keys("ctrl-r");
    assert_eq!(harness.sync_requests, [quill_sync::SyncRequest::SyncNow]);
    assert!(harness.screen().contains("Checking for new mail."));
    // The app's key for it, from any of the panes.
    harness.keys("1 cmd-r 3 cmd-r");
    assert_eq!(harness.sync_requests.len(), 3);

    // It is in the Mailbox menu and has a toolbar button, by its name.
    let registry = Registry::builtin();
    let spec = registry.get(CommandId::MailboxSync).unwrap();
    assert_eq!(spec.name, "mailbox.sync");
    assert_eq!(spec.title, "Check for New Mail");
    assert_eq!(spec.symbol, Some("arrow.clockwise"));
    assert!(spec.menu.is_some());

    // mutt's own key for fetching mail does it too.
    let mut mutt = Harness::with_accounts(120, 30);
    mutt.env.keymap = Keymap::preset(KeymapPreset::Mutt);
    mutt.keys("G");
    assert_eq!(mutt.sync_requests, [quill_sync::SyncRequest::SyncNow]);

    // The demo mailbox has no server behind it: nothing to ask.
    let mut demo = Harness::demo(120, 30);
    assert!(!demo.state.is_enabled(&demo.env, CommandId::MailboxSync));
    demo.keys("ctrl-r");
    assert!(demo.sync_requests.is_empty());
}
