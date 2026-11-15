//! The outbound queue, the outbox and contacts, through the store's public
//! face: what the UI and the sync engine see.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use quill_core::types::{
    Address, FlagChange, Flags, FolderId, FolderRole, Headers, LocalAccountId, LocalFolderId,
    LocalMessageId, RemoteFolder, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_store::{
    DraftAttachment, DraftContent, DraftState, Mailbox, QueuedKind, Store, is_placeholder,
};

struct Rig {
    store: Store,
    account: LocalAccountId,
    inbox: LocalFolderId,
    archive: LocalFolderId,
    trash: LocalFolderId,
    sent: LocalFolderId,
}

fn message(id: &str, from: (&str, &str), to: &[(&str, &str)], date: i64) -> RemoteMessage {
    RemoteMessage {
        id: RemoteId(id.to_owned()),
        headers: Headers {
            message_id: Some(format!("<{id}@example.com>")),
            subject: format!("Message {id}"),
            from: vec![Address::new(from.0, from.1)],
            to: to
                .iter()
                .map(|(name, email)| Address::new(name, email))
                .collect(),
            cc: Vec::new(),
            date,
        },
        flags: Flags::default(),
        size: 100,
        has_attachments: false,
    }
}

fn delta(added: Vec<RemoteMessage>, removed: &[&str]) -> SyncDelta {
    SyncDelta {
        reset: false,
        added,
        changed: Vec::new(),
        removed: removed
            .iter()
            .map(|id| RemoteId((*id).to_owned()))
            .collect(),
        has_more: false,
        next_cursor: SyncCursor("c".to_owned()),
    }
}

const ANA: (&str, &str) = ("Ana Ruiz", "ana.ruiz@example.com");
const ME: [(&str, &str); 1] = [("Robin Vale", "robin@example.com")];

impl Rig {
    /// One account with an inbox of three messages, and empty Archive,
    /// Trash and Sent.
    fn new() -> Self {
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
        let inbox = folder("INBOX", FolderRole::Inbox);
        let archive = folder("Archive", FolderRole::Archive);
        let trash = folder("Trash", FolderRole::Trash);
        let sent = folder("Sent", FolderRole::Sent);
        store
            .apply_delta(
                inbox,
                &delta(
                    vec![
                        message("1", ANA, &ME, 100),
                        message("2", ANA, &ME, 200),
                        message("3", ("Sam Lee", "sam.lee@example.com"), &ME, 300),
                    ],
                    &[],
                ),
            )
            .unwrap();
        Self {
            store,
            account,
            inbox,
            archive,
            trash,
            sent,
        }
    }

    fn id(&self, folder: LocalFolderId, remote: &str) -> LocalMessageId {
        self.store
            .local_message_id(folder, &RemoteId(remote.to_owned()))
            .unwrap()
            .unwrap_or_else(|| panic!("no message {remote}"))
    }

    fn subjects(&self, folder: LocalFolderId) -> Vec<String> {
        self.store
            .messages(Mailbox::Folder(folder))
            .unwrap()
            .into_iter()
            .map(|row| row.headers.subject)
            .collect()
    }

    fn remote(&self, message: LocalMessageId) -> String {
        self.store
            .message_location(message)
            .unwrap()
            .unwrap()
            .remote
            .0
    }

    fn flags(&self, message: LocalMessageId) -> Flags {
        self.store.message_location(message).unwrap().unwrap().flags
    }
}

fn remote(id: &str) -> RemoteId {
    RemoteId(id.to_owned())
}

