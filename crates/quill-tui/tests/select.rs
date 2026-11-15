//! Selecting several messages and acting on them together.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::types::FolderRole;
use quill_store::Mailbox;

fn subjects(harness: &Harness) -> Vec<String> {
    harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.clone())
        .collect()
}

fn selected(harness: &Harness) -> Vec<String> {
    let ids = harness.state.list.marked_ids();
    harness
        .state
        .list
        .rows
        .iter()
        .filter(|row| ids.contains(&row.id))
        .map(|row| row.headers.subject.clone())
        .collect()
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// The subjects in every folder with `role`, whatever the account.
fn in_role(harness: &Harness, role: FolderRole) -> Vec<String> {
    harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .filter(|folder| folder.role == role)
        .flat_map(|folder| harness.store.messages(Mailbox::Folder(folder.id)).unwrap())
        .map(|row| row.headers.subject)
        .collect()
}

#[test]
fn x_selects_the_message_and_moves_on() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("x x");
    assert_eq!(selected(&harness), all[..2]);
    assert_eq!(harness.selected_subject(), all[2]);
    // Said in words and shown by a mark, not by colour alone.
    assert!(harness.screen().contains("2 selected"));
    assert!(harness.row_with(&all[0]).contains(" +"));
    assert!(!harness.row_with(&all[2]).contains('+'));

    // Again on a selected message takes it out.
    harness.keys("k x");
    assert_eq!(selected(&harness), all[..1]);
    assert!(harness.screen().contains("1 selected"));
}

#[test]
fn a_range_follows_the_cursor_until_it_is_ended() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("j V j j");
    assert_eq!(selected(&harness), all[1..4]);
    // Back up past where it began: the range turns round.
    harness.keys("k k k");
    assert_eq!(selected(&harness), all[..2]);
    harness.keys("j j j");
    // Ended, it stays as it was, and the cursor is free again.
    harness.keys("V j j");
    assert_eq!(selected(&harness), all[1..4]);
    // A second range adds to the first.
    harness.keys("V j");
    assert_eq!(
        selected(&harness),
        all[1..4]
            .iter()
            .chain(&all[5..7])
            .cloned()
            .collect::<Vec<_>>()
    );
}

#[test]
fn escape_lets_go_of_the_selection_before_anything_else() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("x V j");
    assert!(harness.state.list.has_marks());
    harness.keys("esc");
    assert!(!harness.state.list.has_marks());
    assert!(!harness.screen().contains("selected"));
}

#[test]
fn everything_listed_can_be_selected_at_once() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("ctrl-a");
    assert_eq!(selected(&harness), all);
    assert!(
        harness
            .screen()
            .contains(&format!("{} selected", all.len()))
    );
}

#[test]
fn archiving_takes_every_selected_message_and_one_undo_brings_them_all_back() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    // The first three are from two accounts: each goes to its own archive.
    harness.keys("x x x e");
    assert_eq!(subjects(&harness), all[3..]);
    let archived = in_role(&harness, FolderRole::Archive);
    for subject in &all[..3] {
        assert!(archived.contains(subject), "{subject} was not archived");
    }
    assert_eq!(harness.store.queued_operations().unwrap(), 3);
    assert!(
        status(&harness).contains("Archived 3 messages. U undoes it."),
        "{}",
        status(&harness)
    );
    assert!(!harness.state.list.has_marks());

    harness.keys("U");
    assert_eq!(subjects(&harness), all);
    assert_eq!(harness.store.queued_operations().unwrap(), 0);
    assert!(
        status(&harness).contains("Undone: 3 messages are back where they were."),
        "{}",
        status(&harness)
    );
}

#[test]
fn deleting_takes_every_selected_message() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("V j d");
    assert_eq!(subjects(&harness), all[2..]);
    let trash = in_role(&harness, FolderRole::Trash);
    assert!(trash.contains(&all[0]) && trash.contains(&all[1]));
    assert!(status(&harness).contains("Deleted 2 messages. U undoes it."));
    harness.keys("U");
    assert_eq!(subjects(&harness), all);
}

#[test]
fn marks_are_set_the_same_way_on_everything_selected() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("x x x");
    let ids = harness.state.list.marked_ids();
    let flags = |harness: &Harness| -> Vec<(bool, bool)> {
        harness
            .state
            .list
            .rows
            .iter()
            .filter(|row| ids.contains(&row.id))
            .map(|row| (row.flags.seen, row.flags.flagged))
            .collect()
    };
    // Some are unread: all become read. Then all read: all become unread.
    assert!(flags(&harness).iter().any(|(seen, _)| !seen));
    harness.keys("u");
    assert!(flags(&harness).iter().all(|(seen, _)| *seen));
    harness.keys("u");
    assert!(flags(&harness).iter().all(|(seen, _)| !seen));
    // Stars likewise, and the selection is still there to go on with.
    harness.keys("s");
    assert!(flags(&harness).iter().all(|(_, starred)| *starred));
    harness.keys("s");
    assert!(flags(&harness).iter().all(|(_, starred)| !starred));
    assert_eq!(harness.state.list.marked_ids(), ids);
    // The store agrees.
    for id in &ids {
        let location = harness.store.message_location(*id).unwrap().unwrap();
        assert!(!location.flags.seen && !location.flags.flagged);
    }
}

#[test]
fn moving_offers_the_first_messages_folders_and_says_what_stayed() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    // Two of Work's and one of Personal's.
    assert_eq!(all[2], "Sunday dinner");
    harness.keys("x x x m");
    harness.type_text("arch");
    harness.keys("enter");
    let archived = in_role(&harness, FolderRole::Archive);
    assert!(archived.contains(&all[0]) && archived.contains(&all[1]));
    assert!(!archived.contains(&all[2]));
    assert!(
        status(&harness).contains("Moved 2 of 3 messages. The others stay where they are."),
        "{}",
        status(&harness)
    );
    assert_eq!(subjects(&harness)[0], "Sunday dinner");
}

#[test]
fn from_the_reader_a_command_is_about_the_message_being_read() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("x x enter e");
    // Only the one that was open went; the two selected are still there.
    assert_eq!(subjects(&harness).len(), all.len() - 1);
    assert!(subjects(&harness).contains(&all[0]));
    assert!(subjects(&harness).contains(&all[1]));
    assert!(!subjects(&harness).contains(&all[2]));
}

#[test]
fn a_selected_message_that_leaves_the_list_leaves_the_selection() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("x x");
    assert_eq!(harness.state.list.marked_ids().len(), 2);
    // Open the first, which is selected, and archive it from the reader.
    harness.keys("g g enter e");
    assert_eq!(harness.state.list.marked_ids().len(), 1);
    assert!(harness.screen().contains("1 selected"));
}

#[test]
fn another_mailbox_starts_with_nothing_selected() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("x x 1 j enter");
    assert!(!harness.state.list.has_marks());
}
