//! The store worker: the only place the UI's data comes from.
//!
//! The UI task never touches disk. It sends a [`DataRequest`] and carries on;
//! the worker thread runs the query against the store and answers with
//! [`DataEvent`]s on a channel the UI task selects on.

use std::sync::mpsc;
use std::thread;

use quill_core::types::{Flags, FolderRole, LocalFolderId, LocalMessageId};
use quill_store::{AccountRow, FolderRow, Mailbox, MessageRow, Store};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRequest {
    LoadSidebar,
    LoadMessages(Mailbox),
    LoadBody(LocalMessageId),
    SetSeen {
        id: LocalMessageId,
        seen: bool,
    },
    SetFlagged {
        id: LocalMessageId,
        flagged: bool,
    },
    /// Move the message to its account's archive folder.
    Archive(LocalMessageId),
    /// Move the message to the trash, or remove it for good if it is
    /// already there.
    Delete(LocalMessageId),
    MoveTo {
        id: LocalMessageId,
        folder: LocalFolderId,
    },
    /// Take back the last change that still can be.
    Undo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataEvent {
    SidebarLoaded {
        accounts: Vec<AccountRow>,
        folders: Vec<FolderRow>,
    },
    MessagesLoaded {
        mailbox: Mailbox,
        rows: Vec<MessageRow>,
    },
    /// `body` is `None` when the message's body is not in the store.
    BodyLoaded {
        id: LocalMessageId,
        body: Option<LoadedBody>,
    },
    /// Something in the store changed; views of it should reload.
    StoreChanged,
    /// A change for a server, or mail to send, has been written to the
    /// store's queue. The sync engine should look.
    Queued,
    /// Something done, in a few words, to show until the next key.
    /// `undoable` says the undo command would take it back.
    Flash {
        text: String,
        undoable: bool,
    },
    Error(String),
}

/// A message's body as the reader needs it. Both parts are untrusted text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedBody {
    /// The body as plain text.
    pub text: String,
    /// The message's HTML, when it has HTML that renders and its raw form
    /// is at hand.
    pub html: Option<String>,
    /// The HTML is what to show first.
    pub html_first: bool,
    /// `text` is the sender's own plain text, not a rendering of the HTML,
    /// so there are two versions to choose between.
    pub has_plain: bool,
}

/// Runs one request against the store. This is the whole of the worker's
/// logic, kept free of threads so tests can drive it directly.
pub fn handle(store: &mut Store, request: DataRequest) -> Vec<DataEvent> {
    // The sync engine writes through a connection of its own. Finding the
    // database busy is not a failure to report, just a reason to ask again.
    let mut attempts = 0;
    loop {
        match run(store, request) {
            Ok(events) => return events,
            Err(err) if err.is_busy() && attempts < BUSY_RETRIES => {
                attempts += 1;
                thread::sleep(BUSY_PAUSE);
            }
            Err(err) => return vec![DataEvent::Error(err.to_string())],
        }
    }
}

const BUSY_RETRIES: u32 = 20;
const BUSY_PAUSE: std::time::Duration = std::time::Duration::from_millis(50);

fn run(store: &mut Store, request: DataRequest) -> quill_store::Result<Vec<DataEvent>> {
    Ok(match request {
        DataRequest::LoadSidebar => vec![DataEvent::SidebarLoaded {
            accounts: store.accounts()?,
            folders: store.folders()?,
        }],
        DataRequest::LoadMessages(mailbox) => vec![DataEvent::MessagesLoaded {
            mailbox,
            rows: store.messages(mailbox)?,
        }],
        DataRequest::LoadBody(id) => {
            let body = store.body(id)?.map(|text| {
                // The HTML is read from the message as it arrived. If that
                // cannot be had, the text is still worth showing.
                let parsed = store
                    .raw(id)
                    .ok()
                    .flatten()
                    .and_then(|raw| quill_mime::parse(&raw, 0));
                match parsed {
                    Some(parsed) => LoadedBody {
                        text,
                        html: parsed.html,
                        html_first: parsed.html_first,
                        has_plain: parsed.has_plain,
                    },
                    None => LoadedBody {
                        text,
                        html: None,
                        html_first: false,
                        has_plain: true,
                    },
                }
            });
            vec![DataEvent::BodyLoaded { id, body }]
        }
        DataRequest::SetSeen { id, seen } => set_flags(store, id, |flags| Flags { seen, ..flags })?,
        DataRequest::SetFlagged { id, flagged } => {
            set_flags(store, id, |flags| Flags { flagged, ..flags })?
        }
        DataRequest::Archive(id) => match folder_with_role(store, id, FolderRole::Archive)? {
            Some(archive) => moved(store, id, archive, "Archived")?,
            None => vec![DataEvent::Error(
                "This account has no archive folder to move the message to.".to_owned(),
            )],
        },
        DataRequest::Delete(id) => {
            let trash = folder_with_role(store, id, FolderRole::Trash)?;
            let already_there = store
                .message_location(id)?
                .is_some_and(|location| Some(location.folder) == trash);
            match trash {
                Some(trash) if !already_there => moved(store, id, trash, "Moved to the trash")?,
                // In the trash already, or the account has none: for good.
                _ => {
                    let at = now();
                    let queued_one = store.queue_delete(id, at, at + UNDO_SECONDS)?.is_some();
                    done(queued_one, "Deleted for good")
                }
            }
        }
        DataRequest::MoveTo { id, folder } => moved(store, id, folder, "Moved")?,
        DataRequest::Undo => {
            let at = now();
            if let Some(operation) = store.undoable(at)?
                && store.undo_operation(operation)?
            {
                vec![DataEvent::StoreChanged, flash("Undone.")]
            } else if let Some(draft) = store.unsendable(at)?
                && store.unsend(draft, at)?
            {
                vec![
                    DataEvent::StoreChanged,
                    flash("Not sent. The message is a draft again."),
                ]
            } else {
                vec![flash("Nothing to undo.")]
            }
        }
    })
}