#[test]
fn a_move_shows_at_once_and_reaches_the_server_in_its_turn() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    rig.store.put_body(two, "the body").unwrap();
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();

    // Here and now: out of the inbox, in the archive, body and all.
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
    assert_eq!(rig.subjects(rig.archive), ["Message 2"]);
    assert_eq!(rig.store.body(two).unwrap().as_deref(), Some("the body"));
    assert!(is_placeholder(&rig.remote(two)));
    assert_eq!(rig.store.queued_operations().unwrap(), 1);

    // Not before its time: the user may still undo it.
    assert_eq!(
        rig.store.next_operation_time(rig.account).unwrap(),
        Some(1010)
    );
    assert_eq!(rig.store.claim_operation(rig.account, 1005).unwrap(), None);
    let claimed = rig
        .store
        .claim_operation(rig.account, 1010)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, operation);
    assert_eq!(
        claimed.kind,
        QueuedKind::Move {
            remote: remote("2"),
            source: FolderId("INBOX".to_owned()),
            target: FolderId("Archive".to_owned()),
            source_local: rig.inbox,
            target_local: rig.archive,
        }
    );

    // The server says what it calls the message now.
    rig.store
        .complete_operation(operation, Some(&remote("77")))
        .unwrap();
    assert_eq!(rig.remote(two), "77");
    assert_eq!(rig.store.body(two).unwrap().as_deref(), Some("the body"));
    assert_eq!(rig.store.queued_operations().unwrap(), 0);

    // The next sync of each folder agrees and changes nothing.
    rig.store
        .apply_delta(rig.inbox, &delta(Vec::new(), &["2"]))
        .unwrap();
    rig.store
        .apply_delta(rig.archive, &delta(vec![message("77", ANA, &[], 200)], &[]))
        .unwrap();
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
    assert_eq!(
        rig.store
            .messages(Mailbox::Folder(rig.archive))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(rig.id(rig.archive, "77"), two);
}

#[test]
fn a_move_can_be_undone_until_the_engine_takes_it() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();
    assert_eq!(rig.store.undoable(1005).unwrap(), Some(operation));
    // Past its window there is nothing to offer.
    assert_eq!(rig.store.undoable(1010).unwrap(), None);

    assert!(rig.store.undo_operation(operation).unwrap());
    assert_eq!(
        rig.subjects(rig.inbox),
        ["Message 3", "Message 2", "Message 1"]
    );
    assert!(rig.subjects(rig.archive).is_empty());
    assert_eq!(rig.remote(two), "2");
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
    assert!(!rig.store.undo_operation(operation).unwrap());

    // Once taken, it is the server's: too late.
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();
    rig.store
        .claim_operation(rig.account, 1010)
        .unwrap()
        .unwrap();
    assert!(!rig.store.undo_operation(operation).unwrap());
    assert_eq!(rig.subjects(rig.archive), ["Message 2"]);
}

#[test]
fn a_move_the_server_refuses_is_taken_back() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    rig.store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    rig.store.fail_operation(operation).unwrap();
    assert_eq!(
        rig.subjects(rig.inbox),
        ["Message 3", "Message 2", "Message 1"]
    );
    assert_eq!(rig.remote(two), "2");
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
}

#[test]
fn a_move_that_does_not_learn_the_new_id_lets_the_sync_bring_the_message_back() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    rig.store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    rig.store.complete_operation(operation, None).unwrap();
    // The local copy went rather than risk showing the message twice.
    assert!(rig.subjects(rig.archive).is_empty());
    rig.store
        .apply_delta(rig.archive, &delta(vec![message("90", ANA, &[], 200)], &[]))
        .unwrap();
    assert_eq!(rig.subjects(rig.archive), ["Message 90"]);
}

#[test]
fn a_sync_before_the_server_hears_of_a_move_does_not_bring_the_message_back() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();

    // The server still lists it in the inbox: an ordinary pass, and then a
    // pass after the server renumbered both folders.
    rig.store
        .apply_delta(rig.inbox, &delta(vec![message("2", ANA, &ME, 200)], &[]))
        .unwrap();
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
    let mut renumbered = delta(vec![message("2", ANA, &ME, 200)], &[]);
    renumbered.reset = true;
    rig.store.apply_delta(rig.inbox, &renumbered).unwrap();
    assert_eq!(rig.subjects(rig.inbox), Vec::<String>::new());
    let mut emptied = delta(Vec::new(), &[]);
    emptied.reset = true;
    rig.store.apply_delta(rig.archive, &emptied).unwrap();
    // The moved message is still where the user put it.
    assert_eq!(rig.subjects(rig.archive), ["Message 2"]);

    // Undone, it goes back, and the inbox has it once.
    assert!(rig.store.undo_operation(operation).unwrap());
    assert_eq!(rig.subjects(rig.inbox), ["Message 2"]);
}

