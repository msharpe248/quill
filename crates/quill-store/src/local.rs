//! Accounts whose mail lives here: a POP3 server hands mail over and keeps
//! no folders or marks, so the copy in this store is the one that matters.

use std::collections::HashSet;

use quill_core::types::{LocalAccountId, LocalFolderId, LocalMessageId, RemoteId, RemoteMessage};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Store, contacts, insert_message_row};

impl Store {
    /// Which of `ids` name a message this account already has, in any of
    /// its folders. A server that lists a message again must not make a
    /// second copy of one that was filed elsewhere here.
    pub fn known_remote_ids(
        &self,
        account: LocalAccountId,
        ids: &[RemoteId],
    ) -> Result<HashSet<RemoteId>> {
        let mut known = self.conn.prepare(
            "SELECT 1 FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.account_id = ?1 AND m.remote_id = ?2",
        )?;
        let mut found = HashSet::new();
        for id in ids {
            if known.exists(params![account.0, id.0])? {
                found.insert(id.clone());
            }
        }
        Ok(found)
    }

    /// Records that the server holds these messages, first seen at `now`.
    /// One already recorded keeps the time it was first seen.
    pub fn note_server_copies(
        &mut self,
        account: LocalAccountId,
        ids: &[RemoteId],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO server_copies (account_id, remote_id, seen_at)
                 VALUES (?1, ?2, ?3)",
            )?;
            for id in ids {
                insert.execute(params![account.0, id.0, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Records that the server no longer holds these messages.
    pub fn forget_server_copies(
        &mut self,
        account: LocalAccountId,
        ids: &[RemoteId],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut delete =
                tx.prepare("DELETE FROM server_copies WHERE account_id = ?1 AND remote_id = ?2")?;
            for id in ids {
                delete.execute(params![account.0, id.0])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The messages that may be deleted from the server: first seen there
    /// at or before `seen_by`, and safe here, which means the whole message
    /// has been downloaded and filed. A message that is not here in full is
    /// never offered, whatever its age: the server's copy is then the only
    /// one.
    pub fn server_copies_to_delete(
        &self,
        account: LocalAccountId,
        seen_by: i64,
    ) -> Result<Vec<RemoteId>> {
        let mut statement = self.conn.prepare(
            "SELECT c.remote_id FROM server_copies c
             WHERE c.account_id = ?1 AND c.seen_at <= ?2
               AND EXISTS (
                   SELECT 1 FROM messages m JOIN folders f ON f.id = m.folder_id
                   WHERE f.account_id = c.account_id AND m.remote_id = c.remote_id
                     AND m.blob_hash IS NOT NULL)
             ORDER BY c.seen_at, c.remote_id",
        )?;
        let rows =
            statement.query_map(params![account.0, seen_by], |row| Ok(RemoteId(row.get(0)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many messages the server is believed to hold for the account.
    pub fn server_copies(&self, account: LocalAccountId) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM server_copies WHERE account_id = ?1",
            [account.0],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Removes the listed messages of an account that were never
    /// downloaded, wherever they were filed. For mail that left the server
    /// before Quill had fetched it: there is nothing here to keep, and a
    /// row that can never be opened is worse than none. A message that was
    /// downloaded stays. Returns whether anything was removed.
    pub fn drop_unfetched(&mut self, account: LocalAccountId, ids: &[RemoteId]) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut removed = 0;
        {
            let mut delete = tx.prepare(
                "DELETE FROM messages
                 WHERE remote_id = ?2 AND blob_hash IS NULL
                   AND folder_id IN (SELECT id FROM folders WHERE account_id = ?1)
                   AND NOT EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = messages.id)",
            )?;
            for id in ids {
                removed += delete.execute(params![account.0, id.0])?;
            }
        }
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Files a whole message in a folder that exists only here: a copy of
    /// mail sent from an account whose server keeps none. Returns the
    /// message's id; filing the same message twice gives the same one.
    pub fn keep_message(
        &mut self,
        folder: LocalFolderId,
        message: &RemoteMessage,
        text: &str,
        blob_hash: Option<&str>,
    ) -> Result<LocalMessageId> {
        let tx = self.conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM messages WHERE folder_id = ?1 AND remote_id = ?2",
                params![folder.0, message.id.0],
                |row| row.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                insert_message_row(&tx, folder, message)?;
                let id = tx.last_insert_rowid();
                crate::threads::assign(
                    &tx,
                    id,
                    message.headers.message_id.as_deref(),
                    &message.thread,
                )?;
                // Mail the user sent: its recipients are people they write
                // to.
                contacts::harvest(&tx, &message.headers, true)?;
                id
            }
        };
        tx.execute(
            "INSERT INTO bodies (message_id, text) VALUES (?1, ?2)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text",
            params![id, text],
        )?;
        if let Some(hash) = blob_hash {
            tx.execute(
                "UPDATE messages SET blob_hash = ?2 WHERE id = ?1",
                params![id, hash],
            )?;
        }
        tx.commit()?;
        Ok(LocalMessageId(id))
    }
}
