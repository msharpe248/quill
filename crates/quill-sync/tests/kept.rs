//! Accounts whose mail is kept here: a server with one mailbox and no
//! folders or marks, as POP3 is. The engine files and marks locally,
//! downloads everything, and deletes from the server only as the account's
//! policy says, and only what is safely here.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use quill_core::types::{
    Capabilities, Flags, FolderId, FolderRole, Headers, LocalAccountId, Operation, RemoteEvent,
    RemoteFolder, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Envelope, Error, Result};
use quill_store::{Mailbox, Store};
use quill_sync::{ServerCopies, SyncAccount, SyncEvent, SyncHandle, SyncOptions, SyncRequest};
use tokio::sync::mpsc::{Sender, UnboundedReceiver, unbounded_channel};

const DAY: i64 = 24 * 60 * 60;

/// One mailbox, ids that never change, and nothing else: what a POP3
/// server is.
#[derive(Clone, Default)]
struct Mailbox1(Arc<Mutex<Server>>);

#[derive(Default)]
struct Server {
    /// id -> raw message
    mail: BTreeMap<String, String>,
    down: bool,
    /// How many sessions were opened to fetch whole messages.
    fetch_sessions: usize,
    /// Ids asked to be deleted, in order, whether or not they were there.
    deleted: Vec<String>,
    /// Ids that vanish between being listed and being fetched.
    vanishing: Vec<String>,
}

impl Mailbox1 {
    fn deliver(&self, id: &str, subject: &str) {
        self.0.lock().unwrap().mail.insert(
            id.to_owned(),
            format!(
                "From: Ana Ruiz <ana.ruiz@example.com>\r\nTo: robin@example.com\r\n\
                 Subject: {subject}\r\nMessage-ID: <{id}@example.com>\r\n\r\nBody of {subject}.\r\n"
            ),
        );
    }

    fn with<T>(&self, change: impl FnOnce(&mut Server) -> T) -> T {
        change(&mut self.0.lock().unwrap())
    }

    fn on_server(&self) -> Vec<String> {
        self.with(|server| server.mail.keys().cloned().collect())
    }

    fn check(&self) -> Result<()> {
        if self.0.lock().unwrap().down {
            return Err(Error::Connection("connection refused".to_owned()));
        }
        Ok(())
    }
}

#[async_trait]
impl Backend for Mailbox1 {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: false,
            flags: false,
            push: false,
            server_search: false,
            threads: false,
        }
    }

    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        Ok(vec![RemoteFolder {
            id: FolderId("INBOX".to_owned()),
            name: "Inbox".to_owned(),
            role: FolderRole::Inbox,
        }])
    }

    /// The cursor is the ids seen so far, one to a line.
    async fn sync_folder(
        &self,
        _folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        self.check()?;
        let server = self.0.lock().unwrap();
        let seen: Vec<String> = cursor
            .map(|cursor| cursor.0.lines().map(str::to_owned).collect())
            .unwrap_or_default();
        let added = server
            .mail
            .iter()
            .filter(|(id, _)| !seen.contains(id))
            .map(|(id, raw)| {
                let subject = raw
                    .lines()
                    .find_map(|line| line.strip_prefix("Subject: "))
                    .unwrap_or_default();
                RemoteMessage {
                    id: RemoteId(id.clone()),
                    headers: Headers {
                        message_id: Some(format!("<{id}@example.com>")),
                        subject: subject.to_owned(),
                        from: Vec::new(),
                        to: Vec::new(),
                        cc: Vec::new(),
                        date: 1_000,
                    },
                    flags: Flags::default(),
                    size: raw.len() as u64,
                    has_attachments: false,
                    thread: Vec::new(),
                }
            })
            .collect();
        let removed = seen
            .iter()
            .filter(|id| !server.mail.contains_key(*id))
            .map(|id| RemoteId(id.clone()))
            .collect();
        Ok(SyncDelta {
            reset: false,
            added,
            changed: Vec::new(),
            removed,
            has_more: false,
            next_cursor: SyncCursor(server.mail.keys().cloned().collect::<Vec<_>>().join("\n")),
        })
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        self.fetch_many(std::slice::from_ref(id))
            .await?
            .pop()
            .flatten()
            .ok_or_else(|| Error::NotFound(id.0.clone()))
    }

    async fn fetch_many(&self, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        server.fetch_sessions += 1;
        let vanishing = std::mem::take(&mut server.vanishing);
        for id in &vanishing {
            server.mail.remove(id);
        }
        Ok(ids
            .iter()
            .map(|id| server.mail.get(&id.0).map(|raw| Bytes::from(raw.clone())))
            .collect())
    }

    async fn apply(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::Delete { id } => self.delete_many(std::slice::from_ref(id)).await,
            _ => Err(Error::Unsupported(
                "only mail, no folders or marks".to_owned(),
            )),
        }
    }

    async fn delete_many(&self, ids: &[RemoteId]) -> Result<()> {
        self.check()?;
        let mut server = self.0.lock().unwrap();
        for id in ids {
            server.deleted.push(id.0.clone());
            server.mail.remove(&id.0);
        }
        Ok(())
    }

    async fn watch(&self, _tx: Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }
}

