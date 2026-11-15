//! The error type shared by backends.

/// An error from a mail backend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The folder or message named in the request does not exist remotely.
    #[error("not found: {0}")]
    NotFound(String),
    /// The backend failed for a reason of its own (network, protocol, server).
    #[error("backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, Error>;
