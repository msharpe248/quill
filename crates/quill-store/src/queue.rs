//! The outbound queue: changes to mail on a server, made locally first.
//!
//! Moving, deleting and flagging a message all happen in the store at once,
//! so the UI never waits and all of it works with no connection. Each also
//! leaves a row here for the sync engine to replay against the server, in
//! the order made. If the server refuses, the local change is taken back.
//!
//! A message that has been moved keeps its row, with its body: the row
//! changes folder and takes a placeholder for a server id until the server
//! says what the message is called in its new folder.

use quill_core::types::{Flags, FolderId, LocalAccountId, LocalFolderId, LocalMessageId, RemoteId};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Result, Store};

/// The id of a queued operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId(pub i64);

/// What a queued operation asks of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedKind {
    /// Move `remote` out of `source` into `target`.
    Move {
        remote: RemoteId,
        source: FolderId,
        target: FolderId,
        /// For re-syncing the two folders afterwards.
        source_local: LocalFolderId,
        target_local: LocalFolderId,
    },
    /// Remove `remote` from `source` for good.
    Delete { remote: RemoteId, source: FolderId },
    /// Set the message's flags.
    Flags { remote: RemoteId, flags: Flags },
    /// Save the raw message filed under `blob_hash` into `target`.
    Append {
        target: FolderId,
        target_local: LocalFolderId,
        blob_hash: String,
        flags: Flags,
    },
    /// The operation has nothing left to do: its message or folder has gone,
    /// or it waited on an earlier operation that could not say where the
    /// message went. It is finished by completing it.
    Nothing,
}

/// An operation the engine has taken to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedOperation {
    pub id: OperationId,
    pub account: LocalAccountId,
    pub kind: QueuedKind,
    /// How many times it has been tried before.
    pub attempts: u32,
}

/// The prefix of the server id a moved message carries until the server
/// says what it is called in its new folder. No server issues such an id.
const PLACEHOLDER: &str = "local:op";

fn placeholder(operation: i64) -> String {
    format!("{PLACEHOLDER}{operation}")
}

/// Whether a server id is a placeholder: the message is on its way
/// somewhere and the server has not been told yet.
pub fn is_placeholder(remote: &str) -> bool {
    remote.starts_with(PLACEHOLDER)
}

fn encode_flags(flags: Flags) -> String {
    [flags.seen, flags.flagged, flags.answered, flags.draft]
        .iter()
        .map(|on| if *on { '1' } else { '0' })
        .collect()
}

fn decode_flags(text: &str) -> Flags {
    let mut bits = text.chars().map(|c| c == '1');
    let mut next = || bits.next().unwrap_or(false);
    Flags {
        seen: next(),
        flagged: next(),
        answered: next(),
        draft: next(),
    }
}

