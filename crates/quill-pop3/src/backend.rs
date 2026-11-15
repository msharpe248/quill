//! `Backend` for a POP3 account.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use quill_core::types::{
    Capabilities, Flags, FolderId, FolderRole, Headers, Operation, RemoteEvent, RemoteFolder,
    RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Credential, Credentials, Error, Result};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;

use crate::client::Session;
use crate::config::Pop3Config;
use crate::cursor::Cursor;
use crate::wire::Refusal;

/// Where the password comes from: the Keychain in the app, a constant in
/// tests. Asked for at each sign-in and never kept.
pub type PasswordProvider = Arc<dyn Credentials>;

/// How many messages one `sync_folder` call brings in.
const DEFAULT_BATCH: usize = 500;

/// The id of the one folder a POP3 account has.
const INBOX: &str = "INBOX";

/// What `STAT` reports: the mailbox as it stands, with nothing fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxStatus {
    pub messages: u64,
    /// The total size of the messages, in bytes.
    pub bytes: u64,
}

pub struct Pop3Backend {
    config: Pop3Config,
    password: PasswordProvider,
    batch: usize,
    patience: Duration,
    /// One session at a time. A POP3 server locks the mailbox for the
    /// length of a session, so two calls at once would shut each other out.
    turn: Mutex<()>,
}

impl Pop3Backend {
    pub fn new(config: Pop3Config, password: PasswordProvider) -> Self {
        Self {
            config,
            password,
            batch: DEFAULT_BATCH,
            patience: quill_net::PATIENCE,
            turn: Mutex::new(()),
        }
    }

    /// Sets how many messages one `sync_folder` call brings in.
    pub fn with_batch(mut self, batch: usize) -> Self {
        self.batch = batch.max(1);
        self
    }

    /// Sets how long to wait for each answer from the server before calling
    /// it unreachable. Opening the connection and the TLS handshake keep
    /// `quill_net`'s own wait.
    pub fn with_patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// Signs in and asks how much mail there is. Fetches nothing, so it is
    /// the cheapest way to find out whether an account's settings work.
    pub async fn status(&self) -> Result<MailboxStatus> {
        let _turn = self.turn.lock().await;
        let mut session = self.sign_in().await?;
        let status = session.stat().await;
        session.goodbye().await;
        status.map(|(messages, bytes)| MailboxStatus { messages, bytes })
    }

    /// Downloads several messages in one session. Each is `Some` with the
    /// whole message, or `None` if it is no longer on the server, in the
    /// order of `ids`.
    ///
    /// One session for many messages matters on POP3: every session is a
    /// new connection and a new sign-in, and servers limit how often an
    /// account may sign in.
    pub async fn fetch_many(&self, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let _turn = self.turn.lock().await;
        let mut session = self.sign_in().await?;
        let messages = fetch(&mut session, ids).await;
        session.goodbye().await;
        messages
    }

    /// Deletes messages from the server, all in one session, and returns
    /// how many it removed.
    ///
    /// A POP3 server removes nothing until the session ends with `QUIT` and
    /// it answers `+OK`. `Ok` here means exactly that happened. On any
    /// error, assume nothing about what was removed and call again:
    ///
    /// - Deleting is idempotent. An id that is not on the server counts as
    ///   deleted already and is not an error, so a repeat after a lost
    ///   connection, or after another mail program got there first, ends
    ///   in `Ok`.
    /// - If the connection is lost before the server answers `QUIT`, the
    ///   result is `Error::Connection`, and the messages may or may not be
    ///   gone. If it is lost any earlier, they are all still there.
    /// - If the server refuses to delete one message, the others are still
    ///   removed, and the refusal is returned once that is done.
    pub async fn delete_many(&self, ids: &[RemoteId]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let _turn = self.turn.lock().await;
        let mut session = self.sign_in().await?;
        // On an error the session is dropped where it stands, with no QUIT.
        // That is the one way to be sure nothing half-done is carried out:
        // a server removes messages only when it is told to QUIT.
        let (marked, refused) = mark(&mut session, ids).await?;
        if marked == 0 && refused.is_none() {
            session.goodbye().await;
            return Ok(0);
        }
        session.commit().await?;
        match refused {
            Some(refusal) => Err(refusal.into_error("DELE")),
            None => Ok(marked),
        }
    }

