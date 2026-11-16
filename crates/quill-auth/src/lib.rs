//! Where Quill keeps secrets, and how it signs in where a password will
//! not do.
//!
//! Passwords and tokens live only in the macOS Keychain. They are never
//! written to config files, the database, logs or crash reports, and this
//! crate is the only place that touches them at rest.
//!
//! [`oauth`] is signing in with OAuth 2.0: the browser and device code
//! flows, and the tokens that come of them.

pub mod oauth;

use std::collections::HashMap;
use std::sync::Mutex;

pub use oauth::{
    BrowserSignIn, DevicePrompt, DeviceSignIn, OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens,
    RedirectHost, Secret, SignInMethod, sign_in,
};

/// The Keychain service every Quill item is filed under.
pub const SERVICE: &str = "Quill";

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("the Keychain refused: {0}")]
    Keychain(String),
    #[error("secrets cannot be stored on this platform yet")]
    Unsupported,
}

/// Somewhere to keep secrets, by name.
pub trait SecretStore: Send + Sync {
    /// The secret called `name`, or `None` if there is none.
    fn get(&self, name: &str) -> Result<Option<String>, AuthError>;
    /// Saves `secret` as `name`, replacing any earlier one.
    fn set(&self, name: &str, secret: &str) -> Result<(), AuthError>;
    /// Removes `name`. Removing what is not there is not an error.
    fn delete(&self, name: &str) -> Result<(), AuthError>;
    /// Lets go of anything remembered about `name`, so that the next
    /// [`SecretStore::get`] asks afresh: a server has refused what was
    /// handed out, and the secret may have been changed since. A store
    /// that remembers nothing has nothing to do.
    fn forget(&self, name: &str) {
        let _ = name;
    }
}

/// A store that asks the one behind it for each secret once, and one
/// request at a time.
///
/// Reading the Keychain can stop to ask the user, and macOS asks once for
/// every request that is waiting: a sync, a watcher and a send starting
/// together used to mean the same question three times over, each one
/// already on its way before the first was answered. Here the first
/// request asks and the others wait for its answer.
///
/// What was found is kept for the life of the process, in memory and
/// nowhere else. A secret that is not there is not remembered, so one
/// saved by another program is found the next time it is asked for.
pub struct Remembered<S> {
    inner: S,
    /// Held across the request to `inner`: that is what makes the
    /// requests one at a time.
    seen: Mutex<HashMap<String, String>>,
}

impl<S> Remembered<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            seen: Mutex::new(HashMap::new()),
        }
    }
}

impl<S> std::fmt::Debug for Remembered<S> {
    /// Never what it holds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Remembered(..)")
    }
}

impl<S: SecretStore> SecretStore for Remembered<S> {
    fn get(&self, name: &str) -> Result<Option<String>, AuthError> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?;
        if let Some(secret) = seen.get(name) {
            return Ok(Some(secret.clone()));
        }
        let found = self.inner.get(name)?;
        if let Some(secret) = &found {
            seen.insert(name.to_owned(), secret.clone());
        }
        Ok(found)
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), AuthError> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?;
        // Forgotten first: if the save fails, what is remembered must not
        // be the old secret passed off as the new.
        seen.remove(name);
        self.inner.set(name, secret)?;
        seen.insert(name.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), AuthError> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?;
        seen.remove(name);
        self.inner.delete(name)
    }

    fn forget(&self, name: &str) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.remove(name);
        }
        self.inner.forget(name);
    }
}

/// The name an account's password is kept under: the protocol, the user and
/// the server, so two accounts never share a secret by accident.
pub fn password_name(protocol: &str, username: &str, host: &str) -> String {
    format!("{protocol}:{username}@{host}")
}

/// The name an account's OAuth refresh token is kept under: the provider
/// (an [`OAuthProvider`]'s `name`) and the account, so that the same
/// address signed in to two providers keeps two tokens. It cannot collide
/// with a [`password_name`], whose protocols are never called `oauth`.
pub fn oauth_name(provider: &str, account: &str) -> String {
    format!("oauth:{provider}:{account}")
}

