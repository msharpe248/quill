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
