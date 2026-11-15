//! Quill's protocol-agnostic core: domain types, the [`Backend`] trait every
//! mail protocol implements, configuration, and a fixture backend for the demo
//! mailbox and tests.

pub mod backend;
pub mod config;
pub mod credentials;
pub mod error;
pub mod fixture;
pub mod links;
pub mod providers;
pub mod search;
pub mod token;
pub mod types;

pub use backend::{Backend, Envelope, Sender};
pub use credentials::{Credential, Credentials};
pub use error::{CertificateDetails, CertificateProblem, Error, RefusedRecipient, Result};
pub use token::TokenSource;
