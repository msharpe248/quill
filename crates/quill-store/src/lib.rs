//! The local SQLite store: the single source of truth for the UI.
//!
//! All calls block on disk. Run them on the store worker, never on the UI task.

mod agents;
mod blobs;
mod contacts;
pub mod draft_file;
mod drafts;
mod folders;
mod local;
mod migrations;
mod queue;
mod rules;
mod search;
mod threads;

pub use agents::{AgentEntry, NewAgentEntry};
pub use blobs::BlobStore;
pub use contacts::Contact;
pub use drafts::{DraftAttachment, DraftContent, DraftId, DraftRow, DraftState, Outgoing};
pub use folders::FolderPlace;
pub use local::{KEPT, LOCAL_FOLDER, is_kept, is_made_here};
pub use queue::{OperationId, QueuedKind, QueuedOperation, is_placeholder};

use std::path::Path;

use quill_core::types::{
    Address, Flags, FolderId, FolderRole, Headers, LocalAccountId, LocalFolderId, LocalMessageId,
    RemoteFolder, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration {version} failed: {source}")]
    Migration {
        version: u32,
        source: rusqlite::Error,
    },
    #[error(
        "the database is at schema version {found}, but this Quill only knows up to {supported}"
    )]
    NewerSchema { found: u32, supported: u32 },
    #[error("cannot encode addresses: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cannot read or write a saved message: {0}")]
    Blob(#[from] std::io::Error),
}

impl StoreError {
    /// Another connection held the database for longer than this one was
    /// willing to wait. Nothing is wrong; trying again is the right answer.
    pub fn is_busy(&self) -> bool {
        use rusqlite::ErrorCode::{DatabaseBusy, DatabaseLocked};
        let code = match self {
            Self::Sqlite(err) | Self::Migration { source: err, .. } => err.sqlite_error_code(),
            Self::NewerSchema { .. } | Self::Json(_) | Self::Blob(_) => None,
        };
        matches!(code, Some(DatabaseBusy | DatabaseLocked))
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// An account as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub id: LocalAccountId,
    pub name: String,
    pub address: String,
    pub kind: String,
    /// The letter or number the user chose to stand for the account where
    /// mail of several is listed together. `None`: the first letter of
    /// its name.
    pub letter: Option<String>,
}

impl AccountRow {
    /// What stands for the account in a list of several accounts' mail:
    /// the letter chosen for it, or else the first letter of its name.
    pub fn initial(&self) -> Option<char> {
        self.letter
            .as_deref()
            .and_then(|letter| letter.chars().next())
            .or_else(|| self.name.chars().next())
    }
}

/// What of an account's exists nowhere but in the store: lost for good if
/// the account is removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Leftovers {
    /// Messages being written and not sent.
    pub drafts: usize,
    /// Messages waiting to be sent, or refused by a server.
    pub outbox: usize,
    /// Changes to mail (moves, deletions, marks) that have not reached the
    /// server yet.
    pub queued: usize,
    /// Messages in the folders made on this Mac, which no server has.
    pub local: usize,
}

/// A folder with its message counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: LocalFolderId,
    pub account: LocalAccountId,
    pub name: String,
    pub role: FolderRole,
    pub unread: u32,
    pub total: u32,
    /// A folder the user made on this Mac: no server has it, and the mail
    /// in it is nowhere else.
    pub local: bool,
}

/// One row of a message list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: LocalMessageId,
    pub folder: LocalFolderId,
    pub account: LocalAccountId,
    pub headers: Headers,
    pub flags: Flags,
    pub has_attachments: bool,
    /// The conversation the message belongs to. Messages with the same
    /// number belong together; one that is alone has a number nothing else
    /// has.
    pub thread: i64,
    /// When the message comes back, while it is snoozed.
    pub snoozed_until: Option<i64>,
}

/// Where a message lives on its server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLocation {
    pub account: LocalAccountId,
    pub folder: LocalFolderId,
    pub remote_folder: FolderId,
    pub remote: RemoteId,
    pub flags: Flags,
    pub date: i64,
}

/// Which messages a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mailbox {
    /// Every account's inbox, merged.
    Unified,
    /// One account's inbox: what choosing the account itself shows.
    Account(LocalAccountId),
    Folder(LocalFolderId),
    /// Mail being written, from every account. It lives only here.
    Drafts,
    /// Mail on its way out, or refused by a server, from every account.
    Outbox,
    /// Mail put aside until a time, from every account, the soonest back
    /// first.
    Snoozed,
}

pub struct Store {
    conn: Connection,
    /// Where raw messages are kept, when this connection may read them.
    blobs: Option<BlobStore>,
}

