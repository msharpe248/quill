//! The trait every mail protocol implements, so the sync engine and the UI
//! stay protocol-agnostic.

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::Result;
use crate::types::{
    Capabilities, Flags, FolderId, Operation, RemoteEvent, RemoteFolder, RemoteId, SyncCursor,
    SyncDelta,
};

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
