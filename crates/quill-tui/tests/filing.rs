//! Archiving, deleting and moving messages, and taking it back.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::types::FolderRole;
use quill_store::Mailbox;
use quill_sync::SyncRequest;
use quill_tui::command::{CommandId, Context};
use quill_tui::state::Action;

/// The subjects in the folder of `account` with `role`, as the store has
/// them.
fn in_folder(harness: &Harness, account: &str, role: FolderRole) -> Vec<String> {
    let account = harness
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|row| row.name == account)
        .unwrap()
        .id;
    let folder = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.account == account && folder.role == role)
        .unwrap()
        .id;
    harness
        .store
        .messages(Mailbox::Folder(folder))
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect()
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

#[test]
fn archiving_moves_the_message_and_the_cursor_takes_the_next() {
    let mut harness = Harness::demo(200, 60);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    let before = harness.state.list.rows.len();
    harness.keys("e");

    // Gone from the inbox, in its own account's archive, and queued for
    // the server.
    assert_eq!(harness.state.list.rows.len(), before - 1);
    assert_eq!(harness.selected_subject(), "[quill] PR #42 passed");
    assert!(in_folder(&harness, "Work", FolderRole::Archive).contains(&"Q4 roadmap".to_owned()));
    assert_eq!(harness.store.queued_operations().unwrap(), 1);
    assert_eq!(harness.sync_requests, [SyncRequest::Queue]);
    // What happened, and how to take it back, until the next key.
    assert!(
        status(&harness).contains("Archived. U undoes it."),
        "{}",
        status(&harness)
    );
    harness.keys("j");
    assert!(!status(&harness).contains("Archived"));
}

#[test]
fn undo_puts_the_message_back_where_it_was() {
    let mut harness = Harness::demo(200, 60);
    let before: Vec<String> = harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.clone())
        .collect();
    harness.keys("e U");
    let after: Vec<String> = harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.clone())
        .collect();
    assert_eq!(after, before);
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
    assert!(status(&harness).contains("Undone."));
    assert!(!status(&harness).contains("undoes it"));

    // With nothing left to take back, it says so.
    harness.keys("U");
    assert!(status(&harness).contains("Nothing to undo."));
}

#[test]
fn a_message_from_another_account_goes_to_that_accounts_folders() {
    let mut harness = Harness::demo(200, 60);
    // The third message of the unified inbox is from the Personal account.
    harness.keys("j j");
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    harness.keys("e");
    assert!(
        in_folder(&harness, "Personal", FolderRole::Archive).contains(&"Sunday dinner".to_owned())
    );
    assert!(
        !in_folder(&harness, "Work", FolderRole::Archive).contains(&"Sunday dinner".to_owned())
    );
}

#[test]
fn deleting_goes_to_the_trash_first_and_from_the_trash_for_good() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("d");
    assert!(in_folder(&harness, "Work", FolderRole::Trash).contains(&"Q4 roadmap".to_owned()));
    assert!(status(&harness).contains("Moved to the trash. U undoes it."));

    // In the trash, deleting is for good: hidden at once, removed when the
    // server confirms, and still undoable until then.
    let trash = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Trash && folder.name == "Trash")
        .unwrap();
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    harness.keys("1");
    // Work's Trash is five mailboxes below the unified inbox.
    harness.keys("j j j j j enter");
    assert_eq!(harness.state.mailbox, Mailbox::Folder(trash.id));
    let there = harness.state.list.rows.len();
    assert!(there >= 1);
    let subject = harness.selected_subject().to_owned();
    harness.keys("d");
    assert_eq!(harness.state.list.rows.len(), there - 1);
    assert!(status(&harness).contains("Deleted for good. U undoes it."));
    assert!(!in_folder(&harness, "Work", FolderRole::Trash).contains(&subject));
    harness.keys("U");
    assert!(in_folder(&harness, "Work", FolderRole::Trash).contains(&subject));
}

#[test]
fn filing_from_the_reader_reads_on_with_the_next_message() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Reader);
    harness.keys("e");
    // Still reading: the next message has taken its place.
    assert_eq!(harness.state.context(), Context::Reader);
    let reading = harness
        .state
        .reader
        .row
        .as_ref()
        .unwrap()
        .headers
        .subject
        .clone();
    assert_eq!(reading, "[quill] PR #42 passed");
    assert!(harness.screen().contains("All 214 checks passed"));
}

#[test]
fn a_message_is_moved_to_a_folder_chosen_by_typing() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("m");
    assert_eq!(harness.state.context(), Context::Folders);
    let screen = harness.screen();
    assert!(screen.contains(" Move to "));
    // The folders of the message's own account, other than where it is.
    let picker = harness.state.folders.clone().unwrap();
    let names: Vec<String> = harness
        .state
        .folder_choices(&picker)
        .into_iter()
        .map(|choice| choice.name)
        .collect();
    assert_eq!(names, ["Drafts", "Sent", "Archive", "Trash"]);

    // Letters are typing here, not commands, and narrow the list.
    let selected = harness.state.list.selected;
    harness.type_text("ah");
    assert_eq!(harness.state.list.selected, selected);
    let picker = harness.state.folders.clone().unwrap();
    assert_eq!(harness.state.folder_choices(&picker).len(), 2);
    assert!(harness.screen().contains("> Archive"));
    harness.type_text("v");
    let picker = harness.state.folders.clone().unwrap();
    assert_eq!(harness.state.folder_choices(&picker).len(), 1);

    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::List);
    assert!(in_folder(&harness, "Work", FolderRole::Archive).contains(&"Q4 roadmap".to_owned()));
    assert!(status(&harness).contains("Moved. U undoes it."));
}

#[test]
fn the_folder_picker_moves_with_the_arrows_and_closes_without_moving() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("m down down");
    assert_eq!(harness.state.folders.as_ref().unwrap().selected, 2);
    harness.keys("up");
    assert!(harness.screen().contains("> Sent"));
    // Typing starts the narrower list from its top.
    harness.type_text("t");
    assert_eq!(harness.state.folders.as_ref().unwrap().selected, 0);
    // Nothing matches: nothing to move to.
    harness.type_text("zzz");
    assert!(harness.screen().contains("No folder matches."));
    assert!(!harness.state.is_enabled(&harness.env, CommandId::NavOpen));
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Folders);

    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
}

#[test]
fn filing_commands_need_a_message() {
    let mut harness = Harness::first_run(200, 60);
    harness.keys("esc");
    for command in [
        CommandId::MessageArchive,
        CommandId::MessageDelete,
        CommandId::MessageMove,
    ] {
        assert!(
            !harness.state.is_enabled(&harness.env, command),
            "{command:?}"
        );
    }
    harness.keys("e d m");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.folders.is_none());
}

#[test]
fn what_just_happened_shows_even_when_the_bar_is_full() {
    let mut harness = Harness::demo(60, 20);
    harness.keys("e");
    assert!(
        status(&harness).contains("Archived."),
        "{}",
        status(&harness)
    );
}

#[test]
fn snapshot_of_the_folder_picker() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let mut harness = Harness::demo(width, height);
        harness.keys("m");
        harness.type_text("a");
        insta::assert_snapshot!(format!("folder_picker_{width}x{height}"), harness.screen());
    }
}
