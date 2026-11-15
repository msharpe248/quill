//! `Backend` for an IMAP account.

use std::collections::HashSet;
use std::sync::Arc;

use async_imap::Session;
use async_imap::imap_proto::BodyStructure;
use async_imap::types::{Fetch, Flag, NameAttribute};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::TryStreamExt;
use quill_core::types::{
    Capabilities, FlagChange, Flags, FolderId, Headers, Operation, RemoteEvent, RemoteFolder,
    RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Error, Result};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;

use crate::config::ImapConfig;
use quill_net::Stream;

use crate::connect;
use crate::names;
use crate::uidset::UidSet;

/// Where the password comes from: the Keychain in the app, a constant in
/// tests. Asked for at each sign-in and never kept.
pub type PasswordProvider = Arc<dyn Fn() -> Result<String> + Send + Sync>;

/// How many messages one `sync_folder` call brings in.
const DEFAULT_BATCH: usize = 500;

/// The most UID ranges put in one command.
const RANGES_PER_COMMAND: usize = 200;

/// What is fetched for a message in a folder sync: enough for a list row,
/// and no body. The named headers go through the same parser as whole
/// messages, so encodings are handled in one place.
const SUMMARY: &str = "(UID FLAGS RFC822.SIZE INTERNALDATE BODYSTRUCTURE \
                       BODY.PEEK[HEADER.FIELDS (DATE SUBJECT FROM TO CC MESSAGE-ID)])";

pub struct ImapBackend {
    config: ImapConfig,
    password: PasswordProvider,
    batch: usize,
    /// One connection, opened on demand and dropped after any failure so the
    /// next call starts clean.
    session: Mutex<Option<Session<Stream>>>,
}

impl ImapBackend {
    pub fn new(config: ImapConfig, password: PasswordProvider) -> Self {
        Self {
            config,
            password,
            batch: DEFAULT_BATCH,
            session: Mutex::new(None),
        }
    }

    /// Sets how many messages one `sync_folder` call brings in.
    pub fn with_batch(mut self, batch: usize) -> Self {
        self.batch = batch.max(1);
        self
    }

    /// Signs out and closes the connection, if there is one.
    pub async fn disconnect(&self) {
        if let Some(mut session) = self.session.lock().await.take() {
            let _ = session.logout().await;
        }
    }

    async fn sign_in(&self) -> Result<Session<Stream>> {
        let client = connect::connect(&self.config).await?;
        let password = (self.password)()?;
        client
            .login(&self.config.username, password)
            .await
            .map_err(|(err, _)| match err {
                async_imap::error::Error::No(text) | async_imap::error::Error::Bad(text) => {
                    Error::Auth(text)
                }
                other => Error::Connection(other.to_string()),
            })
    }
}

/// Runs `$body` with `$session` bound to the open connection, signing in
/// first if need be. A failure closes the connection: an IMAP session after
/// an error is in an unknown state, and the next call should not inherit it.
macro_rules! with_session {
    ($backend:expr, |$session:ident| $body:expr) => {{
        let mut guard = $backend.session.lock().await;
        if guard.is_none() {
            *guard = Some($backend.sign_in().await?);
        }
        let result = match guard.as_mut() {
            Some($session) => $body,
            None => Err(Error::Backend("no connection".to_owned())),
        };
        if result.is_err() {
            *guard = None;
        }
        result
    }};
}

#[async_trait]
impl Backend for ImapBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            // IDLE arrives with push in M6.
            push: false,
            server_search: false,
            threads: false,
        }
    }

    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        with_session!(self, |session| list_folders(session).await)
    }

    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        let batch = self.batch;
        with_session!(self, |session| sync_folder(session, folder, cursor, batch)
            .await)
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        let message = MessageRef::parse(id)?;
        with_session!(self, |session| fetch_raw(session, &message).await)
    }

    async fn apply(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::SetFlags { id, flags } => {
                let message = MessageRef::parse(id)?;
                let flags = *flags;
                with_session!(self, |session| set_flags(session, &message, flags).await)
            }
            Operation::Move { .. } | Operation::Copy { .. } | Operation::Delete { .. } => {
                Err(Error::Unsupported(
                    "moving and deleting arrive with the operation queue".to_owned(),
                ))
            }
        }
    }

    /// Push arrives with IDLE in M6. Until then the sync engine polls.
    async fn watch(&self, _tx: Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }
}

