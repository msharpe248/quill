//! One account's tokens, kept current.
//!
//! The access token is what mail servers are shown. It lasts about an hour
//! and is held in memory only. The refresh token is what the next access
//! token is got with. It lasts for months, so it is the thing worth
//! stealing: it is kept in the [`SecretStore`] and nowhere else, read from
//! there each time it is needed and not held in between.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use quill_core::{Error, Result, TokenSource};
use reqwest::Url;
use tokio::time::Instant;

use super::http::{Grant, OAuthHttp, Refusal};
use super::provider::{OAuthClient, OAuthProvider};
use super::secret::Secret;
use crate::{AuthError, SecretStore, oauth_name};

/// A token with less than this left is renewed before it is handed out, so
/// that it does not run out between being asked for and being used.
const MARGIN: Duration = Duration::from_secs(60);

/// The tokens of one account at one provider. This is the
/// [`TokenSource`] its backends ask.
///
/// It renews the access token by itself, with the stored refresh token, for
/// as long as the provider goes on accepting that. When the provider stops,
/// [`TokenSource::access_token`] fails with [`Error::Auth`] and the user
/// has to sign in again; until then nobody is asked anything.
pub struct OAuthTokens {
    /// Shared with the task that renews, which outlives a caller that
    /// stops waiting for it.
    inner: Arc<Inner>,
}

struct Inner {
    http: OAuthHttp,
    token_url: Url,
    client: OAuthClient,
    store: Arc<dyn SecretStore>,
    /// The name the refresh token is stored under.
    name: String,
    account: String,
    access: Mutex<Option<Access>>,
    /// Held while the token server is being asked, so that callers who all
    /// find the token stale at once make one request between them.
    renewing: tokio::sync::Mutex<()>,
}

/// An access token and when to stop using it.
struct Access {
    token: Secret,
    /// By the clock that only runs forward, which is the one to trust while
    /// the computer is awake.
    until: Instant,
    /// By the wall clock, because the other one stops while the computer
    /// sleeps: after a night with the lid shut it would still say the token
    /// had most of its hour left.
    until_wall: SystemTime,
}

impl OAuthTokens {
    /// The tokens of an account that was signed in to on an earlier run.
    ///
    /// Fails with [`Error::Auth`] if no refresh token is stored for it.
    /// Nothing is asked of the provider here: the first
    /// [`TokenSource::access_token`] does that.
    ///
    /// `account` is the name the sign-in was stored under; see
    /// [`crate::oauth_name`].
    pub async fn resume(
        http: &OAuthHttp,
        provider: &OAuthProvider,
        client: &OAuthClient,
        store: Arc<dyn SecretStore>,
        account: &str,
    ) -> Result<Self> {
        let tokens = Self::new(http, provider, client, store, account)?;
        if tokens.inner.stored().await?.is_none() {
            return Err(tokens.inner.not_signed_in());
        }
        Ok(tokens)
    }

    /// The tokens of an account that has just been signed in to: stores the
    /// refresh token and keeps the access token for its first use.
    pub(crate) async fn begin(
        http: &OAuthHttp,
        provider: &OAuthProvider,
        client: &OAuthClient,
        store: Arc<dyn SecretStore>,
        account: &str,
        grant: Grant,
    ) -> Result<Self> {
        let tokens = Self::new(http, provider, client, store, account)?;
        // Without a refresh token the account would work for an hour and
        // then ask for a sign-in every hour after. Better to say so now.
        let Some(refresh) = grant.refresh.clone() else {
            return Err(Error::Auth(format!(
                "{} signed in but gave no refresh token, so the sign-in could not be kept. Check that the application registered for Quill may have offline access",
                provider.name
            )));
        };
        tokens.inner.store_refresh(refresh).await?;
        tokens.inner.keep(grant);
        Ok(tokens)
    }

