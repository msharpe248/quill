//! The sync engine.
//!
//! It runs each account's [`Backend`] into the local store in the
//! background: folders, then each folder's messages in batches, then the
//! bodies of recent mail. The UI never waits for it. It reads the store,
//! hears a [`SyncEvent`] when the store has changed, and asks for what only
//! the network can give with a [`SyncRequest`].
//!
//! It also carries out what the user did while it was not looking. Moves,
//! deletions, flag changes and sent copies wait in the store's queue and
//! are replayed against the server in the order made; mail to send waits
//! in the outbox. Both survive a restart and a spell offline.
//!
//! Nothing here knows which protocol an account speaks.

pub mod rules;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quill_core::types::{
    Flags, FolderId, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId, Operation,
    RemoteEvent, RemoteFolder, RemoteId, RemoteMessage,
};
use quill_core::{Backend, CertificateProblem, Envelope, Error, Sender};
use quill_store::{BlobStore, Outgoing, QueuedKind, QueuedOperation, Store};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

pub use crate::rules::Rule;

/// An account for the engine to keep in sync.
pub struct SyncAccount {
    pub id: LocalAccountId,
    pub name: String,
    pub backend: Arc<dyn Backend>,
    /// How the account sends mail. Without one, mail for it waits in the
    /// outbox with the reason.
    pub sender: Option<Arc<dyn Sender>>,
    /// The server files its own copy of what is sent through it (Gmail,
    /// Microsoft Graph), so Quill must not save a second one.
    pub server_saves_sent: bool,
    /// For an account whose mail is kept here (POP3): what becomes of the
    /// server's copy.
    pub server_copies: ServerCopies,
}

/// What becomes of the server's copy of a message, for an account whose
/// mail is kept on this Mac and whose server only hands it over. Nothing is
/// ever deleted from the server before the whole message is safely here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerCopies {
    /// Left where it is. Other mail programs see the same mail.
    #[default]
    Keep,
    /// Deleted once the message has been downloaded.
    DeleteWhenDownloaded,
    /// Deleted this many days after Quill first saw it.
    DeleteAfterDays(u32),
}

/// The start of the ids of folders, and of messages, that exist only in
/// the store. No server issues such an id.
pub const KEPT: &str = "kept:";

/// The folders Quill keeps for an account whose server has only an inbox.
fn kept_folders() -> Vec<RemoteFolder> {
    [
        ("sent", "Sent", FolderRole::Sent),
        ("archive", "Archive", FolderRole::Archive),
        ("trash", "Trash", FolderRole::Trash),
    ]
    .into_iter()
    .map(|(id, name, role)| RemoteFolder {
        id: FolderId(format!("{KEPT}{id}")),
        name: name.to_owned(),
        role,
    })
    .collect()
}

impl SyncAccount {
    /// An account that only reads mail.
    pub fn reading(id: LocalAccountId, name: impl Into<String>, backend: Arc<dyn Backend>) -> Self {
        Self {
            id,
            name: name.into(),
            backend,
            sender: None,
            server_saves_sent: false,
            server_copies: ServerCopies::Keep,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    /// How long to wait between passes when nothing asks for one sooner.
    pub interval: Duration,
    /// How long to wait before trying again something the server could not
    /// take just now. Doubled each time it fails again, up to half an hour.
    pub retry: Duration,
    /// Bodies of mail newer than this many days are downloaded ahead of
    /// being opened, so that recent mail reads offline.
    pub prefetch_days: i64,
    /// The most bodies one pass downloads ahead.
    pub prefetch_limit: usize,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5 * 60),
            retry: Duration::from_secs(30),
            prefetch_days: 30,
            prefetch_limit: 2000,
        }
    }
}

/// What the engine tells the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    /// The store changed; views of it should reload.
    Changed,
    /// An account began (`true`) or finished (`false`) a pass.
    Syncing {
        account: LocalAccountId,
        active: bool,
    },
    /// New, unread mail came into an inbox since the last look. Not said
    /// for the mail found the first time a folder is synced.
    Arrived {
        account: LocalAccountId,
        messages: Vec<LocalMessageId>,
    },
    /// A body that was asked for is now in the store.
    BodyReady(LocalMessageId),
    /// A body that was asked for could not be fetched.
    BodyFailed { id: LocalMessageId, reason: String },
    /// A message left the outbox: a server accepted it.
    Sent { account: String },
    /// Something went wrong that the user should see.
    Problem { account: String, message: String },
    /// An account's server presented a certificate that was not accepted.
    /// Nothing was sent to it. The user is shown the certificate and
    /// decides: this is never settled quietly.
    Certificate {
        account: String,
        problem: Box<CertificateProblem>,
        /// It was the server mail is sent through, not the one it is
        /// read from.
        sending: bool,
    },
}

/// What the UI asks of the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRequest {
    /// Sync every account now instead of at the next interval.
    SyncNow,
    /// Sync this account now, and leave the others to their own time. An
    /// account that is not being synced is not asked.
    SyncOne(LocalAccountId),
    /// Download a message's body. Answered by `BodyReady` or `BodyFailed`.
    FetchBody(LocalMessageId),
    /// The store's queue or outbox has something new: a change the user
    /// made, or mail to send. Look now instead of at the next due time.
    Queue,
}

/// The UI's end of the engine. Dropping it stops the engine.
pub struct SyncHandle {
    requests: UnboundedSender<SyncRequest>,
    engine: Arc<Engine>,
    server: JoinHandle<()>,
}

