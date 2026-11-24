//! Looking at the mail: what there is, and what it says. Nothing here
//! changes a message.

use std::path::Path;

use quill_core::types::{FolderRole, LocalMessageId};
use quill_store::{DraftState, Mailbox, MessageRow, Store};
use serde_json::{Value, json};

use super::select::{self, visible};
use super::show::{Names, row_json, row_line, when};
use super::{Call, Code, Done, Refusal, Settings, Then, messages};
use crate::files;
use crate::text;

/// As many messages as a list gives when it is not told how many.
const USUAL_LIMIT: usize = 50;
/// The most a list gives, however many are asked for.
const MOST: usize = 500;
/// As much of a message's text as `read` gives when it is not told how
/// much.
const USUAL_CHARS: usize = 20_000;

fn limit(asked: Option<usize>) -> usize {
    asked.unwrap_or(USUAL_LIMIT).min(MOST)
}

pub(crate) fn run(
    store: &mut Store,
    settings: &Settings,
    call: &Call,
    _now: i64,
) -> Result<Done, Refusal> {
    match call {
        Call::Accounts => accounts(store, settings),
        Call::Folders { account } => folders(store, settings, account.as_deref()),
        Call::List {
            within,
            unread,
            limit: asked,
        } => list(store, settings, within.as_deref(), *unread, limit(*asked)),
        Call::Search {
            query,
            limit: asked,
        } => search(store, settings, query, limit(*asked)),
        Call::Count { query } => count(store, settings, query),
        Call::Read {
            id,
            headers,
            max_chars,
            from_char,
        } => read(store, settings, *id, *headers, *max_chars, *from_char),
        Call::Thread { id } => thread(store, settings, *id),
        Call::Attachments { id } => attachments(store, settings, *id),
        Call::AttachmentSave { id, part, to } => {
            attachment_save(store, settings, *id, *part, to.as_deref())
        }
        Call::Contacts {
            words,
            limit: asked,
        } => contacts(store, words, limit(*asked)),
        Call::Status => status(store, settings),
        Call::Drafts => super::write::list(store, settings, false),
        Call::Outbox => super::write::list(store, settings, true),
        Call::DraftShow { id } => super::write::show(store, settings, *id),
        Call::Log { limit: asked, all } => log(store, limit(*asked), *all),
        Call::Watch { query } => watch(settings, query),
        _ => Err(Refusal::invalid("That is not something to look at.")),
    }
}

fn accounts(store: &Store, settings: &Settings) -> Result<Done, Refusal> {
    let rows: Vec<_> = store
        .accounts()?
        .into_iter()
        .filter(|account| settings.agent.sees(&account.name))
        .collect();
    let value = json!({
        "accounts": rows.iter().map(|account| json!({
            "name": account.name,
            "address": account.address,
            "kind": account.kind,
        })).collect::<Vec<_>>(),
    });
    let lines = rows
        .iter()
        .map(|account| {
            format!(
                "{}  {}  {}",
                text::sanitize_line(&account.name),
                text::sanitize_line(&account.address),
                text::sanitize_line(&account.kind)
            )
        })
        .collect();
    let count = rows.len();
    Ok(Done::look(value, lines, 0, format!("{count} accounts")))
}

fn folders(store: &Store, settings: &Settings, account: Option<&str>) -> Result<Done, Refusal> {
    let seen = visible(store, settings)?;
    let names = Names::of(store)?;
    if let Some(wanted) = account
        && !names
            .accounts
            .iter()
            .any(|(id, name)| seen.contains(id) && name.eq_ignore_ascii_case(wanted))
    {
        return Err(Refusal::not_found(format!(
            "There is no account named \"{}\".",
            text::sanitize_line(wanted)
        )));
    }
    let rows: Vec<_> = names
        .folders
        .iter()
        .filter(|folder| seen.contains(&folder.account))
        .filter(|folder| {
            account.is_none_or(|wanted| names.account(folder.account).eq_ignore_ascii_case(wanted))
        })
        .collect();
    let value = json!({
        "folders": rows.iter().map(|folder| json!({
            "account": names.account(folder.account),
            "name": folder.name,
            "role": (folder.role != FolderRole::Other).then(|| folder.role.as_str()),
            "messages": folder.total,
            "unread": folder.unread,
            "on_this_mac": folder.local,
        })).collect::<Vec<_>>(),
    });
    let lines = rows
        .iter()
        .map(|folder| {
            format!(
                "{:>6} {:>6}  {}",
                folder.unread,
                folder.total,
                text::sanitize_line(&names.place(folder.id)),
            )
        })
        .collect();
    let count = rows.len();
    Ok(Done::look(value, lines, 0, format!("{count} folders")))
}

