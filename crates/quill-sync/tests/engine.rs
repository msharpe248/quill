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
    LocalFolderId, LocalMessageId, Operation, RemoteEvent, RemoteFolder, RemoteId, RemoteMessage,
    SyncCursor, SyncDelta,
};
use quill_core::{Backend, Envelope, Error, Result};
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
    /// (folder, raw message) for everything saved with `append`.
    appended: Vec<(String, String)>,
    /// Move without saying what the message is called afterwards.
    forgets_new_ids: bool,
    batch: usize,
    /// Whether the server says when something changes. Without it `watch`
    /// comes straight back.
    pushes: bool,
    /// How many times `watch` has been called.
    watches: usize,
    /// Fail this many calls to `watch` before one holds.
    watch_failures: usize,
    /// Whoever is listening now.
    listeners: Vec<Sender<RemoteEvent>>,
    /// The folders asked about, in the order they were.
    synced: Vec<String>,
    /// A folder the server will not give, though it lists it.
    refused: Option<String>,
    /// Downloads wait for a permit each, while set.
    download_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// How many downloads have been asked for.
    downloads: usize,
}

/// A sender that keeps what it was given, or refuses as told.
#[derive(Clone, Default)]
struct Outgoing(Arc<Mutex<Post>>);

#[derive(Default)]
struct Post {
    sent: Vec<(Envelope, String)>,
    /// Fail the next sends with these, first first.
    failures: Vec<Error>,
}

#[async_trait]
impl quill_core::Sender for Outgoing {
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()> {
        let mut post = self.0.lock().unwrap();
        if !post.failures.is_empty() {
            return Err(post.failures.remove(0));
        }
        post.sent
            .push((envelope.clone(), String::from_utf8_lossy(raw).into_owned()));
        Ok(())
    }
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
                role: match name.as_str() {
                    "INBOX" => FolderRole::Inbox,
                    "Sent" => FolderRole::Sent,
                    "Archive" => FolderRole::Archive,
                    "Trash" => FolderRole::Trash,
                    _ => FolderRole::Other,
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
        let mut server = self.0.lock().unwrap();
        server.synced.push(folder.0.clone());
        if server.refused.as_deref() == Some(folder.0.as_str()) {
            return Err(Error::Backend("NO not just now".to_owned()));
        }
        let server = server;
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
                thread: Vec::new(),
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
        let gate = {
            let mut server = self.0.lock().unwrap();
            server.downloads += 1;
            server.download_gate.clone()
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
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
        match op {
            Operation::SetFlags { id, flags } => {
                for folder in server.folders.values_mut() {
                    if let Some(mail) = folder.get_mut(&id.0) {
                        mail.flags = *flags;
                    }
                }
            }
            Operation::Delete { id } => {
                for folder in server.folders.values_mut() {
                    folder.remove(&id.0);
                }
            }
            // A second copy, named after the first.
            Operation::Copy { id, to } => {
                let found = server
                    .folders
                    .values()
                    .find_map(|folder| folder.get(&id.0))
                    .map(|mail| Mail {
                        subject: mail.subject.clone(),
                        date: mail.date,
                        flags: mail.flags,
                        body: mail.body.clone(),
                    });
                if let Some(mail) = found {
                    server
                        .folders
                        .entry(to.0.clone())
                        .or_default()
                        .insert(format!("{}-copy", id.0), mail);
                }
            }
            Operation::Move { .. } => {}
        }
        server.applied.push(op.clone());
        Ok(())
    }

    /// A folder at the top of the mailbox, unless there is one already.
    async fn create_folder(&self, name: &str) -> Result<RemoteFolder> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        if server.folders.contains_key(name) {
            return Err(Error::Backend(format!(
                "there is already a folder called {name}"
            )));
        }
        server.folders.insert(name.to_owned(), BTreeMap::new());
        Ok(RemoteFolder {
            id: FolderId(name.to_owned()),
            name: name.to_owned(),
            role: FolderRole::Other,
        })
    }

    /// Moves the message and renames it `<old id>-moved`, as a server that
    /// reports new ids does. With `forgets_new_ids` it moves and says
    /// nothing.
    async fn move_message(&self, id: &RemoteId, to: &FolderId) -> Result<Option<RemoteId>> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        if server.read_only {
            return Err(Error::Backend("the mailbox is read-only".to_owned()));
        }
        if !server.folders.contains_key(&to.0) {
            return Err(Error::NotFound(to.0.clone()));
        }
        let found = server
            .folders
            .values_mut()
            .find_map(|folder| folder.remove(&id.0));
        server.applied.push(Operation::Move {
            id: id.clone(),
            to: to.clone(),
        });
        // Already gone: a replay of a move that had worked.
        let Some(mail) = found else {
            return Ok(None);
        };
        let new_id = format!("{}-moved", id.0);
        server
            .folders
            .entry(to.0.clone())
            .or_default()
            .insert(new_id.clone(), mail);
        Ok((!server.forgets_new_ids).then_some(RemoteId(new_id)))
    }

    async fn append(
        &self,
        folder: &FolderId,
        raw: &[u8],
        flags: Flags,
    ) -> Result<Option<RemoteId>> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        let text = String::from_utf8_lossy(raw).into_owned();
        let subject = text
            .lines()
            .find_map(|line| line.strip_prefix("Subject: "))
            .unwrap_or_default()
            .to_owned();
        let id = format!("appended-{}", server.appended.len() + 1);
        server.appended.push((folder.0.clone(), text.clone()));
        server.folders.entry(folder.0.clone()).or_default().insert(
            id.clone(),
            Mail {
                subject,
                date: NOW_ISH,
                flags,
                body: text,
            },
        );
        Ok(Some(RemoteId(id)))
    }

    async fn watch(&self, tx: Sender<RemoteEvent>) -> Result<()> {
        {
            let mut server = self.0.lock().unwrap();
            server.watches += 1;
            if !server.pushes {
                return Ok(());
            }
            if server.watch_failures > 0 {
                server.watch_failures -= 1;
                return Err(Error::Connection("the line dropped".to_owned()));
            }
            server.listeners.push(tx);
        }
        std::future::pending().await
    }
}