/// A message's place on the server: its UID, the UIDVALIDITY it is good
/// under, and its folder. Written into the `RemoteId` so that the id alone
/// finds the message, and a renumbered folder cannot be misread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageRef {
    pub uid: u32,
    pub validity: u32,
    pub folder: String,
}

impl MessageRef {
    pub fn id(&self) -> RemoteId {
        // The folder goes last because it may contain any character.
        RemoteId(format!("{};{};{}", self.uid, self.validity, self.folder))
    }

    pub fn parse(id: &RemoteId) -> Result<Self> {
        let mut parts = id.0.splitn(3, ';');
        let (uid, validity, folder) = (parts.next(), parts.next(), parts.next());
        match (
            uid.and_then(|uid| uid.parse().ok()),
            validity.and_then(|validity| validity.parse().ok()),
            folder,
        ) {
            (Some(uid), Some(validity), Some(folder)) if !folder.is_empty() => Ok(Self {
                uid,
                validity,
                folder: folder.to_owned(),
            }),
            _ => Err(Error::NotFound(format!(
                "\"{}\" is not an IMAP message id",
                id.0
            ))),
        }
    }
}

/// Where a folder sync stopped: the UIDVALIDITY the UIDs belong to, and the
/// UIDs already delivered. Kept as text in the store's `sync_state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub validity: u32,
    pub known: UidSet,
}

impl Cursor {
    const VERSION: &'static str = "1";

    pub fn encode(&self) -> SyncCursor {
        SyncCursor(format!(
            "{};{};{}",
            Self::VERSION,
            self.validity,
            self.known
        ))
    }

    /// `None` for a cursor this version did not write. The caller then syncs
    /// the folder from scratch, which is always safe.
    pub fn decode(cursor: &SyncCursor) -> Option<Self> {
        let mut parts = cursor.0.splitn(3, ';');
        if parts.next()? != Self::VERSION {
            return None;
        }
        Some(Self {
            validity: parts.next()?.parse().ok()?,
            known: UidSet::parse(parts.next()?)?,
        })
    }
}

fn failed(err: async_imap::error::Error) -> Error {
    use async_imap::error::Error as Imap;
    match err {
        Imap::Io(_) | Imap::ConnectionLost => Error::Connection(err.to_string()),
        Imap::No(text) | Imap::Bad(text) => Error::Backend(text),
        other => Error::Backend(other.to_string()),
    }
}

async fn list_folders(session: &mut Session<Stream>) -> Result<Vec<RemoteFolder>> {
    let names: Vec<_> = session
        .list(Some(""), Some("*"))
        .await
        .map_err(failed)?
        .try_collect()
        .await
        .map_err(failed)?;
    // Servers list in no particular order, and may list a folder twice.
    let mut listed: Vec<names::Listed<'_>> = names
        .iter()
        // A name that only holds other folders has no messages to sync.
        .filter(|name| !name.attributes().contains(&NameAttribute::NoSelect))
        .map(|name| names::Listed {
            raw: name.name(),
            delimiter: name.delimiter(),
            attributes: name.attributes(),
        })
        .collect();
    listed.sort_by(|a, b| a.raw.cmp(b.raw));
    listed.dedup_by(|a, b| a.raw == b.raw);
    let folders = listed
        .iter()
        .zip(names::folders(&listed))
        .map(|(listed, (name, role))| RemoteFolder {
            id: FolderId(listed.raw.to_owned()),
            name,
            role,
        })
        .collect();
    Ok(folders)
}