    /// Opens a session: connect, secure the channel, then sign in. The
    /// password is asked for only once the channel is as secure as the
    /// account requires.
    async fn sign_in(&self) -> Result<Session> {
        let mut session = Session::open(&self.config, self.patience).await?;
        let password = match self.password.get().await? {
            Credential::Password(password) => password,
            Credential::OAuthToken(_) => {
                return Err(Error::Unsupported(
                    "a POP3 account signs in with a password, not an OAuth token".to_owned(),
                ));
            }
        };
        session.sign_in(&self.config.username, &password).await?;
        Ok(session)
    }
}

#[async_trait]
impl Backend for Pop3Backend {
    /// Nothing but the mail itself. Folders, flags and search are kept
    /// locally for a POP3 account.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: false,
            flags: false,
            push: false,
            server_search: false,
            threads: false,
        }
    }

    /// The one mailbox POP3 has. Does not contact the server: use
    /// [`Pop3Backend::status`] to check an account.
    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        Ok(vec![RemoteFolder {
            id: FolderId(INBOX.to_owned()),
            name: "Inbox".to_owned(),
            role: FolderRole::Inbox,
        }])
    }

    /// Compares the server's `UIDL` list with the ids in `cursor`.
    ///
    /// Never deletes anything, never sets `reset` and never reports
    /// `changed`. `removed` means only that a message is no longer on the
    /// server; what that means for the local copy is the sync engine's
    /// decision, since for a POP3 account the local copy is often the only
    /// one.
    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        if folder.0 != INBOX {
            return Err(Error::NotFound(format!(
                "{}: a POP3 account has only its inbox",
                folder.0
            )));
        }
        let _turn = self.turn.lock().await;
        let mut session = self.sign_in().await?;
        let delta = sync(&mut session, cursor.as_ref(), self.batch).await;
        session.goodbye().await;
        delta
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        self.fetch_many(std::slice::from_ref(id))
            .await?
            .pop()
            .flatten()
            .ok_or_else(|| Error::NotFound(format!("message {} is no longer on the server", id.0)))
    }

    /// `Delete` removes the message from the server; see
    /// [`Pop3Backend::delete_many`] for exactly when it is gone. Deleting a
    /// message that is not there is `Ok`. Flags and folders are local-only
    /// for POP3, so the other operations are `Error::Unsupported`.
    async fn apply(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::Delete { id } => {
                self.delete_many(std::slice::from_ref(id)).await.map(|_| ())
            }
            Operation::SetFlags { .. } => Err(Error::Unsupported(
                "a POP3 server keeps no flags; Quill keeps them locally".to_owned(),
            )),
            Operation::Move { .. } | Operation::Copy { .. } => Err(Error::Unsupported(
                "a POP3 server has no folders; Quill keeps them locally".to_owned(),
            )),
        }
    }

    async fn fetch_many(&self, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
        Pop3Backend::fetch_many(self, ids).await
    }

    async fn delete_many(&self, ids: &[RemoteId]) -> Result<()> {
        Pop3Backend::delete_many(self, ids).await.map(|_| ())
    }

    /// POP3 has no push. The sync engine polls.
    async fn watch(&self, _tx: Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }
}

/// Downloads each of `ids` that the server still has.
async fn fetch(session: &mut Session, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
    let listing = session.uidl().await?;
    let numbers: HashMap<&str, u32> = listing
        .iter()
        // Backwards, so that of two messages with one id the first wins.
        .rev()
        .map(|(number, id)| (id.as_str(), *number))
        .collect();
    let mut messages = Vec::with_capacity(ids.len());
    for id in ids {
        messages.push(match numbers.get(id.0.as_str()) {
            Some(number) => Some(Bytes::from(session.retr(*number).await?)),
            None => None,
        });
    }
    Ok(messages)
}