impl Store {
    /// Moves a message to another folder of its account, here and now, and
    /// queues the move for the server. The server is not told before
    /// `not_before`: until then the move can be undone.
    ///
    /// Returns `None`, changing nothing, when the message is missing, is
    /// already in that folder, or the folder is another account's.
    pub fn queue_move(
        &mut self,
        message: LocalMessageId,
        target: LocalFolderId,
        now: i64,
        not_before: i64,
    ) -> Result<Option<OperationId>> {
        let tx = self.conn.transaction()?;
        let found: Option<(i64, i64, String)> = tx
            .query_row(
                "SELECT f.account_id, m.folder_id, m.remote_id
                 FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.id = ?1 AND m.hidden = 0",
                [message.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((account, source, remote)) = found else {
            return Ok(None);
        };
        let target_account: Option<i64> = tx
            .query_row(
                "SELECT account_id FROM folders WHERE id = ?1",
                [target.0],
                |row| row.get(0),
            )
            .optional()?;
        if source == target.0 || target_account != Some(account) {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO operations (account_id, kind, message_id, remote_id, source_folder,
                 target_folder, not_before, created)
             VALUES (?1, 'move', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                account, message.0, remote, source, target.0, not_before, now
            ],
        )?;
        let operation = tx.last_insert_rowid();
        tx.execute(
            "UPDATE messages SET folder_id = ?2, remote_id = ?3 WHERE id = ?1",
            params![message.0, target.0, placeholder(operation)],
        )?;
        tx.commit()?;
        Ok(Some(OperationId(operation)))
    }

    /// Hides a message, here and now, and queues its permanent removal from
    /// the server. Until `not_before` it can be undone.
    pub fn queue_delete(
        &mut self,
        message: LocalMessageId,
        now: i64,
        not_before: i64,
    ) -> Result<Option<OperationId>> {
        let tx = self.conn.transaction()?;
        let found: Option<(i64, i64, String)> = tx
            .query_row(
                "SELECT f.account_id, m.folder_id, m.remote_id
                 FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.id = ?1 AND m.hidden = 0",
                [message.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((account, source, remote)) = found else {
            return Ok(None);
        };
        tx.execute(
            "INSERT INTO operations (account_id, kind, message_id, remote_id, source_folder,
                 not_before, created)
             VALUES (?1, 'delete', ?2, ?3, ?4, ?5, ?6)",
            params![account, message.0, remote, source, not_before, now],
        )?;
        let operation = tx.last_insert_rowid();
        tx.execute("UPDATE messages SET hidden = 1 WHERE id = ?1", [message.0])?;
        tx.commit()?;
        Ok(Some(OperationId(operation)))
    }

    /// Sets a message's flags, here and now, and queues the change for the
    /// server. A second change before the first has gone replaces it: the
    /// server is told the latest, and a refusal goes back to what the flags
    /// were before any of them. Returns whether anything changed.
    pub fn queue_flags(&mut self, message: LocalMessageId, flags: Flags, now: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let found: Option<(i64, bool, bool, bool, bool)> = tx
            .query_row(
                "SELECT f.account_id, m.seen, m.flagged, m.answered, m.draft
                 FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
                [message.0],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((account, seen, flagged, answered, draft)) = found else {
            return Ok(false);
        };
        let previous = Flags {
            seen,
            flagged,
            answered,
            draft,
        };
        if previous == flags {
            return Ok(false);
        }
        tx.execute(
            "UPDATE messages SET seen = ?2, flagged = ?3, answered = ?4, draft = ?5 WHERE id = ?1",
            params![
                message.0,
                flags.seen,
                flags.flagged,
                flags.answered,
                flags.draft
            ],
        )?;
        let replaced = tx.execute(
            "UPDATE operations SET flags = ?2
             WHERE kind = 'flags' AND state = 'pending' AND message_id = ?1",
            params![message.0, encode_flags(flags)],
        )?;
        if replaced == 0 {
            tx.execute(
                "INSERT INTO operations (account_id, kind, message_id, flags, previous,
                     not_before, created)
                 VALUES (?1, 'flags', ?2, ?3, ?4, ?5, ?5)",
                params![
                    account,
                    message.0,
                    encode_flags(flags),
                    encode_flags(previous),
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Queues saving the raw message filed under `blob_hash` into a folder
    /// on the server: a sent copy, or a draft.
    pub fn queue_append(
        &mut self,
        folder: LocalFolderId,
        blob_hash: &str,
        flags: Flags,
        now: i64,
    ) -> Result<Option<OperationId>> {
        let account: Option<i64> = self
            .conn
            .query_row(
                "SELECT account_id FROM folders WHERE id = ?1",
                [folder.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(account) = account else {
            return Ok(None);
        };
        self.conn.execute(
            "INSERT INTO operations (account_id, kind, target_folder, blob_hash, flags,
                 not_before, created)
             VALUES (?1, 'append', ?2, ?3, ?4, ?5, ?5)",
            params![account, folder.0, blob_hash, encode_flags(flags), now],
        )?;
        Ok(Some(OperationId(self.conn.last_insert_rowid())))
    }

    /// The newest change that can still be undone at `now`, if there is one.
    pub fn undoable(&self, now: i64) -> Result<Option<OperationId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM operations
                 WHERE state = 'pending' AND not_before > ?1 AND kind IN ('move', 'delete')
                 ORDER BY id DESC LIMIT 1",
                [now],
                |row| row.get(0),
            )
            .optional()?
            .map(OperationId))
    }

    /// Takes a change back: the message returns to where it was and the
    /// server never hears of it. Only a change the engine has not taken
    /// yet can be undone. Returns whether it was.
    pub fn undo_operation(&mut self, operation: OperationId) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let pending: Option<String> = tx
            .query_row(
                "SELECT state FROM operations WHERE id = ?1",
                [operation.0],
                |row| row.get(0),
            )
            .optional()?;
        if pending.as_deref() != Some("pending") {
            return Ok(false);
        }
        revert(&tx, operation.0)?;
        tx.commit()?;
        Ok(true)
    }

    /// When the account's next queued operation may run, if it has any.
    /// Operations run strictly in the order made, so this is the oldest
    /// one's time.
    pub fn next_operation_time(&self, account: LocalAccountId) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT not_before FROM operations WHERE account_id = ?1 ORDER BY id LIMIT 1",
                [account.0],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Takes the account's oldest operation to run, if it is due at `now`.
    /// An operation already taken (by a run that crashed) is given again:
    /// every operation is safe to repeat.
    pub fn claim_operation(
        &mut self,
        account: LocalAccountId,
        now: i64,
    ) -> Result<Option<QueuedOperation>> {
        let tx = self.conn.transaction()?;
        struct Raw {
            id: i64,
            kind: String,
            message: Option<i64>,
            remote: Option<String>,
            source: Option<i64>,
            target: Option<i64>,
            blob_hash: Option<String>,
            flags: Option<String>,
            not_before: i64,
            attempts: u32,
        }
        let raw: Option<Raw> = tx
            .query_row(
                "SELECT id, kind, message_id, remote_id, source_folder, target_folder, blob_hash,
                        flags, not_before, attempts
                 FROM operations WHERE account_id = ?1 ORDER BY id LIMIT 1",
                [account.0],
                |row| {
                    Ok(Raw {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        message: row.get(2)?,
                        remote: row.get(3)?,
                        source: row.get(4)?,
                        target: row.get(5)?,
                        blob_hash: row.get(6)?,
                        flags: row.get(7)?,
                        not_before: row.get(8)?,
                        attempts: row.get(9)?,
                    })
                },
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        if raw.not_before > now {
            return Ok(None);
        }
        let folder = |id: Option<i64>| -> Result<Option<(LocalFolderId, FolderId)>> {
            let Some(id) = id else { return Ok(None) };
            Ok(tx
                .query_row("SELECT remote_id FROM folders WHERE id = ?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .map(|remote| (LocalFolderId(id), FolderId(remote))))
        };
        // A server id that is still a placeholder belongs to a message whose
        // earlier move never learned where the message went. There is
        // nothing to tell the server about it.
        let remote = raw
            .remote
            .filter(|remote| !is_placeholder(remote))
            .map(RemoteId);
        let kind = match raw.kind.as_str() {
            "move" => match (remote, folder(raw.source)?, folder(raw.target)?) {
                (Some(remote), Some(source), Some(target)) => QueuedKind::Move {
                    remote,
                    source: source.1,
                    target: target.1,
                    source_local: source.0,
                    target_local: target.0,
                },
                _ => QueuedKind::Nothing,
            },
            "delete" => match (remote, folder(raw.source)?) {
                (Some(remote), Some(source)) => QueuedKind::Delete {
                    remote,
                    source: source.1,
                },
                _ => QueuedKind::Nothing,
            },
            "flags" => {
                // The message's id is read now, not when the change was
                // made: a move that ran in between has renamed it.
                let current: Option<String> = match raw.message {
                    Some(message) => tx
                        .query_row(
                            "SELECT remote_id FROM messages WHERE id = ?1",
                            [message],
                            |row| row.get(0),
                        )
                        .optional()?,
                    None => None,
                };
                match (current, raw.flags) {
                    (Some(remote), Some(flags)) if !is_placeholder(&remote) => QueuedKind::Flags {
                        remote: RemoteId(remote),
                        flags: decode_flags(&flags),
                    },
                    _ => QueuedKind::Nothing,
                }
            }
            "append" => match (folder(raw.target)?, raw.blob_hash) {
                (Some(target), Some(blob_hash)) => QueuedKind::Append {
                    target: target.1,
                    target_local: target.0,
                    blob_hash,
                    flags: raw.flags.as_deref().map(decode_flags).unwrap_or_default(),
                },
                _ => QueuedKind::Nothing,
            },
            _ => QueuedKind::Nothing,
        };
        tx.execute(
            "UPDATE operations SET state = 'running' WHERE id = ?1",
            [raw.id],
        )?;
        tx.commit()?;
        Ok(Some(QueuedOperation {
            id: OperationId(raw.id),
            account,
            kind,
            attempts: raw.attempts,
        }))
    }

    /// The server did what the operation asked. For a move or an append,
    /// `new_id` is what the server calls the message now, when it said.
    ///
    /// Without a new id, a moved message's local copy is dropped: the next
    /// sync of its folder brings it back under whatever the server calls
    /// it, where keeping the copy would show the message twice.
    pub fn complete_operation(
        &mut self,
        operation: OperationId,
        new_id: Option<&RemoteId>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let found: Option<(String, Option<i64>, Option<i64>)> = tx
            .query_row(
                "SELECT kind, message_id, target_folder FROM operations WHERE id = ?1",
                [operation.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((kind, message, target)) = found else {
            return Ok(());
        };
        let waiting = placeholder(operation.0);
        match kind.as_str() {
            "move" => match new_id {
                Some(new_id) => {
                    if let Some(target) = target {
                        // A sync of the target may have brought the message
                        // in already, under its new id. Ours has the body.
                        tx.execute(
                            "DELETE FROM messages
                             WHERE folder_id = ?1 AND remote_id = ?2 AND id IS NOT ?3",
                            params![target, new_id.0, message],
                        )?;
                    }
                    // The message may have moved on again since; then a
                    // later operation holds the placeholder instead.
                    tx.execute(
                        "UPDATE messages SET remote_id = ?2 WHERE remote_id = ?1",
                        params![waiting, new_id.0],
                    )?;
                    tx.execute(
                        "UPDATE operations SET remote_id = ?2 WHERE remote_id = ?1",
                        params![waiting, new_id.0],
                    )?;
                }
                None => {
                    tx.execute("DELETE FROM messages WHERE remote_id = ?1", [&waiting])?;
                }
            },
            "delete" => {
                if let Some(message) = message {
                    tx.execute(
                        "DELETE FROM messages WHERE id = ?1 AND hidden = 1",
                        [message],
                    )?;
                }
            }
            _ => {}
        }
        tx.execute("DELETE FROM operations WHERE id = ?1", [operation.0])?;
        tx.commit()?;
        Ok(())
    }

    /// The server refused the operation for good. The local change is taken
    /// back, and so is every later change that was built on it.
    pub fn fail_operation(&mut self, operation: OperationId) -> Result<()> {
        let tx = self.conn.transaction()?;
        revert(&tx, operation.0)?;
        tx.commit()?;
        Ok(())
    }

    /// The operation could not be tried or finished just now (no
    /// connection, a busy server). It stays first in line and is tried
    /// again at `not_before`.
    pub fn retry_operation(&mut self, operation: OperationId, not_before: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE operations SET state = 'pending', attempts = attempts + 1, not_before = ?2
             WHERE id = ?1",
            params![operation.0, not_before],
        )?;
        Ok(())
    }

    /// How many operations are waiting, for every account together.
    pub fn queued_operations(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }
}

/// Undoes an operation's local change and removes it from the queue, after
/// doing the same for any later operation that depends on it.
fn revert(tx: &Transaction<'_>, operation: i64) -> Result<()> {
    // A later move of the same message carries this move's placeholder as
    // the id it moved from. It goes first, putting the message back where
    // this operation left it.
    let waiting = placeholder(operation);
    let dependants: Vec<i64> = tx
        .prepare("SELECT id FROM operations WHERE remote_id = ?1 ORDER BY id DESC")?
        .query_map([&waiting], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for dependant in dependants {
        revert(tx, dependant)?;
    }

    type Found = (
        String,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    );
    let found: Option<Found> = tx
        .query_row(
            "SELECT kind, message_id, remote_id, source_folder, previous
             FROM operations WHERE id = ?1",
            [operation],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, message, remote, source, previous)) = found else {
        return Ok(());
    };
    match (kind.as_str(), message) {
        ("move", Some(message)) => {
            if let (Some(remote), Some(source)) = (remote, source) {
                // If a sync has meanwhile listed the message in its old
                // folder again, that row gives way to this one, which has
                // the body.
                tx.execute(
                    "DELETE FROM messages
                     WHERE folder_id = ?1 AND remote_id = ?2 AND id <> ?3",
                    params![source, remote, message],
                )?;
                tx.execute(
                    "UPDATE messages SET folder_id = ?2, remote_id = ?3 WHERE id = ?1",
                    params![message, source, remote],
                )?;
            }
        }
        ("delete", Some(message)) => {
            tx.execute("UPDATE messages SET hidden = 0 WHERE id = ?1", [message])?;
        }
        ("flags", Some(message)) => {
            if let Some(previous) = previous {
                let flags = decode_flags(&previous);
                tx.execute(
                    "UPDATE messages SET seen = ?2, flagged = ?3, answered = ?4, draft = ?5
                     WHERE id = ?1",
                    params![
                        message,
                        flags.seen,
                        flags.flagged,
                        flags.answered,
                        flags.draft
                    ],
                )?;
            }
        }
        _ => {}
    }
    tx.execute("DELETE FROM operations WHERE id = ?1", [operation])?;
    Ok(())
}

/// What a sync of `folder` must leave alone because a queued operation has
/// already changed it locally. Called inside the transaction that applies
/// the sync.
pub(crate) struct Pending {
    /// Server ids the server still lists in this folder but that have been
    /// moved out of it here.
    pub departed: Vec<String>,
}

impl Pending {
    pub(crate) fn of(tx: &Transaction<'_>, folder: LocalFolderId) -> Result<Self> {
        let departed = tx
            .prepare(
                "SELECT remote_id FROM operations
                 WHERE source_folder = ?1 AND kind = 'move' AND remote_id IS NOT NULL",
            )?
            .query_map([folder.0], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { departed })
    }
}

/// Puts back the flags that queued changes have set, after a sync has
/// written what the server still thinks they are.
pub(crate) fn reapply_flags(tx: &Transaction<'_>, folder: LocalFolderId) -> Result<()> {
    let waiting: Vec<(i64, String)> = tx
        .prepare(
            "SELECT o.message_id, o.flags FROM operations o
             JOIN messages m ON m.id = o.message_id
             WHERE o.kind = 'flags' AND m.folder_id = ?1 AND o.flags IS NOT NULL",
        )?
        .query_map([folder.0], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (message, flags) in waiting {
        let flags = decode_flags(&flags);
        tx.execute(
            "UPDATE messages SET seen = ?2, flagged = ?3, answered = ?4, draft = ?5 WHERE id = ?1",
            params![
                message,
                flags.seen,
                flags.flagged,
                flags.answered,
                flags.draft
            ],
        )?;
    }
    Ok(())
}
