//! Turning the accounts in `config.toml` into backends for the sync engine.

use std::sync::Arc;

use async_trait::async_trait;
use quill_auth::{OAuthHttp, SecretStore, password_name};
use quill_core::config::{AccountConfig, AccountType, Config, DeleteFromServer, TlsMode};
use quill_core::{Backend, Credential, Credentials, Error, Sender, TokenSource};
use quill_ews::{EwsBackend, EwsConfig};
use quill_imap::{ImapBackend, ImapConfig, Trust};
use quill_pop3::{Pop3Backend, Pop3Config};
use quill_smtp::{SmtpAuth, SmtpConfig, SmtpSender};
use quill_store::Store;
use quill_sync::{ServerCopies, SyncAccount};
use quill_tui::state::Notice;

use crate::oauth;

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

/// How a POP3 account is reached, from its config, or why it cannot be.
pub fn pop3_config(account: &AccountConfig) -> Result<Pop3Config, String> {
    let server = account
        .pop3
        .as_ref()
        .ok_or_else(|| "it has no `pop3` server".to_owned())?;
    let pinned_sha256 = server
        .fingerprint
        .as_deref()
        .map(parse_fingerprint)
        .transpose()?;
    Ok(Pop3Config {
        host: server.host.clone(),
        port: server.port.unwrap_or(Pop3Config::default_port(server.tls)),
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

/// The server an account reads its mail from with a password, as far as
/// the password goes: who signs in, where, and what the password is kept
/// under in the Keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub username: String,
    pub host: String,
    pub secret: String,
}

impl Reading {
    fn imap(config: &ImapConfig) -> Self {
        Self {
            username: config.username.clone(),
            host: config.host.clone(),
            secret: imap_password_name(config),
        }
    }

    fn ews(config: &EwsConfig) -> Self {
        let host = ews_host(&config.url);
        Self {
            username: config.username.clone(),
            secret: password_name("ews", &config.username, &host),
            host,
        }
    }

    fn pop3(config: &Pop3Config) -> Self {
        Self {
            username: config.username.clone(),
            host: config.host.clone(),
            secret: password_name("pop3", &config.username, &config.host),
        }
    }
}

/// How an account that keeps a password reads its mail, or why it cannot.
pub fn reading(account: &AccountConfig) -> Result<Reading, String> {
    match account.kind {
        AccountType::Imap => imap_config(account).map(|config| Reading::imap(&config)),
        AccountType::Pop3 => pop3_config(account).map(|config| Reading::pop3(&config)),
        AccountType::Ews => ews_config(account).map(|config| Reading::ews(&config)),
        other => Err(format!("{} accounts are not supported yet", other.as_str())),
    }
}

/// How an on-premises Exchange account is reached, or why it cannot be.
pub fn ews_config(account: &AccountConfig) -> Result<EwsConfig, String> {
    let server = account.ews.as_ref().ok_or_else(|| {
        "it has no `ews` server. Give it `ews = { url = \"https://mail.example.com/EWS/Exchange.asmx\" }`"
            .to_owned()
    })?;
    Ok(EwsConfig::new(
        server.url.trim(),
        server
            .username
            .clone()
            .unwrap_or_else(|| account.address.clone()),
    ))
}

/// The server an EWS address names, for naming its password. What is not
/// an address is used as it stands; the backend refuses it later with a
/// better reason.
fn ews_host(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('@')
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
}

/// Everything the sync engine needs for an on-premises Exchange account:
/// its web services read the mail and send it.
pub fn ews_account(
    id: quill_core::types::LocalAccountId,
    account: &AccountConfig,
    config: EwsConfig,
    secrets: &Arc<dyn SecretStore>,
) -> Result<SyncAccount, String> {
    let password = Arc::new(SavedPassword {
        secrets: Arc::clone(secrets),
        names: vec![Reading::ews(&config).secret],
        account: account.name.clone(),
    });
    // Plain http is taken only for this computer itself, which the backend
    // checks: a stand-in for the server, for trying things out.
    let backend = if config.url.starts_with("http://") {
        EwsBackend::loopback(config, password)
    } else {
        EwsBackend::new(config, password)
    }
    .map_err(|err| err.to_string())?;
    let backend = Arc::new(backend);
    Ok(SyncAccount {
        id,
        name: account.name.clone(),
        backend: Arc::clone(&backend) as Arc<dyn Backend>,
        sender: Some(backend),
        server_saves_sent: EwsBackend::SAVES_SENT_COPY,
        server_copies: ServerCopies::Keep,
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
    let password = Arc::new(SavedPassword {
        secrets: Arc::clone(secrets),
        names: vec![imap_password_name(&imap)],
        account: name.to_owned(),
    });
    Arc::new(ImapBackend::new(imap, password))
}

/// A password kept in the Keychain, read again at every sign-in.
struct SavedPassword {
    secrets: Arc<dyn SecretStore>,
    /// The names it may be saved under; the first one found is used.
    names: Vec<String>,
    /// The account's name, to say which one has no password.
    account: String,
}

#[async_trait]
impl Credentials for SavedPassword {
    async fn get(&self) -> quill_core::Result<Credential> {
        let secrets = Arc::clone(&self.secrets);
        let names = self.names.clone();
        let account = self.account.clone();
        // The Keychain blocks, and for as long as it likes when macOS
        // stops to ask the user about it. Mail for other accounts, and the
        // screen, carry on meanwhile.
        tokio::task::spawn_blocking(move || {
            for name in &names {
                match secrets.get(name) {
                    Ok(Some(password)) => return Ok(Credential::Password(password)),
                    Ok(None) => {}
                    Err(err) => return Err(Error::Auth(err.to_string())),
                }
            }
            Err(Error::Auth(format!(
                "no password is saved; run: quill account password \"{account}\""
            )))
        })
        .await
        .map_err(|err| Error::Backend(err.to_string()))?
    }
}

/// An account's OAuth tokens, as what it signs in to its mail servers
/// with.
struct Tokens(Arc<dyn TokenSource>);

#[async_trait]
impl Credentials for Tokens {
    async fn get(&self) -> quill_core::Result<Credential> {
        Ok(Credential::OAuthToken(self.0.access_token().await?))
    }

    async fn refused(&self) {
        self.0.invalidate().await;
    }
}

/// Everything the sync engine needs for one IMAP account: how to read its
/// mail and, when it has an SMTP server, how to send. With `tokens` it
/// signs in to both servers with an OAuth token, not a saved password.
pub fn sync_account(
    id: quill_core::types::LocalAccountId,
    account: &AccountConfig,
    imap: ImapConfig,
    secrets: &Arc<dyn SecretStore>,
    tokens: Option<Arc<dyn TokenSource>>,
) -> SyncAccount {
    let sender = smtp_sender(account, &Reading::imap(&imap), secrets, tokens.as_ref());
    // Gmail files everything sent through its SMTP server in Sent Mail by
    // itself; a copy saved by Quill would be a second one.
    let server_saves_sent = account.smtp.as_ref().is_some_and(|smtp| {
        let host = smtp.host.to_ascii_lowercase();
        host.ends_with(".gmail.com") || host.ends_with(".googlemail.com")
    });
    let backend = match tokens {
        Some(tokens) => Arc::new(ImapBackend::new(imap, Arc::new(Tokens(tokens)))),
        None => imap_backend(imap, &account.name, secrets),
    };
    SyncAccount {
        id,
        name: account.name.clone(),
        backend,
        sender,
        server_saves_sent,
        server_copies: ServerCopies::Keep,
    }
}

/// Everything the sync engine needs for one POP3 account. Its mail is kept
/// here: the server hands it over and has no folders or marks of its own.
pub fn pop3_account(
    id: quill_core::types::LocalAccountId,
    account: &AccountConfig,
    pop3: Pop3Config,
    secrets: &Arc<dyn SecretStore>,
) -> SyncAccount {
    let reading = Reading::pop3(&pop3);
    let sender = smtp_sender(account, &reading, secrets, None);
    let password = Arc::new(SavedPassword {
        secrets: Arc::clone(secrets),
        names: vec![reading.secret],
        account: account.name.clone(),
    });
    SyncAccount {
        id,
        name: account.name.clone(),
        backend: Arc::new(Pop3Backend::new(pop3, password)),
        sender,
        // An SMTP server sends and keeps nothing; the copy is filed here.
        server_saves_sent: false,
        server_copies: match account.delete_from_server {
            DeleteFromServer::Never => ServerCopies::Keep,
            DeleteFromServer::Downloaded => ServerCopies::DeleteWhenDownloaded,
            DeleteFromServer::AfterDays(days) => ServerCopies::DeleteAfterDays(days),
        },
    }
}

/// How the account sends mail, when its config names an SMTP server. A
/// server Quill cannot use (a bad fingerprint) gives no sender, and sending
/// says so.
fn smtp_sender(
    account: &AccountConfig,
    reading: &Reading,
    secrets: &Arc<dyn SecretStore>,
    tokens: Option<&Arc<dyn TokenSource>>,
) -> Option<Arc<dyn Sender>> {
    let server = account.smtp.as_ref()?;
    let pinned_sha256 = match server.fingerprint.as_deref().map(parse_fingerprint) {
        Some(Ok(pinned)) => Some(pinned),
        Some(Err(_)) => return None,
        None => None,
    };
    // The same name as for reading, unless the SMTP server is given its own.
    let username = server
        .username
        .clone()
        .unwrap_or_else(|| reading.username.clone());
    let config = SmtpConfig {
        host: server.host.clone(),
        port: server.port.unwrap_or(SmtpConfig::default_port(server.tls)),
        tls: server.tls,
        username: username.clone(),
        trust: Trust {
            extra_roots: Vec::new(),
            pinned_sha256,
        },
        auth: SmtpAuth::Automatic,
        hello_name: quill_smtp::DEFAULT_HELLO_NAME.to_owned(),
    };
    let credential: Arc<dyn Credentials> = match tokens {
        // One sign-in serves reading and sending.
        Some(tokens) => Arc::new(Tokens(Arc::clone(tokens))),
        // A password saved for sending wins; otherwise the one for
        // reading, which is the same password at nearly every provider.
        None => Arc::new(SavedPassword {
            secrets: Arc::clone(secrets),
            names: vec![
                password_name("smtp", &username, &server.host),
                reading.secret.clone(),
            ],
            account: account.name.clone(),
        }),
    };
    Some(Arc::new(SmtpSender::new(config, credential)))
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
    // Made when the first account that signs in with OAuth needs it.
    let mut oauth_http: Option<OAuthHttp> = None;
    for account in &config.accounts {
        let name = &account.name;
        if account.kind == AccountType::Graph {
            let built = oauth::tokens(account, secrets, &mut oauth_http).and_then(|tokens| {
                let tokens = tokens.ok_or_else(|| "it does not sign in with OAuth".to_owned())?;
                // Registered only once it is known to be usable.
                let id = store
                    .upsert_account(name, &account.address, account.kind.as_str())
                    .map_err(|err| err.to_string())?;
                oauth::graph_account(id, account, tokens)
            });
            match built {
                Ok(account) => accounts.push(account),
                Err(reason) => {
                    notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                }
            }
            continue;
        }
        if account.kind == AccountType::Pop3 {
            match pop3_config(account) {
                Ok(pop3) => {
                    if pop3.tls == TlsMode::None {
                        notices.push(Notice::error(format!(
                            "{name} connects without encryption. Its password and mail cross the network in the clear."
                        )));
                    }
                    let id = store.upsert_account(name, &account.address, account.kind.as_str())?;
                    accounts.push(pop3_account(id, account, pop3, secrets));
                }
                Err(reason) => {
                    notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                }
            }
            continue;
        }
        if account.kind == AccountType::Ews {
            let built = ews_config(account).and_then(|config| {
                // Checked before the account is registered: one that
                // cannot be reached is not listed, empty, for ever.
                let probe = ews_account(
                    quill_core::types::LocalAccountId(0),
                    account,
                    config.clone(),
                    secrets,
                );
                probe.map(|_| config)
            });
            match built {
                Ok(config) => {
                    let id = store.upsert_account(name, &account.address, account.kind.as_str())?;
                    match ews_account(id, account, config, secrets) {
                        Ok(account) => accounts.push(account),
                        Err(reason) => {
                            notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                        }
                    }
                }
                Err(reason) => {
                    notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                }
            }
            continue;
        }
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
        // A mail server that takes a token in place of a password.
        let tokens = match oauth::tokens(account, secrets, &mut oauth_http) {
            Ok(tokens) => tokens,
            Err(reason) => {
                notices.push(Notice::error(format!("{name} cannot sync: {reason}.")));
                continue;
            }
        };
        let id = store.upsert_account(name, &account.address, account.kind.as_str())?;
        accounts.push(sync_account(id, account, imap, secrets, tokens));
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
type = "ews"
address = "robin@example.com"
ews = { url = "http://mail.example.com/EWS/Exchange.asmx" }

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
        // The other is refused for a reason of its own: Exchange is
        // reached over https or not at all.
        assert_eq!(prepared.notices[0].kind, NoticeKind::Error);
        assert!(
            prepared.notices[0].text.starts_with("Work cannot sync: "),
            "{}",
            prepared.notices[0].text
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
    fn a_graph_account_is_built_with_its_own_application_or_quills() {
        let mut store = Store::open_in_memory().unwrap();
        let text = r#"
[[account]]
name = "Work"
type = "graph"
address = "robin@example.com"
oauth_client_id = "app-1"

[[account]]
name = "Other"
type = "graph"
address = "robin@example.org"
"#;
        let prepared = prepare(&config(text), &mut store, &secrets(), true).unwrap();
        // The one that names no application signs in as Quill itself.
        assert_eq!(prepared.accounts.len(), 2);
        assert_eq!(prepared.accounts[0].name, "Work");
        assert_eq!(prepared.accounts[1].name, "Other");
        assert!(prepared.accounts[0].sender.is_some());
        assert!(prepared.accounts[0].server_saves_sent);
        assert!(
            !prepared
                .notices
                .iter()
                .any(|notice| notice.text.contains("oauth_client_id")),
            "{:?}",
            prepared.notices
        );
        let stored = store.accounts().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(
            (stored[0].name.as_str(), stored[0].kind.as_str()),
            ("Work", "graph")
        );
        assert_eq!(
            (stored[1].name.as_str(), stored[1].kind.as_str()),
            ("Other", "graph")
        );
    }

    #[tokio::test]
    async fn a_mail_server_account_can_sign_in_with_oauth_in_place_of_a_password() {
        let mut store = Store::open_in_memory().unwrap();
        let text = r#"
[[account]]
name = "Mail"
type = "imap"
address = "robin@example.com"
auth = "oauth"
oauth_client_id = "app-2"
oauth_client_secret = "not-really"
imap = { host = "127.0.0.1", port = 1, tls = "none" }
smtp = { host = "127.0.0.1", port = 1, tls = "none" }

[[account]]
name = "No App"
type = "imap"
address = "robin@example.org"
auth = "oauth"
imap = { host = "imap.example.org" }
"#;
        let prepared = prepare(&config(text), &mut store, &secrets(), true).unwrap();
        assert_eq!(prepared.accounts.len(), 1);
        assert_eq!(prepared.accounts[0].name, "Mail");
        assert!(prepared.accounts[0].sender.is_some());
        assert!(
            prepared.notices.iter().any(|notice| notice
                .text
                .starts_with("No App cannot sync: it has no `oauth_client_id`")),
            "{:?}",
            prepared.notices
        );
        // Nobody has signed in, and it is not a password that is missing.
        // (Nothing listens on port 1, so the connection is what fails
        // first; the sender is asked directly.)
        let sender = prepared.accounts[0].sender.clone().unwrap();
        let envelope = quill_core::Envelope {
            from: "robin@example.com".to_owned(),
            to: vec!["ana@example.com".to_owned()],
        };
        assert!(matches!(
            sender.send(b"Subject: x\r\n\r\n", &envelope).await,
            Err(Error::Connection(_))
        ));
    }

    #[tokio::test]
    async fn a_saved_password_is_looked_for_under_each_name_in_turn() {
        let secrets = secrets();
        let saved = SavedPassword {
            secrets: Arc::clone(&secrets),
            names: vec![
                "smtp:a@example.com".to_owned(),
                "imap:a@example.com".to_owned(),
            ],
            account: "Personal".to_owned(),
        };
        match saved.get().await {
            Err(Error::Auth(reason)) => {
                assert!(
                    reason.contains("quill account password \"Personal\""),
                    "{reason}"
                );
            }
            other => panic!("{other:?}"),
        }
        secrets.set("imap:a@example.com", "for-reading").unwrap();
        assert_eq!(
            saved.get().await.unwrap(),
            Credential::Password("for-reading".to_owned())
        );
        secrets.set("smtp:a@example.com", "for-sending").unwrap();
        assert_eq!(
            saved.get().await.unwrap(),
            Credential::Password("for-sending".to_owned())
        );
    }

    #[test]
    fn a_pop3_account_is_built_with_its_policy_and_its_own_password_name() {
        let mut store = Store::open_in_memory().unwrap();
        let text = r#"
[[account]]
name = "Old Mail"
type = "pop3"
address = "robin@example.com"
delete_from_server = 30
pop3 = { host = "pop.example.com", username = "rvale" }
smtp = { host = "smtp.example.com" }

[[account]]
name = "No Server"
type = "pop3"
address = "robin@example.org"
"#;
        let config = config(text);
        let prepared = prepare(&config, &mut store, &secrets(), true).unwrap();
        assert_eq!(prepared.accounts.len(), 1);
        let built = &prepared.accounts[0];
        assert_eq!(built.name, "Old Mail");
        assert_eq!(built.server_copies, ServerCopies::DeleteAfterDays(30));
        assert!(built.sender.is_some() && !built.server_saves_sent);
        // Its server has nothing but mail: no folders, no marks.
        assert!(!built.backend.capabilities().folders);
        assert_eq!(
            prepared.notices[0].text,
            "No Server cannot sync: it has no `pop3` server."
        );
        let stored = store.accounts().unwrap();
        assert_eq!(
            (stored[0].name.as_str(), stored[0].kind.as_str()),
            ("Old Mail", "pop3")
        );

        let pop3 = pop3_config(&config.accounts[0]).unwrap();
        // Implicit TLS on 995 unless told otherwise.
        assert_eq!((pop3.port, pop3.tls), (995, TlsMode::Implicit));
        assert_eq!(
            reading(&config.accounts[0]).unwrap(),
            Reading {
                username: "rvale".to_owned(),
                host: "pop.example.com".to_owned(),
                secret: "pop3:rvale@pop.example.com".to_owned(),
            }
        );
    }

    #[test]
    fn an_exchange_account_is_built_from_its_address_and_keeps_its_own_password() {
        let mut store = Store::open_in_memory().unwrap();
        let text = r#"
[[account]]
name = "Office"
type = "ews"
address = "robin@example.com"
ews = { url = "https://Mail.Example.com/EWS/Exchange.asmx", username = "EXAMPLE\\robin" }

[[account]]
name = "No Server"
type = "ews"
address = "robin@example.org"
"#;
        let config = config(text);
        let prepared = prepare(&config, &mut store, &secrets(), true).unwrap();
        assert_eq!(prepared.accounts.len(), 1, "{:?}", prepared.notices);
        let built = &prepared.accounts[0];
        assert_eq!(built.name, "Office");
        assert!(built.sender.is_some() && built.server_saves_sent);
        assert!(built.backend.capabilities().folders);
        assert!(
            prepared.notices[0]
                .text
                .starts_with("No Server cannot sync: it has no `ews` server."),
            "{}",
            prepared.notices[0].text
        );
        assert_eq!(
            reading(&config.accounts[0]).unwrap(),
            Reading {
                username: "EXAMPLE\\robin".to_owned(),
                host: "mail.example.com".to_owned(),
                secret: "ews:EXAMPLE\\robin@mail.example.com".to_owned(),
            }
        );
        let stored = store.accounts().unwrap();
        assert_eq!(
            (stored[0].name.as_str(), stored[0].kind.as_str()),
            ("Office", "ews")
        );
        assert_eq!(stored.len(), 1);
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
