//! Which messages a request is about.
//!
//! A change names its messages by id, by Message-ID or by a search. What
//! comes back is only ever mail of the accounts agents may see, and never
//! more than a change may touch.

use std::collections::HashSet;

use jiff::tz::TimeZone;
use quill_core::config::BULK_CEILING;
use quill_core::search::Query;
use quill_core::types::{LocalAccountId, LocalMessageId};
use quill_store::{MessageRow, Store};

use super::{Code, Refusal, Settings, Which, messages};

/// Reads a search as the search box would, with days as they begin for
/// the user.
pub(crate) fn query(text: &str, tz: &TimeZone) -> Query {
    quill_core::search::parse(text, |year, month, day| {
        let date = jiff::civil::Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()?;
        Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_second())
    })
}

/// The accounts agents may see.
pub(crate) fn visible(
    store: &Store,
    settings: &Settings,
) -> quill_store::Result<Vec<LocalAccountId>> {
    Ok(store
        .accounts()?
        .into_iter()
        .filter(|account| settings.agent.sees(&account.name))
        .map(|account| account.id)
        .collect())
}

/// What a search finds among the mail agents may see, newest first, at
/// most `limit`. Returns the rows and whether there were more.
pub(crate) fn search(
    store: &Store,
    settings: &Settings,
    text: &str,
    limit: usize,
) -> Result<(Vec<MessageRow>, bool), Refusal> {
    let query = query(text, &settings.tz);
    if query.is_empty() {
        return Err(Refusal::invalid("There is nothing to search for."));
    }
    let seen = visible(store, settings)?;
    // With some accounts out of sight, more has to be asked for than is
    // wanted: what is out of sight is dropped afterwards.
    let all = settings.agent.accounts.is_none();
    let ask = if all {
        limit.saturating_add(1)
    } else {
        usize::MAX
    };
    let mut rows = store.search(&query, ask)?;
    rows.retain(|row| seen.contains(&row.account));
    let more = rows.len() > limit;
    rows.truncate(limit);
    Ok((rows, more))
}

/// The messages a change is for.
pub(crate) struct Chosen {
    /// The messages, as they are now.
    pub rows: Vec<MessageRow>,
    /// Ids and Message-IDs that named nothing agents may see.
    pub missing: Vec<String>,
}

/// Finds the messages `which` names. Refuses when none are named, when
/// too many are, and when a search finds more than may be changed
/// without being told to go on.
pub(crate) fn choose(store: &Store, settings: &Settings, which: &Which) -> Result<Chosen, Refusal> {
    let ceiling = BULK_CEILING as usize;
    let searched = which
        .query
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if which.ids.is_empty() && which.msgids.is_empty() && searched.is_none() {
        return Err(Refusal::invalid(
            "Say which messages: by id (ids), by Message-ID (msgids), or by a search (where).",
        ));
    }
    if which.ids.len() + which.msgids.len() > ceiling {
        return Err(Refusal::new(
            Code::OverLimit,
            format!("One change may touch {ceiling} messages at most."),
        ));
    }
    let seen = visible(store, settings)?;
    let mut rows: Vec<MessageRow> = Vec::new();
    let mut have: HashSet<LocalMessageId> = HashSet::new();
    let mut missing = Vec::new();
    let mut add = |row: MessageRow, rows: &mut Vec<MessageRow>| {
        if have.insert(row.id) {
            rows.push(row);
        }
    };

    for id in &which.ids {
        let found = store
            .message_rows(&[LocalMessageId(*id)])?
            .into_iter()
            .find(|row| seen.contains(&row.account));
        match found {
            Some(row) => add(row, &mut rows),
            None => missing.push(id.to_string()),
        }
    }
    for msgid in &which.msgids {
        let bare = msgid.trim().trim_start_matches('<').trim_end_matches('>');
        let query = Query {
            message_id: Some(bare.to_owned()).filter(|bare| !bare.is_empty()),
            ..Query::default()
        };
        let found: Vec<MessageRow> = store
            .search(&query, ceiling)?
            .into_iter()
            .filter(|row| seen.contains(&row.account))
            .collect();
        if found.is_empty() {
            missing.push(msgid.clone());
        }
        for row in found {
            add(row, &mut rows);
        }
    }
    if let Some(text) = searched {
        // One more than the most there may be, to know that there are
        // more.
        let (found, more) = search(store, settings, text, ceiling)?;
        if more {
            return Err(Refusal::new(
                Code::OverLimit,
                format!(
                    "The search finds more than {ceiling} messages, and one change may touch \
                     {ceiling} at most. Say more of what is wanted."
                ),
            ));
        }
        let limit = settings.agent.bulk_limit as usize;
        if found.len() > limit && !which.confirm && !which.dry_run {
            return Err(Refusal::new(
                Code::OverLimit,
                format!(
                    "The search finds {}, more than the {limit} one change may touch without \
                     being told to. Nothing was changed. Look at what it finds first (count, \
                     search, or dry_run), then give confirm to go on.",
                    messages(found.len())
                ),
            ));
        }
        for row in found {
            add(row, &mut rows);
        }
    }
    if rows.len() > ceiling {
        return Err(Refusal::new(
            Code::OverLimit,
            format!("One change may touch {ceiling} messages at most."),
        ));
    }
    Ok(Chosen { rows, missing })
}

/// How a selection is written in the log: what was asked, never what was
/// found.
pub(crate) fn asked(which: &Which) -> String {
    let mut parts = Vec::new();
    if !which.ids.is_empty() {
        parts.push(format!("{} by id", messages(which.ids.len())));
    }
    if !which.msgids.is_empty() {
        parts.push(format!("{} by Message-ID", messages(which.msgids.len())));
    }
    if let Some(text) = which
        .query
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(format!("where {}", crate::text::sanitize_line(text)));
    }
    parts.join(", ")
}