struct Rig {
    backend: MemoryBackend,
    post: Outgoing,
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
                retry: Duration::from_secs(1),
                prefetch_days: 30,
                prefetch_limit: 100,
                turn: Duration::from_secs(60),
            },
        )
    }

    fn start_with(backend: MemoryBackend, options: SyncOptions) -> Self {
        Self::start_ruled(backend, options, Vec::new())
    }

    /// With rules for mail as it arrives.
    fn start_ruled(
        backend: MemoryBackend,
        options: SyncOptions,
        rules: Vec<quill_sync::Rule>,
    ) -> Self {
        Self::start_set(backend, options, quill_sync::RuleSet::of(rules))
    }

    /// With the rules as a whole: switched on or off, and how far back.
    fn start_set(backend: MemoryBackend, options: SyncOptions, rules: quill_sync::RuleSet) -> Self {
        let post = Outgoing::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let mut store = Store::open(&path).unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let (events_tx, events) = unbounded_channel();
        let handle = quill_sync::spawn_with_rules(
            Store::open(&path).unwrap(),
            Some(dir.path().join("blobs")),
            vec![SyncAccount {
                id: account,
                name: "Work".to_owned(),
                backend: Arc::new(backend.clone()),
                sender: Some(Arc::new(post.clone())),
                server_saves_sent: false,
                server_copies: quill_sync::ServerCopies::Keep,
            }],
            options,
            rules,
            events_tx,
        );
        Self {
            backend,
            post,
            store,
            blobs: BlobStore::new(dir.path().join("blobs")),
            account,
            events,
            handle,
            _dir: dir,
        }
    }

    /// Events up to and including the first that `wanted` accepts.
    async fn until(&mut self, wanted: impl Fn(&SyncEvent) -> bool) -> Vec<SyncEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self.next().await;
            let done = wanted(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    /// Waits until the store's queue of operations is empty.
    async fn queue_empty(&mut self) {
        while self.store.queued_operations().unwrap() > 0 {
            self.next().await;
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

    // The UI writes the change to the store's queue, then tells the engine
    // to look.
    rig.store.queue_flags(id, seen, now()).unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.until(|event| *event == SyncEvent::Changed).await;
    assert_eq!(
        rig.backend.with(|server| server.applied.clone()),
        [Operation::SetFlags {
            id: RemoteId("1".to_owned()),
            flags: seen
        }]
    );
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
    // The next pass confirms it and finds nothing to change.
    let events = rig.sync_now().await;
    assert_eq!(changed(&events), 0, "{events:?}");
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

    rig.store.queue_flags(id, seen, now()).unwrap();
    rig.handle.send(SyncRequest::Queue);
    assert_eq!(rig.next().await, SyncEvent::Changed);
    match rig.next().await {
        SyncEvent::Problem { account, message } => {
            assert_eq!(account, "Work");
            assert!(message.contains("put back as it was"), "{message}");
            assert!(message.contains("read-only"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!rig.store.message_location(id).unwrap().unwrap().flags.seen);
    assert_eq!(rig.store.queued_operations().unwrap(), 0);
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
            retry: Duration::from_secs(1),
            interval: Duration::from_millis(50),
            prefetch_days: 30,
            prefetch_limit: 10,
            turn: Duration::from_secs(60),
        },
    );
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Second", NOW_ISH + 1);
    let events = rig.pass().await;
    assert_eq!(changed(&events), 1);
    assert_eq!(rig.subjects("INBOX"), ["Second", "First"]);
}

/// Tells whoever is listening that the inbox changed, once someone is.
async fn push(backend: &MemoryBackend) {
    let listener = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(listener) = backend.with(|server| server.listeners.last().cloned()) {
                return listener;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("nobody listened for changes");
    listener
        .send(RemoteEvent::FolderChanged(FolderId("INBOX".to_owned())))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_the_server_announces_is_fetched_without_waiting_for_the_interval() {
    let backend = MemoryBackend::new(100);
    backend.with(|server| server.pushes = true);
    backend.add("INBOX", "1", "First", NOW_ISH);
    // The interval is an hour: only a push can bring the second message.
    let mut rig = Rig::start(backend);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Second", NOW_ISH + 1);
    push(&rig.backend).await;
    let events = rig.pass().await;
    assert_eq!(changed(&events), 1);
    assert_eq!(rig.subjects("INBOX"), ["Second", "First"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn listening_starts_again_after_the_connection_drops() {
    let backend = MemoryBackend::new(100);
    backend.with(|server| {
        server.pushes = true;
        server.watch_failures = 1;
    });
    backend.add("INBOX", "1", "First", NOW_ISH);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Second", NOW_ISH + 1);
    // Waits for the second attempt, which holds.
    push(&rig.backend).await;
    let events = rig.pass().await;
    assert_eq!(changed(&events), 1);
    assert_eq!(rig.backend.with(|server| server.watches), 2);
    // A dropped line while listening is not a problem to show: the passes
    // say so if the server is really gone.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Problem { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_cannot_push_is_asked_once() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", NOW_ISH);
    let mut rig = Rig::start_with(
        backend,
        SyncOptions {
            retry: Duration::from_secs(1),
            interval: Duration::from_millis(30),
            prefetch_days: 30,
            prefetch_limit: 10,
            turn: Duration::from_secs(60),
        },
    );
    for _ in 0..3 {
        rig.pass().await;
    }
    assert_eq!(rig.backend.with(|server| server.watches), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn new_unread_mail_in_the_inbox_is_announced_and_nothing_else_is() {
    // Small batches: the first sync of the inbox takes several rounds, and
    // none of what it finds is news.
    let backend = MemoryBackend::new(2);
    for n in 1..=5 {
        backend.add("INBOX", &n.to_string(), &format!("Old {n}"), NOW_ISH + n);
    }
    backend.add("Archive", "1", "Filed", NOW_ISH);
    let mut rig = Rig::start(backend);
    let events = rig.pass().await;
    assert_eq!(rig.subjects("INBOX").len(), 5);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Arrived { .. }))
    );

    // One new unread message in the inbox, one that arrives already read,
    // and one in another folder.
    rig.backend.add("INBOX", "6", "News", NOW_ISH + 10);
    rig.backend
        .add("INBOX", "7", "Read elsewhere", NOW_ISH + 11);
    rig.backend.with(|server| {
        server
            .folders
            .get_mut("INBOX")
            .unwrap()
            .get_mut("7")
            .unwrap()
            .flags
            .seen = true;
    });
    rig.backend.add("Archive", "2", "Filed too", NOW_ISH + 12);
    let events = rig.sync_now().await;
    let arrived: Vec<LocalMessageId> = events
        .iter()
        .filter_map(|event| match event {
            SyncEvent::Arrived { account, messages } => {
                assert_eq!(*account, rig.account);
                Some(messages.clone())
            }
            _ => None,
        })
        .flatten()
        .collect();
    let subjects: Vec<String> = rig
        .store
        .message_rows(&arrived)
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect();
    assert_eq!(subjects, ["News"]);

    // Nothing new, nothing said.
    let events = rig.sync_now().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Arrived { .. }))
    );
}

fn rule(matching: &str) -> quill_sync::Rule {
    quill_sync::Rule {
        name: matching.to_owned(),
        queries: vec![quill_core::search::parse(matching, |_, _, _| None)],
        account: None,
        move_to: None,
        delete: false,
        mark_read: false,
        star: false,
        notify: None,
        enabled: true,
        look_back: None,
    }
}

fn hour_long() -> SyncOptions {
    SyncOptions {
        interval: Duration::from_secs(3600),
        retry: Duration::from_secs(1),
        prefetch_days: 30,
        prefetch_limit: 100,
        turn: Duration::from_secs(60),
    }
}

/// The subjects announced in these events.
fn announced(rig: &Rig, events: &[SyncEvent]) -> Vec<String> {
    let ids: Vec<LocalMessageId> = events
        .iter()
        .filter_map(|event| match event {
            SyncEvent::Arrived { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    rig.store
        .message_rows(&ids)
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_file_and_mark_arriving_mail_on_the_server_too() {
    let backend = MemoryBackend::new(100);
    // Long ago: further back than any rule looks.
    backend.add("INBOX", "1", "Already here: build passed", 1_000_000);
    backend.add("Archive", "1", "Old", NOW_ISH);
    let rules = vec![
        quill_sync::Rule {
            move_to: Some("archive".to_owned()),
            mark_read: true,
            ..rule("subject:build")
        },
        quill_sync::Rule {
            star: true,
            ..rule("subject:invoice")
        },
        quill_sync::Rule {
            notify: Some(false),
            ..rule("subject:digest")
        },
    ];
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    rig.pass().await;
    // Old mail that was there before the rules were is left alone: a
    // rule looks back only so far, and is not a way to reorganise a
    // mailbox.
    assert_eq!(rig.subjects("INBOX"), ["Already here: build passed"]);

    rig.backend.add("INBOX", "2", "Build failed", NOW_ISH + 1);
    rig.backend.add("INBOX", "3", "Invoice 114", NOW_ISH + 2);
    rig.backend.add("INBOX", "4", "Daily digest", NOW_ISH + 3);
    rig.backend.add("INBOX", "5", "Lunch?", NOW_ISH + 4);
    let events = rig.sync_now().await;
    rig.queue_empty().await;

    // Filed and read, here and on the server.
    assert!(rig.subjects("Archive").contains(&"Build failed".to_owned()));
    assert!(!rig.subjects("INBOX").contains(&"Build failed".to_owned()));
    rig.backend.with(|server| {
        assert!(!server.folders["INBOX"].contains_key("2"));
        assert!(
            server.folders["Archive"]
                .values()
                .any(|mail| mail.subject == "Build failed")
        );
    });
    // Starred, and still in the inbox.
    let starred = rig
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| row.headers.subject == "Invoice 114")
        .unwrap();
    assert!(starred.flags.flagged);
    rig.backend
        .with(|server| assert!(server.folders["INBOX"]["3"].flags.flagged));

    // Announced: what stays in the inbox, but not what a rule filed
    // elsewhere or was told to keep quiet about.
    let mut said = announced(&rig, &events);
    said.sort();
    assert_eq!(said, ["Invoice 114", "Lunch?"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_can_be_for_one_account_and_says_when_it_cannot_do_its_work() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", NOW_ISH);
    let rules = vec![
        // For another account: never asked.
        quill_sync::Rule {
            account: Some("Personal".to_owned()),
            star: true,
            ..rule("subject:hello")
        },
        // For this one, into a folder it does not have.
        quill_sync::Rule {
            account: Some("work".to_owned()),
            move_to: Some("Receipts".to_owned()),
            ..rule("subject:hello")
        },
        // Filed and still announced, because the rule says so.
        quill_sync::Rule {
            move_to: Some("trash".to_owned()),
            notify: Some(true),
            ..rule("subject:loud")
        },
    ];
    backend.add("Trash", "1", "Old", NOW_ISH);
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Hello there", NOW_ISH + 1);
    rig.backend.add("INBOX", "3", "Loud one", NOW_ISH + 2);
    let events = rig.sync_now().await;
    rig.queue_empty().await;

    let hello = rig
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| row.headers.subject == "Hello there")
        .unwrap();
    assert!(!hello.flags.flagged);
    let problem = events
        .iter()
        .find_map(|event| match event {
            SyncEvent::Problem { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("the missing folder was not mentioned");
    assert!(
        problem.contains("files mail in \"Receipts\", and Work has no such folder"),
        "{problem}"
    );
    assert!(rig.subjects("Trash").contains(&"Loud one".to_owned()));
    let mut said = announced(&rig, &events);
    said.sort();
    assert_eq!(said, ["Hello there", "Loud one"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn old_downloaded_mail_is_let_go_to_keep_within_a_budget_and_comes_back_when_opened() {
    let backend = MemoryBackend::new(100);
    let day = 24 * 60 * 60;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Three old messages and one from today, each downloaded.
    for (id, age) in [("1", 400), ("2", 300), ("3", 200)] {
        backend.add("INBOX", id, &format!("Old {id}"), now - age * day);
    }
    backend.add("INBOX", "4", "Today", now - 60);
    let mut rig = Rig::start(backend);
    rig.pass().await;
    let rows = rig.store.messages(Mailbox::Unified).unwrap();
    for row in &rows {
        rig.handle.send(SyncRequest::FetchBody(row.id));
    }
    for _ in &rows {
        rig.until(|event| matches!(event, SyncEvent::BodyReady(_)))
            .await;
    }
    let has_copy = |rig: &Rig, subject: &str| {
        let row = rig
            .store
            .messages(Mailbox::Unified)
            .unwrap()
            .into_iter()
            .find(|row| row.headers.subject == subject)
            .unwrap();
        rig.blobs
            .get(&rig.store.blob_hash(row.id).unwrap().unwrap_or_default())
            .unwrap()
            .is_some()
    };
    assert!(
        ["Old 1", "Old 2", "Old 3", "Today"]
            .iter()
            .all(|s| has_copy(&rig, s))
    );

    // Plenty of room: nothing goes.
    assert_eq!(rig.handle.trim_cache(1_000_000).await.unwrap(), 0);
    // No room at all: everything old goes, oldest first, and what is
    // recent stays whatever the budget says.
    let freed = rig.handle.trim_cache(0).await.unwrap();
    assert!(freed > 0);
    assert!(!has_copy(&rig, "Old 1") && !has_copy(&rig, "Old 2") && !has_copy(&rig, "Old 3"));
    assert!(has_copy(&rig, "Today"));
    // The text is still here to search, and nothing was lost from the list.
    let old = rig
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| row.headers.subject == "Old 1")
        .unwrap();
    assert!(rig.store.body(old.id).unwrap().is_some());
    assert_eq!(rig.store.messages(Mailbox::Unified).unwrap().len(), 4);

    // Opened again, it is fetched again.
    rig.handle.send(SyncRequest::FetchBody(old.id));
    rig.until(|event| matches!(event, SyncEvent::BodyReady(_)))
        .await;
    assert!(has_copy(&rig, "Old 1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_handle_stops_the_engine() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Only", NOW_ISH);
    let mut rig = Rig::start_with(
        backend,
        SyncOptions {
            retry: Duration::from_secs(1),
            interval: Duration::from_millis(20),
            prefetch_days: 30,
            prefetch_limit: 10,
            turn: Duration::from_secs(60),
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
    rig.handle.set_account(SyncAccount::reading(
        id,
        "Personal",
        Arc::new(personal.clone()),
    ));
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
async fn one_account_is_synced_on_request_and_the_others_are_left_alone() {
    let work = MemoryBackend::new(10);
    work.add("INBOX", "1", "Work mail", NOW_ISH);
    let mut rig = Rig::start(work.clone());
    rig.pass().await;
    let personal = MemoryBackend::new(10);
    personal.add("INBOX", "1", "Personal mail", NOW_ISH);
    let id = rig
        .store
        .upsert_account("Personal", "robin@home.example.com", "imap")
        .unwrap();
    rig.handle.set_account(SyncAccount::reading(
        id,
        "Personal",
        Arc::new(personal.clone()),
    ));
    rig.pass().await;
    let inbox = |rig: &Rig, account| -> Vec<String> {
        rig.store
            .messages(Mailbox::Account(account))
            .unwrap()
            .into_iter()
            .map(|row| row.headers.subject)
            .collect()
    };

    // Mail arrives on both servers. Only the account asked about is asked.
    work.add("INBOX", "2", "More work", NOW_ISH + 1);
    personal.add("INBOX", "2", "More of my own", NOW_ISH + 1);
    assert!(rig.handle.send(SyncRequest::SyncOne(id)));
    let events = rig.pass().await;
    assert!(
        events.iter().all(|event| !matches!(
            event,
            SyncEvent::Syncing { account, .. } if *account == rig.account
        )),
        "{events:?}"
    );
    assert_eq!(inbox(&rig, id), ["More of my own", "Personal mail"]);
    // Given time, the other still has not been: its interval is an hour.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.events.try_recv().is_err());
    assert_eq!(inbox(&rig, rig.account), ["Work mail"]);

    // An account nobody is syncing is nobody to ask, and nothing is.
    assert!(rig.handle.send(SyncRequest::SyncOne(LocalAccountId(9999))));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.events.try_recv().is_err());

    // And the other in its turn, by itself.
    personal.add("INBOX", "3", "Not asked for yet", NOW_ISH + 2);
    assert!(rig.handle.send(SyncRequest::SyncOne(rig.account)));
    let events = rig.pass().await;
    assert!(
        events.iter().all(|event| !matches!(
            event,
            SyncEvent::Syncing { account, .. } if *account == id
        )),
        "{events:?}"
    );
    assert_eq!(inbox(&rig, rig.account), ["More work", "Work mail"]);
    assert_eq!(inbox(&rig, id), ["More of my own", "Personal mail"]);
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
    rig.handle
        .set_account(SyncAccount::reading(rig.account, "Work", Arc::new(new)));
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
    let SyncEvent::Certificate {
        account,
        problem,
        sending: false,
    } = rig.next().await
    else {
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A rig with an inbox of two messages and empty Archive, Trash and Sent,
/// synced.
async fn filing_rig() -> Rig {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Keep", NOW_ISH);
    backend.add("INBOX", "2", "File me", NOW_ISH + 1);
    for folder in ["Archive", "Trash", "Sent"] {
        backend.with(|server| {
            server.folders.entry(folder.to_owned()).or_default();
        });
    }
    let mut rig = Rig::start(backend);
    rig.pass().await;
    rig
}

impl Rig {
    fn folder(&self, name: &str) -> LocalFolderId {
        self.store
            .folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.name == name)
            .unwrap_or_else(|| panic!("no folder {name}"))
            .id
    }

    fn server_subjects(&self, folder: &str) -> Vec<String> {
        self.backend.with(|server| {
            server.folders[folder]
                .values()
                .map(|mail| mail.subject.clone())
                .collect()
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_move_reaches_the_server_and_the_message_keeps_its_body() {
    let mut rig = filing_rig().await;
    let id = rig.local("File me");
    let body = rig.store.body(id).unwrap();
    assert!(body.is_some());
    let archive = rig.folder("Archive");

    rig.store
        .queue_move(id, archive, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;

    assert_eq!(rig.server_subjects("INBOX"), ["Keep"]);
    assert_eq!(rig.server_subjects("Archive"), ["File me"]);
    // The local copy took the server's new name for it, body and all.
    let location = rig.store.message_location(id).unwrap().unwrap();
    assert_eq!(
        (location.folder, location.remote.0.as_str()),
        (archive, "2-moved")
    );
    assert_eq!(rig.store.body(id).unwrap(), body);

    // The next pass agrees: nothing added, nothing doubled.
    rig.sync_now().await;
    assert_eq!(rig.subjects("Archive"), ["File me"]);
    assert_eq!(rig.subjects("INBOX"), ["Keep"]);
    assert_eq!(rig.local("File me"), id);
}

/// A date old enough that the message's body is not fetched ahead.
fn long_ago() -> i64 {
    now() - 400 * 24 * 60 * 60
}

/// A folder on this Mac for the rig's account.
fn local_folder(rig: &mut Rig, name: &str) -> LocalFolderId {
    rig.store
        .create_local_folder(rig.account, name)
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_moved_to_a_folder_on_this_mac_is_here_whole_and_then_off_the_server() {
    let mut rig = filing_rig().await;
    let receipts = local_folder(&mut rig, "Receipts");
    // A message whose body was never fetched: only its headers are here.
    rig.backend.add("INBOX", "3", "Old one", long_ago());
    rig.sync_now().await;
    let id = rig.local("Old one");
    assert_eq!(rig.store.blob_hash(id).unwrap(), None);

    rig.store
        .queue_move(id, receipts, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;

    // The server has let go of it: deleted, not moved anywhere.
    assert_eq!(rig.server_subjects("INBOX"), ["Keep", "File me"]);
    assert!(rig.backend.with(|server| {
        server
            .applied
            .iter()
            .any(|op| matches!(op, Operation::Delete { id } if id.0 == "3"))
    }));
    // And it is here, whole, under a name of the store's own: the same
    // row, in the folder it was moved to.
    let location = rig.store.message_location(id).unwrap().unwrap();
    assert_eq!(location.folder, receipts);
    assert!(quill_store::is_kept(&location.remote.0), "{location:?}");
    let hash = rig.store.blob_hash(id).unwrap().unwrap();
    assert_eq!(location.remote.0, format!("kept:{hash}"));
    let raw = rig.blobs.get(&hash).unwrap().unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("Subject: Old one"));
    assert!(rig.store.body(id).unwrap().is_some());

    // Passes after that leave it be: no server lists the folder, and the
    // folder is not taken for one a server has deleted.
    rig.sync_now().await;
    rig.sync_now().await;
    assert_eq!(rig.subjects("Receipts"), ["Old one"]);
    assert_eq!(rig.subjects("INBOX"), ["File me", "Keep"]);
    assert_eq!(rig.local("Old one"), id);
    let folder = rig
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.id == receipts)
        .unwrap();
    assert!(folder.local);

    // Marking it is done here and asks nothing of the server.
    let asked = rig.backend.with(|server| server.applied.len());
    let seen = Flags {
        seen: true,
        ..Flags::default()
    };
    assert!(rig.store.queue_flags(id, seen, now()).unwrap());
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.backend.with(|server| server.applied.len()), asked);
    assert!(rig.store.message_location(id).unwrap().unwrap().flags.seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_that_cannot_be_had_whole_is_not_taken_off_the_server() {
    let mut rig = filing_rig().await;
    let receipts = local_folder(&mut rig, "Receipts");
    rig.backend.add("INBOX", "3", "Old one", long_ago());
    rig.sync_now().await;
    let id = rig.local("Old one");

    // The server is unreachable: the move waits, and nothing is deleted.
    rig.backend
        .with(|server| server.down = Some("no route to host".to_owned()));
    rig.store
        .queue_move(id, receipts, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.store.queued_operations().unwrap(), 1);
    assert_eq!(rig.server_subjects("INBOX"), ["Keep", "File me", "Old one"]);

    // Back, with the message gone from it meanwhile and never downloaded:
    // there is nothing to keep, so the move is taken back and said.
    rig.backend.with(|server| {
        server.down = None;
        server.folders.get_mut("INBOX").unwrap().remove("3");
    });
    rig.handle.send(SyncRequest::SyncNow);
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(
        message.contains("could not be moved to this Mac"),
        "{message}"
    );
    rig.queue_empty().await;
    assert!(rig.subjects("Receipts").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_moved_from_this_mac_to_a_servers_folder_is_handed_over() {
    let mut rig = filing_rig().await;
    let receipts = local_folder(&mut rig, "Receipts");
    let id = rig.local("File me");
    rig.store
        .queue_move(id, receipts, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server_subjects("INBOX"), ["Keep"]);

    // Between two folders that are only here: nothing is asked of anyone.
    let taxes = local_folder(&mut rig, "Taxes");
    let asked = rig.backend.with(|server| server.applied.len());
    rig.store
        .queue_move(id, taxes, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.backend.with(|server| server.applied.len()), asked);
    assert_eq!(rig.subjects("Taxes"), ["File me"]);
    assert!(quill_store::is_kept(
        &rig.store.message_location(id).unwrap().unwrap().remote.0
    ));

    // And out to the server's archive: the server is given the message,
    // and what it calls it is the message's name from then on.
    let archive = rig.folder("Archive");
    rig.store
        .queue_move(id, archive, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server_subjects("Archive"), ["File me"]);
    let appended = rig.backend.with(|server| server.appended.clone());
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].0, "Archive");
    let location = rig.store.message_location(id).unwrap().unwrap();
    assert_eq!(
        (location.folder, location.remote.0.as_str()),
        (archive, "appended-1")
    );
    // One message, in one place, pass after pass.
    rig.sync_now().await;
    assert_eq!(rig.subjects("Archive"), ["File me"]);
    assert!(rig.subjects("Taxes").is_empty());
    assert!(rig.subjects("Receipts").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_leaves_the_message_where_it_is_and_puts_another_where_asked() {
    let mut rig = filing_rig().await;
    let receipts = local_folder(&mut rig, "Receipts");
    let id = rig.local("File me");

    // To a folder on this Mac: the server is not changed at all.
    let asked = rig.backend.with(|server| server.applied.len());
    rig.store.queue_copy(id, receipts, now()).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.backend.with(|server| server.applied.len()), asked);
    assert_eq!(rig.server_subjects("INBOX"), ["Keep", "File me"]);
    assert_eq!(rig.subjects("Receipts"), ["File me"]);
    assert_eq!(rig.subjects("INBOX"), ["File me", "Keep"]);
    assert_eq!(
        rig.store.message_location(id).unwrap().unwrap().remote.0,
        "2"
    );
    // The copy is a message of its own, with the text.
    let copy = rig
        .store
        .messages(Mailbox::Folder(receipts))
        .unwrap()
        .remove(0);
    assert_ne!(copy.id, id);
    assert_eq!(
        rig.store.body(copy.id).unwrap(),
        rig.store.body(id).unwrap()
    );
    // Copied there twice, it is there once.
    rig.store.queue_copy(id, receipts, now()).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.subjects("Receipts"), ["File me"]);

    // To another of the server's folders: the server copies it.
    let archive = rig.folder("Archive");
    rig.store.queue_copy(id, archive, now()).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server_subjects("Archive"), ["File me"]);
    assert_eq!(rig.server_subjects("INBOX"), ["Keep", "File me"]);
    rig.sync_now().await;
    assert_eq!(rig.subjects("Archive"), ["File me"]);

    // And from this Mac to the server: the server is given a copy, and
    // the one here stays.
    let sent = rig.folder("Sent");
    rig.store.queue_copy(copy.id, sent, now()).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server_subjects("Sent"), ["File me"]);
    assert_eq!(rig.subjects("Receipts"), ["File me"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_asked_for_is_made_on_the_server_and_is_here_at_once() {
    let mut rig = filing_rig().await;
    rig.handle.send(SyncRequest::CreateFolder {
        account: rig.account,
        name: "Projects".to_owned(),
    });
    let events = rig
        .until(|event| matches!(event, SyncEvent::FolderMade { .. }))
        .await;
    assert_eq!(
        events.last(),
        Some(&SyncEvent::FolderMade {
            account: "Work".to_owned(),
            name: "Projects".to_owned(),
        })
    );
    assert!(
        rig.backend
            .with(|server| server.folders.contains_key("Projects"))
    );
    let made = rig
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.name == "Projects")
        .unwrap();
    // A server's folder, not one of this Mac's.
    assert!(!made.local);

    // One the server will not make is said, and nothing is made here.
    rig.handle.send(SyncRequest::CreateFolder {
        account: rig.account,
        name: "Projects".to_owned(),
    });
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(
        message.contains("The folder Projects could not be made on the server"),
        "{message}"
    );
    assert!(message.contains("already a folder"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_whose_new_id_is_not_told_is_found_by_the_sync_that_follows() {
    let mut rig = filing_rig().await;
    rig.backend.with(|server| server.forgets_new_ids = true);
    let id = rig.local("File me");
    let archive = rig.folder("Archive");
    rig.store
        .queue_move(id, archive, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    // The engine syncs by itself to find the message under its new name.
    loop {
        rig.next().await;
        if rig.subjects("Archive") == ["File me"] && rig.store.queued_operations().unwrap() == 0 {
            break;
        }
    }
    assert_eq!(rig.subjects("INBOX"), ["Keep"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_the_server_refuses_is_put_back_and_said() {
    let mut rig = filing_rig().await;
    let id = rig.local("File me");
    let archive = rig.folder("Archive");
    rig.backend.with(|server| server.read_only = true);
    rig.store
        .queue_move(id, archive, now(), now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    assert!(events.contains(&SyncEvent::Changed));
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(
        message.contains("could not be moved") && message.contains("put back"),
        "{message}"
    );
    assert_eq!(rig.subjects("INBOX"), ["File me", "Keep"]);
    assert!(rig.subjects("Archive").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn work_queued_offline_waits_and_goes_when_the_server_is_back() {
    let mut rig = filing_rig().await;
    let id = rig.local("File me");
    let keep = rig.local("Keep");
    let archive = rig.folder("Archive");
    rig.backend
        .with(|server| server.down = Some("no route to host".to_owned()));

    // Two changes made with no connection, in this order.
    rig.store
        .queue_move(id, archive, now(), now())
        .unwrap()
        .unwrap();
    let seen = Flags {
        seen: true,
        ..Flags::default()
    };
    rig.store.queue_flags(keep, seen, now()).unwrap();
    rig.handle.send(SyncRequest::Queue);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Nothing reached the server, nothing was undone, both still wait.
    assert_eq!(rig.store.queued_operations().unwrap(), 2);
    assert_eq!(rig.subjects("Archive"), ["File me"]);
    assert!(
        rig.store
            .message_location(keep)
            .unwrap()
            .unwrap()
            .flags
            .seen
    );
    assert!(rig.backend.with(|server| server.applied.is_empty()));

    // Back online: they go by themselves, in the order made.
    rig.backend.with(|server| server.down = None);
    rig.queue_empty().await;
    assert_eq!(
        rig.backend.with(|server| server.applied.clone()),
        [
            Operation::Move {
                id: RemoteId("2".to_owned()),
                to: FolderId("Archive".to_owned()),
            },
            Operation::SetFlags {
                id: RemoteId("1".to_owned()),
                flags: seen,
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_is_not_sent_before_its_undo_window_closes() {
    let mut rig = filing_rig().await;
    let id = rig.local("File me");
    let trash = rig.folder("Trash");
    let operation = rig
        .store
        .queue_move(id, trash, now(), now() + 3600)
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.backend.with(|server| server.applied.is_empty()));
    // Undone in time, the server never hears of it.
    assert!(rig.store.undo_operation(operation).unwrap());
    rig.handle.send(SyncRequest::Queue);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.backend.with(|server| server.applied.is_empty()));
    assert_eq!(rig.server_subjects("Trash"), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permanent_delete_removes_the_message_from_the_server() {
    let mut rig = filing_rig().await;
    let id = rig.local("File me");
    rig.store.queue_delete(id, now(), now()).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server_subjects("INBOX"), ["Keep"]);
    assert_eq!(rig.subjects("INBOX"), ["Keep"]);
}

/// Files a finished message among the blobs and puts a draft for it in the
/// outbox, due now.
fn outbox(
    rig: &mut Rig,
    subject: &str,
    replied_to: Option<LocalMessageId>,
) -> quill_store::DraftId {
    let raw = format!(
        "From: robin@example.com\r\nTo: ana.ruiz@example.com\r\nSubject: {subject}\r\n\r\nHello.\r\n"
    );
    let hash = rig.blobs.put(raw.as_bytes()).unwrap();
    let content = quill_store::DraftContent {
        to: "ana.ruiz@example.com".to_owned(),
        subject: subject.to_owned(),
        body: "Hello.".to_owned(),
        replied_to: replied_to.map(|id| id.0),
        ..Default::default()
    };
    let draft = rig
        .store
        .save_draft(None, rig.account, &content, now())
        .unwrap()
        .unwrap();
    assert!(
        rig.store
            .queue_send(
                draft,
                &hash,
                "robin@example.com",
                &["ana.ruiz@example.com".to_owned()],
                now(),
                now(),
            )
            .unwrap()
    );
    draft
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_in_the_outbox_is_sent_filed_in_sent_and_marks_the_original_answered() {
    let mut rig = filing_rig().await;
    let original = rig.local("File me");
    outbox(&mut rig, "Re: File me", Some(original));
    rig.handle.send(SyncRequest::Queue);
    let events = rig
        .until(|event| matches!(event, SyncEvent::Sent { .. }))
        .await;
    assert_eq!(
        events.last(),
        Some(&SyncEvent::Sent {
            account: "Work".to_owned()
        })
    );
    rig.queue_empty().await;

    // Exactly once, to the right people, the bytes that were filed.
    let sent = rig.post.0.lock().unwrap().sent.clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.from, "robin@example.com");
    assert_eq!(sent[0].0.to, ["ana.ruiz@example.com"]);
    assert!(sent[0].1.contains("Subject: Re: File me"));
    // Out of the outbox, and a copy saved to Sent on the server.
    assert!(rig.store.outbox().unwrap().is_empty());
    let appended = rig.backend.with(|server| server.appended.clone());
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].0, "Sent");
    assert_eq!(appended[0].1, sent[0].1);
    // The message replied to is marked answered, and read, here and there.
    let flags = rig.store.message_location(original).unwrap().unwrap().flags;
    assert!(flags.answered && flags.seen, "{flags:?}");
    assert!(
        rig.backend
            .with(|server| server.folders["INBOX"]["2"].flags.answered)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_keeps_its_own_sent_copy_is_not_given_a_second() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Hello", NOW_ISH);
    backend.with(|server| {
        server.folders.entry("Sent".to_owned()).or_default();
    });
    let mut rig = Rig::start(backend.clone());
    rig.pass().await;
    let post = rig.post.clone();
    rig.handle.set_account(SyncAccount {
        id: rig.account,
        name: "Work".to_owned(),
        backend: Arc::new(backend),
        sender: Some(Arc::new(post)),
        server_saves_sent: true,
        server_copies: quill_sync::ServerCopies::Keep,
    });
    outbox(&mut rig, "Hi", None);
    rig.handle.send(SyncRequest::Queue);
    rig.until(|event| matches!(event, SyncEvent::Sent { .. }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.backend.with(|server| server.appended.is_empty()));
    assert_eq!(rig.post.0.lock().unwrap().sent.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_server_is_tried_again_and_the_message_goes_once() {
    let mut rig = filing_rig().await;
    rig.post.0.lock().unwrap().failures = vec![
        Error::Temporary("451 try again later".to_owned()),
        Error::Connection("connection reset".to_owned()),
    ];
    let draft = outbox(&mut rig, "Patience", None);
    rig.handle.send(SyncRequest::Queue);

    // Said once, kept in the outbox with the reason, then sent.
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(
        message.contains("will be tried again") && message.contains("451"),
        "{message}"
    );
    let waiting = rig.store.draft(draft).unwrap().unwrap();
    assert_eq!(waiting.state, quill_store::DraftState::Outbox);
    assert!(waiting.error.unwrap().contains("451"));

    let events = rig
        .until(|event| matches!(event, SyncEvent::Sent { .. }))
        .await;
    // The second failure is not said again.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Problem { .. })),
        "{events:?}"
    );
    assert_eq!(rig.post.0.lock().unwrap().sent.len(), 1);
    assert!(rig.store.outbox().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sending_server_whose_certificate_is_not_accepted_is_asked_about() {
    let mut rig = filing_rig().await;
    rig.post.0.lock().unwrap().failures = vec![Error::Certificate(Box::new(
        quill_core::CertificateProblem {
            host: "smtp.example.com".to_owned(),
            reason: "UnknownIssuer".to_owned(),
            fingerprint: Some("cd".repeat(32)),
            certificate: None,
            details: None,
        },
    ))];
    let draft = outbox(&mut rig, "Held", None);
    rig.handle.send(SyncRequest::Queue);

    let events = rig
        .until(|event| matches!(event, SyncEvent::Certificate { .. }))
        .await;
    let Some(SyncEvent::Certificate {
        account,
        problem,
        sending,
    }) = events.last()
    else {
        unreachable!()
    };
    assert_eq!((account.as_str(), *sending), ("Work", true));
    assert_eq!(problem.host, "smtp.example.com");
    // It is a question, not a failure: nothing else is said, and the
    // message keeps its place in the outbox.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Problem { .. })),
        "{events:?}"
    );
    let waiting = rig.store.draft(draft).unwrap().unwrap();
    assert_eq!(waiting.state, quill_store::DraftState::Outbox);
    assert!(waiting.error.unwrap().contains("certificate"));
    assert!(rig.post.0.lock().unwrap().sent.is_empty());

    // Once the certificate is accepted the waiting mail is made due, and
    // it goes.
    assert_eq!(rig.store.hurry_sends(rig.account, now()).unwrap(), 1);
    rig.handle.send(SyncRequest::Queue);
    rig.until(|event| matches!(event, SyncEvent::Sent { .. }))
        .await;
    assert_eq!(rig.post.0.lock().unwrap().sent.len(), 1);
    assert!(rig.store.outbox().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_a_server_refuses_stays_in_the_outbox_with_the_reason() {
    let mut rig = filing_rig().await;
    rig.post.0.lock().unwrap().failures =
        vec![Error::Recipients(vec![quill_core::RefusedRecipient {
            address: "ana.ruiz@example.com".to_owned(),
            reason: "550 no such user".to_owned(),
        }])];
    let draft = outbox(&mut rig, "Bounce", None);
    rig.handle.send(SyncRequest::Queue);
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(message.contains("waiting in the Outbox"), "{message}");
    assert!(
        message.contains("ana.ruiz@example.com (550 no such user)"),
        "{message}"
    );

    let kept = rig.store.draft(draft).unwrap().unwrap();
    assert_eq!(kept.state, quill_store::DraftState::Failed);
    // Not tried again by itself, and nothing saved to Sent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rig.post.0.lock().unwrap().sent.is_empty());
    assert!(rig.backend.with(|server| server.appended.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_with_no_way_to_send_says_so() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Hello", NOW_ISH);
    let mut rig = Rig::start(backend.clone());
    rig.pass().await;
    rig.handle
        .set_account(SyncAccount::reading(rig.account, "Work", Arc::new(backend)));
    let draft = outbox(&mut rig, "Nowhere to go", None);
    rig.handle.send(SyncRequest::Queue);
    let events = rig
        .until(|event| matches!(event, SyncEvent::Problem { .. }))
        .await;
    let Some(SyncEvent::Problem { message, .. }) = events.last() else {
        unreachable!()
    };
    assert!(message.contains("no server to send through"), "{message}");
    // No server refused it: it waits, with the reason, and is not given
    // up on.
    let waiting = rig.store.draft(draft).unwrap().unwrap();
    assert_eq!(waiting.state, quill_store::DraftState::Outbox);
    assert!(
        waiting
            .error
            .as_deref()
            .is_some_and(|error| error.contains("no server to send through")),
        "{waiting:?}"
    );

    // The account is given a server, as when it is edited: what was
    // waiting goes, with nothing more asked. It is the engine that
    // hurries it, once the queue is run with the new settings and by
    // nothing else: the runner that had no server may be in the middle of
    // looking for more to send, and must not be the one to find it.
    let backend = rig.backend.clone();
    rig.handle.set_account(SyncAccount {
        sender: Some(Arc::new(rig.post.clone())),
        ..SyncAccount::reading(rig.account, "Work", Arc::new(backend))
    });
    rig.until(|event| matches!(event, SyncEvent::Sent { .. }))
        .await;
    assert_eq!(rig.post.0.lock().unwrap().sent.len(), 1);
    assert!(rig.store.draft(draft).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_caught_mid_send_by_a_crash_is_held_not_resent() {
    // A store left by a run that stopped while a message was with the
    // server.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quill.db");
    let draft = {
        let mut store = Store::open(&path).unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let content = quill_store::DraftContent::default();
        let draft = store
            .save_draft(None, account, &content, 1)
            .unwrap()
            .unwrap();
        store
            .queue_send(draft, &"ab".repeat(32), "robin@example.com", &[], 1, 1)
            .unwrap();
        store.claim_send(account, 1).unwrap().unwrap();
        draft
    };
    let (events_tx, mut events) = unbounded_channel();
    let _handle = quill_sync::spawn(
        Store::open(&path).unwrap(),
        None,
        Vec::new(),
        SyncOptions::default(),
        events_tx,
    );
    match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
        Ok(Some(SyncEvent::Problem { message, .. })) => {
            assert!(message.contains("check Sent"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.draft(draft).unwrap().unwrap().state,
        quill_store::DraftState::Failed
    );
}

/// The first event that `wanted` picks something out of.
async fn wait_for<T>(rig: &mut Rig, wanted: impl Fn(&SyncEvent) -> Option<T>) -> T {
    loop {
        let event = rig.next().await;
        if let Some(found) = wanted(&event) {
            return found;
        }
    }
}

fn said_by_rules(events: &[SyncEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SyncEvent::Ruled { said } => Some(said.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn catching_up_asks_about_mail_already_here_once_and_only_as_far_back_as_a_rule_looks() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Build passed", NOW_ISH);
    backend.add("INBOX", "2", "Build from long ago", 1_000_000);
    backend.add("INBOX", "3", "Lunch?", NOW_ISH);
    backend.add("Archive", "a1", "Old", NOW_ISH);
    let rules = vec![quill_sync::Rule {
        move_to: Some("archive".to_owned()),
        ..rule("subject:build")
    }];
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    let events = rig.pass().await;
    rig.queue_empty().await;

    // What the inbox held when it was first synced is caught up on,
    // within the look-back: said to the user, and not announced as new.
    assert_eq!(said_by_rules(&events), ["Rules filed 1"]);
    assert!(announced(&rig, &events).is_empty());
    assert!(rig.subjects("Archive").contains(&"Build passed".to_owned()));
    let mut inbox = rig.subjects("INBOX");
    inbox.sort();
    assert_eq!(inbox, ["Build from long ago", "Lunch?"]);

    // Put back in the inbox by the user, it stays: the rules were asked
    // about it once.
    let message = rig
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .chain(
            rig.store
                .search(
                    &quill_core::search::parse("subject:passed", |_, _, _| None),
                    10,
                )
                .unwrap()
                .into_iter()
                .map(|row| row.id),
        )
        .last()
        .unwrap();
    let inbox_folder = rig
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|row| row.name == "INBOX")
        .unwrap()
        .id;
    assert!(
        rig.store
            .queue_move(message, inbox_folder, 0, 0)
            .unwrap()
            .is_some()
    );
    assert!(rig.handle.send(SyncRequest::Queue));
    rig.queue_empty().await;
    let events = rig.sync_now().await;
    assert!(said_by_rules(&events).is_empty());
    assert!(rig.subjects("INBOX").contains(&"Build passed".to_owned()));

    // The rules read again change nothing either: it is still mail they
    // have been asked about.
    rig.handle
        .set_rules(quill_sync::RuleSet::of(vec![quill_sync::Rule {
            move_to: Some("archive".to_owned()),
            ..rule("subject:build")
        }]));
    let events = rig.sync_now().await;
    assert!(said_by_rules(&events).is_empty());
    assert!(rig.subjects("INBOX").contains(&"Build passed".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_that_are_off_do_nothing_and_catch_up_when_switched_on() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", NOW_ISH);
    backend.add("Archive", "1", "Old", NOW_ISH);
    let rules = || {
        vec![
            quill_sync::Rule {
                move_to: Some("archive".to_owned()),
                ..rule("subject:build")
            },
            // Switched off by itself: asked nothing.
            quill_sync::Rule {
                star: true,
                enabled: false,
                ..rule("subject:build")
            },
        ]
    };
    let off = quill_sync::RuleSet {
        on: false,
        ..quill_sync::RuleSet::of(rules())
    };
    let mut rig = Rig::start_set(backend, hour_long(), off);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Build failed", NOW_ISH + 1);
    let events = rig.sync_now().await;
    // Off: the mail arrives, is announced, and stays.
    assert_eq!(announced(&rig, &events), ["Build failed"]);
    assert!(rig.subjects("INBOX").contains(&"Build failed".to_owned()));

    // Switched on, they catch up on what came meanwhile.
    rig.handle.set_rules(quill_sync::RuleSet::of(rules()));
    let said = wait_for(&mut rig, |event| match event {
        SyncEvent::Ruled { said } => Some(said.clone()),
        _ => None,
    })
    .await;
    assert_eq!(said, "Rules filed 1");
    rig.queue_empty().await;
    assert!(rig.subjects("Archive").contains(&"Build failed".to_owned()));
    let filed = rig
        .store
        .search(
            &quill_core::search::parse("subject:failed", |_, _, _| None),
            10,
        )
        .unwrap();
    assert!(!filed[0].flags.flagged, "a rule that is off was asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_that_deletes_moves_mail_to_the_trash_and_writes_down_how_to_take_it_back() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", 1_000_000);
    backend.add("Trash", "1", "Old", NOW_ISH);
    let rules = vec![quill_sync::Rule {
        delete: true,
        mark_read: true,
        ..rule("subject:offer")
    }];
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    rig.pass().await;
    rig.backend
        .add("INBOX", "2", "A special offer", NOW_ISH + 1);
    let events = rig.sync_now().await;
    rig.queue_empty().await;

    assert!(
        rig.subjects("Trash")
            .contains(&"A special offer".to_owned())
    );
    assert!(announced(&rig, &events).is_empty());
    assert_eq!(said_by_rules(&events), ["Rules deleted 1"]);
    // On the server it is in the trash too, not gone.
    rig.backend.with(|server| {
        assert!(!server.folders["INBOX"].contains_key("2"));
        assert!(
            server.folders["Trash"]
                .values()
                .any(|mail| mail.subject == "A special offer")
        );
    });
    // Where it was is noted, so that it can be put back.
    let offer = rig
        .store
        .search(
            &quill_core::search::parse("subject:offer", |_, _, _| None),
            10,
        )
        .unwrap()[0]
        .id;
    let inbox = rig
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|row| row.name == "INBOX")
        .unwrap()
        .id;
    assert_eq!(rig.store.trashed_from(offer).unwrap(), Some(inbox));
    // And it is in the log, under the rule's name, with what is needed
    // to take it back.
    let log = rig.store.agent_log(10, false).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].way, "rule");
    assert_eq!(log[0].agent, "subject:offer");
    assert_eq!(log[0].summary, "deleted 1 message, marked 1 message");
    assert_eq!(log[0].count, 1);
    let undo: serde_json::Value = serde_json::from_str(log[0].undo.as_deref().unwrap()).unwrap();
    assert_eq!(undo["op"], "rule");
    assert_eq!(undo["moved"][0]["id"], offer.0);
    assert_eq!(undo["moved"][0]["from"], inbox.0);
    assert_eq!(undo["moved"][0]["trashed"], true);
    assert_eq!(undo["marked"][0]["flags"], "0000");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_about_what_a_message_says_has_it_downloaded_before_it_is_asked() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", 1_000_000);
    backend.add("Archive", "1", "Old", NOW_ISH);
    // The body of a message here is "Body of SUBJECT.": `body:` looks
    // only there, and finds nothing until the message is downloaded.
    let rules = vec![quill_sync::Rule {
        move_to: Some("archive".to_owned()),
        ..rule("body:newsletter")
    }];
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Newsletter", NOW_ISH + 1);
    rig.backend.add("INBOX", "3", "Lunch?", NOW_ISH + 2);
    let events = rig.sync_now().await;
    rig.queue_empty().await;

    assert!(rig.subjects("Archive").contains(&"Newsletter".to_owned()));
    // Filed before it was announced: only the other is news.
    assert_eq!(announced(&rig, &events), ["Lunch?"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn catching_up_leaves_more_than_it_should_change_alone_for_the_user() {
    let backend = MemoryBackend::new(200);
    for index in 0..=quill_core::rules::CAP {
        backend.add(
            "INBOX",
            &format!("{}", index + 1),
            &format!("Build {index}"),
            NOW_ISH + i64::try_from(index).unwrap(),
        );
    }
    backend.add("Archive", "1", "Old", NOW_ISH);
    let rules = vec![quill_sync::Rule {
        move_to: Some("archive".to_owned()),
        ..rule("subject:build")
    }];
    let mut rig = Rig::start_ruled(backend, hour_long(), rules);
    let events = rig.pass().await;
    let waiting: Vec<(String, usize)> = events
        .iter()
        .filter_map(|event| match event {
            SyncEvent::RulesWaiting { account, count } => Some((account.clone(), *count)),
            _ => None,
        })
        .collect();
    assert_eq!(waiting, [("Work".to_owned(), quill_core::rules::CAP + 1)]);
    assert_eq!(rig.subjects("Archive"), ["Old"]);
    assert_eq!(rig.subjects("INBOX").len(), quill_core::rules::CAP + 1);
    assert!(rig.store.agent_log(10, false).unwrap().is_empty());

    // Said once, not at every pass.
    let events = rig.sync_now().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::RulesWaiting { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_written_later_is_for_mail_from_then_on_and_not_for_what_was_here() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Build passed", NOW_ISH);
    backend.add("Archive", "a1", "Old", NOW_ISH);
    // Rules are on, and there are none yet.
    let mut rig = Rig::start_ruled(backend, hour_long(), Vec::new());
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Build failed", NOW_ISH + 1);
    rig.sync_now().await;

    // The first rule is written. What is in the inbox already is left
    // for the user to run it on: nothing is filed behind their back.
    rig.handle
        .set_rules(quill_sync::RuleSet::of(vec![quill_sync::Rule {
            move_to: Some("archive".to_owned()),
            ..rule("subject:build")
        }]));
    let events = rig.sync_now().await;
    assert!(said_by_rules(&events).is_empty());
    assert_eq!(rig.subjects("INBOX").len(), 2);

    // Mail from then on is filed.
    rig.backend.add("INBOX", "3", "Build fixed", NOW_ISH + 2);
    let events = rig.sync_now().await;
    rig.queue_empty().await;
    assert_eq!(said_by_rules(&events), ["Rules filed 1"]);
    assert_eq!(rig.subjects("INBOX").len(), 2);
    assert!(rig.subjects("Archive").contains(&"Build fixed".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_with_a_mistake_in_them_leave_mail_to_be_asked_about_when_put_right() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "First", 1_000_000);
    backend.add("Archive", "a1", "Old", NOW_ISH);
    // The file could not be read: none of its rules are here.
    let held = quill_sync::RuleSet {
        held: true,
        ..quill_sync::RuleSet::default()
    };
    let mut rig = Rig::start_set(backend, hour_long(), held);
    rig.pass().await;
    rig.backend.add("INBOX", "2", "Build failed", NOW_ISH + 1);
    rig.sync_now().await;
    assert!(rig.subjects("INBOX").contains(&"Build failed".to_owned()));

    // Put right, the rules are asked about what came meanwhile.
    rig.handle
        .set_rules(quill_sync::RuleSet::of(vec![quill_sync::Rule {
            move_to: Some("archive".to_owned()),
            ..rule("subject:build")
        }]));
    let said = wait_for(&mut rig, |event| match event {
        SyncEvent::Ruled { said } => Some(said.clone()),
        _ => None,
    })
    .await;
    assert_eq!(said, "Rules filed 1");
}

/// How many messages of `folder` have been downloaded.
fn downloaded(rig: &Rig, folder: &str) -> usize {
    let folder = rig.folder(folder);
    rig.store
        .messages(Mailbox::Folder(folder))
        .unwrap()
        .into_iter()
        .filter(|row| rig.store.body(row.id).unwrap().is_some())
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_asked_for_while_bodies_download_does_not_wait_for_them_all() {
    let backend = MemoryBackend::new(100);
    for n in 1..=5 {
        backend.add(
            "INBOX",
            &format!("{n}"),
            &format!("Message {n}"),
            NOW_ISH + n,
        );
    }
    // The first download goes through and the second waits: the pass is
    // held in the middle of downloading, as a slow server holds it.
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    backend.with(|server| server.download_gate = Some(Arc::clone(&gate)));
    let mut rig = Rig::start(backend);
    for _ in 0..500 {
        if rig.backend.with(|server| server.downloads) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(rig.backend.with(|server| server.downloads), 2);

    // Mail arrives and a look is asked for, with three bodies still to come.
    rig.backend.add("INBOX", "6", "Late", NOW_ISH + 6);
    assert!(rig.handle.send(SyncRequest::SyncNow));
    gate.add_permits(100);

    // The pass ends with the download it had in hand, and the next one,
    // which nothing more asks for, has the new mail and the rest.
    rig.pass().await;
    assert_eq!(rig.backend.with(|server| server.downloads), 2);
    rig.pass().await;
    assert_eq!(
        rig.subjects("INBOX").first().map(String::as_str),
        Some("Late")
    );
    assert_eq!(downloaded(&rig, "INBOX"), 6);
    assert_eq!(rig.backend.with(|server| server.downloads), 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_folder_met_for_the_first_time_is_left_between_batches_to_look_at_the_inbox() {
    let backend = MemoryBackend::new(2);
    // Old mail: nothing is downloaded ahead, so only the folders are at work.
    for n in 1..=5 {
        backend.add("INBOX", &format!("i{n}"), &format!("Inbox {n}"), 1000 + n);
    }
    for n in 1..=6 {
        backend.add("Archive", &format!("a{n}"), &format!("Filed {n}"), 1000 + n);
    }
    // No time at all for long work: every pass leaves it at the first chance.
    let mut rig = Rig::start_with(
        backend,
        SyncOptions {
            interval: Duration::from_secs(3600),
            retry: Duration::from_secs(1),
            prefetch_days: 30,
            prefetch_limit: 100,
            turn: Duration::ZERO,
        },
    );

    // The inbox comes whole in the first pass, whatever the time allowed:
    // what it held before Quill knew it is not news, and is not announced.
    let mut events = rig.pass().await;
    assert_eq!(changed(&events), 1 + 3 + 1, "{events:?}");
    // The passes after it follow at once, unasked, a batch of the archive each.
    events.extend(rig.pass().await);
    events.extend(rig.pass().await);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Arrived { .. })),
        "{events:?}"
    );
    assert_eq!(rig.subjects("INBOX").len(), 5);
    assert_eq!(rig.subjects("Archive").len(), 6);
    assert_eq!(
        rig.backend.with(|server| server.synced.clone()),
        [
            "INBOX", "INBOX", "INBOX", "Archive", "INBOX", "Archive", "INBOX", "Archive"
        ]
    );
    // And with nothing left to do, the engine rests.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rig.events.recv())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_the_server_will_not_give_does_not_stop_the_folders_after_it() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Kept", NOW_ISH);
    backend.add("Broken", "2", "Out of reach", NOW_ISH);
    backend.add("Trash", "3", "Thrown away", NOW_ISH);
    backend.with(|server| server.refused = Some("Broken".to_owned()));
    let mut rig = Rig::start(backend);

    // The trash is read all the same, and the trouble is said when the
    // pass is over, with the folder's name.
    rig.pass().await;
    assert_eq!(rig.subjects("Trash"), ["Thrown away"]);
    assert_eq!(downloaded(&rig, "Trash"), 1);
    match rig.next().await {
        SyncEvent::Problem { message, .. } => assert!(message.contains("Broken"), "{message}"),
        other => panic!("{other:?}"),
    }

    // And it goes on being read, with the same trouble said once only.
    rig.backend
        .add("Trash", "4", "Thrown away since", NOW_ISH + 1);
    rig.sync_now().await;
    assert_eq!(rig.subjects("Trash"), ["Thrown away since", "Thrown away"]);

    // When the server gives the folder again, it is read.
    rig.backend.with(|server| server.refused = None);
    let events = rig.sync_now().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SyncEvent::Problem { .. })),
        "{events:?}"
    );
    assert_eq!(rig.subjects("Broken"), ["Out of reach"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_inbox_the_server_will_not_give_stops_the_pass_as_before() {
    let backend = MemoryBackend::new(100);
    backend.add("INBOX", "1", "Kept", NOW_ISH);
    backend.add("Trash", "3", "Thrown away", NOW_ISH);
    backend.with(|server| server.refused = Some("INBOX".to_owned()));
    let mut rig = Rig::start(backend);
    rig.pass().await;
    assert!(matches!(rig.next().await, SyncEvent::Problem { .. }));
    assert_eq!(rig.backend.with(|server| server.synced.clone()), ["INBOX"]);
}
