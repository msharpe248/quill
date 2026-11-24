//! `Backend` and `Sender` for a Microsoft 365 or Outlook.com mailbox.
//!
//! Nothing is remembered between calls but which folder is the inbox and
//! its like. Where a folder sync stopped is the link the service gave for
//! carrying on, kept by the store as the folder's cursor.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use quill_core::types::{
    Capabilities, Flags, FolderId, FolderMail, FolderRenamed, FolderRole, Operation, RemoteEvent,
    RemoteFolder, RemoteId, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Envelope, Error, Result, Sender, TokenSource};
use serde_json::json;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::http::{Client, Reach, Request};
use crate::mime;
use crate::wire::{self, Identified, Message, Page};

/// How many folders to ask for at a time. Graph's own choice is ten.
const FOLDERS_PER_PAGE: usize = 100;

/// How many pages of one folder list are read before the list is taken to
/// be going round in circles.
const FOLDER_PAGES: usize = 1000;

/// How deep folders inside folders are followed.
const FOLDER_DEPTH: usize = 32;

/// Where the service is and how much to ask of it at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphConfig {
    /// `https://graph.microsoft.com/v1.0` unless a test points elsewhere.
    pub base_url: String,
    /// How many messages to ask for per page.
    pub page_size: usize,
}

impl GraphConfig {
    /// The worldwide service. Mailboxes in a national cloud have another.
    pub const BASE_URL: &'static str = "https://graph.microsoft.com/v1.0";
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            base_url: Self::BASE_URL.to_owned(),
            page_size: 200,
        }
    }
}

pub struct GraphBackend {
    client: Client,
    page_size: usize,
    /// The ids of the folders Graph knows by a fixed name (`inbox`,
    /// `sentitems`), as far as they have been found. They are asked for
    /// once: such a folder keeps its id for as long as the mailbox lives.
    well_known: Mutex<HashMap<&'static str, String>>,
}

impl fmt::Debug for GraphBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphBackend")
            .field("base_url", &self.client.base())
            .field("page_size", &self.page_size)
            .finish_non_exhaustive()
    }
}

impl GraphBackend {
    /// Graph files a copy of everything sent through it in Sent Items by
    /// itself, and a copy saved by Quill could only be an unsent draft (see
    /// [`Backend::append`] here). Whoever sets the account up for the sync
    /// engine passes this on, so that no second copy is asked for.
    pub const SAVES_SENT_COPY: bool = true;

    /// A backend for the service at `config.base_url`, which must be an
    /// `https` address: a token is never sent in the clear.
    pub fn new(config: GraphConfig, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        Self::build(config, Reach::Https, tokens)
    }

    /// A backend for a stand-in server on this machine, for tests. The
    /// address may be plain `http`, and must name the machine by a loopback
    /// address (`127.0.0.1`, `[::1]`): anything else is refused.
    pub fn loopback(config: GraphConfig, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        Self::build(config, Reach::Loopback, tokens)
    }

    fn build(config: GraphConfig, reach: Reach, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        Ok(Self {
            client: Client::new(&config.base_url, reach, tokens)?,
            // Graph gives at most a thousand messages at a time.
            page_size: config.page_size.clamp(1, 1000),
            well_known: Mutex::new(HashMap::new()),
        })
    }

