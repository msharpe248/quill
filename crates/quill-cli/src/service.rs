//! The accounts service: what the setup flow in the UI asks for, done.
//!
//! The UI describes an account and asks; this finds the server, signs in to
//! check, and only then writes the account to `config.toml`, its password
//! to the Keychain, and hands it to the sync engine. Accepting a server's
//! certificate for an account that already exists goes the same way.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quill_auth::SecretStore;
use quill_core::config::{AccountConfig, Config};
use quill_core::{Backend, Error};
use quill_imap::{ImapBackend, ImapConfig, Trust};
use quill_smtp::{SmtpAuth, SmtpConfig, SmtpSender};
use quill_store::Store;
use quill_sync::{SyncAccount, SyncHandle};
use quill_tui::app::AccountsLink;
use quill_tui::setup::{
    AccountDraft, AccountEvent, AccountRequest, ServerDraft, Trusting, VerifyProblem,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::accounts::{imap_config, imap_password_name, parse_fingerprint, sync_account};
use crate::config_edit;
use crate::discover::{self, Dns, Prober};
use crate::oauth;

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
}

/// Starts the service on the current Tokio runtime and returns the UI's end
/// of it. It stops when the UI lets go of that.
pub fn spawn(service: Service) -> AccountsLink {
    let (requests_tx, mut requests) = unbounded_channel();
    let (events_tx, events) = unbounded_channel();
    tokio::spawn(async move {
        let service = Arc::new(service);
        while let Some(request) = requests.recv().await {
            let event = service.handle(request).await;
            if events_tx.send(event).is_err() {
                break;
            }
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
                let found = discover::discover(&address, &*self.dns, &*self.prober).await;
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
        }
    }

    fn config_text(&self) -> Result<String, String> {
        match std::fs::read_to_string(&self.config_file) {
            Ok(text) => Ok(text),
            // No config yet is an empty one.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(format!("config.toml cannot be read: {err}")),
        }
    }

    fn register(
        &self,
        account: &AccountConfig,
    ) -> Result<quill_core::types::LocalAccountId, String> {
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
