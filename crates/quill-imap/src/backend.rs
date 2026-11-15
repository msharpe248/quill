//! `Backend` for an IMAP account.
//!
//! Every command is sent through [`Command`], which reads how the server
//! ended it. async-imap's own search, fetch and store do not: a command the
//! server refused comes back from them as one that found nothing, and a
//! sync that trusted that would report a whole folder gone.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use async_imap::Session;
use async_imap::imap_proto::{
    AttributeValue, BodyStructure, MailboxDatum, MailboxListData, MessageSection, NameAttribute,
    Response, SectionPath,
};
use async_trait::async_trait;
use bytes::Bytes;
use quill_core::types::{
    Capabilities, FlagChange, Flags, FolderId, Headers, Operation, RemoteEvent, RemoteFolder,
    RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Credential, Credentials, Error, Result};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;

use crate::config::ImapConfig;
use quill_net::Stream;

use crate::change::{self, Extensions};
use crate::command::{Command, Outcome, Wire, uid_search};
use crate::connect;
use crate::idle;
use crate::names;
use crate::uidset::UidSet;
use crate::wait::Bounded;

/// Where the account's secret comes from: the Keychain or an OAuth
/// provider in the app, a constant in tests. Asked for at each sign-in and
/// never kept.
pub type PasswordProvider = Arc<dyn Credentials>;

/// How many messages one `sync_folder` call brings in.
const DEFAULT_BATCH: usize = 500;

/// The most UID ranges put in one command.
const RANGES_PER_COMMAND: usize = 200;

/// What is fetched for a message in a folder sync: enough for a list row,
/// and no body. The named headers go through the same parser as whole
/// messages, so encodings are handled in one place.
const SUMMARY: &str = "(UID FLAGS RFC822.SIZE INTERNALDATE BODYSTRUCTURE \
                       BODY.PEEK[HEADER.FIELDS (DATE SUBJECT FROM TO CC MESSAGE-ID \
                       IN-REPLY-TO REFERENCES)])";

/// What is known of whether the server pushes: nothing until a sign-in has
/// asked it.
const PUSH_UNKNOWN: u8 = 0;
const PUSH_NO: u8 = 1;
const PUSH_YES: u8 = 2;

pub struct ImapBackend {
    config: ImapConfig,
    password: PasswordProvider,
    batch: usize,
    patience: Duration,
    idle_refresh: Duration,
    /// One connection, opened on demand and dropped after any failure so the
    /// next call starts clean. `watch` does not use it: it opens its own.
    session: Mutex<Option<Open>>,
    /// Whether the server said it has `IDLE`, the last time it was asked.
    /// Kept apart from the connection because `capabilities` is asked
    /// without one and cannot wait for a lock.
    push: AtomicU8,
}

/// Signs in on a connection that has just been made, with whatever the
/// account signs in with: `LOGIN` for a password, `AUTHENTICATE XOAUTH2`
/// for a token.
async fn authenticate<T>(
    client: async_imap::Client<T>,
    user: &str,
    credentials: &dyn Credentials,
) -> Result<Session<T>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let refused = |err| match err {
        async_imap::error::Error::No(text) | async_imap::error::Error::Bad(text) => {
            Error::Auth(text)
        }
        async_imap::error::Error::Io(err) => Error::Connection(err.to_string()),
        other => Error::Connection(other.to_string()),
    };
    match credentials.get().await? {
        Credential::Password(password) => client
            .login(user, password)
            .await
            .map_err(|(err, _)| refused(err)),
        Credential::OAuthToken(token) => {
            match client
                .authenticate("XOAUTH2", XOAuth2::new(user, &token))
                .await
            {
                Ok(session) => Ok(session),
                // A token can be refused before its time: withdrawn, or the
                // clocks disagree. One renewed token is tried before the
                // account is said to need signing in to again.
                Err((async_imap::error::Error::No(text), client)) => {
                    credentials.refused().await;
                    match credentials.get().await? {
                        Credential::OAuthToken(renewed) if renewed != token => client
                            .authenticate("XOAUTH2", XOAuth2::new(user, &renewed))
                            .await
                            .map_err(|(err, _)| refused(err)),
                        _ => Err(Error::Auth(text)),
                    }
                }
                Err((err, _)) => Err(refused(err)),
            }
        }
    }
}

/// Signing in with an OAuth token, the way Gmail's and Microsoft's mail
/// servers take one.
struct XOAuth2 {
    /// What to answer the server's first, empty, challenge with.
    first: Option<String>,
}

impl XOAuth2 {
    fn new(user: &str, token: &str) -> Self {
        Self {
            first: Some(quill_core::credentials::xoauth2(user, token)),
        }
    }
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    /// A server that refuses the token says why in a second challenge, and
    /// wants an empty line back before it gives its real answer.
    fn process(&mut self, _challenge: &[u8]) -> String {
        self.first.take().unwrap_or_default()
    }
}

/// A signed-in connection and what its server can do. The two are kept
/// together because the answer belongs to the connection: a server may be
/// upgraded, or sit behind a proxy that changes, between one sign-in and the
/// next.
struct Open {
    session: Session<Bounded<Stream>>,
    extensions: Extensions,
}

