//! The accounts service: what the setup flow in the UI asks for, done.
//!
//! The UI describes an account and asks; this finds the server, signs in to
//! check, and only then writes the account to `config.toml`, its password
//! to the Keychain, and hands it to the sync engine. Accepting a server's
//! certificate for an account that already exists goes the same way.
//!
//! An account that signs in with Microsoft has no password to check. It is
//! written down and handed to the engine as it is, and signing in to it is
//! a request of its own, which the UI makes next.
//!
//! The UI reaches no server itself, so the one request it ever asks to
//! have made to a server that is not the user's comes through here too:
//! telling a mailing list to unsubscribe them (see `unsubscribe`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quill_auth::SecretStore;
use quill_core::config::{AccountConfig, AccountType, Config};
use quill_core::types::LocalAccountId;
use quill_core::{Backend, Error};
use quill_imap::{ImapBackend, ImapConfig, Trust};
use quill_smtp::{SmtpAuth, SmtpConfig, SmtpSender};
use quill_store::Store;
use quill_sync::{SyncAccount, SyncHandle};
use quill_tui::app::AccountsLink;
use quill_tui::setup::{
    AccountDraft, AccountEvent, AccountRequest, ServerDraft, SignInDraft, Trusting, VerifyProblem,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::accounts::{imap_config, imap_password_name, parse_fingerprint, sync_account};
use crate::config_edit;
use crate::discover::{self, Dns, Prober};
use crate::oauth;

mod unsubscribe;

pub struct Service {
    /// `config.toml`.
    pub config_file: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    /// A connection of the service's own, for registering accounts.
    pub store: Mutex<Store>,
    pub sync: Arc<SyncHandle>,
    pub dns: Box<dyn Dns>,
    pub prober: Box<dyn Prober>,
    /// Certificates accepted for this run only, by account and by whether
    /// it is the sending server's. Written nowhere.
    pub trusted_once: Mutex<HashMap<(String, bool), String>>,
    /// Whether whoever is at the keyboard is at another computer (over
    /// SSH), where a browser opened on this Mac would be no use to them.
    pub remote: bool,
}

/// Starts the service on the current Tokio runtime and returns the UI's end
/// of it. It stops when the UI lets go of that.
pub fn spawn(service: Service) -> AccountsLink {
    let (requests_tx, mut requests) = unbounded_channel();
    let (events_tx, events) = unbounded_channel();
    tokio::spawn(async move {
        let service = Arc::new(service);
        // A sign-in waits on the user for minutes. It runs beside the
        // requests, which go on being answered, and is dropped to give up.
        let mut signing_in: Option<tokio::task::JoinHandle<()>> = None;
        while let Some(request) = requests.recv().await {
            match request {
                AccountRequest::SignIn { account } => {
                    if let Some(earlier) = signing_in.take() {
                        earlier.abort();
                    }
                    let service = Arc::clone(&service);
                    let events = events_tx.clone();
                    signing_in = Some(tokio::spawn(async move {
                        service.sign_in(account, events).await;
                    }));
                }
                AccountRequest::CancelSignIn => {
                    if let Some(sign_in) = signing_in.take() {
                        sign_in.abort();
                    }
                }
                // A list's server may take its time, or never answer. The
                // request runs beside the others, which are not held up.
                request @ AccountRequest::Unsubscribe { .. } => {
                    let service = Arc::clone(&service);
                    let events = events_tx.clone();
                    tokio::spawn(async move {
                        let _ = events.send(service.handle(request).await);
                    });
                }
                request => {
                    let event = service.handle(request).await;
                    if events_tx.send(event).is_err() {
                        break;
                    }
                }
            }
        }
        if let Some(sign_in) = signing_in {
            sign_in.abort();
        }
    });
    AccountsLink {
        requests: Some(requests_tx),
        events,
    }
}

impl Service {
    pub async fn handle(self: &Arc<Self>, request: AccountRequest) -> AccountEvent {
        match request {
            AccountRequest::Discover { address } => {
                let mut found = discover::discover(&address, &*self.dns, &*self.prober).await;
                // The form reads no config: what it may offer from there
                // goes with the answer.
                found.microsoft_application = self.microsoft_application().await;
                AccountEvent::Discovered { address, found }
            }
            AccountRequest::Verify {
                draft,
                password,
                trusting,
            } => AccountEvent::Verified(verify(&draft, password.0, &trusting).await),
            AccountRequest::Save {
                draft,
                password,
                trusting,
            } => {
                let service = Arc::clone(self);
                let name = draft.name.clone();
                // Files, the Keychain and the store all block.
                let saved = tokio::task::spawn_blocking(move || {
                    service.save(&draft, &password.0, &trusting)
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::Saved(saved.map(|account| {
                    // The engine's tasks are started from the runtime.
                    self.sync.set_account(account);
                    name
                }))
            }
            AccountRequest::SaveSignIn { draft } => {
                let service = Arc::clone(self);
                let name = draft.name.clone();
                // Files and the store block.
                let saved = tokio::task::spawn_blocking(move || service.save_sign_in(&draft))
                    .await
                    .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::Saved(saved.map(|account| {
                    // Nobody has signed in to it yet, and the engine is
                    // told so by the account itself, as for one that was
                    // in the config when Quill started. It asks nothing of
                    // Microsoft until there is a sign-in to ask with.
                    self.sync.set_account(account);
                    name
                }))
            }
            AccountRequest::Trust {
                account,
                fingerprint,
                pin,
                sending,
            } => {
                let service = Arc::clone(self);
                let name = account.clone();
                let trusted = tokio::task::spawn_blocking(move || {
                    service.trust(&name, &fingerprint, pin, sending)
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::Trusted {
                    account,
                    sending,
                    result: trusted.map(|account| self.sync.set_account(account)),
                }
            }
            // These two are taken before here: see `spawn`.
            AccountRequest::SignIn { account } => AccountEvent::SignedIn {
                account: account.unwrap_or_default(),
                result: Err("signing in is not done this way".to_owned()),
            },
            AccountRequest::CancelSignIn => AccountEvent::SignedIn {
                account: String::new(),
                result: Err("no sign-in was under way".to_owned()),
            },
            AccountRequest::Unsubscribe { url } => {
                let (host, result) = unsubscribe::one_click(&url).await;
                AccountEvent::Unsubscribed { host, result }
            }
        }
    }

    /// Signs an account in to its provider and says how it went. On the
    /// way it says how the user is to do their part: in the browser, or
    /// with a code.
    pub async fn sign_in(
        self: &Arc<Self>,
        preferred: Option<String>,
        events: tokio::sync::mpsc::UnboundedSender<AccountEvent>,
    ) {
        let (account, result) = match self.choose_for_sign_in(preferred).await {
            Ok(account) => {
                let result = self.sign_in_to(&account, &events).await;
                (account.name, result)
            }
            Err(reason) => (String::new(), Err(reason)),
        };
        let _ = events.send(AccountEvent::SignedIn { account, result });
    }

    /// Which account to sign in: the one asked for if it signs in with a
    /// provider, otherwise the first such account nobody has signed in to
    /// yet, otherwise the first of them.
    async fn choose_for_sign_in(
        self: &Arc<Self>,
        preferred: Option<String>,
    ) -> Result<AccountConfig, String> {
        let service = Arc::clone(self);
        // The config file and the Keychain both block.
        tokio::task::spawn_blocking(move || {
            let config = Config::parse(&service.config_text()?)
                .map_err(|err| format!("config.toml has a mistake to fix first: {err}"))?;
            let candidates: Vec<&AccountConfig> = config
                .accounts
                .iter()
                .filter(|account| oauth::wanted(account))
                .collect();
            if let Some(asked) = candidates
                .iter()
                .find(|account| Some(&account.name) == preferred.as_ref())
            {
                return Ok((*asked).clone());
            }
            let signed_in = |account: &AccountConfig| {
                oauth::setup(account)
                    .ok()
                    .flatten()
                    .and_then(|setup| service.secrets.get(&setup.secret_name(account)).ok())
                    .flatten()
                    .is_some()
            };
            candidates
                .iter()
                .find(|account| !signed_in(account))
                .or(candidates.first())
                .map(|account| (*account).clone())
                .ok_or_else(|| {
                    "No account here signs in with Microsoft or Google. An account with a password keeps it in the Keychain, and there is nothing to sign in to.".to_owned()
                })
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))
    }

    async fn sign_in_to(
        &self,
        account: &AccountConfig,
        events: &tokio::sync::mpsc::UnboundedSender<AccountEvent>,
    ) -> Result<(), String> {
        let setup = oauth::setup(account)?
            .ok_or_else(|| "it signs in with a password, not with a provider".to_owned())?;
        let http = quill_auth::OAuthHttp::new(&Trust::default()).map_err(|err| err.to_string())?;
        let name = account.name.clone();
        let events = events.clone();
        let method = if self.remote {
            if setup.provider.device_url.is_none() {
                return Err(
                    "this provider has no sign-in by code, which is the only way from another computer. Sign in at the Mac itself, or use an app password".to_owned(),
                );
            }
            quill_auth::SignInMethod::DeviceCode(quill_auth::DeviceSignIn::new(
                move |prompt: &quill_auth::DevicePrompt| {
                    let _ = events.send(AccountEvent::SignInCode {
                        account: name,
                        address: prompt.verification_uri.clone(),
                        code: prompt.user_code.clone(),
                        minutes: prompt.expires_in.as_secs().div_ceil(60),
                    });
                },
            ))
        } else {
            // The UI opens the browser: here there may be a sandbox in the
            // way, and it is the UI that knows the way out.
            let mut browser = quill_auth::BrowserSignIn::new(move |url: &str| {
                let _ = events.send(AccountEvent::SignInBrowser {
                    account: name,
                    url: url.to_owned(),
                });
                Ok(())
            });
            browser.login_hint = Some(account.address.clone());
            quill_auth::SignInMethod::Browser(browser)
        };
        oauth::sign_in(&http, &setup, method, Arc::clone(&self.secrets), account)
            .await
            .map_err(|err| {
                sentence(&match err {
                    // Their own words say it; "backend error" and "sign-in
                    // failed" in front add nothing after "is not signed in".
                    Error::Backend(reason) | Error::Auth(reason) => reason,
                    other => other.to_string(),
                })
            })
    }

    fn config_text(&self) -> Result<String, String> {
        match std::fs::read_to_string(&self.config_file) {
            Ok(text) => Ok(text),
            // No config yet is an empty one.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(format!("config.toml cannot be read: {err}")),
        }
    }

    /// The application id the form offers for a Microsoft account: the one
    /// an account in the config already signs in with, if any names one
    /// (the same registration serves every account), and otherwise Quill's
    /// own.
    async fn microsoft_application(self: &Arc<Self>) -> Option<String> {
        let service = Arc::clone(self);
        // The config file blocks.
        tokio::task::spawn_blocking(move || {
            let config = Config::parse(&service.config_text().ok()?).ok()?;
            config
                .accounts
                .iter()
                .filter(|account| account.kind == AccountType::Graph)
                .find_map(|account| {
                    let id = account.oauth_client_id.as_deref()?.trim();
                    (!id.is_empty()).then(|| id.to_owned())
                })
        })
        .await
        .ok()
        .flatten()
        .or_else(|| Some(oauth::MICROSOFT_APPLICATION.to_owned()))
    }

    fn register(&self, account: &AccountConfig) -> Result<LocalAccountId, String> {
        self.store
            .lock()
            .map_err(|_| "the store is unavailable after an earlier failure".to_owned())?
            .upsert_account(&account.name, &account.address, account.kind.as_str())
            .map_err(|err| err.to_string())
    }

    /// Writes the account down and returns it ready to sync. Nothing is
    /// left half done: if the config cannot be written the password is
    /// taken out of the Keychain again.
    fn save(
        &self,
        draft: &AccountDraft,
        password: &str,
        trusting: &Trusting,
    ) -> Result<SyncAccount, String> {
        let before = self.config_text()?;
        let existing = Config::parse(&before)
            .map_err(|err| format!("config.toml has a mistake to fix first: {err}"))?;
        if existing
            .accounts
            .iter()
            .any(|account| account.name.eq_ignore_ascii_case(&draft.name))
        {
            return Err(format!(
                "there is already an account called {}. Go back and give this one another name",
                draft.name
            ));
        }
        let pinned = match trusting {
            Trusting::Pinned(fingerprint) => Some(fingerprint.as_str()),
            Trusting::System | Trusting::Once(_) => None,
        };
        let after = config_edit::add_account(&before, draft, pinned)?;
        let config = Config::parse(&after).map_err(|err| err.to_string())?;
        let account = config
            .accounts
            .last()
            .ok_or("the account was not added to config.toml")?;
        let mut imap = imap_config(account)?;

        let key = imap_password_name(&imap);
        self.secrets
            .set(&key, password)
            .map_err(|err| err.to_string())?;
        if let Err(err) = config_edit::write(&self.config_file, &after) {
            let _ = self.secrets.delete(&key);
            return Err(format!("config.toml cannot be written: {err}"));
        }
        // Trusted for this run only: held here, written nowhere.
        if let Trusting::Once(fingerprint) = trusting {
            imap.trust.pinned_sha256 = Some(parse_fingerprint(fingerprint)?);
        }
        // An account added here signs in with the password just typed.
        Ok(sync_account(
            self.register(account)?,
            account,
            imap,
            &self.secrets,
            None,
        ))
    }

    /// Writes down an account that signs in with Microsoft and returns it
    /// ready to sync, built as one found in the config at startup is.
    /// Nothing is asked of Microsoft and nothing goes in the Keychain:
    /// signing in is what the user does next.
    fn save_sign_in(&self, draft: &SignInDraft) -> Result<SyncAccount, String> {
        let before = self.config_text()?;
        let existing = Config::parse(&before)
            .map_err(|err| format!("config.toml has a mistake to fix first: {err}"))?;
        if existing
            .accounts
            .iter()
            .any(|account| account.name.eq_ignore_ascii_case(&draft.name))
        {
            return Err(format!(
                "there is already an account called {}. Give this one another name",
                draft.name
            ));
        }
        // A sign-in is kept under the address, so a second Microsoft
        // account at the same address would be the first one twice.
        if let Some(same) = existing.accounts.iter().find(|account| {
            account.kind == AccountType::Graph
                && account.address.eq_ignore_ascii_case(&draft.address)
        }) {
            return Err(format!(
                "{} is already the Microsoft account for {}",
                same.name, draft.address
            ));
        }
        let after = config_edit::add_sign_in_account(&before, draft)?;
        let config = Config::parse(&after).map_err(|err| err.to_string())?;
        let account = config
            .accounts
            .last()
            .ok_or("the account was not added to config.toml")?;
        // Built before anything is written: an account whose settings
        // could never sign in (a tenant that is not one) is not saved.
        let tokens = oauth::tokens(account, &self.secrets, &mut None)?
            .ok_or("it does not sign in with a provider")?;
        let mut built = oauth::graph_account(LocalAccountId(0), account, tokens)?;
        config_edit::write(&self.config_file, &after)
            .map_err(|err| format!("config.toml cannot be written: {err}"))?;
        built.id = self.register(account)?;
        Ok(built)
    }

    /// Accepts `fingerprint` as the certificate of one of an existing
    /// account's servers, the one it reads from or the one it sends
    /// through: in the config when `pin` is set, and for this run otherwise.
    fn trust(
        &self,
        name: &str,
        fingerprint: &str,
        pin: bool,
        sending: bool,
    ) -> Result<SyncAccount, String> {
        // Not a fingerprint, not accepted: nothing is written or kept.
        parse_fingerprint(fingerprint)?;
        let mut text = self.config_text()?;
        if pin {
            let server = if sending { "smtp" } else { "imap" };
            text = config_edit::pin_fingerprint(&text, name, server, fingerprint)?;
            config_edit::write(&self.config_file, &text)
                .map_err(|err| format!("config.toml cannot be written: {err}"))?;
        }
        let config = Config::parse(&text).map_err(|err| err.to_string())?;
        let mut account = config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .ok_or_else(|| format!("config.toml has no account called {name}"))?
            .clone();
        let mut imap = imap_config(&account)?;
        let id = self.register(&account)?;
        // What was accepted for this run stays accepted when the account's
        // other server is asked about later.
        let mut once = self
            .trusted_once
            .lock()
            .map_err(|_| "the service is unavailable after an earlier failure".to_owned())?;
        if !pin {
            once.insert((name.to_owned(), sending), fingerprint.to_owned());
        }
        if let Some(reading) = once.get(&(name.to_owned(), false)) {
            imap.trust.pinned_sha256 = Some(parse_fingerprint(reading)?);
        }
        if let Some(sender) = once.get(&(name.to_owned(), true))
            && let Some(smtp) = &mut account.smtp
        {
            smtp.fingerprint = Some(sender.clone());
        }
        drop(once);
        if sending {
            // Mail that was waiting on this answer goes now, not at its
            // next retry.
            self.store
                .lock()
                .map_err(|_| "the store is unavailable after an earlier failure".to_owned())?
                .hurry_sends(id, now())
                .map_err(|err| err.to_string())?;
        }
        let tokens = oauth::tokens(&account, &self.secrets, &mut None)?;
        Ok(sync_account(id, &account, imap, &self.secrets, tokens))
    }
}

/// Signs in with the draft's settings and `password`, to find out whether
/// they work. Nothing is kept.
async fn verify(
    draft: &AccountDraft,
    password: String,
    trusting: &Trusting,
) -> Result<(), VerifyProblem> {
    let pinned_sha256 = match trusting {
        Trusting::System => None,
        Trusting::Once(fingerprint) | Trusting::Pinned(fingerprint) => {
            Some(parse_fingerprint(fingerprint).map_err(VerifyProblem::Other)?)
        }
    };
    let ServerDraft {
        host,
        port,
        tls,
        username,
    } = draft.imap.clone();
    let config = ImapConfig {
        host,
        port,
        tls,
        username,
        trust: Trust {
            extra_roots: Vec::new(),
            pinned_sha256,
        },
    };
    let backend = ImapBackend::new(config, quill_core::credentials::password(password.clone()));
    match backend.list_folders().await {
        Ok(_) => {}
        Err(Error::Auth(reason)) => return Err(VerifyProblem::Auth(reason)),
        Err(Error::Certificate(problem)) => return Err(VerifyProblem::Certificate(problem)),
        Err(other) => return Err(VerifyProblem::Other(sentence(&other.to_string()))),
    }
    // Reading works. If the account is to send, sign in there too, and
    // send nothing.
    let Some(smtp) = &draft.smtp else {
        return Ok(());
    };
    let sender = SmtpSender::new(
        SmtpConfig {
            host: smtp.host.clone(),
            port: smtp.port,
            tls: smtp.tls,
            username: smtp.username.clone(),
            trust: Trust::default(),
            auth: SmtpAuth::Automatic,
            hello_name: quill_smtp::DEFAULT_HELLO_NAME.to_owned(),
        },
        quill_core::credentials::password(password),
    );
    match sender.check().await {
        Ok(()) => Ok(()),
        // A certificate this Mac does not trust is asked about when the
        // first message is sent, with the message safe in the Outbox; the
        // account is not held up for it here.
        Err(Error::Certificate(_)) => Ok(()),
        Err(other) => Err(VerifyProblem::Sending(sentence(&other.to_string()))),
    }
}

/// An error message as a sentence: capitalised, with a full stop.
fn sentence(message: &str) -> String {
    let mut letters = message.chars();
    let mut out: String = match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quill_auth::MemorySecrets;
    use quill_core::config::TlsMode;
    use quill_store::Mailbox;
    use quill_sync::{SyncEvent, SyncOptions};
    use quill_tui::setup::Secret;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::discover::{Dns, Prober, Service as SrvRecord};
    use crate::imap_test_server::{Dovecot, PASSWORD, fill};

    const USER: &str = "robin@example.com";

    struct NoDns;

    impl Dns for NoDns {
        fn services<'a>(
            &'a self,
            _: &'a str,
        ) -> std::pin::Pin<Box<dyn Future<Output = Vec<SrvRecord>> + Send + 'a>> {
            Box::pin(async { Vec::new() })
        }

        fn mail_exchangers<'a>(
            &'a self,
            _: &'a str,
        ) -> std::pin::Pin<Box<dyn Future<Output = Vec<String>> + Send + 'a>> {
            Box::pin(async { Vec::new() })
        }
    }

    struct NoProber;

    impl Prober for NoProber {
        fn imap<'a>(
            &'a self,
            _: &'a str,
            _: u16,
            _: TlsMode,
        ) -> std::pin::Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async { false })
        }
    }

    struct Rig {
        service: Arc<Service>,
        secrets: Arc<MemorySecrets>,
        store: Store,
        events: UnboundedReceiver<SyncEvent>,
        dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            Self::at(false)
        }

        /// With whoever is typing at another computer, or at this one.
        fn at(remote: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let database = dir.path().join("quill.db");
            let store = Store::open(&database).unwrap();
            let (events_tx, events) = unbounded_channel();
            let sync = Arc::new(quill_sync::spawn(
                Store::open(&database).unwrap(),
                Some(dir.path().join("blobs")),
                Vec::new(),
                SyncOptions {
                    interval: Duration::from_secs(3600),
                    ..SyncOptions::default()
                },
                events_tx,
            ));
            let secrets = Arc::new(MemorySecrets::default());
            let service = Arc::new(Service {
                config_file: dir.path().join("config").join("config.toml"),
                secrets: secrets.clone(),
                store: Mutex::new(Store::open(&database).unwrap()),
                sync,
                dns: Box::new(NoDns),
                prober: Box::new(NoProber),
                trusted_once: Mutex::default(),
                remote,
            });
            Self {
                service,
                secrets,
                store,
                events,
                dir,
            }
        }

        fn config(&self) -> Config {
            Config::parse(&std::fs::read_to_string(&self.service.config_file).unwrap()).unwrap()
        }

        /// Waits until the store holds `count` messages.
        async fn messages(&mut self, count: usize) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                if self.store.messages(Mailbox::Unified).unwrap().len() == count {
                    return;
                }
                let event = tokio::time::timeout_at(deadline, self.events.recv()).await;
                assert!(event.is_ok(), "the account never synced {count} messages");
            }
        }
    }

    fn draft(server: &Dovecot, tls: TlsMode) -> AccountDraft {
        let config = server.config(tls, USER);
        AccountDraft {
            name: "Personal".to_owned(),
            address: USER.to_owned(),
            imap: ServerDraft {
                host: config.host,
                port: config.port,
                tls,
                username: USER.to_owned(),
            },
            smtp: None,
        }
    }

    fn verify_request(draft: &AccountDraft, password: &str, trusting: Trusting) -> AccountRequest {
        AccountRequest::Verify {
            draft: draft.clone(),
            password: Secret(password.to_owned()),
            trusting,
        }
    }

    /// A sending server on a local port that signs anyone in, or no one,
    /// and counts what it is asked. It takes no mail at all.
    async fn sending_server(accepts: bool) -> (u16, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&heard);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 localhost ESMTP\r\n").await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let verb = line
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    log.lock().unwrap().push(verb.clone());
                    let reply: &[u8] = match verb.as_str() {
                        "EHLO" => b"250-localhost\r\n250 AUTH PLAIN\r\n",
                        "AUTH" if accepts => b"235 2.7.0 welcome\r\n",
                        "AUTH" => b"535 5.7.8 not you\r\n",
                        "QUIT" => b"221 bye\r\n",
                        _ => b"502 not here\r\n",
                    };
                    if write.write_all(reply).await.is_err() || verb == "QUIT" {
                        break;
                    }
                }
            }
        });
        (port, heard)
    }

    #[tokio::test]
    async fn the_sending_server_is_signed_in_to_as_well_and_sent_nothing() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let rig = Rig::new();
        let sending = |port| ServerDraft {
            host: "127.0.0.1".to_owned(),
            port,
            tls: TlsMode::None,
            username: USER.to_owned(),
        };

        // Both servers take the password: the account is good to save.
        let (port, heard) = sending_server(true).await;
        let mut draft = draft(&server, TlsMode::None);
        draft.smtp = Some(sending(port));
        assert_eq!(
            rig.service
                .handle(verify_request(&draft, PASSWORD, Trusting::System))
                .await,
            AccountEvent::Verified(Ok(()))
        );
        assert_eq!(*heard.lock().unwrap(), ["EHLO", "AUTH", "QUIT"]);

        // Reading works and the sending server says no: said as that.
        let (port, _) = sending_server(false).await;
        draft.smtp = Some(sending(port));
        match rig
            .service
            .handle(verify_request(&draft, PASSWORD, Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Sending(reason))) => {
                assert!(reason.contains("not you"), "{reason}");
            }
            other => panic!("{other:?}"),
        }

        // A password the reading server refuses never reaches the other.
        let (port, heard) = sending_server(true).await;
        draft.smtp = Some(sending(port));
        match rig
            .service
            .handle(verify_request(&draft, "wrong", Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Auth(_))) => {}
            other => panic!("{other:?}"),
        }
        assert!(heard.lock().unwrap().is_empty());

        // Nothing listening where the sending server should be.
        let closed = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        draft.smtp = Some(sending(closed));
        match rig
            .service
            .handle(verify_request(&draft, PASSWORD, Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Sending(_))) => {}
            other => panic!("{other:?}"),
        }
        // Nothing was kept by any of it: there is still no config at all.
        assert!(!rig.service.config_file.exists());
    }

    #[tokio::test]
    async fn signing_in_checks_the_password_and_keeps_nothing() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let rig = Rig::new();
        let draft = draft(&server, TlsMode::None);
        assert_eq!(
            rig.service
                .handle(verify_request(&draft, PASSWORD, Trusting::System))
                .await,
            AccountEvent::Verified(Ok(()))
        );
        match rig
            .service
            .handle(verify_request(&draft, "wrong", Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Auth(_))) => {}
            other => panic!("{other:?}"),
        }
        // Checking is not saving.
        assert!(!rig.service.config_file.exists());
        assert_eq!(
            rig.secrets.get(&format!("imap:{USER}@localhost")).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_server_that_is_not_there_is_reported_as_a_sentence() {
        let rig = Rig::new();
        let draft = AccountDraft {
            name: "Nowhere".to_owned(),
            address: USER.to_owned(),
            imap: ServerDraft {
                host: "127.0.0.1".to_owned(),
                // Nothing listens on the discard port.
                port: 9,
                tls: TlsMode::None,
                username: USER.to_owned(),
            },
            smtp: None,
        };
        match rig
            .service
            .handle(verify_request(&draft, PASSWORD, Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Other(reason))) => {
                assert!(
                    reason.starts_with("Cannot reach the server") && reason.ends_with('.'),
                    "{reason}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn an_untrusted_certificate_is_reported_and_can_then_be_trusted() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let rig = Rig::new();
        // The test server's certificate is signed by an authority of its
        // own, which this Mac has never heard of.
        let draft = draft(&server, TlsMode::Implicit);
        let problem = match rig
            .service
            .handle(verify_request(&draft, PASSWORD, Trusting::System))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Certificate(problem))) => problem,
            other => panic!("{other:?}"),
        };
        assert_eq!(problem.host, "localhost");
        let fingerprint = problem.fingerprint.clone().unwrap();
        let details = problem.details.clone().unwrap();
        assert!(details.not_before < details.not_after);
        assert!(!details.issuer.is_empty());

        // Held to that certificate, the same server is signed in to.
        for trusting in [
            Trusting::Once(fingerprint.clone()),
            Trusting::Pinned(fingerprint),
        ] {
            assert_eq!(
                rig.service
                    .handle(verify_request(&draft, PASSWORD, trusting))
                    .await,
                AccountEvent::Verified(Ok(()))
            );
        }
        // Held to another, it is not.
        match rig
            .service
            .handle(verify_request(
                &draft,
                PASSWORD,
                Trusting::Once("00".repeat(32)),
            ))
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Certificate(_))) => {}
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn saving_writes_the_config_keeps_the_password_and_starts_syncing() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        fill(&mut server.session(USER).await, "INBOX", 3).await;
        let mut rig = Rig::new();
        let draft = draft(&server, TlsMode::None);
        let saved = rig
            .service
            .handle(AccountRequest::Save {
                draft: draft.clone(),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));

        let config = rig.config();
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].name, "Personal");
        // The password is in the Keychain and nowhere in the file.
        assert_eq!(
            rig.secrets
                .get(&format!("imap:{USER}@localhost"))
                .unwrap()
                .as_deref(),
            Some(PASSWORD)
        );
        let text = std::fs::read_to_string(&rig.service.config_file).unwrap();
        assert!(!text.contains(PASSWORD), "{text}");
        // And its mail arrives without Quill being restarted.
        assert_eq!(rig.store.accounts().unwrap().len(), 1);
        rig.messages(3).await;

        // A second account may not take the first one's name.
        match rig
            .service
            .handle(AccountRequest::Save {
                draft: AccountDraft {
                    name: "personal".to_owned(),
                    ..draft
                },
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::System,
            })
            .await
        {
            AccountEvent::Saved(Err(reason)) => {
                assert!(reason.contains("already an account"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rig.config().accounts.len(), 1);
    }

    #[tokio::test]
    async fn a_config_that_cannot_be_written_leaves_no_password_behind() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let rig = Rig::new();
        // Where the config directory should be, there is a file.
        std::fs::write(rig.dir.path().join("config"), "in the way").unwrap();
        match rig
            .service
            .handle(AccountRequest::Save {
                draft: draft(&server, TlsMode::None),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::System,
            })
            .await
        {
            AccountEvent::Saved(Err(reason)) => assert!(reason.contains("config.toml"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            rig.secrets.get(&format!("imap:{USER}@localhost")).unwrap(),
            None
        );
        assert!(rig.store.accounts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_pinned_account_is_saved_with_its_fingerprint_and_a_once_trusted_one_without() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        fill(&mut server.session(USER).await, "INBOX", 2).await;
        let fingerprint = quill_net::tls::fingerprint(&server.certificates.leaf_der);

        let mut rig = Rig::new();
        let saved = rig
            .service
            .handle(AccountRequest::Save {
                draft: draft(&server, TlsMode::Implicit),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::Pinned(fingerprint.clone()),
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        assert_eq!(
            rig.config().accounts[0]
                .imap
                .as_ref()
                .unwrap()
                .fingerprint
                .as_deref(),
            Some(fingerprint.as_str())
        );
        rig.messages(2).await;

        let mut rig = Rig::new();
        rig.service
            .handle(AccountRequest::Save {
                draft: draft(&server, TlsMode::Implicit),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::Once(fingerprint),
            })
            .await;
        // Nothing about the certificate is written down, yet it syncs now.
        assert_eq!(
            rig.config().accounts[0].imap.as_ref().unwrap().fingerprint,
            None
        );
        rig.messages(2).await;
    }

    #[tokio::test]
    async fn an_existing_accounts_certificate_can_be_trusted_once_or_pinned() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        fill(&mut server.session(USER).await, "INBOX", 2).await;
        let fingerprint = quill_net::tls::fingerprint(&server.certificates.leaf_der);

        for pin in [false, true] {
            let mut rig = Rig::new();
            // The account as the user wrote it, with a password saved.
            let text =
                config_edit::add_account("", &draft(&server, TlsMode::Implicit), None).unwrap();
            config_edit::write(&rig.service.config_file, &text).unwrap();
            rig.secrets
                .set(&format!("imap:{USER}@localhost"), PASSWORD)
                .unwrap();

            let trusted = rig
                .service
                .handle(AccountRequest::Trust {
                    account: "Personal".to_owned(),
                    fingerprint: fingerprint.clone(),
                    pin,
                    sending: false,
                })
                .await;
            assert_eq!(
                trusted,
                AccountEvent::Trusted {
                    account: "Personal".to_owned(),
                    sending: false,
                    result: Ok(())
                }
            );
            rig.messages(2).await;
            let written = rig.config().accounts[0]
                .imap
                .as_ref()
                .unwrap()
                .fingerprint
                .clone();
            assert_eq!(written.is_some(), pin);
        }

        // An account that is not in the config cannot be trusted into being.
        let rig = Rig::new();
        match rig
            .service
            .handle(AccountRequest::Trust {
                account: "Nobody".to_owned(),
                fingerprint,
                pin: true,
                sending: false,
            })
            .await
        {
            AccountEvent::Trusted { result: Err(_), .. } => {}
            other => panic!("{other:?}"),
        }
    }

    fn microsoft(name: &str, address: &str, tenant: Option<&str>) -> AccountRequest {
        AccountRequest::SaveSignIn {
            draft: SignInDraft {
                name: name.to_owned(),
                address: address.to_owned(),
                application_id: "app-1".to_owned(),
                tenant: tenant.map(str::to_owned),
            },
        }
    }

    /// Writes `text` as the rig's config, as the user might have.
    fn write_config(rig: &Rig, text: &str) {
        std::fs::create_dir_all(rig.service.config_file.parent().unwrap()).unwrap();
        std::fs::write(&rig.service.config_file, text).unwrap();
    }

    fn config_text(rig: &Rig) -> String {
        std::fs::read_to_string(&rig.service.config_file).unwrap()
    }

    #[tokio::test]
    async fn a_microsoft_account_is_written_down_and_synced_without_a_password_or_a_restart() {
        let mut rig = Rig::new();
        let saved = rig.service.handle(microsoft("Work", USER, None)).await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));

        // The account as docs/oauth.md has it written by hand, and no more.
        assert_eq!(
            config_text(&rig),
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n"
        );
        // It is in the store, to be listed.
        let accounts = rig.store.accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "Work");
        // Nothing was put in the Keychain: nobody has signed in.
        assert_eq!(
            rig.secrets
                .get("oauth:microsoft:robin@example.com")
                .unwrap(),
            None
        );
        // And the engine has it already: the account says for itself that
        // nobody has signed in to it, having asked nothing of Microsoft.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let event = tokio::time::timeout_at(deadline, rig.events.recv())
                .await
                .expect("the engine never tried the new account")
                .expect("the engine stopped");
            if let SyncEvent::Problem { account, message } = event {
                assert_eq!(account, "Work");
                assert!(message.contains("nobody has signed in"), "{message}");
                break;
            }
        }

        // With a tenant, that is written too.
        let rig = Rig::new();
        let saved = rig
            .service
            .handle(microsoft("Work", USER, Some("example.com")))
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        assert_eq!(
            config_text(&rig),
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\noauth_tenant = \"example.com\"\n"
        );
        assert_eq!(
            rig.config().accounts[0].oauth_tenant.as_deref(),
            Some("example.com")
        );
    }

    #[tokio::test]
    async fn a_microsoft_account_is_added_under_what_the_user_wrote_which_is_kept() {
        let rig = Rig::new();
        let before = "# My settings\n[ui]\ntheme = \"light\"   # easier on the eyes\n\n\
                      [[account]]\nname = \"Personal\"   # home\ntype = \"imap\"\naddress = \"robin@example.org\"\n\
                      imap = { host = \"mail.example.org\" }\n";
        write_config(&rig, before);
        let saved = rig.service.handle(microsoft("Work", USER, None)).await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        assert_eq!(
            config_text(&rig),
            format!(
                "{before}\n[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n"
            )
        );
        let config = rig.config();
        assert_eq!(config.accounts.len(), 2);
        assert_eq!(config.accounts[1].kind, AccountType::Graph);
    }

    #[tokio::test]
    async fn a_microsoft_account_with_no_application_of_its_own_signs_in_as_quill() {
        // Left empty, or left as Quill's own id: either way the config
        // names no application, and the account uses Quill's.
        for (name, application) in [("Empty", "  "), ("Quills", oauth::MICROSOFT_APPLICATION)] {
            let rig = Rig::new();
            let saved = rig
                .service
                .handle(AccountRequest::SaveSignIn {
                    draft: SignInDraft {
                        name: name.to_owned(),
                        address: "sam@example.com".to_owned(),
                        application_id: application.to_owned(),
                        tenant: None,
                    },
                })
                .await;
            assert_eq!(saved, AccountEvent::Saved(Ok(name.to_owned())));
            assert_eq!(
                config_text(&rig),
                format!(
                    "[[account]]\nname = \"{name}\"\ntype = \"graph\"\naddress = \"sam@example.com\"\n"
                )
            );
            let config = rig.config();
            let setup = oauth::setup(&config.accounts[0]).unwrap().unwrap();
            assert_eq!(setup.client.id, oauth::MICROSOFT_APPLICATION);
        }
    }

    #[tokio::test]
    async fn a_microsoft_account_that_cannot_be_is_refused_and_nothing_is_written() {
        let rig = Rig::new();
        let before = "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n\n\
                      [[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n";
        write_config(&rig, before);
        let refused = |event| match event {
            AccountEvent::Saved(Err(reason)) => reason,
            other => panic!("{other:?}"),
        };

        // A name another account has, whatever its kind and its capitals.
        for name in ["Personal", "work"] {
            let reason = refused(
                rig.service
                    .handle(microsoft(name, "sam@example.com", None))
                    .await,
            );
            assert!(reason.contains("already an account called"), "{reason}");
        }
        // An address a Microsoft account already signs in as: the sign-in
        // is kept under the address, so it would be the same account twice.
        let reason = refused(
            rig.service
                .handle(microsoft("Second", "Robin@Example.com", None))
                .await,
        );
        assert!(
            reason.contains("Work is already the Microsoft account for"),
            "{reason}"
        );
        // A tenant that is not one.
        let reason = refused(
            rig.service
                .handle(microsoft("Second", "sam@example.com", Some("not/a/tenant")))
                .await,
        );
        assert!(reason.contains("tenant"), "{reason}");
        // None of it changed the file or listed an account.
        assert_eq!(config_text(&rig), before);
        assert!(rig.store.accounts().unwrap().is_empty());

        // An address a password account has is another matter: that one
        // signs in somewhere else.
        let saved = rig
            .service
            .handle(microsoft("Second", "robin@example.org", None))
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Second".to_owned())));

        // A config with a mistake in it is not added to.
        let rig = Rig::new();
        write_config(&rig, "[ui\ntheme = ");
        let reason = refused(rig.service.handle(microsoft("Work", USER, None)).await);
        assert!(reason.contains("mistake to fix first"), "{reason}");
        assert_eq!(config_text(&rig), "[ui\ntheme = ");

        // One that cannot be written says so, and no account is listed.
        let rig = Rig::new();
        std::fs::write(rig.dir.path().join("config"), "in the way").unwrap();
        let reason = refused(rig.service.handle(microsoft("Work", USER, None)).await);
        assert!(reason.contains("config.toml"), "{reason}");
        assert!(rig.store.accounts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn discovery_offers_the_application_id_a_microsoft_account_already_has() {
        let discovered = |event| match event {
            AccountEvent::Discovered { found, .. } => found,
            other => panic!("{other:?}"),
        };
        let look = AccountRequest::Discover {
            address: "sam@example.com".to_owned(),
        };

        // No config at all: Quill's own application is offered.
        let quills = Some(oauth::MICROSOFT_APPLICATION);
        let rig = Rig::new();
        let found = discovered(rig.service.handle(look.clone()).await);
        assert_eq!(found.microsoft_application.as_deref(), quills);

        // Only an application registered with Microsoft is offered: not
        // the one a Gmail account signs in to Google as.
        write_config(
            &rig,
            "[[account]]\nname = \"Mail\"\ntype = \"imap\"\naddress = \"robin@example.org\"\nauth = \"oauth\"\noauth_client_id = \"google-app\"\nimap = { host = \"mail.example.org\" }\n",
        );
        let found = discovered(rig.service.handle(look.clone()).await);
        assert_eq!(found.microsoft_application.as_deref(), quills);

        // A Microsoft account's is, trimmed; one with none is passed over.
        write_config(
            &rig,
            "[[account]]\nname = \"Old\"\ntype = \"graph\"\naddress = \"old@example.com\"\n\n\
             [[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \" app-1 \"\n",
        );
        let found = discovered(rig.service.handle(look.clone()).await);
        assert_eq!(found.microsoft_application.as_deref(), Some("app-1"));
        // The rest of the answer is discovery's own.
        assert_eq!(found.draft.address, "sam@example.com");
        assert!(!found.found);

        // A config that cannot be read still gets Quill's own, and
        // discovery still answers.
        write_config(&rig, "[ui\ntheme = ");
        let found = discovered(rig.service.handle(look).await);
        assert_eq!(found.microsoft_application.as_deref(), quills);
    }

    #[tokio::test]
    async fn a_microsoft_account_just_saved_is_one_the_sign_in_can_be_started_for() {
        let rig = Rig::new();
        write_config(
            &rig,
            "[[account]]\nname = \"Other\"\ntype = \"graph\"\naddress = \"other@example.com\"\noauth_client_id = \"app-0\"\n",
        );
        let (requests, mut events) = link(&rig);
        // What the form asks for, in its order: saved, then signed in to.
        requests
            .send(microsoft("Work", USER, Some("example.com")))
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            AccountEvent::Saved(Ok("Work".to_owned()))
        );
        requests
            .send(AccountRequest::SignIn {
                account: Some("Work".to_owned()),
            })
            .unwrap();
        // The browser is asked for, for the new account and its own
        // application and tenant. Nobody opens it here: nothing is sent.
        let AccountEvent::SignInBrowser { account, url } = next(&mut events).await else {
            panic!("expected to be sent to the browser");
        };
        assert_eq!(account, "Work");
        assert!(url.contains("/example.com/oauth2/v2.0/authorize?"), "{url}");
        assert!(url.contains("client_id=app-1"), "{url}");
        assert!(url.contains("login_hint=robin%40example.com"), "{url}");
        requests.send(AccountRequest::CancelSignIn).unwrap();
        assert_eq!(
            rig.secrets
                .get("oauth:microsoft:robin@example.com")
                .unwrap(),
            None
        );
    }

    /// A stand-in for the sign-in service, and a config with one account
    /// that signs in there and one that has a password.
    async fn provider(rig: &Rig) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        std::fs::create_dir_all(rig.service.config_file.parent().unwrap()).unwrap();
        std::fs::write(
            &rig.service.config_file,
            format!(
                "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"robin@example.org\"\nimap = {{ host = \"mail.example.org\" }}\n\n[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n\n[account.oauth]\nauthorize_url = \"{0}/authorize\"\ntoken_url = \"{0}/token\"\ndevice_url = \"{0}/devicecode\"\n",
                server.uri()
            ),
        )
        .unwrap();
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "short-lived",
                    "refresh_token": "to-keep",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/devicecode"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "device_code": "long-and-private",
                    "user_code": "WXYZ-1234",
                    "verification_uri": "https://login.example.com/device",
                    "expires_in": 600,
                    "interval": 1,
                })),
            )
            .mount(&server)
            .await;
        server
    }

    async fn next(events: &mut UnboundedReceiver<AccountEvent>) -> AccountEvent {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("the service said nothing")
            .expect("the service stopped")
    }

    /// The service as the UI holds it: its two ends.
    fn link(
        rig: &Rig,
    ) -> (
        tokio::sync::mpsc::UnboundedSender<AccountRequest>,
        UnboundedReceiver<AccountEvent>,
    ) {
        // The rig's service is shared with the test; run a second like it.
        let service = Service {
            config_file: rig.service.config_file.clone(),
            secrets: rig.secrets.clone(),
            store: Mutex::new(Store::open(&rig.dir.path().join("quill.db")).unwrap()),
            sync: Arc::clone(&rig.service.sync),
            dns: Box::new(NoDns),
            prober: Box::new(NoProber),
            trusted_once: Mutex::default(),
            remote: rig.service.remote,
        };
        let link = spawn(service);
        (link.requests.unwrap(), link.events)
    }

    #[tokio::test]
    async fn from_another_computer_the_account_that_needs_it_signs_in_with_a_code() {
        let rig = Rig::at(true);
        let _server = provider(&rig).await;
        let (requests, mut events) = link(&rig);
        // The account the user is looking at has a password; the one that
        // needs signing in to is found.
        requests
            .send(AccountRequest::SignIn {
                account: Some("Personal".to_owned()),
            })
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            AccountEvent::SignInCode {
                account: "Work".to_owned(),
                address: "https://login.example.com/device".to_owned(),
                code: "WXYZ-1234".to_owned(),
                minutes: 10,
            }
        );
        assert_eq!(
            next(&mut events).await,
            AccountEvent::SignedIn {
                account: "Work".to_owned(),
                result: Ok(()),
            }
        );
        assert_eq!(
            rig.secrets
                .get("oauth:microsoft:robin@example.com")
                .unwrap(),
            Some("to-keep".to_owned())
        );
    }

    #[tokio::test]
    async fn at_the_mac_the_browser_is_asked_for_and_giving_up_keeps_nothing() {
        let rig = Rig::new();
        let _server = provider(&rig).await;
        let (requests, mut events) = link(&rig);
        requests
            .send(AccountRequest::SignIn { account: None })
            .unwrap();
        let AccountEvent::SignInBrowser { account, url } = next(&mut events).await else {
            panic!("expected to be sent to the browser");
        };
        assert_eq!(account, "Work");
        assert!(
            url.contains("/authorize?") && url.contains("client_id=app-1"),
            "{url}"
        );
        assert!(url.contains("login_hint=robin%40example.com"), "{url}");
        // Quill is listening for the browser's answer.
        let listener = url
            .split('&')
            .find_map(|pair| pair.strip_prefix("redirect_uri="))
            .unwrap()
            .replace("%3A", ":")
            .replace("%2F", "/")
            .replace("http://localhost", "127.0.0.1");
        assert!(tokio::net::TcpStream::connect(&listener).await.is_ok());

        // Other requests are answered while it waits.
        requests
            .send(AccountRequest::Trust {
                account: "Nobody".to_owned(),
                fingerprint: "ab".repeat(32),
                pin: false,
                sending: false,
            })
            .unwrap();
        assert!(matches!(
            next(&mut events).await,
            AccountEvent::Trusted { result: Err(_), .. }
        ));

        requests.send(AccountRequest::CancelSignIn).unwrap();
        // The listener goes, nothing more is said, and nothing was kept.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::net::TcpStream::connect(&listener).await.is_ok() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "still listening after giving up"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(events.try_recv().is_err());
        assert_eq!(
            rig.secrets
                .get("oauth:microsoft:robin@example.com")
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn with_no_account_to_sign_in_it_says_so() {
        let rig = Rig::new();
        std::fs::create_dir_all(rig.service.config_file.parent().unwrap()).unwrap();
        std::fs::write(
            &rig.service.config_file,
            "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"robin@example.org\"\nimap = { host = \"mail.example.org\" }\n",
        )
        .unwrap();
        let (requests, mut events) = link(&rig);
        requests
            .send(AccountRequest::SignIn {
                account: Some("Personal".to_owned()),
            })
            .unwrap();
        match next(&mut events).await {
            AccountEvent::SignedIn {
                account,
                result: Err(reason),
            } => {
                assert_eq!(account, "");
                assert!(reason.starts_with("No account here signs in"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A stand-in for a mailing list's server.
    async fn list_server() -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/u/r0b1n"))
            .and(wiremock::matchers::header(
                "content-type",
                "application/x-www-form-urlencoded",
            ))
            .and(wiremock::matchers::body_string(
                "List-Unsubscribe=One-Click",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::path("/gone"))
            .respond_with(wiremock::ResponseTemplate::new(410))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::path("/slow"))
            .respond_with(
                wiremock::ResponseTemplate::new(202).set_delay(Duration::from_millis(1500)),
            )
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn unsubscribing_asks_the_lists_server_once_and_says_how_it_went() {
        let rig = Rig::new();
        let server = list_server().await;
        let (requests, mut events) = link(&rig);

        requests
            .send(AccountRequest::Unsubscribe {
                url: format!("{}/u/r0b1n", server.uri()),
            })
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            AccountEvent::Unsubscribed {
                host: "127.0.0.1".to_owned(),
                result: Ok(()),
            }
        );
        // One request, and it was the POST the standard describes: the
        // stand-in answers nothing else at that address.
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].method.as_str(), "POST");
        assert_eq!(received[0].body, b"List-Unsubscribe=One-Click");
        assert!(!received[0].headers.contains_key("cookie"));
        assert!(!received[0].headers.contains_key("authorization"));

        // A server that says no is reported as that, with its number.
        requests
            .send(AccountRequest::Unsubscribe {
                url: format!("{}/gone", server.uri()),
            })
            .unwrap();
        match next(&mut events).await {
            AccountEvent::Unsubscribed {
                host,
                result: Err(reason),
            } => {
                assert_eq!(host, "127.0.0.1");
                assert!(reason.contains("refused the request (410)"), "{reason}");
            }
            other => panic!("{other:?}"),
        }

        // An address that is not HTTPS is asked nothing at all. (Only a
        // test's own server on this machine is let off that.)
        for unfit in [
            "http://lists.example.com/u/r0b1n",
            "https://lists.example.com@127.0.0.1/u/r0b1n",
            "https://127.0.0.1/u/r0b1n",
            "mailto:leave@lists.example.com",
        ] {
            requests
                .send(AccountRequest::Unsubscribe {
                    url: unfit.to_owned(),
                })
                .unwrap();
            match next(&mut events).await {
                AccountEvent::Unsubscribed {
                    host,
                    result: Err(reason),
                } => {
                    assert_eq!(host, "");
                    assert!(reason.contains("not one Quill will use"), "{reason}");
                }
                other => panic!("{unfit}: {other:?}"),
            }
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        // None of it touched the config or the Keychain.
        assert!(!rig.service.config_file.exists());
    }

    #[tokio::test]
    async fn a_list_that_is_slow_to_answer_holds_nothing_else_up() {
        let rig = Rig::new();
        let server = list_server().await;
        let (requests, mut events) = link(&rig);
        requests
            .send(AccountRequest::Unsubscribe {
                url: format!("{}/slow", server.uri()),
            })
            .unwrap();
        // Asked after it, answered before it.
        requests
            .send(AccountRequest::Trust {
                account: "Nobody".to_owned(),
                fingerprint: "ab".repeat(32),
                pin: false,
                sending: false,
            })
            .unwrap();
        assert!(matches!(
            next(&mut events).await,
            AccountEvent::Trusted { result: Err(_), .. }
        ));
        assert_eq!(
            next(&mut events).await,
            AccountEvent::Unsubscribed {
                host: "127.0.0.1".to_owned(),
                result: Ok(()),
            }
        );
    }

    #[test]
    fn messages_become_sentences() {
        assert_eq!(
            sentence("cannot reach the server: refused"),
            "Cannot reach the server: refused."
        );
        assert_eq!(sentence("Already fine."), "Already fine.");
        assert_eq!(sentence(""), "");
    }
}