/// The user's login Keychain.
#[derive(Debug, Clone)]
pub struct Keychain {
    service: String,
}

impl Keychain {
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    /// A Keychain under another service name, so tests cannot touch real
    /// secrets.
    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn get(&self, name: &str) -> Result<Option<String>, AuthError> {
        match security_framework::passwords::get_generic_password(&self.service, name) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| AuthError::Keychain("the saved secret is not text".to_owned())),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(AuthError::Keychain(err.to_string())),
        }
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), AuthError> {
        security_framework::passwords::set_generic_password(&self.service, name, secret.as_bytes())
            .map_err(|err| AuthError::Keychain(err.to_string()))
    }

    fn delete(&self, name: &str) -> Result<(), AuthError> {
        match security_framework::passwords::delete_generic_password(&self.service, name) {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(()),
            Err(err) => Err(AuthError::Keychain(err.to_string())),
        }
    }
}

/// `errSecItemNotFound`.
#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = -25300;

#[cfg(not(target_os = "macos"))]
impl SecretStore for Keychain {
    fn get(&self, _name: &str) -> Result<Option<String>, AuthError> {
        Err(AuthError::Unsupported)
    }

    fn set(&self, _name: &str, _secret: &str) -> Result<(), AuthError> {
        Err(AuthError::Unsupported)
    }

    fn delete(&self, _name: &str) -> Result<(), AuthError> {
        Err(AuthError::Unsupported)
    }
}

