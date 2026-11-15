//! Syncing when the server refuses, stops answering, or holds mail that is
//! marked for deletion.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use async_imap::Session;
use common::{Dovecot, PASSWORD, Without, backend, fill};
use futures_util::TryStreamExt;
use quill_core::config::TlsMode;
use quill_core::types::{Flags, FolderId, Operation, RemoteId, SyncCursor, SyncDelta};
use quill_core::{Backend, Error};
use tokio::net::TcpStream;

fn inbox() -> FolderId {
    FolderId("INBOX".to_owned())
}

/// One sync of the inbox from `cursor`. The folders here fit in one batch.
async fn sync(backend: &dyn Backend, cursor: Option<&SyncCursor>) -> SyncDelta {
    let delta = backend
        .sync_folder(&inbox(), cursor.cloned())
        .await
        .unwrap();
    assert!(!delta.has_more && !delta.reset);
    delta
}

fn subjects(delta: &SyncDelta) -> Vec<&str> {
    let mut subjects: Vec<&str> = delta
        .added
        .iter()
        .map(|message| message.headers.subject.as_str())
        .collect();
    subjects.sort_unstable();
    subjects
}

fn id_of(delta: &SyncDelta, subject: &str) -> RemoteId {
    delta
        .added
        .iter()
        .find(|message| message.headers.subject == subject)
        .unwrap()
        .id
        .clone()
}

/// Changes flags the way another mail program would, with no expunge.
async fn store(session: &mut Session<TcpStream>, uids: &str, change: &str) {
    session.select("INBOX").await.unwrap();
    let _: Vec<_> = session
        .uid_store(uids, change)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
}

