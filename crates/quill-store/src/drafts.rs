//! Mail being written, and mail waiting to go.
//!
//! A draft is the fields as the user typed them. Sending turns it into a
//! finished message once, files that among the blobs, and puts the draft in
//! the outbox; the sync engine takes it from there. A message leaves the
//! outbox only when a server has accepted it.

use quill_core::types::{Address, Flags, Headers, LocalAccountId, LocalFolderId, LocalMessageId};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::{MessageRow, Result, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DraftId(pub i64);

/// A draft's fields, as typed. Address fields are kept as text, so a
/// half-typed address survives being saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftContent {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// The Message-ID being replied to, with its angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<DraftAttachment>,
    /// The local message being replied to, to mark it answered once the
    /// reply has gone.
    pub replied_to: Option<i64>,
}

/// A file attached to a draft. Its bytes are among the blobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftAttachment {
    pub name: String,
    pub content_type: String,
    pub blob_hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftState {
    /// Being written.
    Draft,
    /// Waiting to be sent.
    Outbox,
    /// With a server now.
    Sending,
    /// A server refused it. It is kept, with the reason, to correct.
    Failed,
}

impl DraftState {
    fn from_stored(text: &str) -> Self {
        match text {
            "outbox" => Self::Outbox,
            "sending" => Self::Sending,
            "failed" => Self::Failed,
            _ => Self::Draft,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub id: DraftId,
    pub account: LocalAccountId,
    pub state: DraftState,
    pub content: DraftContent,
    /// Outbox: when it will be sent.
    pub send_after: Option<i64>,
    /// Why the last attempt to send it failed.
    pub error: Option<String>,
    pub updated: i64,
}

impl DraftId {
    /// The id a draft goes by in a message list. Drafts are listed beside
    /// messages, and take the negative numbers no message has.
    pub fn as_message_id(self) -> LocalMessageId {
        LocalMessageId(-self.0)
    }

    /// The draft a list row stands for, if it stands for one.
    pub fn from_message_id(id: LocalMessageId) -> Option<Self> {
        (id.0 < 0).then_some(Self(-id.0))
    }
}

impl DraftRow {
    /// The draft as a row of a message list: who it is to, its subject,
    /// when it was last touched.
    pub fn as_message(&self) -> MessageRow {
        let people = |text: &str| -> Vec<Address> {
            text.split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| match part.rsplit_once('<') {
                    Some((name, email)) => Address {
                        name: Some(name.trim().trim_matches('"').to_owned())
                            .filter(|name| !name.is_empty()),
                        email: email.trim_end_matches('>').trim().to_owned(),
                    },
                    None => Address {
                        name: None,
                        email: part.to_owned(),
                    },
                })
                .collect()
        };
        MessageRow {
            id: self.id.as_message_id(),
            // A draft is in no folder of any server.
            folder: LocalFolderId(0),
            account: self.account,
            headers: Headers {
                message_id: None,
                subject: self.content.subject.clone(),
                from: Vec::new(),
                to: people(&self.content.to),
                cc: people(&self.content.cc),
                date: self.send_after.unwrap_or(self.updated),
            },
            // A conversation of its own: no stored message's thread is
            // this far below zero.
            thread: i64::MIN / 2 + self.id.as_message_id().0.rem_euclid(i64::MAX / 4),
            flags: Flags {
                seen: true,
                draft: true,
                ..Flags::default()
            },
            has_attachments: !self.content.attachments.is_empty(),
        }
    }
}

/// A message the engine has taken from the outbox to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub id: DraftId,
    pub account: LocalAccountId,
    /// The finished message, by content hash.
    pub raw_hash: String,
    pub from: String,
    pub to: Vec<String>,
    /// The local message this replies to, to mark answered.
    pub replied_to: Option<i64>,
    pub attempts: u32,
}

#[derive(Serialize, Deserialize)]
struct StoredEnvelope {
    from: String,
    to: Vec<String>,
}

const COLUMNS: &str = "id, account_id, state, content, send_after, error, updated";