/// Secrets held in memory, for tests.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    secrets: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<String>, AuthError> {
        Ok(self
            .secrets
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), AuthError> {
        self.secrets
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?
            .insert(name.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), AuthError> {
        self.secrets
            .lock()
            .map_err(|_| AuthError::Keychain("poisoned".to_owned()))?
            .remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(store: &dyn SecretStore, name: &str) {
        assert_eq!(store.get(name).unwrap(), None);
        store.set(name, "first").unwrap();
        assert_eq!(store.get(name).unwrap().as_deref(), Some("first"));
        store.set(name, "sécond ✓").unwrap();
        assert_eq!(store.get(name).unwrap().as_deref(), Some("sécond ✓"));
        store.delete(name).unwrap();
        assert_eq!(store.get(name).unwrap(), None);
        // Deleting what is gone is fine.
        store.delete(name).unwrap();
    }

    /// A store that counts how often it is asked, and can be slow about
    /// answering, as the Keychain is while macOS asks the user.
    #[derive(Default)]
    struct Counting {
        inner: MemorySecrets,
        gets: std::sync::atomic::AtomicUsize,
        slow: bool,
    }

    impl SecretStore for Counting {
        fn get(&self, name: &str) -> Result<Option<String>, AuthError> {
            self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.slow {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            self.inner.get(name)
        }

        fn set(&self, name: &str, secret: &str) -> Result<(), AuthError> {
            self.inner.set(name, secret)
        }

        fn delete(&self, name: &str) -> Result<(), AuthError> {
            self.inner.delete(name)
        }
    }

    fn gets(store: &Remembered<Counting>) -> usize {
        store.inner.gets.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn a_remembered_secret_is_asked_for_once() {
        let store = Remembered::new(Counting::default());
        store
            .inner
            .inner
            .set("imap:robin@example.com", "hunter2")
            .unwrap();
        exercise_reads(&store, "imap:robin@example.com", "hunter2", 5);
        assert_eq!(gets(&store), 1);
        // Another secret is another question.
        store
            .inner
            .inner
            .set("oauth:microsoft:robin@example.com", "token")
            .unwrap();
        exercise_reads(&store, "oauth:microsoft:robin@example.com", "token", 3);
        assert_eq!(gets(&store), 2);
    }

    fn exercise_reads(store: &dyn SecretStore, name: &str, secret: &str, times: usize) {
        for _ in 0..times {
            assert_eq!(store.get(name).unwrap().as_deref(), Some(secret));
        }
    }

    #[test]
    fn requests_that_arrive_together_ask_once_between_them() {
        // A sync, a watcher and a send all starting at once.
        let store = std::sync::Arc::new(Remembered::new(Counting {
            slow: true,
            ..Counting::default()
        }));
        store
            .inner
            .inner
            .set("imap:robin@example.com", "hunter2")
            .unwrap();
        let threads: Vec<_> = (0..6)
            .map(|_| {
                let store = std::sync::Arc::clone(&store);
                std::thread::spawn(move || store.get("imap:robin@example.com").unwrap())
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap().as_deref(), Some("hunter2"));
        }
        assert_eq!(gets(&store), 1);
    }

    #[test]
    fn what_is_not_there_is_not_remembered_and_what_changes_is_followed() {
        let store = Remembered::new(Counting::default());
        // Not saved yet: asked each time, so that one saved by another
        // program is found.
        assert_eq!(store.get("imap:robin@example.com").unwrap(), None);
        store
            .inner
            .inner
            .set("imap:robin@example.com", "first")
            .unwrap();
        assert_eq!(
            store.get("imap:robin@example.com").unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(gets(&store), 2);
        // Saved through this store: known at once, without asking.
        store.set("imap:robin@example.com", "second").unwrap();
        assert_eq!(
            store.get("imap:robin@example.com").unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(gets(&store), 2);
        // Changed behind its back, then refused by a server: asked afresh.
        store
            .inner
            .inner
            .set("imap:robin@example.com", "third")
            .unwrap();
        assert_eq!(
            store.get("imap:robin@example.com").unwrap().as_deref(),
            Some("second")
        );
        store.forget("imap:robin@example.com");
        assert_eq!(
            store.get("imap:robin@example.com").unwrap().as_deref(),
            Some("third")
        );
        assert_eq!(gets(&store), 3);
        // Deleted: gone from here as well.
        store.delete("imap:robin@example.com").unwrap();
        assert_eq!(store.get("imap:robin@example.com").unwrap(), None);
    }

    #[test]
    fn what_is_remembered_is_never_printed() {
        let store = Remembered::new(MemorySecrets::default());
        store.set("imap:robin@example.com", "hunter2").unwrap();
        store.get("imap:robin@example.com").unwrap();
        let printed = format!("{store:?}");
        assert!(
            !printed.contains("hunter2") && !printed.contains("robin"),
            "{printed}"
        );
    }

    #[test]
    fn memory_secrets_set_replace_and_delete() {
        exercise(
            &MemorySecrets::default(),
            "imap:robin@example.com@mail.example.com",
        );
    }

    #[test]
    fn password_names_keep_accounts_apart() {
        assert_eq!(
            password_name("imap", "robin@example.com", "mail.example.com"),
            "imap:robin@example.com@mail.example.com"
        );
        assert_ne!(
            password_name("imap", "robin", "a.example.com"),
            password_name("imap", "robin", "b.example.com")
        );
        assert_ne!(
            password_name("imap", "robin", "a.example.com"),
            password_name("smtp", "robin", "a.example.com")
        );
    }

    #[test]
    fn oauth_names_keep_providers_and_accounts_apart() {
        assert_eq!(
            oauth_name("microsoft", "robin@example.com"),
            "oauth:microsoft:robin@example.com"
        );
        assert_ne!(
            oauth_name("microsoft", "robin@example.com"),
            oauth_name("google", "robin@example.com")
        );
        assert_ne!(
            oauth_name("google", "robin@example.com"),
            oauth_name("google", "sam@example.com")
        );
    }

    /// Uses the real login Keychain, under a service name of its own, so it
    /// runs only when asked: `QUILL_TEST_KEYCHAIN=1 cargo test -p quill-auth`.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_keychain_sets_replaces_and_deletes() {
        if std::env::var_os("QUILL_TEST_KEYCHAIN").is_none() {
            eprintln!("skipping: set QUILL_TEST_KEYCHAIN=1 to test against the login Keychain");
            return;
        }
        let keychain = Keychain::with_service(&format!("Quill Tests {}", std::process::id()));
        exercise(&keychain, "imap:robin@example.com@mail.example.com");
    }
}