impl SyncHandle {
    /// Queues a request. Returns `false` when the engine has stopped.
    pub fn send(&self, request: SyncRequest) -> bool {
        self.requests.send(request).is_ok()
    }

    /// Starts syncing `account`, in place of any account with the same id:
    /// a new account, or an old one whose settings changed. Must be called
    /// on the engine's Tokio runtime.
    pub fn set_account(&self, account: SyncAccount) {
        Engine::set_account(&self.engine, account);
    }

    /// Stops syncing an account. Its mail stays in the store.
    pub fn remove_account(&self, id: LocalAccountId) {
        self.engine.remove_account(id);
    }

    /// Sets how many bytes downloaded mail may take, and has the engine
    /// keep to it from now on: soon after this is called, and every few
    /// hours after. Must be called on the engine's Tokio runtime.
    pub fn keep_cache_within(&self, bytes: u64) {
        let engine = Arc::downgrade(&self.engine);
        tokio::spawn(async move {
            // After the first pass has had its turn at the disk.
            tokio::time::sleep(CACHE_FIRST_LOOK).await;
            loop {
                let Some(engine) = engine.upgrade() else {
                    return;
                };
                if let Err(problem) = engine.trim_cache(bytes).await {
                    engine.emit(SyncEvent::Problem {
                        account: "Storage".to_owned(),
                        message: format!("Old downloaded mail could not be cleared: {problem}"),
                    });
                }
                drop(engine);
                tokio::time::sleep(CACHE_LOOK_EVERY).await;
            }
        });
    }

    /// Lets downloaded copies go, oldest first, until they come to no more
    /// than `bytes`. Returns how many bytes were freed. What is let go is
    /// fetched again when it is next opened.
    pub async fn trim_cache(&self, bytes: u64) -> Result<u64, String> {
        self.engine.trim_cache(bytes).await
    }
}

/// How long after starting the cache is first measured.
const CACHE_FIRST_LOOK: Duration = Duration::from_secs(120);
/// And how often after that.
const CACHE_LOOK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Mail newer than this is never let go: it would only be fetched again by
/// the next pass, which keeps recent mail readable offline.
const CACHE_KEEPS_DAYS: i64 = 45;

impl Drop for SyncHandle {
    fn drop(&mut self) {
        self.server.abort();
        self.engine.stop();
    }
}

/// Starts the engine on the current Tokio runtime.
///
/// `store` is the engine's own connection to the database. `blobs`, when
/// given, is where raw messages are kept; without it only their text is.
pub fn spawn(
    store: Store,
    blobs: Option<PathBuf>,
    accounts: Vec<SyncAccount>,
    options: SyncOptions,
    events: UnboundedSender<SyncEvent>,
) -> SyncHandle {
    spawn_with_rules(store, blobs, accounts, options, Vec::new(), events)
}

/// As [`spawn`], with rules for mail as it arrives. They are there from
/// the first pass: mail is never looked at before its rules are known.
pub fn spawn_with_rules(
    store: Store,
    blobs: Option<PathBuf>,
    accounts: Vec<SyncAccount>,
    options: SyncOptions,
    rules: Vec<Rule>,
    events: UnboundedSender<SyncEvent>,
) -> SyncHandle {
    let (requests, queue) = unbounded_channel();
    let engine = Arc::new(Engine {
        store: Arc::new(Mutex::new(store)),
        blobs: blobs.map(|root| Arc::new(BlobStore::new(root))),
        options,
        rules,
        events,
        accounts: RwLock::new(HashMap::new()),
    });
    // Mail that was mid-send when Quill last stopped may or may not have
    // gone. It is not sent again without the user's say.
    if let Ok(mut store) = engine.store.lock()
        && let Ok(interrupted) = store.recover_interrupted_sends()
        && interrupted > 0
    {
        engine.emit(SyncEvent::Problem {
            account: "Outbox".to_owned(),
            message: "Quill stopped while sending. What was being sent is in the Outbox; check Sent before sending it again.".to_owned(),
        });
    }
    for account in accounts {
        Engine::set_account(&engine, account);
    }
    // Mail downloaded before conversations were kept is put into them.
    // The engine is held weakly: this must not keep it alive.
    tokio::spawn({
        let engine = Arc::downgrade(&engine);
        async move {
            loop {
                let Some(engine) = engine.upgrade() else {
                    return;
                };
                match engine.thread_some().await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => engine.emit(SyncEvent::Changed),
                }
            }
        }
    });
    let server = tokio::spawn(serve(Arc::clone(&engine), queue));
    SyncHandle {
        requests,
        engine,
        server,
    }
}

async fn serve(engine: Arc<Engine>, mut queue: UnboundedReceiver<SyncRequest>) {
    while let Some(request) = queue.recv().await {
        match request {
            // `notify_one` keeps the request for an account that is mid-pass:
            // it goes round again as soon as it finishes.
            SyncRequest::SyncNow => engine
                .running()
                .iter()
                .for_each(|account| account.wake.notify_one()),
            // The same wake as for all of them, for the one named: its
            // loop makes the pass it would have made at its next turn.
            SyncRequest::SyncOne(id) => {
                if let Some(account) = engine.account(id) {
                    account.wake.notify_one();
                }
            }
            // Each on its own task: a slow download must not hold up the
            // next request. The backend serialises what shares a connection.
            SyncRequest::FetchBody(id) => {
                let engine = Arc::clone(&engine);
                tokio::spawn(async move { engine.fetch_body_for_ui(id).await });
            }
            SyncRequest::Queue => engine
                .running()
                .iter()
                .for_each(|account| account.queue.notify_one()),
        }
    }
}

