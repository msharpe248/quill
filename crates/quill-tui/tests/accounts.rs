//! The accounts pane: accounts that open and close, that can be chosen, and
//! that "Check for New Mail" goes by.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::config::KeymapPreset;
use quill_core::types::{FolderId, FolderRole, LocalAccountId, RemoteFolder};
use quill_store::{Mailbox, Store};
use quill_sync::SyncRequest;
use quill_tui::command::{CommandId, Context, Registry};
use quill_tui::data::DataEvent;
use quill_tui::demo;
use quill_tui::keymap::Keymap;
use quill_tui::state::{Action, Mouse, MouseKind, Pane};
use quill_tui::theme::ColorMode;

/// The demo's accounts, in the order the pane lists them.
fn accounts(harness: &Harness) -> (LocalAccountId, LocalAccountId) {
    let accounts = harness.store.accounts().unwrap();
    assert_eq!(
        (accounts[0].name.as_str(), accounts[1].name.as_str()),
        ("Work", "Personal")
    );
    (accounts[0].id, accounts[1].id)
}

/// The rows of the accounts pane as drawn, without its border.
fn pane(harness: &Harness) -> Vec<String> {
    let width = harness
        .screen()
        .lines()
        .next()
        .unwrap()
        .chars()
        .skip(1)
        .position(|c| c == '┐' || c == '┓')
        .unwrap();
    harness
        .screen()
        .lines()
        .skip(1)
        .map(|line| {
            line.chars()
                .skip(1)
                .take(width)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .take_while(|line| !line.starts_with(['─', '━']))
        .filter(|line| !line.is_empty())
        .collect()
}

fn enabled(harness: &Harness, id: CommandId) -> bool {
    harness.state.is_enabled(&harness.env, id)
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// The cell where `needle` first appears on screen.
fn find(harness: &Harness, needle: &str) -> (u16, u16) {
    for (row, line) in harness.screen().lines().enumerate() {
        if let Some(at) = line.find(needle) {
            return (quill_tui::text::width(&line[..at]) as u16, row as u16);
        }
    }
    panic!("{needle:?} is not on screen:\n{}", harness.screen());
}

fn click(harness: &mut Harness, (column, row): (u16, u16)) {
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::Press,
        column,
        row,
    }));
}

fn changed(harness: &mut Harness) {
    harness.act(Action::Data(DataEvent::StoreChanged));
}

#[test]
fn every_account_starts_open_with_a_marker_that_says_so() {
    let harness = Harness::demo(80, 24);
    assert_eq!(
        pane(&harness),
        [
            "> Unified    12",
            "  ▾ Work",
            "    Inbox     8",
            "    Drafts",
            "    Sent",
            "    Archive",
            "    Trash",
            "  ▾ Personal",
            "    Inbox     4",
            "    Drafts",
            "    Sent",
            "    Archive",
            "    Trash",
        ]
    );
    assert!(harness.state.sidebar.closed.is_empty());
    assert!(harness.store.closed_accounts().unwrap().is_empty());
}

