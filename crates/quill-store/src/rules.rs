//! What the rules need of the store: which mail in an inbox they have not
//! been asked about, a mark for what they have, and the mailing list a
//! message came through.

use quill_core::types::{LocalAccountId, LocalMessageId};
use rusqlite::params;

use crate::{Result, Store};

impl Store {
    /// The mail in the inboxes of `account` dated at or after `since`,
    /// newest first, at most `limit`. With `unasked`, only what the rules
    /// have not been asked about.
    pub fn inbox_mail(
        &self,
        account: LocalAccountId,
        since: i64,
        unasked: bool,
        limit: usize,
    ) -> Result<Vec<LocalMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT m.id FROM messages m JOIN folders f ON f.id = m.folder_id
              WHERE f.account_id = ?1 AND f.role = 'inbox' AND m.hidden = 0
                AND m.date >= ?2 AND (?3 = 0 OR m.ruled_at IS NULL)
              ORDER BY m.date DESC, m.id DESC LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![
                account.0,
                since,
                unasked,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| Ok(LocalMessageId(row.get(0)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes that the rules have been asked about these messages.
    pub fn mark_ruled(&mut self, messages: &[LocalMessageId], now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut statement =
                tx.prepare("UPDATE messages SET ruled_at = ?2 WHERE id = ?1 AND ruled_at IS NULL")?;
            for message in messages {
                statement.execute(params![message.0, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Which of `messages` have been downloaded: their text is here.
    pub fn with_body(&self, messages: &[LocalMessageId]) -> Result<Vec<LocalMessageId>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        // Ids are numbers of the store's own making.
        let ids: Vec<String> = messages.iter().map(|id| id.0.to_string()).collect();
        let mut statement = self.conn.prepare(&format!(
            "SELECT message_id FROM bodies WHERE message_id IN ({})",
            ids.join(",")
        ))?;
        let rows = statement.query_map([], |row| Ok(LocalMessageId(row.get(0)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Which of `messages` have not been downloaded and are no bigger
    /// than `largest` bytes: what is worth fetching before the rules are
    /// asked.
    pub fn small_without_body(
        &self,
        messages: &[LocalMessageId],
        largest: i64,
    ) -> Result<Vec<LocalMessageId>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = messages.iter().map(|id| id.0.to_string()).collect();
        let mut statement = self.conn.prepare(&format!(
            "SELECT m.id FROM messages m
              WHERE m.id IN ({}) AND m.size <= ?1 AND m.hidden = 0
                AND m.remote_id NOT LIKE 'local:op%'
                AND NOT EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = m.id)",
            ids.join(",")
        ))?;
        let rows = statement.query_map([largest], |row| Ok(LocalMessageId(row.get(0)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes the mailing list a message came through, as its `List-Id`
    /// says; `None` that it says none.
    pub fn set_list_id(&mut self, message: LocalMessageId, list: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET list_id = ?2 WHERE id = ?1",
            params![message.0, list.unwrap_or_default()],
        )?;
        Ok(())
    }

    /// Downloaded messages whose list has not been looked for: mail that
    /// was here before lists were kept. At most `limit`.
    pub fn without_list_id(&self, limit: usize) -> Result<Vec<LocalMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT id FROM messages
              WHERE list_id IS NULL AND blob_hash IS NOT NULL
              ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(LocalMessageId(row.get(0)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
