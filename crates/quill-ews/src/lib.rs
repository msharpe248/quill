//! Exchange Web Services: mail on an on-premises Exchange server, read,
//! filed and sent over HTTPS with SOAP.
//!
//! Optional and late: Exchange Online is reached through Microsoft Graph
//! (`quill-graph`). EWS is for servers Graph does not reach.
//!
//! [`EwsBackend`] is both the account's [`quill_core::Backend`] and its
//! [`quill_core::Sender`]. It asks a [`quill_core::Credentials`] for the
//! account's password or token before every request and never keeps one.
//!
//! How EWS differs from a mail server, and what is done about it:
//!
//! - It signs in with a password over HTTPS (Basic authentication), or
//!   with an OAuth token where the server takes one. Windows sign-in
//!   (NTLM, Kerberos) is not spoken; a server that offers nothing else is
//!   reported as exactly that, and not as a wrong password.
//! - A folder is synced with `SyncFolderItems`. Where a sync stopped is a
//!   state the server hands out, kept as the folder's cursor. A new
//!   message and a changed one are both reported as added, and the store
//!   updates the ones it has; a change of the read mark alone is reported
//!   as that.
//! - A sync does not say who a message is to, nor its sender's address.
//!   Each message it reports is asked for again by id, a hundred to a
//!   request, before it is passed on.
//! - A sync gives a folder's messages in an order of the server's
//!   choosing, oldest first as a rule. The newest mail of a large folder
//!   cannot be asked for first.
//! - A mail folder can hold what is not mail: a contact, a task. Only
//!   messages, meeting messages and posts are listed; they are what
//!   Exchange can hand over as the bytes of a message.
//! - A message has a different id in every folder: moving one gives it a
//!   new id, which the server's answer to the move says.
//! - There is no "answered" mark to write. What a sync reports as
//!   `Flags::answered` is read from what Outlook last did with the
//!   message, and is passed over when flags are set.
//! - A message is sent as it stands, `Bcc` header included: Exchange takes
//!   its recipients from the headers, and leaves `Bcc` out of what it
//!   delivers.
//! - Exchange files its own copy of sent mail
//!   ([`EwsBackend::SAVES_SENT_COPY`]).
//! - Marking a message read or deleting it never sends a read receipt.
//! - Nothing is waited on for new mail. The sync engine polls.
//!
//! Every request asks for the schema of Exchange 2013 SP1, so that or a
//! later server is needed. The address of the server's EWS endpoint is
//! given, not found: there is no Autodiscover here.
//!
//! Answers are XML from a server that may be anyone's. They are read by a
//! parser that is held to a size, a depth and a number of elements,
//! refuses a document type, and expands no entity.

mod backend;
mod http;
mod mime;
mod request;
mod wire;
mod xml;

pub use backend::{EwsBackend, EwsConfig};