#[test]
fn a_failed_move_takes_back_the_later_moves_built_on_it() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    rig.store.put_body(two, "the body").unwrap();
    let first = rig
        .store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();
    let second = rig
        .store
        .queue_move(two, rig.trash, 1001, 1011)
        .unwrap()
        .unwrap();
    assert_eq!(rig.subjects(rig.trash), ["Message 2"]);
    // The newest change is the one on offer to undo.
    assert_eq!(rig.store.undoable(1005).unwrap(), Some(second));

    rig.store
        .claim_operation(rig.account, 1010)
        .unwrap()
        .unwrap();
    rig.store.fail_operation(first).unwrap();
    // The first move failed, so the second, built on it, is undone too.
    assert_eq!(
        rig.subjects(rig.inbox),
        ["Message 3", "Message 2", "Message 1"]
    );
    assert!(rig.subjects(rig.archive).is_empty() && rig.subjects(rig.trash).is_empty());
    assert_eq!(rig.remote(two), "2");
    assert_eq!(rig.store.body(two).unwrap().as_deref(), Some("the body"));
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
}

#[test]
fn two_moves_of_one_message_run_in_order_with_the_second_following_the_first() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let first = rig
        .store
        .queue_move(two, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    let second = rig
        .store
        .queue_move(two, rig.trash, 1000, 1000)
        .unwrap()
        .unwrap();

    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, first);
    rig.store
        .complete_operation(first, Some(&remote("50")))
        .unwrap();
    // The message has moved on, so it keeps the later placeholder; the
    // second move now knows what to ask the server to move.
    assert!(is_placeholder(&rig.remote(two)));
    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, second);
    assert_eq!(
        claimed.kind,
        QueuedKind::Move {
            remote: remote("50"),
            source: FolderId("Archive".to_owned()),
            target: FolderId("Trash".to_owned()),
            source_local: rig.archive,
            target_local: rig.trash,
        }
    );
    rig.store
        .complete_operation(second, Some(&remote("9")))
        .unwrap();
    assert_eq!(rig.remote(two), "9");
    assert_eq!(rig.subjects(rig.trash), ["Message 2"]);

    // When the first cannot say where the message went, the second has
    // nothing to ask for and is simply finished.
    let three = rig.id(rig.inbox, "3");
    let first = rig
        .store
        .queue_move(three, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    let second = rig
        .store
        .queue_move(three, rig.trash, 1000, 1000)
        .unwrap()
        .unwrap();
    rig.store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    rig.store.complete_operation(first, None).unwrap();
    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!((claimed.id, &claimed.kind), (second, &QueuedKind::Nothing));
    rig.store.complete_operation(second, None).unwrap();
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
}

#[test]
fn a_move_that_cannot_happen_is_not_queued() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    // Into the folder it is in, into a folder that is not there, into
    // another account's folder, and of a message that is not there.
    assert_eq!(rig.store.queue_move(two, rig.inbox, 0, 0).unwrap(), None);
    assert_eq!(
        rig.store.queue_move(two, LocalFolderId(999), 0, 0).unwrap(),
        None
    );
    let other = rig
        .store
        .upsert_account("Home", "robin@home.example.com", "imap")
        .unwrap();
    let elsewhere = rig
        .store
        .upsert_folder(
            other,
            &RemoteFolder {
                id: FolderId("INBOX".to_owned()),
                name: "Inbox".to_owned(),
                role: FolderRole::Inbox,
            },
        )
        .unwrap();
    assert_eq!(rig.store.queue_move(two, elsewhere, 0, 0).unwrap(), None);
    assert_eq!(
        rig.store
            .queue_move(LocalMessageId(999), rig.archive, 0, 0)
            .unwrap(),
        None
    );
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
    assert_eq!(rig.subjects(rig.inbox).len(), 3);
}

