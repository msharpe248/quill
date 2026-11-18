//! Writing mail: drafts, and sending.
//!
//! A message sent from outside Quill's window is checked and built as the
//! compose screen's is, and goes into the same outbox. Three things are
//! different about it: it is marked with who sent it; with a wait set, it
//! goes no sooner than that long after the last one sent this way, one
//! message at a time; and with holding set, it waits in the outbox until
//! the user lets it go.
//!
//! Nothing here loses the user's own writing: only a draft that was
//! written from outside the window is changed or discarded from outside
//! it.

use quill_core::types::{LocalAccountId, LocalMessageId};
use quill_mime::{BodyFormat, ReplyPosition};
use quill_store::{AccountRow, DraftContent, DraftId, DraftRow, DraftState, Store};
use serde_json::{Value, json};

use super::select::visible;
use super::show::when;
use super::{Call, Caller, Code, Done, Refusal, Settings};
use crate::data::DataEvent;
use crate::outgoing::{self, Response, SendRequest};
use crate::text;

/// The most messages sent from outside the window that may be waiting
/// their turn at once.
const MOST_WAITING: usize = 20;

/// What a message's state is called in an answer.
fn state_word(state: DraftState) -> &'static str {
    match state {
        DraftState::Draft => "draft",
        DraftState::Outbox => "waiting",
        DraftState::Sending => "sending",
        DraftState::Failed => "refused",
        DraftState::Held => "held",
    }
}

fn account_name(accounts: &[AccountRow], id: LocalAccountId) -> &str {
    accounts
        .iter()
        .find(|account| account.id == id)
        .map_or("", |account| account.name.as_str())
}

/// A draft, or a message in the outbox, as data: with its text only when
/// it is asked for by itself.
fn draft_json(row: &DraftRow, accounts: &[AccountRow], whole: bool) -> Value {
    let mut value = json!({
        "draft": row.id.0,
        "account": account_name(accounts, row.account),
        "state": state_word(row.state),
        "to": row.content.to,
        "cc": row.content.cc,
        "bcc": row.content.bcc,
        "subject": row.content.subject,
        "attachments": row.content.attachments.iter().map(|file| json!({
            "name": file.name,
            "type": file.content_type,
            "size": file.size,
        })).collect::<Vec<_>>(),
        "send_at": row.send_after.filter(|_| row.state != DraftState::Draft).map(when),
        "refused_because": row.error,
        "written_by": row.agent,
        "updated": when(row.updated),
    });
    if whole {
        value["body"] = json!(row.content.body);
    }
    value
}

fn draft_line(row: &DraftRow) -> String {
    format!(
        "{:>8}  {:<8}  {}  {}",
        row.id.0,
        state_word(row.state),
        text::fit(&text::sanitize_line(&row.content.to), 28),
        text::sanitize_line(&row.content.subject),
    )
}

/// The draft with this number, if it is of an account agents may see.
fn draft(store: &Store, settings: &Settings, id: i64) -> Result<DraftRow, Refusal> {
    let seen = visible(store, settings)?;
    store
        .draft(DraftId(id))?
        .filter(|row| seen.contains(&row.account))
        .ok_or_else(|| Refusal::not_found(format!("There is no draft {id}.")))
}

/// The drafts, or what is in the outbox.
pub(crate) fn list(store: &Store, settings: &Settings, outbox: bool) -> Result<Done, Refusal> {
    let seen = visible(store, settings)?;
    let accounts = store.accounts()?;
    let rows: Vec<DraftRow> = if outbox {
        store.outbox()?
    } else {
        store.drafts()?
    }
    .into_iter()
    .filter(|row| seen.contains(&row.account))
    .collect();
    let key = if outbox { "outbox" } else { "drafts" };
    let mut value = json!({});
    value[key] = rows
        .iter()
        .map(|row| draft_json(row, &accounts, false))
        .collect();
    let count = rows.len();
    Ok(Done::look(
        value,
        rows.iter().map(draft_line).collect(),
        0,
        format!("{key}: {count}"),
    ))
}

