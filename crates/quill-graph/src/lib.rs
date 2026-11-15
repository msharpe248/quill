//! Microsoft Graph: mail in Microsoft 365 and Outlook.com mailboxes, read,
//! filed and sent over HTTPS with an OAuth token.
//!
//! [`GraphBackend`] is both the account's [`quill_core::Backend`] and its
//! [`quill_core::Sender`]. It gets its token from a
//! [`quill_core::TokenSource`] before every request and never keeps one.
//!
//! How Graph differs from a mail server, and what is done about it:
//!
//! - A folder is synced with `messages/delta`. Where a sync stopped is a
//!   link, kept as the folder's cursor. Graph does not tell a new message
//!   from a changed one, so both are reported as added and the store
//!   updates the ones it has.
//! - Every request asks for immutable ids, so a message keeps its id when
//!   it moves to another folder.
//! - There is no "answered" mark to read or write. `Flags::answered` is
//!   always false in what a sync reports, and is passed over when flags
//!   are set.
//! - A message does not say how large it is. `RemoteMessage::size` is 0.
//! - A message is sent as it stands, `Bcc` header included: Graph takes
//!   its recipients from the headers, and leaves `Bcc` out of what it
//!   delivers.
//! - Graph files its own copy of sent mail
//!   ([`GraphBackend::SAVES_SENT_COPY`]), and can save nothing else but
//!   drafts.
//! - There is nothing to wait on for new mail. The sync engine polls.

mod backend;
mod http;
mod mime;
mod wire;

pub use backend::{GraphBackend, GraphConfig};
