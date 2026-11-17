//! Mail that lives here and nowhere else.
//!
//! A POP3 server hands mail over and keeps no folders or marks, so for
//! such an account the copy in this store is the one that matters. Any
//! account may also have folders the user made on this Mac: mail moved
//! into one is kept here whole and taken off its server.
//!
//! Also here: where a message was before it went to the trash, which no
//! server records, so that it can be put back.

use std::collections::HashSet;

use quill_core::types::{LocalAccountId, LocalFolderId, LocalMessageId, RemoteId, RemoteMessage};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Store, contacts, insert_message_row};

/// The start of the ids of folders, and of messages, that exist only in
/// the store. No server issues such an id.
pub const KEPT: &str = "kept:";

/// The start of the id of a folder the user made on this Mac. The rest of
/// it is the folder's name.
pub const LOCAL_FOLDER: &str = "kept:folder:";

/// Whether a folder's or a message's id is one of the store's own: there
/// is nothing on any server by that name.
pub fn is_kept(remote: &str) -> bool {
    remote.starts_with(KEPT)
}

/// Whether a folder's id is that of one the user made on this Mac.
pub fn is_made_here(remote: &str) -> bool {
    remote.starts_with(LOCAL_FOLDER)
}

/// How long a note of where a trashed message came from is kept.
const TRASHED_KEPT_SECONDS: i64 = 90 * 24 * 60 * 60;