pub(crate) fn show(store: &Store, settings: &Settings, id: i64) -> Result<Done, Refusal> {
    let row = draft(store, settings, id)?;
    let accounts = store.accounts()?;
    let mut lines = vec![
        format!("To:      {}", text::sanitize_line(&row.content.to)),
        format!("Subject: {}", text::sanitize_line(&row.content.subject)),
        String::new(),
    ];
    lines.extend(
        text::sanitize_body(row.content.body.trim_end())
            .lines()
            .map(str::to_owned),
    );
    Ok(Done::look(
        draft_json(&row, &accounts, true),
        lines,
        1,
        format!("draft {id}"),
    ))
}

/// The account a message is to go from: the one named, or the first that
/// agents may see.
fn sender(store: &Store, settings: &Settings, named: Option<&str>) -> Result<AccountRow, Refusal> {
    let seen = visible(store, settings)?;
    let mut accounts = store
        .accounts()?
        .into_iter()
        .filter(|account| seen.contains(&account.id));
    match named {
        Some(name) => accounts
            .find(|account| account.name.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| {
                Refusal::not_found(format!(
                    "There is no account named \"{}\".",
                    text::sanitize_line(name)
                ))
            }),
        None => accounts.next().ok_or_else(|| {
            Refusal::not_found("There is no account to send from. Add one in Quill first.")
        }),
    }
}

/// Reads the files at these paths, to attach them.
fn attached(store: &Store, paths: &[String]) -> Result<Vec<quill_store::DraftAttachment>, Refusal> {
    let mut files = Vec::new();
    for path in paths {
        for event in outgoing::attach(store, path, None) {
            match event {
                DataEvent::Attached(file) => files.push(file),
                DataEvent::AttachFailed(why) => return Err(Refusal::invalid(why)),
                _ => {}
            }
        }
    }
    Ok(files)
}

/// A message's fields as they are given: lists of addresses, and text.
pub(crate) struct Fields<'a> {
    pub to: &'a [String],
    pub cc: &'a [String],
    pub bcc: &'a [String],
    pub subject: &'a str,
    pub body: &'a str,
    pub attach: &'a [String],
}

impl Fields<'_> {
    fn content(&self, store: &Store) -> Result<DraftContent, Refusal> {
        Ok(DraftContent {
            to: self.to.join(", "),
            cc: self.cc.join(", "),
            bcc: self.bcc.join(", "),
            subject: self.subject.to_owned(),
            body: self.body.to_owned(),
            attachments: attached(store, self.attach)?,
            ..DraftContent::default()
        })
    }
}

