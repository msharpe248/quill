//! Domain types shared by every crate.
//!
//! Two families of identifier exist. Remote identifiers ([`FolderId`],
//! [`RemoteId`]) are whatever the server uses and are opaque strings. Local
//! identifiers (`Local*Id`) are row ids in the store and are what the UI holds.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! local_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub i64);
    };
}

local_id!(
    /// Row id of an account in the local store.
    LocalAccountId
);
local_id!(
    /// Row id of a folder in the local store.
    LocalFolderId
);
local_id!(
    /// Row id of a message in the local store.
    LocalMessageId
);

/// A folder's identifier on the server, unique within one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FolderId(pub String);

/// A message's identifier on the server, unique within one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteId(pub String);

/// An opaque position in a folder's change stream (a Graph delta link, an IMAP
/// MODSEQ, a POP3 UIDL snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursor(pub String);

/// One mailbox address with an optional display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn new(name: &str, email: &str) -> Self {
        Self {
            name: (!name.is_empty()).then(|| name.to_owned()),
            email: email.to_owned(),
        }
    }

    /// The display name when there is one, otherwise the address itself.
    pub fn display_name(&self) -> &str {
        match &self.name {
            Some(name) if !name.is_empty() => name,
            _ => &self.email,
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) if !name.is_empty() => write!(f, "{name} <{}>", self.email),
            _ => f.write_str(&self.email),
        }
    }
}

/// Per-message flags that every account type can represent, remotely or locally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
}

/// What a folder is for. Drives sidebar order and special handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FolderRole {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
    Other,
}

impl FolderRole {
    /// The stable name stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Drafts => "drafts",
            Self::Sent => "sent",
            Self::Archive => "archive",
            Self::Junk => "junk",
            Self::Trash => "trash",
            Self::Other => "other",
        }
    }

    /// Parses a stored name. Unknown names become [`FolderRole::Other`].
    pub fn from_stored(name: &str) -> Self {
        match name {
            "inbox" => Self::Inbox,
            "drafts" => Self::Drafts,
            "sent" => Self::Sent,
            "archive" => Self::Archive,
            "junk" => Self::Junk,
            "trash" => Self::Trash,
            _ => Self::Other,
        }
    }
}

/// A folder as the server reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    pub id: FolderId,
    pub name: String,
    pub role: FolderRole,
}

/// A folder that is known by another name on the server than it was: one
/// that was renamed, or one inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRenamed {
    /// What it was known by.
    pub from: FolderId,
    /// The folder as the server has it now.
    pub to: RemoteFolder,
}

/// What became of the mail in a folder that was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderMail {
    /// There was none.
    None,
    /// It was moved to the trash, this many messages, or went there with
    /// its folder.
    Trashed,
    /// The server keeps all mail in one place and a folder is only a
    /// label on it (Gmail): the mail is where it was, without the label.
    Kept,
}

/// The parsed header summary of a message (what IMAP calls the envelope).
///
/// Named `Headers` so that `Envelope` stays free for the SMTP envelope the
/// `Sender` trait takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headers {
    pub message_id: Option<String>,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Seconds since the Unix epoch, UTC.
    pub date: i64,
}

/// A message as a folder sync reports it: headers and flags, no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMessage {
    pub id: RemoteId,
    pub headers: Headers,
    pub flags: Flags,
    pub size: u64,
    pub has_attachments: bool,
    /// The messages this one follows in its conversation, by Message-ID
    /// with angle brackets: its `References`, oldest first, then its
    /// `In-Reply-To`. A backend that knows conversations some other way
    /// (Microsoft Graph) puts a name of its own for the conversation here.
    /// Empty for a message that starts one, or when the backend cannot
    /// say.
    pub thread: Vec<String>,
}

/// New flags for a message that already exists locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagChange {
    pub id: RemoteId,
    pub flags: Flags,
}

/// What changed in a folder since a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncDelta {
    /// Everything known about the folder is void and this delta replaces it.
    /// An IMAP server says so by changing the folder's UIDVALIDITY.
    pub reset: bool,
    pub added: Vec<RemoteMessage>,
    pub changed: Vec<FlagChange>,
    pub removed: Vec<RemoteId>,
    /// The folder has more to deliver: sync again from `next_cursor`. A large
    /// folder arrives in batches, newest first, so there is something to read
    /// long before the last of it has come.
    pub has_more: bool,
    pub next_cursor: SyncCursor,
}

/// A mutation to replay against a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    SetFlags { id: RemoteId, flags: Flags },
    Move { id: RemoteId, to: FolderId },
    Copy { id: RemoteId, to: FolderId },
    Delete { id: RemoteId },
}

/// A push notification from a backend's `watch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEvent {
    FolderChanged(FolderId),
}

/// What a backend can do, so the UI can degrade honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub folders: bool,
    pub flags: bool,
    pub push: bool,
    pub server_search: bool,
    pub threads: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_display_uses_name_when_present() {
        assert_eq!(
            Address::new("Ana Ruiz", "ana.ruiz@example.com").to_string(),
            "Ana Ruiz <ana.ruiz@example.com>"
        );
        assert_eq!(
            Address::new("", "ana.ruiz@example.com").to_string(),
            "ana.ruiz@example.com"
        );
        assert_eq!(
            Address::new("", "ana.ruiz@example.com").display_name(),
            "ana.ruiz@example.com"
        );
    }

    #[test]
    fn folder_role_round_trips_through_its_stored_name() {
        for role in [
            FolderRole::Inbox,
            FolderRole::Drafts,
            FolderRole::Sent,
            FolderRole::Archive,
            FolderRole::Junk,
            FolderRole::Trash,
            FolderRole::Other,
        ] {
            assert_eq!(FolderRole::from_stored(role.as_str()), role);
        }
        assert_eq!(FolderRole::from_stored("bogus"), FolderRole::Other);
    }
}
