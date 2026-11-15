//! Quill's SMTP submission client.
//!
//! [`SmtpSender`] implements `quill_core::Sender`. Each message goes over a
//! connection of its own: connect, secure the channel, sign in, hand the
//! message over, quit. Nothing is kept between sends.
//!
//! Transport security follows the account's setting exactly: implicit TLS,
//! STARTTLS with no fallback, or plaintext only when the account opts in.
//! Credentials and mail are never sent before the channel is as secure as
//! that setting requires, and nothing a server said before a STARTTLS
//! upgrade is believed after it.
//!
//! A message goes to everyone it is addressed to or to no one. If the server
//! refuses any recipient, nothing is sent and the refusals are reported one
//! by one.
//!
//! The client is written directly on `quill-net`'s streams. SMTP is a small
//! line protocol, and the rules above need a hand on every step of it.

mod auth;
mod capabilities;
mod config;
mod connection;
mod message;
mod reply;
mod scram;
mod sender;

pub use auth::{Credential, CredentialProvider, Mechanism};
pub use config::{DEFAULT_HELLO_NAME, SmtpAuth, SmtpConfig, Trust};
pub use sender::SmtpSender;