#[test]
fn a_delete_hides_at_once_and_removes_when_the_server_confirms() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let counts = |rig: &Rig| {
        let folders = rig.store.folders().unwrap();
        let inbox = folders
            .iter()
            .find(|folder| folder.id == rig.inbox)
            .unwrap();
        (inbox.unread, inbox.total)
    };
    assert_eq!(counts(&rig), (3, 3));

    let operation = rig.store.queue_delete(two, 1000, 1010).unwrap().unwrap();
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
    assert_eq!(counts(&rig), (2, 2));
    assert!(
        rig.store
            .messages(Mailbox::Unified)
            .unwrap()
            .iter()
            .all(|row| row.id != two)
    );
    // Deleting what is already deleted queues nothing more.
    assert_eq!(rig.store.queue_delete(two, 1000, 1010).unwrap(), None);

    // Undone, it is back.
    assert!(rig.store.undo_operation(operation).unwrap());
    assert_eq!(rig.subjects(rig.inbox).len(), 3);

    // Done, it is gone for good; a sync that still lists it meanwhile does
    // not unhide it.
    let operation = rig.store.queue_delete(two, 1000, 1000).unwrap().unwrap();
    rig.store
        .apply_delta(rig.inbox, &delta(vec![message("2", ANA, &[], 200)], &[]))
        .unwrap();
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.kind,
        QueuedKind::Delete {
            remote: remote("2"),
            source: FolderId("INBOX".to_owned()),
        }
    );
    rig.store.complete_operation(operation, None).unwrap();
    assert_eq!(
        rig.store.local_message_id(rig.inbox, &remote("2")).unwrap(),
        None
    );

    // Refused, it comes back.
    let three = rig.id(rig.inbox, "3");
    let operation = rig.store.queue_delete(three, 1000, 1000).unwrap().unwrap();
    rig.store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    rig.store.fail_operation(operation).unwrap();
    assert_eq!(rig.subjects(rig.inbox), ["Message 3", "Message 1"]);
}

const SEEN: Flags = Flags {
    seen: true,
    flagged: false,
    answered: false,
    draft: false,
};

#[test]
fn flag_changes_are_queued_merged_and_survive_a_sync() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let seen_and_starred = Flags {
        flagged: true,
        ..SEEN
    };
    assert!(rig.store.queue_flags(two, SEEN, 1000).unwrap());
    // The same again is no change.
    assert!(!rig.store.queue_flags(two, SEEN, 1000).unwrap());
    assert!(rig.store.queue_flags(two, seen_and_starred, 1001).unwrap());
    // Two changes, one thing to tell the server: the latest.
    assert_eq!(rig.store.queued_operations().unwrap(), 1);

    // A sync that still carries the server's old flags does not undo them.
    let mut pass = delta(Vec::new(), &[]);
    pass.changed = vec![FlagChange {
        id: remote("2"),
        flags: Flags::default(),
    }];
    rig.store.apply_delta(rig.inbox, &pass).unwrap();
    assert_eq!(rig.flags(two), seen_and_starred);

    let claimed = rig
        .store
        .claim_operation(rig.account, 1001)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.kind,
        QueuedKind::Flags {
            remote: remote("2"),
            flags: seen_and_starred,
        }
    );
    // Refused: back to what the flags were before either change.
    rig.store.fail_operation(claimed.id).unwrap();
    assert_eq!(rig.flags(two), Flags::default());

    // A change to a message that is not there is nothing.
    assert!(
        !rig.store
            .queue_flags(LocalMessageId(999), SEEN, 1000)
            .unwrap()
    );
}

