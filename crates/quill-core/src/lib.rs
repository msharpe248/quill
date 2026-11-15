//! Quill's protocol-agnostic core: domain types, the [`Backend`] trait every
//! mail protocol implements, configuration, and a fixture backend for the demo
//! mailbox and tests.

pub mod backend;
pub mod config;
pub mod error;
pub mod fixture;
pub mod links;
pub mod types;

pub use backend::Backend;
pub use error::{CertificateProblem, Error, Result};
