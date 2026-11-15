//! Push: `watch` idling on the inbox of a real server.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_imap::Session;
use common::{Dovecot, PASSWORD, Without, backend, fill};
use futures_util::TryStreamExt;
use quill_core::config::TlsMode;
use quill_core::types::{Flags, FolderId, Operation, RemoteEvent};
use quill_core::{Backend, Error, Result};
use quill_imap::ImapBackend;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, Receiver};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// How long a change may take to be reported. Dovecot tells an idling
/// connection within a second or so, and the watch adds a quarter more.
const SOON: Duration = Duration::from_secs(5);

fn inbox() -> FolderId {
    FolderId("INBOX".to_owned())
}

/// A `watch` running the way the sync engine will run it: on a task of its
/// own, with the backend shared.
struct Watch {
    events: Receiver<RemoteEvent>,
    task: JoinHandle<Result<()>>,
}

impl Watch {
    fn start(backend: &Arc<ImapBackend>) -> Self {
        let (tx, events) = mpsc::channel(16);
        let backend = Arc::clone(backend);
        let task = tokio::spawn(async move { backend.watch(tx).await });
        Self { events, task }
    }

    /// The next event, which has to be about the inbox and has to come
    /// soon.
    async fn changed(&mut self, after: &str) {
        match timeout(SOON, self.events.recv()).await {
            Ok(Some(event)) => assert_eq!(event, RemoteEvent::FolderChanged(inbox()), "{after}"),
            Ok(None) => panic!("{after}: the watch ended"),
            Err(_) => panic!("{after}: nothing was reported"),
        }
    }

    /// Checks nothing is reported for `long`.
    async fn quiet(&mut self, long: Duration, after: &str) {
        let waited = timeout(long, self.events.recv()).await;
        assert!(waited.is_err(), "{after}: {waited:?}");
    }

    /// How the watch ended, which has to be soon.
    async fn ended(self) -> Result<()> {
        timeout(SOON, self.task)
            .await
            .expect("the watch to end")
            .unwrap()
    }
}

/// Changes flags the way another mail program would.
async fn store(session: &mut Session<TcpStream>, uids: &str, change: &str) {
    let _: Vec<_> = session
        .uid_store(uids, change)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
}

/// A backend for `user` through a relay that sees every command, can hide
/// what Dovecot offers, and can be told to stop answering.
async fn through_relay(server: &Dovecot, user: &str, hidden: &[&str]) -> (ImapBackend, Without) {
    let relay = Without::start(server.imap_port, hidden).await;
    let mut config = server.config(TlsMode::None, user);
    config.port = relay.port;
    (backend(config, PASSWORD), relay)
}