#[test]
fn a_flag_change_made_during_a_move_uses_the_messages_new_id() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let moved = rig
        .store
        .queue_move(two, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    rig.store.queue_flags(two, SEEN, 1000).unwrap();

    // Strictly in order: the move first.
    assert_eq!(
        rig.store
            .claim_operation(rig.account, 1000)
            .unwrap()
            .unwrap()
            .id,
        moved
    );
    rig.store
        .complete_operation(moved, Some(&remote("61")))
        .unwrap();
    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.kind,
        QueuedKind::Flags {
            remote: remote("61"),
            flags: SEEN,
        }
    );
}

#[test]
fn an_operation_waits_its_turn_behind_one_that_is_not_due() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let three = rig.id(rig.inbox, "3");
    rig.store
        .queue_move(two, rig.archive, 1000, 1010)
        .unwrap()
        .unwrap();
    rig.store.queue_flags(three, SEEN, 1001).unwrap();
    // The flag change is due, but the move ahead of it is not.
    assert_eq!(rig.store.claim_operation(rig.account, 1005).unwrap(), None);
    assert!(
        rig.store
            .claim_operation(rig.account, 1010)
            .unwrap()
            .is_some()
    );
}

#[test]
fn an_operation_interrupted_or_put_off_is_given_again() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    let operation = rig
        .store
        .queue_move(two, rig.archive, 1000, 1000)
        .unwrap()
        .unwrap();
    let first = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    // Quill stopped before finishing. On the next run it is taken again.
    let again = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(first, again);

    // No connection: it keeps its place and is tried later.
    rig.store.retry_operation(operation, 1060).unwrap();
    assert_eq!(rig.store.claim_operation(rig.account, 1030).unwrap(), None);
    let later = rig
        .store
        .claim_operation(rig.account, 1060)
        .unwrap()
        .unwrap();
    assert_eq!((later.id, later.attempts), (operation, 1));
    // Waiting again, it can be undone again.
    rig.store.retry_operation(operation, 2000).unwrap();
    assert!(rig.store.undo_operation(operation).unwrap());
}

#[test]
fn a_sent_copy_is_queued_for_its_folder() {
    let mut rig = Rig::new();
    let operation = rig
        .store
        .queue_append(rig.sent, &"ab".repeat(32), SEEN, 1000)
        .unwrap()
        .unwrap();
    let claimed = rig
        .store
        .claim_operation(rig.account, 1000)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.kind,
        QueuedKind::Append {
            target: FolderId("Sent".to_owned()),
            target_local: rig.sent,
            blob_hash: "ab".repeat(32),
            flags: SEEN,
        }
    );
    rig.store.complete_operation(operation, None).unwrap();
    assert_eq!(
        rig.store
            .queue_append(LocalFolderId(999), "x", SEEN, 0)
            .unwrap(),
        None
    );
}

#[test]
fn operations_go_with_their_account_and_folder() {
    let mut rig = Rig::new();
    let two = rig.id(rig.inbox, "2");
    rig.store.queue_move(two, rig.archive, 1000, 1000).unwrap();
    // The archive folder is deleted on the server and so here.
    rig.store
        .retain_folders(
            rig.account,
            &[FolderId("INBOX".to_owned()), FolderId("Trash".to_owned())],
        )
        .unwrap();
    assert_eq!(rig.store.queued_operations().unwrap(), 0);

    let three = rig.id(rig.inbox, "3");
    rig.store.queue_delete(three, 1000, 1000).unwrap();
    rig.store.retain_accounts(&[]).unwrap();
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
}

