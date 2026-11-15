//! Conversations: which messages belong together.
//!
//! A reply names the messages before it (`References`, `In-Reply-To`).
//! Every Message-ID seen is filed under a thread, and a message joins the
//! thread of any id it names. One that names ids of two threads, the late
//! reply that connects two halves of a conversation, makes them one.
//!
//! Threads are found from what messages say about themselves and nothing
//! else: not from subjects, which strangers share.

use quill_core::types::LocalMessageId;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Result, Store};

/// The most ids of one message that are filed. Enough for any real
/// conversation; a message naming thousands is not describing one.
const MAX_IDS: usize = 64;

/// Files `message` under the thread its ids belong to, making or joining
/// threads as needed. `own` is its Message-ID and `follows` the ids of the
/// messages before it. Returns the thread.
pub(crate) fn assign(
    tx: &Transaction<'_>,
    message: i64,
    own: Option<&str>,
    follows: &[String],
) -> Result<i64> {
    let ids: Vec<&str> = own
        .into_iter()
        .chain(follows.iter().map(String::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .take(MAX_IDS)
        .collect();

    let mut threads: Vec<i64> = Vec::new();
    {
        let mut find = tx.prepare_cached("SELECT thread FROM thread_ids WHERE message_id = ?1")?;
        for id in &ids {
            if let Some(thread) = find.query_row([id], |row| row.get(0)).optional()?
                && !threads.contains(&thread)
            {
                threads.push(thread);
            }
        }
    }
    // A new thread is named after the message that starts it: no other
    // thread can have that name, since no other message has that id.
    let thread = threads.first().copied().unwrap_or(message);
    for other in threads.iter().skip(1) {
        tx.execute(
            "UPDATE messages SET thread_id = ?1 WHERE thread_id = ?2",
            params![thread, other],
        )?;
        tx.execute(
            "UPDATE thread_ids SET thread = ?1 WHERE thread = ?2",
            params![thread, other],
        )?;
    }
    {
        let mut file = tx.prepare_cached(
            "INSERT INTO thread_ids (message_id, thread) VALUES (?1, ?2)
             ON CONFLICT (message_id) DO UPDATE SET thread = excluded.thread",
        )?;
        for id in &ids {
            file.execute(params![id, thread])?;
        }
    }
    tx.execute(
        "UPDATE messages SET thread_id = ?2 WHERE id = ?1",
        params![message, thread],
    )?;
    Ok(thread)
}

impl Store {
    /// Files a message under its thread, from the ids read out of its
    /// downloaded copy. For mail that was listed before threads were
    /// kept, or by a backend that could not say what a message follows.
    /// Returns whether the message exists.
    pub fn thread_message(
        &mut self,
        message: LocalMessageId,
        own: Option<&str>,
        follows: &[String],
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM messages WHERE id = ?1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(false);
        }
        assign(&tx, message.0, own, follows)?;
        tx.commit()?;
        Ok(true)
    }

    /// Downloaded messages that have no thread yet, oldest first, so that
    /// a conversation is put together in the order it happened.
    pub fn unthreaded(&self, limit: usize) -> Result<Vec<LocalMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT id FROM messages
             WHERE thread_id IS NULL AND blob_hash IS NOT NULL
             ORDER BY date, id LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(LocalMessageId(row.get(0)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use quill_core::types::{
        Flags, FolderId, FolderRole, Headers, LocalFolderId, RemoteFolder, RemoteId, RemoteMessage,
        SyncCursor, SyncDelta,
    };

    use super::*;
    use crate::Mailbox;

    fn store() -> (Store, LocalFolderId) {
        let mut store = Store::open_in_memory()
            .unwrap()
            .with_blobs(crate::BlobStore::in_memory());
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let inbox = store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId("INBOX".to_owned()),
                    name: "INBOX".to_owned(),
                    role: FolderRole::Inbox,
                },
            )
            .unwrap();
        (store, inbox)
    }

    /// A message called `name`, with `<name@example.com>` as its id unless
    /// it has none, following the messages named.
    fn message(name: &str, has_id: bool, follows: &[&str]) -> RemoteMessage {
        RemoteMessage {
            id: RemoteId(name.to_owned()),
            headers: Headers {
                message_id: has_id.then(|| format!("<{name}@example.com>")),
                subject: name.to_owned(),
                from: Vec::new(),
                to: Vec::new(),
                cc: Vec::new(),
                date: 100,
            },
            flags: Flags::default(),
            size: 1,
            has_attachments: false,
            thread: follows
                .iter()
                .map(|name| format!("<{name}@example.com>"))
                .collect(),
        }
    }

    fn arrive(store: &mut Store, folder: LocalFolderId, messages: Vec<RemoteMessage>) {
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

    /// The names of the messages, grouped by thread, each group and the
    /// list of groups in alphabetical order.
    fn threads(store: &Store, folder: LocalFolderId) -> Vec<Vec<String>> {
        let mut groups: std::collections::BTreeMap<i64, Vec<String>> = Default::default();
        for row in store.messages(Mailbox::Folder(folder)).unwrap() {
            groups
                .entry(row.thread)
                .or_default()
                .push(row.headers.subject);
        }
        let mut groups: Vec<Vec<String>> = groups
            .into_values()
            .map(|mut names| {
                names.sort();
                names
            })
            .collect();
        groups.sort();
        groups
    }

    #[test]
    fn replies_join_the_conversation_they_name() {
        let (mut store, inbox) = store();
        arrive(
            &mut store,
            inbox,
            vec![
                message("a", true, &[]),
                message("b", true, &["a"]),
                message("c", true, &["a", "b"]),
                message("other", true, &[]),
            ],
        );
        assert_eq!(threads(&store, inbox), [vec!["a", "b", "c"], vec!["other"]]);
    }

    #[test]
    fn the_order_mail_arrives_in_does_not_matter() {
        let (mut store, inbox) = store();
        // The reply first, on its own; the original a sync later.
        arrive(&mut store, inbox, vec![message("b", true, &["a"])]);
        arrive(&mut store, inbox, vec![message("a", true, &[])]);
        // And a reply to a message that never arrives at all.
        arrive(&mut store, inbox, vec![message("d", true, &["lost"])]);
        arrive(&mut store, inbox, vec![message("e", true, &["lost"])]);
        assert_eq!(threads(&store, inbox), [vec!["a", "b"], vec!["d", "e"]]);
    }

    #[test]
    fn a_message_that_names_two_conversations_makes_them_one() {
        let (mut store, inbox) = store();
        arrive(
            &mut store,
            inbox,
            vec![
                message("a", true, &[]),
                message("a2", true, &["a"]),
                message("x", true, &[]),
                message("x2", true, &["x"]),
            ],
        );
        assert_eq!(threads(&store, inbox).len(), 2);
        arrive(
            &mut store,
            inbox,
            vec![message("join", true, &["a2", "x2"])],
        );
        assert_eq!(threads(&store, inbox), [vec!["a", "a2", "join", "x", "x2"]]);
        // And what follows either half afterwards lands in the one.
        arrive(&mut store, inbox, vec![message("late", true, &["x"])]);
        assert_eq!(threads(&store, inbox).len(), 1);
    }

    #[test]
    fn mail_that_says_nothing_of_itself_is_a_conversation_each() {
        let (mut store, inbox) = store();
        arrive(
            &mut store,
            inbox,
            vec![message("one", false, &[]), message("two", false, &[])],
        );
        assert_eq!(threads(&store, inbox), [vec!["one"], vec!["two"]]);
        // Listed again, nothing moves.
        arrive(
            &mut store,
            inbox,
            vec![message("one", false, &[]), message("two", false, &[])],
        );
        assert_eq!(threads(&store, inbox), [vec!["one"], vec!["two"]]);
    }

    #[test]
    fn mail_listed_before_conversations_were_kept_is_threaded_from_its_copy() {
        let (mut store, inbox) = store();
        arrive(
            &mut store,
            inbox,
            vec![message("a", true, &[]), message("b", true, &[])],
        );
        // As a database from before would have them.
        store
            .conn
            .execute_batch("UPDATE messages SET thread_id = NULL; DELETE FROM thread_ids;")
            .unwrap();
        assert_eq!(threads(&store, inbox), [vec!["a"], vec!["b"]]);
        // Nothing is waiting until there is a copy to read the ids from.
        assert!(store.unthreaded(10).unwrap().is_empty());
        let ids: Vec<LocalMessageId> = store
            .messages(Mailbox::Folder(inbox))
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        for id in &ids {
            store.put_message(*id, "text", b"raw").unwrap();
        }
        assert_eq!(store.unthreaded(10).unwrap().len(), 2);
        let b = store
            .local_message_id(inbox, &RemoteId("b".to_owned()))
            .unwrap()
            .unwrap();
        let a = store
            .local_message_id(inbox, &RemoteId("a".to_owned()))
            .unwrap()
            .unwrap();
        assert!(
            store
                .thread_message(a, Some("<a@example.com>"), &[])
                .unwrap()
        );
        assert!(
            store
                .thread_message(b, Some("<b@example.com>"), &["<a@example.com>".to_owned()])
                .unwrap()
        );
        assert_eq!(threads(&store, inbox), [vec!["a", "b"]]);
        assert!(store.unthreaded(10).unwrap().is_empty());
        // A message that has gone is not an error.
        assert!(
            !store
                .thread_message(LocalMessageId(9999), None, &[])
                .unwrap()
        );
    }

    #[test]
    fn a_message_naming_a_great_many_ids_files_only_so_many() {
        let (mut store, inbox) = store();
        let names: Vec<String> = (0..500).map(|n| format!("r{n}")).collect();
        let follows: Vec<&str> = names.iter().map(String::as_str).collect();
        arrive(&mut store, inbox, vec![message("flood", true, &follows)]);
        let filed: i64 = store
            .conn
            .query_row("SELECT count(*) FROM thread_ids", [], |row| row.get(0))
            .unwrap();
        assert_eq!(filed, MAX_IDS as i64);
    }
}