#[test]
fn an_account_is_closed_and_opened_with_the_keys() {
    let mut harness = Harness::demo(80, 24);
    let (work, _) = accounts(&harness);
    // On the unified inbox there is nothing to open or close.
    harness.keys("1");
    assert!(!enabled(&harness, CommandId::SidebarOpen));
    assert!(!enabled(&harness, CommandId::SidebarClose));
    harness.keys("h l left right");
    assert!(harness.state.sidebar.closed.is_empty());

    // On an open account, only closing.
    harness.keys("j");
    assert!(enabled(&harness, CommandId::SidebarClose));
    assert!(!enabled(&harness, CommandId::SidebarOpen));
    assert!(
        status(&harness).contains("h hide folders"),
        "{}",
        status(&harness)
    );
    assert!(!status(&harness).contains("show folders"));
    harness.keys("l");
    assert!(harness.state.sidebar.closed.is_empty());

    harness.keys("h");
    assert_eq!(
        pane(&harness),
        [
            "  Unified    12",
            "> ▸ Work      8",
            "  ▾ Personal",
            "    Inbox     4",
            "    Drafts",
            "    Sent",
            "    Archive",
            "    Trash",
        ]
    );
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    // Closed, only opening; closing again does nothing.
    assert!(enabled(&harness, CommandId::SidebarOpen));
    assert!(!enabled(&harness, CommandId::SidebarClose));
    assert!(
        status(&harness).contains("l show folders"),
        "{}",
        status(&harness)
    );
    harness.keys("h left");
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);

    // The cursor steps over what is folded away.
    harness.keys("j");
    assert_eq!(harness.state.mailbox_title(), "Inbox (Personal)");
    harness.keys("k");
    assert_eq!(harness.state.mailbox_title(), "Inbox (Work)");
    harness.keys("G");
    assert_eq!(harness.state.mailbox_title(), "Personal / Trash");
    harness.keys("g g j");

    harness.keys("l");
    assert_eq!(
        pane(&harness)[1..4],
        ["> ▾ Work", "    Inbox     8", "    Drafts"]
    );
    assert!(harness.store.closed_accounts().unwrap().is_empty());
    // And by the arrows.
    harness.keys("left");
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    harness.keys("right");
    assert!(harness.store.closed_accounts().unwrap().is_empty());

    // The keys are the accounts pane's: in the list they do nothing.
    harness.keys("2 h left");
    assert!(harness.state.sidebar.closed.is_empty());
    assert!(!enabled(&harness, CommandId::SidebarClose));
}

#[test]
fn the_arrows_open_and_close_in_the_mutt_keys_too() {
    let mut harness = Harness::build(
        80,
        24,
        Keymap::preset(KeymapPreset::Mutt),
        ColorMode::TrueColor,
    );
    let (work, _) = accounts(&harness);
    harness.keys("1 j left");
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(
        status(&harness).contains("right show folders"),
        "{}",
        status(&harness)
    );
    harness.keys("right");
    assert!(harness.store.closed_accounts().unwrap().is_empty());
    // `h` and `l` are left free there.
    harness.keys("h");
    assert!(harness.store.closed_accounts().unwrap().is_empty());
}

#[test]
fn both_commands_are_in_the_registry_for_the_menus_and_the_palette() {
    let registry = Registry::builtin();
    for (id, name) in [
        (CommandId::SidebarOpen, "sidebar.open"),
        (CommandId::SidebarClose, "sidebar.close"),
    ] {
        let spec = registry.by_name(name).unwrap();
        assert_eq!(spec.id, id);
        assert_eq!(spec.contexts, [Context::Sidebar]);
        assert!(spec.menu.is_some());
        for preset in [KeymapPreset::Vim, KeymapPreset::Mutt] {
            assert!(!Keymap::preset(preset).keys_for(id).is_empty());
        }
    }
}

#[test]
fn closing_from_a_folder_puts_the_cursor_on_the_account() {
    let mut harness = Harness::demo(200, 60);
    let (work, _) = accounts(&harness);
    harness.keys("1 j j j j");
    assert_eq!(harness.state.mailbox_title(), "Work / Sent");
    assert!(enabled(&harness, CommandId::SidebarClose));
    assert!(!enabled(&harness, CommandId::SidebarOpen));
    harness.keys("h");
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(
        harness.screen().contains("> ▸ Work"),
        "{}",
        harness.screen()
    );
    assert_eq!(harness.state.focus, Pane::Sidebar);
    // The list shows what the row stands for: the account's inbox.
    assert_eq!(harness.state.mailbox, Mailbox::Account(work));
    assert_eq!(harness.state.mailbox_title(), "Inbox (Work)");
    assert_eq!(harness.state.list.rows.len(), 11);
    assert!(harness.state.list.loaded);
    // A reload leaves the cursor there, and the account closed.
    changed(&mut harness);
    assert!(harness.screen().contains("> ▸ Work"));
    assert_eq!(harness.state.mailbox, Mailbox::Account(work));
}