#[test]
fn the_queue_survives_closing_and_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quill.db");
    let operation = {
        let mut store = Store::open(&path).unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let folder = |store: &mut Store, name: &str, role| {
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
        let inbox = folder(&mut store, "INBOX", FolderRole::Inbox);
        let archive = folder(&mut store, "Archive", FolderRole::Archive);
        store
            .apply_delta(inbox, &delta(vec![message("1", ANA, &[], 1)], &[]))
            .unwrap();
        let one = store
            .local_message_id(inbox, &remote("1"))
            .unwrap()
            .unwrap();
        store.queue_move(one, archive, 1000, 1000).unwrap().unwrap()
    };
    let mut store = Store::open(&path).unwrap();
    let account = store.accounts().unwrap()[0].id;
    let claimed = store.claim_operation(account, 1000).unwrap().unwrap();
    assert_eq!(claimed.id, operation);
    assert!(matches!(claimed.kind, QueuedKind::Move { .. }));
}

fn content(to: &str, subject: &str) -> DraftContent {
    DraftContent {
        to: to.to_owned(),
        subject: subject.to_owned(),
        body: "Hello.".to_owned(),
        ..DraftContent::default()
    }
}

fn recipients() -> Vec<String> {
    vec!["ana.ruiz@example.com".to_owned()]
}