    fn new(
        http: &OAuthHttp,
        provider: &OAuthProvider,
        client: &OAuthClient,
        store: Arc<dyn SecretStore>,
        account: &str,
    ) -> Result<Self> {
        let endpoints = provider.endpoints()?;
        client.check()?;
        if account.trim().is_empty() {
            return Err(Error::Backend(
                "an account needs a name to keep its sign-in under".to_owned(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                http: http.clone(),
                token_url: endpoints.token,
                client: client.clone(),
                store,
                name: oauth_name(&provider.name, account),
                account: account.to_owned(),
                access: Mutex::new(None),
                renewing: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// The name the refresh token is stored under, for removing it when
    /// the account is removed.
    pub fn secret_name(&self) -> &str {
        &self.inner.name
    }
}

impl Inner {
    /// A token that is good for a while yet: the one in hand, or a new one.
    async fn fresh(&self) -> Result<String> {
        let _renewing = self.renewing.lock().await;
        // Whoever held the lock a moment ago may have done the work.
        if let Some(token) = self.current() {
            return Ok(token);
        }
        self.renew().await
    }

    /// The access token in hand, if it has long enough left.
    fn current(&self) -> Option<String> {
        let access = self.access.lock().unwrap_or_else(PoisonError::into_inner);
        let access = access.as_ref()?;
        let fresh = Instant::now() + MARGIN < access.until
            && SystemTime::now() + MARGIN < access.until_wall;
        fresh.then(|| access.token.expose().to_owned())
    }

    fn keep(&self, grant: Grant) {
        let access = Access {
            token: grant.access,
            until: Instant::now() + grant.lifetime,
            until_wall: SystemTime::now() + grant.lifetime,
        };
        *self.access.lock().unwrap_or_else(PoisonError::into_inner) = Some(access);
    }

    fn forget(&self) {
        *self.access.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Gets a new access token with the stored refresh token.
    async fn renew(&self) -> Result<String> {
        let Some(refresh) = self.stored().await? else {
            return Err(self.not_signed_in());
        };
        let asked = self
            .http
            .token(
                &self.token_url,
                &self.client,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.expose()),
                ],
            )
            .await;
        let grant = match asked {
            Ok(grant) => grant,
            // The refresh token is dead: expired, revoked, or the consent
            // behind it withdrawn. Keeping it would only repeat this.
            Err(Refusal::Provider(said)) if said.code == "invalid_grant" => {
                self.discard(refresh).await;
                return Err(Error::Auth(format!(
                    "the saved sign-in for {} is no longer accepted ({}). Sign in again",
                    self.account,
                    said.words()
                )));
            }
            // Any other refusal is about the request or the application,
            // not this refresh token, so the token is kept. The user still
            // has to act: nothing here will come right by waiting.
            Err(Refusal::Provider(said)) => {
                return Err(Error::Auth(format!(
                    "the sign-in for {} could not be renewed ({}). Sign in again",
                    self.account,
                    said.words()
                )));
            }
            Err(Refusal::Other(err)) => return Err(err),
        };
        // Microsoft sends a new refresh token with every renewal and
        // expects the newest to be used; Google usually sends none and the
        // old one stays good. If the new one cannot be stored the renewal
        // fails as a whole, loudly, and is tried again from the start next
        // time: carrying on would hide a Keychain problem until the day the
        // old token stopped working.
        if let Some(rotated) = &grant.refresh
            && rotated.expose() != refresh.expose()
        {
            self.store_refresh(rotated.clone()).await?;
        }
        let token = grant.access.expose().to_owned();
        self.keep(grant);
        Ok(token)
    }

    fn not_signed_in(&self) -> Error {
        Error::Auth(format!(
            "no sign-in is saved for {}. Sign in again",
            self.account
        ))
    }

    async fn stored(&self) -> Result<Option<Secret>> {
        let name = self.name.clone();
        self.in_store(move |store| store.get(&name))
            .await
            .map(|secret| secret.filter(|secret| !secret.is_empty()).map(Secret::new))
    }

    async fn store_refresh(&self, refresh: Secret) -> Result<()> {
        let name = self.name.clone();
        self.in_store(move |store| store.set(&name, refresh.expose()))
            .await
    }

    /// Removes a refresh token the provider has refused, unless the store
    /// holds a different one by now: another Quill process may have renewed
    /// in the meantime, and its token was not the one refused.
    async fn discard(&self, refused: Secret) {
        let name = self.name.clone();
        // If the Keychain will not let go of it, the next renewal is
        // refused the same way and this is tried again.
        let _ = self
            .in_store(move |store| {
                if store.get(&name)?.as_deref() == Some(refused.expose()) {
                    store.delete(&name)?;
                }
                Ok(())
            })
            .await;
        self.forget();
    }

    /// Runs `work` against the store off the async threads. The Keychain
    /// can block for as long as macOS takes to ask the user about it, and
    /// nothing else should stop for that.
    async fn in_store<T: Send + 'static>(
        &self,
        work: impl FnOnce(&dyn SecretStore) -> std::result::Result<T, AuthError> + Send + 'static,
    ) -> Result<T> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || work(store.as_ref()))
            .await
            .map_err(|_| Error::Backend("the secret store could not be asked".to_owned()))?
            .map_err(|err| Error::Backend(err.to_string()))
    }
}

#[async_trait]
impl TokenSource for OAuthTokens {
    async fn access_token(&self) -> Result<String> {
        if let Some(token) = self.inner.current() {
            return Ok(token);
        }
        // The renewal runs on a task of its own, so that it finishes even
        // if this caller stops waiting. A renewal abandoned between the
        // provider's answer and the store would lose the refresh token that
        // answer carried, and with a provider that retires the old one on
        // use, the sign-in with it.
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.fresh().await })
            .await
            .map_err(|_| Error::Backend("the token renewal did not finish".to_owned()))?
    }

    async fn invalidate(&self) {
        self.inner.forget();
    }
}

impl fmt::Debug for OAuthTokens {
    /// Which account, and whether a token is in hand. Never the token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let holding = self
            .inner
            .access
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("OAuthTokens")
            .field("name", &self.inner.name)
            .field("client_id", &self.inner.client.id)
            .field("access_token", &if holding { "held" } else { "none" })
            .finish_non_exhaustive()
    }
}