#[test]
fn a_closed_account_shows_what_is_unread_in_its_inbox() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("1 j");
    // Open, the count is on its inbox, the row below.
    assert_eq!(
        pane(&harness)[1..3],
        ["> ▾ Work", "    Inbox               8"]
    );
    harness.keys("h");
    assert_eq!(pane(&harness)[1], "> ▸ Work                8");
    // Together the accounts come to what Unified counts.
    harness.keys("j h");
    assert_eq!(
        pane(&harness),
        [
            "  Unified              12",
            "  ▸ Work                8",
            "> ▸ Personal            4",
        ]
    );

    // Reading one of Work's takes it off the count, as it does off
    // Unified's.
    harness.keys("g g 2");
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    harness.keys("enter esc");
    assert_eq!(
        pane(&harness),
        [
            "> Unified              11",
            "  ▸ Work                7",
            "  ▸ Personal            4",
        ]
    );
    // With nothing unread there is no count, as on a folder.
    let ids: Vec<_> = harness
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .filter(|row| !row.flags.seen)
        .map(|row| row.id)
        .collect();
    for id in ids {
        harness.store.set_seen(id, true).unwrap();
    }
    changed(&mut harness);
    assert_eq!(pane(&harness), ["> Unified", "  ▸ Work", "  ▸ Personal"]);
}

#[test]
fn choosing_an_account_shows_its_inbox() {
    let mut harness = Harness::demo(200, 60);
    let (work, personal) = accounts(&harness);
    harness.keys("1 j");
    assert_eq!(harness.state.mailbox, Mailbox::Account(work));
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.account == work && folder.role == FolderRole::Inbox)
        .unwrap();
    assert_eq!(
        harness.state.list.rows,
        harness.store.messages(Mailbox::Folder(inbox.id)).unwrap()
    );
    assert!(
        harness
            .state
            .list
            .rows
            .iter()
            .all(|row| row.account == work)
    );
    // The list says whose inbox it is, and so does the window.
    let title = harness.row_with("[2]");
    assert!(title.contains("[2] Inbox (Work) - 8 unread"), "{title}");
    assert_eq!(harness.state.host_state(&harness.env).title, "Inbox (Work)");
    // It is a mailbox like any other: its mail is opened and read from it.
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::List);
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(harness.row_with("[2]").contains("Inbox (Work) - 7 unread"));
    // And a message started from it is from that account.
    harness.keys("esc 1 G g g j j j j j j j");
    assert_eq!(harness.state.mailbox, Mailbox::Account(personal));
    assert_eq!(harness.state.list.rows.len(), 6);
    harness.keys("c");
    assert!(harness.row_with("From:").contains("(Personal)"));

    // Even from an inbox with nothing in it to go by.
    let mut harness = Harness::demo(200, 60);
    let ids: Vec<_> = harness
        .store
        .messages(Mailbox::Account(personal))
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect();
    harness.keys("1 G g g j j j j j j j");
    for id in ids {
        harness.act(Action::Data(DataEvent::StoreChanged));
        harness.pump(vec![quill_tui::data::DataRequest::Archive(id)]);
    }
    assert_eq!(harness.state.mailbox, Mailbox::Account(personal));
    assert!(harness.state.list.rows.is_empty(), "{}", harness.screen());
    harness.keys("c");
    assert!(harness.row_with("From:").contains("(Personal)"));
}