/// The longest name a folder made here may have, in characters.
const LONGEST_FOLDER_NAME: usize = 60;

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

    /// Downloaded messages whose copies may be let go to make room, oldest
    /// first: each a message and the hash its copy is stored under. Only
    /// mail dated before `before`, and only mail whose server still has
    /// it to download again: never an account's whose mail is kept here,
    /// never a copy the server never had, and never a copy still waiting
    /// to be saved to a server.
    pub fn evictable(&self, before: i64, limit: usize) -> Result<Vec<(LocalMessageId, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT m.id, m.blob_hash FROM messages m
             JOIN folders f ON f.id = m.folder_id
             JOIN accounts a ON a.id = f.account_id
             WHERE m.blob_hash IS NOT NULL AND m.date < ?1
               AND a.kind <> 'pop3'
               AND m.remote_id NOT LIKE 'kept:%' AND m.remote_id NOT LIKE 'local:op%'
               AND NOT EXISTS (SELECT 1 FROM operations o WHERE o.blob_hash = m.blob_hash)
               AND NOT EXISTS (
                   SELECT 1 FROM messages k
                   JOIN folders kf ON kf.id = k.folder_id
                   JOIN accounts ka ON ka.id = kf.account_id
                   WHERE k.blob_hash = m.blob_hash
                     AND (ka.kind = 'pop3' OR k.remote_id LIKE 'kept:%'
                          OR k.remote_id LIKE 'local:op%' OR k.date >= ?1))
             ORDER BY m.date, m.id LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![before, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| Ok((LocalMessageId(row.get(0)?), row.get(1)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records that the copy stored under `hash` has been let go: every
    /// message that had it is as if not downloaded, to be fetched again
    /// when it is next opened. Their text stays, for searching.
    pub fn forget_blob(&mut self, hash: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE messages SET blob_hash = NULL WHERE blob_hash = ?1",
            [hash],
        )?)
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

    /// Makes a folder on this Mac for `account`, which no server will
    /// have. Returns it, or why there is none, in words for the user: a
    /// name that will not do, or one the account's local folders have
    /// already.
    pub fn create_local_folder(
        &mut self,
        account: LocalAccountId,
        name: &str,
    ) -> Result<std::result::Result<LocalFolderId, String>> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(Err("Give the folder a name.".to_owned()));
        }
        if name.chars().any(char::is_control) {
            return Ok(Err("A folder's name is one line of text.".to_owned()));
        }
        if name.chars().count() > LONGEST_FOLDER_NAME {
            return Ok(Err(format!(
                "A folder's name can be {LONGEST_FOLDER_NAME} characters at most."
            )));
        }
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM folders
                 WHERE account_id = ?1 AND remote_id LIKE 'kept:folder:%'
                   AND name = ?2 COLLATE NOCASE)",
            params![account.0, name],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(Err(format!(
                "There is already a folder called {name} on this Mac."
            )));
        }
        let id = self.conn.query_row(
            "INSERT INTO folders (account_id, remote_id, name, role)
             VALUES (?1, ?2, ?3, 'other') RETURNING id",
            params![account.0, format!("{LOCAL_FOLDER}{name}"), name],
            |row| row.get(0),
        )?;
        Ok(Ok(LocalFolderId(id)))
    }

    /// What a folder is known by: on its server, or, for one that is only
    /// here, in the store.
    pub fn folder_remote(&self, folder: LocalFolderId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT remote_id FROM folders WHERE id = ?1",
                [folder.0],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Whether `folder` is one the user made on this Mac.
    pub fn is_local_folder(&self, folder: LocalFolderId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT remote_id LIKE 'kept:folder:%' FROM folders WHERE id = ?1",
                [folder.0],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Takes away a folder made on this Mac, if there is nothing in it
    /// and nothing on its way into or out of it. Returns whether it went:
    /// a folder with mail in it, or one a server has, stays.
    pub fn delete_empty_local_folder(&mut self, folder: LocalFolderId) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM folders
             WHERE id = ?1 AND remote_id LIKE 'kept:folder:%'
               AND NOT EXISTS (SELECT 1 FROM messages WHERE folder_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM operations
                               WHERE source_folder = ?1 OR target_folder = ?1)",
            [folder.0],
        )? > 0)
    }

    /// Puts a second copy of a message into `folder`, as mail kept here:
    /// the same message, named by its content, with its text and its
    /// place in its conversation. Only for a message that is here whole.
    /// Returns whether there is a copy there now: `false` when the
    /// message is missing or has not been downloaded.
    pub fn copy_here(&mut self, message: LocalMessageId, folder: LocalFolderId) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let hash: Option<Option<String>> = tx
            .query_row(
                "SELECT blob_hash FROM messages WHERE id = ?1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(Some(hash)) = hash else {
            return Ok(false);
        };
        let kept = format!("{KEPT}{hash}");
        tx.execute(
            "INSERT INTO messages (folder_id, remote_id, message_id, subject, from_addrs,
                 to_addrs, cc_addrs, date, size, seen, flagged, answered, draft,
                 has_attachments, blob_hash, thread_id)
             SELECT ?2, ?3, message_id, subject, from_addrs, to_addrs, cc_addrs, date, size,
                    seen, flagged, answered, draft, has_attachments, blob_hash, thread_id
             FROM messages WHERE id = ?1
             ON CONFLICT (folder_id, remote_id) DO NOTHING",
            params![message.0, folder.0, kept],
        )?;
        tx.execute(
            "INSERT INTO bodies (message_id, text)
             SELECT c.id, b.text FROM bodies b, messages c
             WHERE b.message_id = ?1 AND c.folder_id = ?2 AND c.remote_id = ?3
             ON CONFLICT (message_id) DO NOTHING",
            params![message.0, folder.0, kept],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Notes that `message` is about to leave `from` for the trash, so
    /// that it can be put back there. Called before the move is made.
    pub fn note_trashed(
        &mut self,
        message: LocalMessageId,
        from: LocalFolderId,
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let found: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT f.account_id, m.message_id FROM messages m
                 JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
                [message.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((account, header)) = found else {
            return Ok(());
        };
        // One note for a message: where it was last.
        tx.execute(
            "DELETE FROM trashed WHERE message = ?1
                 OR (account_id = ?2 AND message_id IS NOT NULL AND message_id = ?3)",
            params![message.0, account, header],
        )?;
        tx.execute(
            "DELETE FROM trashed WHERE at < ?1",
            [now - TRASHED_KEPT_SECONDS],
        )?;
        tx.execute(
            "INSERT INTO trashed (account_id, message, message_id, from_folder, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account, message.0, header, from.0, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The folder `message` was in before it went to the trash, if Quill
    /// moved it there and the folder is still there. A message is known
    /// by its row, and once a sync has replaced the row, by its
    /// `Message-ID` within its account.
    pub fn trashed_from(&self, message: LocalMessageId) -> Result<Option<LocalFolderId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT t.from_folder FROM trashed t, messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE m.id = ?1 AND t.account_id = f.account_id
                   AND t.from_folder <> m.folder_id
                   AND (t.message = m.id
                        OR (t.message IS NULL AND t.message_id IS NOT NULL
                            AND t.message_id = m.message_id))
                 ORDER BY t.at DESC, t.id DESC LIMIT 1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?
            .map(LocalFolderId))
    }

    /// The message that went to a trash folder last and is still in one,
    /// with where it was before: what "put back the last deleted" means.
    pub fn last_trashed(&self) -> Result<Option<(LocalMessageId, LocalFolderId)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT m.id, t.from_folder FROM trashed t
                 JOIN messages m ON (t.message = m.id
                      OR (t.message IS NULL AND t.message_id IS NOT NULL
                          AND t.message_id = m.message_id))
                 JOIN folders f ON f.id = m.folder_id
                 WHERE f.account_id = t.account_id AND f.role = 'trash' AND m.hidden = 0
                   AND t.from_folder <> m.folder_id
                 ORDER BY t.at DESC, t.id DESC LIMIT 1",
                [],
                |row| Ok((LocalMessageId(row.get(0)?), LocalFolderId(row.get(1)?))),
            )
            .optional()?)
    }

    /// Lets go of the note of where `message` came from: it has been put
    /// back.
    pub fn forget_trashed(&mut self, message: LocalMessageId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM trashed WHERE message = ?1
                 OR (message IS NULL AND message_id IS NOT NULL AND message_id =
                     (SELECT message_id FROM messages WHERE id = ?1)
                     AND account_id = (SELECT f.account_id FROM messages m
                         JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1))",
            [message.0],
        )?;
        Ok(())
    }
}