/// Changes a message's flags here and queues the change for its server.
fn set_flags(
    store: &mut Store,
    id: LocalMessageId,
    change: impl FnOnce(Flags) -> Flags,
) -> quill_store::Result<Vec<DataEvent>> {
    let Some(location) = store.message_location(id)? else {
        return Ok(Vec::new());
    };
    Ok(queued(store.queue_flags(
        id,
        change(location.flags),
        now(),
    )?))
}

/// How long a move or a deletion can be undone before the server hears of
/// it.
pub const UNDO_SECONDS: i64 = 10;

/// The folder of the message's account that has `role`, if it has one.
fn folder_with_role(
    store: &Store,
    id: LocalMessageId,
    role: FolderRole,
) -> quill_store::Result<Option<LocalFolderId>> {
    let Some(location) = store.message_location(id)? else {
        return Ok(None);
    };
    Ok(store
        .folders()?
        .into_iter()
        .find(|folder| folder.account == location.account && folder.role == role)
        .map(|folder| folder.id))
}

/// Moves a message here and queues the move for its server.
fn moved(
    store: &mut Store,
    id: LocalMessageId,
    folder: LocalFolderId,
    what: &str,
) -> quill_store::Result<Vec<DataEvent>> {
    let at = now();
    let queued_one = store
        .queue_move(id, folder, at, at + UNDO_SECONDS)?
        .is_some();
    Ok(done(queued_one, what))
}

fn flash(text: &str) -> DataEvent {
    DataEvent::Flash {
        text: text.to_owned(),
        undoable: false,
    }
}

/// What to tell the UI after a move or a deletion.
fn done(queued_one: bool, what: &str) -> Vec<DataEvent> {
    if queued_one {
        vec![
            DataEvent::StoreChanged,
            DataEvent::Queued,
            DataEvent::Flash {
                text: format!("{what}."),
                undoable: true,
            },
        ]
    } else {
        Vec::new()
    }
}

/// What to tell the UI after a change that was also queued for a server.
fn queued(did_change: bool) -> Vec<DataEvent> {
    if did_change {
        vec![DataEvent::StoreChanged, DataEvent::Queued]
    } else {
        Vec::new()
    }
}

/// Seconds since the Unix epoch.
fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// The UI's end of the worker. Dropping it stops the worker thread.
pub struct DataHandle {
    requests: mpsc::Sender<DataRequest>,
}

impl DataHandle {
    /// Queues a request. Returns `false` when the worker has gone.
    pub fn send(&self, request: DataRequest) -> bool {
        self.requests.send(request).is_ok()
    }
}

/// Moves the store onto its own thread. Answers arrive on `events`.
pub fn spawn(mut store: Store, events: UnboundedSender<DataEvent>) -> std::io::Result<DataHandle> {
    let (requests, queue) = mpsc::channel::<DataRequest>();
    thread::Builder::new()
        .name("quill-store".to_owned())
        .spawn(move || {
            while let Ok(request) = queue.recv() {
                for event in handle(&mut store, request) {
                    if events.send(event).is_err() {
                        return;
                    }
                }
            }
        })?;
    Ok(DataHandle { requests })
}

#[cfg(test)]
mod tests {
    use quill_core::types::{
        Address, Flags, FolderId, FolderRole, Headers, RemoteFolder, RemoteId, RemoteMessage,
        SyncCursor, SyncDelta,
    };

    use super::*;

