//! The store worker's side of writing mail: completing addresses, saving
//! drafts, attaching files, starting replies, and turning a finished draft
//! into a message in the outbox.
//!
//! This runs on the worker thread, where disk work belongs. It builds the
//! message; it does not send it. Sending is the sync engine's, from the
//! outbox, when the message's time comes.

use std::hash::{BuildHasher, Hasher};

use quill_core::types::{Address, LocalAccountId, LocalMessageId};
use quill_mime::invite::Answer;
use quill_mime::{BodyFormat, OutgoingAttachment, ReplyPosition, Warning};
use quill_store::{DraftAttachment, DraftContent, DraftId, DraftState, Store};

use crate::compose::{Field, Prefill, content_type_for, parse_recipients, written_address};
use crate::data::{DataEvent, DataRequest};

/// A finished draft and how to send it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub draft: Option<DraftId>,
    pub account: LocalAccountId,
    pub content: DraftContent,
    /// The user's name, to go with the account's address.
    pub from_name: Option<String>,
    pub format: BodyFormat,
    /// How long the message waits in the outbox, where it can be taken
    /// back: undo send.
    pub delay_seconds: i64,
    /// The local time zone's offset, for the Date header.
    pub utc_offset: i32,
    /// The user has seen the doubts about this message and wants it sent.
    pub confirmed: bool,
}

/// The largest file that can be attached. Most servers refuse a message
/// well below this once it is encoded.
const MAX_ATTACHMENT: u64 = 25 * 1024 * 1024;

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn flash(text: impl Into<String>) -> DataEvent {
    DataEvent::Flash {
        text: text.into(),
        undoable: false,
    }
}

pub fn contacts(store: &Store, typed: String) -> quill_store::Result<Vec<DataEvent>> {
    let people = store.contacts_matching(&typed, 6)?;
    Ok(vec![DataEvent::Contacts { typed, people }])
}

pub fn save_draft(
    store: &mut Store,
    draft: Option<DraftId>,
    account: LocalAccountId,
    content: &DraftContent,
    announce: bool,
) -> quill_store::Result<Vec<DataEvent>> {
    let Some(saved) = store.save_draft(draft, account, content, now())? else {
        return Ok(vec![DataEvent::Error(
            "The draft could not be saved: it is already on its way out.".to_owned(),
        )]);
    };
    let mut events = vec![
        DataEvent::DraftSaved { draft: saved },
        DataEvent::StoreChanged,
    ];
    if announce {
        events.push(flash("Saved to Drafts."));
    }
    Ok(events)
}

/// Sixteen bytes nobody could guess, for a Message-ID. The standard
/// library seeds each hasher's keys from the system's random source.
fn random_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for half in bytes.chunks_mut(8) {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos()),
        );
        half.copy_from_slice(&hasher.finish().to_be_bytes()[..half.len()]);
    }
    bytes
}