/// Messages as an answer: each as data, and a line for each.
fn listed(store: &Store, rows: &[MessageRow], more: bool) -> Result<(Value, Vec<String>), Refusal> {
    let names = Names::of(store)?;
    let value = json!({
        "messages": rows.iter().map(|row| row_json(row, &names)).collect::<Vec<_>>(),
        "more": more,
    });
    Ok((value, rows.iter().map(row_line).collect()))
}

fn list(
    store: &Store,
    settings: &Settings,
    within: Option<&str>,
    unread: bool,
    limit: usize,
) -> Result<Done, Refusal> {
    let seen = visible(store, settings)?;
    let mut rows = match within {
        None => store.messages(Mailbox::Unified)?,
        Some(place) => {
            let query = select::query(&format!("in:{place}"), &settings.tz);
            if query.within.is_none() {
                return Err(Refusal::invalid(format!(
                    "\"{}\" does not name a folder. Try an account and a folder, as in work/sent.",
                    text::sanitize_line(place)
                )));
            }
            store.search(&query, usize::MAX)?
        }
    };
    rows.retain(|row| seen.contains(&row.account) && (!unread || !row.flags.seen));
    let more = rows.len() > limit;
    rows.truncate(limit);
    let (value, lines) = listed(store, &rows, more)?;
    let summary = match within {
        Some(place) => format!(
            "in {}: {}",
            text::sanitize_line(place),
            messages(rows.len())
        ),
        None => format!("the inboxes: {}", messages(rows.len())),
    };
    Ok(Done::look(value, lines, rows.len(), summary))
}

fn search(store: &Store, settings: &Settings, words: &str, limit: usize) -> Result<Done, Refusal> {
    let (rows, more) = select::search(store, settings, words, limit)?;
    let (value, lines) = listed(store, &rows, more)?;
    let summary = format!(
        "{}: {}{}",
        text::sanitize_line(words.trim()),
        messages(rows.len()),
        if more { ", and more" } else { "" }
    );
    Ok(Done::look(value, lines, rows.len(), summary))
}

fn count(store: &Store, settings: &Settings, words: &str) -> Result<Done, Refusal> {
    let (rows, _) = select::search(store, settings, words, usize::MAX - 1)?;
    let count = rows.len();
    Ok(Done::look(
        json!({ "count": count }),
        vec![count.to_string()],
        count,
        format!("{}: {}", text::sanitize_line(words.trim()), messages(count)),
    ))
}

/// What needs the Quill that is running says so when none is.
pub(crate) fn needs_quill(what: &str) -> Refusal {
    Refusal::new(
        Code::NotRunning,
        format!("{what} Quill is not running. Open Quill and try again."),
    )
}

/// Asks for a message to be shown in Quill's window. The window marks it
/// read, as it does any message opened in it.
pub(crate) fn open(store: &Store, settings: &Settings, id: i64) -> Result<Done, Refusal> {
    let row = message(store, settings, id)?;
    if !settings.running {
        return Err(needs_quill("A message is shown in Quill's window."));
    }
    let location = store
        .message_location(row.id)?
        .ok_or_else(|| Refusal::not_found(format!("There is no message {id}.")))?;
    Ok(Done {
        value: json!({ "action": "open", "id": id, "changed": 1 }),
        lines: vec![format!("Message {id} is on show in Quill.")],
        count: 1,
        summary: format!("showed message {id} in the window"),
        undo: None,
        // The window shows it: there is nothing more to say there.
        said: None,
        undoable: false,
        queued: false,
        due: None,
        then: Some(Then::Show {
            message: row.id,
            folder: location.folder,
        }),
    })
}