/// A sender that takes everything.
struct Post;

#[async_trait]
impl quill_core::Sender for Post {
    async fn send(&self, _raw: &[u8], _envelope: &Envelope) -> Result<()> {
        Ok(())
    }
}

struct Rig {
    server: Mailbox1,
    store: Store,
    account: LocalAccountId,
    events: UnboundedReceiver<SyncEvent>,
    handle: SyncHandle,
    dir: tempfile::TempDir,
}

impl Rig {
    fn start(server: Mailbox1, policy: ServerCopies) -> Self {
        Self::open(server, policy, tempfile::tempdir().unwrap())
    }

    /// Starts the engine on the store in `dir`, as a run of Quill does.
    fn open(server: Mailbox1, policy: ServerCopies, dir: tempfile::TempDir) -> Self {
        let path = dir.path().join("quill.db");
        let blobs = quill_store::BlobStore::new(dir.path().join("blobs"));
        let mut store = Store::open(&path).unwrap().with_blobs(blobs);
        let account = store
            .upsert_account("Old Mail", "robin@example.com", "pop3")
            .unwrap();
        let (events_tx, events) = unbounded_channel();
        let handle = quill_sync::spawn(
            Store::open(&path).unwrap(),
            Some(dir.path().join("blobs")),
            vec![SyncAccount {
                id: account,
                name: "Old Mail".to_owned(),
                backend: Arc::new(server.clone()),
                sender: Some(Arc::new(Post)),
                server_saves_sent: false,
                server_copies: policy,
            }],
            SyncOptions {
                interval: Duration::from_secs(3600),
                retry: Duration::from_secs(1),
                prefetch_days: 30,
                prefetch_limit: 100,
                turn: Duration::from_secs(60),
            },
            events_tx,
        );
        Self {
            server,
            store,
            account,
            events,
            handle,
            dir,
        }
    }

    /// Quits and starts again on the same store.
    async fn restart(self, policy: ServerCopies) -> Self {
        let Self {
            server,
            dir,
            handle,
            mut events,
            store,
            ..
        } = self;
        drop(handle);
        drop(store);
        while events.recv().await.is_some() {}
        let mut rig = Self::open(server, policy, dir);
        rig.pass().await;
        rig
    }

    async fn next(&mut self) -> SyncEvent {
        tokio::time::timeout(Duration::from_secs(5), self.events.recv())
            .await
            .expect("timed out waiting for the engine")
            .expect("the engine stopped")
    }

    /// Waits out the pass in progress.
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

    /// Asks for a pass and waits it out.
    async fn poll(&mut self) -> Vec<SyncEvent> {
        self.handle.send(SyncRequest::SyncNow);
        self.pass().await
    }

    async fn queue_empty(&mut self) {
        while self.store.queued_operations().unwrap() > 0 {
            self.next().await;
        }
    }

    fn folder(&self, role: FolderRole) -> quill_core::types::LocalFolderId {
        self.store
            .folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.account == self.account && folder.role == role)
            .unwrap_or_else(|| panic!("no {role:?} folder"))
            .id
    }

    fn subjects(&self, role: FolderRole) -> Vec<String> {
        let mut subjects: Vec<String> = self
            .store
            .messages(Mailbox::Folder(self.folder(role)))
            .unwrap()
            .into_iter()
            .map(|row| row.headers.subject)
            .collect();
        subjects.sort();
        subjects
    }

    fn id(&self, role: FolderRole, subject: &str) -> quill_core::types::LocalMessageId {
        self.store
            .messages(Mailbox::Folder(self.folder(role)))
            .unwrap()
            .into_iter()
            .find(|row| row.headers.subject == subject)
            .unwrap_or_else(|| panic!("{subject} is not in {role:?}"))
            .id
    }

    /// Every message of the account, in every folder: for "none lost, none
    /// twice".
    fn everything(&self) -> Vec<String> {
        let mut all = Vec::new();
        for folder in self.store.folders().unwrap() {
            if folder.account == self.account {
                all.extend(
                    self.store
                        .messages(Mailbox::Folder(folder.id))
                        .unwrap()
                        .into_iter()
                        .map(|row| row.headers.subject),
                );
            }
        }
        all.sort();
        all
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }
}