impl ImapBackend {
    pub fn new(config: ImapConfig, password: PasswordProvider) -> Self {
        Self {
            config,
            password,
            batch: DEFAULT_BATCH,
            patience: quill_net::PATIENCE,
            idle_refresh: idle::REFRESH,
            session: Mutex::new(None),
            push: AtomicU8::new(PUSH_UNKNOWN),
        }
    }

    /// Sets how many messages one `sync_folder` call brings in.
    pub fn with_batch(mut self, batch: usize) -> Self {
        self.batch = batch.max(1);
        self
    }

    /// Sets how long the server may stay silent while it is being waited
    /// for, before the connection is given up as lost. It is a limit on
    /// silence, not on a whole command: a large message on a slow line is
    /// fine for as long as it keeps arriving.
    pub fn with_patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// Sets how long `watch` idles before it stops and starts again, which
    /// is how it finds out that the connection has died: five minutes
    /// unless set. A connection that died quietly is noticed within this
    /// plus the patience of [`ImapBackend::with_patience`]. Servers end an
    /// idle of about half an hour themselves, so keep it well under that.
    pub fn with_idle_refresh(mut self, refresh: Duration) -> Self {
        self.idle_refresh = refresh;
        self
    }

    /// Whether the server pushes (has `IDLE`): `None` until a sign-in has
    /// asked it, and after that what it said the last time it was asked.
    /// Every sign-in asks, `watch`'s own included.
    ///
    /// After `watch` has returned `Ok` this is how to tell why: `Some(false)`
    /// means there is no push to be had and the account must be polled.
    pub fn supports_idle(&self) -> Option<bool> {
        match self.push.load(Ordering::Relaxed) {
            PUSH_YES => Some(true),
            PUSH_NO => Some(false),
            _ => None,
        }
    }

    /// Signs out and closes the command connection, if there is one. A
    /// `watch` in progress has a connection of its own and is not touched:
    /// it ends when its future is dropped.
    pub async fn disconnect(&self) {
        if let Some(mut open) = self.session.lock().await.take() {
            let _ = open.session.logout().await;
        }
    }

    async fn sign_in(&self) -> Result<Open> {
        let client = connect::connect(&self.config, self.patience).await?;
        let mut session = authenticate(client, &self.config.username, &*self.password).await?;
        let extensions = Extensions::ask(&mut session).await?;
        let push = if extensions.idle { PUSH_YES } else { PUSH_NO };
        self.push.store(push, Ordering::Relaxed);
        Ok(Open {
            session,
            extensions,
        })
    }
}