/// Begins saying what arrives. What arrives is found by [`arrived`], for
/// as long as whoever asked stays.
fn watch(settings: &Settings, query: &str) -> Result<Done, Refusal> {
    if !settings.running {
        return Err(needs_quill("Mail arrives in the Quill that is running."));
    }
    let asked = query.trim();
    Ok(Done {
        then: Some(Then::Watch {
            query: asked.to_owned(),
        }),
        ..Done::look(
            json!({ "watching": asked }),
            Vec::new(),
            0,
            if asked.is_empty() {
                "watch: all that arrives".to_owned()
            } else {
                format!("watch: {}", text::sanitize_line(asked))
            },
        )
    })
}

/// Of the messages that have just arrived, the ones a watcher's search
/// finds among the mail agents may see, as a list or a search would give
/// them: with nothing to find, all of them.
pub fn arrived(
    store: &Store,
    settings: &Settings,
    query: &str,
    ids: &[LocalMessageId],
) -> Vec<Value> {
    let read = select::query(query, &settings.tz);
    let rows = if read.is_empty() {
        store.message_rows(ids)
    } else {
        store.search_among(&read, ids)
    };
    let (Ok(rows), Ok(seen), Ok(names)) = (rows, visible(store, settings), Names::of(store)) else {
        return Vec::new();
    };
    rows.iter()
        .filter(|row| seen.contains(&row.account))
        .map(|row| {
            let mut value = row_json(row, &names);
            value["schema"] = json!(super::SCHEMA);
            value
        })
        .collect()
}

/// The message with this id, if agents may see it.
pub(crate) fn message(store: &Store, settings: &Settings, id: i64) -> Result<MessageRow, Refusal> {
    let seen = visible(store, settings)?;
    store
        .message_rows(&[LocalMessageId(id)])?
        .into_iter()
        .find(|row| seen.contains(&row.account))
        .ok_or_else(|| Refusal::not_found(format!("There is no message {id}.")))
}

/// The header lines of a message as it arrived: everything before the
/// first empty line.
fn header_block(raw: &[u8]) -> String {
    let end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|window| window == b"\n\n"))
        .unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).replace("\r\n", "\n")
}