fn draft_row(row: &Row<'_>) -> rusqlite::Result<DraftRow> {
    let state: String = row.get(2)?;
    let content: String = row.get(3)?;
    Ok(DraftRow {
        id: DraftId(row.get(0)?),
        account: LocalAccountId(row.get(1)?),
        state: DraftState::from_stored(&state),
        // A draft written by a later Quill may hold fields this one does
        // not know: what can be read is kept and the rest is left out.
        content: serde_json::from_str(&content).unwrap_or_default(),
        send_after: row.get(4)?,
        error: row.get(5)?,
        updated: row.get(6)?,
    })
}

impl Store {
    /// Saves a draft: a new one when `id` is `None`, otherwise that one. A
    /// message that is on its way (in the outbox or being sent) is not
    /// changed; `None` comes back for it, and for a draft that is gone.
    pub fn save_draft(
        &mut self,
        id: Option<DraftId>,
        account: LocalAccountId,
        content: &DraftContent,
        now: i64,
    ) -> Result<Option<DraftId>> {
        let json = serde_json::to_string(content)?;
        match id {
            None => {
                self.conn.execute(
                    "INSERT INTO drafts (account_id, content, updated) VALUES (?1, ?2, ?3)",
                    params![account.0, json, now],
                )?;
                Ok(Some(DraftId(self.conn.last_insert_rowid())))
            }
            Some(id) => {
                // Saving a refused message makes it a draft again: it has
                // been corrected, or at least looked at.
                let changed = self.conn.execute(
                    "UPDATE drafts SET account_id = ?2, content = ?3, updated = ?4,
                         state = 'draft', error = NULL, raw_hash = NULL, envelope = NULL,
                         send_after = NULL
                     WHERE id = ?1 AND state IN ('draft', 'failed')",
                    params![id.0, account.0, json, now],
                )?;
                Ok((changed > 0).then_some(id))
            }
        }
    }

