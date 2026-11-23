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
use crate::hook::{self, Hook, HookInput, HookRun};
use crate::outgoing::{self, Response, SendRequest};
use crate::vim::{self, Verdict};

/// What a message's page is wanted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageUse {
    /// The user's browser.
    Browser,
    /// The web reading pane, which stands in the reader's place and
    /// follows the list.
    Reading,
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
    /// Remember that an account's folders are on show in the accounts
    /// pane, or folded away. Nothing is answered: the pane already shows
    /// it.
    SetAccountOpen {
        account: LocalAccountId,
        open: bool,
    },
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
        /// For a dark window: the message's own colours are made ones
        /// for a dark page.
        dark: bool,
        then: PageUse,
    },
    /// Write a message's text out as a page for the web reading pane:
    /// one whose downloaded copy is not at hand to make its page from (a
    /// draft, or mail whose copy was let go). Answered as
    /// [`DataEvent::PageReady`].
    TextPage {
        message: LocalMessageId,
        title: String,
        text: String,
    },
    /// Write words of Quill's own out as a page for the web reading pane:
    /// why there is no message in it.
    NotePage {
        text: String,
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
    /// Answer the invitation a message carries: build the reply to its
    /// organizer, with the calendar's own form of the answer attached, and
    /// put it in the outbox.
    AnswerInvite(Box<outgoing::InviteAnswer>),
    /// Take a draft, or mail from the outbox, up again.
    OpenDraft(DraftId),
    /// Send a message that is in the outbox now, as it is: one waiting
    /// for its time or to be tried again, or one that was not sent.
    SendNow(DraftId),
    /// Put these messages, which are in a trash folder, back where they
    /// were before, or in their inbox when that is not known.
    PutBack(Vec<LocalMessageId>),
    /// Put back the message that went to a trash folder last.
    PutBackLast,
    /// Put a second copy of these messages into a folder of their account.
    CopyTo {
        ids: Vec<LocalMessageId>,
        folder: LocalFolderId,
    },
    /// Make a folder on this Mac for the account, which no server has.
    NewLocalFolder {
        account: LocalAccountId,
        name: String,
    },
    /// Take away a folder made on this Mac, if it is empty.
    RemoveLocalFolder(LocalFolderId),
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
    /// Write a message out as a file of its own, named for `subject`:
    /// made ready for the host to ask where it goes (`ask`, answered as
    /// [`DataEvent::Extracted`] to be saved), or put in the Downloads
    /// folder.
    SaveMessage {
        message: LocalMessageId,
        subject: String,
        what: files::SaveAs,
        ask: bool,
    },
    /// Write the message being written out as a draft file for Vim, in a
    /// folder of its own. `plugin`: put a copy of Quill's Vim plugin
    /// beside it, for a Vim that is not Quill's own. `share`: all of the
    /// message, or its text alone.
    EditOut {
        account: LocalAccountId,
        content: Box<DraftContent>,
        plugin: bool,
        share: vim::Share,
    },
    /// Vim has gone: read the draft file and what Vim said to do with it,
    /// and remove the folder. `before` is the draft as it was written out.
    EditBack {
        dir: std::path::PathBuf,
        before: Box<DraftContent>,
    },
    /// Remove a draft's folder unread: Vim never ran on it, or what it
    /// left is not wanted.
    EditDrop {
        dir: std::path::PathBuf,
    },
    /// Tell the Vim running on a draft's folder to do this with it now.
    EditTell {
        dir: std::path::PathBuf,
        verdict: Verdict,
    },
    /// Nothing to do but answer: the answer comes when everything asked
    /// before this has been done.
    Settle,
    /// Read a message in the form a hook takes it, ready to give to the
    /// hook's program. Nothing is run here.
    ReadForHook {
        message: LocalMessageId,
        hook: Box<Hook>,
    },
    /// The last changes made from outside Quill's window, for the
    /// settings window to show.
    AgentLog,
    /// The user stopped the agents: mail they sent that has not gone is
    /// taken back to Drafts.
    AgentsStop,
    /// Say what rules would do with the mail in the inboxes, as far back
    /// as they look: the rule called `which`, or every rule that is on.
    /// Nothing is changed. `text` tells this answer from the answer to
    /// what was asked before it. Answered with
    /// [`DataEvent::RulesTried`].
    RulesTry {
        text: String,
        set: std::sync::Arc<quill_sync::RuleSet>,
        which: Option<String>,
    },
    /// Run rules on that mail now, by the user's hand: the rule called
    /// `which`, or every rule that is on. Answered with
    /// [`DataEvent::RulesRan`].
    RulesRun {
        set: std::sync::Arc<quill_sync::RuleSet>,
        which: Option<String>,
    },
    /// What rules have done lately, from the log. Answered with
    /// [`DataEvent::RulesLog`].
    RulesLog,
    /// Take back what a rule did: its line of the log, by number.
    RulesUndo(i64),
    /// Something asked from outside Quill's window: by a command for a
    /// shell, or by an agent over MCP. Done here so that there is one
    /// writer, and answered with [`DataEvent::AgentDone`] under the same
    /// `ticket`, after whatever the UI is to hear of it.
    Agent {
        ticket: u64,
        caller: Box<crate::agent::Caller>,
        call: Box<crate::agent::Call>,
        settings: Box<crate::agent::Settings>,
    },
}