async fn sync_folder(
    session: &mut Session<Stream>,
    folder: &FolderId,
    cursor: Option<SyncCursor>,
    batch: usize,
) -> Result<SyncDelta> {
    let mailbox = session.select(&folder.0).await.map_err(|err| match err {
        async_imap::error::Error::No(text) => Error::NotFound(format!("{}: {text}", folder.0)),
        other => failed(other),
    })?;
    let validity = mailbox
        .uid_validity
        .ok_or_else(|| Error::Backend(format!("{} has no UIDVALIDITY", folder.0)))?;

    let previous = cursor.as_ref().and_then(Cursor::decode);
    // A new UIDVALIDITY means every UID we hold names some other message
    // now, or none. Start the folder again.
    let reset = previous.as_ref().is_some_and(|c| c.validity != validity);
    let known = match previous {
        Some(c) if c.validity == validity => c.known,
        _ => UidSet::default(),
    };

    let present: HashSet<u32> = session.uid_search("ALL").await.map_err(failed)?;
    let message = |uid: u32| MessageRef {
        uid,
        validity,
        folder: folder.0.clone(),
    };

    let removed: Vec<RemoteId> = known
        .iter()
        .filter(|uid| !present.contains(uid))
        .map(|uid| message(uid).id())
        .collect();

    // Newest first: the mail people want is the mail that just arrived.
    let mut missing: Vec<u32> = present
        .iter()
        .copied()
        .filter(|uid| !known.contains(*uid))
        .collect();
    missing.sort_unstable_by(|a, b| b.cmp(a));
    let has_more = missing.len() > batch;
    missing.truncate(batch);

    let mut added = Vec::with_capacity(missing.len());
    let mut fetched = Vec::with_capacity(missing.len());
    for set in UidSet::from_uids(missing.iter().copied()).sequence_sets(RANGES_PER_COMMAND) {
        let fetches: Vec<Fetch> = session
            .uid_fetch(&set, SUMMARY)
            .await
            .map_err(failed)?
            .try_collect()
            .await
            .map_err(failed)?;
        for fetch in &fetches {
            if let Some(uid) = fetch.uid {
                fetched.push(uid);
                added.push(summary(fetch, message(uid).id()));
            }
        }
    }
    added.sort_by_key(|message| std::cmp::Reverse(message.headers.date));

    // Flags of what we already had, once the folder has caught up. While a
    // first sync is still bringing mail in, the new mail matters more.
    let still_here: Vec<u32> = known.iter().filter(|uid| present.contains(uid)).collect();
    let mut changed = Vec::new();
    if !has_more {
        for set in UidSet::from_uids(still_here.iter().copied()).sequence_sets(RANGES_PER_COMMAND) {
            let fetches: Vec<Fetch> = session
                .uid_fetch(&set, "(UID FLAGS)")
                .await
                .map_err(failed)?
                .try_collect()
                .await
                .map_err(failed)?;
            changed.extend(fetches.iter().filter_map(|fetch| {
                Some(FlagChange {
                    id: message(fetch.uid?).id(),
                    flags: flags(fetch),
                })
            }));
        }
    }

    let known = UidSet::from_uids(still_here.into_iter().chain(fetched));
    Ok(SyncDelta {
        reset,
        added,
        changed,
        removed: if reset { Vec::new() } else { removed },
        has_more,
        next_cursor: Cursor { validity, known }.encode(),
    })
}

fn summary(fetch: &Fetch, id: RemoteId) -> RemoteMessage {
    let received = fetch.internal_date().map_or(0, |date| date.timestamp());
    let headers = fetch
        .header()
        .and_then(|raw| quill_mime::parse(raw, received))
        .map_or_else(
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
        id,
        headers,
        flags: flags(fetch),
        size: u64::from(fetch.size.unwrap_or(0)),
        has_attachments: fetch.bodystructure().is_some_and(has_attachments),
    }
}

fn flags(fetch: &Fetch) -> Flags {
    let mut flags = Flags::default();
    for flag in fetch.flags() {
        match flag {
            Flag::Seen => flags.seen = true,
            Flag::Flagged => flags.flagged = true,
            Flag::Answered => flags.answered = true,
            Flag::Draft => flags.draft = true,
            _ => {}
        }
    }
    flags
}

/// A guess from the structure alone, good enough for a marker in the list.
/// The exact attachment list comes from the message once it is downloaded.
fn has_attachments(structure: &BodyStructure<'_>) -> bool {
    match structure {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(has_attachments),
        BodyStructure::Text { common, .. } => common
            .disposition
            .as_ref()
            .is_some_and(|disposition| disposition.ty.eq_ignore_ascii_case("attachment")),
        BodyStructure::Basic { .. } | BodyStructure::Message { .. } => true,
    }
}