/// Runs `$body` with `$session` bound to the open connection, signing in
/// first if need be. A failure closes the connection: an IMAP session after
/// an error is in an unknown state, and the next call should not inherit it.
/// That includes a server that stopped answering, so the call after a
/// timeout starts on a new connection.
///
/// With a second name, `$body` is also given what the server can do.
macro_rules! with_session {
    ($backend:expr, |$session:ident| $body:expr) => {
        with_session!($backend, |$session, _extensions| $body)
    };
    ($backend:expr, |$session:ident, $extensions:ident| $body:expr) => {{
        let mut guard = $backend.session.lock().await;
        if guard.is_none() {
            *guard = Some($backend.sign_in().await?);
        }
        let result = match guard.as_mut() {
            Some(open) => {
                let $extensions = open.extensions;
                let $session = &mut open.session;
                $body
            }
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
    /// `push` is what the server said at the last sign-in, and `false`
    /// before the first: it is not claimed until it is known. So it can
    /// change from one call to the next, and is worth asking again after a
    /// sync or after `watch` returns.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            push: self.supports_idle() == Some(true),
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
            Operation::Move { id, to } => self.move_message(id, to).await.map(drop),
            Operation::Copy { id, to } => {
                let message = MessageRef::parse(id)?;
                with_session!(self, |session| change::copy(session, &message, to).await)
            }
            Operation::Delete { id } => {
                let message = MessageRef::parse(id)?;
                with_session!(self, |session, extensions| change::delete(
                    session, extensions, &message
                )
                .await)
            }
        }
    }

    /// The new id is known whenever the server has UIDPLUS, which is what
    /// makes it say where a copy went. `change::move_message` says how the
    /// move is done on servers with and without `MOVE`.
    async fn move_message(&self, id: &RemoteId, to: &FolderId) -> Result<Option<RemoteId>> {
        let message = MessageRef::parse(id)?;
        with_session!(self, |session, extensions| change::move_message(
            session, extensions, &message, to
        )
        .await)
    }

    async fn append(
        &self,
        folder: &FolderId,
        raw: &[u8],
        flags: Flags,
    ) -> Result<Option<RemoteId>> {
        with_session!(self, |session| change::append(session, folder, raw, flags)
            .await)
    }

    /// Watches the inbox with IDLE and sends `FolderChanged`, naming the
    /// inbox as `list_folders` does, whenever it is worth syncing: once as
    /// soon as the watch is in place, and after that each time the server
    /// reports a change (`idle` says why, and how reports are gathered into
    /// events).
    ///
    /// It runs until one of three things happens:
    ///
    /// - `Err`: the connection, or the sign-in, failed. Wait a while and
    ///   call it again.
    /// - `Ok` with the receiver dropped: nobody is listening any more.
    /// - `Ok` with the receiver still held: there is nothing here to wait
    ///   for, so poll. It comes after one sign-in and no waiting, when the
    ///   server has no `IDLE` ([`ImapBackend::supports_idle`] and
    ///   `capabilities().push` say so from then on) or lists no inbox.
    ///
    /// The connection is the watch's own, signed in here, so every other
    /// call on the backend works while this one waits. It lives inside this
    /// future: dropping the future closes it, and nothing is left running.
    async fn watch(&self, tx: Sender<RemoteEvent>) -> Result<()> {
        if tx.is_closed() {
            return Ok(());
        }
        let mut open = self.sign_in().await?;
        idle::watch_inbox(&mut open.session, open.extensions, &tx, self.idle_refresh).await
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

pub(crate) async fn list_folders<T: Wire>(session: &mut Session<T>) -> Result<Vec<RemoteFolder>> {
    let mut names: Vec<MailboxListData<'static>> = Vec::new();
    let reply = Command::new("LIST")
        .arg("\"\"")
        .arg("*")
        .run_with(session, |response| {
            if let Response::MailboxData(MailboxDatum::List(name)) = response {
                names.push(name.clone().into_owned());
            }
        })
        .await?;
    // A list the server refused is not an account with no folders.
    reply.require("the server would not list its folders")?;

    let named: Vec<_> = names
        .iter()
        // A name that only holds other folders has no messages to sync.
        .filter(|name| !name.name_attributes.contains(&NameAttribute::NoSelect))
        .map(|name| {
            (
                names::unescape(&name.name),
                name.delimiter.as_deref().map(names::unescape),
                name.name_attributes.as_slice(),
            )
        })
        .collect();
    // Servers list in no particular order, and may list a folder twice.
    let mut listed: Vec<names::Listed<'_>> = named
        .iter()
        .map(|(raw, delimiter, attributes)| names::Listed {
            raw,
            delimiter: delimiter.as_deref(),
            attributes,
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

/// Selects a folder and returns the UIDVALIDITY its UIDs belong to.
async fn open<T: Wire>(session: &mut Session<T>, folder: &str) -> Result<u32> {
    let reply = Command::new("SELECT").mailbox(folder)?.run(session).await?;
    match reply.outcome {
        Outcome::Ok => reply
            .uid_validity
            .ok_or_else(|| Error::Backend(format!("{folder} has no UIDVALIDITY"))),
        Outcome::No => Err(Error::NotFound(format!(
            "{folder}: {}",
            reply.words("the server would not open it")
        ))),
        Outcome::Bad => Err(Error::Backend(format!(
            "{folder}: {}",
            reply.words("the server did not understand")
        ))),
    }
}

/// What changed in a folder since `cursor`.
///
/// A message marked `\Deleted` is treated as not there, whoever marked it:
/// it is on its way out and must not be shown as ordinary mail. It is never
/// in `added`, and one that was delivered before is reported in `removed`
/// and forgotten by the cursor. Forgetting it is what keeps this safe: if
/// the mark is taken off again the message is simply one the cursor has not
/// seen, and the next sync brings it back in `added` under the id it always
/// had. Nothing on the server is touched either way.
///
/// Any command the server refuses fails the sync. Half an answer is never
/// turned into a delta: what a failed command would have listed would
/// otherwise look removed.
async fn sync_folder<T: Wire>(
    session: &mut Session<T>,
    folder: &FolderId,
    cursor: Option<SyncCursor>,
    batch: usize,
) -> Result<SyncDelta> {
    let validity = open(session, &folder.0).await?;

    let previous = cursor.as_ref().and_then(Cursor::decode);
    // A new UIDVALIDITY means every UID we hold names some other message
    // now, or none. Start the folder again.
    let reset = previous.as_ref().is_some_and(|c| c.validity != validity);
    let known = match previous {
        Some(c) if c.validity == validity => c.known,
        _ => UidSet::default(),
    };

    let present = uid_search(session, "UNDELETED").await?;
    let message = |uid: u32| MessageRef {
        uid,
        validity,
        folder: folder.0.clone(),
    };

    // Newest first: the mail people want is the mail that just arrived.
    let mut missing: Vec<u32> = present
        .iter()
        .copied()
        .filter(|uid| !known.contains(*uid))
        .collect();
    missing.sort_unstable_by(|a, b| b.cmp(a));
    let has_more = missing.len() > batch;
    missing.truncate(batch);

    let wanted: HashSet<u32> = missing.iter().copied().collect();
    let mut added = Vec::with_capacity(missing.len());
    let mut fetched: HashSet<u32> = HashSet::with_capacity(missing.len());
    for set in UidSet::from_uids(missing.iter().copied()).sequence_sets(RANGES_PER_COMMAND) {
        let reply = Command::new("UID FETCH")
            .arg(&set)
            .arg(SUMMARY)
            .run_with(session, |response| {
                let Response::Fetch(_, attributes) = response else {
                    return;
                };
                let item = Fetched(attributes);
                // The server may slip in news of other messages, flags and
                // nothing else. Only an answer with everything in it, about
                // a message that was asked for, is a summary; the size
                // stands for "everything", since news never carries it.
                let (Some(uid), Some(_)) = (item.uid(), item.size()) else {
                    return;
                };
                if !wanted.contains(&uid) || fetched.contains(&uid) {
                    return;
                }
                let received = item.received().unwrap_or(0);
                let (flags, deleted) = item.flags().unwrap_or_default();
                // Marked since the search, a moment ago. Left unknown, so it
                // is looked at again next time.
                if deleted {
                    return;
                }
                fetched.insert(uid);
                added.push(summary(&item, message(uid).id(), flags, received));
            })
            .await?;
        reply.require("the server would not fetch the new messages")?;
    }
    added.sort_by_key(|message| std::cmp::Reverse(message.headers.date));

    // Flags of what we already had, once the folder has caught up. While a
    // first sync is still bringing mail in, the new mail matters more.
    let mut still_here: Vec<u32> = known.iter().filter(|uid| present.contains(uid)).collect();
    let mut changed = Vec::new();
    // Known messages found marked `\Deleted` after all.
    let mut marked: HashSet<u32> = HashSet::new();
    if !has_more {
        let asked: HashSet<u32> = still_here.iter().copied().collect();
        let mut latest: HashMap<u32, (Flags, bool)> = HashMap::with_capacity(asked.len());
        for set in UidSet::from_uids(still_here.iter().copied()).sequence_sets(RANGES_PER_COMMAND) {
            let reply = Command::new("UID FETCH")
                .arg(&set)
                .arg("(UID FLAGS)")
                .run_with(session, |response| {
                    if let Response::Fetch(_, attributes) = response {
                        let item = Fetched(attributes);
                        if let (Some(uid), Some(flags)) = (item.uid(), item.flags())
                            && asked.contains(&uid)
                        {
                            latest.insert(uid, flags);
                        }
                    }
                })
                .await?;
            reply.require("the server would not fetch the flags")?;
        }
        for uid in &still_here {
            match latest.get(uid) {
                Some((flags, false)) => changed.push(FlagChange {
                    id: message(*uid).id(),
                    flags: *flags,
                }),
                Some((_, true)) => {
                    marked.insert(*uid);
                }
                // Expunged since the search. The next sync reports it.
                None => {}
            }
        }
        still_here.retain(|uid| !marked.contains(uid));
    }

    let removed: Vec<RemoteId> = known
        .iter()
        .filter(|uid| !present.contains(uid) || marked.contains(uid))
        .map(|uid| message(uid).id())
        .collect();

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

/// What a FETCH reply says about one message.
struct Fetched<'a>(&'a [AttributeValue<'a>]);

impl<'a> Fetched<'a> {
    fn uid(&self) -> Option<u32> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::Uid(uid) => Some(*uid),
            _ => None,
        })
    }

    fn size(&self) -> Option<u32> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::Rfc822Size(size) => Some(*size),
            _ => None,
        })
    }

    /// The flags Quill tracks, and whether the message is marked
    /// `\Deleted`. `None` when the reply does not mention flags.
    fn flags(&self) -> Option<(Flags, bool)> {
        let names = self.0.iter().find_map(|value| match value {
            AttributeValue::Flags(names) => Some(names),
            _ => None,
        })?;
        let mut flags = Flags::default();
        let mut deleted = false;
        for name in names {
            // Flag names are not case-sensitive.
            match name.to_ascii_lowercase().as_str() {
                "\\seen" => flags.seen = true,
                "\\flagged" => flags.flagged = true,
                "\\answered" => flags.answered = true,
                "\\draft" => flags.draft = true,
                "\\deleted" => deleted = true,
                _ => {}
            }
        }
        Some((flags, deleted))
    }

    /// When the server received the message, in seconds since the Unix
    /// epoch.
    fn received(&self) -> Option<i64> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::InternalDate(date) => internal_date(date),
            _ => None,
        })
    }

    /// The header lines that were asked for.
    fn header(&self) -> Option<&'a [u8]> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Header)),
                data: Some(header),
                ..
            }
            | AttributeValue::Rfc822Header(Some(header)) => Some(header.as_ref()),
            _ => None,
        })
    }

    /// The whole message.
    fn body(&self) -> Option<&'a [u8]> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::BodySection {
                section: None,
                data: Some(body),
                ..
            }
            | AttributeValue::Rfc822(Some(body)) => Some(body.as_ref()),
            _ => None,
        })
    }

    fn structure(&self) -> Option<&'a BodyStructure<'a>> {
        self.0.iter().find_map(|value| match value {
            AttributeValue::BodyStructure(structure) => Some(structure),
            _ => None,
        })
    }
}

