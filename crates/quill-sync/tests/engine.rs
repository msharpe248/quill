//! The sync engine against a scripted backend: a mailbox in memory that the
//! test changes behind the engine's back, the way a server would.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use quill_core::types::{
    Address, Capabilities, FlagChange, Flags, FolderId, FolderRole, Headers, LocalAccountId,
    LocalMessageId, Operation, RemoteEvent, RemoteFolder, RemoteId, RemoteMessage, SyncCursor,
    SyncDelta,
};
use quill_core::{Backend, Error, Result};
use quill_store::{BlobStore, Mailbox, Store};
use quill_sync::{SyncAccount, SyncEvent, SyncHandle, SyncOptions, SyncRequest};
use tokio::sync::mpsc::{Sender, UnboundedReceiver, unbounded_channel};

const NOW_ISH: i64 = 4_000_000_000; // far enough ahead to be "recent" for years

#[derive(Clone)]
struct Mail {
    subject: String,
    date: i64,
    flags: Flags,
    body: String,
}

#[derive(Default)]
struct Server {
    /// folder -> remote id -> message
    folders: BTreeMap<String, BTreeMap<String, Mail>>,
    /// Fail every call with this, while set.
    down: Option<String>,
    /// Present a certificate nobody trusts, while set.
    bad_certificate: bool,
    /// Refuse flag changes.
    read_only: bool,
    applied: Vec<Operation>,
    batch: usize,
}

#[derive(Clone, Default)]
struct MemoryBackend(Arc<Mutex<Server>>);

impl MemoryBackend {
    fn new(batch: usize) -> Self {
        let backend = Self::default();
        backend.0.lock().unwrap().batch = batch;
        backend
    }

    fn add(&self, folder: &str, id: &str, subject: &str, date: i64) {
        self.0
            .lock()
            .unwrap()
            .folders
            .entry(folder.to_owned())
            .or_default()
            .insert(
                id.to_owned(),
                Mail {
                    subject: subject.to_owned(),
                    date,
                    flags: Flags::default(),
                    body: format!("Body of {subject}."),
                },
            );
    }

    fn with<T>(&self, change: impl FnOnce(&mut Server) -> T) -> T {
        change(&mut self.0.lock().unwrap())
    }

    fn check(&self) -> Result<()> {
        if self.0.lock().unwrap().bad_certificate {
            return Err(Error::Certificate(Box::new(
                quill_core::CertificateProblem {
                    host: "mail.example.com".to_owned(),
                    reason: "UnknownIssuer".to_owned(),
                    fingerprint: Some("ab".repeat(32)),
                    certificate: Some(vec![1, 2, 3]),
                    details: None,
                },
            )));
        }
        match &self.0.lock().unwrap().down {
            Some(reason) => Err(Error::Connection(reason.clone())),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Backend for MemoryBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            push: false,
            server_search: false,
            threads: false,
        }
    }

    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        self.check()?;
        Ok(self
            .0
            .lock()
            .unwrap()
            .folders
            .keys()
            .map(|name| RemoteFolder {
                id: FolderId(name.clone()),
                name: name.clone(),
                role: if name == "INBOX" {
                    FolderRole::Inbox
                } else {
                    FolderRole::Other
                },
            })
            .collect())
    }

    /// The cursor is the comma-separated ids already delivered.
    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        self.check()?;
        let server = self.0.lock().unwrap();
        let mail = server
            .folders
            .get(&folder.0)
            .ok_or_else(|| Error::NotFound(folder.0.clone()))?;
        let known: Vec<String> = cursor
            .map(|cursor| cursor.0)
            .unwrap_or_default()
            .split(',')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        let missing: Vec<&String> = mail.keys().filter(|id| !known.contains(id)).collect();
        let batch: Vec<&String> = missing.iter().copied().take(server.batch).collect();
        let still: Vec<&String> = known.iter().filter(|id| mail.contains_key(*id)).collect();
        let message = |id: &String| {
            let item = &mail[id];
            RemoteMessage {
                id: RemoteId(id.clone()),
                headers: Headers {
                    message_id: None,
                    subject: item.subject.clone(),
                    from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                    to: Vec::new(),
                    cc: Vec::new(),
                    date: item.date,
                },
                flags: item.flags,
                size: item.body.len() as u64,
                has_attachments: false,
            }
        };
        let next: Vec<String> = still
            .iter()
            .chain(batch.iter())
            .map(|id| (*id).clone())
            .collect();
        Ok(SyncDelta {
            reset: false,
            added: batch.iter().map(|id| message(id)).collect(),
            changed: still
                .iter()
                .map(|id| FlagChange {
                    id: RemoteId((*id).clone()),
                    flags: mail[*id].flags,
                })
                .collect(),
            removed: known
                .iter()
                .filter(|id| !mail.contains_key(*id))
                .map(|id| RemoteId(id.clone()))
                .collect(),
            has_more: missing.len() > batch.len(),
            next_cursor: SyncCursor(next.join(",")),
        })
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        self.check()?;
        let server = self.0.lock().unwrap();
        let mail = server
            .folders
            .values()
            .find_map(|folder| folder.get(&id.0))
            .ok_or_else(|| Error::NotFound(id.0.clone()))?;
        Ok(Bytes::from(format!(
            "Subject: {}\r\nContent-Type: text/plain\r\n\r\n{}\r\n",
            mail.subject, mail.body
        )))
    }

    async fn apply(&self, op: &Operation) -> Result<()> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        if server.read_only {
            return Err(Error::Backend("the mailbox is read-only".to_owned()));
        }
        if let Operation::SetFlags { id, flags } = op {
            for folder in server.folders.values_mut() {
                if let Some(mail) = folder.get_mut(&id.0) {
                    mail.flags = *flags;
                }
            }
        }
        server.applied.push(op.clone());
        Ok(())
    }

    async fn watch(&self, _tx: Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }
}

