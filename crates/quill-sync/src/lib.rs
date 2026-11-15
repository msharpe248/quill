//! The sync engine.
//!
//! It runs each account's [`Backend`] into the local store in the
//! background: folders, then each folder's messages in batches, then the
//! bodies of recent mail. The UI never waits for it. It reads the store,
//! hears a [`SyncEvent`] when the store has changed, and asks for what only
//! the network can give with a [`SyncRequest`].
//!
//! Nothing here knows which protocol an account speaks.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quill_core::types::{
    Flags, FolderId, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId, Operation,
};
use quill_core::{Backend, Error};
use quill_store::{BlobStore, Store};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

/// An account for the engine to keep in sync.
pub struct SyncAccount {
    pub id: LocalAccountId,
    pub name: String,
    pub backend: Arc<dyn Backend>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    /// How long to wait between passes when nothing asks for one sooner.
    pub interval: Duration,
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
    /// A body that was asked for is now in the store.
    BodyReady(LocalMessageId),
    /// A body that was asked for could not be fetched.
    BodyFailed { id: LocalMessageId, reason: String },
    /// Something went wrong that the user should see.
    Problem { account: String, message: String },
}

/// What the UI asks of the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRequest {
    /// Sync every account now instead of at the next interval.
    SyncNow,
    /// Download a message's body. Answered by `BodyReady` or `BodyFailed`.
    FetchBody(LocalMessageId),
    /// The user changed a message's flags, and the store already says so.
    /// Tell the server. If it refuses, the store is put back to `previous`.
    SetFlags {
        id: LocalMessageId,
        flags: Flags,
        previous: Flags,
    },
}

/// The UI's end of the engine. Dropping it stops the engine.
pub struct SyncHandle {
    requests: UnboundedSender<SyncRequest>,
    tasks: Vec<JoinHandle<()>>,
}

impl SyncHandle {
    /// Queues a request. Returns `false` when the engine has stopped.
    pub fn send(&self, request: SyncRequest) -> bool {
        self.requests.send(request).is_ok()
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
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
    let (requests, queue) = unbounded_channel();
    let engine = Arc::new(Engine {
        store: Arc::new(Mutex::new(store)),
        blobs: blobs.map(|root| Arc::new(BlobStore::new(root))),
        options,
        events,
        wake: (0..accounts.len()).map(|_| Notify::new()).collect(),
        accounts,
    });
    let mut tasks = Vec::new();
    for index in 0..engine.accounts.len() {
        let engine = Arc::clone(&engine);
        tasks.push(tokio::spawn(
            async move { engine.keep_in_sync(index).await },
        ));
    }
    tasks.push(tokio::spawn(serve(engine, queue)));
    SyncHandle { requests, tasks }
}

async fn serve(engine: Arc<Engine>, mut queue: UnboundedReceiver<SyncRequest>) {
    while let Some(request) = queue.recv().await {
        match request {
            // `notify_one` keeps the request for an account that is mid-pass:
            // it goes round again as soon as it finishes.
            SyncRequest::SyncNow => engine.wake.iter().for_each(Notify::notify_one),
            // Each on its own task: a slow download must not hold up the
            // next request. The backend serialises what shares a connection.
            SyncRequest::FetchBody(id) => {
                let engine = Arc::clone(&engine);
                tokio::spawn(async move { engine.fetch_body_for_ui(id).await });
            }
            SyncRequest::SetFlags {
                id,
                flags,
                previous,
            } => {
                let engine = Arc::clone(&engine);
                tokio::spawn(async move { engine.set_flags(id, flags, previous).await });
            }
        }
    }
}

struct Engine {
    store: Arc<Mutex<Store>>,
    blobs: Option<Arc<BlobStore>>,
    accounts: Vec<SyncAccount>,
    options: SyncOptions,
    events: UnboundedSender<SyncEvent>,
    /// One per account: wakes its loop before the interval is up.
    wake: Vec<Notify>,
}

/// Why a step of syncing failed, as text for the user.
type Failure = String;

impl Engine {
    fn emit(&self, event: SyncEvent) {
        let _ = self.events.send(event);
    }

