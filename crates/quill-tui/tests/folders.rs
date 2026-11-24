//! Folders the user makes, mail kept on this Mac, and mail put back where
//! it came from, from the UI's side.
//!
//! The store is real and the engine is the harness: what the UI asked of
//! it is looked at, and its answers are handed in. What the engine does
//! with mail that changes hands between a server and this Mac is tested
//! where it is, in `quill-sync`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::types::{
    Address, Flags, FolderId, FolderRole, Headers, LocalAccountId, LocalFolderId, RemoteFolder,
    RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_host::bridge::{FormField, FormUsed};
use quill_store::{Mailbox, Store};
use quill_sync::{SyncEvent, SyncRequest};
use quill_tui::command::{CommandId, Context};
use quill_tui::setup::Leaving;
use quill_tui::state::{Action, RemovePrompt, SidebarEntry};

fn message(id: &str, subject: &str, date: i64) -> RemoteMessage {
    RemoteMessage {
        id: RemoteId(id.to_owned()),
        headers: Headers {
            message_id: Some(format!("<{id}@example.com>")),
            subject: subject.to_owned(),
            from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
            to: vec![Address::new("Robin Vale", "robin@example.com")],
            cc: Vec::new(),
            date,
        },
        flags: Flags::default(),
        size: 100,
        has_attachments: false,
        thread: Vec::new(),
    }
}

fn filled(store: &mut Store, folder: LocalFolderId, messages: Vec<RemoteMessage>) {
    store
        .apply_delta(
            folder,
            &SyncDelta {
                reset: false,
                added: messages,
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
}

/// One account read over IMAP, with an inbox of three, an archive of one,
/// and a trash holding one message that was deleted somewhere else.
fn mailbox() -> (Store, LocalAccountId) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store
        .upsert_account("Work", "robin@example.com", "imap")
        .unwrap();
    let mut folder = |name: &str, role| {
        store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId(name.to_owned()),
                    name: name.to_owned(),
                    role,
                },
            )
            .unwrap()
    };
    let inbox = folder("Inbox", FolderRole::Inbox);
    let archive = folder("Archive", FolderRole::Archive);
    let trash = folder("Trash", FolderRole::Trash);
    filled(
        &mut store,
        inbox,
        vec![
            message("1", "Quarterly figures", 300),
            message("2", "Lunch on Thursday", 200),
            message("3", "Parking permit", 100),
        ],
    );
    filled(&mut store, archive, vec![message("4", "Old contract", 50)]);
    filled(
        &mut store,
        trash,
        vec![message("5", "Deleted on my phone", 40)],
    );
    (store, account)
}

fn harness() -> (Harness, LocalAccountId) {
    let (store, account) = mailbox();
    (Harness::over_store(store, 140, 40, true), account)
}

fn folder(harness: &Harness, name: &str) -> LocalFolderId {
    harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.name == name)
        .unwrap_or_else(|| panic!("no folder {name}"))
        .id
}

fn subjects(harness: &Harness, name: &str) -> Vec<String> {
    let folder = folder(harness, name);
    harness
        .store
        .messages(Mailbox::Folder(folder))
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect()
}

