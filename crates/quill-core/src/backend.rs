//! The trait every mail protocol implements, so the sync engine and the UI
//! stay protocol-agnostic.

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::Result;
use crate::types::{
    Capabilities, Flags, FolderId, FolderMail, FolderRenamed, Operation, RemoteEvent, RemoteFolder,
    RemoteId, SyncCursor, SyncDelta,
};

/// The names in a folder's name as it is shown, outermost first: a `/`
/// puts a folder inside another (`Projects/2026`). Each must be a name:
/// a name with nothing on one side of a `/` is refused.
pub fn folder_path(name: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = name.split('/').map(str::trim).collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(crate::Error::Backend(
            "a folder's name needs something on each side of a /".to_owned(),
        ));
    }
    Ok(parts)
}

/// Who a message is from and to on the wire. Not the same as its headers:
/// `Bcc` recipients are here and nowhere in what is transmitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// The address bounces go to: the sender's own.
    pub from: String,
    /// Every recipient: To, Cc and Bcc together.
    pub to: Vec<String>,
}

/// Something that can send mail: SMTP submission, Microsoft Graph, EWS.
#[async_trait]
pub trait Sender: Send + Sync {
    /// Sends `raw`, a complete RFC 5322 message, to everyone in `envelope`.
    /// The sender removes any `Bcc` header before transmitting.
    ///
    /// `Ok` means the server accepted the message for every recipient. On
    /// `Error::Temporary`, `Error::Refused`, `Error::Recipients` and
    /// `Error::TooLarge` nothing was sent. `Error::Connection` is the one
    /// case that cannot promise that: if the connection drops after the
    /// whole message was handed over and before the server's answer, the
    /// server may have taken it. A retry after that can deliver twice,
    /// which is why every message carries a `Message-ID`.
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()>;
}

#[async_trait]
pub trait Backend: Send + Sync {
    /// Folders, flags, push, server search, threads.
    fn capabilities(&self) -> Capabilities;

    async fn list_folders(&self) -> Result<Vec<RemoteFolder>>;

    /// Added, changed and removed messages since `cursor`, plus the next cursor.
    /// `None` asks for the whole folder.
    async fn sync_folder(&self, folder: &FolderId, cursor: Option<SyncCursor>)
    -> Result<SyncDelta>;

    /// The full RFC 5322 message.
    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes>;

    /// Flag, move, copy or delete.
    async fn apply(&self, op: &Operation) -> Result<()>;

    /// IDLE, polling, or a no-op that returns at once.
    async fn watch(&self, tx: mpsc::Sender<RemoteEvent>) -> Result<()>;

    /// The whole message, for someone who is waiting to read it. The same
    /// as [`Backend::fetch_raw`], except that a backend whose other work
    /// shares one connection may have another for this, so that a message
    /// that is opened does not wait for a sync to finish what it has in
    /// hand.
    async fn fetch_raw_now(&self, id: &RemoteId) -> Result<Bytes> {
        self.fetch_raw(id).await
    }

    /// Moves a message to another folder and says what it is called there,
    /// when the server tells: with that the local copy can keep its body
    /// and simply take the new id. `None` means the move worked and the
    /// new id is not known; the next sync of the folder will find it.
    ///
    /// Moving a message that is no longer in its folder is `Ok(None)`, not
    /// an error: a replay after a crash must not fail because the first
    /// attempt had already worked.
    async fn move_message(&self, id: &RemoteId, to: &FolderId) -> Result<Option<RemoteId>> {
        self.apply(&Operation::Move {
            id: id.clone(),
            to: to.clone(),
        })
        .await
        .map(|()| None)
    }

    /// Several whole messages, in the order asked for: `None` for one that
    /// is no longer on the server. A backend for which every call is a new
    /// sign-in (POP3) does them all in one.
    async fn fetch_many(&self, ids: &[RemoteId]) -> Result<Vec<Option<Bytes>>> {
        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            messages.push(match self.fetch_raw(id).await {
                Ok(raw) => Some(raw),
                Err(crate::Error::NotFound(_)) => None,
                Err(other) => return Err(other),
            });
        }
        Ok(messages)
    }

    /// Removes several messages from the server for good. One that is
    /// already gone is not an error, so this is safe to repeat. On an
    /// error, assume nothing about which were removed.
    async fn delete_many(&self, ids: &[RemoteId]) -> Result<()> {
        for id in ids {
            self.apply(&Operation::Delete { id: id.clone() }).await?;
        }
        Ok(())
    }

    /// Makes a folder called `name` on the server, beside the ones it has,
    /// and says what it is known by there.
    ///
    /// `name` is as folders are shown: a `/` in it puts the folder inside
    /// another (`Projects/2026`), which is made too if it is not there.
    async fn create_folder(&self, name: &str) -> Result<RemoteFolder> {
        Err(crate::Error::Unsupported(format!(
            "a folder called {name} cannot be made on this kind of server from here"
        )))
    }

    /// Gives a folder another name, as folders are shown, and says every
    /// folder that is known by another name for it: the folder, and those
    /// inside it where their names say where they are.
    ///
    /// A name that is taken is refused, and nothing is changed.
    async fn rename_folder(&self, folder: &FolderId, name: &str) -> Result<Vec<FolderRenamed>> {
        let _ = folder;
        Err(crate::Error::Unsupported(format!(
            "a folder cannot be renamed {name} on this kind of server from here"
        )))
    }

    /// What a message is known by once its folder is known as `folder`,
    /// where a message's name says which folder it is in (IMAP). `None`
    /// where it does not, and the message is known as it was.
    fn message_in(&self, message: &RemoteId, folder: &FolderId) -> Option<RemoteId> {
        let _ = (message, folder);
        None
    }

    /// Deletes a folder, and says what became of the mail in it. Mail is
    /// never deleted with its folder: it is moved to `trash` first, or
    /// goes to the trash with the folder, or stays where the server keeps
    /// all mail. A folder with mail in it and nowhere for the mail to go
    /// is refused, and so is one with folders inside it.
    async fn delete_folder(
        &self,
        folder: &FolderId,
        trash: Option<&FolderId>,
    ) -> Result<FolderMail> {
        let _ = (folder, trash);
        Err(crate::Error::Unsupported(
            "a folder cannot be deleted on this kind of server from here".to_owned(),
        ))
    }

    /// Saves a complete message into a folder, as a sent copy or a draft,
    /// and says what it is called there when the server tells.
    async fn append(
        &self,
        folder: &FolderId,
        raw: &[u8],
        flags: Flags,
    ) -> Result<Option<RemoteId>> {
        let _ = (raw, flags);
        Err(crate::Error::Unsupported(format!(
            "messages cannot be saved into {}",
            folder.0
        )))
    }
}