#[test]
fn how_the_accounts_were_left_is_kept_and_a_new_account_starts_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quill.db");
    let seeded = || {
        let mut store = Store::open(&path).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(demo::seed(&mut store, NOW))
            .unwrap();
        store
    };

    let mut harness = Harness::over_store(seeded(), 80, 24, false);
    let (_, personal) = accounts(&harness);
    harness.keys("1 G h");
    assert_eq!(pane(&harness).last().unwrap(), "> ▸ Personal  4");
    drop(harness);

    // Quill started again, on the same store.
    let mut harness = Harness::over_store(Store::open(&path).unwrap(), 80, 24, false);
    assert_eq!(
        pane(&harness),
        [
            "> Unified    12",
            "  ▾ Work",
            "    Inbox     8",
            "    Drafts",
            "    Sent",
            "    Archive",
            "    Trash",
            "  ▸ Personal  4",
        ]
    );
    assert_eq!(
        harness
            .state
            .sidebar
            .closed
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [personal]
    );

    // An account added now starts open, and what was closed stays closed.
    let club = harness
        .store
        .upsert_account("Club", "robin@club.example.com", "imap")
        .unwrap();
    harness
        .store
        .upsert_folder(
            club,
            &RemoteFolder {
                id: FolderId("INBOX".to_owned()),
                name: "Inbox".to_owned(),
                role: FolderRole::Inbox,
            },
        )
        .unwrap();
    changed(&mut harness);
    assert_eq!(
        pane(&harness)[7..],
        ["  ▸ Personal  4", "  ▾ Club", "    Inbox"]
    );

    // Opened again, that is kept too.
    harness.keys("1 G k k l");
    assert!(harness.screen().contains("> ▾ Personal"));
    drop(harness);
    let harness = Harness::over_store(Store::open(&path).unwrap(), 80, 24, false);
    assert!(harness.state.sidebar.closed.is_empty());
    assert!(harness.screen().contains("  ▾ Personal"));
}

#[test]
fn an_answer_from_before_an_account_was_closed_does_not_open_it_again() {
    let mut harness = Harness::demo(80, 24);
    let (work, _) = accounts(&harness);
    // What the store would have said a moment before the key.
    let stale = quill_tui::data::handle(
        &mut harness.store,
        quill_tui::data::DataRequest::LoadSidebar,
    );
    harness.keys("1 j h");
    for event in stale {
        harness.act(Action::Data(event));
    }
    assert!(
        harness.screen().contains("> ▸ Work"),
        "{}",
        harness.screen()
    );
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
}

#[test]
fn the_marker_is_clicked_to_open_and_close_and_the_name_to_choose() {
    let mut harness = Harness::demo(200, 60);
    let (work, personal) = accounts(&harness);
    // The marker itself.
    let marker = find(&harness, "▾ Work");
    click(&mut harness, marker);
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(
        harness.screen().contains("  ▸ Work"),
        "{}",
        harness.screen()
    );
    // Nothing was chosen by it: the list is what it was, the cursor too.
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert!(harness.screen().contains("> Unified"));
    assert_eq!(harness.state.focus, Pane::Sidebar);
    // The margin left of it is the marker's too.
    click(&mut harness, (marker.0 - 2, marker.1));
    assert!(harness.store.closed_accounts().unwrap().is_empty());
    click(&mut harness, (marker.0 + 1, marker.1));
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);

    // The name chooses the account and leaves it as it is.
    let name = find(&harness, "Work");
    assert_eq!(name, (marker.0 + 2, marker.1));
    click(&mut harness, name);
    assert_eq!(harness.state.mailbox, Mailbox::Account(work));
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(harness.screen().contains("> ▸ Work"));
    // A second click on it goes to its mail, as on any mailbox.
    click(&mut harness, name);
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);

    // The rows below a closed account are where they are drawn.
    let sent = find(&harness, "    Sent");
    click(&mut harness, sent);
    assert_eq!(harness.state.mailbox_title(), "Personal / Sent");
    // Closing the account the cursor is in, by its marker, takes the
    // cursor to the account.
    let marker = find(&harness, "▾ Personal");
    click(&mut harness, marker);
    assert_eq!(harness.store.closed_accounts().unwrap(), [work, personal]);
    assert_eq!(harness.state.mailbox, Mailbox::Account(personal));
    assert!(harness.screen().contains("> ▸ Personal"));
    // Under the last row there is nothing to click.
    click(&mut harness, (marker.0, marker.1 + 1));
    click(&mut harness, (marker.0 + 4, marker.1 + 3));
    assert_eq!(harness.state.mailbox, Mailbox::Account(personal));
    assert_eq!(harness.store.closed_accounts().unwrap(), [work, personal]);

    // The wheel goes over the rows that are shown.
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::WheelUp,
        column: marker.0,
        row: marker.1,
    }));
    assert_eq!(harness.state.mailbox, Mailbox::Account(work));
}