/// The folder the accounts pane's cursor is on.
fn chosen(harness: &Harness) -> String {
    let sidebar = &harness.state.sidebar;
    match &sidebar.entries[sidebar.selected] {
        SidebarEntry::Folder { name, .. } => name.clone(),
        other => panic!("the cursor is on {other:?}"),
    }
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// Makes a folder on this Mac by its keys, from the accounts pane.
fn make_local(harness: &mut Harness, name: &str) {
    harness.keys("1 g g j +");
    harness.type_text(name);
    harness.keys("tab enter");
}

#[test]
fn a_new_folder_is_asked_for_by_name_and_by_where_it_is_to_be() {
    let (mut harness, account) = harness();
    harness.keys("1 g g j");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::FolderCreate)
    );
    harness.keys("+");
    assert_eq!(harness.state.context(), Context::NewFolder);
    let screen = harness.screen();
    for said in ["New folder in Work", "Name:", "Where:", "On the server"] {
        assert!(screen.contains(said), "{said}\n{screen}");
    }
    assert!(screen.contains("tab changes where it is made."), "{screen}");

    // No name, no folder: said where it is typed.
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::NewFolder);
    assert!(
        harness.screen().contains("Give the folder a name."),
        "{}",
        harness.screen()
    );

    // On the server: the engine is asked, and nothing is made here.
    harness.type_text("Projects");
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Sidebar);
    assert_eq!(
        harness.sync_requests.last(),
        Some(&SyncRequest::CreateFolder {
            account,
            name: "Projects".to_owned(),
        })
    );
    assert!(
        status(&harness).contains("Asking the server for Projects"),
        "{}",
        status(&harness)
    );
    assert!(harness.store.folders().unwrap().iter().all(|f| !f.local));
    // The engine's word that it is made is passed on.
    harness.act(Action::Sync(SyncEvent::FolderMade {
        account: "Work".to_owned(),
        name: "Projects".to_owned(),
    }));
    assert_eq!(
        harness.state.notices.back().unwrap().text,
        "Made the folder Projects on Work's server."
    );

    // On this Mac: `tab` changes the place, and what it means is said
    // before anything is made.
    harness.keys("esc +");
    harness.type_text("Receipts");
    harness.keys("tab");
    let screen = harness.screen();
    assert!(screen.contains("On this Mac"), "{screen}");
    // Said in however many lines it takes.
    assert!(
        screen.contains("Mail moved into a folder on this Mac"),
        "{screen}"
    );
    let means = harness.state.new_folder.as_ref().unwrap().means();
    assert!(
        means.contains("kept here whole and taken off the server"),
        "{means}"
    );
    let asked = harness.sync_requests.len();
    harness.keys("enter");
    assert_eq!(harness.sync_requests.len(), asked);
    let made = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.local)
        .unwrap();
    assert_eq!(made.name, "Receipts");
    assert!(
        status(&harness).contains("Made Receipts, on this Mac."),
        "{}",
        status(&harness)
    );

    // `esc` makes nothing.
    harness.keys("+");
    harness.type_text("Never");
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::Sidebar);
    assert_eq!(harness.store.folders().unwrap().len(), 4);
}

