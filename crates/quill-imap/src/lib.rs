//! Quill's IMAP backend.
//!
//! [`ImapBackend`] implements `quill_core::Backend` over one connection that
//! it opens on demand and reopens after a failure. Folders sync in batches,
//! newest mail first, so a large mailbox is usable long before it has all
//! arrived.
//!
//! Messages are moved, copied, deleted and saved in ways that are safe to
//! repeat, since the operation queue replays after a crash: see the `change`
//! module for what each one does on servers with and without `MOVE` and
//! `UIDPLUS`.
//!
//! Transport security follows the account's setting exactly: implicit TLS,
//! STARTTLS with no fallback, or plaintext only when the account opts in.
//! Credentials are never sent before the channel is as secure as that
//! setting requires.

mod backend;
mod change;
mod command;
mod config;
mod connect;
mod names;
mod uidset;

pub use backend::{ImapBackend, PasswordProvider};
pub use config::{ImapConfig, Trust};
use quill_core::Result;
pub use uidset::UidSet;

/// Finds out whether an IMAP server answers at `config`'s address in the
/// way its TLS mode asks: connects, reads the greeting, upgrades with
/// STARTTLS if that is the mode, and hangs up. Nothing is sent that needs a
/// password.
///
/// A certificate the system does not trust comes back as
/// `Error::Certificate`: there is a server, and the user decides about it.
pub async fn probe(config: &ImapConfig) -> Result<()> {
    connect::connect(config).await.map(drop)
}
