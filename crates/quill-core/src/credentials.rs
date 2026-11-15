//! What an account signs in to a mail server with, and where it comes from.
//!
//! A backend does not know whether its account has a password in the
//! Keychain or a token renewed from an OAuth provider. It asks a
//! [`Credentials`] at each sign-in, once the channel is secure, keeps
//! nothing, and says so when the server refuses what it was given.

use std::fmt;

use async_trait::async_trait;

use crate::Result;

/// The secret an account signs in with.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Password(String),
    /// An OAuth access token, good for a while yet. Renewing it is the
    /// business of whoever handed it over.
    OAuthToken(String),
}

impl fmt::Debug for Credential {
    /// Never the secret itself: this ends up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Password(_) => "Password(..)",
            Self::OAuthToken(_) => "OAuthToken(..)",
        })
    }
}

/// Where an account's secret comes from: the Keychain or an OAuth provider
/// in the app, a constant in tests.
#[async_trait]
pub trait Credentials: Send + Sync {
    /// The secret to sign in with now.
    ///
    /// Fails with [`crate::Error::Auth`] when there is none and the user
    /// must supply one (no password saved, not signed in), and with
    /// [`crate::Error::Connection`] or [`crate::Error::Temporary`] when a
    /// token could not be renewed just now.
    async fn get(&self) -> Result<Credential>;

    /// A server refused the secret last handed out. A token is forgotten,
    /// so that the next [`Credentials::get`] renews it; a password is what
    /// it is.
    async fn refused(&self) {}
}

/// A function is a source of secrets: `|| Ok(Credential::Password(..))`.
#[async_trait]
impl<F> Credentials for F
where
    F: Fn() -> Result<Credential> + Send + Sync,
{
    async fn get(&self) -> Result<Credential> {
        self()
    }
}

/// The same password every time. For tests, and for trying out settings
/// with a password that has just been typed.
pub fn password(password: impl Into<String>) -> std::sync::Arc<dyn Credentials> {
    let password = password.into();
    std::sync::Arc::new(move || Ok(Credential::Password(password.clone())))
}

/// The line XOAUTH2 signs in with, before it is base64-encoded: the user,
/// the token, and the control characters between them.
pub fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_never_printed() {
        let secret = Credential::Password("hunter2".to_owned());
        assert_eq!(format!("{secret:?}"), "Password(..)");
        let token = Credential::OAuthToken("ya29.token".to_owned());
        assert_eq!(format!("{token:?}"), "OAuthToken(..)");
    }

    #[test]
    fn xoauth2_is_user_and_token_between_control_characters() {
        assert_eq!(
            xoauth2("robin@example.com", "tok"),
            "user=robin@example.com\u{1}auth=Bearer tok\u{1}\u{1}"
        );
    }
}