/// Checks a draft, builds the message and puts it in the outbox.
pub fn send(store: &mut Store, request: &SendRequest) -> quill_store::Result<Vec<DataEvent>> {
    let refuse = |why: String| Ok(vec![DataEvent::SendRefused(why)]);
    let Some(account) = store
        .accounts()?
        .into_iter()
        .find(|row| row.id == request.account)
    else {
        return refuse("The account this was to be sent from is gone.".to_owned());
    };
    let content = &request.content;
    let mut everyone = Vec::new();
    for (field, text) in [
        ("To", &content.to),
        ("Cc", &content.cc),
        ("Bcc", &content.bcc),
    ] {
        match parse_recipients(text) {
            Ok(people) => everyone.push(people),
            Err(problem) => return refuse(format!("{field}: {problem}.")),
        }
    }
    let (bcc, cc, to) = (
        everyone.pop().unwrap_or_default(),
        everyone.pop().unwrap_or_default(),
        everyone.pop().unwrap_or_default(),
    );
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return refuse("There is nobody to send it to. Add an address to To.".to_owned());
    }

    let mut attachments = Vec::new();
    for attachment in &content.attachments {
        match store.blob(&attachment.blob_hash)? {
            Some(data) => attachments.push(OutgoingAttachment {
                name: attachment.name.clone(),
                content_type: attachment.content_type.clone(),
                data,
            }),
            None => {
                return refuse(format!(
                    "The attached file {} is no longer at hand. Attach it again.",
                    attachment.name
                ));
            }
        }
    }
    let message = quill_mime::Outgoing {
        from: Address {
            name: request
                .from_name
                .clone()
                .filter(|name| !name.trim().is_empty()),
            email: account.address.clone(),
        },
        to,
        cc,
        bcc,
        subject: content.subject.trim().to_owned(),
        body: content.body.clone(),
        format: request.format,
        in_reply_to: content.in_reply_to.clone(),
        references: content.references.clone(),
        attachments,
        date: now(),
        message_id: quill_mime::new_message_id(&account.address, &random_bytes()),
    };
    if !request.confirmed {
        let doubts: Vec<String> = quill_mime::warnings(&message)
            .into_iter()
            .map(|warning| match warning {
                Warning::EmptySubject => "There is no subject.".to_owned(),
                Warning::MissingAttachment => {
                    "The message mentions an attachment and has none.".to_owned()
                }
            })
            .collect();
        if !doubts.is_empty() {
            return Ok(vec![DataEvent::SendDoubts(doubts)]);
        }
    }

    let raw = quill_mime::build(&message, request.utc_offset);
    let Some(raw_hash) = store.put_blob(&raw)? else {
        return refuse("The finished message could not be filed for sending.".to_owned());
    };
    let at = now();
    let Some(draft) = store.save_draft(request.draft, request.account, content, at)? else {
        return refuse("This message is already on its way out.".to_owned());
    };
    let envelope = quill_mime::envelope(&message);
    let delay = request.delay_seconds.max(0);
    if !store.queue_send(
        draft,
        &raw_hash,
        &envelope.from,
        &envelope.to,
        at + delay,
        at,
    )? {
        return refuse("This message is already on its way out.".to_owned());
    }
    Ok(vec![
        DataEvent::StoreChanged,
        DataEvent::Queued,
        DataEvent::SendQueued,
        DataEvent::Flash {
            text: match delay {
                0 => "Sending.".to_owned(),
                // The moment to take it back in.
                1..=120 => format!("Sending in {delay} seconds."),
                // Sent later, by choice.
                _ => format!(
                    "In the Outbox. It goes in about {}, if Quill is running then, or when it next is.",
                    about(delay)
                ),
            },
            undoable: delay > 0,
        },
    ])
}

/// An answer to the invitation a message carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteAnswer {
    /// The message that carries the invitation.
    pub message: LocalMessageId,
    pub answer: Answer,
    /// The user's name, to go with the account's address.
    pub from_name: Option<String>,
    /// How long the answer waits in the outbox, where it can be taken
    /// back.
    pub delay_seconds: i64,
    /// The local time zone's offset, for the Date header.
    pub utc_offset: i32,
}

/// What an answer to an invitation is called when it is attached, and what
/// kind of file it says it is.
pub const INVITE_REPLY_NAME: &str = "invite.ics";
pub const INVITE_REPLY_TYPE: &str = "text/calendar; method=REPLY; charset=utf-8";

/// Answers the invitation a message carries: a short message to its
/// organizer with the calendar's own form of the answer attached, put in
/// the outbox the way any message is.
///
/// Everything is read again from the message as it arrived. What the
/// reader was showing is not taken on trust.
pub fn answer_invite(
    store: &mut Store,
    request: &InviteAnswer,
) -> quill_store::Result<Vec<DataEvent>> {
    let id = request.message;
    let Some(location) = store.message_location(id)? else {
        return Ok(vec![flash("That message is gone.")]);
    };
    let Some(raw) = store.raw(id)? else {
        return Ok(vec![flash(
            "The invitation is not on this Mac right now. Open the message again, then answer.",
        )]);
    };
    let Some(invite) = quill_mime::invite::find(&raw) else {
        return Ok(vec![flash("This message carries no invitation.")]);
    };
    let Some(account) = store
        .accounts()?
        .into_iter()
        .find(|row| row.id == location.account)
    else {
        return Ok(vec![flash("The account this came to is gone.")]);
    };
    let organizer = match &invite.organizer {
        Some(organizer) if invite.asks(&account.address) => organizer,
        _ => {
            return Ok(vec![flash(
                "This invitation does not ask you for an answer.",
            )]);
        }
    };
    let name = request
        .from_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let calendar = invite.reply(request.answer, &account.address, name, &stamp);
    let Some(blob_hash) = store.put_blob(calendar.as_bytes())? else {
        return Ok(vec![flash("There is nowhere to keep the answer.")]);
    };
    let original = quill_mime::parse(&raw, location.date);
    let message_id = original
        .as_ref()
        .and_then(|parsed| parsed.headers.message_id.clone());
    let mut references = original.map(|parsed| parsed.thread()).unwrap_or_default();
    references.extend(message_id.clone());
    let what = if invite.summary.trim().is_empty() {
        "an invitation"
    } else {
        invite.summary.trim()
    };
    let content = DraftContent {
        to: written_address(
            organizer.name.as_deref().unwrap_or_default(),
            &organizer.address,
        ),
        cc: String::new(),
        bcc: String::new(),
        subject: format!("{}: {what}", request.answer.word()),
        body: format!(
            "{} has {}.\n",
            name.unwrap_or(&account.address),
            match request.answer {
                Answer::Accepted => "accepted",
                Answer::Declined => "declined",
                Answer::Tentative => "tentatively accepted",
            }
        ),
        in_reply_to: message_id,
        references,
        attachments: vec![DraftAttachment {
            name: INVITE_REPLY_NAME.to_owned(),
            content_type: INVITE_REPLY_TYPE.to_owned(),
            blob_hash,
            size: calendar.len() as u64,
        }],
        // Answering is replying: the invitation is marked answered once
        // the answer has gone.
        replied_to: Some(id.0),
    };
    let sent = send(
        store,
        &SendRequest {
            draft: None,
            account: account.id,
            content,
            from_name: name.map(str::to_owned),
            // One line of words: there is nothing to mark up.
            format: BodyFormat::Plain,
            delay_seconds: request.delay_seconds,
            utc_offset: request.utc_offset,
            // Nobody is asked about a message they did not write.
            confirmed: true,
        },
    )?;
    let delay = request.delay_seconds.max(0);
    let word = request.answer.word();
    Ok(sent
        .into_iter()
        .filter_map(|event| match event {
            // There is no message being written to put away.
            DataEvent::SendQueued => None,
            DataEvent::SendRefused(why) => {
                Some(flash(format!("The answer could not be sent. {why}")))
            }
            // Never raised for a message sent as confirmed.
            DataEvent::SendDoubts(_) => None,
            DataEvent::Flash { undoable, .. } => Some(DataEvent::Flash {
                // The status bar adds the key that takes it back.
                text: match delay {
                    0 => format!("{word}. The answer is on its way."),
                    1 => format!("{word}. The answer goes in 1 second."),
                    _ => format!("{word}. The answer goes in {delay} seconds."),
                },
                undoable,
            }),
            other => Some(other),
        })
        .collect())
}

