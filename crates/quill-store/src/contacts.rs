//! Everyone mail has come from or gone to, for completing addresses.
//!
//! Harvested from message headers as they sync. People the user has written
//! to rank above people who have only written to them: a newsletter sends
//! every week and is never who a new message is for.

use quill_core::types::Headers;
use rusqlite::{Transaction, params};

use crate::{Result, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub email: String,
}

/// How much more a recipient of sent mail counts than a sender of received
/// mail.
const SENT_WEIGHT: i64 = 5;

/// Records the people on one message. Called inside the transaction that
/// stores the message.
pub(crate) fn harvest(tx: &Transaction<'_>, headers: &Headers, sent_by_user: bool) -> Result<()> {
    let mut upsert = tx.prepare_cached(
        "INSERT INTO contacts (email, name, weight, last_seen) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (email) DO UPDATE SET
             weight = weight + excluded.weight,
             last_seen = max(last_seen, excluded.last_seen),
             -- A name beats no name, and the newest name is the one kept.
             name = CASE
                 WHEN excluded.name <> '' AND excluded.last_seen >= last_seen THEN excluded.name
                 WHEN name = '' THEN excluded.name
                 ELSE name
             END",
    )?;
    let people = if sent_by_user {
        headers
            .to
            .iter()
            .chain(headers.cc.iter())
            .collect::<Vec<_>>()
    } else {
        headers.from.iter().collect()
    };
    let weight = if sent_by_user { SENT_WEIGHT } else { 1 };
    for person in people {
        let email = person.email.trim();
        if email.is_empty() || !email.contains('@') {
            continue;
        }
        let name = person.name.as_deref().unwrap_or_default().trim();
        upsert.execute(params![email, name, weight, headers.date])?;
    }
    Ok(())
}

impl Store {
    /// People whose name or address has a part starting with `typed` (a
    /// word of the name, or what follows a dot or the @ of the address),
    /// best first: those written to most, then most recently.
    pub fn contacts_matching(&self, typed: &str, limit: usize) -> Result<Vec<Contact>> {
        let typed = typed.trim();
        if typed.is_empty() {
            return Ok(Vec::new());
        }
        // LIKE's own wildcards in what was typed are matched as themselves.
        let escaped = typed
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let start = format!("{escaped}%");
        let word = format!("% {escaped}%");
        let after_dot = format!("%.{escaped}%");
        let domain = format!("%@{escaped}%");
        let mut statement = self.conn.prepare(
            "SELECT name, email FROM contacts
             WHERE email LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\'
                OR name LIKE ?2 ESCAPE '\\' OR email LIKE ?3 ESCAPE '\\'
                OR email LIKE ?4 ESCAPE '\\'
             ORDER BY weight DESC, last_seen DESC, email
             LIMIT ?5",
        )?;
        let rows = statement
            .query_map(
                params![
                    start,
                    word,
                    after_dot,
                    domain,
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok(Contact {
                        name: row.get(0)?,
                        email: row.get(1)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