#[tokio::test]
async fn mail_marked_deleted_is_not_shown_and_comes_back_if_the_mark_is_taken_off() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "marked@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 3).await;
    let backend = server.backend(TlsMode::Implicit, user);

    let first = sync(&backend, None).await;
    assert_eq!(subjects(&first), ["Message 1", "Message 2", "Message 3"]);
    let two = id_of(&first, "Message 2");
    let raw = backend.fetch_raw(&two).await.unwrap();

    // Another mail program marks a message and does not expunge it.
    store(&mut other_client, "2", "+FLAGS (\\Deleted)").await;
    let marked = sync(&backend, Some(&first.next_cursor)).await;
    assert!(marked.added.is_empty());
    assert_eq!(marked.removed, std::slice::from_ref(&two));
    assert_eq!(marked.changed.len(), 2);
    assert!(marked.changed.iter().all(|change| change.id != two));
    // Nothing new to say about it on the next sync.
    let again = sync(&backend, Some(&marked.next_cursor)).await;
    assert!(again.added.is_empty() && again.removed.is_empty());
    assert_eq!(again.changed.len(), 2);

    // Someone syncing the folder for the first time never sees it at all,
    // nor a message that arrives already marked.
    other_client
        .append(
            "INBOX",
            Some("(\\Deleted)"),
            None,
            common::message(
                "Ana Ruiz <ana.ruiz@example.com>",
                "Marked on arrival",
                "Sat, 14 Nov 2026 10:00:00 +0000",
                "Never shown.",
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let fresh = sync(&server.backend(TlsMode::Implicit, user), None).await;
    assert_eq!(subjects(&fresh), ["Message 1", "Message 3"]);
    let still = sync(&backend, Some(&again.next_cursor)).await;
    assert!(still.added.is_empty() && still.removed.is_empty());

    // The message itself was never touched: it is on the server, under the
    // id it always had.
    assert_eq!(backend.fetch_raw(&two).await.unwrap(), raw);

    // The mark is taken off, and the message is back as new mail with its
    // old id. It is reported once.
    store(&mut other_client, "2", "-FLAGS (\\Deleted)").await;
    let back = sync(&backend, Some(&still.next_cursor)).await;
    assert_eq!(subjects(&back), ["Message 2"]);
    assert_eq!(back.added[0].id, two);
    assert!(back.removed.is_empty());
    assert_eq!(back.changed.len(), 2);
    let settled = sync(&backend, Some(&back.next_cursor)).await;
    assert!(settled.added.is_empty() && settled.removed.is_empty());
    assert_eq!(settled.changed.len(), 3);

    // Expunging the one that arrived marked removes something the cursor
    // never held, so there is nothing to report.
    let _: Vec<_> = other_client
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let after = sync(&backend, Some(&settled.next_cursor)).await;
    assert!(after.added.is_empty() && after.removed.is_empty());
    assert_eq!(after.changed.len(), 3);
}

/// A backend for `user` through a relay that can be told to misbehave.
async fn through_relay(server: &Dovecot, user: &str) -> (quill_imap::ImapBackend, Without) {
    let relay = Without::start(server.imap_port, &[]).await;
    let mut config = server.config(TlsMode::None, user);
    config.port = relay.port;
    (backend(config, PASSWORD), relay)
}

fn refused(result: Result<impl std::fmt::Debug, Error>) {
    match result {
        Err(Error::Backend(text)) => assert!(text.contains("Refused for the test"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_sync_the_server_refuses_part_of_fails_and_reports_nothing_removed() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "refused@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 3).await;
    let (backend, relay) = through_relay(&server, user).await;
    let first = sync(&backend, None).await;
    assert_eq!(first.added.len(), 3);
    let cursor = first.next_cursor.clone();
    // One more arrives, so the next sync has new mail to fetch as well as
    // flags to check.
    fill(&mut other_client, "INBOX", 1).await;

    for command in ["UID SEARCH", "UID FETCH", "SELECT"] {
        relay.refuse(&[command]);
        relay.clear();
        let synced = backend.sync_folder(&inbox(), Some(cursor.clone())).await;
        match command {
            // A folder the server will not open is, as before, not found.
            "SELECT" => assert!(matches!(synced, Err(Error::NotFound(_))), "{synced:?}"),
            _ => refused(synced),
        }
        assert!(
            relay
                .commands()
                .iter()
                .any(|sent| sent.starts_with(command)),
            "{command} was not sent: {:?}",
            relay.commands()
        );
    }
    relay.refuse(&["LIST"]);
    refused(backend.list_folders().await);

    // With the server itself again, the same cursor gives the true answer:
    // nothing was removed, and nothing was lost along the way.
    relay.refuse(&[]);
    let delta = sync(&backend, Some(&cursor)).await;
    assert!(delta.removed.is_empty(), "{:?}", delta.removed);
    assert_eq!(subjects(&delta), ["Message 1"]);
    assert_eq!(delta.changed.len(), 3);
}

#[tokio::test]
async fn a_refused_flag_change_or_fetch_is_an_error_and_not_an_answer() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "refused-change@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 1).await;
    let (backend, relay) = through_relay(&server, user).await;
    let first = sync(&backend, None).await;
    let id = first.added[0].id.clone();
    let raw = backend.fetch_raw(&id).await.unwrap();
    let seen = Operation::SetFlags {
        id: id.clone(),
        flags: Flags {
            seen: true,
            ..Flags::default()
        },
    };

    relay.refuse(&["UID STORE"]);
    refused(backend.apply(&seen).await);
    // It was not made, so it must not be taken for made.
    relay.refuse(&[]);
    let unchanged = sync(&backend, Some(&first.next_cursor)).await;
    assert!(!unchanged.changed[0].flags.seen);

    // The message is there. A fetch the server refuses must not say it
    // is gone.
    relay.refuse(&["UID FETCH"]);
    refused(backend.fetch_raw(&id).await);

    relay.refuse(&[]);
    assert_eq!(backend.fetch_raw(&id).await.unwrap(), raw);
    backend.apply(&seen).await.unwrap();
    let changed = sync(&backend, Some(&unchanged.next_cursor)).await;
    assert!(changed.changed[0].flags.seen);
    // A message that really is gone is still not found.
    let gone = RemoteId(id.0.replacen('1', "999", 1));
    assert!(matches!(
        backend.fetch_raw(&gone).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn a_server_that_stops_answering_is_given_up_on_and_the_next_call_reconnects() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "silent@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 2).await;
    let (backend, relay) = through_relay(&server, user).await;
    let patience = Duration::from_secs(1);
    let backend = backend.with_patience(patience);
    let first = sync(&backend, None).await;
    assert_eq!(first.added.len(), 2);
    let id = first.added[0].id.clone();
    let raw = backend.fetch_raw(&id).await.unwrap();

    relay.fall_silent(true);
    // Signed in already, and waiting for the answer to a command; then
    // with no connection, waiting to be signed in.
    for attempt in ["mid-session", "at sign-in"] {
        let started = Instant::now();
        let synced = backend
            .sync_folder(&inbox(), Some(first.next_cursor.clone()))
            .await;
        match synced {
            Err(Error::Connection(text)) => assert!(text.contains("timed out"), "{text}"),
            other => panic!("{attempt}: {other:?}"),
        }
        let waited = started.elapsed();
        assert!(waited >= patience, "{attempt}: {waited:?}");
        assert!(waited < patience * 5, "{attempt}: {waited:?}");
    }
    // Every kind of call gives up the same way, and none of them holds the
    // connection for the next.
    let started = Instant::now();
    assert!(matches!(
        backend.fetch_raw(&id).await,
        Err(Error::Connection(_))
    ));
    assert!(matches!(
        backend
            .move_message(&id, &FolderId("Archive".to_owned()))
            .await,
        Err(Error::Connection(_))
    ));
    assert!(started.elapsed() < patience * 10);

    // The server comes back, and so does the backend, on a new connection.
    relay.fall_silent(false);
    let delta = sync(&backend, Some(&first.next_cursor)).await;
    assert!(delta.added.is_empty() && delta.removed.is_empty());
    assert_eq!(delta.changed.len(), 2);
    assert_eq!(backend.fetch_raw(&id).await.unwrap(), raw);
}