/// A length of time in round words: "45 minutes", "3 hours", "2 days".
fn about(seconds: i64) -> String {
    let (count, unit) = match seconds {
        ..5_400 => ((seconds + 30) / 60, "minute"),
        5_400..129_600 => ((seconds + 1_800) / 3_600, "hour"),
        _ => ((seconds + 43_200) / 86_400, "day"),
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// Reads a file and files it among the blobs, ready to attach.
pub fn attach(store: &Store, path: &str, handed: Option<Handed>) -> Vec<DataEvent> {
    let events = read_attachment(store, path, handed.is_some());
    // A copy the host made for Quill has served its purpose, whether or
    // not it could be attached. Its folder goes with the last file in it.
    if handed == Some(Handed::Temporary) {
        let file = std::path::Path::new(path);
        let _ = std::fs::remove_file(file);
        if let Some(folder) = file.parent() {
            let _ = std::fs::remove_dir(folder);
        }
    }
    events
}

/// How a file the host handed over is to be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handed {
    /// The user's own file: read and left alone.
    Kept,
    /// A copy made for Quill: removed once read.
    Temporary,
}

fn read_attachment(store: &Store, path: &str, exact: bool) -> Vec<DataEvent> {
    let failed = |why: String| vec![DataEvent::AttachFailed(why)];
    // A path that was typed or pasted may be quoted or escaped the way a
    // shell would write it. One the host handed over is exactly the path.
    let cleaned = if exact {
        path.to_owned()
    } else {
        path.trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .replace("\\ ", " ")
    };
    let expanded = match (cleaned.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) if !exact => std::path::Path::new(&home).join(rest),
        _ => std::path::PathBuf::from(&cleaned),
    };
    let name = expanded
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A handed-over file is named by its name: where the host put its copy
    // means nothing to the user.
    let cleaned = if exact { name.clone() } else { cleaned };
    let size = match std::fs::metadata(&expanded) {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => return failed(format!("{cleaned} is not a file.")),
        Err(err) => return failed(format!("{cleaned} cannot be attached: {err}.")),
    };
    if size > MAX_ATTACHMENT {
        return failed(format!(
            "{name} is {} MB. The most that can be attached is {} MB.",
            size / (1024 * 1024),
            MAX_ATTACHMENT / (1024 * 1024)
        ));
    }
    let bytes = match std::fs::read(&expanded) {
        Ok(bytes) => bytes,
        Err(err) => return failed(format!("{cleaned} cannot be read: {err}.")),
    };
    match store.put_blob(&bytes) {
        Ok(Some(blob_hash)) => vec![DataEvent::Attached(DraftAttachment {
            content_type: content_type_for(&name).to_owned(),
            name,
            blob_hash,
            size,
        })],
        Ok(None) => failed("There is nowhere to keep the attached file.".to_owned()),
        Err(err) => failed(format!("{name} could not be kept: {err}.")),
    }
}