/// Waits for `what` to become true, which has to be soon.
async fn eventually(what: impl Fn() -> bool, describe: &str) {
    let deadline = Instant::now() + SOON;
    while !what() {
        assert!(Instant::now() < deadline, "{describe}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn changes_made_elsewhere_are_pushed_while_the_backend_goes_on_working() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "pushed@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 2).await;
    other_client.select("INBOX").await.unwrap();

    let backend = Arc::new(server.backend(TlsMode::Implicit, user));
    // Not claimed before the server has been asked.
    assert_eq!(backend.supports_idle(), None);
    assert!(!backend.capabilities().push);

    let mut watch = Watch::start(&backend);
    // The watch is in place. Nothing has changed, and nothing more is said
    // until something does.
    watch.changed("starting to watch").await;
    assert_eq!(backend.supports_idle(), Some(true));
    assert!(backend.capabilities().push);
    watch
        .quiet(Duration::from_secs(2), "nothing happening")
        .await;

    // New mail, delivered over another connection: one event for it.
    fill(&mut other_client, "INBOX", 1).await;
    watch.changed("a delivery").await;
    watch.quiet(Duration::from_secs(1), "one delivery").await;

    // The backend's own connection is free the whole time: a sync runs
    // while the watch waits, and finds what the event was about.
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(delta.added.len(), 3);
    assert_eq!(backend.list_folders().await.unwrap().len(), 6);
    watch.quiet(Duration::from_secs(1), "a sync").await;

    // A flag changed by another mail program.
    store(&mut other_client, "1", "+FLAGS (\\Seen)").await;
    watch.changed("a flag change elsewhere").await;
    watch.quiet(Duration::from_secs(1), "one flag change").await;

    // A flag changed by this backend is a change like any other: the
    // server does not know the two connections are the same program.
    let flagged = Operation::SetFlags {
        id: delta.added[0].id.clone(),
        flags: Flags {
            flagged: true,
            ..delta.added[0].flags
        },
    };
    backend.apply(&flagged).await.unwrap();
    watch.changed("a flag change of our own").await;
    watch.quiet(Duration::from_secs(1), "our flag change").await;

    // A message removed.
    store(&mut other_client, "2", "+FLAGS (\\Deleted)").await;
    watch.changed("a message marked deleted").await;
    watch.quiet(Duration::from_secs(1), "the mark").await;
    let _: Vec<_> = other_client
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    watch.changed("an expunge").await;
    watch.quiet(Duration::from_secs(1), "one expunge").await;

    // The engine goes away, and the watch ends well.
    let Watch { events, task } = watch;
    drop(events);
    timeout(SOON, task).await.unwrap().unwrap().unwrap();
    backend.disconnect().await;
}

#[tokio::test]
async fn the_idle_is_refreshed_and_changes_are_still_pushed_after() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "refreshed@example.com";
    let mut other_client = server.session(user).await;
    let (backend, relay) = through_relay(&server, user, &[]).await;
    let refresh = Duration::from_millis(400);
    let backend = Arc::new(backend.with_idle_refresh(refresh));

    let mut watch = Watch::start(&backend);
    watch.changed("starting to watch").await;
    // Several refreshes, and not a word about them.
    watch.quiet(refresh * 5, "refreshing").await;
    fill(&mut other_client, "INBOX", 1).await;
    watch.changed("a delivery after refreshing").await;
    watch.quiet(refresh * 3, "refreshing again").await;

    let Watch { events, task } = watch;
    drop(events);
    timeout(SOON, task).await.unwrap().unwrap().unwrap();

    // Only the watch went through the relay, so this is its whole
    // conversation: sign in, open the inbox read-only, and then nothing
    // but idles, each ended before the next thing is sent.
    let commands = relay.commands();
    let verbs: Vec<&str> = commands
        .iter()
        .map(|command| command.split(' ').next().unwrap())
        .collect();
    assert_eq!(
        verbs[..4],
        ["LOGIN", "CAPABILITY", "LIST", "EXAMINE"],
        "{commands:?}"
    );
    assert_eq!(commands[3], "EXAMINE \"INBOX\"");
    let (idles, goodbye) = verbs[4..].split_at(verbs.len() - 5);
    assert_eq!(goodbye, ["LOGOUT"], "{commands:?}");
    assert!(idles.len() >= 2 * 6, "{commands:?}");
    for pair in idles.chunks(2) {
        assert_eq!(pair, ["IDLE", "DONE"], "{commands:?}");
    }
    eventually(|| relay.connections() == 0, "the watch's connection closed").await;
}

#[tokio::test]
async fn dropping_the_watch_closes_its_connection_and_leaves_the_backend_working() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "dropped@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 1).await;
    let (backend, relay) = through_relay(&server, user, &[]).await;
    let backend = Arc::new(backend);

    let mut watch = Watch::start(&backend);
    watch.changed("starting to watch").await;
    assert_eq!(relay.connections(), 1);
    // And one for commands.
    assert_eq!(
        backend
            .sync_folder(&inbox(), None)
            .await
            .unwrap()
            .added
            .len(),
        1
    );
    assert_eq!(relay.connections(), 2);

    // The engine drops the future, as it does when an account is removed.
    let Watch { events, task } = watch;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    eventually(|| relay.connections() == 1, "the watch's connection closed").await;
    // Dovecot saw it go, too: the session that was idling is over.
    eventually(
        || server.log().contains("IDLE running"),
        "Dovecot logging the idling session's end",
    )
    .await;
    // Nothing is left behind to send anything.
    drop(events);

    // The command connection was never part of it.
    assert_eq!(
        backend
            .sync_folder(&inbox(), None)
            .await
            .unwrap()
            .added
            .len(),
        1
    );
    assert_eq!(relay.connections(), 1);
    // And watching can begin again.
    let mut watch = Watch::start(&backend);
    watch.changed("starting to watch again").await;
    fill(&mut other_client, "INBOX", 1).await;
    watch.changed("a delivery").await;
    watch.task.abort();
    backend.disconnect().await;
    eventually(|| relay.connections() == 0, "every connection closed").await;
}