#[test]
fn a_folder_on_this_mac_is_under_its_own_heading_and_named_as_such_everywhere() {
    let (mut harness, _) = harness();
    make_local(&mut harness, "Receipts");
    let screen = harness.screen();
    // Under the account, after the server's folders, beneath a heading.
    let rows: Vec<&str> = screen.lines().collect();
    let heading = rows
        .iter()
        .position(|row| row.contains("On This Mac"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(rows[heading - 1].contains("Trash"), "{screen}");
    assert!(rows[heading + 1].contains("Receipts"), "{screen}");

    // The cursor steps over the heading: from Trash straight to the folder.
    harness.keys("1 g g j j j j");
    assert_eq!(chosen(&harness), "Trash");
    harness.keys("j");
    assert_eq!(chosen(&harness), "Receipts");
    assert!(!harness.row_with("On This Mac").contains("> "));
    // And back over it.
    harness.keys("k");
    assert_eq!(chosen(&harness), "Trash");
    harness.keys("j");
    // Opened, the list says whose folder it is and where it is.
    harness.keys("enter");
    assert_eq!(
        harness.state.mailbox_title(),
        "Work / Receipts (on this Mac)"
    );

    // Where a message is about to be moved, the folder is named as what
    // it is.
    harness.keys("1 g g j enter m");
    assert_eq!(harness.state.context(), Context::Folders);
    harness.type_text("rec");
    assert!(
        harness.screen().contains("Receipts (on this Mac)"),
        "{}",
        harness.screen()
    );
    harness.keys("enter");
    assert!(
        status(&harness).contains("Moved to this Mac: it is taken off the server."),
        "{}",
        status(&harness)
    );
    assert_eq!(subjects(&harness, "Receipts"), ["Quarterly figures"]);
    assert_eq!(
        subjects(&harness, "Inbox"),
        ["Lunch on Thursday", "Parking permit"]
    );
    // Removing the account would lose it, and that is said first of all.
    let prompt = RemovePrompt {
        account: "Work".to_owned(),
        leaving: Leaving {
            address: "robin@example.com".to_owned(),
            local: 1,
            ..Leaving::default()
        },
        busy: false,
    };
    assert_eq!(
        prompt.losses(),
        ["1 message in its folders on this Mac is nowhere else and will be deleted."]
    );
    assert!(prompt.loses_something());
}

#[test]
fn mail_on_this_mac_is_deleted_from_this_mac_and_an_empty_folder_can_be_taken_away() {
    let (mut harness, _) = harness();
    make_local(&mut harness, "Receipts");
    harness.keys("1 g g j enter m");
    harness.type_text("rec");
    harness.keys("enter");

    // On the folder, with mail in it: it stays, and why is said.
    harness.keys("1 g g j j j j j");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::FolderRemove)
    );
    assert!(
        status(&harness).contains("delete folder"),
        "{}",
        status(&harness)
    );
    harness.keys("_");
    assert!(
        status(&harness).contains("has mail in it. Move or delete that first."),
        "{}",
        status(&harness)
    );
    // The command is for a folder on this Mac and no other.
    harness.keys("k");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::FolderRemove)
    );

    // Its message is deleted: not to the server's trash, which never had
    // it, but from this Mac, and said so.
    harness.keys("j enter d");
    assert!(
        status(&harness).contains("Deleted from this Mac for good."),
        "{}",
        status(&harness)
    );
    assert!(subjects(&harness, "Receipts").is_empty());
    assert_eq!(subjects(&harness, "Trash"), ["Deleted on my phone"]);
    // For as long as any other change, it can be had back.
    harness.keys("U");
    assert_eq!(subjects(&harness, "Receipts"), ["Quarterly figures"]);
}

#[test]
fn a_message_in_the_trash_is_put_back_where_it_came_from() {
    let (mut harness, _) = harness();
    // One deleted from the inbox, one from the archive.
    harness.keys("1 g g j enter d");
    assert!(
        status(&harness).contains("Moved to the trash."),
        "{}",
        status(&harness)
    );
    harness.keys("1 g g j j j enter d");
    assert!(subjects(&harness, "Archive").is_empty());
    assert_eq!(
        subjects(&harness, "Trash"),
        ["Quarterly figures", "Old contract", "Deleted on my phone"]
    );

    // From anywhere but a trash folder, the one deleted last comes back.
    harness.keys("1 g g enter");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessagePutBack)
    );
    harness.keys("P");
    assert!(
        status(&harness).contains("Put back in Archive."),
        "{}",
        status(&harness)
    );
    assert_eq!(subjects(&harness, "Archive"), ["Old contract"]);

    // In the trash, the message under the cursor does: the one from the
    // inbox to the inbox.
    harness.keys("1 g g j j j j enter g g");
    assert_eq!(harness.selected_subject(), "Quarterly figures");
    harness.keys("P");
    assert!(
        status(&harness).contains("Put back in Inbox."),
        "{}",
        status(&harness)
    );
    assert_eq!(
        subjects(&harness, "Inbox"),
        ["Quarterly figures", "Lunch on Thursday", "Parking permit"]
    );
    // One that was deleted somewhere else has no place remembered: it
    // goes to the inbox, and that is said.
    assert_eq!(harness.selected_subject(), "Deleted on my phone");
    harness.keys("P");
    assert!(
        status(&harness).contains("Put back in the inbox: where it came from is not known."),
        "{}",
        status(&harness)
    );
    assert!(subjects(&harness, "Trash").is_empty());
    // It is a change like any other: one undo takes it back.
    harness.keys("U");
    assert_eq!(subjects(&harness, "Trash"), ["Deleted on my phone"]);

    // With nothing left that Quill deleted, there is nothing to put back.
    harness.keys("1 g g enter P");
    assert!(
        status(&harness).contains("Nothing to put back"),
        "{}",
        status(&harness)
    );
}

