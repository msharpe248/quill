//! The store worker: the only place the UI's data comes from.
//!
//! The UI task never touches disk. It sends a [`DataRequest`] and carries on;
//! the worker thread runs the query against the store and answers with
//! [`DataEvent`]s on a channel the UI task selects on.

use std::sync::mpsc;
use std::thread;

use quill_core::types::LocalMessageId;
use quill_store::{AccountRow, FolderRow, Mailbox, MessageRow, Store};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRequest {
    LoadSidebar,
    LoadMessages(Mailbox),
    LoadBody(LocalMessageId),
    SetSeen { id: LocalMessageId, seen: bool },
    SetFlagged { id: LocalMessageId, flagged: bool },
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
        body: Option<String>,
    },
    /// Something in the store changed; views of it should reload.
    StoreChanged,
    Error(String),
}

/// Runs one request against the store. This is the whole of the worker's
/// logic, kept free of threads so tests can drive it directly.
pub fn handle(store: &mut Store, request: DataRequest) -> Vec<DataEvent> {
    match run(store, request) {
        Ok(events) => events,
        Err(err) => vec![DataEvent::Error(err.to_string())],
    }
}

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
        DataRequest::LoadBody(id) => vec![DataEvent::BodyLoaded {
            id,
            body: store.body(id)?,
        }],
        DataRequest::SetSeen { id, seen } => changed(store.set_seen(id, seen)?),
        DataRequest::SetFlagged { id, flagged } => changed(store.set_flagged(id, flagged)?),
    })
}

fn changed(did_change: bool) -> Vec<DataEvent> {
    if did_change {
        vec![DataEvent::StoreChanged]
    } else {
        Vec::new()
    }
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
                body: Some("Body text".to_owned())
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
            [DataEvent::StoreChanged]
        );
        assert_eq!(
            handle(&mut store, DataRequest::SetFlagged { id, flagged: true }),
            [DataEvent::StoreChanged]
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
}
