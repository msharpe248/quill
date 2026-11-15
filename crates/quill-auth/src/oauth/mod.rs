//! Signing in with OAuth 2.0.
//!
//! Microsoft 365 and Gmail do not take a password from a mail program.
//! The user signs in to the provider in a browser, the provider hands Quill
//! a refresh token, and from then on Quill exchanges that for short-lived
//! access tokens without the user being involved.
//!
//! There are two ways to sign in. With a browser on this computer:
//! authorization code with PKCE, the redirect received on a loopback port
//! ([`BrowserSignIn`]). Without one, as over SSH: a code typed into the
//! provider's page on some other device ([`DeviceSignIn`]). Both end the
//! same way, in [`sign_in`]: the refresh token goes into the
//! [`SecretStore`] and the caller gets an [`OAuthTokens`], which is the
//! [`quill_core::TokenSource`] the account's backends ask for tokens. On
//! later runs [`OAuthTokens::resume`] picks up from what was stored.
//!
//! The flows are written directly on `reqwest`. They are three form posts
//! and one tiny listener, and writing them here keeps every error message
//! and every copy of a token in Quill's hands. Nothing in this module logs,
//! and no token, code, verifier or client secret is ever part of an error
//! or of a `Debug` rendering.
//!
//! Only the provider's own endpoints are ever contacted, and only over
//! `https` (or `http` to this computer, for tests). Nothing here opens a
//! browser or draws anything: the caller is handed the address or the code
//! and decides how to show it.

mod code;
mod device;
mod http;
mod loopback;
mod pkce;
mod provider;
mod secret;
mod tokens;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use quill_core::{Error, Result};

pub use code::{BROWSER_WAIT, BrowserSignIn, OpenBrowser};
pub use device::{DevicePrompt, DeviceSignIn, ShowDeviceCode};
pub use http::OAuthHttp;
pub use provider::{OAuthClient, OAuthProvider, RedirectHost};
pub use secret::Secret;
pub use tokens::OAuthTokens;

use crate::SecretStore;

/// How the user is to sign in.
#[derive(Debug)]
pub enum SignInMethod {
    /// In a browser on this computer.
    Browser(BrowserSignIn),
    /// By a code typed into the provider's page on any device.
    DeviceCode(DeviceSignIn),
}

/// Signs `account` in to `provider`, stores its refresh token in `store`
/// and returns its tokens, ready to use.
///
/// `account` names the sign-in in the store (see [`crate::oauth_name`]).
/// Use the mail address: it does not change when the user renames the
/// account in their config.
///
/// This waits for the user, for minutes. Dropping the future cancels the
/// sign-in: the listener closes, the polling stops, and nothing is stored.
///
/// Fails with [`Error::Auth`] when the user or the provider said no, or
/// nobody finished in time; [`Error::Connection`] or [`Error::Temporary`]
/// when the provider could not be asked; [`Error::Insecure`],
/// [`Error::Unsupported`] or [`Error::Backend`] when the provider
/// description, the client or this computer is the problem.
pub async fn sign_in(
    http: &OAuthHttp,
    provider: &OAuthProvider,
    client: &OAuthClient,
    method: SignInMethod,
    store: Arc<dyn SecretStore>,
    account: &str,
) -> Result<OAuthTokens> {
    // Everything that can be wrong without asking anyone is found first,
    // so the user is not sent to a browser for a sign-in that cannot end.
    let endpoints = provider.endpoints()?;
    client.check()?;
    if account.trim().is_empty() {
        return Err(Error::Backend(
            "an account needs a name to keep its sign-in under".to_owned(),
        ));
    }
    let grant = match method {
        SignInMethod::Browser(request) => {
            code::authorize(http, provider, &endpoints, client, request).await?
        }
        SignInMethod::DeviceCode(request) => {
            let authorization = device::start(http, provider, &endpoints, client).await?;
            (request.show)(&authorization.prompt);
            device::poll(http, &endpoints, client, &authorization).await?
        }
    };
    OAuthTokens::begin(http, provider, client, store, account, grant).await
}