#[test]
fn several_messages_are_put_back_together_and_copied_together() {
    let (mut harness, _) = harness();
    make_local(&mut harness, "Receipts");
    // Two selected in the inbox and deleted at once.
    harness.keys("1 g g j enter x x d");
    assert_eq!(subjects(&harness, "Inbox"), ["Parking permit"]);
    // Selected in the trash and put back at once.
    harness.keys("1 g g j j j j enter g g x x P");
    assert!(
        status(&harness).contains("Put back 2 messages where they came from."),
        "{}",
        status(&harness)
    );
    assert_eq!(
        subjects(&harness, "Inbox"),
        ["Quarterly figures", "Lunch on Thursday", "Parking permit"]
    );

    // Copied: the picker says so, and nothing moves.
    harness.keys("1 g g j enter x x C");
    assert_eq!(harness.state.context(), Context::Folders);
    assert!(
        harness.screen().contains(" Copy to "),
        "{}",
        harness.screen()
    );
    harness.type_text("rec");
    harness.keys("enter");
    assert!(
        status(&harness).contains("Copying 2 messages to Receipts, on this Mac."),
        "{}",
        status(&harness)
    );
    assert_eq!(subjects(&harness, "Inbox").len(), 3);
    // The engine has the copying to do.
    assert_eq!(harness.store.queued_operations().unwrap(), 2 + 2 + 2);
    assert!(
        harness
            .sync_requests
            .iter()
            .any(|request| matches!(request, SyncRequest::Queue))
    );
}