struct Engine {
    store: Arc<Mutex<Store>>,
    blobs: Option<Arc<BlobStore>>,
    /// The accounts being kept in sync. Accounts come and go while the
    /// engine runs: one is added from the setup flow, or replaced when its
    /// settings change.
    accounts: RwLock<HashMap<LocalAccountId, Arc<Running>>>,
    options: SyncOptions,
    /// What to do with mail as it arrives.
    rules: Vec<Rule>,
    events: UnboundedSender<SyncEvent>,
}

/// An account and the loop that syncs it.
struct Running {
    account: SyncAccount,
    /// Wakes the loop before the interval is up.
    wake: Notify,
    /// Wakes the queue runner before its next due time.
    queue: Notify,
    /// The sync loop and the queue runner, to stop them when the account is
    /// replaced or removed.
    task: Mutex<Vec<JoinHandle<()>>>,
}

impl Running {
    fn stop(&self) {
        if let Ok(mut tasks) = self.task.lock() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
    }
}

/// Why a step of syncing failed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// Text for the user.
    Message(String),
    /// The server's certificate was not accepted.
    Certificate(Box<CertificateProblem>),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl Failure {
    /// A backend's error, with what was being done when it struck.
    fn from_backend(err: Error, doing: Option<&str>) -> Self {
        match err {
            Error::Certificate(problem) => Self::Certificate(problem),
            other => Self::Message(match doing {
                Some(doing) => format!("{doing}: {other}"),
                None => other.to_string(),
            }),
        }
    }
}

impl Engine {
    fn emit(&self, event: SyncEvent) {
        let _ = self.events.send(event);
    }

    fn running(&self) -> Vec<Arc<Running>> {
        self.accounts
            .read()
            .map(|accounts| accounts.values().cloned().collect())
            .unwrap_or_default()
    }

    fn account(&self, id: LocalAccountId) -> Option<Arc<Running>> {
        self.accounts.read().ok()?.get(&id).cloned()
    }

    fn set_account(engine: &Arc<Self>, account: SyncAccount) {
        let id = account.id;
        let running = Arc::new(Running {
            account,
            wake: Notify::new(),
            queue: Notify::new(),
            task: Mutex::new(Vec::new()),
        });
        let sync = tokio::spawn({
            let engine = Arc::clone(engine);
            let running = Arc::clone(&running);
            async move { engine.keep_in_sync(&running).await }
        });
        let queue = tokio::spawn({
            let engine = Arc::clone(engine);
            let running = Arc::clone(&running);
            async move { engine.run_queue(&running).await }
        });
        let watch = tokio::spawn({
            let engine = Arc::clone(engine);
            let running = Arc::clone(&running);
            async move { engine.watch(&running).await }
        });
        if let Ok(mut tasks) = running.task.lock() {
            tasks.extend([sync, queue, watch]);
        }
        let replaced = engine
            .accounts
            .write()
            .ok()
            .and_then(|mut accounts| accounts.insert(id, running));
        if let Some(replaced) = replaced {
            replaced.stop();
            // Its pass may have been cut short mid-way.
            engine.emit(SyncEvent::Syncing {
                account: id,
                active: false,
            });
        }
    }

    fn remove_account(&self, id: LocalAccountId) {
        let removed = self
            .accounts
            .write()
            .ok()
            .and_then(|mut accounts| accounts.remove(&id));
        if let Some(removed) = removed {
            removed.stop();
            self.emit(SyncEvent::Syncing {
                account: id,
                active: false,
            });
        }
    }

    fn stop(&self) {
        for running in self.running() {
            running.stop();
        }
    }