#[test]
fn checking_for_new_mail_goes_by_what_is_chosen_in_the_accounts_pane() {
    let mut harness = Harness::with_accounts(120, 30);
    let (work, personal) = accounts(&harness);
    let last = |harness: &Harness| harness.sync_requests.last().unwrap().clone();

    // Unified: every account.
    harness.keys("ctrl-r");
    assert_eq!(harness.sync_requests, [SyncRequest::SyncNow]);
    assert!(harness.screen().contains("Checking for new mail."));

    // An account's own row: that account, and it is said which.
    harness.keys("1 j ctrl-r");
    assert_eq!(last(&harness), SyncRequest::SyncOne(work));
    assert!(harness.screen().contains("Checking Work for new mail."));
    // One of its folders: the same account.
    harness.keys("j j j cmd-r");
    assert_eq!(harness.state.mailbox_title(), "Work / Sent");
    assert_eq!(last(&harness), SyncRequest::SyncOne(work));
    // With the keys in the list, and in the reader, it is still the row
    // chosen in the accounts pane that says.
    harness.keys("2 ctrl-r");
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(last(&harness), SyncRequest::SyncOne(work));
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::Reader);
    harness.keys("cmd-r");
    assert_eq!(last(&harness), SyncRequest::SyncOne(work));
    assert!(harness.screen().contains("Checking Work for new mail."));
    assert_eq!(harness.sync_requests.len(), 5);

    // The other account, from a folder of its own and closed.
    harness.keys("1 G ctrl-r");
    assert_eq!(harness.state.mailbox_title(), "Personal / Trash");
    assert_eq!(last(&harness), SyncRequest::SyncOne(personal));
    assert!(harness.screen().contains("Checking Personal for new mail."));
    harness.keys("h 2 ctrl-r");
    assert_eq!(last(&harness), SyncRequest::SyncOne(personal));

    // Back on Unified, every account again.
    harness.keys("1 g g 2 ctrl-r");
    assert_eq!(last(&harness), SyncRequest::SyncNow);
    assert!(harness.screen().contains("Checking for new mail."));

    // Mail kept only here is nobody's alone: every account.
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    harness.keys("escape escape 1 g g j");
    assert_eq!(harness.state.mailbox, Mailbox::Drafts);
    harness.keys("ctrl-r");
    assert_eq!(last(&harness), SyncRequest::SyncNow);
}

#[test]
fn a_notification_takes_the_panes_cursor_to_unified_with_the_list() {
    let mut harness = Harness::with_accounts(200, 60);
    let id = harness
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| !row.flags.seen)
        .unwrap()
        .id;
    harness.keys("1 j j j j");
    assert_eq!(harness.state.mailbox_title(), "Work / Sent");
    harness.act(Action::Notification {
        id: "arrived".to_owned(),
        action: None,
    });
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert!(
        harness.screen().contains("> Unified"),
        "{}",
        harness.screen()
    );
    harness.keys("1 j j j j");
    harness.act(Action::Notification {
        id: format!("message:{}", id.0),
        action: None,
    });
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert!(harness.screen().contains("> Unified"));
    // So what is checked is what is on show: every account.
    harness.keys("ctrl-r");
    assert_eq!(harness.sync_requests.last(), Some(&SyncRequest::SyncNow));
}