#[test]
fn in_the_app_the_new_folder_is_asked_for_in_a_sheet() {
    let (mut harness, account) = harness();
    harness.env.sheets = true;
    harness.keys("1 g g j +");
    let form = harness.state.form(&harness.env).unwrap();
    assert_eq!(form.step(), "new_folder");
    assert_eq!(form.title(), "New Folder in Work");
    match form.fields() {
        [
            FormField::Text { id: name, .. },
            FormField::Choice {
                id: place,
                value,
                options,
                help,
                ..
            },
        ] => {
            assert_eq!((name.as_str(), place.as_str()), ("name", "place"));
            assert_eq!(value, "server");
            let titles: Vec<&str> = options.iter().map(|option| option.title.as_str()).collect();
            assert_eq!(titles, ["On the server", "On this Mac"]);
            assert!(
                help.as_deref()
                    .unwrap()
                    .contains("made on the account's server")
            );
        }
        other => panic!("{other:?}"),
    }
    let buttons: Vec<(&str, &str)> = form
        .buttons()
        .iter()
        .map(|button| (button.id.as_str(), button.title.as_str()))
        .collect();
    assert_eq!(buttons, [("cancel", "Cancel"), ("confirm", "Create")]);

    // The place is changed: what is said of it changes with it, and
    // nothing is made yet.
    harness.act(Action::FormUsed(
        FormUsed::new(
            "new_folder",
            None,
            &[("name", "Receipts"), ("place", "here")],
            &[],
        )
        .unwrap(),
    ));
    let form = harness.state.form(&harness.env).unwrap();
    match &form.fields()[1] {
        FormField::Choice { value, help, .. } => {
            assert_eq!(value, "here");
            assert!(
                help.as_deref().unwrap().contains("taken off the"),
                "{help:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(harness.store.folders().unwrap().iter().all(|f| !f.local));

    // Create: the folder is made on this Mac, and the sheet goes.
    harness.act(Action::FormUsed(
        FormUsed::new(
            "new_folder",
            Some("confirm"),
            &[("name", "Receipts"), ("place", "here")],
            &[],
        )
        .unwrap(),
    ));
    assert!(harness.state.form(&harness.env).is_none());
    assert!(harness.store.folders().unwrap().iter().any(|f| f.local));

    // On the server, from the sheet: the engine is asked.
    harness.keys("+");
    harness.act(Action::FormUsed(
        FormUsed::new(
            "new_folder",
            Some("confirm"),
            &[("name", "Projects"), ("place", "server")],
            &[],
        )
        .unwrap(),
    ));
    assert_eq!(
        harness.sync_requests.last(),
        Some(&SyncRequest::CreateFolder {
            account,
            name: "Projects".to_owned(),
        })
    );
}

#[test]
fn the_demo_mailbox_has_no_server_so_its_folders_are_made_on_this_mac() {
    let mut harness = Harness::demo(140, 40);
    harness.keys("1 j +");
    assert_eq!(harness.state.context(), Context::NewFolder);
    let screen = harness.screen();
    assert!(screen.contains("On this Mac"), "{screen}");
    assert!(
        !screen.contains("tab changes where it is made."),
        "{screen}"
    );
    harness.type_text("Receipts");
    // `tab` has nothing to change.
    harness.keys("tab enter");
    assert!(
        harness
            .sync_requests
            .iter()
            .all(|request| !matches!(request, SyncRequest::CreateFolder { .. }))
    );
    assert!(harness.store.folders().unwrap().iter().any(|f| f.local));
}

/// The mailbox, with folders of the user's own on the server: `Projects`
/// with two messages and `2026` inside it, and `Projects 2` beside it.
fn with_projects() -> (Harness, LocalAccountId) {
    let (mut store, account) = mailbox();
    let mut folder = |name: &str| {
        store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId(name.to_owned()),
                    name: name.to_owned(),
                    role: FolderRole::Other,
                },
            )
            .unwrap()
    };
    let projects = folder("Projects");
    folder("Projects 2");
    folder("Projects/2026");
    folder("Projects/2026/May");
    filled(
        &mut store,
        projects,
        vec![message("6", "Plan", 30), message("7", "Budget", 20)],
    );
    (Harness::over_store(store, 140, 40, true), account)
}

/// Puts the accounts pane's cursor on the folder called `name`.
fn go_to(harness: &mut Harness, name: &str) {
    harness.keys("1 g g");
    for _ in 0..40 {
        let sidebar = &harness.state.sidebar;
        if matches!(&sidebar.entries[sidebar.selected],
            SidebarEntry::Folder { name: here, .. } if here == name)
        {
            return;
        }
        harness.keys("j");
    }
    panic!("no folder {name} in the accounts pane");
}

#[test]
fn a_folder_inside_another_is_drawn_under_it_and_a_step_further_in() {
    let (mut harness, _) = with_projects();
    make_local(&mut harness, "Kept");
    make_local(&mut harness, "Kept/Tax");
    let rows: Vec<(String, u8, String)> = harness
        .state
        .sidebar
        .entries
        .iter()
        .filter_map(|entry| match entry {
            SidebarEntry::Folder {
                name, depth, label, ..
            } => Some((name.clone(), *depth, label.clone())),
            _ => None,
        })
        .collect();
    let row = |name: &str, depth: u8, label: &str| (name.to_owned(), depth, label.to_owned());
    assert_eq!(
        rows,
        [
            row("Inbox", 0, "Inbox"),
            row("Archive", 0, "Archive"),
            row("Trash", 0, "Trash"),
            // Under the folder it is inside, before the one that only
            // sorts between them.
            row("Projects", 0, "Projects"),
            row("Projects/2026", 1, "2026"),
            row("Projects/2026/May", 2, "May"),
            row("Projects 2", 0, "Projects 2"),
            // This Mac's folders among themselves.
            row("Kept", 0, "Kept"),
            row("Kept/Tax", 1, "Tax"),
        ]
    );
    let screen = harness.screen();
    let column = |label: &str| {
        screen
            .lines()
            .find_map(|line| {
                // The accounts pane's part of the row, without its edge,
                // its cursor and its count.
                let cell: String = line.chars().skip(1).take(16).collect();
                (cell.trim() == label).then(|| cell.chars().position(|c| c != ' '))?
            })
            .unwrap_or_else(|| panic!("no {label} in\n{screen}"))
    };
    assert_eq!(column("2026"), column("Projects 2") + 2);
    assert_eq!(column("May"), column("2026") + 2);
    assert_eq!(column("Tax"), column("Kept") + 2);
    // Wherever else it is named, a folder has its whole name.
    go_to(&mut harness, "Projects/2026");
    harness.keys("enter");
    assert!(screen_has(&harness, "Projects/2026"));
}

/// What was asked of the engine about folders: not the messages the
/// reader asked for as the cursor went by.
fn asked(harness: &Harness) -> Vec<SyncRequest> {
    harness
        .sync_requests
        .iter()
        .filter(|request| !matches!(request, SyncRequest::FetchBody(_)))
        .cloned()
        .collect()
}

fn screen_has(harness: &Harness, text: &str) -> bool {
    harness.screen().contains(text)
}

#[test]
fn a_folder_on_the_server_is_renamed_by_asking_the_server() {
    let (mut harness, _) = with_projects();
    // Not a folder the server keeps for a purpose.
    go_to(&mut harness, "Trash");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::FolderRename)
    );
    go_to(&mut harness, "Projects");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::FolderRename)
    );
    assert!(status(&harness).contains("rename"), "{}", status(&harness));

    harness.keys("R");
    assert_eq!(harness.state.context(), Context::NewFolder);
    let screen = harness.screen();
    assert!(screen.contains("Rename Projects"), "{screen}");
    assert!(screen.contains("On the server"), "{screen}");
    assert!(
        screen.contains("renamed on the account's server"),
        "{screen}"
    );
    assert!(status(&harness).contains("rename"), "{}", status(&harness));
    // The name it has is there to be changed. A name with nothing on one
    // side of a slash is refused before anything is asked.
    harness.type_text("/");
    harness.keys("enter");
    assert!(harness.screen().contains("each side of a /"));
    assert!(asked(&harness).is_empty());
    harness.keys("backspace");
    harness.type_text(" 2027");
    harness.keys("enter");
    assert_eq!(
        asked(&harness),
        [SyncRequest::RenameFolder {
            folder: folder(&harness, "Projects"),
            name: "Projects 2027".to_owned(),
        }]
    );
    assert!(harness.state.new_folder.is_none());
    assert!(
        status(&harness).contains("Asking the server to rename Projects to Projects 2027"),
        "{}",
        status(&harness)
    );

    // What the engine says is said: done, or why not.
    harness.act(Action::Sync(SyncEvent::FolderRenamed {
        account: "Work".to_owned(),
        from: "Projects".to_owned(),
        to: "Projects 2027".to_owned(),
    }));
    assert!(screen_has(
        &harness,
        "Renamed Projects to Projects 2027 on Work's server."
    ));

    // The name it has, entered again, asks the server for nothing.
    harness.sync_requests.clear();
    harness.keys("R enter");
    assert!(harness.state.new_folder.is_none());
    assert!(asked(&harness).is_empty());
    // And `esc` leaves it as it was.
    harness.keys("R");
    harness.type_text("x");
    harness.keys("esc");
    assert!(harness.state.new_folder.is_none());
    assert!(asked(&harness).is_empty());
}

