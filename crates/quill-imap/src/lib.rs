//! Quill's IMAP backend.
//!
//! [`ImapBackend`] implements `quill_core::Backend` over one connection that
//! it opens on demand and reopens after a failure. Folders sync in batches,
//! newest mail first, so a large mailbox is usable long before it has all
//! arrived.
//!
//! Transport security follows the account's setting exactly: implicit TLS,
//! STARTTLS with no fallback, or plaintext only when the account opts in.
//! Credentials are never sent before the channel is as secure as that
//! setting requires.

mod backend;
mod config;
mod connect;
mod names;
mod uidset;

pub use backend::{ImapBackend, PasswordProvider};
pub use config::{ImapConfig, Trust};
pub use uidset::UidSet;