    /// Sets how long one request may take from start to finish before the
    /// service is called unreachable. Five minutes unless set.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.client.set_timeout(timeout);
        self
    }

    /// What each well-known folder is for, by folder id.
    ///
    /// A mailbox need not have them all (the archive folder is made when it
    /// is first used), so one that is missing is asked for again next time.
    async fn roles(&self) -> Result<HashMap<String, FolderRole>> {
        let mut found = self.well_known.lock().await;
        for (name, _) in wire::WELL_KNOWN {
            if found.contains_key(name) {
                continue;
            }
            let url = self.client.url(&["me", "mailFolders", name], "$select=id");
            let reply = self.client.send(&Request::get(url)).await?;
            if reply.missing() {
                continue;
            }
            let folder: Identified = reply.json("find the account's standard folders")?;
            found.insert(name, folder.id);
        }
        Ok(wire::WELL_KNOWN
            .iter()
            .filter_map(|(name, role)| found.get(name).map(|id| (id.clone(), *role)))
            .collect())
    }

    /// The folders directly inside `parent`, or at the top of the mailbox.
    async fn folders_in(&self, parent: Option<&str>) -> Result<Vec<wire::Folder>> {
        const DOING: &str = "list the folders";
        let query = format!("$select=id,displayName,childFolderCount&$top={FOLDERS_PER_PAGE}");
        let mut url = match parent {
            Some(parent) => self
                .client
                .url(&["me", "mailFolders", parent, "childFolders"], &query),
            None => self.client.url(&["me", "mailFolders"], &query),
        };
        let mut folders = Vec::new();
        for _ in 0..FOLDER_PAGES {
            let reply = self.client.send(&Request::get(url)).await?;
            // Deleted since its parent was listed.
            if parent.is_some() && reply.missing() {
                return Ok(folders);
            }
            let page: Page<wire::Folder> = reply.json(DOING)?;
            folders.extend(page.value);
            match page.next {
                Some(next) => url = self.follow(&next, DOING)?,
                None => return Ok(folders),
            }
        }
        Err(Error::Backend(
            "Microsoft Graph's list of folders does not end".to_owned(),
        ))
    }

    /// Makes a folder called `name` inside `parent`, or at the top of the
    /// mailbox.
    async fn make_folder(&self, parent: Option<&str>, name: &str) -> Result<wire::Folder> {
        let url = match parent {
            Some(parent) => self
                .client
                .url(&["me", "mailFolders", parent, "childFolders"], ""),
            None => self.client.url(&["me", "mailFolders"], ""),
        };
        self.client
            .send(&Request::post(
                url,
                serde_json::json!({ "displayName": name }),
            ))
            .await?
            .json("make the folder")
    }

    /// A link out of the service's answer, as an address to ask next.
    fn follow(&self, link: &str, doing: &str) -> Result<reqwest::Url> {
        self.client.link(link).ok_or_else(|| {
            Error::Backend(format!(
                "Microsoft Graph answered with a link to another server while trying to {doing}, which is not followed"
            ))
        })
    }

    /// A request for one page of a folder's changes.
    fn changes(&self, url: reqwest::Url) -> Request {
        Request::get(url).prefer(format!("odata.maxpagesize={}", self.page_size))
    }

    /// One message's row for the list, or `None` if it is gone.
    async fn summary(&self, id: &str) -> Result<Option<Message>> {
        let url = self.client.url(
            &["me", "messages", id],
            &format!("$select={}", wire::SUMMARY),
        );
        let reply = self.client.send(&Request::get(url)).await?;
        if reply.missing() {
            return Ok(None);
        }
        reply.json("read a changed message").map(Some)
    }

    /// Whether the message is anywhere in the mailbox.
    async fn exists(&self, id: &RemoteId) -> Result<bool> {
        let url = self.client.url(&["me", "messages", &id.0], "$select=id");
        let reply = self.client.send(&Request::get(url)).await?;
        if reply.missing() {
            return Ok(false);
        }
        reply.done("look for the message").map(|()| true)
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
        let message = ["me", "messages", &id.0];
        let reply = self
            .client
            .send(&Request::get(
                self.client.url(&message, "$select=isRead,flag"),
            ))
            .await?;
        if reply.missing() {
            return Ok(());
        }
        let now: Message = reply.json("read the message's flags")?;
        let Some(change) = flag_change(&now, flags) else {
            return Ok(());
        };
        let reply = self
            .client
            .send(&Request::patch(self.client.url(&message, ""), change))
            .await?;
        if reply.missing() {
            return Ok(());
        }
        reply.done("change the message's flags")
    }

    async fn copy(&self, id: &RemoteId, to: &FolderId) -> Result<()> {
        let url = self.client.url(&["me", "messages", &id.0, "copy"], "");
        let reply = self
            .client
            .send(&Request::post(url, json!({ "destinationId": to.0 })))
            .await?;
        // Unlike a move, a message that is gone is an error here: nothing
        // says an earlier attempt copied it.
        reply.done("copy the message")
    }

    /// Removes a message from its folder for good: not to Deleted Items,
    /// which is a move. `Ok` when it is already gone.
    async fn delete(&self, id: &RemoteId) -> Result<()> {
        let url = self.client.url(&["me", "messages", &id.0], "");
        let reply = self.client.send(&Request::delete(url)).await?;
        if reply.missing() {
            return Ok(());
        }
        reply.done("delete the message")
    }
}

