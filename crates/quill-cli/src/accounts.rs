//! Turning the accounts in `config.toml` into backends for the sync engine.

use std::sync::Arc;

use quill_auth::{SecretStore, password_name};
use quill_core::config::{AccountConfig, AccountType, Config, TlsMode};
use quill_core::{Backend, Error};
use quill_imap::{ImapBackend, ImapConfig, Trust};
use quill_store::Store;
use quill_sync::SyncAccount;
use quill_tui::state::Notice;

pub struct Prepared {
    pub accounts: Vec<SyncAccount>,
    pub notices: Vec<Notice>,
}

/// How an IMAP account is reached, from its config, or why it cannot be.
pub fn imap_config(account: &AccountConfig) -> Result<ImapConfig, String> {
    let server = account
        .imap
        .as_ref()
        .ok_or_else(|| "it has no `imap` server".to_owned())?;
    let pinned_sha256 = server
        .fingerprint
        .as_deref()
        .map(parse_fingerprint)
        .transpose()?;
    Ok(ImapConfig {
        host: server.host.clone(),
        port: server.port.unwrap_or(ImapConfig::default_port(server.tls)),
        tls: server.tls,
        username: server
            .username
            .clone()
            .unwrap_or_else(|| account.address.clone()),
        trust: Trust {
            extra_roots: Vec::new(),
            pinned_sha256,
        },
    })
}

/// The name an account's IMAP password is kept under in the Keychain.
pub fn imap_password_name(config: &ImapConfig) -> String {
    password_name("imap", &config.username, &config.host)
}

/// Reads a SHA-256 fingerprint: 64 hex digits, with or without colons,
/// spaces or a `sha256:` prefix.
pub fn parse_fingerprint(text: &str) -> Result<[u8; 32], String> {
    let trimmed = text.trim();
    let digits: String = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed)
        .chars()
        .filter(|c| !matches!(c, ':' | ' '))
        .collect();
    let mut out = [0u8; 32];
    if digits.len() != 64 || !digits.is_ascii() {
        return Err("its fingerprint is not 64 hex digits".to_owned());
    }
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16)
            .map_err(|_| "its fingerprint is not 64 hex digits".to_owned())?;
    }
    Ok(out)
}

/// The backend for an IMAP account, which asks the Keychain for the
/// account's password each time it signs in.
pub fn imap_backend(
    imap: ImapConfig,
    name: &str,
    secrets: &Arc<dyn SecretStore>,
) -> Arc<dyn Backend> {
    let key = imap_password_name(&imap);
    let secrets = Arc::clone(secrets);
    let hint = format!("no password is saved; run: quill account password \"{name}\"");
    let password = Arc::new(move || match secrets.get(&key) {
        Ok(Some(password)) => Ok(password),
        Ok(None) => Err(Error::Auth(hint.clone())),
        Err(err) => Err(Error::Auth(err.to_string())),
    });
    Arc::new(ImapBackend::new(imap, password))
}

/// Registers the configured accounts in the store and builds a backend for
/// each one Quill can sync. An account it cannot use becomes a notice, never
/// a failure to start.
///
/// `prune` removes accounts that are in the store but no longer in the
/// config. Pass `false` when the config could not be read: an unreadable
/// file must not be taken for "the user removed every account".
pub fn prepare(
    config: &Config,
    store: &mut Store,
    secrets: &Arc<dyn SecretStore>,
    prune: bool,
) -> quill_store::Result<Prepared> {
    let mut accounts = Vec::new();
    let mut notices = Vec::new();
    for account in &config.accounts {
        let name = &account.name;
        if account.kind != AccountType::Imap {
            notices.push(Notice::info(format!(
                "{name}: {} accounts are not supported yet.",
                account.kind.as_str()
            )));
            continue;
        }
        let imap = match imap_config(account) {
            Ok(imap) => imap,
            Err(reason) => {
                notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                continue;
            }
        };
        if imap.tls == TlsMode::None {
            notices.push(Notice::error(format!(
                "{name} connects without encryption. Its password and mail cross the network in the clear."
            )));
        }
        let id = store.upsert_account(name, &account.address, account.kind.as_str())?;
        accounts.push(SyncAccount {
            id,
            name: name.clone(),
            backend: imap_backend(imap, name, secrets),
        });
    }
    if prune {
        let keep: Vec<&str> = config
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect();
        store.retain_accounts(&keep)?;
    }
    Ok(Prepared { accounts, notices })
}

#[cfg(test)]
mod tests {
    use quill_auth::MemorySecrets;
    use quill_tui::state::NoticeKind;

    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    fn secrets() -> Arc<dyn SecretStore> {
        Arc::new(MemorySecrets::default())
    }

    const TWO_ACCOUNTS: &str = r#"
[[account]]
name = "Work"
type = "graph"
address = "robin@example.com"

[[account]]
name = "Personal"
type = "imap"
address = "robin.vale@example.com"
imap = { host = "mail.example.com", tls = "starttls", username = "rvale" }
"#;