fn three() -> Mailbox1 {
    let server = Mailbox1::default();
    server.deliver("a", "One");
    server.deliver("b", "Two");
    server.deliver("c", "Three");
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn the_account_gets_folders_of_its_own_and_every_message_whole() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    assert_eq!(rig.subjects(FolderRole::Inbox), ["One", "Three", "Two"]);
    // Sent, Archive and Trash exist here though the server has none.
    for role in [FolderRole::Sent, FolderRole::Archive, FolderRole::Trash] {
        assert!(rig.subjects(role).is_empty());
    }
    // All of it was downloaded, however old, in one session.
    for subject in ["One", "Two", "Three"] {
        let id = rig.id(FolderRole::Inbox, subject);
        assert!(rig.store.raw(id).unwrap().is_some(), "{subject}");
    }
    assert_eq!(rig.server.with(|server| server.fetch_sessions), 1);
    // And nothing was taken off the server.
    assert_eq!(rig.server.on_server(), ["a", "b", "c"]);
    assert_eq!(rig.store.server_copies(rig.account).unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn left_on_the_server_repeated_polls_bring_each_message_once() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    for round in 0..3 {
        rig.server
            .deliver(&format!("n{round}"), &format!("New {round}"));
        rig.poll().await;
        rig.poll().await;
    }
    assert_eq!(
        rig.everything(),
        ["New 0", "New 1", "New 2", "One", "Three", "Two"]
    );
    assert_eq!(rig.server.on_server().len(), 6);
    assert!(rig.server.with(|server| server.deleted.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_when_downloaded_the_server_is_emptied_and_nothing_is_lost() {
    let mut rig = Rig::start(three(), ServerCopies::DeleteWhenDownloaded);
    rig.pass().await;
    assert!(rig.server.on_server().is_empty());
    assert_eq!(rig.everything(), ["One", "Three", "Two"]);
    assert_eq!(rig.store.server_copies(rig.account).unwrap(), 0);

    // More arrives, between polls and during them; each comes once, and the
    // mail already here stays although the server no longer has it.
    for round in 0..3 {
        rig.server
            .deliver(&format!("n{round}"), &format!("New {round}"));
        rig.poll().await;
        rig.poll().await;
    }
    assert_eq!(
        rig.everything(),
        ["New 0", "New 1", "New 2", "One", "Three", "Two"]
    );
    assert!(rig.server.on_server().is_empty());
    // Each was deleted once, and still reads whole.
    let mut deleted = rig.server.with(|server| server.deleted.clone());
    deleted.sort();
    assert_eq!(deleted, ["a", "b", "c", "n0", "n1", "n2"]);
    let id = rig.id(FolderRole::Inbox, "One");
    assert!(rig.store.raw(id).unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_after_some_days_only_what_is_old_enough_goes() {
    let mut rig = Rig::start(three(), ServerCopies::DeleteAfterDays(14));
    rig.pass().await;
    // Just arrived: all still there.
    assert_eq!(rig.server.on_server(), ["a", "b", "c"]);

    // Two weeks on for one of them, as far as the store knows.
    let path = rig.dir.path().join("quill.db");
    let old = Rig::now() - 15 * DAY;
    backdate(&path, "b", old);
    rig.server.deliver("d", "Four");
    rig.poll().await;
    assert_eq!(rig.server.on_server(), ["a", "c", "d"]);
    assert_eq!(rig.server.with(|server| server.deleted.clone()), ["b"]);
    // It went from the server only. Polls after that change nothing.
    rig.poll().await;
    rig.poll().await;
    assert_eq!(rig.everything(), ["Four", "One", "Three", "Two"]);
    assert_eq!(rig.server.on_server(), ["a", "c", "d"]);
    assert_eq!(rig.server.with(|server| server.deleted.clone()), ["b"]);
}

/// Backdates when a message was first seen on the server, as time passing
/// would.
fn backdate(path: &std::path::Path, remote: &str, at: i64) {
    let status = std::process::Command::new("sqlite3")
        .arg(path)
        .arg(format!(
            "UPDATE server_copies SET seen_at = {at} WHERE remote_id = '{remote}';"
        ))
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_deleted_from_the_server_that_is_not_safely_here() {
    let mut rig = Rig::start(three(), ServerCopies::DeleteWhenDownloaded);
    rig.pass().await;
    assert!(rig.server.on_server().is_empty());

    rig.server.deliver("d", "Four");
    rig.server.deliver("e", "Five");
    // "d" vanishes before it can be fetched: another program took it.
    rig.server
        .with(|server| server.vanishing = vec!["d".to_owned()]);
    rig.poll().await;
    // What never arrived leaves no row that cannot be opened; what did
    // arrive is here whole, and only then gone from the server.
    assert_eq!(rig.everything(), ["Five", "One", "Three", "Two"]);
    assert!(rig.server.on_server().is_empty());
    assert!(
        !rig.server
            .with(|server| server.deleted.contains(&"d".to_owned()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_another_program_deleted_from_the_server_stays_here() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    rig.server.with(|server| server.mail.remove("b"));
    rig.poll().await;
    rig.poll().await;
    assert_eq!(rig.everything(), ["One", "Three", "Two"]);
    assert_eq!(rig.store.server_copies(rig.account).unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn filing_and_marking_happen_here_and_survive_polls_and_restarts() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    let archive = rig.folder(FolderRole::Archive);
    let one = rig.id(FolderRole::Inbox, "One");
    let two = rig.id(FolderRole::Inbox, "Two");
    let at = Rig::now();
    rig.store.queue_move(one, archive, at, at).unwrap().unwrap();
    rig.store
        .queue_flags(
            two,
            Flags {
                seen: true,
                flagged: true,
                ..Flags::default()
            },
            at,
        )
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    // The server was asked for nothing, and nothing was put back.
    assert_eq!(rig.subjects(FolderRole::Archive), ["One"]);
    assert_eq!(rig.subjects(FolderRole::Inbox), ["Three", "Two"]);

    // The server still lists the archived message. It does not come back
    // to the inbox, now or after quitting and starting again.
    rig.poll().await;
    let mut rig = rig.restart(ServerCopies::Keep).await;
    rig.poll().await;
    assert_eq!(rig.subjects(FolderRole::Archive), ["One"]);
    assert_eq!(rig.subjects(FolderRole::Inbox), ["Three", "Two"]);
    assert_eq!(rig.everything(), ["One", "Three", "Two"]);
    let two = rig.id(FolderRole::Inbox, "Two");
    let flags = rig.store.message_location(two).unwrap().unwrap().flags;
    assert!(flags.seen && flags.flagged);
    // The archived message still opens.
    let one = rig.id(FolderRole::Archive, "One");
    assert!(rig.store.raw(one).unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_for_good_takes_the_servers_copy_too() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    let one = rig.id(FolderRole::Inbox, "One");
    let at = Rig::now();
    rig.store.queue_delete(one, at, at).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.server.on_server(), ["b", "c"]);
    // And it does not come back.
    rig.poll().await;
    assert_eq!(rig.everything(), ["Three", "Two"]);
    assert_eq!(rig.store.server_copies(rig.account).unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_sent_mail_is_filed_here() {
    let mut rig = Rig::start(three(), ServerCopies::Keep);
    rig.pass().await;
    let raw = "From: Robin Vale <robin@example.com>\r\nTo: Ana Ruiz <ana.ruiz@example.com>\r\n\
               Subject: Thanks\r\nMessage-ID: <sent-1@example.com>\r\n\r\nThanks, Ana.\r\n";
    let blobs = quill_store::BlobStore::new(rig.dir.path().join("blobs"));
    let hash = blobs.put(raw.as_bytes()).unwrap();
    let sent = rig.folder(FolderRole::Sent);
    let seen = Flags {
        seen: true,
        ..Flags::default()
    };
    rig.store
        .queue_append(sent, &hash, seen, Rig::now())
        .unwrap()
        .unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert_eq!(rig.subjects(FolderRole::Sent), ["Thanks"]);
    let id = rig.id(FolderRole::Sent, "Thanks");
    assert_eq!(rig.store.raw(id).unwrap().unwrap(), raw.as_bytes());
    assert!(rig.store.message_location(id).unwrap().unwrap().flags.seen);
    // Polling leaves it be, and deleting it for good asks the server for
    // nothing: the server never had it.
    rig.poll().await;
    assert_eq!(rig.subjects(FolderRole::Sent), ["Thanks"]);
    let at = Rig::now();
    rig.store.queue_delete(id, at, at).unwrap().unwrap();
    rig.handle.send(SyncRequest::Queue);
    rig.queue_empty().await;
    assert!(rig.subjects(FolderRole::Sent).is_empty());
    assert!(rig.server.with(|server| server.deleted.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_kept_here_is_never_let_go_to_make_room() {
    // The messages are dated long ago, and the budget is nothing at all.
    let mut rig = Rig::start(three(), ServerCopies::DeleteWhenDownloaded);
    rig.pass().await;
    assert!(rig.server.on_server().is_empty());
    assert_eq!(rig.handle.trim_cache(0).await.unwrap(), 0);
    for subject in ["One", "Two", "Three"] {
        let id = rig.id(FolderRole::Inbox, subject);
        assert!(rig.store.raw(id).unwrap().is_some(), "{subject}");
    }
}