struct Rig {
    backend: MemoryBackend,
    /// A second connection to the same database, as the UI has.
    store: Store,
    blobs: BlobStore,
    account: LocalAccountId,
    events: UnboundedReceiver<SyncEvent>,
    handle: SyncHandle,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn start(backend: MemoryBackend) -> Self {
        Self::start_with(
            backend,
            SyncOptions {
                interval: Duration::from_secs(3600),
                prefetch_days: 30,
                prefetch_limit: 100,
            },
        )
    }

    fn start_with(backend: MemoryBackend, options: SyncOptions) -> Self {
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
                backend: Arc::new(backend.clone()),
            }],
            options,
            events_tx,
        );
        Self {
            backend,
            store,
            blobs: BlobStore::new(dir.path().join("blobs")),
            account,
            events,
            handle,
            _dir: dir,
        }
    }

    async fn next(&mut self) -> SyncEvent {
        tokio::time::timeout(Duration::from_secs(5), self.events.recv())
            .await
            .expect("timed out waiting for the engine")
            .expect("the engine stopped")
    }

    /// Events up to and including the end of the pass in progress.
    async fn pass(&mut self) -> Vec<SyncEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self.next().await;
            let done = matches!(event, SyncEvent::Syncing { active: false, .. });
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    async fn sync_now(&mut self) -> Vec<SyncEvent> {
        assert!(self.handle.send(SyncRequest::SyncNow));
        self.pass().await
    }

    fn subjects(&self, folder: &str) -> Vec<String> {
        let folder = self
            .store
            .folders()
            .unwrap()
            .into_iter()
            .find(|row| row.name == folder)
            .unwrap_or_else(|| panic!("no folder {folder}"));
        self.store
            .messages(Mailbox::Folder(folder.id))
            .unwrap()
            .into_iter()
            .map(|row| row.headers.subject)
            .collect()
    }

    fn local(&self, subject: &str) -> LocalMessageId {
        self.store
            .folders()
            .unwrap()
            .into_iter()
            .flat_map(|folder| self.store.messages(Mailbox::Folder(folder.id)).unwrap())
            .find(|row| row.headers.subject == subject)
            .unwrap_or_else(|| panic!("no message {subject}"))
            .id
    }
}

