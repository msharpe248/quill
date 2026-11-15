//! Access tokens, for the servers that take them in place of a password.
//!
//! A backend that signs in with OAuth does not know how its token is got or
//! renewed. It asks a [`TokenSource`] for one each time it needs to sign in,
//! and says so when a server refuses the one it was given.

use async_trait::async_trait;

use crate::Result;

/// Something that can produce a current access token for one account.
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// A token that is good for at least a little while yet, renewed first
    /// if the last one is about to run out.
    ///
    /// Fails with [`crate::Error::Auth`] when the account must be signed in
    /// to again by the user (the consent was withdrawn, or the renewal
    /// token has expired), and with [`crate::Error::Connection`] or
    /// [`crate::Error::Temporary`] when the token server could not be
    /// asked just now.
    async fn access_token(&self) -> Result<String>;

    /// A server refused the token last handed out. It is forgotten, so the
    /// next call fetches a new one instead of offering the same again.
    async fn invalidate(&self);
}
