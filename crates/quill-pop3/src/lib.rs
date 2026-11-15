//! Quill's POP3 backend.
//!
//! [`Pop3Backend`] implements `quill_core::Backend` with a small client of
//! its own. POP3 has one mailbox, no flags and no folders; what the server
//! does offer is `UIDL`, a stable id for each message, and every sync
//! compares that list with the ids already delivered.
//!
//! A POP3 session holds a lock on the mailbox, so no session outlives the
//! call that opened it: connect, do the work, `QUIT`.
//!
//! Syncing never deletes. Messages leave the server only through
//! `apply(Operation::Delete)` or [`Pop3Backend::delete_many`], and whether
//! mail is left on the server, deleted after some days or deleted once
//! downloaded is the sync engine's decision, not this crate's.
//!
//! Transport security follows the account's setting exactly: implicit TLS,
//! `STLS` with no fallback, or plaintext only when the account opts in.
//! Credentials are never sent before the channel is as secure as that
//! setting requires.

mod backend;
mod client;
mod config;
mod cursor;
mod wire;

pub use backend::{MailboxStatus, PasswordProvider, Pop3Backend};
pub use config::{Pop3Config, Trust};