/// Reads an IMAP date-time, such as `14-Nov-2026 09:14:00 +0000`, as
/// seconds since the Unix epoch. A day below ten may be written with a
/// space in front.
fn internal_date(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut parts = text.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next()?);

    let mut date = date.split('-');
    let day: i64 = date.next()?.parse().ok()?;
    let name = date.next()?.to_ascii_lowercase();
    let month = i64::try_from(MONTHS.iter().position(|month| *month == name)?).ok()? + 1;
    let year: i64 = date.next()?.parse().ok()?;

    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;

    let (sign, digits) = match zone.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let offset: i64 = digits.parse().ok()?;
    let offset = sign * ((offset / 100) * 3600 + (offset % 100) * 60);

    if !(1..=31).contains(&day)
        || !(0..=9999).contains(&year)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    // Days since the epoch, counting years from March so that the leap day
    // is the last day of the year.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn summary(item: &Fetched<'_>, id: RemoteId, flags: Flags, received: i64) -> RemoteMessage {
    let parsed = item
        .header()
        .and_then(|raw| quill_mime::parse(raw, received));
    let thread = parsed
        .as_ref()
        .map(quill_mime::Parsed::thread)
        .unwrap_or_default();
    let headers = parsed.map_or_else(
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
        flags,
        size: u64::from(item.size().unwrap_or(0)),
        has_attachments: item.structure().is_some_and(has_attachments),
        thread,
    }
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
async fn select_for<T: Wire>(session: &mut Session<T>, message: &MessageRef) -> Result<()> {
    if open(session, &message.folder).await? != message.validity {
        return Err(Error::NotFound(format!(
            "{} was renumbered by the server; sync it again",
            message.folder
        )));
    }
    Ok(())
}