    #[test]
    fn an_imap_account_becomes_a_backend_and_others_a_notice() {
        let mut store = Store::open_in_memory().unwrap();
        let prepared = prepare(&config(TWO_ACCOUNTS), &mut store, &secrets(), true).unwrap();
        assert_eq!(prepared.accounts.len(), 1);
        assert_eq!(prepared.accounts[0].name, "Personal");
        assert_eq!(prepared.notices.len(), 1);
        assert_eq!(prepared.notices[0].kind, NoticeKind::Info);
        assert_eq!(
            prepared.notices[0].text,
            "Work: graph accounts are not supported yet."
        );
        // Only the account that syncs is in the store.
        let stored = store.accounts().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            (stored[0].name.as_str(), stored[0].kind.as_str()),
            ("Personal", "imap")
        );
        assert_eq!(stored[0].id, prepared.accounts[0].id);
    }

    #[test]
    fn server_settings_come_from_the_config_with_sensible_defaults() {
        let accounts = config(TWO_ACCOUNTS).accounts;
        let imap = imap_config(&accounts[1]).unwrap();
        assert_eq!((imap.host.as_str(), imap.port), ("mail.example.com", 143));
        assert_eq!(imap.tls, TlsMode::Starttls);
        assert_eq!(imap.username, "rvale");
        assert_eq!(imap_password_name(&imap), "imap:rvale@mail.example.com");

        let other = config(
            "[[account]]\nname = \"A\"\ntype = \"imap\"\naddress = \"a@example.com\"\nimap = { host = \"imap.example.com\" }\n",
        );
        let imap = imap_config(&other.accounts[0]).unwrap();
        // Implicit TLS on 993, signing in with the address.
        assert_eq!((imap.port, imap.tls), (993, TlsMode::Implicit));
        assert_eq!(imap.username, "a@example.com");
        assert_eq!(imap.trust, Trust::default());
    }

    #[test]
    fn an_account_that_cannot_be_used_is_a_notice_not_a_failure() {
        let mut store = Store::open_in_memory().unwrap();
        let text = r#"
[[account]]
name = "No Server"
type = "imap"
address = "a@example.com"

[[account]]
name = "Bad Pin"
type = "imap"
address = "b@example.com"
imap = { host = "mail.example.com", fingerprint = "12:34" }
"#;
        let prepared = prepare(&config(text), &mut store, &secrets(), true).unwrap();
        assert!(prepared.accounts.is_empty());
        let texts: Vec<&str> = prepared.notices.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "No Server cannot sync: it has no `imap` server.",
                "Bad Pin cannot sync: its fingerprint is not 64 hex digits.",
            ]
        );
        assert!(store.accounts().unwrap().is_empty());
    }

    #[test]
    fn fingerprints_are_read_in_the_forms_people_paste() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899AABBCCDDEEFF";
        let colons: String = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8_lossy(pair).into_owned())
            .collect::<Vec<_>>()
            .join(":");
        let expected = parse_fingerprint(hex).unwrap();
        assert_eq!(expected[0], 0x00);
        assert_eq!(expected[15], 0xff);
        assert_eq!(parse_fingerprint(&colons).unwrap(), expected);
        assert_eq!(
            parse_fingerprint(&format!("sha256:{hex}")).unwrap(),
            expected
        );
        assert_eq!(
            parse_fingerprint(&format!("  SHA256:{colons} ")).unwrap(),
            expected
        );
        for bad in [
            "",
            "1234",
            &hex[..63],
            &format!("{hex}00"),
            &hex.replace('0', "g"),
        ] {
            assert!(parse_fingerprint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_plaintext_account_carries_a_warning() {
        let mut store = Store::open_in_memory().unwrap();
        let text = "[[account]]\nname = \"Lab\"\ntype = \"imap\"\naddress = \"a@example.com\"\nimap = { host = \"10.0.0.5\", tls = \"none\" }\n";
        let prepared = prepare(&config(text), &mut store, &secrets(), true).unwrap();
        assert_eq!(prepared.accounts.len(), 1);
        assert_eq!(prepared.notices[0].kind, NoticeKind::Error);
        assert!(
            prepared.notices[0]
                .text
                .starts_with("Lab connects without encryption.")
        );
    }

    #[test]
    fn removed_accounts_are_pruned_only_when_the_config_was_read() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_account("Old", "old@example.com", "imap")
            .unwrap();
        // An unreadable config looks like one with no accounts. Do not prune.
        prepare(&Config::default(), &mut store, &secrets(), false).unwrap();
        assert_eq!(store.accounts().unwrap().len(), 1);
        prepare(&Config::default(), &mut store, &secrets(), true).unwrap();
        assert!(store.accounts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_password_is_read_from_the_secret_store_at_sign_in() {
        let mut store = Store::open_in_memory().unwrap();
        let text = "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"a@example.com\"\nimap = { host = \"127.0.0.1\", port = 1, tls = \"none\" }\n";
        let prepared = prepare(&config(text), &mut store, &secrets(), true).unwrap();
        // Nothing listens on port 1, so the connection fails before any
        // password is needed: a connection error, not an auth one.
        assert!(matches!(
            prepared.accounts[0].backend.list_folders().await,
            Err(Error::Connection(_))
        ));
    }
}