#[test]
fn a_folder_on_this_mac_is_renamed_here_with_the_folders_inside_it() {
    let (mut harness, _) = harness();
    make_local(&mut harness, "Kept");
    make_local(&mut harness, "Kept/Tax");
    go_to(&mut harness, "Kept");
    harness.keys("R");
    let screen = harness.screen();
    assert!(screen.contains("Rename Kept"), "{screen}");
    assert!(screen.contains("On this Mac"), "{screen}");
    harness.keys("ctrl-u");
    harness.type_text("Papers");
    harness.keys("enter");
    assert!(asked(&harness).is_empty());
    assert!(
        status(&harness).contains("Renamed Kept to Papers, on this Mac."),
        "{}",
        status(&harness)
    );
    let names: Vec<String> = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .filter(|folder| folder.local)
        .map(|folder| folder.name)
        .collect();
    assert_eq!(names, ["Papers", "Papers/Tax"]);
    // A name that is taken is said, and nothing is renamed.
    go_to(&mut harness, "Papers/Tax");
    harness.keys("R ctrl-u");
    harness.type_text("papers");
    harness.keys("enter");
    assert!(
        status(&harness).contains("already a folder called papers on this Mac"),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_folder_on_the_server_is_deleted_only_after_saying_where_its_mail_goes() {
    let (mut harness, _) = with_projects();
    // One with folders inside it is not asked about: why is said.
    go_to(&mut harness, "Projects");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::FolderRemove)
    );
    harness.keys("_");
    assert!(harness.state.new_folder.is_none());
    assert!(
        status(&harness).contains("Projects has folders inside it."),
        "{}",
        status(&harness)
    );

    // An empty one is asked about, and nothing is asked of the server
    // until the answer.
    go_to(&mut harness, "Projects/2026/May");
    harness.keys("_");
    assert_eq!(harness.state.context(), Context::NewFolder);
    let screen = harness.screen();
    assert!(screen.contains("Delete Projects/2026/May?"), "{screen}");
    assert!(screen.contains("There is no mail in it."), "{screen}");
    assert!(!screen.contains("Name:"), "{screen}");
    assert!(status(&harness).contains("delete"), "{}", status(&harness));
    assert!(asked(&harness).is_empty());
    // Typing is not an answer, and `esc` keeps the folder.
    harness.type_text("y");
    assert!(asked(&harness).is_empty());
    harness.keys("esc");
    assert!(harness.state.new_folder.is_none());
    assert!(asked(&harness).is_empty());

    // One with mail in it says where the mail goes: this account has a
    // trash folder.
    let gone = folder(&harness, "Projects/2026");
    harness.store.forget_folder(gone).unwrap();
    let gone = folder(&harness, "Projects/2026/May");
    harness.store.forget_folder(gone).unwrap();
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    go_to(&mut harness, "Projects");
    harness.keys("_");
    let screen = harness.screen();
    assert!(screen.contains("Delete Projects?"), "{screen}");
    assert!(
        screen.contains("The 2 messages in it are moved to Trash first"),
        "{screen}"
    );
    harness.keys("enter");
    assert_eq!(
        asked(&harness),
        [SyncRequest::DeleteFolder(folder(&harness, "Projects"))]
    );
    assert!(
        status(&harness).contains("Asking the server to delete Projects"),
        "{}",
        status(&harness)
    );
    harness.act(Action::Sync(SyncEvent::FolderDeleted {
        account: "Work".to_owned(),
        name: "Projects".to_owned(),
        mail: quill_core::types::FolderMail::Trashed,
        messages: 2,
    }));
    assert!(screen_has(
        &harness,
        "Deleted Projects from Work's server. The mail that was in it is in the trash."
    ));
}