/// The whole message. `NotFound` only when the server looked and the
/// message was not there: a fetch it refused is an error of its own, and
/// says nothing about whether the message exists.
async fn fetch_raw<T: Wire>(session: &mut Session<T>, message: &MessageRef) -> Result<Bytes> {
    select_for(session, message).await?;
    let gone = || Error::NotFound(format!("message {} is gone", message.uid));
    if message.uid == 0 {
        // No message has UID 0, and asking for it is a syntax error.
        return Err(gone());
    }
    let mut body = None;
    // PEEK: reading a message here must not mark it read on the server.
    let reply = Command::new("UID FETCH")
        .arg(&message.uid.to_string())
        .arg("(UID BODY.PEEK[])")
        .run_with(session, |response| {
            if let Response::Fetch(_, attributes) = response {
                let item = Fetched(attributes);
                if item.uid() == Some(message.uid)
                    && let Some(bytes) = item.body()
                {
                    body = Some(Bytes::copy_from_slice(bytes));
                }
            }
        })
        .await?;
    reply.require("the server would not fetch the message")?;
    body.ok_or_else(gone)
}

async fn set_flags<T: Wire>(
    session: &mut Session<T>,
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
        let reply = Command::new("UID STORE")
            .arg(&message.uid.to_string())
            .arg(verb)
            .arg(&format!("({})", names.join(" ")))
            .run(session)
            .await?;
        // A change the server refused has not been made, and saying it had
        // would take it off the queue for good.
        reply.require("the server would not change the flags")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::{scripted, scripted_within, steps};

    const SELECTED: (&str, &str) = (
        "SELECT \"INBOX\"",
        "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\n{tag} OK [READ-WRITE] Selected\r\n",
    );

    fn inbox() -> FolderId {
        FolderId("INBOX".to_owned())
    }

    /// Plays the server's side of a sign-in: for each line expected (after
    /// the tag, where there is one), what to answer, with `{tag}` standing
    /// for the tag last seen.
    async fn signing_in(
        script: &'static [(&'static str, &'static str)],
        credentials: std::sync::Arc<dyn Credentials>,
    ) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (ours, theirs) = tokio::io::duplex(8 * 1024);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(theirs);
            let mut lines = BufReader::new(read);
            let mut tag = String::new();
            for (expect, answer) in script {
                let mut line = String::new();
                lines.read_line(&mut line).await.unwrap();
                let line = line.trim_end_matches("\r\n");
                let said = match line.split_once(' ') {
                    Some((theirs, rest)) if theirs.starts_with('A') => {
                        tag = theirs.to_owned();
                        rest
                    }
                    _ => line,
                };
                assert_eq!(said, *expect);
                write
                    .write_all(answer.replace("{tag}", &tag).as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = async_imap::Client::new(Bounded::new(ours, std::time::Duration::from_secs(5)));
        let outcome = authenticate(client, "robin@example.com", &*credentials)
            .await
            .map(|_| ());
        server.await.unwrap();
        outcome
    }

    /// Tokens that are renewed when one is refused: `first`, then `second`.
    struct Renewing {
        refused: std::sync::atomic::AtomicBool,
        second: &'static str,
    }

    #[async_trait]
    impl Credentials for Renewing {
        async fn get(&self) -> Result<Credential> {
            Ok(Credential::OAuthToken(
                if self.refused.load(Ordering::Relaxed) {
                    self.second.to_owned()
                } else {
                    "first".to_owned()
                },
            ))
        }

        async fn refused(&self) {
            self.refused.store(true, Ordering::Relaxed);
        }
    }

    // base64 of "user=robin@example.com^Aauth=Bearer first^A^A", and of
    // the same with "second".
    const FIRST: &str = "dXNlcj1yb2JpbkBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciBmaXJzdAEB";
    const SECOND: &str = "dXNlcj1yb2JpbkBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciBzZWNvbmQBAQ==";

    #[tokio::test]
    async fn a_token_signs_in_with_xoauth2() {
        let token: std::sync::Arc<dyn Credentials> =
            std::sync::Arc::new(|| Ok(Credential::OAuthToken("first".to_owned())));
        signing_in(
            &[
                ("AUTHENTICATE XOAUTH2", "+ \r\n"),
                (FIRST, "{tag} OK Welcome\r\n"),
            ],
            token,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_refused_token_is_renewed_and_tried_once_more() {
        // The server says why in a challenge and wants an empty line back
        // before it refuses.
        const REFUSAL: &[(&str, &str)] = &[
            ("AUTHENTICATE XOAUTH2", "+ \r\n"),
            (FIRST, "+ eyJzdGF0dXMiOiI0MDEifQ==\r\n"),
            ("", "{tag} NO Invalid credentials\r\n"),
            ("AUTHENTICATE XOAUTH2", "+ \r\n"),
            (SECOND, "{tag} OK Welcome\r\n"),
        ];
        let renewing = std::sync::Arc::new(Renewing {
            refused: false.into(),
            second: "second",
        });
        signing_in(REFUSAL, renewing).await.unwrap();

        // When renewing gives the same token again, that is the answer.
        let stuck = std::sync::Arc::new(Renewing {
            refused: false.into(),
            second: "first",
        });
        match signing_in(&REFUSAL[..3], stuck).await {
            Err(Error::Auth(text)) => assert!(text.contains("Invalid credentials"), "{text}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_password_signs_in_with_login_and_a_refusal_is_final() {
        signing_in(
            &[("LOGIN \"robin@example.com\" \"secret\"", "{tag} OK\r\n")],
            quill_core::credentials::password("secret"),
        )
        .await
        .unwrap();
        let refused = signing_in(
            &[(
                "LOGIN \"robin@example.com\" \"wrong\"",
                "{tag} NO Authentication failed\r\n",
            )],
            quill_core::credentials::password("wrong"),
        )
        .await;
        assert!(matches!(refused, Err(Error::Auth(_))), "{refused:?}");
    }

    fn message(uid: u32) -> MessageRef {
        MessageRef {
            uid,
            validity: 7,
            folder: "INBOX".to_owned(),
        }
    }

    /// A cursor that has seen messages 1 to 3: if a failed command were
    /// read as an empty answer, these are what would be reported removed.
    fn seen_three() -> Option<SyncCursor> {
        Some(
            Cursor {
                validity: 7,
                known: UidSet::from_uids([1, 2, 3]),
            }
            .encode(),
        )
    }

    fn fetch_of(uids: &str) -> String {
        format!("UID FETCH {uids} {SUMMARY}")
    }

    /// The server's full answer about a new message.
    fn summary_of(uid: u32, flags: &str, subject: &str) -> String {
        let header = format!("Subject: {subject}\r\n\r\n");
        format!(
            "* {uid} FETCH (UID {uid} FLAGS ({flags}) RFC822.SIZE 321 \
             INTERNALDATE \"14-Nov-2026 09:14:00 +0000\" \
             BODY[HEADER.FIELDS (DATE SUBJECT FROM TO CC MESSAGE-ID)] {{{}}}\r\n{header})\r\n",
            header.len()
        )
    }

    #[tokio::test]
    async fn a_search_the_server_refuses_fails_the_sync_and_removes_nothing() {
        for (answer, words) in [
            (
                "{tag} NO Search is unavailable\r\n",
                "Search is unavailable",
            ),
            ("{tag} BAD Unknown search key\r\n", "Unknown search key"),
            // With no words of its own from the server.
            ("{tag} NO\r\n", "would not search"),
        ] {
            let script = [SELECTED, ("UID SEARCH UNDELETED", answer)];
            let (mut session, server) = scripted(&script).await;
            match sync_folder(&mut session, &inbox(), seen_three(), 500).await {
                Err(Error::Backend(text)) => assert!(text.contains(words), "{text}"),
                other => panic!("{answer}: {other:?}"),
            }
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_connection_that_drops_in_a_search_is_a_connection_error() {
        // The script ends before the search is answered, and the server
        // hangs up.
        let script = [SELECTED];
        let (mut session, server) = scripted(&script).await;
        let synced = sync_folder(&mut session, &inbox(), seen_three(), 500).await;
        assert!(matches!(synced, Err(Error::Connection(_))), "{synced:?}");
        drop(session);
        // The server saw a search it had no answer for, which is the point.
        assert!(server.await.is_err());
    }

    #[tokio::test]
    async fn a_fetch_the_server_refuses_fails_the_sync() {
        let new_mail = fetch_of("4");
        // Refused while bringing in new mail.
        let script = [
            SELECTED,
            (
                "UID SEARCH UNDELETED",
                "* SEARCH 1 2 3 4\r\n{tag} OK Done\r\n",
            ),
            (new_mail.as_str(), "{tag} NO Fetch is unavailable\r\n"),
        ];
        let (mut session, server) = scripted(&script).await;
        match sync_folder(&mut session, &inbox(), seen_three(), 500).await {
            Err(Error::Backend(text)) => assert!(text.contains("Fetch is unavailable"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();

        // Refused while checking the flags of what was already here, with
        // part of the answer already sent.
        let script = [
            SELECTED,
            (
                "UID SEARCH UNDELETED",
                "* SEARCH 1 2 3\r\n{tag} OK Done\r\n",
            ),
            (
                "UID FETCH 1:3 (UID FLAGS)",
                "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n{tag} BAD Too busy\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        match sync_folder(&mut session, &inbox(), seen_three(), 500).await {
            Err(Error::Backend(text)) => assert!(text.contains("Too busy"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_folder_that_will_not_open_fails_the_sync() {
        for (answer, missing) in [
            ("{tag} NO No such mailbox\r\n", true),
            ("{tag} BAD Command not understood\r\n", false),
            // Opened, but with no word of which numbering its UIDs are in.
            ("* 3 EXISTS\r\n{tag} OK Selected\r\n", false),
        ] {
            let script = [("SELECT \"INBOX\"", answer)];
            let (mut session, server) = scripted(&script).await;
            let err = sync_folder(&mut session, &inbox(), seen_three(), 500)
                .await
                .unwrap_err();
            assert_eq!(matches!(err, Error::NotFound(_)), missing, "{err:?}");
            assert_eq!(matches!(err, Error::Backend(_)), !missing, "{err:?}");
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn mail_marked_deleted_between_two_commands_is_not_shown() {
        let new_mail = fetch_of("4:5");
        // The search saw 1, 3, 4 and 5 unmarked. By the time they are
        // fetched, 5 and 3 have been marked by another mail program.
        let arrived = format!(
            "{}{}{{tag}} OK Done\r\n",
            summary_of(5, "\\Deleted", "Marked just now"),
            summary_of(4, "\\Flagged", "Kept"),
        );
        let script = [
            SELECTED,
            (
                "UID SEARCH UNDELETED",
                "* SEARCH 1 3 4 5\r\n{tag} OK Done\r\n",
            ),
            (new_mail.as_str(), arrived.as_str()),
            (
                "UID FETCH 1,3 (UID FLAGS)",
                "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n\
                 * 2 FETCH (UID 3 FLAGS (\\Seen \\DELETED))\r\n{tag} OK Done\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        let delta = sync_folder(&mut session, &inbox(), seen_three(), 500)
            .await
            .unwrap();
        drop(session);
        server.await.unwrap();

        let subjects: Vec<&str> = delta
            .added
            .iter()
            .map(|message| message.headers.subject.as_str())
            .collect();
        assert_eq!(subjects, ["Kept"]);
        assert_eq!(delta.added[0].id, message(4).id());
        assert_eq!(delta.added[0].size, 321);
        assert_eq!(delta.added[0].headers.date, 1_794_647_640);
        assert!(delta.added[0].flags.flagged);
        // 2 was marked before the search, 3 after it. Both are gone from
        // the list, and from the cursor, so either comes back as new mail
        // if its mark is taken off.
        assert_eq!(delta.removed, [message(2).id(), message(3).id()]);
        assert_eq!(delta.changed.len(), 1);
        assert_eq!(delta.changed[0].id, message(1).id());
        assert_eq!(
            Cursor::decode(&delta.next_cursor).unwrap().known,
            UidSet::from_uids([1, 4])
        );
    }

    #[tokio::test]
    async fn news_of_other_messages_in_a_fetch_is_not_taken_for_a_summary() {
        let new_mail = fetch_of("4");
        let arrived = format!(
            "* 1 FETCH (FLAGS (\\Seen))\r\n* 9 FETCH (UID 9 FLAGS (\\Seen))\r\n\
             * 4 FETCH (UID 4 FLAGS (\\Answered))\r\n{}{{tag}} OK Done\r\n",
            summary_of(4, "", "The real one"),
        );
        let script = [
            SELECTED,
            ("UID SEARCH UNDELETED", "* SEARCH 4\r\n{tag} OK Done\r\n"),
            (new_mail.as_str(), arrived.as_str()),
        ];
        let (mut session, server) = scripted(&script).await;
        let delta = sync_folder(&mut session, &inbox(), None, 500)
            .await
            .unwrap();
        drop(session);
        server.await.unwrap();
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.added[0].headers.subject, "The real one");
        assert_eq!(delta.added[0].flags, Flags::default());
    }

    #[tokio::test]
    async fn a_flag_change_the_server_refuses_is_an_error() {
        let wanted = Flags {
            seen: true,
            ..Flags::default()
        };
        let script = [
            SELECTED,
            (
                "UID STORE 2 +FLAGS.SILENT (\\Seen)",
                "{tag} NO Mailbox is read-only\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        match set_flags(&mut session, &message(2), wanted).await {
            Err(Error::Backend(text)) => assert!(text.contains("read-only"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();

        // Refused on the second of its two commands.
        let script = [
            SELECTED,
            ("UID STORE 2 +FLAGS.SILENT (\\Seen)", "{tag} OK Done\r\n"),
            (
                "UID STORE 2 -FLAGS.SILENT (\\Flagged \\Answered \\Draft)",
                "{tag} BAD Not now\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        assert!(matches!(
            set_flags(&mut session, &message(2), wanted).await,
            Err(Error::Backend(_))
        ));
        drop(session);
        server.await.unwrap();

        // And a renumbered folder is still not touched at all.
        let script = [SELECTED];
        let (mut session, server) = scripted(&script).await;
        let stale = MessageRef {
            validity: 8,
            ..message(2)
        };
        assert!(matches!(
            set_flags(&mut session, &stale, wanted).await,
            Err(Error::NotFound(_))
        ));
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn only_a_fetch_that_worked_and_found_nothing_means_the_message_is_gone() {
        let asked = "UID FETCH 2 (UID BODY.PEEK[])";
        let cases: [(&str, Option<bool>); 5] = [
            // Refused: nothing is known about the message.
            ("{tag} NO Fetch is unavailable\r\n", Some(false)),
            ("{tag} BAD Not understood\r\n", Some(false)),
            // Carried out, with nothing to show: the message is gone.
            ("{tag} OK Done\r\n", Some(true)),
            // News of some other message is not the message asked for.
            (
                "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n{tag} OK Done\r\n",
                Some(true),
            ),
            (
                "* 2 FETCH (UID 2 BODY[] {18}\r\nSubject: Hello\r\n\r\n)\r\n{tag} OK Done\r\n",
                None,
            ),
        ];
        for (answer, not_found) in cases {
            let script = [SELECTED, (asked, answer)];
            let (mut session, server) = scripted(&script).await;
            let fetched = fetch_raw(&mut session, &message(2)).await;
            match (&fetched, not_found) {
                (Ok(bytes), None) => assert_eq!(bytes.as_ref(), b"Subject: Hello\r\n\r\n"),
                (Err(Error::NotFound(_)), Some(true)) | (Err(Error::Backend(_)), Some(false)) => {}
                _ => panic!("{answer}: {fetched:?}"),
            }
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_list_the_server_refuses_is_not_an_account_with_no_folders() {
        let script = [("LIST \"\" *", "{tag} NO Not now\r\n")];
        let (mut session, server) = scripted(&script).await;
        assert!(matches!(
            list_folders(&mut session).await,
            Err(Error::Backend(_))
        ));
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_server_that_stops_answering_is_given_up_on() {
        let patience = Duration::from_millis(80);
        // Silent from the first command, and silent halfway through one.
        let scripts: [&[(&str, &str)]; 2] = [
            &[("SELECT \"INBOX\"", "")],
            &[SELECTED, ("UID SEARCH UNDELETED", "* SEARCH 1 2 3\r\n")],
        ];
        for script in scripts {
            let (mut session, server) = scripted_within(script, patience).await;
            let started = std::time::Instant::now();
            match sync_folder(&mut session, &inbox(), seen_three(), 500).await {
                Err(Error::Connection(text)) => {
                    assert_eq!(text, "timed out waiting for the server");
                }
                other => panic!("{other:?}"),
            }
            assert!(started.elapsed() >= patience);
            assert!(started.elapsed() < Duration::from_secs(5));
            drop(session);
            server.await.unwrap();
        }

        let script = steps(&[
            &[SELECTED],
            &[(
                "UID FETCH 2 (UID BODY.PEEK[])",
                "* 2 FETCH (UID 2 BODY[] {18}\r\nSubj",
            )],
        ]);
        let (mut session, server) = scripted_within(&script, patience).await;
        assert!(matches!(
            fetch_raw(&mut session, &message(2)).await,
            Err(Error::Connection(_))
        ));
        drop(session);
        server.await.unwrap();
    }

    #[test]
    fn the_time_a_message_was_received_is_read_in_any_zone() {
        for (written, seconds) in [
            ("14-Nov-2026 09:14:00 +0000", 1_794_647_640),
            ("14-nov-2026 10:14:00 +0100", 1_794_647_640),
            ("14-Nov-2026 03:44:00 -0530", 1_794_647_640),
            (" 1-Jan-1970 00:00:00 +0000", 0),
            ("01-Jan-1970 00:00:00 +0000", 0),
            ("31-Dec-1969 23:59:59 +0000", -1),
            ("29-Feb-2024 12:00:00 +0000", 1_709_208_000),
            ("01-Mar-2024 00:00:00 +0000", 1_709_251_200),
            ("31-Dec-1999 23:59:60 +0000", 946_684_800),
        ] {
            assert_eq!(internal_date(written), Some(seconds), "{written}");
        }
        for nonsense in [
            "",
            "14-Nov-2026",
            "14-Nov-2026 09:14:00",
            "14-Nov-2026 09:14:00 UTC",
            "14-Nov-2026 09:14:00 +00",
            "14-Brumaire-2026 09:14:00 +0000",
            "32-Nov-2026 09:14:00 +0000",
            "14-Nov-2026 24:14:00 +0000",
            "14-Nov-2026 09:14 +0000",
        ] {
            assert_eq!(internal_date(nonsense), None, "{nonsense}");
        }
    }

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
