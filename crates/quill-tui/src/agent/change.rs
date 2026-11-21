//! Changing the mail: marks, moves, snoozing, putting back, deleting to
//! the trash, and taking a change back.
//!
//! Every change goes through the store's queue, as the UI's own do, so it
//! reaches the server the same way and survives a restart. Each records
//! how things were before, so that it can be taken back long after the
//! few seconds `U` allows.

use quill_core::types::{Flags, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_store::{FolderRow, MessageRow, OperationId, Store};
use serde_json::{Value, json};

use super::select::{self, Chosen, asked, visible};
use super::show::{Names, described};
use super::{Call, Code, Done, Mark, Refusal, Settings, messages};
use crate::data::{Deletion, UNDO_SECONDS, deletion, folder_with_role};
use crate::text;
use crate::when;

/// A message a change passed over, and why.
struct Skipped {
    id: i64,
    why: String,
}

/// What is known of a change while it is made.
struct Work {
    /// The messages changed.
    changed: Vec<LocalMessageId>,
    skipped: Vec<Skipped>,
    /// How each changed message was before, for taking the change back.
    before: Vec<Value>,
    /// What was queued for a server.
    operations: Vec<OperationId>,
    /// Something was queued that is not among `operations` (a mark, a
    /// copy).
    queued: bool,
}

impl Work {
    fn new() -> Self {
        Self {
            changed: Vec::new(),
            skipped: Vec::new(),
            before: Vec::new(),
            operations: Vec::new(),
            queued: false,
        }
    }

    fn skip(&mut self, row: &MessageRow, why: impl Into<String>) {
        self.skipped.push(Skipped {
            id: row.id.0,
            why: why.into(),
        });
    }
}

/// Four digits, as the store writes flags: seen, flagged, answered, draft.
fn flags_text(flags: Flags) -> String {
    [flags.seen, flags.flagged, flags.answered, flags.draft]
        .iter()
        .map(|on| if *on { '1' } else { '0' })
        .collect()
}

fn flags_from(text: &str) -> Option<Flags> {
    let bits: Vec<bool> = text.chars().map(|c| c == '1').collect();
    (bits.len() == 4 && text.chars().all(|c| c == '0' || c == '1')).then(|| Flags {
        seen: bits[0],
        flagged: bits[1],
        answered: bits[2],
        draft: bits[3],
    })
}

/// The folder of `account` that `to` names: by its name, or by what it is
/// for. `to` may say the account as well (`work/receipts`), and then names
/// no folder of any other.
fn folder_named<'a>(names: &'a Names, account: LocalAccountId, to: &str) -> Option<&'a FolderRow> {
    let to = to.trim();
    let (wanted_account, wanted) = match to.split_once('/') {
        Some((first, rest))
            if names
                .accounts
                .iter()
                .any(|(_, name)| name.eq_ignore_ascii_case(first.trim())) =>
        {
            (Some(first.trim()), rest.trim())
        }
        _ => (None, to),
    };
    if wanted_account.is_some_and(|name| !names.account(account).eq_ignore_ascii_case(name)) {
        return None;
    }
    let of_account = || {
        names
            .folders
            .iter()
            .filter(|folder| folder.account == account)
    };
    // Its name first: a folder called "Archive" is that folder, whatever
    // else is the account's archive.
    of_account()
        .find(|folder| folder.name.eq_ignore_ascii_case(wanted))
        // Or the whole name one of Google's own folders has on its
        // servers, which is how it was shown once.
        .or_else(|| {
            of_account().find(|folder| quill_core::providers::names_folder(wanted, &folder.name))
        })
        .or_else(|| {
            of_account().find(|folder| {
                folder.role != FolderRole::Other
                    && folder.role.as_str().eq_ignore_ascii_case(wanted)
            })
        })
}