fn written(people: &[Address]) -> String {
    people
        .iter()
        .map(|person| written_address(person.name.as_deref().unwrap_or_default(), &person.email))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Starts a reply to, or a forward of, a message that has been downloaded.
pub fn respond(
    store: &Store,
    id: LocalMessageId,
    how: Response,
    date_line: &str,
    position: ReplyPosition,
) -> quill_store::Result<Vec<DataEvent>> {
    let Some(location) = store.message_location(id)? else {
        return Ok(vec![flash("That message is gone.")]);
    };
    let Some(raw) = store.raw(id)? else {
        // Only its headers are here so far. The answer is started again
        // once the rest has come.
        return Ok(vec![DataEvent::RespondNeedsBody(Box::new(
            DataRequest::Respond {
                id,
                how,
                date_line: date_line.to_owned(),
                position,
            },
        ))]);
    };
    let Some(original) = quill_mime::parse(&raw, location.date) else {
        return Ok(vec![flash("That message cannot be read.")]);
    };
    let mine: Vec<String> = store
        .accounts()?
        .into_iter()
        .map(|account| account.address)
        .collect();
    let (draft, title, focus) = match how {
        Response::Reply { all } => (
            quill_mime::reply(&original, all, &mine, position, date_line),
            if all { "Reply to all" } else { "Reply" },
            Field::Body,
        ),
        Response::Forward => (
            quill_mime::forward(&original, date_line),
            "Forward",
            Field::To,
        ),
    };
    Ok(vec![DataEvent::ComposeReady(Box::new(Prefill {
        draft: None,
        account: location.account,
        content: DraftContent {
            to: written(&draft.to),
            cc: written(&draft.cc),
            bcc: String::new(),
            subject: draft.subject,
            body: draft.body,
            in_reply_to: draft.in_reply_to,
            references: draft.references,
            attachments: Vec::new(),
            // Only a reply marks the original answered.
            replied_to: matches!(how, Response::Reply { .. }).then_some(id.0),
        },
        cursor: draft.cursor,
        focus,
        title,
        note: None,
    }))])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Reply { all: bool },
    Forward,
}

/// Takes a draft up again. Mail waiting in the outbox is taken back out of
/// it first; mail a server has right now cannot be touched.
/// What is said when a message in the outbox is sent at the user's word.
pub const SENDING_NOW: &str = "Sending it now.";

/// Sends a message that is in the outbox now, as it is, at the user's
/// word: one waiting for its time or to be tried again, or one that was
/// not sent and was waiting for them. The engine is told there is mail to
/// send.
pub fn send_now(store: &mut Store, id: DraftId) -> quill_store::Result<Vec<DataEvent>> {
    let Some(row) = store.draft(id)? else {
        return Ok(vec![flash("That message is gone.")]);
    };
    match row.state {
        DraftState::Sending => Ok(vec![flash("It is being sent right now.")]),
        DraftState::Draft => Ok(vec![flash("That is a draft: open it to send it.")]),
        DraftState::Outbox | DraftState::Failed | DraftState::Held => {
            if store.send_now(id, now())? {
                Ok(vec![
                    DataEvent::StoreChanged,
                    DataEvent::Queued,
                    flash(SENDING_NOW),
                ])
            } else {
                Ok(vec![flash(
                    "The finished message is missing. Open it and send it again.",
                )])
            }
        }
    }
}

pub fn open_draft(store: &mut Store, id: DraftId) -> quill_store::Result<Vec<DataEvent>> {
    let Some(row) = store.draft(id)? else {
        return Ok(vec![flash("That draft is gone.")]);
    };
    let mut events = Vec::new();
    match row.state {
        DraftState::Sending => {
            return Ok(vec![flash("It is being sent right now.")]);
        }
        DraftState::Outbox | DraftState::Held => {
            if !store.unsend(id, now())? {
                return Ok(vec![flash("It is being sent right now.")]);
            }
            events.push(DataEvent::StoreChanged);
            events.push(flash("Taken back from the outbox."));
        }
        DraftState::Draft | DraftState::Failed => {}
    }
    let cursor = row.content.body.len();
    // A message a server would not take says so as it opens.
    let note = row.error.as_ref().map(|why| format!("Not sent: {why}"));
    events.push(DataEvent::ComposeReady(Box::new(Prefill {
        draft: Some(id),
        account: row.account,
        content: row.content,
        cursor,
        focus: Field::Body,
        title: "Draft",
        note,
    })));
    Ok(events)
}