    pub fn draft(&self, id: DraftId) -> Result<Option<DraftRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM drafts WHERE id = ?1"),
                [id.0],
                draft_row,
            )
            .optional()?)
    }

    /// Drafts being written, newest first.
    pub fn drafts(&self) -> Result<Vec<DraftRow>> {
        self.drafts_where("state = 'draft'")
    }

    /// Mail waiting to go, being sent, or refused: oldest first, the order
    /// it will go in.
    pub fn outbox(&self) -> Result<Vec<DraftRow>> {
        self.drafts_where("state <> 'draft'")
    }

    fn drafts_where(&self, condition: &str) -> Result<Vec<DraftRow>> {
        let order = if condition.contains("<>") {
            "id"
        } else {
            "updated DESC, id DESC"
        };
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM drafts WHERE {condition} ORDER BY {order}"
        ))?;
        let rows = statement
            .query_map([], draft_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Throws a draft away. Mail a server has right now is not thrown away
    /// from under it. Returns whether the draft was removed.
    pub fn delete_draft(&mut self, id: DraftId) -> Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM drafts WHERE id = ?1 AND state <> 'sending'",
            [id.0],
        )?;
        Ok(removed > 0)
    }

    /// Puts a draft in the outbox, to be sent at `send_after`. `raw_hash`
    /// is the finished message among the blobs; `from` and `to` are who it
    /// goes from and to on the wire.
    pub fn queue_send(
        &mut self,
        id: DraftId,
        raw_hash: &str,
        from: &str,
        to: &[String],
        send_after: i64,
        now: i64,
    ) -> Result<bool> {
        let envelope = serde_json::to_string(&StoredEnvelope {
            from: from.to_owned(),
            to: to.to_vec(),
        })?;
        let changed = self.conn.execute(
            "UPDATE drafts SET state = 'outbox', raw_hash = ?2, envelope = ?3, send_after = ?4,
                 error = NULL, attempts = 0, updated = ?5
             WHERE id = ?1 AND state IN ('draft', 'failed')",
            params![id.0, raw_hash, envelope, send_after, now],
        )?;
        Ok(changed > 0)
    }

    /// Takes a message back out of the outbox before it has gone: undo
    /// send. Returns whether it was still there to take.
    pub fn unsend(&mut self, id: DraftId, now: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE drafts SET state = 'draft', raw_hash = NULL, envelope = NULL,
                 send_after = NULL, updated = ?2
             WHERE id = ?1 AND state = 'outbox'",
            params![id.0, now],
        )?;
        Ok(changed > 0)
    }

    /// The newest message that is still waiting in the outbox at `now` and
    /// so can be taken back.
    pub fn unsendable(&self, now: i64) -> Result<Option<DraftId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM drafts WHERE state = 'outbox' AND send_after > ?1
                 ORDER BY id DESC LIMIT 1",
                [now],
                |row| row.get(0),
            )
            .optional()?
            .map(DraftId))
    }

    /// Makes the account's mail that is waiting to be tried again due now:
    /// what stopped it has been dealt with. Returns how many were waiting.
    pub fn hurry_sends(&mut self, account: LocalAccountId, now: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE drafts SET send_after = ?2
             WHERE account_id = ?1 AND state = 'outbox' AND error IS NOT NULL
               AND send_after > ?2",
            params![account.0, now],
        )?)
    }

    /// When the account's next message is due to go, if any is waiting.
    pub fn next_send_time(&self, account: LocalAccountId) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT min(send_after) FROM drafts WHERE account_id = ?1 AND state = 'outbox'",
                [account.0],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Takes the account's oldest due message to send.
    pub fn claim_send(&mut self, account: LocalAccountId, now: i64) -> Result<Option<Outgoing>> {
        let tx = self.conn.transaction()?;
        type Found = (i64, Option<String>, Option<String>, String, u32);
        let found: Option<Found> = tx
            .query_row(
                "SELECT id, raw_hash, envelope, content, attempts FROM drafts
                 WHERE account_id = ?1 AND state = 'outbox' AND send_after <= ?2
                 ORDER BY id LIMIT 1",
                params![account.0, now],
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
        let Some((id, raw_hash, envelope, content, attempts)) = found else {
            return Ok(None);
        };
        let envelope: Option<StoredEnvelope> =
            envelope.and_then(|text| serde_json::from_str(&text).ok());
        let (Some(raw_hash), Some(envelope)) = (raw_hash, envelope) else {
            // Without the finished message there is nothing to send.
            tx.execute(
                "UPDATE drafts SET state = 'failed',
                     error = 'The finished message is missing. Open it and send it again.'
                 WHERE id = ?1",
                [id],
            )?;
            tx.commit()?;
            return Ok(None);
        };
        tx.execute("UPDATE drafts SET state = 'sending' WHERE id = ?1", [id])?;
        tx.commit()?;
        let content: DraftContent = serde_json::from_str(&content).unwrap_or_default();
        Ok(Some(Outgoing {
            id: DraftId(id),
            account,
            raw_hash,
            from: envelope.from,
            to: envelope.to,
            replied_to: content.replied_to,
            attempts,
        }))
    }

    /// A server accepted the message: it leaves the outbox.
    pub fn sent(&mut self, id: DraftId) -> Result<()> {
        self.conn
            .execute("DELETE FROM drafts WHERE id = ?1", [id.0])?;
        Ok(())
    }

    /// The message did not go. With `retry_at` it stays in the outbox and
    /// is tried again then; without, a server has refused it and it waits
    /// for the user.
    pub fn send_failed(&mut self, id: DraftId, error: &str, retry_at: Option<i64>) -> Result<()> {
        match retry_at {
            Some(at) => self.conn.execute(
                "UPDATE drafts SET state = 'outbox', error = ?2, send_after = ?3,
                     attempts = attempts + 1
                 WHERE id = ?1",
                params![id.0, error, at],
            )?,
            None => self.conn.execute(
                "UPDATE drafts SET state = 'failed', error = ?2, attempts = attempts + 1
                 WHERE id = ?1",
                params![id.0, error],
            )?,
        };
        Ok(())
    }

    /// Deals with mail that was being sent when Quill last stopped. Whether
    /// it went is not known, and sending it again could deliver it twice,
    /// so it is not sent: it waits, marked, for the user to decide. Returns
    /// how many there were.
    pub fn recover_interrupted_sends(&mut self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE drafts SET state = 'failed',
                 error = 'Quill stopped while this was being sent. It may have gone: look in Sent before sending it again.'
             WHERE state = 'sending'",
            [],
        )?)
    }
}