    fn store_with_one_message() -> (Store, Mailbox, LocalMessageId) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        let folder = store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId("inbox".to_owned()),
                    name: "Inbox".to_owned(),
                    role: FolderRole::Inbox,
                },
            )
            .unwrap();
        let remote = RemoteId("1".to_owned());
        let delta = SyncDelta {
            added: vec![RemoteMessage {
                id: remote.clone(),
                headers: Headers {
                    message_id: None,
                    subject: "Hello".to_owned(),
                    from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                    to: Vec::new(),
                    cc: Vec::new(),
                    date: 100,
                },
                flags: Flags::default(),
                size: 10,
                has_attachments: false,
            }],
            changed: Vec::new(),
            removed: Vec::new(),
            reset: false,
            has_more: false,
            next_cursor: SyncCursor("c1".to_owned()),
        };
        store.apply_delta(folder, &delta).unwrap();
        let id = store.local_message_id(folder, &remote).unwrap().unwrap();
        store.put_body(id, "Body text").unwrap();
        (store, Mailbox::Folder(folder), id)
    }

    #[test]
    fn loads_answer_with_the_matching_event() {
        let (mut store, mailbox, id) = store_with_one_message();
        match handle(&mut store, DataRequest::LoadSidebar).as_slice() {
            [DataEvent::SidebarLoaded { accounts, folders }] => {
                assert_eq!(accounts.len(), 1);
                assert_eq!(folders[0].unread, 1);
            }
            other => panic!("{other:?}"),
        }
        match handle(&mut store, DataRequest::LoadMessages(mailbox)).as_slice() {
            [
                DataEvent::MessagesLoaded {
                    mailbox: loaded,
                    rows,
                },
            ] => {
                assert_eq!(*loaded, mailbox);
                assert_eq!(rows[0].headers.subject, "Hello");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            handle(&mut store, DataRequest::LoadBody(id)),
            [DataEvent::BodyLoaded {
                id,
                body: Some(LoadedBody {
                    text: "Body text".to_owned(),
                    html: None,
                    html_first: false,
                    has_plain: true,
                })
            }]
        );
        let missing = LocalMessageId(999);
        assert_eq!(
            handle(&mut store, DataRequest::LoadBody(missing)),
            [DataEvent::BodyLoaded {
                id: missing,
                body: None
            }]
        );
    }

    #[test]
    fn a_write_announces_the_change_only_when_something_changed() {
        let (mut store, _, id) = store_with_one_message();
        assert_eq!(
            handle(&mut store, DataRequest::SetSeen { id, seen: true }),
            [DataEvent::StoreChanged, DataEvent::Queued]
        );
        assert_eq!(
            handle(&mut store, DataRequest::SetFlagged { id, flagged: true }),
            [DataEvent::StoreChanged, DataEvent::Queued]
        );
        let missing = LocalMessageId(999);
        assert!(
            handle(
                &mut store,
                DataRequest::SetSeen {
                    id: missing,
                    seen: true
                }
            )
            .is_empty()
        );
    }

    #[tokio::test]
    async fn the_worker_thread_answers_over_the_channel_and_stops_when_dropped() {
        let (store, mailbox, _) = store_with_one_message();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(store, tx).unwrap();
        assert!(handle.send(DataRequest::LoadMessages(mailbox)));
        match rx.recv().await {
            Some(DataEvent::MessagesLoaded { rows, .. }) => assert_eq!(rows.len(), 1),
            other => panic!("{other:?}"),
        }
        drop(handle);
        // The worker exits, dropping its sender, which closes the channel.
        assert_eq!(rx.recv().await, None);
    }

    #[test]
    fn a_downloaded_message_brings_its_html() {
        let (mut store, _, id) = store_with_one_message();
        let raw = b"Subject: Hello\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                    <p>Your hold is <b>ready</b>.</p>\r\n";
        store.put_message(id, "Your hold is ready.", raw).unwrap();
        match handle(&mut store, DataRequest::LoadBody(id)).as_slice() {
            [
                DataEvent::BodyLoaded {
                    body: Some(body), ..
                },
            ] => {
                assert_eq!(body.text, "Your hold is ready.");
                assert!(body.html_first && !body.has_plain);
                assert_eq!(
                    body.html.as_deref().map(str::trim_end),
                    Some("<p>Your hold is <b>ready</b>.</p>")
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_plain_message_has_no_html_to_bring() {
        let (mut store, _, id) = store_with_one_message();
        store
            .put_message(id, "Just text.", b"Subject: Hello\r\n\r\nJust text.\r\n")
            .unwrap();
        assert_eq!(
            handle(&mut store, DataRequest::LoadBody(id)),
            [DataEvent::BodyLoaded {
                id,
                body: Some(LoadedBody {
                    text: "Just text.".to_owned(),
                    html: None,
                    html_first: false,
                    has_plain: true,
                })
            }]
        );
    }
}