fn wrote(row: &DraftRow, accounts: &[AccountRow], action: &str, said: String) -> Done {
    let mut value = draft_json(row, accounts, false);
    value["action"] = json!(action);
    value["changed"] = json!(1);
    Done {
        value,
        lines: vec![format!("{} It is draft {}.", capital(&said), row.id.0)],
        count: 1,
        summary: format!("{said} (draft {})", row.id.0),
        // A draft made from outside is unmade by discarding it.
        undo: None,
        said: Some(said),
        undoable: false,
        queued: false,
        due: None,
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Who a message is to, in a line, for the log and the bottom bar: the
/// addresses, never what the message says.
fn recipients(content: &DraftContent) -> String {
    let all = [&content.to, &content.cc, &content.bcc]
        .into_iter()
        .filter(|field| !field.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    text::truncate(&text::sanitize_line(&all), 120)
}

fn saved(
    store: &mut Store,
    caller: &Caller,
    account: LocalAccountId,
    content: &DraftContent,
    now: i64,
) -> Result<DraftRow, Refusal> {
    let id = store
        .save_draft(None, account, content, now)?
        .ok_or_else(|| Refusal::new(Code::Failed, "The draft could not be saved."))?;
    store.mark_agent_mail(id, &caller.name)?;
    store
        .draft(id)?
        .ok_or_else(|| Refusal::new(Code::Failed, "The draft could not be saved."))
}

pub(crate) fn run(
    store: &mut Store,
    settings: &Settings,
    caller: &Caller,
    call: &Call,
    now: i64,
) -> Result<Done, Refusal> {
    let accounts = store.accounts()?;
    match call {
        Call::DraftNew {
            to,
            cc,
            bcc,
            subject,
            body,
            account,
            attach,
        } => {
            let account = sender(store, settings, account.as_deref())?;
            let content = Fields {
                to,
                cc,
                bcc,
                subject,
                body,
                attach,
            }
            .content(store)?;
            let row = saved(store, caller, account.id, &content, now)?;
            let said = format!("wrote a draft to {}.", recipients(&content));
            Ok(wrote(&row, &accounts, "draft_new", said))
        }
        Call::DraftReply { id, all, body } => {
            let original = super::look::message(store, settings, *id)?;
            let position = if settings.reply_above {
                ReplyPosition::Above
            } else {
                ReplyPosition::Below
            };
            let date_line = text::long_date(original.headers.date, &settings.tz);
            let events = outgoing::respond(
                store,
                LocalMessageId(*id),
                Response::Reply { all: *all },
                &date_line,
                position,
            )?;
            let Some(prefill) = events.into_iter().find_map(|event| match event {
                DataEvent::ComposeReady(prefill) => Some(prefill),
                _ => None,
            }) else {
                return Err(Refusal::not_found(format!(
                    "Message {id} has not been downloaded, so it cannot be answered yet. Open \
                     it in Quill, or run `quill sync`."
                )));
            };
            let mut content = prefill.content;
            // The answer goes where the cursor would be put for typing it.
            let at = prefill.cursor.min(content.body.len());
            let at = (0..=at)
                .rev()
                .find(|at| content.body.is_char_boundary(*at))
                .unwrap_or(0);
            let mut written = body.trim_end().to_owned();
            written.push('\n');
            content.body.insert_str(at, &written);
            let row = saved(store, caller, prefill.account, &content, now)?;
            let said = format!("wrote a reply to {}.", recipients(&content));
            Ok(wrote(&row, &accounts, "draft_reply", said))
        }
        Call::DraftEdit {
            id,
            to,
            cc,
            bcc,
            subject,
            body,
        } => {
            let row = own_draft(store, settings, *id, "changed")?;
            let mut content = row.content.clone();
            if let Some(to) = to {
                content.to = to.join(", ");
            }
            if let Some(cc) = cc {
                content.cc = cc.join(", ");
            }
            if let Some(bcc) = bcc {
                content.bcc = bcc.join(", ");
            }
            if let Some(subject) = subject {
                content.subject.clone_from(subject);
            }
            if let Some(body) = body {
                content.body.clone_from(body);
            }
            if store
                .save_draft(Some(row.id), row.account, &content, now)?
                .is_none()
            {
                return Err(Refusal::new(
                    Code::Failed,
                    format!("Draft {id} is on its way out and cannot be changed."),
                ));
            }
            let row = draft(store, settings, *id)?;
            Ok(wrote(
                &row,
                &accounts,
                "draft_edit",
                "changed a draft.".to_owned(),
            ))
        }
        Call::DraftDiscard { id } => {
            let row = own_draft(store, settings, *id, "discarded")?;
            if !store.delete_draft(row.id)? {
                return Err(Refusal::new(
                    Code::Failed,
                    format!("Draft {id} is being sent right now."),
                ));
            }
            let said = "discarded a draft it had written.".to_owned();
            Ok(Done {
                value: json!({ "action": "draft_discard", "draft": id, "changed": 1 }),
                lines: vec![format!("Discarded draft {id}.")],
                count: 1,
                summary: format!("discarded draft {id}"),
                undo: None,
                said: Some(said),
                undoable: false,
                queued: false,
                due: None,
            })
        }
        Call::Send {
            to,
            cc,
            bcc,
            subject,
            body,
            account,
            attach,
            at,
        } => {
            let account = sender(store, settings, account.as_deref())?;
            let content = Fields {
                to,
                cc,
                bcc,
                subject,
                body,
                attach,
            }
            .content(store)?;
            send(
                store,
                settings,
                caller,
                &account,
                content,
                None,
                at.as_deref(),
                now,
            )
        }
        Call::DraftSend { id, at } => {
            let row = draft(store, settings, *id)?;
            if !matches!(row.state, DraftState::Draft | DraftState::Failed) {
                return Err(Refusal::invalid(format!(
                    "Draft {id} is in the outbox already."
                )));
            }
            let account = accounts
                .iter()
                .find(|account| account.id == row.account)
                .cloned()
                .ok_or_else(|| Refusal::not_found("The draft's account is gone."))?;
            send(
                store,
                settings,
                caller,
                &account,
                row.content,
                Some(row.id),
                at.as_deref(),
                now,
            )
        }
        Call::OutboxCancel { id } => {
            let row = draft(store, settings, *id)?;
            if !store.unsend(row.id, now)? {
                return Err(Refusal::new(
                    Code::CannotUndo,
                    format!(
                        "Message {id} is not waiting in the outbox: it has gone, is going now, \
                         or is a draft."
                    ),
                ));
            }
            Ok(Done {
                value: json!({ "action": "outbox_cancel", "draft": id, "changed": 1, "state": "draft" }),
                lines: vec![format!("Taken back: it is draft {id} again.")],
                count: 1,
                summary: format!("took message {id} back out of the outbox"),
                undo: None,
                said: Some("took a message back out of the Outbox.".to_owned()),
                undoable: false,
                queued: false,
                due: None,
            })
        }
        _ => Err(Refusal::invalid("That is not a way to write mail.")),
    }
}

/// A draft that was written from outside the window, which may be
/// changed or discarded from outside it. The user's own is theirs.
fn own_draft(store: &Store, settings: &Settings, id: i64, done: &str) -> Result<DraftRow, Refusal> {
    let row = draft(store, settings, id)?;
    if row.agent.is_none() {
        return Err(Refusal::new(
            Code::Failed,
            format!(
                "Draft {id} was written in Quill, by its user, and is not {done} from outside \
                 it. That is done in Quill."
            ),
        ));
    }
    if row.state != DraftState::Draft {
        return Err(Refusal::invalid(format!(
            "Message {id} is in the outbox. Take it back first (outbox_cancel)."
        )));
    }
    Ok(row)
}

/// Checks a message, builds it and puts it in the outbox: to go at once,
/// at the time asked for, or after the wait that is kept between messages
/// sent this way; or to be held there for the user.
#[allow(clippy::too_many_arguments)]
fn send(
    store: &mut Store,
    settings: &Settings,
    caller: &Caller,
    account: &AccountRow,
    content: DraftContent,
    existing: Option<DraftId>,
    at: Option<&str>,
    now: i64,
) -> Result<Done, Refusal> {
    let mut due = now;
    if let Some(at) = at {
        due = crate::when::parse(at, now, &settings.tz).ok_or_else(|| {
            Refusal::invalid(format!(
                "\"{}\" is not a time to come. {}",
                text::sanitize_line(at),
                crate::when::such_as()
            ))
        })?;
    }
    let held = settings.agent.hold_sends;
    // One message at a time, each no sooner than the wait after the one
    // before. A held message goes when the user says, so it keeps no
    // place in the line.
    let gap = i64::from(settings.agent.send_gap);
    if gap > 0
        && !held
        && let Some(last) = store.last_agent_send()?
    {
        due = due.max(last + gap);
    }
    if due > now && !held && store.agent_mail_waiting(now)? >= MOST_WAITING {
        return Err(Refusal::new(
            Code::OverLimit,
            format!(
                "{MOST_WAITING} messages are waiting their turn to go already. Nothing was \
                 added. Wait for them, or take some back (outbox, outbox_cancel)."
            ),
        ));
    }

    let draft = match existing {
        Some(draft) => draft,
        None => store
            .save_draft(None, account.id, &content, now)?
            .ok_or_else(|| Refusal::new(Code::Failed, "The message could not be saved."))?,
    };
    let offset = jiff::Timestamp::from_second(now)
        .map(|at| at.to_zoned(settings.tz.clone()).offset().seconds())
        .unwrap_or(0);
    let request = SendRequest {
        draft: Some(draft),
        account: account.id,
        content: content.clone(),
        from_name: settings
            .from_names
            .iter()
            .find(|(name, _)| name == &account.name)
            .map(|(_, full)| full.clone()),
        format: if settings.markdown {
            BodyFormat::Markdown
        } else {
            BodyFormat::Plain
        },
        delay_seconds: due - now,
        utc_offset: offset,
        // Nobody is here to be asked whether a message with no subject is
        // meant. The checks that refuse a message still do.
        confirmed: true,
    };
    let mut refused = None;
    for event in outgoing::send(store, &request)? {
        match event {
            DataEvent::SendRefused(why) => refused = Some(why),
            DataEvent::SendDoubts(doubts) => refused = Some(doubts.join(" ")),
            _ => {}
        }
    }
    if let Some(why) = refused {
        // Nothing is left behind of a message that was never one.
        if existing.is_none() {
            store.delete_draft(draft)?;
        }
        return Err(Refusal::invalid(why));
    }
    store.mark_agent_mail(draft, &caller.name)?;
    // To the second: the wait between messages is counted from it.
    store.set_send_time(draft, due)?;
    if held {
        store.hold_send(draft)?;
    }

    let accounts = store.accounts()?;
    let row = store
        .draft(draft)?
        .ok_or_else(|| Refusal::new(Code::Failed, "The message could not be queued."))?;
    let to = recipients(&content);
    let files: Vec<String> = content
        .attachments
        .iter()
        .map(|file| text::sanitize_line(&file.name))
        .collect();
    let with = if files.is_empty() {
        String::new()
    } else {
        format!(", with {}", files.join(", "))
    };
    let (said, line) = if held {
        (
            format!("wrote a message to {to}{with}. It is waiting in the Outbox for your OK."),
            format!(
                "In the outbox of {}, waiting for the user's OK. It does not go until they \
                 send it from Quill. Tell them.",
                text::sanitize_line(&account.name)
            ),
        )
    } else if due > now {
        let wait = text::long_date(due, &settings.tz);
        (
            format!("sent a message to {to}{with}. It goes {wait}."),
            format!(
                "In the outbox of {}. It goes {wait}, if Quill is running then, or when it \
                 next is.",
                text::sanitize_line(&account.name)
            ),
        )
    } else {
        (
            format!("sent a message to {to}{with}."),
            format!(
                "In the outbox of {}. It goes when Quill next syncs: leave the app running, \
                 or run `quill sync`.",
                text::sanitize_line(&account.name)
            ),
        )
    };
    let mut value = draft_json(&row, &accounts, false);
    value["action"] = json!("send");
    value["changed"] = json!(1);
    value["held"] = json!(held);
    value["queued"] = json!(true);
    Ok(Done {
        value,
        lines: vec![line],
        count: 1,
        summary: said.clone(),
        // Taken back while it has not gone.
        undo: Some(json!({ "op": "send", "draft": draft.0 })),
        said: Some(said),
        undoable: false,
        queued: true,
        // A held message keeps no place in the line.
        due: (!held).then_some(due),
    })
}

/// Takes back every message sent from outside the window that has not
/// gone, because the user stopped the agents. Returns what the UI is to
/// hear of it.
pub fn stop(store: &mut Store, now: i64) -> quill_store::Result<Vec<DataEvent>> {
    let taken = store.unsend_agent_mail(now)?;
    if taken == 0 {
        return Ok(Vec::new());
    }
    Ok(vec![
        DataEvent::StoreChanged,
        DataEvent::Flash {
            text: format!(
                "{} that agents sent had not gone, and {} back in Drafts.",
                super::messages(taken),
                if taken == 1 { "is" } else { "are" }
            ),
            undoable: false,
        },
    ])
}

/// Takes a message that was sent back out of the outbox, to undo sending
/// it. `false` when it has gone.
pub(crate) fn unsend(store: &mut Store, draft: i64, now: i64) -> Result<bool, Refusal> {
    Ok(store.unsend(DraftId(draft), now)?)
}