/// Marks each of `ids` that the server still has for deletion. Returns how
/// many messages were marked, and the first refusal if the server would
/// not mark one. An error means the session is over or nothing was marked.
async fn mark(session: &mut Session, ids: &[RemoteId]) -> Result<(usize, Option<Refusal>)> {
    let listing = session.uidl().await?;
    let wanted: HashSet<&str> = ids.iter().map(|id| id.0.as_str()).collect();
    // Every message with a wanted id. A server that gives two messages one
    // id showed them to Quill as one message, so both go.
    let numbers: Vec<u32> = listing
        .iter()
        .filter(|(_, id)| wanted.contains(id.as_str()))
        .map(|(number, _)| *number)
        .collect();
    let refused = session.dele(&numbers).await?;
    Ok((numbers.len() - refused.len(), refused.into_iter().next()))
}

async fn sync(
    session: &mut Session,
    cursor: Option<&SyncCursor>,
    batch: usize,
) -> Result<SyncDelta> {
    let listing = session.uidl().await?;
    // A cursor that cannot be read is treated as no cursor: everything on
    // the server is reported as added again and nothing as removed. Never
    // `reset`, which would tell the engine to drop local mail that may not
    // be on the server any more.
    let known = cursor.and_then(Cursor::decode).unwrap_or_default().seen;

    let present: HashSet<&str> = listing.iter().map(|(_, id)| id.as_str()).collect();
    let removed: Vec<RemoteId> = known
        .iter()
        .filter(|id| !present.contains(id.as_str()))
        .map(|id| RemoteId(id.clone()))
        .collect();

    // Newest first: the mail people want is the mail that just arrived, and
    // a server numbers its messages in the order they came.
    let mut newest_first: Vec<&(u32, String)> = listing.iter().collect();
    newest_first.sort_by_key(|(number, _)| Reverse(*number));
    let mut chosen = HashSet::new();
    let mut missing: Vec<(u32, &str)> = newest_first
        .into_iter()
        // A server that gives two messages the same id has made them one
        // message as far as anyone can tell. Take the newer.
        .filter(|(_, id)| !known.contains(id) && chosen.insert(id.as_str()))
        .map(|(number, id)| (*number, id.as_str()))
        .collect();
    let more_waiting = missing.len() > batch;
    missing.truncate(batch);

    let numbers: Vec<u32> = missing.iter().map(|(number, _)| *number).collect();
    let (sizes, headers) = if numbers.is_empty() {
        (HashMap::new(), Vec::new())
    } else {
        (
            session.sizes(&numbers, listing.len()).await?,
            session.headers(&numbers).await?,
        )
    };

    let received = now();
    let mut seen: BTreeSet<String> = known
        .into_iter()
        .filter(|id| present.contains(id.as_str()))
        .collect();
    let mut added = Vec::with_capacity(missing.len());
    for ((number, id), headers) in missing.iter().zip(headers) {
        // A message the server lists but will not hand over is left out of
        // the cursor, so the next sync asks for it again.
        let Some(headers) = headers else { continue };
        let size = sizes.get(number).copied().unwrap_or(0);
        added.push(summary(id, &headers, size, received));
        seen.insert((*id).to_owned());
    }

    Ok(SyncDelta {
        reset: false,
        // If nothing could be fetched, asking again at once would get the
        // same nothing. Leave the rest for the next poll.
        has_more: more_waiting && !added.is_empty(),
        added,
        changed: Vec::new(),
        removed,
        next_cursor: Cursor { seen }.encode(),
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// A list row from a message's headers. `received` stands in for the date
/// when the message has none: POP3 does not say when a message arrived, so
/// the best that can be said is when Quill first saw it.
fn summary(id: &str, headers: &[u8], size: u64, received: i64) -> RemoteMessage {
    let read = quill_mime::parse(headers, received);
    let thread = read
        .as_ref()
        .map(quill_mime::Parsed::thread)
        .unwrap_or_default();
    let parsed = read.map_or_else(
        || Headers {
            message_id: None,
            subject: String::new(),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date: received,
        },
        |parsed| parsed.headers,
    );
    RemoteMessage {
        id: RemoteId(id.to_owned()),
        headers: parsed,
        // Unread and unmarked. POP3 has no flags to say otherwise.
        flags: Flags::default(),
        size,
        has_attachments: looks_attached(headers),
        thread,
    }
}

/// A guess from the headers alone, good enough for a marker in the list.
/// The exact attachment list comes from the message once it is downloaded.
fn looks_attached(headers: &[u8]) -> bool {
    match content_type(headers).as_deref() {
        None | Some("") => false,
        Some("multipart/mixed") => true,
        // Other multiparts are a body in several forms, or signed or
        // encrypted. Anything that is not text is a file sent bare.
        Some(kind) => !(kind.starts_with("multipart/") || kind.starts_with("text/")),
    }
}

/// The media type from the `Content-Type` header, in lower case.
fn content_type(headers: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(headers);
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            // The end of the headers. A body part's own headers do not count.
            return None;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        // The value may be folded over several lines.
        let mut value = value.to_owned();
        while let Some(more) = lines.next_if(|line| line.starts_with([' ', '\t'])) {
            value.push_str(more);
        }
        let kind = value.split(';').next().unwrap_or_default().trim();
        return Some(kind.to_ascii_lowercase());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_type_is_found_however_the_header_is_written() {
        assert_eq!(
            content_type(b"Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n")
                .as_deref(),
            Some("text/plain")
        );
        assert_eq!(
            content_type(b"content-type:Multipart/Mixed;\r\n boundary=\"b\"\r\n").as_deref(),
            Some("multipart/mixed")
        );
        // Folded before the type itself.
        assert_eq!(
            content_type(b"Content-Type:\r\n\tapplication/pdf;\r\n\tname=a.pdf\r\n").as_deref(),
            Some("application/pdf")
        );
        assert_eq!(content_type(b"Subject: no type\r\n\r\n"), None);
        // A part's header, below the blank line, is not the message's.
        assert_eq!(
            content_type(b"Subject: x\r\n\r\nContent-Type: application/pdf\r\n"),
            None
        );
        // Not the header, though it ends the same way.
        assert_eq!(content_type(b"X-Content-Type: application/pdf\r\n"), None);
        assert_eq!(content_type(b""), None);
    }

    #[test]
    fn the_attachment_marker_is_a_guess_from_the_media_type() {
        let attached = |kind: &str| looks_attached(format!("Content-Type: {kind}\r\n").as_bytes());
        assert!(attached("multipart/mixed; boundary=b"));
        assert!(attached("application/pdf; name=\"invoice.pdf\""));
        assert!(attached("image/png"));
        assert!(!attached("text/plain; charset=utf-8"));
        assert!(!attached("text/html"));
        assert!(!attached("multipart/alternative; boundary=b"));
        assert!(!attached("multipart/related; boundary=b"));
        assert!(!attached("multipart/signed; boundary=b"));
        assert!(!attached(""));
        assert!(!looks_attached(b"Subject: plain old mail\r\n\r\n"));
    }

    #[test]
    fn a_summary_reads_the_headers_and_falls_back_on_when_it_was_seen() {
        let headers = b"From: Ana Ruiz <ana.ruiz@example.com>\r\n\
                        Subject: =?utf-8?Q?Caf=C3=A9_on_Friday?=\r\n\
                        Date: Sat, 14 Nov 2026 09:14:00 +0000\r\n\r\n";
        let message = summary("0000000169185d3a", headers, 321, 5);
        assert_eq!(message.id, RemoteId("0000000169185d3a".to_owned()));
        assert_eq!(message.headers.subject, "Café on Friday");
        assert_eq!(message.headers.from[0].email, "ana.ruiz@example.com");
        assert_eq!(message.headers.date, 1_794_647_640);
        assert_eq!(message.size, 321);
        assert_eq!(message.flags, Flags::default());
        assert!(!message.has_attachments);

        // No date of its own, and nothing at all.
        assert_eq!(summary("a", b"Subject: x\r\n\r\n", 0, 5).headers.date, 5);
        let empty = summary("b", b"", 0, 5);
        assert_eq!(empty.headers.subject, "");
        assert_eq!(empty.headers.date, 5);
    }
}