/// What to write so that a message's read mark and flag become `wanted`.
/// `None` when they already are.
///
/// A flag marked complete counts as not flagged and is left as it is.
/// Whether a message is a draft or was answered cannot be written.
fn flag_change(now: &Message, wanted: Flags) -> Option<serde_json::Value> {
    let mut change = serde_json::Map::new();
    if now.is_read != Some(wanted.seen) {
        change.insert("isRead".to_owned(), wanted.seen.into());
    }
    if now.flagged() != wanted.flagged {
        let status = if wanted.flagged {
            "flagged"
        } else {
            "notFlagged"
        };
        change.insert("flag".to_owned(), json!({ "flagStatus": status }));
    }
    (!change.is_empty()).then(|| change.into())
}

#[async_trait]
impl Backend for GraphBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            // Graph has no connection to hold open. Its change
            // notifications are sent to a public web address, which a mail
            // client on a Mac does not have. The sync engine polls.
            push: false,
            // Graph can search, but nothing here asks it to yet.
            server_search: false,
            // Graph groups mail into conversations, but a folder sync does
            // not report them, so threads are worked out locally as for
            // every other account.
            threads: false,
        }
    }

    /// Every mail folder, a folder inside another named `Parent/Child`, in
    /// the order the service lists them with children after their parent.
    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        let roles = self.roles().await?;
        let mut folders = Vec::new();
        // A folder listed twice would otherwise be synced twice, and one
        // listed inside itself for ever.
        let mut listed: HashSet<String> = HashSet::new();
        // The levels being walked, outermost first: what each folder name
        // on a level starts with, and the folders on it still to come.
        let mut levels = vec![(String::new(), self.folders_in(None).await?.into_iter())];
        while let Some((prefix, level)) = levels.last_mut() {
            let Some(folder) = level.next() else {
                levels.pop();
                continue;
            };
            if !listed.insert(folder.id.clone()) {
                continue;
            }
            let name = format!(
                "{prefix}{}",
                folder.display_name.as_deref().unwrap_or(&folder.id)
            );
            let has_children = folder.child_folder_count.unwrap_or(0) > 0;
            folders.push(RemoteFolder {
                role: roles.get(&folder.id).copied().unwrap_or(FolderRole::Other),
                id: FolderId(folder.id.clone()),
                name: name.clone(),
            });
            if has_children && levels.len() < FOLDER_DEPTH {
                let children = self.folders_in(Some(&folder.id)).await?;
                levels.push((format!("{name}/"), children.into_iter()));
            }
        }
        Ok(folders)
    }

    /// One page of what changed in a folder since `cursor`.
    ///
    /// The cursor is the link the service gave last time: the link to the
    /// next page while a round is under way (`has_more`), the link to the
    /// next round once it has finished.
    ///
    /// Graph does not say whether a message it reports is new to the folder
    /// or has only changed, so every message it reports is in `added`, as
    /// it is now. The store takes an `added` message it already holds as an
    /// update of that message. Nothing is ever in `changed`.
    ///
    /// When the service no longer keeps the place the cursor names, or the
    /// cursor is not a link to this service at all, the folder is read
    /// again from the start and the delta says `reset`.
    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        const DOING: &str = "read what changed in the folder";
        let resume = cursor
            .as_ref()
            .and_then(|cursor| self.client.link(&cursor.0));
        let mut reset = cursor.is_some() && resume.is_none();
        let mut reply = None;
        if let Some(url) = resume {
            let resumed = self.client.send(&self.changes(url)).await?;
            if resumed.expired() {
                reset = true;
            } else {
                reply = Some(resumed);
            }
        }
        let reply = match reply {
            Some(reply) => reply,
            None => {
                // Newest first, so there is something to read long before
                // the last of a large folder has come. Whatever is asked
                // for here the service carries along in its links.
                let url = self.client.url(
                    &["me", "mailFolders", &folder.0, "messages", "delta"],
                    &format!("$select={}&$orderby=receivedDateTime desc", wire::SUMMARY),
                );
                self.client.send(&self.changes(url)).await?
            }
        };

        let page: Page<Message> = reply.json(DOING)?;
        let (has_more, next) = match (page.next, page.delta) {
            (Some(next), _) => (true, next),
            (None, Some(delta)) => (false, delta),
            (None, None) => {
                return Err(Error::Backend(format!(
                    "Microsoft Graph did not say where to carry on from while trying to {DOING}"
                )));
            }
        };
        // Checked before it is kept: a cursor that led elsewhere would be
        // thrown away next time and the whole folder read again, every time.
        self.follow(&next, DOING)?;

        let mut added = Vec::with_capacity(page.value.len());
        let mut removed = Vec::new();
        for entry in page.value {
            if entry.removed.is_some() {
                removed.push(RemoteId(entry.id));
                continue;
            }
            let entry = if entry.whole() {
                entry
            } else {
                // Only what changed was sent. The rest is asked for, so
                // that the row is not overwritten with blanks.
                match self.summary(&entry.id).await? {
                    Some(whole) => whole,
                    // Gone since. A later delta says so.
                    None => continue,
                }
            };
            added.push(entry.into_remote());
        }
        Ok(SyncDelta {
            reset,
            added,
            changed: Vec::new(),
            removed,
            has_more,
            next_cursor: SyncCursor(next),
        })
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        let url = self.client.url(&["me", "messages", &id.0, "$value"], "");
        self.client
            .send(&Request::get(url))
            .await?
            .bytes("download the message")
    }

    async fn apply(&self, op: &Operation) -> Result<()> {
        match op {
            Operation::SetFlags { id, flags } => self.set_flags(id, *flags).await,
            Operation::Move { id, to } => self.move_message(id, to).await.map(drop),
            Operation::Copy { id, to } => self.copy(id, to).await,
            Operation::Delete { id } => self.delete(id).await,
        }
    }

    /// Graph has nothing to wait on. The sync engine polls.
    async fn watch(&self, _tx: mpsc::Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }

    /// With immutable ids the message is called the same in its new folder,
    /// and the service's answer says so.
    ///
    /// `Ok(None)` when the message is gone, so that a replay after a crash
    /// does not fail. "Not found" may also be about the folder it was to go
    /// to, and the service's words do not reliably tell the two apart, so
    /// the message is looked for before the answer is taken that way.
    async fn move_message(&self, id: &RemoteId, to: &FolderId) -> Result<Option<RemoteId>> {
        const DOING: &str = "move the message";
        let url = self.client.url(&["me", "messages", &id.0, "move"], "");
        let reply = self
            .client
            .send(&Request::post(url, json!({ "destinationId": to.0 })))
            .await?;
        if reply.missing() {
            return if self.exists(id).await? {
                Err(reply.error(DOING))
            } else {
                Ok(None)
            };
        }
        let moved = reply.bytes(DOING)?;
        // It has moved, whatever the answer looks like. Without an id in
        // it the next sync of the folder finds the message.
        Ok(serde_json::from_slice::<Identified>(&moved)
            .ok()
            .map(|message| RemoteId(message.id)))
    }

    /// A new folder at the top of the mailbox, beside the inbox, or
    /// inside the folder its name says (`Projects/2026`), which is made
    /// first if it is not there.
    async fn create_folder(&self, name: &str) -> Result<RemoteFolder> {
        let parts = quill_core::backend::folder_path(name)?;
        let (last, parents) = match parts.split_last() {
            Some(split) => split,
            None => return Err(Error::Backend("a folder needs a name".to_owned())),
        };
        // The folder it goes inside: what it is known by, and its name in
        // full as the service has it.
        let mut parent: Option<(String, String)> = None;
        if !parents.is_empty() {
            let listed = self.list_folders().await?;
            for part in parents {
                let path = match &parent {
                    Some((_, path)) => format!("{path}/{part}"),
                    None => (*part).to_owned(),
                };
                let found = listed
                    .iter()
                    .find(|folder| folder.name.to_lowercase() == path.to_lowercase());
                parent = Some(match found {
                    Some(found) => (found.id.0.clone(), found.name.clone()),
                    None => {
                        let inside = parent.as_ref().map(|(id, _)| id.as_str());
                        let made = self.make_folder(inside, part).await?;
                        (made.id, path)
                    }
                });
            }
        }
        let inside = parent.as_ref().map(|(id, _)| id.as_str());
        let made = self.make_folder(inside, last).await?;
        let own = made.display_name.unwrap_or_else(|| (*last).to_owned());
        Ok(RemoteFolder {
            name: match &parent {
                Some((_, path)) => format!("{path}/{own}"),
                None => own,
            },
            id: FolderId(made.id),
            role: FolderRole::Other,
        })
    }

    /// Gives a folder another name where it is. The service knows a
    /// folder by an id that a name does not change, so only the names
    /// shown are other: the folder's, and those of the folders inside it.
    async fn rename_folder(&self, folder: &FolderId, name: &str) -> Result<Vec<FolderRenamed>> {
        let parts = quill_core::backend::folder_path(name)?;
        let (last, parents) = match parts.split_last() {
            Some(split) => split,
            None => return Err(Error::Backend("a folder needs a name".to_owned())),
        };
        let listed = self.list_folders().await?;
        let Some(current) = listed.iter().find(|listed| listed.id == *folder) else {
            return Err(Error::NotFound(
                "the folder is no longer in the mailbox".to_owned(),
            ));
        };
        let within = current.name.rsplit_once('/').map(|(within, _)| within);
        if within.unwrap_or_default().to_lowercase() != parents.join("/").to_lowercase() {
            return Err(Error::Unsupported(
                "on this kind of account a folder is renamed where it is: it cannot be moved \
                 into another folder from here"
                    .to_owned(),
            ));
        }
        let renamed = match within {
            Some(within) => format!("{within}/{last}"),
            None => (*last).to_owned(),
        };
        if renamed == current.name {
            return Ok(Vec::new());
        }
        if listed
            .iter()
            .any(|other| other.id != *folder && other.name.to_lowercase() == renamed.to_lowercase())
        {
            return Err(Error::Backend(format!(
                "there is already a folder called {name}"
            )));
        }
        let url = self.client.url(&["me", "mailFolders", &folder.0], "");
        self.client
            .send(&Request::patch(
                url,
                serde_json::json!({ "displayName": last }),
            ))
            .await?
            .done("rename the folder")?;
        let inside = format!("{}/", current.name);
        Ok(listed
            .iter()
            .filter_map(|listed| {
                let name = if listed.id == *folder {
                    renamed.clone()
                } else {
                    format!("{renamed}/{}", listed.name.strip_prefix(&inside)?)
                };
                Some(FolderRenamed {
                    from: listed.id.clone(),
                    to: RemoteFolder {
                        id: listed.id.clone(),
                        name,
                        role: listed.role,
                    },
                })
            })
            .collect())
    }

    /// Deletes a folder by moving it into Deleted Items, with what is in
    /// it: nothing is deleted for good, and the mailbox's own Deleted
    /// Items is where the user finds it again. `trash` is not needed: the
    /// service knows the folder by a name of its own.
    async fn delete_folder(
        &self,
        folder: &FolderId,
        _trash: Option<&FolderId>,
    ) -> Result<FolderMail> {
        let url = self
            .client
            .url(&["me", "mailFolders", &folder.0, "move"], "");
        let reply = self
            .client
            .send(&Request::post(
                url,
                serde_json::json!({ "destinationId": wire::DELETED_ITEMS }),
            ))
            .await?;
        // Gone already: a second asking is not a failure.
        if reply.missing() {
            return Ok(FolderMail::None);
        }
        reply.done("delete the folder")?;
        Ok(FolderMail::Trashed)
    }

    /// Saves a draft into `folder`.
    ///
    /// Graph files a message it is handed as a draft, wherever it is put
    /// and whatever it is: it cannot be told the message was sent. So only
    /// drafts are saved this way. A copy of sent mail is refused, and is
    /// not needed: see [`GraphBackend::SAVES_SENT_COPY`].
    ///
    /// Like any append this cannot be made idempotent: a replay after a
    /// crash that followed the service's answer saves a second draft.
    async fn append(
        &self,
        folder: &FolderId,
        raw: &[u8],
        flags: Flags,
    ) -> Result<Option<RemoteId>> {
        if !flags.draft {
            return Err(Error::Unsupported(
                "Microsoft Graph files every message it is handed as an unsent draft, so a copy \
                 of sent mail cannot be saved through it; it keeps its own copy of what it sends"
                    .to_owned(),
            ));
        }
        let url = self
            .client
            .url(&["me", "mailFolders", &folder.0, "messages"], "");
        let created = self
            .client
            .send(&Request::post_message(url, raw))
            .await?
            .bytes("save the draft")?;
        // Saved, whatever the answer looks like.
        let Ok(created) = serde_json::from_slice::<Message>(&created) else {
            return Ok(None);
        };
        if let Some(change) = flag_change(&created, flags) {
            let url = self.client.url(&["me", "messages", &created.id], "");
            // The draft is saved. If its marks cannot be set, failing now
            // would have it saved a second time on the next try.
            let _ = self.client.send(&Request::patch(url, change)).await;
        }
        Ok(Some(RemoteId(created.id)))
    }
}

