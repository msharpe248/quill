//! A folder renamed and a folder deleted, from the sync engine down to a
//! real server: the engine, the store and this backend together, as Quill
//! runs them.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Dovecot, fill};
use quill_core::Backend;
use quill_core::config::TlsMode;
use quill_core::types::{FolderMail, LocalFolderId, LocalMessageId};
use quill_store::{Mailbox, Store};
use quill_sync::{ServerCopies, SyncAccount, SyncEvent, SyncHandle, SyncOptions, SyncRequest};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct Rig {
    /// A second connection to the database, as the UI has.
    store: Store,
    events: UnboundedReceiver<SyncEvent>,
    handle: SyncHandle,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn start(backend: Arc<dyn Backend>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let mut store = Store::open(&path).unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let (events_tx, events) = unbounded_channel();
        let handle = quill_sync::spawn(
            Store::open(&path).unwrap(),
            Some(dir.path().join("blobs")),
            vec![SyncAccount {
                id: account,
                name: "Work".to_owned(),
                backend,
                sender: None,
                server_saves_sent: false,
                server_copies: ServerCopies::Keep,
            }],
            SyncOptions {
                interval: Duration::from_secs(3600),
                retry: Duration::from_secs(1),
                // Everything is downloaded ahead, however it is dated.
                prefetch_days: 365 * 100,
                prefetch_limit: 100,
                turn: Duration::from_secs(60),
            },
            events_tx,
        );
        Self {
            store,
            events,
            handle,
            _dir: dir,
        }
    }

    /// Events up to and including the first that `wanted` accepts.
    async fn until(&mut self, wanted: impl Fn(&SyncEvent) -> bool) -> Vec<SyncEvent> {
        let mut seen = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for the engine, after {seen:?}"))
                .expect("the engine stopped");
            let done = wanted(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    /// Waits for the pass in progress, or the next one, to end.
    async fn pass(&mut self) {
        self.until(|event| matches!(event, SyncEvent::Syncing { active: false, .. }))
            .await;
    }

    /// Waits until the message has been downloaded.
    async fn downloaded(&self, subject: &str, id: LocalMessageId) {
        for _ in 0..1000 {
            if self.store.body(id).unwrap().is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{subject} was not downloaded");
    }

    fn folder(&self, name: &str) -> LocalFolderId {
        self.store
            .folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.name == name)
            .unwrap_or_else(|| panic!("no folder {name}"))
            .id
    }

    fn names(&self) -> Vec<String> {
        self.store
            .folders()
            .unwrap()
            .into_iter()
            .map(|folder| folder.name)
            .collect()
    }

    /// The messages of a folder, by subject, with what each is known by
    /// here.
    fn mail(&self, name: &str) -> Vec<(String, LocalMessageId)> {
        let mut mail: Vec<(String, LocalMessageId)> = self
            .store
            .messages(Mailbox::Folder(self.folder(name)))
            .unwrap()
            .into_iter()
            .map(|row| (row.headers.subject, row.id))
            .collect();
        mail.sort();
        mail
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_renamed_from_quill_keeps_its_mail_here_and_goes_on_syncing() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "engine-rename@example.com";
    let mut other_client = server.session(user).await;
    other_client.create("Projects").await.unwrap();
    other_client.create("Projects/2026").await.unwrap();
    fill(&mut other_client, "Projects", 2).await;
    fill(&mut other_client, "Projects/2026", 1).await;
    let backend = Arc::new(server.backend(TlsMode::Implicit, user));
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let before = rig.mail("Projects");
    assert_eq!(before.len(), 2);
    let inside = rig.mail("Projects/2026");
    // A pass leaves the downloading to look at the folders again when the
    // server says something changed, and takes it up at once: wait for
    // the downloads themselves.
    for (subject, id) in before.iter().chain(&inside) {
        rig.downloaded(subject, *id).await;
    }

    rig.handle.send(SyncRequest::RenameFolder {
        folder: rig.folder("Projects"),
        name: "Work".to_owned(),
    });
    let events = rig
        .until(|event| {
            matches!(
                event,
                SyncEvent::FolderRenamed { .. } | SyncEvent::Problem { .. }
            )
        })
        .await;
    assert_eq!(
        events.last(),
        Some(&SyncEvent::FolderRenamed {
            account: "Work".to_owned(),
            from: "Projects".to_owned(),
            to: "Work".to_owned(),
        })
    );
    // The pass that follows finds nothing new and nothing gone.
    rig.pass().await;
    let names = rig.names();
    assert!(names.iter().any(|name| name == "Work"), "{names:?}");
    assert!(names.iter().any(|name| name == "Work/2026"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("Projects")),
        "{names:?}"
    );
    // The same messages, known here as they were, with what was
    // downloaded of them.
    assert_eq!(rig.mail("Work"), before);
    assert_eq!(rig.mail("Work/2026"), inside);
    for (_, id) in before.iter().chain(&inside) {
        assert!(rig.store.body(*id).unwrap().is_some());
    }

    // The folder goes on being synced under its new name: mail that
    // arrives in it is listed, once.
    fill(&mut other_client, "Work", 3).await;
    assert!(rig.handle.send(SyncRequest::SyncNow));
    rig.pass().await;
    assert_eq!(rig.mail("Work").len(), 5);
    // And a message that was there before is still fetched by what it is
    // known by now.
    let old = before[0].1;
    rig.handle.send(SyncRequest::FetchBody(old));
    rig.until(|event| *event == SyncEvent::BodyReady(old)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_deleted_from_quill_leaves_its_mail_in_the_trash_there_and_here() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "engine-delete@example.com";
    let mut other_client = server.session(user).await;
    other_client.create("Receipts").await.unwrap();
    fill(&mut other_client, "Receipts", 3).await;
    fill(&mut other_client, "INBOX", 1).await;
    let backend = Arc::new(server.backend(TlsMode::Implicit, user));
    let mut rig = Rig::start(backend);
    rig.pass().await;
    assert_eq!(rig.mail("Receipts").len(), 3);
    assert!(rig.mail("Trash").is_empty());

    rig.handle
        .send(SyncRequest::DeleteFolder(rig.folder("Receipts")));
    let events = rig
        .until(|event| {
            matches!(
                event,
                SyncEvent::FolderDeleted { .. } | SyncEvent::Problem { .. }
            )
        })
        .await;
    assert_eq!(
        events.last(),
        Some(&SyncEvent::FolderDeleted {
            account: "Work".to_owned(),
            name: "Receipts".to_owned(),
            mail: FolderMail::Trashed,
            messages: 3,
        })
    );
    rig.pass().await;

    assert!(!rig.names().iter().any(|name| name == "Receipts"));
    let trashed: Vec<String> = rig
        .mail("Trash")
        .into_iter()
        .map(|(subject, _)| subject)
        .collect();
    assert_eq!(trashed, ["Message 1", "Message 2", "Message 3"]);
    assert_eq!(rig.mail("Inbox").len(), 1);
    // On the server too: the folder is gone and its mail is in the trash.
    let listed = other_client.list(Some(""), Some("*")).await.unwrap();
    let listed: Vec<String> = futures_util::TryStreamExt::try_collect::<Vec<_>>(listed)
        .await
        .unwrap()
        .iter()
        .map(|name| name.name().to_owned())
        .collect();
    assert!(!listed.iter().any(|name| name == "Receipts"), "{listed:?}");
    let trash = other_client.select("Trash").await.unwrap();
    assert_eq!(trash.exists, 3);

    // A folder the server keeps for a purpose is not deleted.
    rig.handle
        .send(SyncRequest::DeleteFolder(rig.folder("Trash")));
    let events = rig
        .until(|event| {
            matches!(
                event,
                SyncEvent::FolderDeleted { .. } | SyncEvent::Problem { .. }
            )
        })
        .await;
    assert!(matches!(events.last(), Some(SyncEvent::Problem { .. })));
}