pub(crate) fn run(
    store: &mut Store,
    settings: &Settings,
    call: &Call,
    now: i64,
) -> Result<Done, Refusal> {
    if let Call::FolderCreate { name, account } = call {
        return folder_create(store, settings, name, account.as_deref());
    }
    let Some(which) = call.which() else {
        return Err(Refusal::invalid("That is not a change to mail."));
    };
    // What cannot be read is refused before any message is looked for.
    let until = match call {
        Call::Snooze { until, .. } => {
            Some(when::parse(until, now, &settings.tz).ok_or_else(|| {
                Refusal::invalid(format!(
                    "\"{}\" is not a time to come. {}",
                    text::sanitize_line(until),
                    when::such_as()
                ))
            })?)
        }
        _ => None,
    };
    let Chosen { rows, missing } = select::choose(store, settings, which)?;
    let names = Names::of(store)?;
    let mut work = Work::new();
    let dry = which.dry_run;

    for row in &rows {
        match call {
            Call::Mark { mark, .. } => mark_one(store, row, *mark, dry, now, &mut work)?,
            Call::Move { to, .. } => match folder_named(&names, row.account, to) {
                Some(folder) => move_one(store, row, folder.id, dry, now, &mut work, None)?,
                None => work.skip(
                    row,
                    format!(
                        "its account, {}, has no folder \"{}\"",
                        names.account(row.account),
                        text::sanitize_line(to.trim())
                    ),
                ),
            },
            Call::Copy { to, .. } => match folder_named(&names, row.account, to) {
                Some(folder) if folder.id == row.folder => work.skip(row, "it is in that folder"),
                Some(_) if dry => work.changed.push(row.id),
                Some(folder) => match store.queue_copy(row.id, folder.id, now)? {
                    Some(_) => {
                        work.changed.push(row.id);
                        work.queued = true;
                    }
                    None => work.skip(row, "it cannot be copied there"),
                },
                None => work.skip(
                    row,
                    format!(
                        "its account, {}, has no folder \"{}\"",
                        names.account(row.account),
                        text::sanitize_line(to.trim())
                    ),
                ),
            },
            Call::Archive { .. } => match folder_with_role(store, row.id, FolderRole::Archive)? {
                Some(archive) => move_one(store, row, archive, dry, now, &mut work, None)?,
                None => work.skip(row, "its account has no archive folder"),
            },
            Call::Delete { .. } => match deletion(store, row.id)? {
                Deletion::ToTrash { from, trash } => {
                    move_one(store, row, trash, dry, now, &mut work, Some(from))?;
                }
                // Never for good from here: that is for the user, in
                // Quill.
                Deletion::ForGood => work.skip(
                    row,
                    "it is in the trash already, or its account has no trash folder: deleting \
                     it would be for good, which is done in Quill",
                ),
                Deletion::FromThisMac => work.skip(
                    row,
                    "it is in a folder that is only on this Mac: deleting it would be for good, \
                     which is done in Quill",
                ),
            },
            Call::Restore { .. } => restore_one(store, &names, row, dry, now, &mut work)?,
            Call::Snooze { .. } => {
                if let Some(until) = until {
                    if !dry {
                        store.snooze(&[row.id], until)?;
                    }
                    work.changed.push(row.id);
                    work.before
                        .push(json!({ "id": row.id.0, "until": row.snoozed_until }));
                }
            }
            Call::Unsnooze { .. } => {
                if row.snoozed_until.is_none() {
                    work.skip(row, "it is not snoozed");
                } else {
                    if !dry {
                        store.wake_snoozed(now, Some(&[row.id]))?;
                    }
                    work.changed.push(row.id);
                    work.before
                        .push(json!({ "id": row.id.0, "until": row.snoozed_until }));
                }
            }
            _ => {}
        }
    }
    // Moved together, taken back together by one `U` in Quill.
    store.group_operations(&work.operations)?;

    let count = work.changed.len();
    let (verb, to) = words(call, &names, &rows, &work, until, settings);
    let after = store.message_rows(&work.changed)?;
    let (messages_json, ids) = described(&after, &names);
    let mut value = json!({
        "action": call.name(),
        "dry_run": dry,
        "changed": if dry { 0 } else { count },
        "ids": ids,
        "messages": messages_json,
        "skipped": work.skipped.iter().map(|skipped| json!({
            "id": skipped.id,
            "why": skipped.why,
        })).collect::<Vec<_>>(),
        "missing": missing,
    });
    if dry {
        value["would_change"] = json!(count);
    }
    if let Some(until) = until {
        value["until"] = json!(super::show::when(until));
    }

    let mut lines = Vec::new();
    lines.push(if dry {
        format!(
            "Would have {verb} {}{to}. Nothing was changed.",
            messages(count)
        )
    } else {
        format!("{} {}{to}.", capital(&verb), messages(count))
    });
    for skipped in &work.skipped {
        lines.push(format!("{:>8}  left alone: {}", skipped.id, skipped.why));
    }
    for name in &missing {
        lines.push(format!("{}  no such message", text::sanitize_line(name)));
    }

    let undo = match call {
        _ if dry || count == 0 => None,
        // A copy is made by the server, later: there is nothing here to
        // take back.
        Call::Copy { .. } => None,
        Call::Mark { .. } => Some(json!({ "op": "mark", "items": work.before })),
        Call::Snooze { .. } | Call::Unsnooze { .. } => {
            Some(json!({ "op": "snooze", "items": work.before }))
        }
        _ => Some(json!({ "op": "move", "items": work.before })),
    };
    Ok(Done {
        value,
        lines,
        count: if dry { 0 } else { count },
        summary: format!(
            "{}{verb} {}{to} ({})",
            if dry { "dry run: would have " } else { "" },
            messages(count),
            asked(which)
        ),
        undo,
        said: (!dry).then(|| format!("{verb} {}{to}.", messages(count))),
        undoable: !work.operations.is_empty(),
        queued: work.queued || !work.operations.is_empty(),
        due: None,
    })
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// What a change is called once done ("moved", "marked read"), and where
/// to or until when (" to Work/Archive").
fn words(
    call: &Call,
    names: &Names,
    rows: &[MessageRow],
    work: &Work,
    until: Option<i64>,
    settings: &Settings,
) -> (String, String) {
    // Where the first changed message is now says where they went, when
    // they are all of one account; otherwise the name that was asked for
    // is all there is to say.
    let one_account = rows
        .iter()
        .filter(|row| work.changed.contains(&row.id))
        .map(|row| row.account)
        .collect::<std::collections::HashSet<_>>()
        .len()
        == 1;
    let place = |to: &str| -> String {
        let first = rows.iter().find(|row| work.changed.contains(&row.id));
        match first.and_then(|row| folder_named(names, row.account, to)) {
            Some(folder) if one_account => format!(" to {}", names.place(folder.id)),
            _ => format!(" to {}", text::sanitize_line(to.trim())),
        }
    };
    match call {
        Call::Mark { mark, .. } => ("marked".to_owned(), format!(" {}", mark.as_str())),
        Call::Move { to, .. } => {
            let here = rows
                .iter()
                .filter(|row| work.changed.contains(&row.id))
                .filter_map(|row| folder_named(names, row.account, to))
                .any(|folder| folder.local);
            let mut said = place(to);
            if here {
                said.push_str(": taken off the server, and now only on this Mac");
            }
            ("moved".to_owned(), said)
        }
        Call::Copy { to, .. } => ("copied".to_owned(), place(to)),
        Call::Archive { .. } => ("archived".to_owned(), String::new()),
        Call::Delete { .. } => ("moved".to_owned(), " to the trash".to_owned()),
        Call::Restore { .. } => ("put back".to_owned(), String::new()),
        Call::Snooze { .. } => (
            "snoozed".to_owned(),
            until.map_or_else(String::new, |until| {
                format!(" until {}", text::long_date(until, &settings.tz))
            }),
        ),
        Call::Unsnooze { .. } => ("brought back".to_owned(), String::new()),
        _ => ("changed".to_owned(), String::new()),
    }
}

fn mark_one(
    store: &mut Store,
    row: &MessageRow,
    mark: Mark,
    dry: bool,
    now: i64,
    work: &mut Work,
) -> Result<(), Refusal> {
    let wanted = match mark {
        Mark::Read => Flags {
            seen: true,
            ..row.flags
        },
        Mark::Unread => Flags {
            seen: false,
            ..row.flags
        },
        Mark::Starred => Flags {
            flagged: true,
            ..row.flags
        },
        Mark::Unstarred => Flags {
            flagged: false,
            ..row.flags
        },
    };
    if wanted == row.flags {
        work.skip(row, format!("it is {} already", mark.as_str()));
        return Ok(());
    }
    if dry || store.queue_flags(row.id, wanted, now)? {
        work.changed.push(row.id);
        work.before
            .push(json!({ "id": row.id.0, "flags": flags_text(row.flags) }));
        work.queued |= !dry;
    } else {
        work.skip(row, "it could not be marked");
    }
    Ok(())
}

/// Moves one message, noting where it was. `trashed_from`: it is being
/// deleted, and this is where it can be put back.
fn move_one(
    store: &mut Store,
    row: &MessageRow,
    folder: LocalFolderId,
    dry: bool,
    now: i64,
    work: &mut Work,
    trashed_from: Option<LocalFolderId>,
) -> Result<(), Refusal> {
    if folder == row.folder {
        work.skip(row, "it is in that folder already");
        return Ok(());
    }
    if dry {
        work.changed.push(row.id);
        return Ok(());
    }
    if let Some(from) = trashed_from {
        store.note_trashed(row.id, from, now)?;
    }
    match store.queue_move(row.id, folder, now, now + UNDO_SECONDS)? {
        Some(operation) => {
            work.operations.push(operation);
            work.changed.push(row.id);
            work.before.push(json!({
                "id": row.id.0,
                "from": row.folder.0,
                "trashed": trashed_from.is_some(),
            }));
        }
        None => {
            if trashed_from.is_some() {
                store.forget_trashed(row.id)?;
            }
            work.skip(row, "it cannot be moved there");
        }
    }
    Ok(())
}

fn restore_one(
    store: &mut Store,
    names: &Names,
    row: &MessageRow,
    dry: bool,
    now: i64,
    work: &mut Work,
) -> Result<(), Refusal> {
    let in_trash = names
        .folder(row.folder)
        .is_some_and(|folder| folder.role == FolderRole::Trash);
    if !in_trash {
        work.skip(row, "it is not in a trash folder");
        return Ok(());
    }
    let known = store.trashed_from(row.id)?;
    let target = match known {
        Some(folder) => Some(folder),
        None => folder_with_role(store, row.id, FolderRole::Inbox)?,
    };
    let Some(target) = target else {
        work.skip(row, "its account has no inbox to put it back in");
        return Ok(());
    };
    if dry {
        work.changed.push(row.id);
        return Ok(());
    }
    match store.queue_move(row.id, target, now, now + UNDO_SECONDS)? {
        Some(operation) => {
            store.forget_trashed(row.id)?;
            work.operations.push(operation);
            work.changed.push(row.id);
            // Taken back, it is in the trash again, and where it came
            // from is known again.
            work.before.push(json!({
                "id": row.id.0,
                "from": row.folder.0,
                "note": target.0,
            }));
        }
        None => work.skip(row, "it cannot be put back"),
    }
    Ok(())
}

fn folder_create(
    store: &mut Store,
    settings: &Settings,
    name: &str,
    account: Option<&str>,
) -> Result<Done, Refusal> {
    let seen = visible(store, settings)?;
    let accounts: Vec<_> = store
        .accounts()?
        .into_iter()
        .filter(|row| seen.contains(&row.id))
        .collect();
    let account = match account {
        Some(wanted) => accounts
            .iter()
            .find(|row| row.name.eq_ignore_ascii_case(wanted.trim()))
            .ok_or_else(|| {
                Refusal::not_found(format!(
                    "There is no account named \"{}\".",
                    text::sanitize_line(wanted)
                ))
            })?,
        None => match accounts.as_slice() {
            [only] => only,
            [] => {
                return Err(Refusal::not_found(
                    "There is no account to make a folder for.",
                ));
            }
            _ => {
                return Err(Refusal::invalid(
                    "Say which account the folder is for: there is more than one.",
                ));
            }
        },
    };
    let folder = store
        .create_local_folder(account.id, name)?
        .map_err(Refusal::invalid)?;
    let called = text::sanitize_line(name.trim());
    let place = format!(
        "{}/{called} (on this Mac)",
        text::sanitize_line(&account.name)
    );
    Ok(Done {
        value: json!({
            "action": "folder_create",
            "changed": 1,
            "account": account.name,
            "name": name.trim(),
            "on_this_mac": true,
        }),
        lines: vec![format!("Made {place}.")],
        count: 1,
        summary: format!("made the folder {place}"),
        undo: Some(json!({ "op": "folder", "folder": folder.0 })),
        said: Some(format!("made the folder {place}.")),
        undoable: false,
        queued: false,
        due: None,
    })
}

/// Takes back a change in the log: the one numbered `entry`, or the newest
/// that can be.
/// Puts messages back in the folders a change moved them out of. Those
/// put back go in `back`, with the moves made in `operations`; those that
/// are no longer where the change left them go in `gone`.
fn undo_moves(
    store: &mut Store,
    items: &[Value],
    now: i64,
    back: &mut Vec<LocalMessageId>,
    gone: &mut Vec<i64>,
    operations: &mut Vec<quill_store::OperationId>,
) -> Result<(), Refusal> {
    for item in items {
        let (Some(id), Some(from)) = (item["id"].as_i64(), item["from"].as_i64()) else {
            continue;
        };
        let message = LocalMessageId(id);
        match store.queue_move(message, LocalFolderId(from), now, now + UNDO_SECONDS)? {
            Some(operation) => {
                if item["trashed"].as_bool() == Some(true) {
                    store.forget_trashed(message)?;
                }
                // A message put back out of the trash is in the
                // trash again, with where it came from known.
                if let Some(note) = item["note"].as_i64() {
                    store.note_trashed(message, LocalFolderId(note), now)?;
                }
                operations.push(operation);
                back.push(message);
            }
            None => gone.push(id),
        }
    }
    Ok(())
}

/// Gives messages back the marks a change took or made. Returns whether
/// anything was queued for a server.
fn undo_marks(
    store: &mut Store,
    items: &[Value],
    now: i64,
    back: &mut Vec<LocalMessageId>,
    gone: &mut Vec<i64>,
) -> Result<bool, Refusal> {
    let mut queued = false;
    for item in items {
        let (Some(id), Some(flags)) = (
            item["id"].as_i64(),
            item["flags"].as_str().and_then(flags_from),
        ) else {
            continue;
        };
        let message = LocalMessageId(id);
        // Only the two marks that are given here are put back:
        // whether it has been answered since is not undone.
        match store.message_location(message)? {
            Some(location) => {
                let wanted = Flags {
                    seen: flags.seen,
                    flagged: flags.flagged,
                    ..location.flags
                };
                if wanted != location.flags && store.queue_flags(message, wanted, now)? {
                    queued = true;
                }
                back.push(message);
            }
            None => gone.push(id),
        }
    }
    Ok(queued)
}

pub(crate) fn undo(
    store: &mut Store,
    _settings: &Settings,
    entry: Option<i64>,
    now: i64,
) -> Result<Done, Refusal> {
    undo_entry(store, entry, now)
}

/// Takes back a change in the log: the one numbered `entry`, or the
/// newest that can be. Whoever made it: an agent, a command, or a rule.
pub(crate) fn undo_entry(store: &mut Store, entry: Option<i64>, now: i64) -> Result<Done, Refusal> {
    let found = match entry {
        Some(id) => store.agent_entry(id)?.ok_or_else(|| {
            Refusal::not_found(format!("There is no change numbered {id} in the log."))
        })?,
        None => store
            .last_agent_change()?
            .ok_or_else(|| Refusal::new(Code::CannotUndo, "There is nothing to take back."))?,
    };
    if found.undone.is_some() {
        return Err(Refusal::new(
            Code::CannotUndo,
            format!("Change {} was taken back already.", found.id),
        ));
    }
    let data: Option<Value> = found
        .undo
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok());
    let Some(data) = data.filter(|_| found.change) else {
        return Err(Refusal::new(
            Code::CannotUndo,
            format!(
                "Change {} ({}) cannot be taken back.",
                found.id, found.action
            ),
        ));
    };
    let items: Vec<Value> = data["items"].as_array().cloned().unwrap_or_default();
    let mut back: Vec<LocalMessageId> = Vec::new();
    let mut gone: Vec<i64> = Vec::new();
    let mut operations = Vec::new();
    let mut queued = false;
    match data["op"].as_str() {
        Some("move") => {
            undo_moves(store, &items, now, &mut back, &mut gone, &mut operations)?;
            store.group_operations(&operations)?;
        }
        Some("mark") => {
            queued = undo_marks(store, &items, now, &mut back, &mut gone)?;
        }
        // What a rule did: it may have filed mail and marked it too.
        Some("rule") => {
            let moved: Vec<Value> = data["moved"].as_array().cloned().unwrap_or_default();
            let marked: Vec<Value> = data["marked"].as_array().cloned().unwrap_or_default();
            undo_moves(store, &moved, now, &mut back, &mut gone, &mut operations)?;
            store.group_operations(&operations)?;
            queued = undo_marks(store, &marked, now, &mut back, &mut gone)?;
            // A message both filed and marked is one message.
            let mut seen = std::collections::HashSet::new();
            back.retain(|id| seen.insert(*id));
            gone.sort_unstable();
            gone.dedup();
            gone.retain(|id| !seen.contains(&LocalMessageId(*id)));
        }
        Some("snooze") => {
            for item in &items {
                let Some(id) = item["id"].as_i64() else {
                    continue;
                };
                let message = LocalMessageId(id);
                if store.message_location(message)?.is_none() {
                    gone.push(id);
                    continue;
                }
                match item["until"].as_i64() {
                    Some(until) => {
                        store.snooze(&[message], until)?;
                    }
                    None => {
                        store.wake_snoozed(now, Some(&[message]))?;
                    }
                }
                back.push(message);
            }
        }
        // A message that was sent, while it has not gone.
        Some("send") => {
            let draft = data["draft"].as_i64().unwrap_or_default();
            if !super::write::unsend(store, draft, now)? {
                return Err(Refusal::new(
                    Code::CannotUndo,
                    format!(
                        "Change {} cannot be taken back: the message has gone, or is not in \
                         the outbox any more.",
                        found.id
                    ),
                ));
            }
            store.mark_agent_undone(found.id, now)?;
            return Ok(Done {
                value: json!({
                    "action": "undo", "entry": found.id, "undid": found.action,
                    "changed": 1, "draft": draft, "state": "draft",
                }),
                lines: vec![format!(
                    "Took back change {}: the message is draft {draft} again.",
                    found.id
                )],
                count: 1,
                summary: format!("took back change {} ({})", found.id, found.action),
                undo: None,
                said: Some(format!(
                    "took back change {}: a message is out of the Outbox, and a draft again.",
                    found.id
                )),
                undoable: false,
                queued: false,
                due: None,
            });
        }
        Some("folder") => {
            let folder = LocalFolderId(data["folder"].as_i64().unwrap_or_default());
            if !store.delete_empty_local_folder(folder)? {
                return Err(Refusal::new(
                    Code::CannotUndo,
                    "The folder has mail in it, or has gone. Move the mail out first.",
                ));
            }
            store.mark_agent_undone(found.id, now)?;
            return Ok(Done {
                value: json!({ "action": "undo", "entry": found.id, "undid": found.action, "changed": 1 }),
                lines: vec![format!(
                    "Took back change {}: the folder is gone.",
                    found.id
                )],
                count: 1,
                summary: format!("took back change {} ({})", found.id, found.action),
                undo: None,
                said: Some(format!("took back change {}.", found.id)),
                undoable: false,
                queued: false,
                due: None,
            });
        }
        _ => {
            return Err(Refusal::new(
                Code::CannotUndo,
                format!("Change {} cannot be taken back.", found.id),
            ));
        }
    }
    if back.is_empty() {
        return Err(Refusal::new(
            Code::CannotUndo,
            format!(
                "Change {} cannot be taken back: its messages are no longer where it left them.",
                found.id
            ),
        ));
    }
    store.mark_agent_undone(found.id, now)?;
    let names = Names::of(store)?;
    let after = store.message_rows(&back)?;
    let (messages_json, ids) = described(&after, &names);
    let count = back.len();
    let mut lines = vec![format!(
        "Took back change {} ({}): {}.",
        found.id,
        found.action,
        messages(count)
    )];
    if !gone.is_empty() {
        lines.push(format!(
            "{} could not be put back: no longer where the change left them.",
            messages(gone.len())
        ));
    }
    Ok(Done {
        value: json!({
            "action": "undo",
            "entry": found.id,
            "undid": found.action,
            "changed": count,
            "ids": ids,
            "messages": messages_json,
            "not_undone": gone,
        }),
        lines,
        count,
        summary: format!(
            "took back change {} ({}): {}",
            found.id,
            found.action,
            messages(count)
        ),
        undo: None,
        said: Some(format!(
            "took back change {} ({}): {}.",
            found.id,
            found.action,
            messages(count)
        )),
        undoable: !operations.is_empty(),
        queued: queued || !operations.is_empty(),
        due: None,
    })
}