#[test]
fn what_becomes_of_a_deleted_folders_mail_goes_by_the_kind_of_account() {
    use quill_tui::state::{FolderLeaving, LeavingMail};
    let leaving = |messages, inside, mail| FolderLeaving {
        folder: LocalFolderId(7),
        account: LocalAccountId(1),
        name: "Receipts".to_owned(),
        messages,
        inside,
        busy: false,
        mail,
    };
    let trash = || LeavingMail::Trash("Bin".to_owned());
    assert!(leaving(3, 0, trash()).refusal().is_none());
    assert!(
        leaving(3, 0, trash())
            .becomes()
            .contains("The 3 messages in it are moved to Bin first")
    );
    assert!(
        leaving(1, 0, LeavingMail::AllMail)
            .becomes()
            .contains("The 1 message in it stays in All Mail.")
    );
    assert!(
        leaving(2, 0, LeavingMail::WithFolder)
            .becomes()
            .contains("moved into Deleted Items")
    );
    assert!(
        leaving(0, 0, LeavingMail::Nowhere)
            .becomes()
            .contains("There is no mail in it.")
    );
    // With mail and nowhere for it to go, or folders inside, or changes
    // still on their way, it is not asked about.
    let refused = leaving(2, 0, LeavingMail::Nowhere).refusal().unwrap();
    assert!(refused.contains("no trash folder"), "{refused}");
    assert!(leaving(0, 1, trash()).refusal().unwrap().contains("inside"));
    let mut busy = leaving(0, 0, trash());
    busy.busy = true;
    assert!(busy.refusal().unwrap().contains("on their way"));
}

