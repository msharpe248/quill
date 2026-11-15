//! Where Quill keeps secrets.
//!
//! Passwords and tokens live only in the macOS Keychain. They are never
//! written to config files, the database, logs or crash reports, and this
//! crate is the only place that touches them at rest.

use std::collections::HashMap;
use std::sync::Mutex;

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
}

/// The name an account's password is kept under: the protocol, the user and
/// the server, so two accounts never share a secret by accident.
pub fn password_name(protocol: &str, username: &str, host: &str) -> String {
    format!("{protocol}:{username}@{host}")
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
