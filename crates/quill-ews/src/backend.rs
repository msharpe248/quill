//! `Backend` and `Sender` for a mailbox on an Exchange server.
//!
//! Nothing is remembered between calls but which folder is the inbox and
//! its like. Where a folder sync stopped is the state the server gave for
//! carrying on, kept by the store as the folder's cursor.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use bytes::Bytes;
use quill_core::types::{
    Capabilities, FlagChange, Flags, FolderId, FolderRole, Operation, RemoteEvent, RemoteFolder,
    RemoteId, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Credentials, Envelope, Error, Result, Sender};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::http::{Client, Kind, Message, Reach, Trouble};
use crate::mime;
use crate::request::{self, Marks};
use crate::wire::{self, Folder, Item};
use crate::xml::{Element, Ns};

/// How many folders to ask for at a time.
const FOLDERS_PER_PAGE: usize = 256;

/// How many pages of the folder list are read before the list is taken to
/// be going round in circles.
const FOLDER_PAGES: usize = 1000;

/// How deep folders inside folders are followed.
const FOLDER_DEPTH: usize = 32;

/// The most changes Exchange gives in one round of a folder sync.
const CHANGES_PER_ROUND: usize = 512;

/// How many messages' list rows are asked for in one request.
const ROWS_PER_REQUEST: usize = 100;

/// How many whole messages are asked for in one request. Few, because all
/// of them come back in one answer, in memory.
const MESSAGES_PER_REQUEST: usize = 10;

/// How many messages are deleted in one request.
const DELETES_PER_REQUEST: usize = 100;

/// The longest sync state that is kept or sent back. Exchange's grows with
/// the folder, to some tens of kilobytes for a very large one.
const STATE_LIMIT: usize = 8 * 1024 * 1024;

/// Base64 as servers write it: with or without its padding.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Where the server is, who signs in to it, and how much to ask of it at a
/// time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EwsConfig {
    /// The address of the server's EWS endpoint, such as
    /// `https://mail.example.com/EWS/Exchange.asmx`.
    pub url: String,
    /// The name the account signs in with when it has a password: the
    /// address, or `DOMAIN\name`, as the server wants it. Not used when the
    /// account signs in with an OAuth token.
    pub username: String,
    /// How many changes to ask for in one round of a folder sync.
    pub page_size: usize,
}

impl EwsConfig {
    /// The server at `url`, signed in to as `username`, asked for as much
    /// at a time as it gives.
    pub fn new(url: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            username: username.into(),
            page_size: CHANGES_PER_ROUND,
        }
    }
}

/// The folders EWS knows by a fixed name, as far as they have been found.
#[derive(Default)]
struct WellKnown {
    /// Their ids, by that name. They are asked for once: such a folder
    /// keeps its id for as long as the mailbox lives.
    ids: HashMap<&'static str, String>,
    /// The server has never heard of an archive folder, and is not asked
    /// about one again.
    no_archive: bool,
}

pub struct EwsBackend {
    client: Client,
    page_size: usize,
    well_known: Mutex<WellKnown>,
}

impl fmt::Debug for EwsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EwsBackend")
            .field("url", &self.client.url())
            .field("page_size", &self.page_size)
            .finish_non_exhaustive()
    }
}

impl EwsBackend {
    /// Exchange files a copy of everything sent through it in Sent Items by
    /// itself: sending and filing are one request. Whoever sets the account
    /// up for the sync engine passes this on, so that no second copy is
    /// asked for.
    pub const SAVES_SENT_COPY: bool = true;

    /// A backend for the server at `config.url`, which must be an `https`
    /// address: neither a password nor a token is ever sent in the clear.
    pub fn new(config: EwsConfig, credentials: Arc<dyn Credentials>) -> Result<Self> {
        Self::build(config, Reach::Https, credentials)
    }

    /// A backend for a stand-in server on this machine, for tests. The
    /// address may be plain `http`, and must name the machine by a loopback
    /// address (`127.0.0.1`, `[::1]`): anything else is refused.
    pub fn loopback(config: EwsConfig, credentials: Arc<dyn Credentials>) -> Result<Self> {
        Self::build(config, Reach::Loopback, credentials)
    }

    fn build(config: EwsConfig, reach: Reach, credentials: Arc<dyn Credentials>) -> Result<Self> {
        Ok(Self {
            client: Client::new(&config.url, reach, &config.username, credentials)?,
            page_size: config.page_size.clamp(1, CHANGES_PER_ROUND),
            well_known: Mutex::new(WellKnown::default()),
        })
    }