    /// Runs `work` on the store, off the async threads: SQLite blocks.
    async fn db<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Store) -> quill_store::Result<T> + Send + 'static,
    ) -> Result<T, Failure> {
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

    async fn keep_in_sync(&self, index: usize) {
        let account = &self.accounts[index];
        // The same failure every few minutes is one problem, not many.
        let mut last_problem: Option<String> = None;
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
                Err(message) => {
                    if last_problem.as_ref() != Some(&message) {
                        self.emit(SyncEvent::Problem {
                            account: account.name.clone(),
                            message: message.clone(),
                        });
                    }
                    last_problem = Some(message);
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.options.interval) => {}
                () = self.wake[index].notified() => {}
            }
        }
    }

    /// One pass over an account: its folders, their messages, recent bodies.
    async fn pass(&self, account: &SyncAccount) -> Result<(), Failure> {
        let remote = account
            .backend
            .list_folders()
            .await
            .map_err(|err| err.to_string())?;
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
        for (local, remote, _) in folders {
            loop {
                let cursor = self.db(move |store| store.sync_cursor(local)).await?;
                let delta = account
                    .backend
                    .sync_folder(&remote, cursor)
                    .await
                    .map_err(|err| format!("{}: {err}", remote.0))?;
                let has_more = delta.has_more;
                let changed = self
                    .db(move |store| store.apply_delta(local, &delta))
                    .await?;
                if changed {
                    self.emit(SyncEvent::Changed);
                }
                if !has_more {
                    break;
                }
            }
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
                Err(FetchFailure::Other(message)) => return Err(message),
            }
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
        let account = self
            .accounts
            .iter()
            .find(|account| account.id == location.account)
            .ok_or_else(|| FetchFailure::Other("This account does not sync.".to_owned()))?;
        let raw = account
            .backend
            .fetch_raw(&location.remote)
            .await
            .map_err(|err| match err {
                Error::NotFound(_) => FetchFailure::Gone,
                other => FetchFailure::Other(other.to_string()),
            })?;

        let text = quill_mime::parse(&raw, location.date)
            .map(|parsed| parsed.body)
            .unwrap_or_default();
        let blobs = self.blobs.clone();
        let hash = tokio::task::spawn_blocking(move || match blobs {
            Some(blobs) => blobs.put(&raw).map(Some),
            None => Ok(None),
        })
        .await
        .map_err(|err| FetchFailure::Other(err.to_string()))?
        .map_err(|err| FetchFailure::Other(format!("cannot save the message: {err}")))?;
        self.db(move |store| match hash {
            Some(hash) => store.put_downloaded(id, &text, &hash),
            None => store.put_body(id, &text),
        })
        .await
        .map_err(FetchFailure::Other)
    }

    /// Tells the server about a flag change the store already shows, and
    /// takes the change back if the server will not have it.
    async fn set_flags(&self, id: LocalMessageId, flags: Flags, previous: Flags) {
        let Ok(Some(location)) = self.db(move |store| store.message_location(id)).await else {
            return;
        };
        let Some(account) = self
            .accounts
            .iter()
            .find(|account| account.id == location.account)
        else {
            // An account with no server, such as the demo: local is all.
            return;
        };
        let operation = Operation::SetFlags {
            id: location.remote,
            flags,
        };
        if let Err(err) = account.backend.apply(&operation).await {
            let rolled_back = self.db(move |store| store.set_flags(id, previous)).await;
            if rolled_back.is_ok() {
                self.emit(SyncEvent::Changed);
            }
            self.emit(SyncEvent::Problem {
                account: account.name.clone(),
                message: format!("The change was not saved on the server: {err}"),
            });
        }
    }
}

enum FetchFailure {
    /// The message is not there any more, locally or on the server.
    Gone,
    Other(Failure),
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