/// How many of the last changes made from outside the window the
/// settings window shows.
const RECENT_CHANGES: usize = 8;
/// How many of the log's changes are looked through for what rules did,
/// and how many of those are shown.
const RULES_LOG_LOOKS: usize = 500;
const RULES_LOG_SHOWN: usize = 100;

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
        /// The accounts whose folders were folded away when the accounts
        /// pane was last used. Every other account is open.
        closed: Vec<LocalAccountId>,
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
    /// A page for the web reading pane could not be written, and why.
    PageFailed {
        message: LocalMessageId,
        problem: String,
    },
    /// A page of Quill's own words is written and ready to show.
    NoteReady {
        text: String,
        page: WebPage,
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
    /// The message being written is in a draft file at `path`, in the
    /// folder `dir`, for Vim.
    EditReady {
        dir: std::path::PathBuf,
        path: std::path::PathBuf,
        /// The line of the file its body starts on, counting from 1.
        body_line: u32,
    },
    /// It could not be written out, and why. Nothing was left behind.
    EditFailed(String),
    /// What was written in Vim, and what Vim said to do with it.
    EditBack {
        file: Box<quill_store::draft_file::DraftFile>,
        verdict: Option<Verdict>,
    },
    /// What was written in Vim could not be read, and why.
    EditUnread(String),
    /// The answer to [`DataRequest::Settle`].
    Settled,
    /// A hook and its message, ready to run.
    HookReady(Box<HookRun>),
    /// The message cannot be given to a hook, and why.
    HookUnread(String),
    /// Something done, in a few words, to show until the next key.
    /// `undoable` says the undo command would take it back.
    Flash {
        text: String,
        undoable: bool,
    },
    /// The answer to [`DataRequest::AgentLog`], newest first.
    AgentLog(Vec<quill_store::AgentEntry>),
    /// What rules would change, newest first, and what they would do
    /// with it, in words. `count` is how many messages in all; `more`
    /// that there are more than the list shows.
    RulesTried {
        text: String,
        rows: Vec<MessageRow>,
        more: bool,
        says: String,
        count: usize,
    },
    /// Rules were run by hand: what they did, in a line, and what they
    /// could not do. `undoable` says the undo command would take it
    /// back.
    RulesRan {
        said: String,
        problems: Vec<String>,
        undoable: bool,
    },
    /// The answer to [`DataRequest::RulesLog`], newest first.
    RulesLog(Vec<quill_store::AgentEntry>),
    /// What came of a [`DataRequest::Agent`].
    AgentDone {
        ticket: u64,
        result: Box<Result<crate::agent::Answer, crate::agent::Refusal>>,
        /// What the user is to be told of it, when the mail was changed.
        notice: Option<crate::agent::Notice>,
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
    /// The calendar invitation the message carries, when it carries one.
    pub invite: Option<Box<crate::invite::Invitation>>,
    /// The text is here and the message's downloaded copy is not: it was
    /// let go to save disk space, or never kept. Its HTML, files and
    /// invitation come with the copy, once it is fetched again.
    pub copy_missing: bool,
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
        DataRequest::AnswerInvite(answer) => outgoing::answer_invite(store, &answer)?,
        DataRequest::OpenDraft(draft) => outgoing::open_draft(store, draft)?,
        DataRequest::SendNow(draft) => outgoing::send_now(store, draft)?,
        DataRequest::LoadSidebar => vec![DataEvent::SidebarLoaded {
            accounts: store.accounts()?,
            folders: store.folders()?,
            drafts: u32::try_from(store.drafts()?.len()).unwrap_or(u32::MAX),
            outbox: u32::try_from(store.outbox()?.len()).unwrap_or(u32::MAX),
            snoozed: store.snoozed()?,
            closed: store.closed_accounts()?,
        }],
        DataRequest::SetAccountOpen { account, open } => {
            store.set_account_open(account, open)?;
            Vec::new()
        }
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
                        // Sent from outside the window, and not to go
                        // until the user says so.
                        (DraftState::Held, _) => {
                            let who = draft
                                .agent
                                .as_deref()
                                .map(crate::text::sanitize_line)
                                .unwrap_or_else(|| "an agent".to_owned());
                            text.push_str(&format!(
                                "Waiting for your OK. Written by {who}. Send it from here to let \
                                 it go, or delete it.\n\n"
                            ));
                        }
                        (DraftState::Outbox, Some(error)) => {
                            text.push_str(&format!("Will be tried again. Last time: {error}\n\n"));
                        }
                        _ => {}
                    }
                    text.push_str(&draft.content.body);
                    LoadedBody {
                        text,
                        has_plain: true,
                        ..LoadedBody::default()
                    }
                });
            vec![DataEvent::BodyLoaded { id, body }]
        }
        DataRequest::LoadBody(id) => {
            let body = store.body(id)?.map(|text| {
                // The HTML is read from the message as it arrived. If that
                // cannot be had, the text is still worth showing.
                let raw = store.raw(id).ok().flatten();
                let copy_missing = raw.is_none();
                let invite = raw
                    .as_deref()
                    .and_then(crate::invite::Invitation::find)
                    .map(Box::new);
                match raw.and_then(|raw| quill_mime::parse(&raw, 0)) {
                    Some(parsed) => LoadedBody {
                        text,
                        html: parsed.html,
                        html_first: parsed.html_first,
                        has_plain: parsed.has_plain,
                        files: files::attached(&parsed.attachments),
                        unsubscribe: parsed.unsubscribe,
                        invite,
                        copy_missing,
                    },
                    None => LoadedBody {
                        text,
                        has_plain: true,
                        invite,
                        copy_missing,
                        ..LoadedBody::default()
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
            let at = now();
            match deletion(store, id)? {
                Deletion::ToTrash { from, trash } => {
                    // Where it was, so that it can be put back.
                    store.note_trashed(id, from, at)?;
                    moved(store, id, trash, "Moved to the trash")?
                }
                // In the trash already, or the account has none: for good.
                Deletion::ForGood => {
                    let queued_one = store.queue_delete(id, at, at + UNDO_SECONDS)?.is_some();
                    done(queued_one, "Deleted for good")
                }
                // Mail that is only here is no server's to keep in a
                // trash folder: it goes from this Mac.
                Deletion::FromThisMac => {
                    let queued_one = store.queue_delete(id, at, at + UNDO_SECONDS)?.is_some();
                    done(queued_one, "Deleted from this Mac for good")
                }
            }
        }
        DataRequest::MoveTo { id, folder } => {
            let what = move_words(store, id, folder)?;
            moved(store, id, folder, what)?
        }
        DataRequest::PutBack(ids) => put_back(store, &ids)?,
        DataRequest::PutBackLast => match store.last_trashed()? {
            Some((id, _)) => put_back(store, &[id])?,
            None => vec![flash(
                "Nothing to put back: no message deleted here is still in a trash folder.",
            )],
        },
        DataRequest::CopyTo { ids, folder } => {
            let at = now();
            let mut copying = 0usize;
            for id in &ids {
                copying += usize::from(store.queue_copy(*id, folder, at)?.is_some());
            }
            let name = folder_name(store, folder)?;
            if copying == 0 {
                vec![flash(
                    "Nothing was copied: the folder belongs to another account.",
                )]
            } else {
                vec![
                    DataEvent::Queued,
                    flash(&match copying {
                        1 => format!("Copying it to {name}."),
                        many => format!("Copying {many} messages to {name}."),
                    }),
                ]
            }
        }
        DataRequest::NewLocalFolder { account, name } => {
            match store.create_local_folder(account, &name)? {
                Ok(_) => vec![
                    DataEvent::StoreChanged,
                    flash(&format!("Made {}, on this Mac.", name.trim())),
                ],
                Err(problem) => vec![flash(&problem)],
            }
        }
        DataRequest::RemoveLocalFolder(folder) => {
            let name = folder_name(store, folder)?;
            if store.delete_empty_local_folder(folder)? {
                vec![
                    DataEvent::StoreChanged,
                    flash(&format!("Removed {name} from this Mac.")),
                ]
            } else {
                vec![flash(&format!(
                    "{name} has mail in it. Move or delete that first."
                ))]
            }
        }
        DataRequest::Many { ids, what } => many(store, &ids, what)?,
        DataRequest::WebPage {
            message,
            remote,
            dark,
            then,
        } => match files::web_page(store, files, message, remote, dark) {
            Ok(page) => vec![DataEvent::PageReady {
                message,
                page,
                then,
            }],
            // The reading pane must not go on showing another message:
            // the reason is shown there, in this one's place.
            Err(problem) if then == PageUse::Reading => vec![DataEvent::PageFailed {
                message,
                problem: problem.0,
            }],
            Err(problem) => vec![flash(&problem.0)],
        },
        DataRequest::TextPage {
            message,
            title,
            text,
        } => match files::text_page(files, message, &title, &text) {
            Ok(page) => vec![DataEvent::PageReady {
                message,
                page,
                then: PageUse::Reading,
            }],
            Err(problem) => vec![DataEvent::PageFailed {
                message,
                problem: problem.0,
            }],
        },
        DataRequest::NotePage { text } => match files::note_page(files, &text) {
            Ok(page) => vec![DataEvent::NoteReady { text, page }],
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
        DataRequest::SaveMessage {
            message,
            subject,
            what,
            ask,
        } => match files::save_message(store, files, message, &subject, what, ask) {
            Ok(files::SavedMessage::Ready(file)) => vec![DataEvent::Extracted {
                files: vec![file],
                index: 0,
                then: FileUse::Save,
            }],
            Ok(files::SavedMessage::InDownloads(name)) => {
                vec![flash(&format!("Saved {name} in Downloads."))]
            }
            Err(problem) => vec![flash(&problem.0)],
        },
        DataRequest::EditOut {
            account,
            content,
            plugin,
            share,
        } => match vim::hand_out(store, files, account, &content, plugin, share) {
            Ok(out) => vec![DataEvent::EditReady {
                dir: out.dir,
                path: out.path,
                body_line: out.body_line,
            }],
            Err(why) => vec![DataEvent::EditFailed(why)],
        },
        DataRequest::EditBack { dir, before } => match vim::take_back(files, &dir, &before) {
            Ok((file, verdict)) => vec![DataEvent::EditBack {
                file: Box::new(file),
                verdict,
            }],
            Err(why) => vec![DataEvent::EditUnread(why)],
        },
        DataRequest::EditDrop { dir } => {
            vim::drop_folder(files, &dir);
            Vec::new()
        }
        DataRequest::EditTell { dir, verdict } => match vim::tell(files, &dir, verdict) {
            Ok(()) => Vec::new(),
            Err(why) => vec![flash(&why)],
        },
        DataRequest::Settle => vec![DataEvent::Settled],
        DataRequest::AgentLog => {
            // What was done from outside the window: not what the
            // user's own rules did, which the log keeps beside it.
            let lines = store
                .agent_log(RECENT_CHANGES * 8, false)?
                .into_iter()
                .filter(|entry| entry.way != quill_sync::rules::LOG_WAY)
                .take(RECENT_CHANGES)
                .collect();
            vec![DataEvent::AgentLog(lines)]
        }
        DataRequest::AgentsStop => crate::agent::stop(store, now())?,
        DataRequest::RulesTry { text, set, which } => {
            let at = now();
            let hand = quill_sync::rules::by_hand(store, &set, which.as_deref(), true, at, at)?;
            let count = hand.changed.len();
            let shown: Vec<LocalMessageId> =
                hand.changed.iter().copied().take(SEARCH_LIMIT).collect();
            let mut rows = store.message_rows(&shown)?;
            rows.sort_by_key(|row| std::cmp::Reverse((row.headers.date, row.id.0)));
            vec![DataEvent::RulesTried {
                text,
                rows,
                more: count > SEARCH_LIMIT,
                says: hand.would(),
                count,
            }]
        }
        DataRequest::RulesRun { set, which } => {
            let at = now();
            let hand = quill_sync::rules::by_hand(
                store,
                &set,
                which.as_deref(),
                false,
                at,
                at + UNDO_SECONDS,
            )?;
            let mut events = vec![DataEvent::StoreChanged];
            if hand.queued {
                events.push(DataEvent::Queued);
            }
            events.push(DataEvent::RulesRan {
                said: quill_sync::rules::said_by_hand(&hand),
                undoable: !hand.operations.is_empty(),
                problems: hand.problems,
            });
            events
        }
        DataRequest::RulesLog => {
            let lines = store
                .agent_log(RULES_LOG_LOOKS, false)?
                .into_iter()
                .filter(|entry| entry.way == quill_sync::rules::LOG_WAY)
                .take(RULES_LOG_SHOWN)
                .collect();
            vec![DataEvent::RulesLog(lines)]
        }
        DataRequest::RulesUndo(entry) => match crate::agent::take_back(store, entry, now()) {
            Ok(taken) => {
                let mut events = vec![DataEvent::StoreChanged];
                if taken.queued {
                    events.push(DataEvent::Queued);
                }
                events.push(DataEvent::Flash {
                    text: taken.said,
                    undoable: taken.undoable,
                });
                events
            }
            Err(why) => vec![flash(&why)],
        },
        DataRequest::Agent {
            ticket,
            caller,
            call,
            settings,
        } => {
            // A file an agent asks for goes in a folder of this run's
            // own, beside the ones written for viewing.
            let mut settings = *settings;
            if settings.files.is_none() {
                settings.files = files.view_dir.as_ref().map(|dir| dir.join("agents"));
            }
            let outcome = crate::agent::perform(store, &settings, &caller, &call, now());
            let mut events = outcome.events;
            events.push(DataEvent::AgentDone {
                ticket,
                result: Box::new(outcome.result),
                notice: outcome.notice,
            });
            events
        }
        DataRequest::ReadForHook { message, hook } => read_for_hook(store, message, &hook)?,
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

/// Reads a message for a hook: as it arrived, or as its headers and plain
/// text. A message that is not here in that form says so.
fn read_for_hook(
    store: &Store,
    message: LocalMessageId,
    hook: &Hook,
) -> quill_store::Result<Vec<DataEvent>> {
    let unread = |why: &str| Ok(vec![DataEvent::HookUnread(why.to_owned())]);
    let Some(row) = store.message_rows(&[message])?.into_iter().next() else {
        return unread("That message is no longer here.");
    };
    let not_downloaded = "That message has not been downloaded yet. Open it first.";
    let input = match hook.input {
        HookInput::Raw => match store.raw(message)? {
            Some(raw) => raw,
            None => return unread(not_downloaded),
        },
        HookInput::Text => match store.body(message)? {
            Some(body) => hook::as_text(&row, &body).into_bytes(),
            None => return unread(not_downloaded),
        },
    };
    Ok(vec![DataEvent::HookReady(Box::new(HookRun::new(
        hook, &row, input,
    )))])
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
pub(crate) fn folder_with_role(
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

/// What deleting a message comes to.
pub(crate) enum Deletion {
    /// Into its account's trash folder, from the folder it is in.
    ToTrash {
        from: LocalFolderId,
        trash: LocalFolderId,
    },
    /// Removed: it is in the trash already, or its account has none.
    ForGood,
    /// Removed: it is in a folder made on this Mac, and the trash is a
    /// server's.
    FromThisMac,
}

pub(crate) fn deletion(store: &Store, id: LocalMessageId) -> quill_store::Result<Deletion> {
    let Some(location) = store.message_location(id)? else {
        return Ok(Deletion::ForGood);
    };
    let Some(trash) = folder_with_role(store, id, FolderRole::Trash)? else {
        return Ok(Deletion::ForGood);
    };
    if location.folder == trash {
        return Ok(Deletion::ForGood);
    }
    let trash_is_here = store
        .folder_remote(trash)?
        .is_some_and(|remote| quill_store::is_kept(&remote));
    if quill_store::is_made_here(&location.remote_folder.0) && !trash_is_here {
        return Ok(Deletion::FromThisMac);
    }
    Ok(Deletion::ToTrash {
        from: location.folder,
        trash,
    })
}

/// What a folder is called, for saying where something went.
fn folder_name(store: &Store, folder: LocalFolderId) -> quill_store::Result<String> {
    Ok(store
        .folders()?
        .into_iter()
        .find(|row| row.id == folder)
        .map_or_else(
            || "that folder".to_owned(),
            |row| {
                let name = crate::text::sanitize_line(&row.name);
                if row.local {
                    format!("{name}, on this Mac")
                } else {
                    name
                }
            },
        ))
}

/// What is said of a move: plainly so when the message changes hands
/// between a server and this Mac.
fn move_words(
    store: &Store,
    id: LocalMessageId,
    folder: LocalFolderId,
) -> quill_store::Result<&'static str> {
    let from_here = store
        .message_location(id)?
        .is_some_and(|location| quill_store::is_made_here(&location.remote_folder.0));
    let to_here = store.is_local_folder(folder)?;
    Ok(match (from_here, to_here) {
        (false, true) => "Moved to this Mac: it is taken off the server",
        (true, false) => "Moved from this Mac to the server",
        _ => "Moved",
    })
}

/// Puts messages that are in a trash folder back where they were, as one
/// change to undo. One whose old folder is not known, or has gone, goes
/// to its account's inbox.
fn put_back(store: &mut Store, ids: &[LocalMessageId]) -> quill_store::Result<Vec<DataEvent>> {
    let at = now();
    let mut operations = Vec::new();
    let mut to_inbox = 0usize;
    let mut last = None;
    for &id in ids {
        let known = store.trashed_from(id)?;
        let Some(folder) = (match known {
            Some(folder) => Some(folder),
            None => folder_with_role(store, id, FolderRole::Inbox)?,
        }) else {
            continue;
        };
        if let Some(operation) = store.queue_move(id, folder, at, at + UNDO_SECONDS)? {
            store.forget_trashed(id)?;
            operations.push(operation);
            to_inbox += usize::from(known.is_none());
            last = Some(folder);
        }
    }
    store.group_operations(&operations)?;
    let text = match (operations.len(), last) {
        (0, _) | (_, None) => {
            return Ok(vec![flash(
                "Nothing was put back: it is not in a trash folder.",
            )]);
        }
        (1, Some(folder)) if to_inbox == 0 => {
            format!("Put back in {}.", folder_name(store, folder)?)
        }
        (1, Some(_)) => "Put back in the inbox: where it came from is not known.".to_owned(),
        (many, _) if to_inbox == 0 => format!("Put back {many} messages where they came from."),
        (many, _) => format!(
            "Put back {many} messages. {to_inbox} went to the inbox: where they came from is not known."
        ),
    };
    Ok(vec![
        DataEvent::StoreChanged,
        DataEvent::Queued,
        DataEvent::Flash {
            text,
            undoable: true,
        },
    ])
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
                operations.extend(match deletion(store, id)? {
                    Deletion::ToTrash { from, trash } => {
                        store.note_trashed(id, from, at)?;
                        store.queue_move(id, trash, at, at + UNDO_SECONDS)?
                    }
                    Deletion::ForGood | Deletion::FromThisMac => {
                        store.queue_delete(id, at, at + UNDO_SECONDS)?
                    }
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
                // Only its text was ever put here: the copy is to fetch.
                body: Some(LoadedBody {
                    text: "Body text".to_owned(),
                    has_plain: true,
                    copy_missing: true,
                    ..LoadedBody::default()
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
                    has_plain: true,
                    ..LoadedBody::default()
                })
            }]
        );
    }
}