    /// Sets how long one request may take from start to finish before the
    /// server is called unreachable. Five minutes unless set.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.client.set_timeout(timeout);
        self
    }

    /// Sets how many bytes of one answer are read before it is given up as
    /// too large. 256 MiB unless set: a message arrives as base64 inside
    /// one answer, a third larger than it is.
    pub fn with_answer_limit(mut self, bytes: usize) -> Self {
        self.client.set_limit(bytes);
        self
    }

    /// What each well-known folder is for, by folder id.
    ///
    /// A mailbox need not have them all, so one that is missing is asked
    /// for again next time.
    async fn roles(&self) -> Result<HashMap<String, FolderRole>> {
        const DOING: &str = "find the account's standard folders";
        let mut found = self.well_known.lock().await;
        let missing: Vec<&'static str> = wire::WELL_KNOWN
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !found.ids.contains_key(name))
            .collect();
        if !missing.is_empty() {
            let ids = self
                .client
                .call(&request::get_folders(&missing), |messages| {
                    each(messages, missing.len(), DOING)?
                        .iter()
                        .map(|message| {
                            if message.kind() == Kind::Missing {
                                return Ok(None);
                            }
                            message.done(DOING).map(first_folder_id)
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .await
                .map_err(|trouble| trouble.error(DOING))?;
            for (name, id) in missing.into_iter().zip(ids) {
                if let Some(id) = id {
                    found.ids.insert(name, id);
                }
            }
        }

        if !found.no_archive && !found.ids.contains_key(wire::ARCHIVE) {
            // Asked for by itself: a server that has never heard of the
            // name turns down the whole request it is in.
            let archive = self
                .client
                .call(&request::get_folders(&[wire::ARCHIVE]), |messages| {
                    let message = one(messages, DOING)?;
                    Ok(match message.kind() {
                        _ if message.ok() => Ok(first_folder_id(message.body)),
                        // Made when it is first used. Asked for again
                        // next time.
                        Kind::Missing => Ok(None),
                        Kind::Refusal => Err(()),
                        Kind::Passing | Kind::SignIn => return Err(message.error(DOING)),
                    })
                })
                .await;
            match archive {
                Ok(Ok(Some(id))) => {
                    found.ids.insert(wire::ARCHIVE, id);
                }
                Ok(Ok(None)) => {}
                Ok(Err(())) => found.no_archive = true,
                Err(Trouble::Fault(fault)) if fault.kind() == Kind::Refusal => {
                    found.no_archive = true;
                }
                Err(Trouble::Fault(fault)) if fault.kind() == Kind::Missing => {}
                Err(trouble) => return Err(trouble.error(DOING)),
            }
        }

        Ok(wire::WELL_KNOWN
            .iter()
            .copied()
            .chain([(wire::ARCHIVE, FolderRole::Archive)])
            .filter_map(|(name, role)| found.ids.get(name).map(|id| (id.clone(), role)))
            .collect())
    }

    /// Every folder under the top of the mailbox, mail or not, in the
    /// order the server lists them.
    async fn all_folders(&self) -> Result<Vec<Folder>> {
        const DOING: &str = "list the folders";
        let mut folders = Vec::new();
        let mut offset = 0;
        for _ in 0..FOLDER_PAGES {
            let (page, last, next) = self
                .client
                .call(
                    &request::find_folders(offset, FOLDERS_PER_PAGE),
                    |messages| {
                        let root = one(messages, DOING)?
                            .done(DOING)?
                            .child(Ns::Messages, "RootFolder")
                            .ok_or_else(|| incomplete(DOING))?;
                        let page: Vec<Folder> = root
                            .child(Ns::Types, "Folders")
                            .map(|folders| {
                                folders.children().iter().filter_map(Folder::read).collect()
                            })
                            .unwrap_or_default();
                        // A list that does not say whether it has ended has.
                        let last = root
                            .attribute("IncludesLastItemInRange")
                            .is_none_or(wire::is_true);
                        let next = root
                            .attribute("IndexedPagingOffset")
                            .and_then(|offset| offset.parse::<usize>().ok());
                        Ok((page, last, next))
                    },
                )
                .await
                .map_err(|trouble| trouble.error(DOING))?;
            let next = next.unwrap_or(offset + page.len());
            folders.extend(page);
            if last {
                return Ok(folders);
            }
            if next <= offset {
                break;
            }
            offset = next;
        }
        Err(Error::Backend(
            "Exchange's list of folders does not end".to_owned(),
        ))
    }

    /// One round of a folder's changes since `state`, or from the start.
    /// `None` when the server no longer knows what `state` refers to.
    async fn changes(&self, folder: &FolderId, state: Option<&str>) -> Result<Option<Round>> {
        const DOING: &str = "read what changed in the folder";
        self.client
            .call(
                &request::sync_items(&folder.0, state, self.page_size),
                |messages| {
                    let message = one(messages, DOING)?;
                    if message.code == "ErrorInvalidSyncStateData" {
                        return Ok(None);
                    }
                    Round::read(message.done(DOING)?).map(Some).ok_or_else(|| {
                        Error::Backend(format!(
                            "Exchange did not say where to carry on from while trying to {DOING}"
                        ))
                    })
                },
            )
            .await
            .map_err(|trouble| trouble.error(DOING))
    }

    /// The list rows of these messages, each as it is now, in the order
    /// asked for.
    async fn rows(&self, ids: &[&str]) -> Result<Vec<Row>> {
        const DOING: &str = "read the changed messages";
        let mut rows = Vec::with_capacity(ids.len());
        for batch in ids.chunks(ROWS_PER_REQUEST) {
            let part = self
                .client
                .call(&request::get_summaries(batch), |messages| {
                    each(messages, batch.len(), DOING)?
                        .iter()
                        .map(|message| {
                            if message.item_missing() {
                                return Ok(Row::Gone);
                            }
                            if message.ok() {
                                return Ok(first_item(message.body)
                                    .and_then(Item::read)
                                    .map_or(Row::Unknown, |item| Row::Whole(Box::new(item))));
                            }
                            match message.kind() {
                                // Exchange would not describe this one
                                // item this way. What the sync itself said
                                // of it has to do.
                                Kind::Refusal | Kind::Missing => Ok(Row::Unknown),
                                Kind::Passing | Kind::SignIn => Err(message.error(DOING)),
                            }
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .await
                .map_err(|trouble| trouble.error(DOING))?;
            rows.extend(part);
        }
        Ok(rows)
    }

    /// These messages whole, in the order asked for, in one request. An
    /// error in place of one that is no longer on the server.
    async fn whole(&self, ids: &[&str]) -> std::result::Result<Vec<Result<Bytes>>, Trouble> {
        const DOING: &str = "download the message";
        self.client
            .call(&request::get_mime(ids), |messages| {
                each(messages, ids.len(), DOING)?
                    .iter()
                    .map(|message| {
                        if message.item_missing() {
                            return Ok(Err(message.error(DOING)));
                        }
                        let content = first_item(message.done(DOING)?)
                            .and_then(|item| item.child(Ns::Types, "MimeContent"))
                            .ok_or_else(|| incomplete(DOING))?;
                        decode(content.text()).map(Ok).ok_or_else(|| {
                            Error::Backend(format!(
                                "Exchange's answer could not be read while trying to {DOING}: \
                                 the message is not base64"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .await
    }

    /// Makes the read mark and the flag of a message what `flags` says.
    ///
    /// The message is read first and only what differs is written. Written
    /// blind, a flag would lose the dates Outlook keeps with it each time
    /// the message was merely marked read, and one marked complete there
    /// would become not flagged at all.
    ///
    /// `Ok` when the message is gone: there is nothing left to mark.
    async fn set_flags(&self, id: &RemoteId, flags: Flags) -> Result<()> {
        const READING: &str = "read the message's flags";
        const WRITING: &str = "change the message's flags";
        let now = self
            .client
            .call(&request::get_marks(&id.0), |messages| {
                let message = one(messages, READING)?;
                if message.item_missing() {
                    return Ok(None);
                }
                first_item(message.done(READING)?)
                    .and_then(Item::read)
                    .map(Some)
                    .ok_or_else(|| incomplete(READING))
            })
            .await
            .map_err(|trouble| trouble.error(READING))?;
        let Some(now) = now else {
            return Ok(());
        };
        // A flag marked complete counts as not flagged and is left as it
        // is. Whether a message is a draft or was answered is not written.
        let read = (now.is_read() != flags.seen).then_some(flags.seen);
        let flagged = (now.flagged() != flags.flagged).then_some(flags.flagged);
        if read.is_none() && flagged.is_none() {
            return Ok(());
        }
        self.client
            .call(&request::update_marks(&id.0, read, flagged), |messages| {
                let message = one(messages, WRITING)?;
                if message.item_missing() {
                    return Ok(());
                }
                message.done(WRITING).map(drop)
            })
            .await
            .map_err(|trouble| trouble.error(WRITING))
    }

    async fn copy(&self, id: &RemoteId, to: &FolderId) -> Result<()> {
        const DOING: &str = "copy the message";
        self.client
            .call(&request::copy_item(&id.0, &to.0), |messages| {
                // Unlike a move, a message that is gone is an error here:
                // nothing says an earlier attempt copied it.
                one(messages, DOING)?.done(DOING).map(drop)
            })
            .await
            .map_err(|trouble| trouble.error(DOING))
    }
}

/// One round of a folder sync, as the server gave it.
struct Round {
    /// Where to carry on from.
    state: String,
    /// Nothing more is waiting behind this round.
    last: bool,
    changes: Vec<Change>,
}

enum Change {
    /// New to the folder, or changed in it: the store does not need to
    /// know which.
    Listed(Box<Item>),
    /// Only whether it has been read has changed.
    Read(String),
    Removed(String),
}

impl Round {
    /// `None` when there is no usable state in the answer: without one the
    /// next round could only start the folder over.
    fn read(answer: &Element<'_>) -> Option<Self> {
        let state: String = answer
            .child_text(Ns::Messages, "SyncState")?
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        if !usable(&state) {
            return None;
        }
        let listed = answer.child(Ns::Messages, "Changes");
        let changes: Vec<Change> = listed
            .map(Element::children)
            .unwrap_or_default()
            .iter()
            .filter_map(|change| {
                if change.is(Ns::Types, "Create") || change.is(Ns::Types, "Update") {
                    // What is not mail (a contact that strayed into a mail
                    // folder) is not listed.
                    change
                        .children()
                        .iter()
                        .find_map(Item::read)
                        .map(|item| Change::Listed(Box::new(item)))
                } else if change.is(Ns::Types, "Delete") {
                    wire::id_of(change.child(Ns::Types, "ItemId")).map(Change::Removed)
                } else if change.is(Ns::Types, "ReadFlagChange") {
                    wire::id_of(change.child(Ns::Types, "ItemId")).map(Change::Read)
                } else {
                    None
                }
            })
            .collect();
        // A round that does not say whether it is the last is, unless it
        // brought something: then the next one says.
        let last = match answer.child_text(Ns::Messages, "IncludesLastItemInRange") {
            Some(last) => wire::is_true(last),
            None => listed.is_none_or(|listed| listed.children().is_empty()),
        };
        Some(Self {
            state,
            last,
            changes,
        })
    }
}

/// What `GetItem` had to say about a message a sync named.
enum Row {
    Whole(Box<Item>),
    /// Gone since. A later round says so.
    Gone,
    /// It is there, and Exchange would not describe it.
    Unknown,
}

/// Whether `state` is something Exchange could have handed out: base64,
/// and not absurdly long.
fn usable(state: &str) -> bool {
    !state.is_empty()
        && state.len() <= STATE_LIMIT
        && state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// The answer to the one thing a request asked for.
fn one<'m, 'a>(messages: &'m [Message<'a>], doing: &str) -> Result<&'m Message<'a>> {
    each(messages, 1, doing).map(|messages| &messages[0])
}

/// The answers to the `count` things a request asked for. Fewer or more
/// cannot be matched to what was asked.
fn each<'m, 'a>(
    messages: &'m [Message<'a>],
    count: usize,
    doing: &str,
) -> Result<&'m [Message<'a>]> {
    if messages.len() == count {
        Ok(messages)
    } else {
        Err(Error::Backend(format!(
            "Exchange gave {} answers to {count} questions while trying to {doing}",
            messages.len()
        )))
    }
}

/// An answer that worked and does not hold what was asked for.
fn incomplete(doing: &str) -> Error {
    Error::Backend(format!(
        "Exchange's answer did not hold what was asked for while trying to {doing}"
    ))
}

/// The first item in an answer's `<m:Items>`, whatever kind it is.
fn first_item<'a>(answer: &'a Element<'a>) -> Option<&'a Element<'a>> {
    answer.child(Ns::Messages, "Items")?.children().first()
}

/// The id of the first folder in an answer's `<m:Folders>`.
fn first_folder_id(answer: &Element<'_>) -> Option<String> {
    let folder = answer.child(Ns::Messages, "Folders")?.children().first()?;
    wire::id_of(folder.child(Ns::Types, "FolderId"))
}

/// The bytes that base64 `text` stands for. Line breaks in it are passed
/// over; Exchange writes none, and something between it and here might.
fn decode(text: &str) -> Option<Bytes> {
    let decoded = if text.bytes().any(|byte| byte.is_ascii_whitespace()) {
        let packed: Vec<u8> = text
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        BASE64.decode(packed)
    } else {
        BASE64.decode(text)
    };
    decoded.ok().map(Bytes::from)
}

/// The names of `folders` that hold mail, a folder inside another named
/// `Parent/Child`, children after their parent.
fn named(folders: Vec<Folder>, roles: &HashMap<String, FolderRole>) -> Vec<RemoteFolder> {
    // A folder listed twice would otherwise be synced twice.
    let mut listed: HashSet<&str> = HashSet::new();
    let folders: Vec<&Folder> = folders
        .iter()
        .filter(|folder| listed.insert(&folder.id))
        .collect();
    let mut children: HashMap<&str, Vec<&Folder>> = HashMap::new();
    let mut top = Vec::new();
    for folder in &folders {
        match folder.parent.as_deref() {
            // Inside a folder that was listed. One that claims to be
            // inside itself, or inside a folder that is inside it, is
            // under no top folder and is never reached.
            Some(parent) if listed.contains(parent) => {
                children.entry(parent).or_default().push(folder);
            }
            _ => top.push(*folder),
        }
    }

    let mut remote = Vec::new();
    // The levels being walked, outermost first: what each folder name on
    // a level starts with, and the folders on it still to come.
    let mut levels = vec![(String::new(), top.into_iter())];
    while let Some((prefix, level)) = levels.last_mut() {
        let Some(folder) = level.next() else {
            levels.pop();
            continue;
        };
        let name = format!("{prefix}{}", folder.name);
        if folder.mail {
            remote.push(RemoteFolder {
                role: roles.get(&folder.id).copied().unwrap_or(FolderRole::Other),
                id: FolderId(folder.id.clone()),
                name: name.clone(),
            });
        }
        if levels.len() < FOLDER_DEPTH
            && let Some(inside) = children.remove(folder.id.as_str())
        {
            levels.push((format!("{name}/"), inside.into_iter()));
        }
    }
    remote
}

#[async_trait]
impl Backend for EwsBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            // EWS can keep a connection open and say when mail comes
            // (streaming notifications). That is not built: the sync
            // engine polls.
            push: false,
            // EWS can search, but nothing here asks it to yet.
            server_search: false,
            // Exchange groups mail into conversations, and a sync says
            // which each message is in; that goes into each message's
            // `thread`, and threads are worked out locally as for every
            // other account.
            threads: false,
        }
    }

    /// Every mail folder, a folder inside another named `Parent/Child`,
    /// children after their parent. Calendars, contacts and their like are
    /// left out.
    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        let roles = self.roles().await?;
        Ok(named(self.all_folders().await?, &roles))
    }

    /// One round of what changed in a folder since `cursor`.
    ///
    /// The cursor is the sync state the server gave last time, whether the
    /// round it ended was the last for now or not (`has_more`).
    ///
    /// A message that is new and one that has changed are both in `added`,
    /// as they are now: the store takes an `added` message it already
    /// holds as an update of that message. One whose read mark alone has
    /// changed is in `changed`.
    ///
    /// A sync names each message without saying who it is to, and names
    /// its sender without an address. So every message a round reports is
    /// asked for once more by id before it is passed on, a hundred to a
    /// request.
    ///
    /// Exchange gives a folder's messages in an order of its own choosing,
    /// oldest first as a rule: unlike the other backends this one cannot
    /// bring the newest mail of a large folder first.
    ///
    /// When the server no longer knows what the cursor refers to, or the
    /// cursor is not a sync state at all, the folder is read again from
    /// the start and the delta says `reset`.
    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        const DOING: &str = "read what changed in the folder";
        let state = cursor
            .as_ref()
            .map(|cursor| cursor.0.as_str())
            .filter(|state| usable(state));
        let mut reset = cursor.is_some() && state.is_none();
        let mut round = None;
        if let Some(state) = state {
            round = self.changes(folder, Some(state)).await?;
            reset = round.is_none();
        }
        let round = match round {
            Some(round) => round,
            None => self.changes(folder, None).await?.ok_or_else(|| {
                Error::Backend(format!(
                    "Exchange called a sync from the start out of date while trying to {DOING}"
                ))
            })?,
        };
        // The sync engine comes back with the new state for as long as
        // there is more, and would for ever.
        if !round.last && round.changes.is_empty() && !reset && state == Some(round.state.as_str())
        {
            return Err(Error::Backend(format!(
                "Exchange said there is more and gave nothing while trying to {DOING}"
            )));
        }

        let asked: Vec<&str> = round
            .changes
            .iter()
            .filter_map(|change| match change {
                Change::Listed(item) => Some(item.id.as_str()),
                Change::Read(id) => Some(id.as_str()),
                Change::Removed(_) => None,
            })
            .collect();
        let mut rows = self.rows(&asked).await?.into_iter();

        let mut added = Vec::new();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        for change in round.changes {
            match change {
                Change::Removed(id) => removed.push(RemoteId(id)),
                Change::Listed(listed) => match rows.next() {
                    Some(Row::Whole(whole)) => added.push(whole.into_remote()),
                    Some(Row::Gone) => {}
                    Some(Row::Unknown) | None => added.push(listed.into_remote()),
                },
                Change::Read(id) => {
                    if let Some(Row::Whole(whole)) = rows.next() {
                        changed.push(FlagChange {
                            id: RemoteId(id),
                            flags: whole.flags(),
                        });
                    }
                }
            }
        }
        Ok(SyncDelta {
            reset,
            added,
            changed,
            removed,
            has_more: !round.last,
            next_cursor: SyncCursor(round.state),
        })
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        const DOING: &str = "download the message";
        self.whole(&[&id.0])
            .await
            .map_err(|trouble| trouble.error(DOING))?
            .pop()
            .unwrap_or_else(|| Err(incomplete(DOING)))
    }

    async fn apply(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::SetFlags { id, flags } => self.set_flags(id, *flags).await,
            Operation::Move { id, to } => self.move_message(id, to).await.map(drop),
            Operation::Copy { id, to } => self.copy(id, to).await,
            Operation::Delete { id } => self.delete_many(std::slice::from_ref(id)).await,
        }
    }

    /// Nothing is waited on. The sync engine polls.
    async fn watch(&self, _tx: mpsc::Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }

    /// A message has another id in every folder it is in, and the server's
    /// answer says what it is called where it went.
    ///
    /// `Ok(None)` when the message is gone, so that a replay after a crash
    /// does not fail. A folder that is gone is an error: Exchange tells
    /// the two apart.
    async fn move_message(&self, id: &RemoteId, to: &FolderId) -> Result<Option<RemoteId>> {
        const DOING: &str = "move the message";
        self.client
            .call(&request::move_item(&id.0, &to.0), |messages| {
                let message = one(messages, DOING)?;
                if message.item_missing() {
                    return Ok(None);
                }
                // It has moved, whatever the answer looks like. Without an
                // id in it the next sync of the folder finds the message.
                Ok(first_item(message.done(DOING)?)
                    .and_then(|item| wire::id_of(item.child(Ns::Types, "ItemId")))
                    .map(RemoteId))
            })
            .await
            .map_err(|trouble| trouble.error(DOING))
    }

    /// Whole messages, ten to a request. When ten together are more than
    /// one answer may hold, they are asked for one by one.
    async fn fetch_many(&self, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
        const DOING: &str = "download the messages";
        let gone_or = |fetched: Result<Bytes>| match fetched {
            Ok(raw) => Ok(Some(raw)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(other) => Err(other),
        };
        let mut messages = Vec::with_capacity(ids.len());
        for batch in ids.chunks(MESSAGES_PER_REQUEST) {
            let asked: Vec<&str> = batch.iter().map(|id| id.0.as_str()).collect();
            match self.whole(&asked).await {
                Ok(fetched) => {
                    for fetched in fetched {
                        messages.push(gone_or(fetched)?);
                    }
                }
                Err(Trouble::TooLarge(_)) if batch.len() > 1 => {
                    for id in batch {
                        messages.push(gone_or(self.fetch_raw(id).await)?);
                    }
                }
                Err(trouble) => return Err(trouble.error(DOING)),
            }
        }
        Ok(messages)
    }

    /// Removes messages from their folder for good: not to Deleted Items,
    /// which is a move. A hundred to a request; one that is already gone
    /// is not an error.
    async fn delete_many(&self, ids: &[RemoteId]) -> Result<()> {
        const DOING: &str = "delete the message";
        for batch in ids.chunks(DELETES_PER_REQUEST) {
            let asked: Vec<&str> = batch.iter().map(|id| id.0.as_str()).collect();
            self.client
                .call(&request::delete_items(&asked), |messages| {
                    for message in each(messages, asked.len(), DOING)? {
                        if !message.item_missing() {
                            message.done(DOING)?;
                        }
                    }
                    Ok(())
                })
                .await
                .map_err(|trouble| trouble.error(DOING))?;
        }
        Ok(())
    }

    /// Saves a message into `folder`: a draft as a draft, anything else as
    /// a message that was sent or received, read or not and flagged or not
    /// as `flags` says. Whether it was answered cannot be said.
    ///
    /// Like any append this cannot be made idempotent: a replay after a
    /// crash that followed the server's answer saves a second copy.
    async fn append(
        &self,
        folder: &FolderId,
        raw: &[u8],
        flags: Flags,
    ) -> Result<Option<RemoteId>> {
        const DOING: &str = "save the message";
        let marks = Marks {
            read: flags.seen,
            flagged: flags.flagged,
            draft: flags.draft,
        };
        self.client
            .call(&request::save_item(&folder.0, raw, marks), |messages| {
                // Saved, whatever else the answer looks like.
                Ok(first_item(one(messages, DOING)?.done(DOING)?)
                    .and_then(|item| wire::id_of(item.child(Ns::Types, "ItemId")))
                    .map(RemoteId))
            })
            .await
            .map_err(|trouble| trouble.error(DOING))
    }
}

/// What became of a message handed over for sending, when the server did
/// not say it was sent.
fn not_sent(kind: Kind, words: &str) -> Error {
    match kind {
        // The server's own failure. It does not say whether the message
        // went before things went wrong, and the one error that makes no
        // promise about that is `Error::Connection`.
        Kind::Passing => Error::Connection(format!(
            "Exchange failed while it was given the message, and did not say whether it was \
             sent: {words}"
        )),
        Kind::SignIn => Error::Auth(format!("Exchange: {words}")),
        // Anything the server holds against the message or its sender is
        // a refusal: the message stays in the Outbox with the server's
        // words, and is not sent again by itself.
        Kind::Refusal | Kind::Missing => Error::Refused(format!("Exchange: {words}")),
    }
}

#[async_trait]
impl Sender for EwsBackend {
    /// Sends `raw` as it stands, its `Bcc` header included, and has
    /// Exchange file its copy in Sent Items.
    ///
    /// EWS is not given an envelope. Exchange delivers to the addresses
    /// written in the message's own `To`, `Cc` and `Bcc` headers, and
    /// leaves `Bcc` out of what the recipients get. So unlike SMTP the
    /// header must stay, and a recipient the headers do not name would get
    /// nothing: such a message is refused here, before anything is sent,
    /// instead of going to only some of the people it was meant for.
    ///
    /// The message is sent from the signed-in mailbox, whatever
    /// `envelope.from` says.
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()> {
        if let Some(address) = mime::unnamed(raw, &envelope.to) {
            return Err(Error::Unsupported(format!(
                "Exchange sends to the addresses in a message's To, Cc and Bcc headers, and \
                 {address} is in none of them"
            )));
        }
        let unclear = |why: &str| {
            Error::Connection(format!(
                "Exchange did not say whether the message was sent: {why}"
            ))
        };
        let sent = self
            .client
            .call(&request::send_item(raw), |messages| {
                Ok(match messages {
                    [message] if message.ok() => Ok(()),
                    // Too many messages in too short a time. This one was
                    // not taken, and will be later.
                    [message] if message.code == "ErrorSubmissionQuotaExceeded" => {
                        Err(Error::Temporary(format!("Exchange: {}", message.words())))
                    }
                    [message] => Err(not_sent(message.kind(), &message.words())),
                    _ => Err(unclear("its answer is not about one message")),
                })
            })
            .await;
        match sent {
            Ok(sent) => sent,
            // Not sent at all, or turned away at the door while busy.
            Err(Trouble::Error(err)) => Err(err),
            Err(Trouble::Fault(fault)) => Err(not_sent(fault.kind(), &fault.words())),
            Err(Trouble::Unreadable(why)) => Err(unclear(&why)),
            Err(Trouble::TooLarge(_)) => Err(unclear("its answer was too large to read")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    fn backend(page_size: usize) -> EwsBackend {
        EwsBackend::new(
            EwsConfig {
                page_size,
                ..EwsConfig::new(
                    "https://mail.example.com/EWS/Exchange.asmx",
                    "robin@example.com",
                )
            },
            quill_core::credentials::password("test-password-1"),
        )
        .unwrap()
    }

    #[test]
    fn the_page_size_is_kept_within_what_exchange_gives() {
        assert_eq!(backend(0).page_size, 1);
        assert_eq!(backend(50).page_size, 50);
        assert_eq!(backend(1_000_000).page_size, 512);
        assert_eq!(
            EwsConfig::new("https://mail.example.com/EWS/Exchange.asmx", "robin").page_size,
            512
        );
    }

    #[test]
    fn debug_output_names_the_server_and_nothing_secret() {
        let shown = format!("{:?}", backend(50));
        assert!(shown.contains("https://mail.example.com/EWS/Exchange.asmx"));
        assert!(!shown.contains("password"));
    }

    #[test]
    fn a_sync_state_is_base64_of_a_sane_length() {
        assert!(usable(
            "H4sIAAAAAAAEAO29B2AcSZYlJi9tynt/SvVK1+B0oQiAYBMk2JBAEOzBiM3mkuwdaUcjKasqgcplVmVdZhZAzO2dvPfee++999577733ujudTif33/8/XGZkAWz2zkrayZ4hgKrIHz9+fB8/Ih7/Hu8WZXqZ101RLT/7aHe881GaL6fVrFhefPbRuj3fPvjo9zh6/Hs8eXOSN9O6WLXU8MvrVV5nbVXTtw19/eL3T589f3T3zaunj5589ebLk2f0w+voKGX4T54/enD32ZOzs2enZyfPnn355cndV8+fnbx6dnf32enOiX559+zB/sP7H4+HX/5x/fJX0de//2/wJM/q/2EAgOE8BABAAA=="
        ));
        assert!(!usable(""));
        assert!(!usable("has space"));
        assert!(!usable("</m:SyncState>"));
        assert!(!usable("https://mail.example.com/EWS/Exchange.asmx"));
        assert!(usable(&"A".repeat(STATE_LIMIT)));
        assert!(!usable(&"A".repeat(STATE_LIMIT + 1)));
    }

    #[test]
    fn base64_is_read_with_or_without_padding_and_line_breaks() {
        assert_eq!(decode("SGVsbG8=").unwrap(), &b"Hello"[..]);
        assert_eq!(decode("SGVsbG8").unwrap(), &b"Hello"[..]);
        assert_eq!(decode("SGVs\r\nbG8=\n").unwrap(), &b"Hello"[..]);
        assert_eq!(decode("").unwrap(), &b""[..]);
        assert!(decode("not base64!").is_none());
        assert!(decode("SGVsbG8=SGVsbG8=").is_none());
    }

    fn folder(id: &str, parent: &str, name: &str, mail: bool) -> Folder {
        Folder {
            id: id.to_owned(),
            parent: (!parent.is_empty()).then(|| parent.to_owned()),
            name: name.to_owned(),
            mail,
        }
    }

    fn names(folders: Vec<Folder>) -> Vec<String> {
        named(folders, &HashMap::new())
            .into_iter()
            .map(|folder| folder.name)
            .collect()
    }

    #[test]
    fn folders_are_named_by_their_path_with_children_after_their_parent() {
        // As a server may list them: in no order at all.
        let listed = vec![
            folder("f-2026", "f-projects", "2026", true),
            folder("f-inbox", "f-top", "Inbox", true),
            folder("f-harbour", "f-2026", "Harbour", true),
            folder("f-projects", "f-inbox", "Projects", true),
            folder("f-calendar", "f-top", "Calendar", false),
            folder("f-sent", "f-top", "Sent Items", true),
            // Mail inside what is not mail keeps its place and its name.
            folder("f-invites", "f-calendar", "Invitations", true),
        ];
        assert_eq!(
            names(listed),
            [
                "Inbox",
                "Inbox/Projects",
                "Inbox/Projects/2026",
                "Inbox/Projects/2026/Harbour",
                "Calendar/Invitations",
                "Sent Items",
            ]
        );
    }

    #[test]
    fn a_folder_listed_twice_or_inside_itself_does_no_harm() {
        let listed = vec![
            folder("f-inbox", "f-top", "Inbox", true),
            folder("f-inbox", "f-top", "Inbox again", true),
            folder("f-self", "f-self", "Inside itself", true),
            folder("f-a", "f-b", "A", true),
            folder("f-b", "f-a", "B", true),
            folder("f-orphan", "", "No parent", true),
        ];
        assert_eq!(names(listed), ["Inbox", "No parent"]);
    }

    #[test]
    fn folders_are_followed_only_so_deep() {
        let mut listed = vec![folder("f-0", "f-top", "0", true)];
        for depth in 1..100 {
            listed.push(folder(
                &format!("f-{depth}"),
                &format!("f-{}", depth - 1),
                &depth.to_string(),
                true,
            ));
        }
        let names = names(listed);
        assert_eq!(names.len(), FOLDER_DEPTH);
        assert_eq!(names[1], "0/1");
    }

    #[test]
    fn roles_go_to_the_folders_with_those_ids() {
        let roles = HashMap::from([
            ("f-inbox".to_owned(), FolderRole::Inbox),
            ("f-calendar".to_owned(), FolderRole::Trash),
        ]);
        let remote = named(
            vec![
                folder("f-inbox", "f-top", "Posteingang", true),
                folder("f-other", "f-top", "Inbox", true),
                folder("f-calendar", "f-top", "Kalender", false),
            ],
            &roles,
        );
        assert_eq!(
            remote,
            [
                RemoteFolder {
                    id: FolderId("f-inbox".to_owned()),
                    name: "Posteingang".to_owned(),
                    role: FolderRole::Inbox,
                },
                RemoteFolder {
                    id: FolderId("f-other".to_owned()),
                    name: "Inbox".to_owned(),
                    role: FolderRole::Other,
                },
            ]
        );
    }

    fn round(inside: &str) -> Option<Round> {
        let text = format!(
            "<m:SyncFolderItemsResponseMessage ResponseClass=\"Success\" \
             xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" \
             xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">{inside}\
             </m:SyncFolderItemsResponseMessage>"
        );
        Round::read(&parse(&text).unwrap())
    }

    #[test]
    fn a_round_without_a_usable_state_is_no_round() {
        assert!(round("<m:Changes/>").is_none());
        assert!(round("<m:SyncState/>").is_none());
        assert!(round("<m:SyncState>not-a-state!</m:SyncState>").is_none());
        let wrapped = round("<m:SyncState>\n  H4sI\n  AAA=\n</m:SyncState>").unwrap();
        assert_eq!(wrapped.state, "H4sIAAA=");
    }

    #[test]
    fn a_round_that_does_not_say_whether_it_is_the_last_is_judged_by_what_it_brought() {
        let state = "<m:SyncState>H4sIAAA=</m:SyncState>";
        assert!(round(state).unwrap().last);
        assert!(round(&format!("{state}<m:Changes/>")).unwrap().last);
        let brought =
            format!("{state}<m:Changes><t:Delete><t:ItemId Id=\"m1\"/></t:Delete></m:Changes>");
        assert!(!round(&brought).unwrap().last);
        let said = format!(
            "{state}<m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>\
             <m:Changes><t:Delete><t:ItemId Id=\"m1\"/></t:Delete></m:Changes>"
        );
        assert!(round(&said).unwrap().last);
    }

    #[test]
    fn changes_that_cannot_be_used_are_passed_over() {
        let round = round(
            "<m:SyncState>H4sIAAA=</m:SyncState>\
             <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>\
             <m:Changes>\
             <t:Create><t:Contact><t:ItemId Id=\"c1\"/></t:Contact></t:Create>\
             <t:Create/>\
             <t:Delete/>\
             <t:ReadFlagChange><t:IsRead>true</t:IsRead></t:ReadFlagChange>\
             <t:Unheard><t:ItemId Id=\"x\"/></t:Unheard>\
             <t:Update><t:Message><t:ItemId Id=\"m1\"/></t:Message></t:Update>\
             </m:Changes>",
        )
        .unwrap();
        assert_eq!(round.changes.len(), 1);
        assert!(matches!(&round.changes[0], Change::Listed(item) if item.id == "m1"));
    }
}
