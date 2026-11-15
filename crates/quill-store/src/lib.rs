//! The local SQLite store: the single source of truth for the UI.
//!
//! All calls block on disk. Run them on the store worker, never on the UI task.

mod migrations;

use std::path::Path;

use quill_core::types::{
    Address, Flags, FolderRole, Headers, LocalAccountId, LocalFolderId, LocalMessageId,
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
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// An account as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub id: LocalAccountId,
    pub name: String,
    pub address: String,
    pub kind: String,
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
}

/// Which messages a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mailbox {
    /// Every account's inbox, merged.
    Unified,
    Folder(LocalFolderId),
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` in WAL mode and
    /// migrates it to the current schema.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// A private, empty database that lives as long as the `Store`.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // In-memory databases report "memory" here; files report "wal".
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations::run(&mut conn)?;
        Ok(Self { conn })
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
    /// same delta twice leaves the store unchanged.
    pub fn apply_delta(&mut self, folder: LocalFolderId, delta: &SyncDelta) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO messages (folder_id, remote_id, message_id, subject, from_addrs,
                     to_addrs, cc_addrs, date, size, seen, flagged, answered, draft,
                     has_attachments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT (folder_id, remote_id) DO UPDATE SET
                     message_id = excluded.message_id, subject = excluded.subject,
                     from_addrs = excluded.from_addrs, to_addrs = excluded.to_addrs,
                     cc_addrs = excluded.cc_addrs, date = excluded.date, size = excluded.size,
                     seen = excluded.seen, flagged = excluded.flagged,
                     answered = excluded.answered, draft = excluded.draft,
                     has_attachments = excluded.has_attachments",
            )?;
            for message in &delta.added {
                insert_message(&mut insert, folder, message)?;
            }
            let mut update = tx.prepare(
                "UPDATE messages SET seen = ?3, flagged = ?4, answered = ?5, draft = ?6
                 WHERE folder_id = ?1 AND remote_id = ?2",
            )?;
            for change in &delta.changed {
                let flags = change.flags;
                update.execute(params![
                    folder.0,
                    change.id.0,
                    flags.seen,
                    flags.flagged,
                    flags.answered,
                    flags.draft
                ])?;
            }
            let mut delete =
                tx.prepare("DELETE FROM messages WHERE folder_id = ?1 AND remote_id = ?2")?;
            for id in &delta.removed {
                delete.execute(params![folder.0, id.0])?;
            }
            tx.execute(
                "INSERT INTO sync_state (folder_id, cursor) VALUES (?1, ?2)
                 ON CONFLICT (folder_id) DO UPDATE SET cursor = excluded.cursor",
                params![folder.0, delta.next_cursor.0],
            )?;
        }
        tx.commit()?;
        Ok(())
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
            .prepare("SELECT id, name, address, kind FROM accounts ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(AccountRow {
                id: LocalAccountId(row.get(0)?),
                name: row.get(1)?,
                address: row.get(2)?,
                kind: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every folder with its counts, grouped by account and ordered by role:
    /// Inbox, Drafts, Sent, Archive, Junk, Trash, then the rest by name.
    pub fn folders(&self) -> Result<Vec<FolderRow>> {
        let mut statement = self.conn.prepare(
            "SELECT f.id, f.account_id, f.name, f.role,
                    (SELECT count(*) FROM messages m WHERE m.folder_id = f.id AND m.seen = 0),
                    (SELECT count(*) FROM messages m WHERE m.folder_id = f.id)
             FROM folders f
             ORDER BY f.account_id,
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
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The messages of a mailbox, newest first.
    pub fn messages(&self, mailbox: Mailbox) -> Result<Vec<MessageRow>> {
        const COLUMNS: &str = "m.id, m.folder_id, f.account_id, m.message_id, m.subject,
             m.from_addrs, m.to_addrs, m.cc_addrs, m.date, m.seen, m.flagged, m.answered,
             m.draft, m.has_attachments";
        const ORDER: &str = "ORDER BY m.date DESC, m.id DESC";
        let rows = match mailbox {
            Mailbox::Unified => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE f.role = 'inbox' {ORDER}"
                ))?;
                statement
                    .query_map([], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            Mailbox::Folder(folder) => {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM messages m JOIN folders f ON f.id = m.folder_id
                     WHERE m.folder_id = ?1 {ORDER}"
                ))?;
                statement
                    .query_map([folder.0], message_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// Marks a message read or unread. Returns whether the message exists.
    pub fn set_seen(&mut self, message: LocalMessageId, seen: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET seen = ?2 WHERE id = ?1",
            params![message.0, seen],
        )?;
        Ok(changed > 0)
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
) -> Result<()> {
    let headers = &message.headers;
    let flags = message.flags;
    insert.execute(params![
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
    ])?;
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
        }
    }

    fn delta(added: Vec<RemoteMessage>, cursor: &str) -> SyncDelta {
        SyncDelta {
            added,
            changed: Vec::new(),
            removed: Vec::new(),
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