#[test]
fn a_saved_search_checks_every_account_and_an_odd_name_is_cleaned() {
    let mut store = Store::open_in_memory().unwrap();
    let odd = store
        .upsert_account("Cl\u{1b}[31mub\u{7}", "robin@club.example.com", "imap")
        .unwrap();
    let mut harness = Harness::over_store(store, 120, 30, true);
    harness.env.searches = vec![("From Ana".to_owned(), "from:ana".to_owned())];
    changed(&mut harness);
    harness.keys("1 j ctrl-r");
    assert_eq!(harness.sync_requests, [SyncRequest::SyncOne(odd)]);
    let screen = harness.screen();
    assert!(screen.contains("Checking Club for new mail."), "{screen}");
    assert!(!screen.contains('\u{1b}') && !screen.contains('\u{7}'));

    harness.keys("G");
    assert!(harness.screen().contains("Search: from:ana"));
    harness.keys("ctrl-r");
    assert_eq!(harness.sync_requests.last(), Some(&SyncRequest::SyncNow));
}

#[test]
fn there_is_nobody_to_check_in_the_demo_or_before_an_account_is_added() {
    // The demo has no servers.
    let mut harness = Harness::demo(120, 30);
    harness.keys("1 j");
    assert!(!enabled(&harness, CommandId::MailboxSync));
    harness.keys("ctrl-r");
    assert!(harness.sync_requests.is_empty());

    // A first run has no accounts: nothing to check, open, close or choose.
    let mut harness = Harness::first_run(120, 30);
    harness.keys("esc");
    assert!(
        harness.screen().contains("No accounts"),
        "{}",
        harness.screen()
    );
    harness.keys("1");
    for id in [
        CommandId::MailboxSync,
        CommandId::SidebarOpen,
        CommandId::SidebarClose,
    ] {
        assert!(!enabled(&harness, id), "{id:?}");
    }
    harness.keys("h l left right j k");
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert!(harness.sync_requests.is_empty());
}

#[test]
fn the_palette_goes_to_a_folder_of_a_closed_account_by_opening_it() {
    let mut harness = Harness::demo(200, 60);
    let (_, personal) = accounts(&harness);
    harness.keys("1 G h g g");
    assert_eq!(harness.store.closed_accounts().unwrap(), [personal]);
    assert!(harness.screen().contains("  ▸ Personal"));

    // Its folders are still offered by name.
    harness.keys(":");
    harness.type_text("persent");
    let offered: Vec<String> = harness
        .state
        .palette_items(&harness.env)
        .into_iter()
        .map(|item| item.title)
        .collect();
    assert_eq!(offered[0], "Go to Personal / Sent");
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(harness.row_with("[2]").contains("Personal / Sent"));
    // The account was opened for it, the cursor is on the folder, and
    // that is how it is kept.
    assert!(
        harness.screen().contains("  ▾ Personal"),
        "{}",
        harness.screen()
    );
    assert!(harness.row_with(">   Sent").contains("Sent"));
    assert!(harness.state.sidebar.closed.is_empty());
    assert!(harness.store.closed_accounts().unwrap().is_empty());

    // Going somewhere else by name leaves a closed account closed.
    harness.keys("1 h");
    assert_eq!(harness.store.closed_accounts().unwrap(), [personal]);
    harness.keys(":");
    harness.type_text("worksent");
    harness.keys("enter");
    assert!(harness.row_with("[2]").contains("Work / Sent"));
    assert_eq!(harness.store.closed_accounts().unwrap(), [personal]);
}

#[test]
fn leaving_a_search_for_a_folder_closed_meanwhile_opens_its_account() {
    let mut harness = Harness::demo(200, 60);
    harness.env.searches = vec![("From Ana".to_owned(), "from:ana".to_owned())];
    changed(&mut harness);
    let (work, _) = accounts(&harness);
    // From Work / Sent to the saved search: the search is over the list,
    // and Work / Sent is where leaving it goes back to.
    harness.keys("1 j j j j G");
    assert!(harness.screen().contains("Search: from:ana"));
    // Work is closed by its marker while the cursor is elsewhere.
    let marker = find(&harness, "▾ Work");
    click(&mut harness, marker);
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(harness.screen().contains("Search: from:ana"));
    assert!(harness.screen().contains(">   From Ana"));

    harness.keys("esc");
    assert!(!harness.screen().contains("Search: from:ana"));
    assert_eq!(harness.state.mailbox_title(), "Work / Sent");
    assert!(
        harness.screen().contains("  ▾ Work"),
        "{}",
        harness.screen()
    );
    assert!(harness.row_with(">   Sent").contains("Sent"));
    assert!(harness.store.closed_accounts().unwrap().is_empty());
}