#[test]
fn a_draft_is_saved_changed_listed_and_thrown_away() {
    let mut rig = Rig::new();
    let first = rig
        .store
        .save_draft(
            None,
            rig.account,
            &content("ana.ruiz@example.com", "One"),
            100,
        )
        .unwrap()
        .unwrap();
    let second = rig
        .store
        .save_draft(None, rig.account, &content("sam", "Two"), 200)
        .unwrap()
        .unwrap();
    let mut changed = content("ana.ruiz@example.com", "One, revised");
    changed.attachments.push(DraftAttachment {
        name: "notes.pdf".to_owned(),
        content_type: "application/pdf".to_owned(),
        blob_hash: "cd".repeat(32),
        size: 2048,
    });
    changed.in_reply_to = Some("<1@example.com>".to_owned());
    changed.replied_to = Some(7);
    assert_eq!(
        rig.store
            .save_draft(Some(first), rig.account, &changed, 300)
            .unwrap(),
        Some(first)
    );

    let drafts = rig.store.drafts().unwrap();
    // Newest first.
    assert_eq!(
        drafts.iter().map(|draft| draft.id).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(drafts[0].content, changed);
    assert_eq!(drafts[0].state, DraftState::Draft);
    assert!(rig.store.outbox().unwrap().is_empty());

    assert!(rig.store.delete_draft(second).unwrap());
    assert!(!rig.store.delete_draft(second).unwrap());
    assert_eq!(rig.store.draft(second).unwrap(), None);
    // Saving what is gone saves nothing.
    assert_eq!(
        rig.store
            .save_draft(Some(second), rig.account, &changed, 400)
            .unwrap(),
        None
    );
}

#[test]
fn a_message_goes_through_the_outbox_once() {
    let mut rig = Rig::new();
    let mut reply = content("ana.ruiz@example.com", "Re: Message 2");
    reply.replied_to = Some(rig.id(rig.inbox, "2").0);
    let draft = rig
        .store
        .save_draft(None, rig.account, &reply, 100)
        .unwrap()
        .unwrap();
    let to = recipients();
    assert!(
        rig.store
            .queue_send(
                draft,
                &"ee".repeat(32),
                "robin@example.com",
                &to,
                1010,
                1000
            )
            .unwrap()
    );

    // In the outbox, not among the drafts, and not to be edited there.
    assert!(rig.store.drafts().unwrap().is_empty());
    let outbox = rig.store.outbox().unwrap();
    assert_eq!(
        (outbox.len(), outbox[0].state, outbox[0].send_after),
        (1, DraftState::Outbox, Some(1010))
    );
    assert_eq!(
        rig.store
            .save_draft(Some(draft), rig.account, &reply, 1001)
            .unwrap(),
        None
    );

    // Not before its time.
    assert_eq!(rig.store.next_send_time(rig.account).unwrap(), Some(1010));
    assert_eq!(rig.store.claim_send(rig.account, 1005).unwrap(), None);
    let outgoing = rig.store.claim_send(rig.account, 1010).unwrap().unwrap();
    assert_eq!(outgoing.raw_hash, "ee".repeat(32));
    assert_eq!(
        (outgoing.from.as_str(), &outgoing.to),
        ("robin@example.com", &to)
    );
    assert_eq!(outgoing.replied_to, reply.replied_to);
    // While a server has it: not given out twice, not taken back, not
    // thrown away.
    assert_eq!(rig.store.claim_send(rig.account, 1010).unwrap(), None);
    assert!(!rig.store.unsend(draft, 1010).unwrap());
    assert!(!rig.store.delete_draft(draft).unwrap());

    rig.store.sent(draft).unwrap();
    assert!(rig.store.outbox().unwrap().is_empty());
    assert_eq!(rig.store.next_send_time(rig.account).unwrap(), None);
}

#[test]
fn undo_send_takes_a_waiting_message_back_to_a_draft() {
    let mut rig = Rig::new();
    let draft = rig
        .store
        .save_draft(
            None,
            rig.account,
            &content("ana.ruiz@example.com", "Oops"),
            100,
        )
        .unwrap()
        .unwrap();
    rig.store
        .queue_send(
            draft,
            &"ee".repeat(32),
            "robin@example.com",
            &recipients(),
            1010,
            1000,
        )
        .unwrap();
    assert_eq!(rig.store.unsendable(1005).unwrap(), Some(draft));
    assert_eq!(rig.store.unsendable(1010).unwrap(), None);

    assert!(rig.store.unsend(draft, 1005).unwrap());
    assert_eq!(rig.store.drafts().unwrap()[0].state, DraftState::Draft);
    assert_eq!(rig.store.claim_send(rig.account, 2000).unwrap(), None);
    assert!(!rig.store.unsend(draft, 1005).unwrap());
}

#[test]
fn a_send_that_fails_waits_to_retry_or_waits_for_the_user() {
    let mut rig = Rig::new();
    let draft = rig
        .store
        .save_draft(
            None,
            rig.account,
            &content("ana.ruiz@example.com", "Hi"),
            100,
        )
        .unwrap()
        .unwrap();
    let to = recipients();
    rig.store
        .queue_send(
            draft,
            &"ee".repeat(32),
            "robin@example.com",
            &to,
            1000,
            1000,
        )
        .unwrap();

    // The server is busy: it stays in the outbox for later.
    rig.store.claim_send(rig.account, 1000).unwrap().unwrap();
    rig.store
        .send_failed(draft, "451 try again later", Some(1060))
        .unwrap();
    let waiting = &rig.store.outbox().unwrap()[0];
    assert_eq!(
        (waiting.state, waiting.error.as_deref()),
        (DraftState::Outbox, Some("451 try again later"))
    );
    assert_eq!(rig.store.claim_send(rig.account, 1030).unwrap(), None);
    let again = rig.store.claim_send(rig.account, 1060).unwrap().unwrap();
    assert_eq!(again.attempts, 1);

    // The server refuses it for good: it waits, with the reason.
    rig.store
        .send_failed(draft, "550 no such user", None)
        .unwrap();
    let refused = &rig.store.outbox().unwrap()[0];
    assert_eq!(
        (refused.state, refused.error.as_deref()),
        (DraftState::Failed, Some("550 no such user"))
    );
    assert_eq!(rig.store.claim_send(rig.account, 9999).unwrap(), None);

    // Corrected and saved, it is a draft again and can be sent again.
    let corrected = content("ana.ruiz@example.com", "Hi again");
    assert_eq!(
        rig.store
            .save_draft(Some(draft), rig.account, &corrected, 2000)
            .unwrap(),
        Some(draft)
    );
    let fixed = rig.store.draft(draft).unwrap().unwrap();
    assert_eq!((fixed.state, fixed.error), (DraftState::Draft, None));
    assert!(
        rig.store
            .queue_send(
                draft,
                &"ff".repeat(32),
                "robin@example.com",
                &to,
                2000,
                2000
            )
            .unwrap()
    );
}

#[test]
fn a_send_interrupted_by_a_crash_is_never_sent_again_by_itself() {
    let mut rig = Rig::new();
    let draft = rig
        .store
        .save_draft(
            None,
            rig.account,
            &content("ana.ruiz@example.com", "Hi"),
            100,
        )
        .unwrap()
        .unwrap();
    rig.store
        .queue_send(
            draft,
            &"ee".repeat(32),
            "robin@example.com",
            &recipients(),
            1000,
            1000,
        )
        .unwrap();
    rig.store.claim_send(rig.account, 1000).unwrap().unwrap();

    // Quill stopped here. Whether the server took it is not known.
    assert_eq!(rig.store.recover_interrupted_sends().unwrap(), 1);
    assert_eq!(rig.store.claim_send(rig.account, 9999).unwrap(), None);
    let kept = &rig.store.outbox().unwrap()[0];
    assert_eq!(kept.state, DraftState::Failed);
    assert!(kept.error.as_deref().unwrap().contains("look in Sent"));
    assert_eq!(rig.store.recover_interrupted_sends().unwrap(), 0);
}

#[test]
fn people_written_to_come_before_people_who_only_write() {
    let mut rig = Rig::new();
    // The inbox so far: Ana twice, Sam once. Now mail sent by the user, to
    // Priya four times.
    let me = ("Robin Vale", "robin@example.com");
    let sent: Vec<RemoteMessage> = (0..4)
        .map(|n| {
            message(
                &format!("s{n}"),
                me,
                &[
                    ("", "priya.nair@example.com"),
                    ("Sam Lee", "sam.lee@example.com"),
                ],
                400 + n,
            )
        })
        .collect();
    rig.store.apply_delta(rig.sent, &delta(sent, &[])).unwrap();
    let emails = |typed: &str| -> Vec<String> {
        rig.store
            .contacts_matching(typed, 10)
            .unwrap()
            .into_iter()
            .map(|contact| contact.email)
            .collect()
    };
    assert_eq!(emails("a"), ["ana.ruiz@example.com"]);
    // A word of the name, the start of the address, a later part of it.
    assert_eq!(emails("lee"), ["sam.lee@example.com"]);
    assert_eq!(emails("RUIZ"), ["ana.ruiz@example.com"]);
    assert_eq!(emails("nair"), ["priya.nair@example.com"]);
    assert_eq!(emails(""), Vec::<String>::new());
    // The middle of a word is not a match.
    assert_eq!(emails("xample"), Vec::<String>::new());
    // What was typed is matched as itself, not as a pattern.
    assert_eq!(emails("%"), Vec::<String>::new());
    assert_eq!(emails("_"), Vec::<String>::new());

    // Everyone at example.com: the two written to come first, though Ana
    // has written more often than Sam.
    let everyone = emails("example");
    assert_eq!(
        everyone,
        [
            "sam.lee@example.com",
            "priya.nair@example.com",
            "ana.ruiz@example.com"
        ]
    );
    // The user is not their own contact: only senders of received mail and
    // recipients of sent mail are kept.
    assert!(emails("robin").is_empty());
    assert!(rig.store.contacts_matching("s", 0).unwrap().is_empty());
    assert_eq!(rig.store.contacts_matching("example", 2).unwrap().len(), 2);
}

#[test]
fn a_contact_is_one_person_and_keeps_a_name() {
    let mut rig = Rig::new();
    rig.store
        .apply_delta(
            rig.inbox,
            &delta(
                vec![
                    message("10", ("", "Ana.Ruiz@Example.com"), &ME, 900),
                    message("11", ("Ana R. Ruiz", "ana.ruiz@example.com"), &ME, 50),
                ],
                &[],
            ),
        )
        .unwrap();
    let found = rig.store.contacts_matching("ana", 5).unwrap();
    // One person, whatever the case of the address. The newer message had
    // no name and does not wipe it; the older one's name is not newer than
    // the one already held.
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Ana Ruiz");
}