#[tokio::test]
async fn a_server_stopped_while_the_watch_idles_ends_it_with_an_error() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = Arc::new(server.backend(TlsMode::Implicit, "stopped@example.com"));
    let mut watch = Watch::start(&backend);
    watch.changed("starting to watch").await;

    let started = Instant::now();
    drop(server);
    let ended = watch.ended().await;
    assert!(matches!(ended, Err(Error::Connection(_))), "{ended:?}");
    // Found from the connection closing, long before any refresh.
    assert!(started.elapsed() < SOON);
}

#[tokio::test]
async fn a_server_that_goes_silent_is_found_out_at_the_next_refresh() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let (backend, relay) = through_relay(&server, "silent-idle@example.com", &[]).await;
    let (patience, refresh) = (Duration::from_millis(700), Duration::from_millis(1500));
    let backend = Arc::new(backend.with_patience(patience).with_idle_refresh(refresh));
    let mut watch = Watch::start(&backend);
    watch.changed("starting to watch").await;

    // The connection stays open and nothing answers on it any more: what a
    // laptop finds after it has slept.
    let started = Instant::now();
    relay.fall_silent(true);
    match watch.ended().await {
        Err(Error::Connection(text)) => assert!(text.contains("timed out"), "{text}"),
        other => panic!("{other:?}"),
    }
    // It was the `DONE` that went unanswered, and was waited on for as long
    // as any command is.
    assert_eq!(relay.commands().last().map(String::as_str), Some("DONE"));
    assert!(started.elapsed() >= patience);
    assert!(started.elapsed() < refresh + patience * 4);
    eventually(|| relay.connections() == 0, "the watch's connection closed").await;
}

#[tokio::test]
async fn a_server_without_idle_is_not_watched_and_says_so() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "no-push@example.com";
    let (backend, relay) = through_relay(&server, user, &["IDLE"]).await;
    let backend = Arc::new(backend);

    let (tx, mut events) = mpsc::channel(16);
    let started = Instant::now();
    backend.watch(tx).await.unwrap();
    // At once, with the receiver still held and nothing in it: poll.
    assert!(started.elapsed() < SOON);
    assert!(events.try_recv().is_err());
    assert_eq!(backend.supports_idle(), Some(false));
    assert!(!backend.capabilities().push);
    let commands = relay.commands();
    assert_eq!(
        commands
            .iter()
            .map(|command| command.split(' ').next().unwrap())
            .collect::<Vec<_>>(),
        ["LOGIN", "CAPABILITY", "LOGOUT"],
        "{commands:?}"
    );
    eventually(|| relay.connections() == 0, "the watch's connection closed").await;

    // The rest of the backend is as it would be anywhere.
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 1).await;
    assert_eq!(
        backend
            .sync_folder(&inbox(), None)
            .await
            .unwrap()
            .added
            .len(),
        1
    );
    assert_eq!(backend.supports_idle(), Some(false));
}

#[tokio::test]
async fn a_watch_nobody_listens_to_does_not_begin() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let (backend, relay) = through_relay(&server, "unheard@example.com", &[]).await;
    let (tx, events) = mpsc::channel(16);
    drop(events);
    backend.watch(tx).await.unwrap();
    assert!(relay.commands().is_empty());
    assert_eq!(backend.supports_idle(), None);
}

#[tokio::test]
async fn a_wrong_password_ends_the_watch_with_an_auth_error() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let config = server.config(TlsMode::Implicit, "wrong@example.com");
    let backend = backend(config, "not-the-password");
    let (tx, _events) = mpsc::channel(16);
    let ended = timeout(SOON * 2, backend.watch(tx)).await.unwrap();
    assert!(matches!(ended, Err(Error::Auth(_))), "{ended:?}");
}