    /// Runs `work` on the store, off the async threads: SQLite blocks.
    async fn db<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Store) -> quill_store::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let mut store = store
                .lock()
                .map_err(|_| "the store is unavailable after an earlier failure".to_owned())?;
            work(&mut store).map_err(|err| err.to_string())
        })
        .await
        .map_err(|err| err.to_string())?
    }

    /// Listens for the server saying that something has changed, and syncs
    /// the account when it does, so new mail shows without waiting for the
    /// interval. The interval goes on regardless: push is a hurry-up, not
    /// what correctness rests on.
    async fn watch(&self, running: &Running) {
        let backend = Arc::clone(&running.account.backend);
        let mut failures: u32 = 0;
        loop {
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            let watching = backend.watch(tx);
            tokio::pin!(watching);
            // The backend holds the other end until it returns.
            let mut listening = true;
            let outcome = loop {
                tokio::select! {
                    outcome = &mut watching => break outcome,
                    event = rx.recv(), if listening => match event {
                        Some(RemoteEvent::FolderChanged(_)) => {
                            failures = 0;
                            running.wake.notify_one();
                        }
                        None => listening = false,
                    },
                }
            };
            match outcome {
                // It came back by itself with nothing wrong: this server
                // does not push. The interval does all the work.
                Ok(()) => return,
                // The connection it listens on failed. The sync loop says
                // so if the server is really unreachable; here it is only
                // a reason to wait before listening again.
                Err(_) => {
                    let wait = self.retry_at(failures).saturating_sub(now());
                    failures = failures.saturating_add(1);
                    tokio::time::sleep(Duration::from_secs(
                        u64::try_from(wait).unwrap_or(1).max(1),
                    ))
                    .await;
                }
            }
        }
    }

    async fn keep_in_sync(&self, running: &Running) {
        let account = &running.account;
        // The same failure every few minutes is one problem, not many.
        let mut last_problem: Option<Failure> = None;
        loop {
            self.emit(SyncEvent::Syncing {
                account: account.id,
                active: true,
            });
            let outcome = self.pass(account).await;
            self.emit(SyncEvent::Syncing {
                account: account.id,
                active: false,
            });
            match outcome {
                Ok(()) => last_problem = None,
                Err(failure) => {
                    if last_problem.as_ref() != Some(&failure) {
                        self.emit(match failure.clone() {
                            Failure::Message(message) => SyncEvent::Problem {
                                account: account.name.clone(),
                                message,
                            },
                            Failure::Certificate(problem) => SyncEvent::Certificate {
                                account: account.name.clone(),
                                problem,
                                sending: false,
                            },
                        });
                    }
                    last_problem = Some(failure);
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.options.interval) => {}
                () = running.wake.notified() => {}
            }
        }
    }

    /// One pass over an account: its folders, their messages, recent bodies.
    async fn pass(&self, account: &SyncAccount) -> Result<(), Failure> {
        // A server with no folders of its own (POP3) only hands mail over:
        // it is kept here, in folders that exist only here.
        let keeps_here = !account.backend.capabilities().folders;
        let mut remote = account
            .backend
            .list_folders()
            .await
            .map_err(|err| Failure::from_backend(err, None))?;
        if keeps_here {
            remote.extend(kept_folders());
        }
        let account_id = account.id;
        let listed = remote.clone();
        let (mut folders, folders_changed): (Vec<(LocalFolderId, FolderId, FolderRole)>, bool) =
            self.db(move |store| {
                let of_account = |store: &Store| -> quill_store::Result<Vec<_>> {
                    Ok(store
                        .folders()?
                        .into_iter()
                        .filter(|folder| folder.account == account_id)
                        .map(|folder| (folder.id, folder.name, folder.role))
                        .collect())
                };
                let before = of_account(store)?;
                let mut folders = Vec::with_capacity(listed.len());
                for folder in &listed {
                    let id = store.upsert_folder(account_id, folder)?;
                    folders.push((id, folder.id.clone(), folder.role));
                }
                let keep: Vec<FolderId> = listed.iter().map(|folder| folder.id.clone()).collect();
                store.retain_folders(account_id, &keep)?;
                Ok((folders, before != of_account(store)?))
            })
            .await?;
        if folders_changed {
            // New or deleted folders change the sidebar before any mail does.
            self.emit(SyncEvent::Changed);
        }

        // The inbox first, the rubbish last.
        folders.sort_by_key(|(_, _, role)| match role {
            FolderRole::Inbox => 0,
            FolderRole::Drafts | FolderRole::Sent | FolderRole::Other => 1,
            FolderRole::Archive => 2,
            FolderRole::Junk | FolderRole::Trash => 3,
        });
        for (local, remote, role) in folders {
            if remote.0.starts_with(KEPT) {
                // Nothing on any server to compare it with.
                continue;
            }
            // What a folder holds when it is first synced is not news.
            let mut first_look = None;
            loop {
                let cursor = self.db(move |store| store.sync_cursor(local)).await?;
                let first_look = *first_look.get_or_insert(cursor.is_none());
                let mut delta = account
                    .backend
                    .sync_folder(&remote, cursor)
                    .await
                    .map_err(|err| Failure::from_backend(err, Some(&remote.0)))?;
                let has_more = delta.has_more;
                let mut changed = false;
                if keeps_here {
                    // The copy here is the one that matters. A message the
                    // server lists again is not a second message, wherever
                    // the first was filed since; and one the server no
                    // longer has stays here, unless it never arrived.
                    let listed: Vec<RemoteId> = delta
                        .added
                        .iter()
                        .map(|message| message.id.clone())
                        .collect();
                    let gone = std::mem::take(&mut delta.removed);
                    let at = now();
                    let (known, dropped) = self
                        .db(move |store| {
                            let known = store.known_remote_ids(account_id, &listed)?;
                            store.note_server_copies(account_id, &listed, at)?;
                            store.forget_server_copies(account_id, &gone)?;
                            let dropped = store.drop_unfetched(account_id, &gone)?;
                            Ok((known, dropped))
                        })
                        .await?;
                    delta.added.retain(|message| !known.contains(&message.id));
                    changed = dropped;
                }
                // Mail the inbox did not have before is put to the rules,
                // and what of it is unread is announced.
                let listed: Vec<(RemoteId, bool)> = if role == FolderRole::Inbox && !first_look {
                    delta
                        .added
                        .iter()
                        .map(|message| (message.id.clone(), message.flags.seen))
                        .collect()
                } else {
                    Vec::new()
                };
                let rules = self.rules.clone();
                let account_name = account.name.clone();
                let (applied, mut arrived, ruled) = self
                    .db(move |store| {
                        // A backend may list a message again because it
                        // changed; that is not an arrival.
                        let ids: Vec<RemoteId> = listed.iter().map(|(id, _)| id.clone()).collect();
                        let known = store.known_remote_ids(account_id, &ids)?;
                        let applied = store.apply_delta(local, &delta)?;
                        let mut new = Vec::new();
                        let mut unread = Vec::new();
                        for (id, seen) in listed.iter().filter(|(id, _)| !known.contains(id)) {
                            if let Some(message) = store.local_message_id(local, id)? {
                                new.push(message);
                                if !seen {
                                    unread.push(message);
                                }
                            }
                        }
                        let ruled =
                            rules::apply(store, &rules, account_id, &account_name, &new, now())?;
                        Ok((applied, unread, ruled))
                    })
                    .await?;
                changed |= applied | ruled.queued;
                if ruled.queued {
                    // What the rules decided goes to the server now.
                    if let Some(running) = self.account(account_id) {
                        running.queue.notify_one();
                    }
                }
                for problem in ruled.problems {
                    self.problem(account, problem);
                }
                arrived.retain(|id| !ruled.quiet.contains(id));
                if !arrived.is_empty() {
                    self.emit(SyncEvent::Arrived {
                        account: account_id,
                        messages: arrived,
                    });
                }
                if changed {
                    self.emit(SyncEvent::Changed);
                }
                if !has_more {
                    break;
                }
            }
        }

        if keeps_here {
            return self.keep_here(account).await;
        }

        // Recent mail should read offline, so fetch its bodies ahead.
        let since = now() - self.options.prefetch_days * 24 * 60 * 60;
        let limit = self.options.prefetch_limit;
        let wanted = self
            .db(move |store| store.messages_without_body(account_id, since, limit))
            .await?;
        for id in wanted {
            match self.fetch_body(id).await {
                Ok(()) => {}
                // A message that vanished meanwhile is no reason to stop.
                Err(FetchFailure::Gone) => {}
                Err(FetchFailure::Other(message)) => return Err(message.into()),
            }
        }
        Ok(())
    }

    /// The rest of a pass for an account whose mail is kept here: every
    /// message is downloaded whole, and then the server's copies are dealt
    /// with as the account asks.
    async fn keep_here(&self, account: &SyncAccount) -> Result<(), Failure> {
        let account_id = account.id;
        loop {
            // A session at a time: a POP3 server locks the mailbox for as
            // long as one lasts, and limits how often an account signs in.
            let wanted = self
                .db(move |store| {
                    let mut wanted = Vec::new();
                    for id in store.messages_without_body(account_id, i64::MIN, 50)? {
                        if let Some(location) = store.message_location(id)? {
                            wanted.push((id, location.remote, location.date));
                        }
                    }
                    Ok(wanted)
                })
                .await?;
            if wanted.is_empty() {
                break;
            }
            let ids: Vec<RemoteId> = wanted.iter().map(|(_, remote, _)| remote.clone()).collect();
            let fetched = account
                .backend
                .fetch_many(&ids)
                .await
                .map_err(|err| Failure::from_backend(err, None))?;
            let mut gone = Vec::new();
            for ((id, remote, date), raw) in wanted.into_iter().zip(fetched) {
                match raw {
                    Some(raw) => self
                        .store_downloaded(id, date, raw)
                        .await
                        .map_err(|failure| match failure {
                            FetchFailure::Gone => "a message could not be saved".to_owned(),
                            FetchFailure::Other(message) => message,
                        })?,
                    // It left the server before it was fetched. There is
                    // nothing of it to keep.
                    None => gone.push(remote),
                }
            }
            if !gone.is_empty() {
                self.db(move |store| {
                    store.forget_server_copies(account_id, &gone)?;
                    store.drop_unfetched(account_id, &gone)
                })
                .await?;
            }
            self.emit(SyncEvent::Changed);
        }

        let seen_by = match account.server_copies {
            ServerCopies::Keep => return Ok(()),
            ServerCopies::DeleteWhenDownloaded => now(),
            ServerCopies::DeleteAfterDays(days) => now() - i64::from(days) * 24 * 60 * 60,
        };
        let done = self
            .db(move |store| store.server_copies_to_delete(account_id, seen_by))
            .await?;
        for batch in done.chunks(200) {
            account
                .backend
                .delete_many(batch)
                .await
                .map_err(|err| Failure::from_backend(err, None))?;
            let batch = batch.to_vec();
            self.db(move |store| store.forget_server_copies(account_id, &batch))
                .await?;
        }
        Ok(())
    }

    async fn fetch_body_for_ui(&self, id: LocalMessageId) {
        match self.fetch_body(id).await {
            Ok(()) => self.emit(SyncEvent::BodyReady(id)),
            Err(FetchFailure::Gone) => self.emit(SyncEvent::BodyFailed {
                id,
                reason: "The message is no longer on the server.".to_owned(),
            }),
            Err(FetchFailure::Other(reason)) => self.emit(SyncEvent::BodyFailed { id, reason }),
        }
    }

    /// Downloads a message, files its raw bytes and stores its text.
    async fn fetch_body(&self, id: LocalMessageId) -> Result<(), FetchFailure> {
        let location = self
            .db(move |store| store.message_location(id))
            .await
            .map_err(FetchFailure::Other)?
            .ok_or(FetchFailure::Gone)?;
        let running = self
            .account(location.account)
            .ok_or_else(|| FetchFailure::Other("This account does not sync.".to_owned()))?;
        let raw = running
            .account
            .backend
            .fetch_raw(&location.remote)
            .await
            .map_err(|err| match err {
                Error::NotFound(_) => FetchFailure::Gone,
                other => FetchFailure::Other(other.to_string()),
            })?;
        self.store_downloaded(id, location.date, raw).await
    }

    /// Files a downloaded message's raw bytes and stores its text.
    async fn store_downloaded(
        &self,
        id: LocalMessageId,
        date: i64,
        raw: bytes::Bytes,
    ) -> Result<(), FetchFailure> {
        let parsed = quill_mime::parse(&raw, date);
        // The whole message says what it follows, where a listing may not
        // have: its place in a conversation is settled from it.
        let ids = parsed
            .as_ref()
            .map(|parsed| (parsed.headers.message_id.clone(), parsed.thread()));
        let text = parsed.map(|parsed| parsed.body).unwrap_or_default();
        let blobs = self.blobs.clone();
        let hash = tokio::task::spawn_blocking(move || match blobs {
            Some(blobs) => blobs.put(&raw).map(Some),
            None => Ok(None),
        })
        .await
        .map_err(|err| FetchFailure::Other(err.to_string()))?
        .map_err(|err| FetchFailure::Other(format!("cannot save the message: {err}")))?;
        self.db(move |store| {
            match hash {
                Some(hash) => store.put_downloaded(id, &text, &hash)?,
                None => store.put_body(id, &text)?,
            }
            if let Some((own, follows)) = ids {
                store.thread_message(id, own.as_deref(), &follows)?;
            }
            Ok(())
        })
        .await
        .map_err(FetchFailure::Other)
    }

    /// Carries out the account's queued changes and sends its waiting
    /// mail, each in the order made, for as long as the account runs.
    async fn run_queue(&self, running: &Running) {
        let account = &running.account;
        let id = account.id;
        loop {
            // Changes first: a reply's "answered" flag and sent copy follow
            // the send that queued them on the next turn of this loop.
            let mut waiting_until: Option<i64> = None;
            loop {
                let claimed = self.db(move |store| store.claim_operation(id, now())).await;
                match claimed {
                    Ok(Some(operation)) => {
                        if let Some(retry_at) = self.carry_out(running, operation).await {
                            // The server cannot be reached or is busy:
                            // everything behind this operation waits too.
                            waiting_until = Some(retry_at);
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(message) => {
                        self.problem(account, message);
                        break;
                    }
                }
            }
            if waiting_until.is_none() {
                loop {
                    let claimed = self.db(move |store| store.claim_send(id, now())).await;
                    match claimed {
                        Ok(Some(outgoing)) => {
                            let queued_more = self.send(account, outgoing).await;
                            if queued_more {
                                // The sent copy and the answered flag.
                                running.queue.notify_one();
                            }
                        }
                        Ok(None) => break,
                        Err(message) => {
                            self.problem(account, message);
                            break;
                        }
                    }
                }
            }
            // Sleep until the next thing is due, or until told there is
            // something new.
            let due = self
                .db(move |store| {
                    let operation = store.next_operation_time(id)?;
                    let send = store.next_send_time(id)?;
                    Ok([operation, send].into_iter().flatten().min())
                })
                .await
                .ok()
                .flatten();
            let wait = match [due, waiting_until].into_iter().flatten().max() {
                Some(at) => Duration::from_secs(u64::try_from(at - now()).unwrap_or(0).max(1)),
                None => Duration::from_secs(24 * 60 * 60),
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = running.queue.notified() => {}
            }
        }
    }

    fn problem(&self, account: &SyncAccount, message: String) {
        self.emit(SyncEvent::Problem {
            account: account.name.clone(),
            message,
        });
    }

    /// When to try again after `attempts` failures.
    fn retry_at(&self, attempts: u32) -> i64 {
        let base = self.options.retry.as_secs().max(1);
        let wait = base.saturating_mul(1 << attempts.min(10)).min(30 * 60);
        now() + i64::try_from(wait).unwrap_or(30 * 60)
    }

    /// Replays one queued operation against the server. Returns when to try
    /// again if it could not be tried or finished just now; `None` when it
    /// is settled, done or refused.
    async fn carry_out(&self, running: &Running, operation: QueuedOperation) -> Option<i64> {
        let account = &running.account;
        let id = operation.id;
        let can = account.backend.capabilities();
        // A server with no folders keeps nothing but what it was sent:
        // filing and marking happen here and nowhere else.
        let keeps_here = !can.folders;
        let mut left_server = None;
        let (outcome, what, resync) = match operation.kind {
            QueuedKind::Nothing => (Ok(None), "", false),
            // The move is already made here; the message keeps its name.
            QueuedKind::Move { remote, .. } if keeps_here => (Ok(Some(remote)), "moved", false),
            // A copy the server never had needs nothing done there.
            QueuedKind::Delete { remote, .. } if remote.0.starts_with(KEPT) => {
                (Ok(None), "deleted", false)
            }
            QueuedKind::Flags { .. } if !can.flags => (Ok(None), "changed", false),
            QueuedKind::Append {
                target_local,
                blob_hash,
                flags,
                ..
            } if keeps_here => (
                self.keep_copy(target_local, blob_hash, flags).await,
                "saved",
                false,
            ),
            QueuedKind::Move { remote, target, .. } => (
                account.backend.move_message(&remote, &target).await,
                "moved",
                true,
            ),
            QueuedKind::Delete { remote, .. } => {
                let outcome = account
                    .backend
                    .apply(&Operation::Delete { id: remote.clone() })
                    .await
                    .map(|()| None);
                if keeps_here && outcome.is_ok() {
                    left_server = Some(remote);
                }
                (outcome, "deleted", false)
            }
            QueuedKind::Flags { remote, flags } => (
                account
                    .backend
                    .apply(&Operation::SetFlags { id: remote, flags })
                    .await
                    .map(|()| None),
                "changed",
                false,
            ),
            QueuedKind::Append {
                target,
                blob_hash,
                flags,
                ..
            } => {
                let blobs = self.blobs.clone();
                let raw = tokio::task::spawn_blocking(move || match blobs {
                    Some(blobs) => blobs.get(&blob_hash),
                    None => Ok(None),
                })
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
                match raw {
                    // A saved copy has no row here until a sync lists it,
                    // whether or not the server said what it called it.
                    Some(raw) => (
                        account
                            .backend
                            .append(&target, &raw, flags)
                            .await
                            .map(|_| None),
                        "saved",
                        true,
                    ),
                    // The message to save has gone: there is nothing to do.
                    None => (Ok(None), "saved", false),
                }
            }
        };
        match outcome {
            Ok(new_id) => {
                let knows_new_id = new_id.is_some();
                let account_id = account.id;
                let done = self
                    .db(move |store| {
                        if let Some(remote) = left_server {
                            store.forget_server_copies(account_id, &[remote])?;
                        }
                        store.complete_operation(id, new_id.as_ref())
                    })
                    .await;
                if let Err(message) = done {
                    self.problem(account, message);
                }
                self.emit(SyncEvent::Changed);
                if resync && !knows_new_id {
                    // The message is in its new folder under a name only a
                    // sync will tell.
                    running.wake.notify_one();
                }
                None
            }
            // Not the operation's fault: the server could not be asked. It
            // keeps its place and is tried again.
            Err(
                Error::Connection(_) | Error::Temporary(_) | Error::Auth(_) | Error::Certificate(_),
            ) => {
                let retry_at = self.retry_at(operation.attempts);
                let _ = self
                    .db(move |store| store.retry_operation(id, retry_at))
                    .await;
                Some(retry_at)
            }
            Err(refused) => {
                let undone = self.db(move |store| store.fail_operation(id)).await;
                if undone.is_ok() {
                    self.emit(SyncEvent::Changed);
                }
                self.problem(
                    account,
                    format!("A message could not be {what} on the server and has been put back as it was: {refused}"),
                );
                None
            }
        }
    }

    /// Lets downloaded copies go, oldest first, until what is stored comes
    /// to no more than `budget` bytes, and a little under, so that the
    /// next message does not start it again.
    async fn trim_cache(&self, budget: u64) -> Result<u64, String> {
        let Some(blobs) = self.blobs.clone() else {
            return Ok(0);
        };
        let measured = Arc::clone(&blobs);
        let total = tokio::task::spawn_blocking(move || measured.total())
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())?;
        if total <= budget {
            return Ok(0);
        }
        let aim = budget - budget / 10;
        let before = now() - CACHE_KEEPS_DAYS.max(self.options.prefetch_days) * 24 * 60 * 60;
        let mut freed = 0;
        while total - freed > aim {
            let batch = self.db(move |store| store.evictable(before, 200)).await?;
            if batch.is_empty() {
                // Everything left is recent, or is the only copy there is.
                break;
            }
            for (_, hash) in batch {
                if total - freed <= aim {
                    break;
                }
                // The store forgets first: a message that says it has a
                // copy it does not have is worse than a file nothing
                // points to.
                let forgotten = hash.clone();
                self.db(move |store| store.forget_blob(&forgotten)).await?;
                let blobs = Arc::clone(&blobs);
                freed += tokio::task::spawn_blocking(move || blobs.remove(&hash))
                    .await
                    .map_err(|err| err.to_string())?
                    .map_err(|err| err.to_string())?;
            }
        }
        Ok(freed)
    }

    /// Finds the conversations of a batch of downloaded messages that have
    /// none yet, from their own copies. Returns how many it dealt with.
    async fn thread_some(&self) -> Result<usize, String> {
        let blobs = self.blobs.clone();
        self.db(move |store| {
            let waiting = store.unthreaded(200)?;
            for id in &waiting {
                let raw = match (&blobs, store.blob_hash(*id)?) {
                    (Some(blobs), Some(hash)) => blobs.get(&hash).ok().flatten(),
                    _ => None,
                };
                let parsed = raw.and_then(|raw| quill_mime::parse(&raw, 0));
                // One that cannot be read is a conversation of its own:
                // it is settled either way, and not looked at again.
                match parsed {
                    Some(parsed) => store.thread_message(
                        *id,
                        parsed.headers.message_id.as_deref(),
                        &parsed.thread(),
                    )?,
                    None => store.thread_message(*id, None, &[])?,
                };
            }
            Ok(waiting.len())
        })
        .await
    }

    /// Files a copy of a message in a folder that exists only here: the
    /// sent copy, for an account whose server keeps none.
    async fn keep_copy(
        &self,
        folder: LocalFolderId,
        blob_hash: String,
        flags: Flags,
    ) -> quill_core::Result<Option<RemoteId>> {
        let blobs = self.blobs.clone();
        let hash = blob_hash.clone();
        let raw = tokio::task::spawn_blocking(move || match blobs {
            Some(blobs) => blobs.get(&hash),
            None => Ok(None),
        })
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
        // The message to save has gone: there is nothing to do.
        let Some(raw) = raw else {
            return Ok(None);
        };
        let Some(parsed) = quill_mime::parse(&raw, now()) else {
            return Ok(None);
        };
        let message = RemoteMessage {
            // Named by its content: saving the same message again, after a
            // crash, files it once.
            id: RemoteId(format!("{KEPT}{blob_hash}")),
            thread: parsed.thread(),
            headers: parsed.headers,
            flags,
            size: u64::try_from(raw.len()).unwrap_or(u64::MAX),
            has_attachments: !parsed.attachments.is_empty(),
        };
        let text = parsed.body;
        self.db(move |store| store.keep_message(folder, &message, &text, Some(&blob_hash)))
            .await
            .map(|_| None)
            .map_err(Error::Backend)
    }

    /// Sends one message from the outbox. Returns whether sending it
    /// queued further work (a sent copy, an answered flag).
    async fn send(&self, account: &SyncAccount, outgoing: Outgoing) -> bool {
        let id = outgoing.id;
        let refuse = |reason: String| async move {
            let kept = reason.clone();
            let _ = self
                .db(move |store| store.send_failed(id, &kept, None))
                .await;
            self.emit(SyncEvent::Changed);
            self.problem(
                account,
                format!("A message was not sent and is waiting in the Outbox: {reason}"),
            );
            false
        };
        let Some(sender) = &account.sender else {
            return refuse(
                "this account has no server to send through. Add `smtp = { host = \"...\" }` to it in config.toml."
                    .to_owned(),
            )
            .await;
        };
        let blobs = self.blobs.clone();
        let hash = outgoing.raw_hash.clone();
        let raw = tokio::task::spawn_blocking(move || match blobs {
            Some(blobs) => blobs.get(&hash),
            None => Ok(None),
        })
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
        let Some(raw) = raw else {
            return refuse(
                "the finished message is missing. Open it and send it again.".to_owned(),
            )
            .await;
        };
        let envelope = Envelope {
            from: outgoing.from.clone(),
            to: outgoing.to.clone(),
        };
        match sender.send(&raw, &envelope).await {
            Ok(()) => {
                let account_id = account.id;
                let save_copy = !account.server_saves_sent;
                let raw_hash = outgoing.raw_hash.clone();
                let replied_to = outgoing.replied_to;
                let queued = self
                    .db(move |store| {
                        store.sent(id)?;
                        let mut queued = false;
                        if save_copy {
                            let sent_folder = store
                                .folders()?
                                .into_iter()
                                .find(|folder| {
                                    folder.account == account_id && folder.role == FolderRole::Sent
                                })
                                .map(|folder| folder.id);
                            if let Some(folder) = sent_folder {
                                let seen = Flags {
                                    seen: true,
                                    ..Flags::default()
                                };
                                queued |= store
                                    .queue_append(folder, &raw_hash, seen, now())?
                                    .is_some();
                            }
                        }
                        if let Some(original) = replied_to {
                            let original = LocalMessageId(original);
                            if let Some(location) = store.message_location(original)? {
                                // Answering a message is having read it.
                                let answered = Flags {
                                    answered: true,
                                    seen: true,
                                    ..location.flags
                                };
                                queued |= store.queue_flags(original, answered, now())?;
                            }
                        }
                        Ok(queued)
                    })
                    .await;
                self.emit(SyncEvent::Sent {
                    account: account.name.clone(),
                });
                self.emit(SyncEvent::Changed);
                match queued {
                    Ok(queued) => queued,
                    Err(message) => {
                        self.problem(account, message);
                        false
                    }
                }
            }
            // The server could not take it just now. It stays in the outbox
            // and goes later.
            Err(err @ (Error::Connection(_) | Error::Temporary(_))) => {
                let retry_at = self.retry_at(outgoing.attempts);
                let reason = err.to_string();
                let _ = self
                    .db(move |store| store.send_failed(id, &reason, Some(retry_at)))
                    .await;
                self.emit(SyncEvent::Changed);
                if outgoing.attempts == 0 {
                    self.problem(
                        account,
                        format!("A message could not be sent yet and will be tried again: {err}"),
                    );
                }
                false
            }
            // Nothing was said to a server whose certificate was not
            // accepted. The message waits while the user is asked.
            Err(Error::Certificate(problem)) => {
                let retry_at = self.retry_at(outgoing.attempts);
                let _ = self
                    .db(move |store| {
                        store.send_failed(
                            id,
                            "the server's certificate was not accepted",
                            Some(retry_at),
                        )
                    })
                    .await;
                self.emit(SyncEvent::Changed);
                self.emit(SyncEvent::Certificate {
                    account: account.name.clone(),
                    problem,
                    sending: true,
                });
                false
            }
            Err(Error::Recipients(refused)) => {
                let list = refused
                    .iter()
                    .map(|recipient| format!("{} ({})", recipient.address, recipient.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
                refuse(format!("the server refused these recipients: {list}")).await
            }
            Err(other) => refuse(other.to_string()).await,
        }
    }
}

enum FetchFailure {
    /// The message is not there any more, locally or on the server.
    Gone,
    Other(String),
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
