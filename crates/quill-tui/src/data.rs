//! The store worker: the only place the UI's data comes from.
//!
//! The UI task never touches disk. It sends a [`DataRequest`] and carries on;
//! the worker thread runs the query against the store and answers with
//! [`DataEvent`]s on a channel the UI task selects on.

use std::sync::{Mutex, mpsc};
use std::thread;

use quill_core::types::{Flags, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_mime::ReplyPosition;
use quill_store::{
    AccountRow, Contact, DraftAttachment, DraftContent, DraftId, DraftState, FolderRow, Mailbox,
    MessageRow, Store,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::compose::Prefill;
use crate::files::{self, AttachedFile, ExtractedFile, Files, RemoteContent, WebPage};
use crate::outgoing::{self, Response, SendRequest};

/// What a message's page is wanted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageUse {
    /// The web view pane beside the terminal.
    Pane,
    /// The user's browser.
    Browser,
}

/// What [`DataRequest::Many`] does to each of its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bulk {
    Archive,
    Delete,
    MoveTo(LocalFolderId),
    Seen(bool),
    Flagged(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// One change to several messages, taken back together by one undo.
    Many {
        ids: Vec<LocalMessageId>,
        what: Bulk,
    },
    /// Write the message's HTML out as a safe page, to show or to open.
    WebPage {
        message: LocalMessageId,
        remote: RemoteContent,
        then: PageUse,
    },
    /// Let this sender's mail load remote content from now on.
    AllowRemote {
        address: String,
    },
    /// Look up mail that has just arrived, to tell the user about it.
    Announce(Vec<LocalMessageId>),
    /// Put messages aside until a time; with no time, bring them back now.
    Snooze {
        ids: Vec<LocalMessageId>,
        until: Option<i64>,
    },
    /// Bring back the snoozed messages whose time has come.
    Wake,
    /// Take back the last change that still can be.
    Undo,
    /// People whose name or address starts with what was typed.
    Contacts {
        typed: String,
    },
    SaveDraft {
        draft: Option<DraftId>,
        account: LocalAccountId,
        content: Box<DraftContent>,
        /// Say so in the status bar: the user asked for it.
        announce: bool,
    },
    Send(Box<SendRequest>),
    /// Read the file at this path, to attach it. `handed`: the host
    /// supplied the path, which is then exact, and says what to do with
    /// the file afterwards. `None`: the user typed or pasted it.
    Attach {
        path: String,
        handed: Option<outgoing::Handed>,
    },
    /// Start a reply to, or a forward of, a message.
    Respond {
        id: LocalMessageId,
        how: Response,
        /// When the original was sent, in words, for the attribution line.
        date_line: String,
        position: ReplyPosition,
    },
    /// Take a draft, or mail from the outbox, up again.
    OpenDraft(DraftId),
    /// Write a message's attachments where a viewer can read them: all of
    /// them when they are to be looked at, so the viewer can step from one
    /// to the next, and otherwise the one at `part` alone.
    Extract {
        message: LocalMessageId,
        part: u32,
        then: FileUse,
    },
    /// Find mail. `text` is what was typed, to tell this answer from the
    /// answer to what was typed before it; `query` is that text, read.
    Search {
        text: String,
        query: Box<quill_core::search::Query>,
    },
    /// Save one attachment in the Downloads folder.
    SaveAttachment {
        message: LocalMessageId,
        part: u32,
    },
}

/// The most messages a search shows. Past this the thing to do is to say
/// more of what is wanted.
pub const SEARCH_LIMIT: usize = 500;

/// What a file taken out of a message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileUse {
    /// To look at or play, in the host's pane.
    View,
    /// To open in the application the Mac opens its kind with.
    Open,
    /// To save where the user chooses, in the host's panel.
    Save,
    /// To draw in the reader, above the message's text.
    Picture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataEvent {
    SidebarLoaded {
        accounts: Vec<AccountRow>,
        folders: Vec<FolderRow>,
        /// How many drafts are being written, and how much mail is on its
        /// way out or was refused.
        drafts: u32,
        outbox: u32,
        /// How many messages are snoozed, and when the next is due back.
        snoozed: (u32, Option<i64>),
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
    /// People to complete an address with.
    Contacts {
        typed: String,
        people: Vec<Contact>,
    },
    /// The message being written is saved as this draft.
    DraftSaved {
        draft: DraftId,
    },
    /// The message is in the outbox: there is nothing left to write.
    SendQueued,
    /// Things to look at before the message goes. Sending again sends it.
    SendDoubts(Vec<String>),
    /// The message cannot be sent as written, and why.
    SendRefused(String),
    Attached(DraftAttachment),
    AttachFailed(String),
    /// A message to start writing: a reply, a forward, or a draft.
    ComposeReady(Box<Prefill>),
    /// Mail that has just arrived, as far as it is still here.
    Arrived(Vec<MessageRow>),
    /// Messages were put aside until `until`.
    Snoozed {
        count: usize,
        until: i64,
    },
    /// A message's page is written and ready to show.
    PageReady {
        message: LocalMessageId,
        page: WebPage,
        then: PageUse,
    },
    /// What a search found, newest first. `more`: there were more than are
    /// shown.
    SearchLoaded {
        text: String,
        rows: Vec<MessageRow>,
        more: bool,
    },
    /// Files are out of the message and readable. `index` is the one that
    /// was asked for.
    Extracted {
        files: Vec<ExtractedFile>,
        index: usize,
        then: FileUse,
    },
    /// The message to answer has not been downloaded: this is the request
    /// to make again once it has.
    RespondNeedsBody(Box<DataRequest>),
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
    /// The files attached to the message.
    pub files: Vec<AttachedFile>,
    /// How to leave the list that sent the message, when it says.
    pub unsubscribe: Option<quill_mime::Unsubscribe>,
}

/// Runs one request against the store. This is the whole of the worker's
/// logic, kept free of threads so tests can drive it directly.
pub fn handle(store: &mut Store, request: DataRequest) -> Vec<DataEvent> {
    handle_with(store, &Files::default(), request)
}

/// As [`handle`], with somewhere to put files. Without it, a request that
/// needs to write one says that it cannot.
pub fn handle_with(store: &mut Store, files: &Files, request: DataRequest) -> Vec<DataEvent> {
    // The sync engine writes through a connection of its own. Finding the
    // database busy is not a failure to report, just a reason to ask again.
    let mut attempts = 0;
    loop {
        match run(store, files, &request) {
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

fn run(
    store: &mut Store,
    files: &Files,
    request: &DataRequest,
) -> quill_store::Result<Vec<DataEvent>> {
    Ok(match request.clone() {
        DataRequest::Contacts { typed } => outgoing::contacts(store, typed)?,
        DataRequest::SaveDraft {
            draft,
            account,
            content,
            announce,
        } => outgoing::save_draft(store, draft, account, &content, announce)?,
        DataRequest::Send(request) => outgoing::send(store, &request)?,
        DataRequest::Attach { path, handed } => outgoing::attach(store, &path, handed),
        DataRequest::Respond {
            id,
            how,
            date_line,
            position,
        } => outgoing::respond(store, id, how, &date_line, position)?,
        DataRequest::OpenDraft(draft) => outgoing::open_draft(store, draft)?,
        DataRequest::LoadSidebar => vec![DataEvent::SidebarLoaded {
            accounts: store.accounts()?,
            folders: store.folders()?,
            drafts: u32::try_from(store.drafts()?.len()).unwrap_or(u32::MAX),
            outbox: u32::try_from(store.outbox()?.len()).unwrap_or(u32::MAX),
            snoozed: store.snoozed()?,
        }],
        DataRequest::LoadMessages(mailbox) => vec![DataEvent::MessagesLoaded {
            mailbox,
            rows: store.messages(mailbox)?,
        }],
        DataRequest::LoadBody(id) if DraftId::from_message_id(id).is_some() => {
            // A draft's body is what has been written so far. Mail a server
            // refused says why above it.
            let body = DraftId::from_message_id(id)
                .map(|draft| store.draft(draft))
                .transpose()?
                .flatten()
                .map(|draft| {
                    let mut text = String::new();
                    match (draft.state, &draft.error) {
                        (DraftState::Failed, Some(error)) => {
                            text.push_str(&format!("Not sent: {error}\n\n"));
                        }
                        (DraftState::Outbox, Some(error)) => {
                            text.push_str(&format!("Will be tried again. Last time: {error}\n\n"));
                        }
                        _ => {}
                    }
                    text.push_str(&draft.content.body);
                    LoadedBody {
                        text,
                        html: None,
                        html_first: false,
                        has_plain: true,
                        files: Vec::new(),
                        unsubscribe: None,
                    }
                });
            vec![DataEvent::BodyLoaded { id, body }]
        }
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
                        files: files::attached(&parsed.attachments),
                        unsubscribe: parsed.unsubscribe,
                    },
                    None => LoadedBody {
                        text,
                        html: None,
                        html_first: false,
                        has_plain: true,
                        files: Vec::new(),
                        unsubscribe: None,
                    },
                }
            });
            vec![DataEvent::BodyLoaded { id, body }]
        }
        DataRequest::SetSeen { id, seen } => set_flags(store, id, |flags| Flags { seen, ..flags })?,
        DataRequest::SetFlagged { id, flagged } => {
            set_flags(store, id, |flags| Flags { flagged, ..flags })?
        }
        // A message that has gone since it was asked about (from a
        // notification, say) is nothing to complain of.
        DataRequest::Archive(id) if store.message_location(id)?.is_none() => Vec::new(),
        DataRequest::Archive(id) => match folder_with_role(store, id, FolderRole::Archive)? {
            Some(archive) => moved(store, id, archive, "Archived")?,
            None => vec![DataEvent::Error(
                "This account has no archive folder to move the message to.".to_owned(),
            )],
        },
        DataRequest::Delete(id) if DraftId::from_message_id(id).is_some() => {
            let removed = match DraftId::from_message_id(id) {
                Some(draft) => store.delete_draft(draft)?,
                None => false,
            };
            if removed {
                vec![DataEvent::StoreChanged, flash("Thrown away.")]
            } else {
                vec![flash(
                    "It is being sent right now and cannot be thrown away.",
                )]
            }
        }
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
        DataRequest::Many { ids, what } => many(store, &ids, what)?,
        DataRequest::WebPage {
            message,
            remote,
            then,
        } => match files::web_page(store, files, message, remote) {
            Ok(page) => vec![DataEvent::PageReady {
                message,
                page,
                then,
            }],
            Err(problem) => vec![flash(&problem.0)],
        },
        DataRequest::AllowRemote { address } => {
            store.allow_remote_content(&address, now())?;
            Vec::new()
        }
        DataRequest::Announce(ids) => vec![DataEvent::Arrived(store.message_rows(&ids)?)],
        DataRequest::Snooze {
            ids,
            until: Some(until),
        } => {
            let count = store.snooze(&ids, until)?;
            if count == 0 {
                Vec::new()
            } else {
                vec![DataEvent::StoreChanged, DataEvent::Snoozed { count, until }]
            }
        }
        DataRequest::Snooze { ids, until: None } => {
            let back = store.wake_snoozed(now(), Some(&ids))?;
            if back.is_empty() {
                Vec::new()
            } else {
                vec![
                    DataEvent::StoreChanged,
                    flash(if back.len() == 1 {
                        "Back in its folder."
                    } else {
                        "Back in their folders."
                    }),
                ]
            }
        }
        DataRequest::Wake => {
            let back = store.wake_snoozed(now(), None)?;
            if back.is_empty() {
                Vec::new()
            } else {
                // Mail that comes back is announced as mail that arrives.
                vec![
                    DataEvent::StoreChanged,
                    DataEvent::Arrived(store.message_rows(&back)?),
                ]
            }
        }
        DataRequest::Extract {
            message,
            part,
            then,
        } => {
            // To look at: every file, so the viewer can go from one to the
            // next. For anything else, the one asked for.
            let all = then == FileUse::View;
            match files::extract(store, files, message, part, all) {
                Ok((extracted, index)) => vec![DataEvent::Extracted {
                    files: extracted,
                    index,
                    then,
                }],
                Err(problem) => vec![flash(&problem.0)],
            }
        }
        DataRequest::Search { text, query } => {
            // One more than is shown, to know whether there are more.
            let mut rows = store.search(&query, SEARCH_LIMIT + 1)?;
            let more = rows.len() > SEARCH_LIMIT;
            rows.truncate(SEARCH_LIMIT);
            vec![DataEvent::SearchLoaded { text, rows, more }]
        }
        DataRequest::SaveAttachment { message, part } => {
            match files::save(store, files, message, part) {
                Ok(name) => vec![flash(&format!("Saved {name} in Downloads."))],
                Err(problem) => vec![flash(&problem.0)],
            }
        }
        DataRequest::Undo => {
            let at = now();
            let undone = match store.undoable(at)? {
                Some(operation) => store.undo_batch(operation)?,
                None => 0,
            };
            if undone > 1 {
                vec![
                    DataEvent::StoreChanged,
                    flash(&format!(
                        "Undone: {undone} messages are back where they were."
                    )),
                ]
            } else if undone == 1 {
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

/// Does one thing to several messages. The moves and deletions among them
/// are queued as one batch, so that one undo takes them all back.
fn many(
    store: &mut Store,
    ids: &[LocalMessageId],
    what: Bulk,
) -> quill_store::Result<Vec<DataEvent>> {
    let at = now();
    let mut operations = Vec::new();
    // Changes that are not queued moves or deletions: marks, and drafts
    // thrown away.
    let mut others = 0usize;
    let mut marks = false;
    for &id in ids {
        match what {
            Bulk::Seen(seen) => {
                if let Some(location) = store.message_location(id)?
                    && store.queue_flags(
                        id,
                        Flags {
                            seen,
                            ..location.flags
                        },
                        at,
                    )?
                {
                    marks = true;
                }
            }
            Bulk::Flagged(flagged) => {
                if let Some(location) = store.message_location(id)?
                    && store.queue_flags(
                        id,
                        Flags {
                            flagged,
                            ..location.flags
                        },
                        at,
                    )?
                {
                    marks = true;
                }
            }
            Bulk::Archive => {
                if let Some(archive) = folder_with_role(store, id, FolderRole::Archive)? {
                    operations.extend(store.queue_move(id, archive, at, at + UNDO_SECONDS)?);
                }
            }
            Bulk::MoveTo(folder) => {
                operations.extend(store.queue_move(id, folder, at, at + UNDO_SECONDS)?);
            }
            Bulk::Delete => {
                if let Some(draft) = DraftId::from_message_id(id) {
                    others += usize::from(store.delete_draft(draft)?);
                    continue;
                }
                let trash = folder_with_role(store, id, FolderRole::Trash)?;
                let already_there = store
                    .message_location(id)?
                    .is_some_and(|location| Some(location.folder) == trash);
                operations.extend(match trash {
                    Some(trash) if !already_there => {
                        store.queue_move(id, trash, at, at + UNDO_SECONDS)?
                    }
                    _ => store.queue_delete(id, at, at + UNDO_SECONDS)?,
                });
            }
        }
    }
    store.group_operations(&operations)?;

    if matches!(what, Bulk::Seen(_) | Bulk::Flagged(_)) {
        return Ok(queued(marks));
    }
    let done = operations.len() + others;
    let verb = match what {
        Bulk::Archive => "Archived",
        Bulk::Delete => "Deleted",
        _ => "Moved",
    };
    if done == 0 {
        // Say so: several messages were asked about and none moved.
        return Ok(vec![flash(match what {
            Bulk::Archive => "None of these can be archived: no archive folder to move them to.",
            Bulk::Delete => "None of these could be deleted.",
            _ => "None of these can be moved there: the folder belongs to another account.",
        })]);
    }
    let text = if done == ids.len() {
        format!("{verb} {done} messages.")
    } else {
        format!(
            "{verb} {done} of {} messages. The others stay where they are.",
            ids.len()
        )
    };
    let mut events = vec![DataEvent::StoreChanged];
    if !operations.is_empty() {
        events.push(DataEvent::Queued);
    }
    events.push(DataEvent::Flash {
        text,
        undoable: !operations.is_empty(),
    });
    Ok(events)
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
    requests: Mutex<Option<mpsc::Sender<DataRequest>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl DataHandle {
    /// Queues a request. Returns `false` when the worker has gone.
    pub fn send(&self, request: DataRequest) -> bool {
        self.requests
            .lock()
            .ok()
            .and_then(|requests| requests.as_ref().map(|tx| tx.send(request).is_ok()))
            .unwrap_or(false)
    }

    /// Takes no more requests and waits for those already made to be
    /// carried out. Whoever reads the worker's answers must still be
    /// there, or it stops at the first one it cannot deliver.
    pub fn finish(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.take();
        }
        let worker = self.worker.lock().ok().and_then(|mut worker| worker.take());
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

/// Moves the store onto its own thread. Answers arrive on `events`.
pub fn spawn(
    mut store: Store,
    files: Files,
    events: UnboundedSender<DataEvent>,
) -> std::io::Result<DataHandle> {
    let (requests, queue) = mpsc::channel::<DataRequest>();
    let worker = thread::Builder::new()
        .name("quill-store".to_owned())
        .spawn(move || {
            while let Ok(request) = queue.recv() {
                // With nobody left to hear the answers, what was asked is
                // still done: a draft saved on the way out is saved.
                for event in handle_with(&mut store, &files, request) {
                    if events.send(event).is_err() {
                        break;
                    }
                }
            }
        })?;
    Ok(DataHandle {
        requests: Mutex::new(Some(requests)),
        worker: Mutex::new(Some(worker)),
    })
}

#[cfg(test)]
mod tests {
    use quill_core::types::{
        Address, Flags, FolderId, FolderRole, Headers, RemoteFolder, RemoteId, RemoteMessage,
        SyncCursor, SyncDelta,
    };

    use super::*;

    #[test]
    fn finishing_waits_for_what_was_asked_even_with_nobody_listening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let mut store = Store::open(&path).unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        let (events, listener) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(store, Files::default(), events).unwrap();
        // The UI has gone.
        drop(listener);
        for subject in ["One", "Two"] {
            assert!(handle.send(DataRequest::SaveDraft {
                draft: None,
                account,
                content: Box::new(quill_store::DraftContent {
                    subject: subject.to_owned(),
                    ..Default::default()
                }),
                announce: true,
            }));
        }
        handle.finish();
        // Nothing more is taken, and both drafts were written.
        assert!(!handle.send(DataRequest::Undo));
        let drafts = Store::open(&path).unwrap().drafts().unwrap();
        assert_eq!(drafts.len(), 2);
    }

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
                thread: Vec::new(),
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
            [
                DataEvent::SidebarLoaded {
                    accounts, folders, ..
                },
            ] => {
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
                    files: Vec::new(),
                    unsubscribe: None,
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
        let handle = spawn(store, Files::default(), tx).unwrap();
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
                    files: Vec::new(),
                    unsubscribe: None,
                })
            }]
        );
    }
}
