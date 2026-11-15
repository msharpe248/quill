//! The store worker's side of writing mail: completing addresses, saving
//! drafts, attaching files, starting replies, and turning a finished draft
//! into a message in the outbox.
//!
//! This runs on the worker thread, where disk work belongs. It builds the
//! message; it does not send it. Sending is the sync engine's, from the
//! outbox, when the message's time comes.

use std::hash::{BuildHasher, Hasher};

use quill_core::types::{Address, LocalAccountId, LocalMessageId};
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
            text: if delay > 0 {
                format!("Sending in {delay} seconds.")
            } else {
                "Sending.".to_owned()
            },
            undoable: delay > 0,
        },
    ])
}

/// Reads a file and files it among the blobs, ready to attach.
pub fn attach(store: &Store, path: &str) -> Vec<DataEvent> {
    let failed = |why: String| vec![DataEvent::AttachFailed(why)];
    // A pasted or dropped path may be quoted or escaped the way a shell
    // would write it.
    let cleaned = path
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .replace("\\ ", " ");
    let expanded = match (cleaned.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::Path::new(&home).join(rest),
        _ => std::path::PathBuf::from(&cleaned),
    };
    let name = expanded
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
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
pub fn open_draft(store: &mut Store, id: DraftId) -> quill_store::Result<Vec<DataEvent>> {
    let Some(row) = store.draft(id)? else {
        return Ok(vec![flash("That draft is gone.")]);
    };
    let mut events = Vec::new();
    match row.state {
        DraftState::Sending => {
            return Ok(vec![flash("It is being sent right now.")]);
        }
        DraftState::Outbox => {
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