fn read(
    store: &Store,
    settings: &Settings,
    id: i64,
    headers: bool,
    max_chars: Option<usize>,
    from_char: usize,
) -> Result<Done, Refusal> {
    let row = message(store, settings, id)?;
    let names = Names::of(store)?;
    let body = store.body(row.id)?;
    let mut value = row_json(&row, &names);
    let mut lines = Vec::new();

    let join = |list: &[quill_core::types::Address]| {
        list.iter()
            .map(|person| text::sanitize_line(&person.to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if headers {
        match store.raw(row.id)? {
            Some(raw) => {
                let block = header_block(&raw);
                lines.extend(block.lines().map(text::sanitize_line));
                value["headers"] = json!(block);
            }
            None => value["headers"] = Value::Null,
        }
    }
    if lines.is_empty() {
        lines.push(format!("From:    {}", join(&row.headers.from)));
        lines.push(format!("To:      {}", join(&row.headers.to)));
        if !row.headers.cc.is_empty() {
            lines.push(format!("Cc:      {}", join(&row.headers.cc)));
        }
        lines.push(format!("Date:    {}", when(row.headers.date)));
        lines.push(format!(
            "Subject: {}",
            text::sanitize_line(&row.headers.subject)
        ));
    }
    lines.push(String::new());

    value["downloaded"] = json!(body.is_some());
    match &body {
        Some(body) => {
            // Counted in characters, so that a piece never ends inside
            // one.
            let total = body.chars().count();
            let most = match max_chars {
                Some(0) => usize::MAX,
                Some(most) => most,
                None => USUAL_CHARS,
            };
            let piece: String = body.chars().skip(from_char).take(most).collect();
            let given = piece.chars().count();
            let end = from_char.saturating_add(given).min(total);
            let left = total.saturating_sub(end);
            lines.extend(
                text::sanitize_body(piece.trim_end())
                    .lines()
                    .map(str::to_owned),
            );
            if left > 0 {
                lines.push(String::new());
                lines.push(format!(
                    "({left} more characters. Read on with --from-char {end}.)"
                ));
            }
            value["body"] = json!(piece);
            value["body_chars"] = json!(total);
            value["from_char"] = json!(from_char.min(total));
            value["remaining_chars"] = json!(left);
            value["next_from_char"] = if left > 0 { json!(end) } else { Value::Null };
        }
        None => {
            lines.push("(Not downloaded yet. Open it in Quill, or run `quill sync`.)".to_owned());
            value["body"] = Value::Null;
        }
    }
    Ok(Done::look(value, lines, 1, format!("message {id}")))
}

fn thread(store: &Store, settings: &Settings, id: i64) -> Result<Done, Refusal> {
    let row = message(store, settings, id)?;
    let seen = visible(store, settings)?;
    let query = quill_core::search::Query {
        thread_of: Some(row.id.0),
        ..Default::default()
    };
    let mut rows = store.search(&query, MOST)?;
    rows.retain(|row| seen.contains(&row.account));
    // Oldest first: the order it was written in.
    rows.reverse();
    let (value, lines) = listed(store, &rows, false)?;
    let summary = format!("the conversation of message {id}: {}", messages(rows.len()));
    Ok(Done::look(value, lines, rows.len(), summary))
}

fn attachments(store: &Store, settings: &Settings, id: i64) -> Result<Done, Refusal> {
    let row = message(store, settings, id)?;
    let Some(raw) = store.raw(row.id)? else {
        return Err(Refusal::not_found(format!(
            "Message {id} has not been downloaded. Open it in Quill, or run `quill sync`."
        )));
    };
    let attached = quill_mime::parse(&raw, 0)
        .map(|parsed| files::attached(&parsed.attachments))
        .unwrap_or_default();
    let value = json!({
        "id": id,
        "attachments": attached.iter().map(|file| json!({
            "part": file.part,
            "name": file.name,
            "type": file.content_type,
            "size": file.size,
            "inline": file.inline,
        })).collect::<Vec<_>>(),
    });
    let lines = attached
        .iter()
        .map(|file| {
            format!(
                "{:>4}  {:>9}  {}  {}",
                file.part,
                text::file_size(file.size),
                text::sanitize_line(&file.name),
                text::sanitize_line(&file.content_type),
            )
        })
        .collect();
    let count = attached.len();
    Ok(Done::look(
        value,
        lines,
        0,
        format!("the files of message {id}: {count}"),
    ))
}

fn attachment_save(
    store: &Store,
    settings: &Settings,
    id: i64,
    part: u32,
    to: Option<&str>,
) -> Result<Done, Refusal> {
    let row = message(store, settings, id)?;
    let (folder, called) = match (to, &settings.files) {
        (Some(to), _) => (Path::new(to).to_path_buf(), "That folder"),
        // A message's files go in a folder of its own, so that two of
        // one name never meet.
        (None, Some(files)) => (files.join(format!("m{id}")), "Quill's folder for files"),
        (None, None) => {
            return Err(Refusal::invalid(
                "There is nowhere to write the file. Say where, with --to.",
            ));
        }
    };
    let (name, path) = files::save_into(store, &folder, called, row.id, part)
        .map_err(|problem| Refusal::not_found(problem.0))?;
    Ok(Done::look(
        json!({ "id": id, "part": part, "name": name, "path": path }),
        vec![path.display().to_string()],
        1,
        format!("file {part} of message {id} written out"),
    ))
}

fn contacts(store: &Store, words: &str, limit: usize) -> Result<Done, Refusal> {
    let found = store.contacts_matching(words.trim(), limit)?;
    let value = json!({
        "contacts": found.iter().map(|person| json!({
            "name": person.name,
            "address": person.email,
        })).collect::<Vec<_>>(),
    });
    let lines = found
        .iter()
        .map(|person| {
            if person.name.is_empty() {
                text::sanitize_line(&person.email)
            } else {
                format!(
                    "{} <{}>",
                    text::sanitize_line(&person.name),
                    text::sanitize_line(&person.email)
                )
            }
        })
        .collect();
    let count = found.len();
    Ok(Done::look(
        value,
        lines,
        0,
        format!(
            "people matching {}: {count}",
            text::sanitize_line(words.trim())
        ),
    ))
}

fn status(store: &Store, settings: &Settings) -> Result<Done, Refusal> {
    let seen = visible(store, settings)?;
    let names = Names::of(store)?;
    let mut accounts = Vec::new();
    let mut lines = Vec::new();
    for (id, name) in names.accounts.iter().filter(|(id, _)| seen.contains(id)) {
        let unread: u32 = names
            .folders
            .iter()
            .filter(|folder| folder.account == *id && folder.role == FolderRole::Inbox)
            .map(|folder| folder.unread)
            .sum();
        accounts.push(json!({ "name": name, "unread": unread }));
        lines.push(format!(
            "{}: {unread} unread in the inbox",
            text::sanitize_line(name)
        ));
    }
    let drafts = store
        .drafts()?
        .iter()
        .filter(|draft| seen.contains(&draft.account))
        .count();
    let outbox: Vec<_> = store
        .outbox()?
        .into_iter()
        .filter(|draft| seen.contains(&draft.account))
        .collect();
    let refused = outbox
        .iter()
        .filter(|draft| draft.state == DraftState::Failed)
        .count();
    let held = outbox
        .iter()
        .filter(|draft| draft.state == DraftState::Held)
        .count();
    let waiting = outbox.len() - refused - held;
    let (snoozed, _) = store.snoozed()?;
    let queued = store.queued_operations()?;
    lines.push(format!(
        "{drafts} drafts, {waiting} waiting to be sent, {held} held for an OK, {refused} not \
         sent, {snoozed} snoozed"
    ));
    lines.push(format!("{queued} changes waiting to reach a server"));
    Ok(Done::look(
        json!({
            "accounts": accounts,
            "drafts": drafts,
            "outbox": { "waiting": waiting, "refused": refused, "held": held },
            "snoozed": snoozed,
            "queued_changes": queued,
        }),
        lines,
        0,
        "how things stand".to_owned(),
    ))
}

fn log(store: &Store, limit: usize, all: bool) -> Result<Done, Refusal> {
    let entries = store.agent_log(limit, all)?;
    let value = json!({
        "entries": entries.iter().map(|entry| json!({
            "entry": entry.id,
            "at": when(entry.at),
            "way": entry.way,
            "agent": entry.agent,
            "action": entry.action,
            "change": entry.change,
            "summary": entry.summary,
            "messages": entry.count,
            "can_undo": entry.change && entry.undo.is_some() && entry.undone.is_none(),
            "undone": entry.undone.map(when),
        })).collect::<Vec<_>>(),
    });
    let lines = entries
        .iter()
        .map(|entry| {
            let at = when(entry.at);
            format!(
                "{:>6}  {}  {}  {}  {}{}",
                entry.id,
                at.get(..16).unwrap_or(&at).replace('T', " "),
                text::fit(&text::sanitize_line(&entry.agent), 16),
                text::fit(&entry.action, 10),
                text::sanitize_line(&entry.summary),
                if entry.undone.is_some() {
                    "  (taken back)"
                } else {
                    ""
                },
            )
        })
        .collect();
    Ok(Done::look(value, lines, 0, String::new()))
}