#[test]
fn in_the_app_renaming_and_deleting_a_folder_are_asked_in_sheets_of_their_own() {
    let (mut harness, _) = with_projects();
    harness.env.sheets = true;
    go_to(&mut harness, "Projects 2");
    harness.keys("R");
    let form = harness.state.form(&harness.env).unwrap();
    assert_eq!(form.step(), "rename_folder");
    assert_eq!(form.title(), "Rename Projects 2");
    match form.fields() {
        [
            FormField::Text { id, value, .. },
            FormField::Static { help, .. },
        ] => {
            assert_eq!((id.as_str(), value.as_str()), ("name", "Projects 2"));
            let help = help.as_deref().unwrap();
            assert!(help.contains("renamed on the account's server"), "{help}");
            assert!(help.contains("Projects/2026"), "{help}");
        }
        other => panic!("{other:?}"),
    }
    let titles: Vec<&str> = form.buttons().iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["Cancel", "Rename"]);
    harness.act(Action::FormUsed(
        FormUsed::new(
            "rename_folder",
            Some("confirm"),
            &[("name", "Projects 3")],
            &[],
        )
        .unwrap(),
    ));
    assert_eq!(
        asked(&harness),
        [SyncRequest::RenameFolder {
            folder: folder(&harness, "Projects 2"),
            name: "Projects 3".to_owned(),
        }]
    );

    // Deleting: nothing to type, what happens said under the question,
    // and no button that `return` presses.
    harness.sync_requests.clear();
    harness.keys("_");
    let form = harness.state.form(&harness.env).unwrap();
    assert_eq!(form.step(), "delete_folder");
    assert_eq!(form.title(), "Delete Projects 2?");
    assert!(form.fields().is_empty());
    assert!(form.note().unwrap().contains("There is no mail in it."));
    assert!(
        form.buttons()
            .iter()
            .all(|button| { button.role != Some(quill_host::bridge::FormRole::Default) })
    );
    // A press meant for the question before is not this one's answer.
    harness.act(Action::FormUsed(
        FormUsed::new("rename_folder", Some("confirm"), &[("name", "x")], &[]).unwrap(),
    ));
    assert!(asked(&harness).is_empty());
    harness.act(Action::FormUsed(
        FormUsed::new("delete_folder", Some("confirm"), &[], &[]).unwrap(),
    ));
    assert_eq!(
        asked(&harness),
        [SyncRequest::DeleteFolder(folder(&harness, "Projects 2"))]
    );
}