#[async_trait]
impl Sender for GraphBackend {
    /// Sends `raw` as it stands, its `Bcc` header included.
    ///
    /// Graph is not given an envelope. It delivers to the addresses written
    /// in the message's own `To`, `Cc` and `Bcc` headers, and leaves `Bcc`
    /// out of what the recipients get. So unlike SMTP the header must stay,
    /// and a recipient the headers do not name would get nothing: such a
    /// message is refused here, before anything is sent, instead of going
    /// to only some of the people it was meant for.
    ///
    /// The message is sent from the signed-in mailbox, whatever
    /// `envelope.from` says.
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()> {
        if let Some(address) = mime::unnamed(raw, &envelope.to) {
            return Err(Error::Unsupported(format!(
                "Microsoft Graph sends to the addresses in a message's To, Cc and Bcc headers, \
                 and {address} is in none of them"
            )));
        }
        let url = self.client.url(&["me", "sendMail"], "");
        self.client
            .send(&Request::post_message(url, raw))
            .await?
            .accepted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    const READ: Flags = Flags {
        seen: true,
        flagged: false,
        answered: false,
        draft: false,
    };

    #[test]
    fn only_what_differs_is_written() {
        let unread =
            message(json!({"id": "m", "isRead": false, "flag": {"flagStatus": "notFlagged"}}));
        assert_eq!(flag_change(&unread, READ), Some(json!({"isRead": true})));
        assert_eq!(flag_change(&unread, Flags::default()), None);
        assert_eq!(
            flag_change(
                &unread,
                Flags {
                    flagged: true,
                    ..Flags::default()
                }
            ),
            Some(json!({"flag": {"flagStatus": "flagged"}}))
        );

        let flagged =
            message(json!({"id": "m", "isRead": true, "flag": {"flagStatus": "flagged"}}));
        assert_eq!(
            flag_change(&flagged, Flags::default()),
            Some(json!({"isRead": false, "flag": {"flagStatus": "notFlagged"}}))
        );
    }

    #[test]
    fn a_completed_flag_is_left_alone_and_answered_is_never_written() {
        let complete =
            message(json!({"id": "m", "isRead": true, "flag": {"flagStatus": "complete"}}));
        assert_eq!(flag_change(&complete, READ), None);
        assert_eq!(
            flag_change(
                &complete,
                Flags {
                    answered: true,
                    draft: true,
                    ..READ
                }
            ),
            None
        );
    }

    #[test]
    fn the_page_size_is_kept_within_what_graph_gives() {
        struct NoTokens;
        #[async_trait]
        impl TokenSource for NoTokens {
            async fn access_token(&self) -> Result<String> {
                Err(Error::Auth("no token in this test".to_owned()))
            }
            async fn invalidate(&self) {}
        }
        let with = |page_size| {
            GraphBackend::new(
                GraphConfig {
                    page_size,
                    ..GraphConfig::default()
                },
                Arc::new(NoTokens),
            )
            .unwrap()
            .page_size
        };
        assert_eq!(with(0), 1);
        assert_eq!(with(50), 50);
        assert_eq!(with(1_000_000), 1000);
    }

    #[test]
    fn debug_output_names_the_service_and_nothing_secret() {
        struct OneToken;
        #[async_trait]
        impl TokenSource for OneToken {
            async fn access_token(&self) -> Result<String> {
                Ok("test-access-token-1".to_owned())
            }
            async fn invalidate(&self) {}
        }
        let backend = GraphBackend::new(GraphConfig::default(), Arc::new(OneToken)).unwrap();
        let shown = format!("{backend:?}");
        assert!(shown.contains("https://graph.microsoft.com/v1.0"));
        assert!(!shown.contains("token"));
    }
}