/// Selects the message's folder and checks its UIDs still mean what they did.
async fn select_for(session: &mut Session<Stream>, message: &MessageRef) -> Result<()> {
    let mailbox = session
        .select(&message.folder)
        .await
        .map_err(|err| match err {
            async_imap::error::Error::No(text) => {
                Error::NotFound(format!("{}: {text}", message.folder))
            }
            other => failed(other),
        })?;
    if mailbox.uid_validity != Some(message.validity) {
        return Err(Error::NotFound(format!(
            "{} was renumbered by the server; sync it again",
            message.folder
        )));
    }
    Ok(())
}

async fn fetch_raw(session: &mut Session<Stream>, message: &MessageRef) -> Result<Bytes> {
    select_for(session, message).await?;
    // PEEK: reading a message here must not mark it read on the server.
    let fetches: Vec<Fetch> = session
        .uid_fetch(message.uid.to_string(), "(UID BODY.PEEK[])")
        .await
        .map_err(failed)?
        .try_collect()
        .await
        .map_err(failed)?;
    fetches
        .iter()
        .find(|fetch| fetch.uid == Some(message.uid))
        .and_then(Fetch::body)
        .map(Bytes::copy_from_slice)
        .ok_or_else(|| Error::NotFound(format!("message {} is gone", message.uid)))
}

async fn set_flags(
    session: &mut Session<Stream>,
    message: &MessageRef,
    flags: Flags,
) -> Result<()> {
    select_for(session, message).await?;
    let all = [
        (flags.seen, "\\Seen"),
        (flags.flagged, "\\Flagged"),
        (flags.answered, "\\Answered"),
        (flags.draft, "\\Draft"),
    ];
    // Added and removed by name, never replaced wholesale: the message may
    // carry keywords and flags that Quill does not track and must not drop.
    for (wanted, verb) in [(true, "+FLAGS.SILENT"), (false, "-FLAGS.SILENT")] {
        let names: Vec<&str> = all
            .iter()
            .filter(|(on, _)| *on == wanted)
            .map(|(_, name)| *name)
            .collect();
        if names.is_empty() {
            continue;
        }
        let query = format!("{verb} ({})", names.join(" "));
        let _: Vec<Fetch> = session
            .uid_store(message.uid.to_string(), &query)
            .await
            .map_err(failed)?
            .try_collect()
            .await
            .map_err(failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_id_round_trips_even_with_awkward_folder_names() {
        for folder in ["INBOX", "Work/Projects; 2026", "a;b;c", "&U,BTFw-"] {
            let message = MessageRef {
                uid: 4521,
                validity: 1_794_713_293,
                folder: folder.to_owned(),
            };
            assert_eq!(MessageRef::parse(&message.id()).unwrap(), message);
        }
    }

    #[test]
    fn what_is_not_a_message_id_is_not_found() {
        for bad in [
            "",
            "12",
            "12;34",
            "12;34;",
            "x;34;INBOX",
            "12;y;INBOX",
            "inbox-1",
        ] {
            assert!(
                matches!(
                    MessageRef::parse(&RemoteId(bad.to_owned())),
                    Err(Error::NotFound(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_cursor_round_trips() {
        let cursor = Cursor {
            validity: 7,
            known: UidSet::from_uids([1, 2, 3, 9]),
        };
        assert_eq!(cursor.encode().0, "1;7;1:3,9");
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        let empty = Cursor {
            validity: 7,
            known: UidSet::default(),
        };
        assert_eq!(Cursor::decode(&empty.encode()), Some(empty));
    }

    #[test]
    fn a_cursor_from_elsewhere_is_ignored_not_trusted() {
        for other in ["", "fixture-v1", "2;7;1:3", "1;x;1:3", "1;7;3:1", "1;7"] {
            assert_eq!(
                Cursor::decode(&SyncCursor(other.to_owned())),
                None,
                "{other}"
            );
        }
    }
}