#[test]
fn a_long_pane_scrolls_by_the_rows_that_are_shown() {
    // Thirteen rows in a pane ten rows high.
    let mut harness = Harness::demo(80, 13);
    harness.keys("1 G");
    assert_eq!(pane(&harness).last().unwrap(), ">   Trash");
    assert_eq!(pane(&harness).first().unwrap(), "    Drafts");
    // With Work closed everything fits, and nothing is scrolled off.
    harness.keys("g g j h");
    assert_eq!(
        pane(&harness),
        [
            "  Unified    12",
            "> ▸ Work      8",
            "  ▾ Personal",
            "    Inbox     4",
            "    Drafts",
            "    Sent",
            "    Archive",
            "    Trash",
        ]
    );
    harness.keys("G");
    assert_eq!(pane(&harness).first().unwrap(), "  Unified    12");
    assert_eq!(pane(&harness).last().unwrap(), ">   Trash");
    // A click lands on the row that is drawn there.
    let marker = find(&harness, "▾ Personal");
    click(&mut harness, (marker.0 + 2, marker.1));
    assert_eq!(harness.state.mailbox_title(), "Inbox (Personal)");
    assert!(harness.state.sidebar.closed.len() == 1);
}

#[test]
fn under_60_columns_the_overlay_opens_and_closes_accounts_too() {
    let mut harness = Harness::demo(50, 20);
    let (work, _) = accounts(&harness);
    harness.keys("1 j h");
    assert!(
        harness.screen().contains("> ▸ Work"),
        "{}",
        harness.screen()
    );
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    let marker = find(&harness, "▸ Work");
    click(&mut harness, marker);
    assert!(harness.store.closed_accounts().unwrap().is_empty());
    // `enter` goes to the account's mail and puts the overlay away.
    harness.keys("enter");
    assert_eq!(harness.state.focus, Pane::List);
    assert!(harness.screen().contains("Inbox (Work)"));
    assert!(!harness.screen().contains("[1] Accounts"));
}

#[test]
fn with_web_reading_on_the_accounts_pane_is_the_same() {
    let mut harness = Harness::in_the_app(120, 30, true);
    let (work, _) = accounts(&harness);
    assert!(harness.state.web_reading);
    harness.keys("1 j j j h");
    assert_eq!(harness.store.closed_accounts().unwrap(), [work]);
    assert!(
        harness.screen().contains("> ▸ Work"),
        "{}",
        harness.screen()
    );
    assert!(harness.row_with("[2]").contains("Inbox (Work)"));
    // No reader is drawn: the accounts and the list are.
    assert!(!harness.screen().contains("[3] Reader"));
    let marker = find(&harness, "▸ Work");
    click(&mut harness, marker);
    assert!(harness.store.closed_accounts().unwrap().is_empty());
    let name = find(&harness, "Personal");
    click(&mut harness, name);
    assert_eq!(harness.state.mailbox_title(), "Inbox (Personal)");
    // The page follows the account's inbox as it does any list.
    harness.keys("2");
    harness.settle();
    assert!(harness.state.reading_asked().is_some());
}

#[test]
fn snapshots_with_an_account_closed() {
    for (width, height) in [(80, 24), (200, 60)] {
        let mut harness = Harness::demo(width, height);
        // Work closed from one of its folders, the cursor left on it.
        harness.keys("1 j j j h");
        insta::assert_snapshot!(format!("account_closed_{width}x{height}"), harness.screen());
    }
}