fn changed(events: &[SyncEvent]) -> usize {
    events
        .iter()
        .filter(|event| **event == SyncEvent::Changed)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_pass_brings_in_folders_messages_and_recent_bodies() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Older", NOW_ISH - 60);
    backend.add("INBOX", "2", "Newer", NOW_ISH);
    backend.add("Archive", "3", "Filed", NOW_ISH - 120);
    let mut rig = Rig::start(backend);

    let events = rig.pass().await;
    assert_eq!(
        events.first(),
        Some(&SyncEvent::Syncing {
            account: rig.account,
            active: true
        })
    );
    assert!(changed(&events) >= 2, "{events:?}");
    assert_eq!(rig.subjects("INBOX"), ["Newer", "Older"]);
    assert_eq!(rig.subjects("Archive"), ["Filed"]);

    // Bodies were fetched ahead, their raw bytes filed by content hash.
    let id = rig.local("Newer");
    assert_eq!(
        rig.store.body(id).unwrap().as_deref(),
        Some("Body of Newer.\n")
    );
    let hash = rig.store.blob_hash(id).unwrap().unwrap();
    let raw = rig.blobs.get(&hash).unwrap().unwrap();
    assert!(raw.starts_with(b"Subject: Newer\r\n"));
    assert_eq!(BlobStore::hash(&raw), hash);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_larger_than_a_batch_arrives_in_several_steps() {
    let backend = MemoryBackend::new(2);
    for n in 1..=5 {
        backend.add(
            "INBOX",
            &format!("{n}"),
            &format!("Message {n}"),
            NOW_ISH + n,
        );
    }
    let mut rig = Rig::start(backend);
    let events = rig.pass().await;
    // One change per batch: the UI could show the first before the last came.
    assert_eq!(changed(&events), 1 + 3, "{events:?}");
    assert_eq!(rig.subjects("INBOX").len(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pass_that_finds_nothing_new_changes_nothing() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Only", NOW_ISH);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let events = rig.sync_now().await;
    assert_eq!(changed(&events), 0, "{events:?}");
    assert_eq!(events.len(), 2, "{events:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn later_passes_follow_the_server() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Stays", NOW_ISH);
    backend.add("INBOX", "2", "Goes", NOW_ISH - 1);
    backend.add("Old Project", "3", "Buried", NOW_ISH - 2);
    let mut rig = Rig::start(backend);
    rig.pass().await;

    rig.backend.with(|server| {
        let inbox = server.folders.get_mut("INBOX").unwrap();
        inbox.remove("2");
        inbox.get_mut("1").unwrap().flags.seen = true;
        server.folders.remove("Old Project");
    });
    rig.backend.add("INBOX", "4", "Arrives", NOW_ISH + 1);

    let events = rig.sync_now().await;
    assert!(changed(&events) >= 1);
    assert_eq!(rig.subjects("INBOX"), ["Arrives", "Stays"]);
    let stays = rig.local("Stays");
    assert!(
        rig.store
            .message_location(stays)
            .unwrap()
            .unwrap()
            .flags
            .seen
    );
    let folders: Vec<String> = rig
        .store
        .folders()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(folders, ["INBOX"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_recent_bodies_are_fetched_ahead_and_older_ones_on_request() {
    let backend = MemoryBackend::new(100);
    let day = 24 * 60 * 60;
    let real_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    backend.add("INBOX", "1", "This week", real_now - 2 * day);
    backend.add("INBOX", "2", "Last year", real_now - 400 * day);
    let mut rig = Rig::start(backend);
    rig.pass().await;

    let (recent, old) = (rig.local("This week"), rig.local("Last year"));
    assert!(rig.store.body(recent).unwrap().is_some());
    assert_eq!(rig.store.body(old).unwrap(), None);

    assert!(rig.handle.send(SyncRequest::FetchBody(old)));
    assert_eq!(rig.next().await, SyncEvent::BodyReady(old));
    assert_eq!(
        rig.store.body(old).unwrap().as_deref(),
        Some("Body of Last year.\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_that_cannot_be_fetched_says_why() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Old", 1000);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let id = rig.local("Old");

    rig.backend
        .with(|server| server.down = Some("no route to host".to_owned()));
    rig.handle.send(SyncRequest::FetchBody(id));
    match rig.next().await {
        SyncEvent::BodyFailed { id: failed, reason } => {
            assert_eq!(failed, id);
            assert!(reason.contains("no route to host"), "{reason}");
        }
        other => panic!("{other:?}"),
    }

    // Gone from the server by the time it was asked for.
    rig.backend.with(|server| {
        server.down = None;
        server.folders.get_mut("INBOX").unwrap().clear();
    });
    rig.handle.send(SyncRequest::FetchBody(id));
    match rig.next().await {
        SyncEvent::BodyFailed { reason, .. } => assert!(reason.contains("no longer"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // And a message the store has never heard of.
    rig.handle
        .send(SyncRequest::FetchBody(LocalMessageId(9999)));
    assert!(matches!(rig.next().await, SyncEvent::BodyFailed { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flag_change_reaches_the_server() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Read me", NOW_ISH);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let id = rig.local("Read me");
    let seen = Flags {
        seen: true,
        ..Flags::default()
    };

    // The UI writes the store first, then tells the engine.
    rig.store.set_flags(id, seen).unwrap();
    rig.handle.send(SyncRequest::SetFlags {
        id,
        flags: seen,
        previous: Flags::default(),
    });
    // The next pass confirms it and finds nothing to change.
    let events = rig.sync_now().await;
    assert_eq!(changed(&events), 0, "{events:?}");
    assert_eq!(
        rig.backend.with(|server| server.applied.clone()),
        [Operation::SetFlags {
            id: RemoteId("1".to_owned()),
            flags: seen
        }]
    );
    assert!(rig.store.message_location(id).unwrap().unwrap().flags.seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flag_change_the_server_refuses_is_taken_back() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Read me", NOW_ISH);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let id = rig.local("Read me");
    let seen = Flags {
        seen: true,
        ..Flags::default()
    };
    rig.backend.with(|server| server.read_only = true);

    rig.store.set_flags(id, seen).unwrap();
    rig.handle.send(SyncRequest::SetFlags {
        id,
        flags: seen,
        previous: Flags::default(),
    });
    assert_eq!(rig.next().await, SyncEvent::Changed);
    match rig.next().await {
        SyncEvent::Problem { account, message } => {
            assert_eq!(account, "Work");
            assert!(message.contains("not saved on the server"), "{message}");
            assert!(message.contains("read-only"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!rig.store.message_location(id).unwrap().unwrap().flags.seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outage_is_reported_once_and_sync_resumes_after_it() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Before", NOW_ISH);
    backend.with(|server| server.down = Some("connection refused".to_owned()));
    let mut rig = Rig::start(backend);

    let events = rig.pass().await;
    assert_eq!(changed(&events), 0);
    // The problem follows the end of the pass.
    match rig.next().await {
        SyncEvent::Problem { account, message } => {
            assert_eq!(account, "Work");
            assert!(message.contains("connection refused"), "{message}");
        }
        other => panic!("{other:?}"),
    }

    // Still down: the same failure is not announced a second time.
    rig.sync_now().await;
    rig.backend.with(|server| server.down = None);
    let events = rig.sync_now().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Problem { .. }))
    );
    assert_eq!(rig.subjects("INBOX"), ["Before"]);

    // A new outage after recovery is news again.
    rig.backend
        .with(|server| server.down = Some("connection refused".to_owned()));
    rig.sync_now().await;
    assert!(matches!(rig.next().await, SyncEvent::Problem { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_interval_drives_passes_without_being_asked() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", NOW_ISH);
    let mut rig = Rig::start_with(
        backend,
        SyncOptions {
            interval: Duration::from_millis(50),
            prefetch_days: 30,
            prefetch_limit: 10,
        },
    );
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Second", NOW_ISH + 1);
    let events = rig.pass().await;
    assert_eq!(changed(&events), 1);
    assert_eq!(rig.subjects("INBOX"), ["Second", "First"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_handle_stops_the_engine() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Only", NOW_ISH);
    let mut rig = Rig::start_with(
        backend,
        SyncOptions {
            interval: Duration::from_millis(20),
            prefetch_days: 30,
            prefetch_limit: 10,
        },
    );
    rig.pass().await;
    drop(rig.handle);
    // The channel closes once every task holding the engine has gone.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while rig.events.recv().await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "the engine kept running");
}

#[tokio::test]
async fn an_account_added_while_running_starts_syncing() {
    let work = MemoryBackend::new(10);
    work.add("INBOX", "1", "Work mail", NOW_ISH);
    let mut rig = Rig::start(work);
    rig.pass().await;

    let personal = MemoryBackend::new(10);
    personal.add("INBOX", "1", "Personal mail", NOW_ISH);
    let id = rig
        .store
        .upsert_account("Personal", "robin@home.example.com", "imap")
        .unwrap();
    rig.handle.set_account(SyncAccount {
        id,
        name: "Personal".to_owned(),
        backend: Arc::new(personal.clone()),
    });
    let events = rig.pass().await;
    assert!(events.contains(&SyncEvent::Syncing {
        account: id,
        active: true
    }));
    let subjects: Vec<String> = rig
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect();
    assert!(
        subjects.contains(&"Personal mail".to_owned()),
        "{subjects:?}"
    );
    assert!(subjects.contains(&"Work mail".to_owned()));

    // Both accounts answer a request to sync now.
    assert!(rig.handle.send(SyncRequest::SyncNow));
    let mut finished = Vec::new();
    while finished.len() < 2 {
        if let SyncEvent::Syncing {
            account,
            active: false,
        } = rig.next().await
        {
            finished.push(account);
        }
    }
    assert!(finished.contains(&id) && finished.contains(&rig.account));
}

#[tokio::test]
async fn an_account_set_again_is_replaced_not_doubled() {
    let old = MemoryBackend::new(10);
    old.add("INBOX", "1", "From the old settings", NOW_ISH);
    let mut rig = Rig::start(old.clone());
    rig.pass().await;

    // The same account, reached another way: say, after its certificate was
    // pinned. The old connection must not go on syncing beside the new one.
    let new = MemoryBackend::new(10);
    new.add("INBOX", "1", "From the old settings", NOW_ISH);
    new.add("INBOX", "2", "Arrived since", NOW_ISH + 1);
    rig.handle.set_account(SyncAccount {
        id: rig.account,
        name: "Work".to_owned(),
        backend: Arc::new(new),
    });
    // The replaced loop reports that it stopped; then the new one runs.
    loop {
        if rig.subjects("INBOX").contains(&"Arrived since".to_owned()) {
            break;
        }
        rig.next().await;
    }
    old.add("INBOX", "3", "Only the old server has this", NOW_ISH + 2);
    rig.sync_now().await;
    assert!(
        !rig.subjects("INBOX")
            .contains(&"Only the old server has this".to_owned())
    );

    // Removed, it stops; its mail stays.
    rig.handle.remove_account(rig.account);
    assert!(rig.handle.send(SyncRequest::SyncNow));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.subjects("INBOX").contains(&"Arrived since".to_owned()));
}

#[tokio::test]
async fn a_refused_certificate_is_reported_with_the_certificate_once() {
    let backend = MemoryBackend::new(10);
    backend.add("INBOX", "1", "Hello", NOW_ISH);
    backend.with(|server| server.bad_certificate = true);
    let mut rig = Rig::start(backend.clone());
    rig.pass().await;
    // The report follows the end of the pass that failed.
    let SyncEvent::Certificate { account, problem } = rig.next().await else {
        panic!("expected a certificate event");
    };
    assert_eq!(account, "Work");
    assert_eq!(problem.host, "mail.example.com");
    assert_eq!(
        problem.fingerprint.as_deref(),
        Some("ab".repeat(32).as_str())
    );
    // Nothing was fetched: not even the list of folders.
    assert!(rig.store.folders().unwrap().is_empty());

    // The same refusal on the next pass is not news.
    rig.sync_now().await;
    let quiet = tokio::time::timeout(Duration::from_millis(150), rig.events.recv()).await;
    assert!(quiet.is_err(), "{quiet:?}");

    // Once the certificate is accepted, mail flows.
    backend.with(|server| server.bad_certificate = false);
    rig.sync_now().await;
    assert_eq!(rig.subjects("INBOX"), ["Hello"]);
}