impl Store {
    /// Opens (creating if needed) the database at `path` in WAL mode and
    /// migrates it to the current schema.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A private, empty database that lives as long as the `Store`, with
    /// raw messages kept in memory beside it.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self::init(Connection::open_in_memory()?)?.with_blobs(BlobStore::in_memory()))
    }

    /// Says where raw messages are kept, so that [`Store::raw`] can read
    /// them and [`Store::put_message`] can save them.
    pub fn with_blobs(mut self, blobs: BlobStore) -> Self {
        self.blobs = Some(blobs);
        self
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // In-memory databases report "memory" here; files report "wal".
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // The UI and the sync engine each hold a connection. A writer that
        // finds the other mid-write waits its turn instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        // A transaction that reads first and writes later is refused
        // outright, not made to wait, when the other connection wrote in
        // between. Taking the write lock at the start makes it queue.
        conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        migrations::run(&mut conn)?;
        Ok(Self { conn, blobs: None })
    }

    pub fn schema_version(&self) -> Result<u32> {
        migrations::current_version(&self.conn)
    }

    /// Inserts the account, or updates it when the name is already known.
    pub fn upsert_account(
        &mut self,
        name: &str,
        address: &str,
        kind: &str,
    ) -> Result<LocalAccountId> {
        let id = self.conn.query_row(
            "INSERT INTO accounts (name, address, kind) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET address = excluded.address, kind = excluded.kind
             RETURNING id",
            params![name, address, kind],
            |row| row.get(0),
        )?;
        Ok(LocalAccountId(id))
    }

    /// Removes every account not named in `keep`, with its folders and mail.
    /// For accounts the user has taken out of the config.
    pub fn retain_accounts(&mut self, keep: &[&str]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let names: Vec<String> = {
            let mut statement = tx.prepare("SELECT name FROM accounts")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut removed = 0;
        for name in names {
            if !keep.contains(&name.as_str()) {
                removed += tx.execute("DELETE FROM accounts WHERE name = ?1", [&name])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Removes one account with everything of its own: its folders and
    /// mail, what it was writing, what was waiting to be sent and what had
    /// not reached its server yet. Returns whether there was such an
    /// account. For an account the user has asked to have removed, while
    /// Quill runs.
    pub fn remove_account(&mut self, id: LocalAccountId) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM accounts WHERE id = ?1", [id.0])?;
        Ok(removed > 0)
    }

    /// What of an account's is kept only here, and would go with it.
    pub fn account_leftovers(&self, id: LocalAccountId) -> Result<Leftovers> {
        let count = |sql: &str| -> Result<usize> {
            let count: i64 = self.conn.query_row(sql, [id.0], |row| row.get(0))?;
            Ok(usize::try_from(count).unwrap_or(0))
        };
        Ok(Leftovers {
            drafts: count("SELECT count(*) FROM drafts WHERE account_id = ?1 AND state = 'draft'")?,
            outbox: count(
                "SELECT count(*) FROM drafts WHERE account_id = ?1 AND state <> 'draft'",
            )?,
            queued: count("SELECT count(*) FROM operations WHERE account_id = ?1")?,
            local: count(
                "SELECT count(*) FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE f.account_id = ?1 AND f.remote_id LIKE 'kept:folder:%' AND m.hidden = 0",
            )?,
        })
    }

    /// Inserts the folder, or updates its name and role when already known.
    pub fn upsert_folder(
        &mut self,
        account: LocalAccountId,
        folder: &RemoteFolder,
    ) -> Result<LocalFolderId> {
        let id = self.conn.query_row(
            "INSERT INTO folders (account_id, remote_id, name, role) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (account_id, remote_id)
             DO UPDATE SET name = excluded.name, role = excluded.role
             RETURNING id",
            params![account.0, folder.id.0, folder.name, folder.role.as_str()],
            |row| row.get(0),
        )?;
        Ok(LocalFolderId(id))
    }

    /// Applies a folder sync and stores its cursor, atomically. Applying the
    /// same delta twice leaves the store unchanged. Returns whether any
    /// message was added, removed or changed, so that a sync which found
    /// nothing new need not make anything redraw.
    pub fn apply_delta(&mut self, folder: LocalFolderId, delta: &SyncDelta) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut touched = 0;
        {
            if delta.reset {
                // The server has renumbered the folder: nothing held for it
                // can be matched to a message any more. A message moved here
                // that the server has not heard about yet is not the
                // server's to renumber, and stays.
                touched += tx.execute(
                    "DELETE FROM messages WHERE folder_id = ?1 AND remote_id NOT LIKE 'local:op%'",
                    [folder.0],
                )?;
            }
            // Microsoft Graph does not say whether a message was answered,
            // so for those accounts the mark made here is the only one
            // there is, and what the server reports must not wipe it.
            let server_knows_answered: bool = tx
                .query_row(
                    "SELECT a.kind <> 'graph' FROM folders f
                     JOIN accounts a ON a.id = f.account_id WHERE f.id = ?1",
                    [folder.0],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(true);
            let answered = if server_knows_answered {
                "excluded.answered"
            } else {
                "answered"
            };
            // `WHERE` on the update keeps a re-sent, unchanged message from
            // counting as a change.
            let mut insert = tx.prepare(&format!(
                "INSERT INTO messages (folder_id, remote_id, message_id, subject, from_addrs,
                     to_addrs, cc_addrs, date, size, seen, flagged, answered, draft,
                     has_attachments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT (folder_id, remote_id) DO UPDATE SET
                     message_id = excluded.message_id, subject = excluded.subject,
                     from_addrs = excluded.from_addrs, to_addrs = excluded.to_addrs,
                     cc_addrs = excluded.cc_addrs, date = excluded.date, size = excluded.size,
                     seen = excluded.seen, flagged = excluded.flagged,
                     answered = {answered}, draft = excluded.draft,
                     has_attachments = excluded.has_attachments
                 WHERE message_id IS NOT excluded.message_id OR subject <> excluded.subject
                    OR from_addrs <> excluded.from_addrs OR to_addrs <> excluded.to_addrs
                    OR cc_addrs <> excluded.cc_addrs OR date <> excluded.date
                    OR size <> excluded.size OR seen <> excluded.seen
                    OR flagged <> excluded.flagged OR answered <> {answered}
                    OR draft <> excluded.draft OR has_attachments <> excluded.has_attachments"
            ))?;
            let mut known = tx.prepare(
                "SELECT id, thread_id FROM messages WHERE folder_id = ?1 AND remote_id = ?2",
            )?;
            // A message moved out of this folder here, which the server
            // has not been told about yet, is still listed by the server.
            // It is not brought back.
            let pending = queue::Pending::of(&tx, folder)?;
            let sent_folder: bool = tx
                .query_row(
                    "SELECT role = 'sent' FROM folders WHERE id = ?1",
                    [folder.0],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(false);
            for message in &delta.added {
                if pending.departed.contains(&message.id.0) {
                    continue;
                }
                // A backend may report a message again when it changes.
                // Its people are counted the first time only.
                let find = |known: &mut rusqlite::Statement<'_>| {
                    known
                        .query_row(params![folder.0, message.id.0], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
                        })
                        .optional()
                };
                let before = find(&mut known)?;
                touched += insert_message(&mut insert, folder, message)?;
                if before.is_none() {
                    contacts::harvest(&tx, &message.headers, sent_folder)?;
                }
                // Its place in a conversation, the first time anything is
                // known of it: when it is new, or was listed before
                // conversations were kept.
                let has_ids = message.headers.message_id.is_some() || !message.thread.is_empty();
                match before {
                    None => {
                        if let Some((id, _)) = find(&mut known)? {
                            threads::assign(
                                &tx,
                                id,
                                message.headers.message_id.as_deref(),
                                &message.thread,
                            )?;
                        }
                    }
                    Some((id, None)) if has_ids => {
                        threads::assign(
                            &tx,
                            id,
                            message.headers.message_id.as_deref(),
                            &message.thread,
                        )?;
                    }
                    Some(_) => {}
                }
            }
            let mut update = tx.prepare(
                "UPDATE messages SET seen = ?3, flagged = ?4, draft = ?6,
                     answered = CASE WHEN ?7 THEN ?5 ELSE answered END
                 WHERE folder_id = ?1 AND remote_id = ?2
                   AND (seen <> ?3 OR flagged <> ?4 OR (?7 AND answered <> ?5) OR draft <> ?6)",
            )?;
            for change in &delta.changed {
                let flags = change.flags;
                touched += update.execute(params![
                    folder.0,
                    change.id.0,
                    flags.seen,
                    flags.flagged,
                    flags.answered,
                    flags.draft,
                    server_knows_answered
                ])?;
            }
            let mut delete =
                tx.prepare("DELETE FROM messages WHERE folder_id = ?1 AND remote_id = ?2")?;
            for id in &delta.removed {
                touched += delete.execute(params![folder.0, id.0])?;
            }
            tx.execute(
                "INSERT INTO sync_state (folder_id, cursor) VALUES (?1, ?2)
                 ON CONFLICT (folder_id) DO UPDATE SET cursor = excluded.cursor",
                params![folder.0, delta.next_cursor.0],
            )?;
            // Flags changed here and not yet on the server win over what
            // the server still says.
            queue::reapply_flags(&tx, folder)?;
        }
        tx.commit()?;
        Ok(touched > 0)
    }

    /// Where the last sync of `folder` stopped, if it has ever synced.
    pub fn sync_cursor(&self, folder: LocalFolderId) -> Result<Option<SyncCursor>> {
        let cursor: Option<String> = self
            .conn
            .query_row(
                "SELECT cursor FROM sync_state WHERE folder_id = ?1",
                [folder.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(cursor.map(SyncCursor))
    }

    pub fn local_message_id(
        &self,
        folder: LocalFolderId,
        remote: &RemoteId,
    ) -> Result<Option<LocalMessageId>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM messages WHERE folder_id = ?1 AND remote_id = ?2",
                params![folder.0, remote.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id.map(LocalMessageId))
    }

    /// Stores the extracted body text of a message, replacing any earlier text.
    pub fn put_body(&mut self, message: LocalMessageId, text: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO bodies (message_id, text) VALUES (?1, ?2)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text",
            params![message.0, text],
        )?;
        Ok(())
    }

    /// Saves a whole message: its raw bytes among the blobs, when this store
    /// has somewhere to keep them, and its text for search.
    pub fn put_message(&mut self, message: LocalMessageId, text: &str, raw: &[u8]) -> Result<()> {
        match &self.blobs {
            Some(blobs) => {
                let hash = blobs.put(raw)?;
                self.put_downloaded(message, text, &hash)
            }
            None => self.put_body(message, text),
        }
    }

    /// Files raw bytes among the blobs and returns their hash, or `None`
    /// when this store has nowhere to keep them.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<Option<String>> {
        match &self.blobs {
            Some(blobs) => Ok(Some(blobs.put(bytes)?)),
            None => Ok(None),
        }
    }

    /// The bytes filed under `hash`, if this store can reach them.
    pub fn blob(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        match &self.blobs {
            Some(blobs) => Ok(blobs.get(hash)?),
            None => Ok(None),
        }
    }

    /// The message as it arrived, or `None` when it has not been downloaded
    /// or this store has no access to the raw messages.
    pub fn raw(&self, message: LocalMessageId) -> Result<Option<Vec<u8>>> {
        let Some(blobs) = &self.blobs else {
            return Ok(None);
        };
        match self.blob_hash(message)? {
            Some(hash) => Ok(blobs.get(&hash)?),
            None => Ok(None),
        }
    }

    /// The body text, or `None` when it has not been downloaded.
    pub fn body(&self, message: LocalMessageId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT text FROM bodies WHERE message_id = ?1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Accounts in the order they were added.
    pub fn accounts(&self) -> Result<Vec<AccountRow>> {
        let mut statement = self
            .conn
            .prepare("SELECT id, name, address, kind, letter FROM accounts ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(AccountRow {
                id: LocalAccountId(row.get(0)?),
                name: row.get(1)?,
                address: row.get(2)?,
                kind: row.get(3)?,
                letter: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes the letter that stands for the account called `name` where
    /// mail of several accounts is listed together, or with `None` that
    /// it has none of its own. Returns whether there is such an account.
    pub fn set_account_letter(&mut self, name: &str, letter: Option<&str>) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE accounts SET letter = ?2 WHERE name = ?1",
            params![name, letter],
        )?;
        Ok(changed > 0)
    }

    /// The accounts whose folders are folded away in the accounts pane.
    /// Every other account is open, one just added among them.
    pub fn closed_accounts(&self) -> Result<Vec<LocalAccountId>> {
        let mut statement = self
            .conn
            .prepare("SELECT account_id FROM closed_accounts ORDER BY account_id")?;
        let rows = statement.query_map([], |row| Ok(LocalAccountId(row.get(0)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Remembers whether an account's folders are on show in the accounts
    /// pane. An account that is not here any more is not remembered.
    pub fn set_account_open(&mut self, account: LocalAccountId, open: bool) -> Result<()> {
        if open {
            self.conn.execute(
                "DELETE FROM closed_accounts WHERE account_id = ?1",
                [account.0],
            )?;
        } else {
            self.conn.execute(
                "INSERT OR IGNORE INTO closed_accounts (account_id)
                 SELECT id FROM accounts WHERE id = ?1",
                [account.0],
            )?;
        }
        Ok(())
    }

    /// Every folder with its counts, grouped by account and ordered by role:
    /// Inbox, Drafts, Sent, Archive, Junk, Trash, then the rest by name.
    pub fn folders(&self) -> Result<Vec<FolderRow>> {
        let mut statement = self.conn.prepare(
            "SELECT f.id, f.account_id, f.name, f.role,
                    (SELECT count(*) FROM messages m
                     WHERE m.folder_id = f.id AND m.seen = 0 AND m.hidden = 0
                       AND m.snoozed_until IS NULL),
                    (SELECT count(*) FROM messages m
                     WHERE m.folder_id = f.id AND m.hidden = 0 AND m.snoozed_until IS NULL),
                    f.remote_id LIKE 'kept:folder:%'
             FROM folders f
             ORDER BY f.account_id,
                      -- The folders made on this Mac come after the rest.
                      f.remote_id LIKE 'kept:folder:%',
                      CASE f.role
                          WHEN 'inbox' THEN 0 WHEN 'drafts' THEN 1 WHEN 'sent' THEN 2
                          WHEN 'archive' THEN 3 WHEN 'junk' THEN 4 WHEN 'trash' THEN 5
                          ELSE 6
                      END,
                      f.name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            let role: String = row.get(3)?;
            Ok(FolderRow {
                id: LocalFolderId(row.get(0)?),
                account: LocalAccountId(row.get(1)?),
                name: row.get(2)?,
                role: FolderRole::from_stored(&role),
                unread: row.get(4)?,
                total: row.get(5)?,
                local: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The messages of a mailbox, newest first.
    pub fn messages(&self, mailbox: Mailbox) -> Result<Vec<MessageRow>> {
        const COLUMNS: &str = "m.id, m.folder_id, f.account_id, m.message_id, m.subject,
             m.from_addrs, m.to_addrs, m.cc_addrs, m.date, m.seen, m.flagged, m.answered,
             m.draft, m.has_attachments, COALESCE(m.thread_id, -m.id), m.snoozed_until";
        // A message back from a snooze sorts by when it came back.
        const ORDER: &str = "ORDER BY COALESCE(m.surfaced, m.date) DESC, m.id DESC";
        let rows = match mailbox {
            Mailbox::Unified => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE f.role = 'inbox' AND m.hidden = 0 AND m.snoozed_until IS NULL
                     {ORDER}"
                ))?;
                statement
                    .query_map([], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            Mailbox::Account(account) => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE f.role = 'inbox' AND f.account_id = ?1
                       AND m.hidden = 0 AND m.snoozed_until IS NULL
                     {ORDER}"
                ))?;
                statement
                    .query_map([account.0], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            Mailbox::Folder(folder) => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE m.folder_id = ?1 AND m.hidden = 0 AND m.snoozed_until IS NULL
                     {ORDER}"
                ))?;
                statement
                    .query_map([folder.0], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            Mailbox::Drafts => self.drafts()?.iter().map(DraftRow::as_message).collect(),
            Mailbox::Outbox => self.outbox()?.iter().map(DraftRow::as_message).collect(),
            Mailbox::Snoozed => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE m.snoozed_until IS NOT NULL AND m.hidden = 0
                     ORDER BY m.snoozed_until, m.id"
                ))?;
                statement
                    .query_map([], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// These messages, in the order asked for, leaving out any that are no
    /// longer here.
    pub fn message_rows(&self, ids: &[LocalMessageId]) -> Result<Vec<MessageRow>> {
        let mut statement = self.conn.prepare(
            "SELECT m.id, m.folder_id, f.account_id, m.message_id, m.subject,
                    m.from_addrs, m.to_addrs, m.cc_addrs, m.date, m.seen, m.flagged, m.answered,
                    m.draft, m.has_attachments, COALESCE(m.thread_id, -m.id), m.snoozed_until
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.id = ?1 AND m.hidden = 0",
        )?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            rows.extend(statement.query_row([id.0], message_row).optional()?);
        }
        Ok(rows)
    }

    /// Puts messages aside until `until`: out of their folders' lists and
    /// counts, and into the Snoozed mailbox. Snoozing one again changes
    /// when it comes back. Returns how many were put aside.
    pub fn snooze(&mut self, messages: &[LocalMessageId], until: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut count = 0;
        {
            let mut set =
                tx.prepare("UPDATE messages SET snoozed_until = ?2 WHERE id = ?1 AND hidden = 0")?;
            for message in messages {
                count += set.execute(params![message.0, until])?;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    /// Brings back every message whose time has come by `now`, to the top
    /// of its folder, and returns them. With `only`, brings those back
    /// whatever their time: the user wants them now.
    pub fn wake_snoozed(
        &mut self,
        now: i64,
        only: Option<&[LocalMessageId]>,
    ) -> Result<Vec<LocalMessageId>> {
        let tx = self.conn.transaction()?;
        let due: Vec<i64> = match only {
            Some(ids) => {
                let mut find = tx.prepare(
                    "SELECT id FROM messages WHERE id = ?1 AND snoozed_until IS NOT NULL",
                )?;
                let mut due = Vec::new();
                for id in ids {
                    due.extend(
                        find.query_row([id.0], |row| row.get::<_, i64>(0))
                            .optional()?,
                    );
                }
                due
            }
            None => tx
                .prepare(
                    "SELECT id FROM messages
                     WHERE snoozed_until IS NOT NULL AND snoozed_until <= ?1
                     ORDER BY snoozed_until, id",
                )?
                .query_map([now], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        };
        for id in &due {
            tx.execute(
                "UPDATE messages SET snoozed_until = NULL, surfaced = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        }
        tx.commit()?;
        Ok(due.into_iter().map(LocalMessageId).collect())
    }

    /// How many messages are snoozed, and when the next of them is due.
    pub fn snoozed(&self) -> Result<(u32, Option<i64>)> {
        Ok(self.conn.query_row(
            "SELECT count(*), min(snoozed_until) FROM messages
             WHERE snoozed_until IS NOT NULL AND hidden = 0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// Marks a message read or unread. Returns whether the message exists.
    pub fn set_seen(&mut self, message: LocalMessageId, seen: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET seen = ?2 WHERE id = ?1",
            params![message.0, seen],
        )?;
        Ok(changed > 0)
    }

    /// Where a message lives on its server, and its flags as last known.
    pub fn message_location(&self, message: LocalMessageId) -> Result<Option<MessageLocation>> {
        Ok(self
            .conn
            .query_row(
                "SELECT f.account_id, f.id, f.remote_id, m.remote_id,
                        m.seen, m.flagged, m.answered, m.draft, m.date
                 FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.id = ?1",
                [message.0],
                |row| {
                    Ok(MessageLocation {
                        account: LocalAccountId(row.get(0)?),
                        folder: LocalFolderId(row.get(1)?),
                        remote_folder: FolderId(row.get(2)?),
                        remote: RemoteId(row.get(3)?),
                        flags: Flags {
                            seen: row.get(4)?,
                            flagged: row.get(5)?,
                            answered: row.get(6)?,
                            draft: row.get(7)?,
                        },
                        date: row.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    /// Sets all four flags of a message. Returns whether the message exists.
    pub fn set_flags(&mut self, message: LocalMessageId, flags: Flags) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET seen = ?2, flagged = ?3, answered = ?4, draft = ?5 WHERE id = ?1",
            params![
                message.0,
                flags.seen,
                flags.flagged,
                flags.answered,
                flags.draft
            ],
        )?;
        Ok(changed > 0)
    }

    /// Records the downloaded message: its extracted text, and the hash its
    /// raw bytes are filed under.
    pub fn put_downloaded(
        &mut self,
        message: LocalMessageId,
        text: &str,
        blob_hash: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO bodies (message_id, text) VALUES (?1, ?2)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text",
            params![message.0, text],
        )?;
        tx.execute(
            "UPDATE messages SET blob_hash = ?2 WHERE id = ?1",
            params![message.0, blob_hash],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The hash of a message's raw bytes, if it has been downloaded.
    pub fn blob_hash(&self, message: LocalMessageId) -> Result<Option<String>> {
        let hash: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT blob_hash FROM messages WHERE id = ?1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(hash.flatten())
    }

    /// The hash of the raw bytes of another copy of this message that has
    /// been downloaded: the same account's, with the same `Message-ID`,
    /// the same size and the same date. One message is often in several
    /// folders (Gmail keeps everything in All Mail too), and is the same
    /// bytes in each.
    ///
    /// The size is what makes the three enough: a copy that was changed on
    /// its way (a `Bcc` kept in the sender's own, a list's footer) is not
    /// the same length.
    pub fn downloaded_copy(&self, message: LocalMessageId) -> Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT o.blob_hash FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 JOIN messages o ON o.message_id = m.message_id
                 JOIN folders g ON g.id = o.folder_id
                 WHERE m.id = ?1 AND o.id != m.id AND g.account_id = f.account_id
                   AND m.message_id IS NOT NULL AND m.message_id != ''
                   AND m.size > 0 AND o.size = m.size AND o.date = m.date
                   AND o.blob_hash IS NOT NULL
                   AND EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = o.id)
                 LIMIT 1",
                [message.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(hash)
    }

    /// Those of `messages` that are a server's and have not been
    /// downloaded, in the order given, each with its account.
    pub fn not_downloaded(
        &self,
        messages: &[LocalMessageId],
    ) -> Result<Vec<(LocalMessageId, LocalAccountId)>> {
        let mut statement = self.conn.prepare(
            "SELECT f.account_id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.id = ?1 AND m.remote_id NOT LIKE 'local:op%'
               AND NOT EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = m.id)",
        )?;
        let mut found = Vec::new();
        for message in messages {
            let account: Option<i64> = statement
                .query_row([message.0], |row| row.get(0))
                .optional()?;
            if let Some(account) = account {
                found.push((*message, LocalAccountId(account)));
            }
        }
        Ok(found)
    }

    /// Messages of an account dated at or after `since` whose bodies have
    /// not been downloaded, newest first, at most `limit` of them.
    pub fn messages_without_body(
        &self,
        account: LocalAccountId,
        since: i64,
        limit: usize,
    ) -> Result<Vec<LocalMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT m.id FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE f.account_id = ?1 AND m.date >= ?2 AND m.hidden = 0
               AND m.remote_id NOT LIKE 'local:op%'
               AND NOT EXISTS (SELECT 1 FROM bodies b WHERE b.message_id = m.id)
             ORDER BY m.date DESC, m.id DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![account.0, since, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| Ok(LocalMessageId(row.get(0)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Removes the folders of `account` that are not in `keep`, with their
    /// messages. For folders deleted on the server: a folder made on this
    /// Mac is no server's to delete, and always stays.
    pub fn retain_folders(&mut self, account: LocalAccountId, keep: &[FolderId]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let existing: Vec<(i64, String)> = {
            let mut statement =
                tx.prepare("SELECT id, remote_id FROM folders WHERE account_id = ?1")?;
            let rows = statement.query_map([account.0], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut removed = 0;
        for (id, remote) in existing {
            if !keep.iter().any(|folder| folder.0 == remote) && !local::is_made_here(&remote) {
                removed += tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Stars or unstars a message. Returns whether the message exists.
    pub fn set_flagged(&mut self, message: LocalMessageId, flagged: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET flagged = ?2 WHERE id = ?1",
            params![message.0, flagged],
        )?;
        Ok(changed > 0)
    }
}

fn insert_message(
    insert: &mut rusqlite::Statement<'_>,
    folder: LocalFolderId,
    message: &RemoteMessage,
) -> Result<usize> {
    let headers = &message.headers;
    let flags = message.flags;
    Ok(insert.execute(params![
        folder.0,
        message.id.0,
        headers.message_id,
        headers.subject,
        serde_json::to_string(&headers.from)?,
        serde_json::to_string(&headers.to)?,
        serde_json::to_string(&headers.cc)?,
        headers.date,
        i64::try_from(message.size).unwrap_or(i64::MAX),
        flags.seen,
        flags.flagged,
        flags.answered,
        flags.draft,
        message.has_attachments,
    ])?)
}

/// Inserts a message that is known not to be in the folder yet.
pub(crate) fn insert_message_row(
    tx: &rusqlite::Transaction<'_>,
    folder: LocalFolderId,
    message: &RemoteMessage,
) -> Result<()> {
    let mut insert = tx.prepare(
        "INSERT INTO messages (folder_id, remote_id, message_id, subject, from_addrs,
             to_addrs, cc_addrs, date, size, seen, flagged, answered, draft,
             has_attachments)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;
    insert_message(&mut insert, folder, message)?;
    Ok(())
}

fn message_row(row: &Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: LocalMessageId(row.get(0)?),
        folder: LocalFolderId(row.get(1)?),
        account: LocalAccountId(row.get(2)?),
        headers: Headers {
            message_id: row.get(3)?,
            subject: row.get(4)?,
            from: addresses(row, 5)?,
            to: addresses(row, 6)?,
            cc: addresses(row, 7)?,
            date: row.get(8)?,
        },
        flags: Flags {
            seen: row.get(9)?,
            flagged: row.get(10)?,
            answered: row.get(11)?,
            draft: row.get(12)?,
        },
        has_attachments: row.get(13)?,
        thread: row.get(14)?,
        snoozed_until: row.get(15)?,
    })
}

fn addresses(row: &Row<'_>, index: usize) -> rusqlite::Result<Vec<Address>> {
    let json: String = row.get(index)?;
    serde_json::from_str(&json)
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(err)))
}

#[cfg(test)]
mod tests {
    use quill_core::types::{FlagChange, FolderId};

    use super::*;

    fn folder(id: &str, role: FolderRole) -> RemoteFolder {
        RemoteFolder {
            id: FolderId(id.to_owned()),
            name: id.to_owned(),
            role,
        }
    }

    fn message(id: &str, subject: &str, date: i64, seen: bool) -> RemoteMessage {
        RemoteMessage {
            id: RemoteId(id.to_owned()),
            headers: Headers {
                message_id: Some(format!("<{id}@example.com>")),
                subject: subject.to_owned(),
                from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                to: vec![Address::new("", "robin@example.com")],
                cc: Vec::new(),
                date,
            },
            flags: Flags {
                seen,
                ..Flags::default()
            },
            size: 100,
            has_attachments: false,
            thread: Vec::new(),
        }
    }

    fn delta(added: Vec<RemoteMessage>, cursor: &str) -> SyncDelta {
        SyncDelta {
            added,
            changed: Vec::new(),
            removed: Vec::new(),
            reset: false,
            has_more: false,
            next_cursor: SyncCursor(cursor.to_owned()),
        }
    }

    /// One account, an inbox with three messages (two unread) and an archive
    /// with one.
    fn seeded() -> (Store, LocalFolderId, LocalFolderId) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        let inbox = store
            .upsert_folder(account, &folder("inbox", FolderRole::Inbox))
            .unwrap();
        let archive = store
            .upsert_folder(account, &folder("archive", FolderRole::Archive))
            .unwrap();
        store
            .apply_delta(
                inbox,
                &delta(
                    vec![
                        message("1", "oldest", 100, true),
                        message("2", "newest", 300, false),
                        message("3", "middle", 200, false),
                    ],
                    "c1",
                ),
            )
            .unwrap();
        store
            .apply_delta(
                archive,
                &delta(vec![message("4", "archived", 250, true)], "c1"),
            )
            .unwrap();
        (store, inbox, archive)
    }

    fn subjects(rows: &[MessageRow]) -> Vec<&str> {
        rows.iter()
            .map(|row| row.headers.subject.as_str())
            .collect()
    }

    #[test]
    fn a_new_file_database_is_migrated_and_in_wal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            migrations::latest_version()
        );
        let mode: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let foreign_keys: bool = store
            .conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert!(foreign_keys);
    }

    #[test]
    fn reopening_keeps_the_data_and_does_not_rerun_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        {
            let mut store = Store::open(&path).unwrap();
            store
                .upsert_account("Work", "robin@example.com", "demo")
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.accounts().unwrap().len(), 1);
        assert_eq!(
            store.schema_version().unwrap(),
            migrations::latest_version()
        );
    }

    #[test]
    fn a_database_from_a_newer_quill_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        drop(Store::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 9999).unwrap();
        drop(conn);
        match Store::open(&path) {
            Err(StoreError::NewerSchema { found: 9999, .. }) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => panic!("a newer database must not open"),
        }
    }

    #[test]
    fn an_account_is_stood_for_by_its_letter_or_else_its_names_first() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_account("work@example.com", "work@example.com", "demo")
            .unwrap();
        let initial = |store: &Store| store.accounts().unwrap()[0].initial();
        assert_eq!(initial(&store), Some('w'));

        assert!(
            store
                .set_account_letter("work@example.com", Some("7"))
                .unwrap()
        );
        assert_eq!(initial(&store), Some('7'));
        // Registered again, as it is every time Quill starts, it keeps it.
        store
            .upsert_account("work@example.com", "work@example.com", "demo")
            .unwrap();
        assert_eq!(initial(&store), Some('7'));

        assert!(store.set_account_letter("work@example.com", None).unwrap());
        assert_eq!(initial(&store), Some('w'));
        // No such account: nothing is noted.
        assert!(!store.set_account_letter("Nobody", Some("n")).unwrap());
    }

    #[test]
    fn upserting_an_account_or_folder_twice_keeps_one_row() {
        let mut store = Store::open_in_memory().unwrap();
        let first = store
            .upsert_account("Work", "old@example.com", "demo")
            .unwrap();
        let second = store
            .upsert_account("Work", "new@example.com", "demo")
            .unwrap();
        assert_eq!(first, second);
        let accounts = store.accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].address, "new@example.com");

        let a = store
            .upsert_folder(first, &folder("inbox", FolderRole::Other))
            .unwrap();
        let b = store
            .upsert_folder(first, &folder("inbox", FolderRole::Inbox))
            .unwrap();
        assert_eq!(a, b);
        let folders = store.folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].role, FolderRole::Inbox);
    }

    #[test]
    fn messages_come_back_newest_first_with_their_headers() {
        let (store, inbox, _) = seeded();
        let rows = store.messages(Mailbox::Folder(inbox)).unwrap();
        assert_eq!(subjects(&rows), ["newest", "middle", "oldest"]);
        let first = &rows[0];
        assert_eq!(first.headers.from[0].display_name(), "Ana Ruiz");
        assert_eq!(first.headers.to[0].name, None);
        assert_eq!(first.headers.message_id.as_deref(), Some("<2@example.com>"));
        assert!(!first.flags.seen);
    }

    #[test]
    fn folder_counts_track_unread_and_total() {
        let (store, inbox, archive) = seeded();
        let folders = store.folders().unwrap();
        let find = |id| folders.iter().find(|f| f.id == id).unwrap();
        assert_eq!((find(inbox).unread, find(inbox).total), (2, 3));
        assert_eq!((find(archive).unread, find(archive).total), (0, 1));
    }

    #[test]
    fn folders_are_ordered_by_role_within_an_account() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        for (id, role) in [
            ("trash", FolderRole::Trash),
            ("zebra", FolderRole::Other),
            ("sent", FolderRole::Sent),
            ("Alpha", FolderRole::Other),
            ("inbox", FolderRole::Inbox),
            ("drafts", FolderRole::Drafts),
        ] {
            store.upsert_folder(account, &folder(id, role)).unwrap();
        }
        let names: Vec<String> = store
            .folders()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(
            names,
            ["inbox", "drafts", "sent", "trash", "Alpha", "zebra"]
        );
    }

    #[test]
    fn the_unified_mailbox_merges_inboxes_only() {
        let (mut store, _, _) = seeded();
        let personal = store
            .upsert_account("Personal", "robin.vale@example.com", "demo")
            .unwrap();
        let inbox = store
            .upsert_folder(personal, &folder("inbox", FolderRole::Inbox))
            .unwrap();
        store
            .apply_delta(
                inbox,
                &delta(vec![message("1", "personal", 275, false)], "c1"),
            )
            .unwrap();
        let rows = store.messages(Mailbox::Unified).unwrap();
        // "archived" (date 250) is not in an inbox and must not appear.
        assert_eq!(subjects(&rows), ["newest", "personal", "middle", "oldest"]);
        assert_eq!(rows[1].account, personal);

        // An account's own mailbox is its inbox and nobody else's.
        let rows = store.messages(Mailbox::Account(personal)).unwrap();
        assert_eq!(subjects(&rows), ["personal"]);
        let work = store.accounts().unwrap()[0].id;
        let rows = store.messages(Mailbox::Account(work)).unwrap();
        assert_eq!(subjects(&rows), ["newest", "middle", "oldest"]);
        assert!(
            store
                .messages(Mailbox::Account(LocalAccountId(9999)))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_closed_account_is_remembered_across_a_restart_and_a_new_one_starts_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let (work, personal) = {
            let mut store = Store::open(&path).unwrap();
            let work = store
                .upsert_account("Work", "robin@example.com", "imap")
                .unwrap();
            let personal = store
                .upsert_account("Personal", "robin.vale@example.com", "imap")
                .unwrap();
            // Nothing has been closed yet: every account is open.
            assert!(store.closed_accounts().unwrap().is_empty());
            store.set_account_open(personal, false).unwrap();
            // Said twice, it is still one account closed.
            store.set_account_open(personal, false).unwrap();
            assert_eq!(store.closed_accounts().unwrap(), [personal]);
            (work, personal)
        };

        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.closed_accounts().unwrap(), [personal]);
        // An account added later starts open, and so does one whose
        // settings are written again.
        let third = store
            .upsert_account("Club", "robin@club.example.com", "imap")
            .unwrap();
        store
            .upsert_account("Personal", "robin.vale@example.com", "imap")
            .unwrap();
        assert_eq!(store.closed_accounts().unwrap(), [personal]);
        assert!(!store.closed_accounts().unwrap().contains(&third));

        store.set_account_open(work, false).unwrap();
        store.set_account_open(personal, true).unwrap();
        // Opening what is open changes nothing.
        store.set_account_open(third, true).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.closed_accounts().unwrap(), [work]);

        // An account that is not here is not remembered, and one that is
        // taken away takes what was remembered with it.
        store.set_account_open(LocalAccountId(9999), false).unwrap();
        assert_eq!(store.closed_accounts().unwrap(), [work]);
        store.retain_accounts(&["Personal", "Club"]).unwrap();
        assert!(store.closed_accounts().unwrap().is_empty());
    }

    #[test]
    fn applying_the_same_delta_twice_changes_nothing() {
        let (mut store, inbox, _) = seeded();
        let before = store.messages(Mailbox::Folder(inbox)).unwrap();
        let again = delta(
            vec![
                message("1", "oldest", 100, true),
                message("2", "newest", 300, false),
                message("3", "middle", 200, false),
            ],
            "c1",
        );
        store.apply_delta(inbox, &again).unwrap();
        assert_eq!(store.messages(Mailbox::Folder(inbox)).unwrap(), before);
    }

    #[test]
    fn a_delta_changes_flags_removes_messages_and_moves_the_cursor() {
        let (mut store, inbox, _) = seeded();
        assert_eq!(
            store.sync_cursor(inbox).unwrap(),
            Some(SyncCursor("c1".to_owned()))
        );
        let next = SyncDelta {
            added: vec![message("5", "brand new", 400, false)],
            changed: vec![FlagChange {
                id: RemoteId("2".to_owned()),
                flags: Flags {
                    seen: true,
                    flagged: true,
                    ..Flags::default()
                },
            }],
            removed: vec![RemoteId("1".to_owned())],
            reset: false,
            has_more: false,
            next_cursor: SyncCursor("c2".to_owned()),
        };
        store.apply_delta(inbox, &next).unwrap();
        let rows = store.messages(Mailbox::Folder(inbox)).unwrap();
        assert_eq!(subjects(&rows), ["brand new", "newest", "middle"]);
        assert!(rows[1].flags.seen && rows[1].flags.flagged);
        assert_eq!(
            store.sync_cursor(inbox).unwrap(),
            Some(SyncCursor("c2".to_owned()))
        );
    }

    #[test]
    fn a_folder_that_never_synced_has_no_cursor() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        let inbox = store
            .upsert_folder(account, &folder("inbox", FolderRole::Inbox))
            .unwrap();
        assert_eq!(store.sync_cursor(inbox).unwrap(), None);
    }

    #[test]
    fn flags_can_be_toggled_and_missing_messages_report_false() {
        let (mut store, inbox, _) = seeded();
        let id = store
            .local_message_id(inbox, &RemoteId("2".to_owned()))
            .unwrap()
            .unwrap();
        assert!(store.set_seen(id, true).unwrap());
        assert!(store.set_flagged(id, true).unwrap());
        let rows = store.messages(Mailbox::Folder(inbox)).unwrap();
        let row = rows.iter().find(|row| row.id == id).unwrap();
        assert!(row.flags.seen && row.flags.flagged);
        assert_eq!(store.folders().unwrap()[0].unread, 1);
        assert!(!store.set_seen(LocalMessageId(9999), true).unwrap());
    }

    #[test]
    fn bodies_are_stored_replaced_and_removed_with_their_message() {
        let (mut store, inbox, _) = seeded();
        let id = store
            .local_message_id(inbox, &RemoteId("2".to_owned()))
            .unwrap()
            .unwrap();
        assert_eq!(store.body(id).unwrap(), None);
        store.put_body(id, "first").unwrap();
        store.put_body(id, "second").unwrap();
        assert_eq!(store.body(id).unwrap().as_deref(), Some("second"));

        let removal = SyncDelta {
            added: Vec::new(),
            changed: Vec::new(),
            removed: vec![RemoteId("2".to_owned())],
            reset: false,
            has_more: false,
            next_cursor: SyncCursor("c2".to_owned()),
        };
        store.apply_delta(inbox, &removal).unwrap();
        assert_eq!(store.body(id).unwrap(), None);
        let orphans: i64 = store
            .conn
            .query_row("SELECT count(*) FROM bodies", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }
}

#[cfg(test)]
mod reset_tests {
    use quill_core::types::{FolderId, FolderRole, Headers, RemoteFolder};

    use super::*;

    #[test]
    fn a_reset_delta_replaces_the_folders_contents_and_only_that_folders() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let folder = |id: &str| RemoteFolder {
            id: FolderId(id.to_owned()),
            name: id.to_owned(),
            role: FolderRole::Other,
        };
        let message = |id: &str| RemoteMessage {
            id: RemoteId(id.to_owned()),
            headers: Headers {
                message_id: None,
                subject: id.to_owned(),
                from: Vec::new(),
                to: Vec::new(),
                cc: Vec::new(),
                date: 1,
            },
            flags: Flags::default(),
            size: 1,
            has_attachments: false,
            thread: Vec::new(),
        };
        let delta = |reset, ids: &[&str], cursor: &str| SyncDelta {
            reset,
            added: ids.iter().map(|id| message(id)).collect(),
            changed: Vec::new(),
            removed: Vec::new(),
            has_more: false,
            next_cursor: SyncCursor(cursor.to_owned()),
        };
        let a = store.upsert_folder(account, &folder("a")).unwrap();
        let b = store.upsert_folder(account, &folder("b")).unwrap();
        store
            .apply_delta(a, &delta(false, &["1", "2"], "v1"))
            .unwrap();
        store.apply_delta(b, &delta(false, &["1"], "v1")).unwrap();
        let old = store
            .local_message_id(a, &RemoteId("1".to_owned()))
            .unwrap()
            .unwrap();
        store.put_body(old, "old body").unwrap();

        store.apply_delta(a, &delta(true, &["9"], "v2")).unwrap();
        let subjects: Vec<String> = store
            .messages(Mailbox::Folder(a))
            .unwrap()
            .into_iter()
            .map(|row| row.headers.subject)
            .collect();
        assert_eq!(subjects, ["9"]);
        assert_eq!(store.body(old).unwrap(), None);
        assert_eq!(store.messages(Mailbox::Folder(b)).unwrap().len(), 1);
        assert_eq!(
            store.sync_cursor(a).unwrap(),
            Some(SyncCursor("v2".to_owned()))
        );
    }
}

#[cfg(test)]
mod sync_support_tests {
    use quill_core::types::{FolderRole, Headers, RemoteFolder};

    use super::*;

    fn store_with(folders: &[&str], messages: &[(&str, &str, i64)]) -> (Store, LocalAccountId) {
        let mut store = Store::open_in_memory().unwrap();
        let account = fill(&mut store, folders, messages);
        (store, account)
    }

    fn fill(store: &mut Store, folders: &[&str], messages: &[(&str, &str, i64)]) -> LocalAccountId {
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        for folder in folders {
            let id = store
                .upsert_folder(
                    account,
                    &RemoteFolder {
                        id: FolderId((*folder).to_owned()),
                        name: (*folder).to_owned(),
                        role: FolderRole::Other,
                    },
                )
                .unwrap();
            let added = messages
                .iter()
                .filter(|(in_folder, _, _)| in_folder == folder)
                .map(|(_, remote, date)| RemoteMessage {
                    id: RemoteId((*remote).to_owned()),
                    headers: Headers {
                        message_id: None,
                        subject: (*remote).to_owned(),
                        from: Vec::new(),
                        to: Vec::new(),
                        cc: Vec::new(),
                        date: *date,
                    },
                    flags: Flags::default(),
                    size: 1,
                    has_attachments: false,
                    thread: Vec::new(),
                })
                .collect();
            store
                .apply_delta(
                    id,
                    &SyncDelta {
                        reset: false,
                        added,
                        changed: Vec::new(),
                        removed: Vec::new(),
                        has_more: false,
                        next_cursor: SyncCursor("c".to_owned()),
                    },
                )
                .unwrap();
        }
        account
    }

    fn local(store: &Store, folder: &str, remote: &str) -> LocalMessageId {
        let folder = store
            .folders()
            .unwrap()
            .into_iter()
            .find(|row| row.name == folder)
            .unwrap();
        store
            .local_message_id(folder.id, &RemoteId(remote.to_owned()))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_message_knows_where_it_lives_remotely() {
        let (mut store, account) = store_with(&["INBOX"], &[("INBOX", "7;1;INBOX", 50)]);
        let id = local(&store, "INBOX", "7;1;INBOX");
        let wanted = Flags {
            seen: true,
            flagged: false,
            answered: true,
            draft: false,
        };
        assert!(store.set_flags(id, wanted).unwrap());
        let location = store.message_location(id).unwrap().unwrap();
        assert_eq!(location.account, account);
        assert_eq!(location.remote_folder, FolderId("INBOX".to_owned()));
        assert_eq!(location.remote, RemoteId("7;1;INBOX".to_owned()));
        assert_eq!((location.flags, location.date), (wanted, 50));
        assert_eq!(store.message_location(LocalMessageId(999)).unwrap(), None);
        assert!(!store.set_flags(LocalMessageId(999), wanted).unwrap());
    }

    #[test]
    fn a_download_records_text_and_blob_hash_together() {
        let (mut store, _) = store_with(&["INBOX"], &[("INBOX", "1", 50)]);
        let id = local(&store, "INBOX", "1");
        assert_eq!(store.blob_hash(id).unwrap(), None);
        store.put_downloaded(id, "the text", "abc123").unwrap();
        assert_eq!(store.body(id).unwrap().as_deref(), Some("the text"));
        assert_eq!(store.blob_hash(id).unwrap().as_deref(), Some("abc123"));
        assert_eq!(store.blob_hash(LocalMessageId(999)).unwrap(), None);
    }

    #[test]
    fn a_whole_message_is_saved_and_read_back_raw() {
        let (mut store, _) = store_with(&["INBOX"], &[("INBOX", "1", 50), ("INBOX", "2", 60)]);
        let id = local(&store, "INBOX", "1");
        let raw = b"Subject: hello\r\n\r\nthe text\r\n";
        assert_eq!(store.raw(id).unwrap(), None);
        store.put_message(id, "the text", raw).unwrap();
        assert_eq!(store.body(id).unwrap().as_deref(), Some("the text"));
        assert_eq!(store.blob_hash(id).unwrap(), Some(BlobStore::hash(raw)));
        assert_eq!(store.raw(id).unwrap().as_deref(), Some(&raw[..]));
        // A message with text but no raw copy has none to give.
        let other = local(&store, "INBOX", "2");
        store.put_body(other, "only text").unwrap();
        assert_eq!(store.raw(other).unwrap(), None);
    }

    #[test]
    fn a_store_without_blobs_keeps_the_text_and_has_no_raw_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let mut store = Store::open(&path).unwrap();
        fill(&mut store, &["INBOX"], &[("INBOX", "1", 50)]);
        let id = local(&store, "INBOX", "1");
        store.put_message(id, "the text", b"raw").unwrap();
        assert_eq!(store.body(id).unwrap().as_deref(), Some("the text"));
        assert_eq!(store.raw(id).unwrap(), None);

        // Another connection, told where the engine keeps raw messages,
        // reads what the engine saved there.
        let blobs = BlobStore::new(dir.path().join("blobs"));
        let hash = blobs.put(b"raw bytes").unwrap();
        store.put_downloaded(id, "the text", &hash).unwrap();
        let reader = Store::open(&path)
            .unwrap()
            .with_blobs(BlobStore::new(dir.path().join("blobs")));
        assert_eq!(reader.raw(id).unwrap().as_deref(), Some(&b"raw bytes"[..]));
    }

    #[test]
    fn messages_without_bodies_are_found_newest_first_within_the_window() {
        let (mut store, account) = store_with(
            &["INBOX", "Archive"],
            &[
                ("INBOX", "old", 10),
                ("INBOX", "recent", 100),
                ("Archive", "newest", 200),
                ("INBOX", "has-body", 150),
            ],
        );
        let with_body = local(&store, "INBOX", "has-body");
        store.put_body(with_body, "text").unwrap();
        let found = store.messages_without_body(account, 50, 10).unwrap();
        assert_eq!(
            found,
            [
                local(&store, "Archive", "newest"),
                local(&store, "INBOX", "recent")
            ]
        );
        assert_eq!(
            store.messages_without_body(account, 50, 1).unwrap().len(),
            1
        );
        assert!(
            store
                .messages_without_body(LocalAccountId(99), 0, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn folders_gone_from_the_server_are_removed_with_their_mail() {
        let (mut store, account) = store_with(
            &["INBOX", "Old Project"],
            &[("INBOX", "1", 1), ("Old Project", "2", 2)],
        );
        let removed = store
            .retain_folders(account, &[FolderId("INBOX".to_owned())])
            .unwrap();
        assert_eq!(removed, 1);
        let names: Vec<String> = store
            .folders()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["INBOX"]);
        let left: i64 = store
            .conn
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn a_version_one_database_is_migrated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            migrations::apply(&mut conn, &migrations::MIGRATIONS[..1]).unwrap();
            conn.execute(
                "INSERT INTO accounts (name, address, kind) VALUES ('Work', 'r@example.com', 'demo')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            migrations::latest_version()
        );
        assert_eq!(store.accounts().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod change_detection_tests {
    use quill_core::types::{Address, FlagChange, FolderRole, Headers, RemoteFolder};

    use super::*;

    fn inbox(store: &mut Store, kind: &str) -> LocalFolderId {
        let account = store
            .upsert_account("Work", "robin@example.com", kind)
            .unwrap();
        store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId("INBOX".to_owned()),
                    name: "INBOX".to_owned(),
                    role: FolderRole::Inbox,
                },
            )
            .unwrap()
    }

    fn from_ana(flags: Flags) -> RemoteMessage {
        RemoteMessage {
            id: RemoteId("1".to_owned()),
            headers: Headers {
                message_id: None,
                subject: "hello".to_owned(),
                from: vec![Address {
                    name: Some("Ana Ruiz".to_owned()),
                    email: "ana@example.com".to_owned(),
                }],
                to: Vec::new(),
                cc: Vec::new(),
                date: 5,
            },
            flags,
            size: 1,
            has_attachments: false,
            thread: Vec::new(),
        }
    }

    fn report(added: Vec<RemoteMessage>, changed: Vec<FlagChange>) -> SyncDelta {
        SyncDelta {
            reset: false,
            added,
            changed,
            removed: Vec::new(),
            has_more: false,
            next_cursor: SyncCursor("c".to_owned()),
        }
    }

    fn answered(store: &Store, folder: LocalFolderId) -> bool {
        store.messages(Mailbox::Folder(folder)).unwrap()[0]
            .flags
            .answered
    }

    #[test]
    fn a_graph_account_keeps_the_answered_mark_made_here() {
        let mut store = Store::open_in_memory().unwrap();
        let folder = inbox(&mut store, "graph");
        store
            .apply_delta(
                folder,
                &report(vec![from_ana(Flags::default())], Vec::new()),
            )
            .unwrap();
        let id = store
            .local_message_id(folder, &RemoteId("1".to_owned()))
            .unwrap()
            .unwrap();
        let marked = Flags {
            answered: true,
            seen: true,
            ..Flags::default()
        };
        store.queue_flags(id, marked, 10).unwrap();
        let account = store.accounts().unwrap()[0].id;
        let operation = store.claim_operation(account, 10).unwrap().unwrap();
        store.complete_operation(operation.id, None).unwrap();
        assert!(answered(&store, folder));

        // The service reports the message again, read, and as ever says
        // nothing of answered. Then a change of marks alone.
        let read = Flags {
            seen: true,
            ..Flags::default()
        };
        assert!(
            !store
                .apply_delta(folder, &report(vec![from_ana(read)], Vec::new()))
                .unwrap()
        );
        assert!(answered(&store, folder));
        store
            .apply_delta(
                folder,
                &report(
                    Vec::new(),
                    vec![FlagChange {
                        id: RemoteId("1".to_owned()),
                        flags: Flags::default(),
                    }],
                ),
            )
            .unwrap();
        assert!(answered(&store, folder));
        assert!(
            !store.messages(Mailbox::Folder(folder)).unwrap()[0]
                .flags
                .seen
        );
    }

    #[test]
    fn an_imap_server_has_the_last_word_on_answered() {
        let mut store = Store::open_in_memory().unwrap();
        let folder = inbox(&mut store, "imap");
        let marked = Flags {
            answered: true,
            ..Flags::default()
        };
        store
            .apply_delta(folder, &report(vec![from_ana(marked)], Vec::new()))
            .unwrap();
        assert!(answered(&store, folder));
        store
            .apply_delta(
                folder,
                &report(vec![from_ana(Flags::default())], Vec::new()),
            )
            .unwrap();
        assert!(!answered(&store, folder));
    }

    #[test]
    fn a_message_reported_again_does_not_count_its_people_again() {
        let mut store = Store::open_in_memory().unwrap();
        let folder = inbox(&mut store, "graph");
        let count = |store: &Store| -> i64 {
            store
                .conn
                .query_row(
                    "SELECT weight FROM contacts WHERE email = 'ana@example.com'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        store
            .apply_delta(
                folder,
                &report(vec![from_ana(Flags::default())], Vec::new()),
            )
            .unwrap();
        let once = count(&store);
        let read = Flags {
            seen: true,
            ..Flags::default()
        };
        store
            .apply_delta(folder, &report(vec![from_ana(read)], Vec::new()))
            .unwrap();
        assert_eq!(count(&store), once);
    }

    #[test]
    fn apply_delta_reports_whether_anything_really_changed() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let folder = store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId("INBOX".to_owned()),
                    name: "INBOX".to_owned(),
                    role: FolderRole::Inbox,
                },
            )
            .unwrap();
        let message = RemoteMessage {
            id: RemoteId("1".to_owned()),
            headers: Headers {
                message_id: None,
                subject: "hello".to_owned(),
                from: Vec::new(),
                to: Vec::new(),
                cc: Vec::new(),
                date: 5,
            },
            flags: Flags::default(),
            size: 1,
            has_attachments: false,
            thread: Vec::new(),
        };
        let delta =
            |added: Vec<RemoteMessage>, changed: Vec<FlagChange>, removed: Vec<RemoteId>| {
                SyncDelta {
                    reset: false,
                    added,
                    changed,
                    removed,
                    has_more: false,
                    next_cursor: SyncCursor("c".to_owned()),
                }
            };
        let flags = |seen| FlagChange {
            id: RemoteId("1".to_owned()),
            flags: Flags {
                seen,
                ..Flags::default()
            },
        };

        assert!(
            store
                .apply_delta(folder, &delta(vec![message.clone()], vec![], vec![]))
                .unwrap()
        );
        // The same message again, and its flags as they already are: nothing.
        assert!(
            !store
                .apply_delta(folder, &delta(vec![message], vec![], vec![]))
                .unwrap()
        );
        assert!(
            !store
                .apply_delta(folder, &delta(vec![], vec![flags(false)], vec![]))
                .unwrap()
        );
        assert!(
            !store
                .apply_delta(folder, &delta(vec![], vec![], vec![]))
                .unwrap()
        );
        // A real flag change, then a removal.
        assert!(
            store
                .apply_delta(folder, &delta(vec![], vec![flags(true)], vec![]))
                .unwrap()
        );
        assert!(
            store
                .apply_delta(
                    folder,
                    &delta(vec![], vec![], vec![RemoteId("1".to_owned())])
                )
                .unwrap()
        );
        // Removing what is already gone is nothing too.
        assert!(
            !store
                .apply_delta(
                    folder,
                    &delta(vec![], vec![], vec![RemoteId("1".to_owned())])
                )
                .unwrap()
        );
    }
}

#[cfg(test)]
mod retain_accounts_tests {
    use super::*;

    #[test]
    fn one_account_is_removed_with_everything_of_its_own_and_the_rest_is_left() {
        let mut store = Store::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for name in ["Work", "Personal"] {
            let account = store.upsert_account(name, "r@example.com", "imap").unwrap();
            store
                .upsert_folder(
                    account,
                    &RemoteFolder {
                        id: FolderId("INBOX".to_owned()),
                        name: "INBOX".to_owned(),
                        role: FolderRole::Inbox,
                    },
                )
                .unwrap();
            ids.push(account);
        }
        let (work, personal) = (ids[0], ids[1]);
        // Work has a message being written, one on its way out, and a
        // change its server has not heard of. Personal has a draft.
        let content = DraftContent::default();
        store.save_draft(None, work, &content, 1).unwrap().unwrap();
        let going = store.save_draft(None, work, &content, 1).unwrap().unwrap();
        assert!(
            store
                .queue_send(
                    going,
                    "hash",
                    "r@example.com",
                    &["a@example.com".to_owned()],
                    5,
                    1
                )
                .unwrap()
        );
        store
            .conn
            .execute(
                "INSERT INTO operations (account_id, kind, not_before, created) \
                 VALUES (?1, 'flags', 0, 0)",
                [work.0],
            )
            .unwrap();
        store.save_draft(None, personal, &content, 1).unwrap();
        assert_eq!(
            store.account_leftovers(work).unwrap(),
            Leftovers {
                drafts: 1,
                outbox: 1,
                queued: 1,
                local: 0,
            }
        );
        assert_eq!(
            store.account_leftovers(personal).unwrap(),
            Leftovers {
                drafts: 1,
                outbox: 0,
                queued: 0,
                local: 0,
            }
        );

        assert!(store.remove_account(work).unwrap());
        // It is gone, with its folders, its drafts, its outbox and its
        // queue; the other account has all it had.
        let names: Vec<String> = store
            .accounts()
            .unwrap()
            .into_iter()
            .map(|account| account.name)
            .collect();
        assert_eq!(names, ["Personal"]);
        assert_eq!(store.account_leftovers(work).unwrap(), Leftovers::default());
        assert_eq!(store.queued_operations().unwrap(), 0);
        assert_eq!(store.account_leftovers(personal).unwrap().drafts, 1);
        assert_eq!(store.folders().unwrap().len(), 1);
        // Removing what is not there is nothing, and says so.
        assert!(!store.remove_account(work).unwrap());
    }

    #[test]
    fn accounts_taken_out_of_the_config_are_removed_with_their_mail() {
        let mut store = Store::open_in_memory().unwrap();
        for name in ["Work", "Personal", "Old"] {
            let account = store.upsert_account(name, "r@example.com", "imap").unwrap();
            store
                .upsert_folder(
                    account,
                    &RemoteFolder {
                        id: FolderId("INBOX".to_owned()),
                        name: "INBOX".to_owned(),
                        role: FolderRole::Inbox,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            store
                .retain_accounts(&["Work", "Personal", "Never Synced"])
                .unwrap(),
            1
        );
        let names: Vec<String> = store
            .accounts()
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["Work", "Personal"]);
        assert_eq!(store.folders().unwrap().len(), 2);
        assert_eq!(store.retain_accounts(&["Work", "Personal"]).unwrap(), 0);
        assert_eq!(store.retain_accounts(&[]).unwrap(), 2);
    }
}

#[cfg(test)]
mod busy_tests {
    use super::*;

    #[test]
    fn a_database_held_by_another_connection_is_busy_not_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let mut first = Store::open(&path).unwrap();
        let second = Store::open(&path).unwrap();
        // Do not wait ten seconds to find out.
        first.conn.busy_timeout(std::time::Duration::ZERO).unwrap();

        // The second connection holds the write lock...
        second.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let err = first
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap_err();
        assert!(err.is_busy(), "{err:?}");
        // ...and once it lets go, the same call works.
        second.conn.execute_batch("COMMIT").unwrap();
        first
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();

        let other = StoreError::NewerSchema {
            found: 9,
            supported: 2,
        };
        assert!(!other.is_busy());
    }
}
