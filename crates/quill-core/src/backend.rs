//! The trait every mail protocol implements, so the sync engine and the UI
//! stay protocol-agnostic.

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc::Sender;

use crate::Result;
use crate::types::{
    Capabilities, FolderId, Operation, RemoteEvent, RemoteFolder, RemoteId, SyncCursor, SyncDelta,
};

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
    async fn watch(&self, tx: Sender<RemoteEvent>) -> Result<()>;
}
