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
//!
//! And it reads and writes no file itself, so a choice made in the UI that
//! is to outlast the run is written into the config here (`remember`), and
//! the app's settings window is answered from here (`settings`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    AccountDraft, AccountEvent, AccountRequest, Preference, ServerDraft, SignInDraft, Trusting,
    VerifyProblem,
};
use quill_tui::setup::{GoogleApplication, Leaving};
use tokio::sync::mpsc::unbounded_channel;

use crate::accounts::{imap_config, imap_password_name, parse_fingerprint, sync_account};
use crate::config_edit;
use crate::discover::{self, Dns, Prober};
use crate::oauth;
use crate::settings;

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

/// Starts a service that does one thing: reads and writes the settings in
/// the config. It remembers the choices the user makes in the UI, and
/// answers for the app's settings window. It is what the demo mailbox has
/// in place of the whole service: it has no accounts to add or sign in,
/// and still a config that says how Quill starts. Anything else asked of
/// it goes unanswered, as it does with no service at all.
pub fn spawn_remembering(config_file: PathBuf) -> AccountsLink {
    let (requests_tx, mut requests) = unbounded_channel();
    let (events_tx, events) = unbounded_channel();
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let event = match request {
                AccountRequest::Remember(preference) => AccountEvent::Remembered(
                    remember_off_thread(config_file.clone(), preference).await,
                ),
                AccountRequest::Settings(change) => {
                    settings::answer_off_thread(config_file.clone(), change).await
                }
                AccountRequest::Keys(change) => {
                    settings::keys_off_thread(config_file.clone(), change).await
                }
                AccountRequest::Themes(change) => {
                    settings::themes_off_thread(config_file.clone(), change).await
                }
                _ => continue,
            };
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

/// [`remember`], off the async threads: the config file blocks.
async fn remember_off_thread(config_file: PathBuf, preference: Preference) -> Result<(), String> {
    tokio::task::spawn_blocking(move || remember(&config_file, preference))
        .await
        .unwrap_or_else(|err| Err(err.to_string()))
}

/// Writes a choice made in the UI into `config.toml`, leaving the rest of
/// the file as the user wrote it. A config Quill cannot read is not
/// touched.
fn remember(config_file: &Path, preference: Preference) -> Result<(), String> {
    let before = match std::fs::read_to_string(config_file) {
        Ok(text) => text,
        // No config yet is an empty one.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("config.toml cannot be read: {err}")),
    };
    let after = match preference {
        Preference::WebReading(on) => config_edit::set_web_reading(&before, on)?,
        Preference::Sidebar(on) => config_edit::set_sidebar(&before, on)?,
        Preference::SidebarWidth(columns) => config_edit::set_sidebar_width(&before, columns)?,
        Preference::ReaderWidth(share) => config_edit::set_reader_width(&before, share)?,
        Preference::EditorTips(on) => config_edit::set_editor_tips(&before, on)?,
        Preference::AgentsOff => config_edit::stop_agents(&before)?,
    };
    if after == before {
        return Ok(());
    }
    config_edit::write(config_file, &after)
        .map_err(|err| format!("config.toml cannot be written: {err}"))
}

impl Service {
    pub async fn handle(self: &Arc<Self>, request: AccountRequest) -> AccountEvent {
        match request {
            AccountRequest::Discover { address } => {
                let mut found = discover::discover(&address, &*self.dns, &*self.prober).await;
                // The form reads no config: what it may offer from there
                // goes with the answer.
                found.microsoft_application = self.microsoft_application().await;
                found.microsoft_own =
                    found.microsoft_application.as_deref() == Some(oauth::MICROSOFT_APPLICATION);
                found.google_application = self.google_application().await.map(Box::new);
                found.google_own = match (&found.google_application, oauth::GOOGLE_APPLICATION) {
                    (Some(offered), Some((id, _))) => offered.client_id == id,
                    _ => false,
                };
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
            AccountRequest::EditAsk { account } => {
                let service = Arc::clone(self);
                let name = account.clone();
                // The config file blocks.
                let found = tokio::task::spawn_blocking(move || service.as_it_is(&name))
                    .await
                    .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::EditAsked {
                    account,
                    found: found.map(Box::new),
                }
            }
            AccountRequest::VerifyChange {
                account,
                draft,
                password,
                trusting,
            } => {
                // With none typed, the one Quill has is what is tried:
                // the Keychain blocks.
                let password = match password {
                    Some(typed) => Ok(typed.0),
                    None => {
                        let service = Arc::clone(self);
                        tokio::task::spawn_blocking(move || service.kept_password(&account))
                            .await
                            .unwrap_or_else(|err| Err(err.to_string()))
                    }
                };
                AccountEvent::Verified(match password {
                    Ok(password) => verify(&draft, password, &trusting).await,
                    Err(reason) => Err(VerifyProblem::Password(reason)),
                })
            }
            AccountRequest::Change {
                account,
                draft,
                password,
                trusting,
            } => {
                let service = Arc::clone(self);
                let name = account.clone();
                // Files, the Keychain and the store all block.
                let saved = tokio::task::spawn_blocking(move || {
                    service.change(
                        &name,
                        &draft,
                        password.as_ref().map(|typed| typed.0.as_str()),
                        &trusting,
                    )
                })
                .await
                .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::Saved(saved.map(|changed| {
                    // It takes the place of the account as it was, whose
                    // connections are let go of.
                    self.sync.set_account(changed);
                    // Mail that was waiting on this goes now.
                    self.sync.send(quill_sync::SyncRequest::Queue);
                    account
                }))
            }
            AccountRequest::RemoveAsk { account } => {
                let service = Arc::clone(self);
                let name = account.clone();
                // The config file and the store block.
                let found = tokio::task::spawn_blocking(move || service.leaving(&name))
                    .await
                    .unwrap_or_else(|err| Err(err.to_string()));
                AccountEvent::RemoveAsked { account, found }
            }
            AccountRequest::Remove { account } => {
                let result = self.remove(&account).await;
                AccountEvent::Removed { account, result }
            }
            AccountRequest::Unsubscribe { url } => {
                let (host, result) = unsubscribe::one_click(&url).await;
                AccountEvent::Unsubscribed { host, result }
            }
            AccountRequest::Remember(preference) => AccountEvent::Remembered(
                remember_off_thread(self.config_file.clone(), preference).await,
            ),
            AccountRequest::Settings(change) => {
                settings::answer_off_thread(self.config_file.clone(), change).await
            }
            AccountRequest::Keys(change) => {
                settings::keys_off_thread(self.config_file.clone(), change).await
            }
            AccountRequest::Themes(change) => {
                settings::themes_off_thread(self.config_file.clone(), change).await
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

    /// The account called `name` in the config as it stands, with the
    /// config's text: or why there is none to remove.
    fn account_named(&self, name: &str) -> Result<(String, Config, AccountConfig), String> {
        let text = self.config_text()?;
        let config = Config::parse(&text)
            .map_err(|err| format!("The settings file has a mistake to fix first: {err}"))?;
        let account = config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .cloned()
            .ok_or_else(|| format!("There is no account called {name} in the settings file."))?;
        Ok((text, config, account))
    }

    /// The store's own name for the account called `name`, if it has it.
    fn stored(&self, name: &str) -> Result<Option<LocalAccountId>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "the store is unavailable after an earlier failure".to_owned())?;
        Ok(store
            .accounts()
            .map_err(|err| err.to_string())?
            .into_iter()
            .find(|account| account.name == name)
            .map(|account| account.id))
    }

    /// What removing the account called `name` would lose. Changes nothing.
    fn leaving(&self, name: &str) -> Result<Leaving, String> {
        let (_, _, account) = self.account_named(name)?;
        let leftovers = match self.stored(name)? {
            Some(id) => self
                .store
                .lock()
                .map_err(|_| "the store is unavailable after an earlier failure".to_owned())?
                .account_leftovers(id)
                .map_err(|err| err.to_string())?,
            // Never synced: there is nothing of its own here.
            None => quill_store::Leftovers::default(),
        };
        Ok(Leaving {
            address: account.address.clone(),
            // A POP3 account's mail is taken off its server as it is
            // fetched, and kept here.
            only_copy: account.kind == AccountType::Pop3,
            drafts: leftovers.drafts,
            outbox: leftovers.outbox,
            queued: leftovers.queued,
            local: leftovers.local,
        })
    }

    /// Removes the account called `name`, which the user has just agreed
    /// to: out of the config, then its syncing stopped, then its mail out
    /// of the store and its secrets out of the Keychain.
    ///
    /// The config goes first, and if it cannot be written nothing else is
    /// done: the account is as it was. Once it is written the account is
    /// removed, and whatever cannot be cleared up after it is said and not
    /// undone (`Ok(Some(..))`): the store would let go of it at the next
    /// start in any case, and a secret left behind harms nothing.
    async fn remove(self: &Arc<Self>, name: &str) -> Result<Option<String>, String> {
        let service = Arc::clone(self);
        let named = name.to_owned();
        // Files and the store block.
        let (id, secrets) = tokio::task::spawn_blocking(move || service.take_out(&named))
            .await
            .unwrap_or_else(|err| Err(err.to_string()))?;
        // From the runtime, as the engine's tasks were started: nothing
        // more is fetched or sent for it, and nothing written to the store
        // behind the removal.
        if let Some(id) = id {
            self.sync.remove_account(id);
        }
        let service = Arc::clone(self);
        Ok(
            tokio::task::spawn_blocking(move || service.clear_up(id, &secrets))
                .await
                .unwrap_or_else(|err| Some(err.to_string())),
        )
    }

    /// Takes the account called `name` out of the config. Returns the
    /// store's name for it, if the store has it, and the secrets that were
    /// its alone: one that an account still in the config goes by is not
    /// among them.
    fn take_out(&self, name: &str) -> Result<(Option<LocalAccountId>, Vec<String>), String> {
        let (before, _, account) = self.account_named(name)?;
        let after = config_edit::remove_account(&before, name)?;
        let left = Config::parse(&after).map_err(|err| err.to_string())?;
        let kept: Vec<String> = left
            .accounts
            .iter()
            .flat_map(crate::accounts::secret_names)
            .collect();
        let secrets = crate::accounts::secret_names(&account)
            .into_iter()
            .filter(|name| !kept.contains(name))
            .collect();
        let id = self.stored(name)?;
        config_edit::write(&self.config_file, &after)
            .map_err(|err| format!("The settings file cannot be written: {err}"))?;
        Ok((id, secrets))
    }

    /// Deletes a removed account's mail and secrets. Returns what could
    /// not be cleared up, in words for the user, if anything.
    fn clear_up(&self, id: Option<LocalAccountId>, secrets: &[String]) -> Option<String> {
        let mut left = Vec::new();
        if let Some(id) = id {
            let deleted = self
                .store
                .lock()
                .map_err(|_| "the store is unavailable".to_owned())
                .and_then(|mut store| store.remove_account(id).map_err(|err| err.to_string()));
            if let Err(reason) = deleted {
                left.push(format!(
                    "The copy of its mail could not be deleted yet ({reason}); it goes when \
                     Quill is next opened."
                ));
            }
        }
        for name in secrets {
            if let Err(reason) = self.secrets.delete(name) {
                left.push(format!(
                    "Its password could not be taken out of the Keychain ({reason}); look for \
                     \"Quill\" in Keychain Access."
                ));
            }
            self.secrets.forget(name);
        }
        (!left.is_empty()).then(|| left.join(" "))
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

    /// The application the form offers for an account that signs in with
    /// Google: the one an account in the config already signs in as, if
    /// any names one (the same registration serves every account), and
    /// otherwise Quill's own, if it has one. `None`: the user has to
    /// register one, and the form asks for it.
    async fn google_application(self: &Arc<Self>) -> Option<GoogleApplication> {
        let service = Arc::clone(self);
        // The config file blocks.
        tokio::task::spawn_blocking(move || {
            let config = Config::parse(&service.config_text().ok()?).ok()?;
            config
                .accounts
                .iter()
                .filter(|account| account.kind == AccountType::Imap && oauth::wanted(account))
                .find_map(|account| {
                    let id = account.oauth_client_id.as_deref()?.trim();
                    (!id.is_empty()).then(|| GoogleApplication {
                        client_id: id.to_owned(),
                        client_secret: account
                            .oauth_client_secret
                            .as_deref()
                            .map(str::trim)
                            .filter(|secret| !secret.is_empty())
                            .map(str::to_owned),
                    })
                })
        })
        .await
        .ok()
        .flatten()
        .or_else(|| {
            oauth::GOOGLE_APPLICATION.map(|(id, _)| GoogleApplication {
                client_id: id.to_owned(),
                client_secret: None,
            })
        })
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

    /// The account called `name` if it is one the form can change: one
    /// that reads its mail over IMAP and signs in with a password, and
    /// whose sending server, if it has one, signs in with the same name.
    /// Otherwise why not, in words for the user.
    fn changeable(&self, name: &str) -> Result<(String, AccountConfig), String> {
        let (text, _, account) = self.account_named(name)?;
        if account.kind != AccountType::Imap {
            return Err(format!(
                "Its mail is not read over IMAP ({}), which is all this form knows how to \
                 change. Its settings are in the settings file.",
                account.kind.as_str()
            ));
        }
        match oauth::setup(&account) {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(
                    "It signs in with its provider, and has no servers or password of its \
                     own to change. To sign in to it again, use Sign In."
                        .to_owned(),
                );
            }
            Err(reason) => return Err(sentence(&reason)),
        }
        let reading = imap_config(&account).map_err(|reason| sentence(&reason))?;
        if let Some(sending) = &account.smtp
            && sending
                .username
                .as_ref()
                .is_some_and(|username| *username != reading.username)
        {
            return Err(
                "Its sending server signs in with a name of its own, which this form does \
                 not show. Change it in the settings file."
                    .to_owned(),
            );
        }
        Ok((text, account))
    }

    /// How the account called `name` is set up now, for the form that
    /// changes it. Changes nothing.
    fn as_it_is(&self, name: &str) -> Result<AccountDraft, String> {
        let (_, account) = self.changeable(name)?;
        let reading = imap_config(&account).map_err(|reason| sentence(&reason))?;
        let smtp = account.smtp.as_ref().map(|server| ServerDraft {
            host: server.host.clone(),
            port: server.port.unwrap_or(SmtpConfig::default_port(server.tls)),
            tls: server.tls,
            username: reading.username.clone(),
        });
        Ok(AccountDraft {
            name: account.name.clone(),
            address: account.address.clone(),
            imap: ServerDraft {
                host: reading.host,
                port: reading.port,
                tls: reading.tls,
                username: reading.username,
            },
            smtp,
        })
    }

    /// The password Quill has for the account called `name`, for trying
    /// changed settings with when none was typed.
    fn kept_password(&self, name: &str) -> Result<String, String> {
        let (_, account) = self.changeable(name)?;
        let reading = imap_config(&account).map_err(|reason| sentence(&reason))?;
        self.secrets
            .get(&imap_password_name(&reading))
            .map_err(|err| sentence(&err.to_string()))?
            .ok_or_else(|| NO_KEPT_PASSWORD.to_owned())
    }

    /// Writes the account called `name` down as `draft` has it and
    /// returns it ready to sync in the place of what it was. Its password
    /// is kept under the name its new settings give it: the one typed,
    /// or with none typed the one Quill had. Nothing is left half done:
    /// if the config cannot be written, the Keychain is as it was.
    fn change(
        &self,
        name: &str,
        draft: &AccountDraft,
        password: Option<&str>,
        trusting: &Trusting,
    ) -> Result<SyncAccount, String> {
        let (before, old) = self.changeable(name)?;
        let was = Config::parse(&before).map_err(|err| err.to_string())?;
        let old_key = imap_password_name(&imap_config(&old)?);
        let pinned = match trusting {
            Trusting::Pinned(fingerprint) => Some(fingerprint.as_str()),
            Trusting::System | Trusting::Once(_) => None,
        };
        let after = config_edit::change_account(&before, name, draft, pinned)?;
        let config = Config::parse(&after).map_err(|err| err.to_string())?;
        let account = config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .ok_or("the account is not in config.toml any more")?
            .clone();
        let mut imap = imap_config(&account)?;
        let key = imap_password_name(&imap);

        let password = match password {
            Some(typed) => typed.to_owned(),
            None => self
                .secrets
                .get(&old_key)
                .map_err(|err| err.to_string())?
                .ok_or(NO_KEPT_PASSWORD)?,
        };
        // A name no account went by before is this change's to take back.
        let new_name = !was
            .accounts
            .iter()
            .flat_map(crate::accounts::secret_names)
            .any(|kept| kept == key);
        self.secrets
            .set(&key, &password)
            .map_err(|err| err.to_string())?;
        if let Err(err) = config_edit::write(&self.config_file, &after) {
            if new_name {
                let _ = self.secrets.delete(&key);
            }
            return Err(format!("config.toml cannot be written: {err}"));
        }
        // What the account went by and no account goes by now.
        let kept: Vec<String> = config
            .accounts
            .iter()
            .flat_map(crate::accounts::secret_names)
            .collect();
        for gone in crate::accounts::secret_names(&old) {
            if !kept.contains(&gone) {
                let _ = self.secrets.delete(&gone);
                self.secrets.forget(&gone);
            }
        }
        // What was accepted for this run was accepted of the servers the
        // account had.
        if let Ok(mut once) = self.trusted_once.lock() {
            once.retain(|(account, _), _| account != name);
        }
        // Trusted for this run only: held here, written nowhere.
        if let Trusting::Once(fingerprint) = trusting {
            imap.trust.pinned_sha256 = Some(parse_fingerprint(fingerprint)?);
        }
        let id = self.register(&account)?;
        // Mail that was waiting, for a server to send through or for one
        // that answers, is tried with the new settings as soon as the
        // engine has the account as it now is: the engine hurries it, when
        // nothing with the old settings can take it first.
        Ok(sync_account(id, &account, imap, &self.secrets, None))
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
        // A sign-in is kept under the provider and the address, so a
        // second account of the same provider at the same address would
        // be the first one twice.
        let google = draft.google.is_some();
        if let Some(same) = existing.accounts.iter().find(|account| {
            account.address.eq_ignore_ascii_case(&draft.address)
                && if google {
                    account.kind == AccountType::Imap && oauth::wanted(account)
                } else {
                    account.kind == AccountType::Graph
                }
        }) {
            return Err(format!(
                "{} is already the {} account for {}",
                same.name,
                if google { "Google" } else { "Microsoft" },
                draft.address
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
        let mut built = if google {
            // Its mail is read and sent as any mail server's is, with the
            // sign-in in the password's place.
            sync_account(
                LocalAccountId(0),
                account,
                imap_config(account)?,
                &self.secrets,
                Some(tokens),
            )
        } else {
            oauth::graph_account(LocalAccountId(0), account, tokens)?
        };
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
        // Mail that was waiting on this answer goes when the engine has
        // the account as it now is, and not at its next retry: the engine
        // hurries what an account it is given again was waiting to send.
        let tokens = oauth::tokens(&account, &self.secrets, &mut None)?;
        Ok(sync_account(id, &account, imap, &self.secrets, tokens))
    }
}

/// Said when an account is changed with no password typed and Quill has
/// none kept for it.
const NO_KEPT_PASSWORD: &str = "Quill has no password kept for this account. Type it here.";

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

/// The time now, for the tests' own mail.
#[cfg(test)]
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
                    // A message is taken whole and kept nowhere.
                    if verb == "DATA" {
                        let _ = write.write_all(b"354 go on\r\n").await;
                        while let Ok(Some(line)) = lines.next_line().await {
                            if line == "." {
                                break;
                            }
                        }
                        if write.write_all(b"250 2.0.0 taken\r\n").await.is_err() {
                            break;
                        }
                        continue;
                    }
                    let reply: &[u8] = match verb.as_str() {
                        "EHLO" => b"250-localhost\r\n250 AUTH PLAIN\r\n",
                        "AUTH" if accepts => b"235 2.7.0 welcome\r\n",
                        "AUTH" => b"535 5.7.8 not you\r\n",
                        "MAIL" | "RCPT" | "RSET" | "NOOP" => b"250 2.0.0 ok\r\n",
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

    /// The whole of what an account that cannot send is put right by: it
    /// was added to read mail only, a message written from it waits, and
    /// the account is then given a server to send through.
    #[tokio::test]
    async fn an_account_given_a_sending_server_sends_what_was_waiting_for_one() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let mut rig = Rig::new();
        let reading = draft(&server, TlsMode::None);
        assert!(reading.smtp.is_none());
        let saved = rig
            .service
            .handle(AccountRequest::Save {
                draft: reading.clone(),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        let account = rig.store.accounts().unwrap()[0].id;

        // A message from it, finished and in the outbox.
        let raw = "From: robin@example.com\r\nTo: ana.ruiz@example.com\r\nSubject: Waiting\r\n\r\nHello.\r\n";
        let hash = quill_store::BlobStore::new(rig.dir.path().join("blobs"))
            .put(raw.as_bytes())
            .unwrap();
        let content = quill_store::DraftContent {
            to: "ana.ruiz@example.com".to_owned(),
            subject: "Waiting".to_owned(),
            body: "Hello.".to_owned(),
            ..Default::default()
        };
        let message = rig
            .store
            .save_draft(None, account, &content, now())
            .unwrap()
            .unwrap();
        assert!(
            rig.store
                .queue_send(
                    message,
                    &hash,
                    USER,
                    &["ana.ruiz@example.com".to_owned()],
                    now(),
                    now(),
                )
                .unwrap()
        );
        rig.service.sync.send(quill_sync::SyncRequest::Queue);
        // It cannot go, and waits with the reason: it is not given up on.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let row = rig.store.draft(message).unwrap().unwrap();
            if let Some(error) = &row.error {
                assert!(error.contains("no server to send through"), "{error}");
                assert_eq!(row.state, quill_store::DraftState::Outbox);
                break;
            }
            let event = tokio::time::timeout_at(deadline, rig.events.recv()).await;
            assert!(event.is_ok(), "the message was never tried");
        }

        // The form opens on the account as it is, and is given a server
        // to send through. No password is typed: the one kept is tried,
        // at both servers, and both take it.
        let as_it_is = match rig
            .service
            .handle(AccountRequest::EditAsk {
                account: "Personal".to_owned(),
            })
            .await
        {
            AccountEvent::EditAsked { found, .. } => *found.unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(as_it_is, reading);
        let (port, heard) = sending_server(true).await;
        let changed = AccountDraft {
            smtp: Some(ServerDraft {
                host: "127.0.0.1".to_owned(),
                port,
                tls: TlsMode::None,
                username: USER.to_owned(),
            }),
            ..as_it_is
        };
        assert_eq!(
            rig.service
                .handle(AccountRequest::VerifyChange {
                    account: "Personal".to_owned(),
                    draft: changed.clone(),
                    password: None,
                    trusting: Trusting::System,
                })
                .await,
            AccountEvent::Verified(Ok(()))
        );
        assert_eq!(*heard.lock().unwrap(), ["EHLO", "AUTH", "QUIT"]);
        let saved = rig
            .service
            .handle(AccountRequest::Change {
                account: "Personal".to_owned(),
                draft: changed,
                password: None,
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        assert_eq!(
            rig.config().accounts[0].smtp.as_ref().unwrap().port,
            Some(port)
        );

        // The message goes, through the new server, without being
        // touched again and without Quill being restarted.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while rig.store.draft(message).unwrap().is_some() {
            let event = tokio::time::timeout_at(deadline, rig.events.recv()).await;
            assert!(event.is_ok(), "the waiting message never went");
        }
        let heard = heard.lock().unwrap().clone();
        assert!(heard.contains(&"DATA".to_owned()), "{heard:?}");
        // And the account still reads its mail: it is one account still.
        assert_eq!(rig.store.accounts().unwrap().len(), 1);
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
    async fn a_synced_account_is_removed_with_its_mail_its_password_and_its_place_in_the_file() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        fill(&mut server.session(USER).await, "INBOX", 3).await;
        let mut rig = Rig::new();
        let saved = rig
            .service
            .handle(AccountRequest::Save {
                draft: draft(&server, TlsMode::None),
                password: Secret(PASSWORD.to_owned()),
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        rig.messages(3).await;
        let secret = format!("imap:{USER}@localhost");
        assert!(rig.secrets.get(&secret).unwrap().is_some());

        // Asking first changes nothing, and says there is nothing here
        // that is not on the server too.
        let asked = rig
            .service
            .handle(AccountRequest::RemoveAsk {
                account: "Personal".to_owned(),
            })
            .await;
        assert_eq!(
            asked,
            AccountEvent::RemoveAsked {
                account: "Personal".to_owned(),
                found: Ok(Leaving {
                    address: USER.to_owned(),
                    ..Leaving::default()
                }),
            }
        );
        assert_eq!(rig.config().accounts.len(), 1);
        assert_eq!(rig.store.messages(Mailbox::Unified).unwrap().len(), 3);

        let removed = rig
            .service
            .handle(AccountRequest::Remove {
                account: "Personal".to_owned(),
            })
            .await;
        assert_eq!(
            removed,
            AccountEvent::Removed {
                account: "Personal".to_owned(),
                result: Ok(None),
            }
        );
        // Gone from the file, the store and the Keychain, at once.
        assert!(rig.config().accounts.is_empty());
        assert!(rig.store.accounts().unwrap().is_empty());
        assert!(rig.store.messages(Mailbox::Unified).unwrap().is_empty());
        assert_eq!(rig.secrets.get(&secret).unwrap(), None);
        // The mail on the server is as it was.
        let on_server = server.session(USER).await.select("INBOX").await.unwrap();
        assert_eq!(on_server.exists, 3);
        // And it is not there to be removed twice.
        match rig
            .service
            .handle(AccountRequest::Remove {
                account: "Personal".to_owned(),
            })
            .await
        {
            AccountEvent::Removed {
                result: Err(reason),
                ..
            } => assert!(reason.contains("no account called Personal"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn removing_one_account_leaves_the_others_and_what_they_share_and_says_what_is_lost() {
        let mut rig = Rig::new();
        let written = "[ui]\ntheme = \"light\"   # mine\n\n\
            [[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"robin@example.com\"\n\
            imap = { host = \"mail.example.com\" }\nsmtp = { host = \"smtp.example.com\" }\n\n\
            [[account]]\nname = \"Home\"\ntype = \"imap\"\naddress = \"robin@example.org\"\n\
            imap = { host = \"mail.example.org\" }\n\n\
            [[account]]\nname = \"Alias\"\ntype = \"imap\"\naddress = \"r@example.org\"\n\
            imap = { host = \"mail.example.org\", username = \"robin@example.org\" }\n\n\
            [[account]]\nname = \"Old\"\ntype = \"pop3\"\naddress = \"robin@example.net\"\n\
            pop3 = { host = \"pop.example.net\" }\n";
        write_config(&rig, written);
        let config = rig.config();
        let mut ids = HashMap::new();
        for account in &config.accounts {
            ids.insert(account.name.clone(), rig.service.register(account).unwrap());
            for name in crate::accounts::secret_names(account) {
                rig.secrets.set(&name, "hunter2").unwrap();
            }
        }
        let secrets = |names: &[&str]| -> Vec<bool> {
            names
                .iter()
                .map(|name| rig.secrets.get(name).unwrap().is_some())
                .collect()
        };
        let all = [
            "imap:robin@example.com@mail.example.com",
            "smtp:robin@example.com@smtp.example.com",
            "imap:robin@example.org@mail.example.org",
            "pop3:robin@example.net@pop.example.net",
        ];
        assert_eq!(secrets(&all), [true, true, true, true]);

        // Work has a message being written, one waiting to go, and Home a
        // draft of its own.
        let content = quill_store::DraftContent::default();
        rig.store
            .save_draft(None, ids["Work"], &content, 1)
            .unwrap();
        let going = rig
            .store
            .save_draft(None, ids["Work"], &content, 1)
            .unwrap()
            .unwrap();
        rig.store
            .queue_send(
                going,
                "hash",
                "robin@example.com",
                &["a@example.com".to_owned()],
                5,
                1,
            )
            .unwrap();
        rig.store
            .save_draft(None, ids["Home"], &content, 1)
            .unwrap();

        let ask = |account: &str| {
            let service = Arc::clone(&rig.service);
            let account = account.to_owned();
            async move {
                match service.handle(AccountRequest::RemoveAsk { account }).await {
                    AccountEvent::RemoveAsked { found, .. } => found,
                    other => panic!("{other:?}"),
                }
            }
        };
        assert_eq!(
            ask("Work").await,
            Ok(Leaving {
                address: "robin@example.com".to_owned(),
                only_copy: false,
                drafts: 1,
                outbox: 1,
                queued: 0,
                local: 0,
            })
        );
        // A POP3 account's copy here may be the only one.
        assert!(ask("Old").await.unwrap().only_copy);
        // A name is the whole name, as the file has it.
        for absent in ["work", "Club", ""] {
            assert!(
                ask(absent)
                    .await
                    .unwrap_err()
                    .contains("There is no account called")
            );
        }
        // Asking changed nothing.
        assert_eq!(config_text(&rig), written);

        let remove = |account: &str| {
            let service = Arc::clone(&rig.service);
            let account = account.to_owned();
            async move {
                match service.handle(AccountRequest::Remove { account }).await {
                    AccountEvent::Removed { result, .. } => result,
                    other => panic!("{other:?}"),
                }
            }
        };
        assert_eq!(remove("Work").await, Ok(None));
        let names = |rig: &Rig| -> Vec<String> {
            rig.config()
                .accounts
                .iter()
                .map(|account| account.name.clone())
                .collect()
        };
        assert_eq!(names(&rig), ["Home", "Alias", "Old"]);
        // What else the user wrote is as they wrote it.
        assert!(config_text(&rig).starts_with("[ui]\ntheme = \"light\"   # mine\n"));
        // Its passwords are gone, for reading and for sending; the others'
        // are not.
        assert_eq!(secrets(&all), [false, false, true, true]);
        // And everything of its own in the store, the outbox included.
        let stored: Vec<String> = rig
            .store
            .accounts()
            .unwrap()
            .into_iter()
            .map(|account| account.name)
            .collect();
        assert_eq!(stored, ["Home", "Alias", "Old"]);
        assert!(rig.store.outbox().unwrap().is_empty());
        assert_eq!(rig.store.drafts().unwrap().len(), 1);

        // Home and Alias sign in to one mailbox with one password: taking
        // one of them away leaves the other its password.
        assert_eq!(remove("Home").await, Ok(None));
        assert_eq!(names(&rig), ["Alias", "Old"]);
        assert_eq!(secrets(&all), [false, false, true, true]);
        assert_eq!(remove("Alias").await, Ok(None));
        assert_eq!(secrets(&all), [false, false, false, true]);
        // The last one too: no accounts is a file Quill reads.
        assert_eq!(remove("Old").await, Ok(None));
        assert!(rig.config().accounts.is_empty());
        assert!(rig.store.accounts().unwrap().is_empty());
        assert_eq!(secrets(&all), [false, false, false, false]);
        let _ = &mut rig;
    }

    /// An account whose servers are this computer's, at ports nothing
    /// listens on: syncing it reaches nobody.
    const CHANGEABLE: &str = "# My accounts.\n[[account]]\nname = \"Work\"\ntype = \"imap\"\n\
        address = \"robin@example.com\"\nsignature = \"Robin\" # as I sign\n\
        imap = { host = \"127.0.0.1\", port = 1, tls = \"none\" }\n";

    fn changed_draft() -> AccountDraft {
        AccountDraft {
            name: "Work".to_owned(),
            address: "robin@example.com".to_owned(),
            imap: ServerDraft {
                host: "localhost".to_owned(),
                port: 1,
                tls: TlsMode::None,
                username: "robin".to_owned(),
            },
            smtp: Some(ServerDraft {
                host: "127.0.0.1".to_owned(),
                port: 2,
                tls: TlsMode::None,
                username: "robin".to_owned(),
            }),
        }
    }

    #[tokio::test]
    async fn an_account_is_shown_as_it_is_and_changed_with_its_password_following_it() {
        let rig = Rig::new();
        write_config(&rig, CHANGEABLE);
        rig.secrets
            .set("imap:robin@example.com@127.0.0.1", "hunter2")
            .unwrap();

        // As it is: what the file says, with what it leaves unsaid.
        let found = match rig
            .service
            .handle(AccountRequest::EditAsk {
                account: "Work".to_owned(),
            })
            .await
        {
            AccountEvent::EditAsked { account, found } => {
                assert_eq!(account, "Work");
                *found.unwrap()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(
            found,
            AccountDraft {
                name: "Work".to_owned(),
                address: "robin@example.com".to_owned(),
                imap: ServerDraft {
                    host: "127.0.0.1".to_owned(),
                    port: 1,
                    tls: TlsMode::None,
                    username: "robin@example.com".to_owned(),
                },
                smtp: None,
            }
        );
        assert_eq!(config_text(&rig), CHANGEABLE);

        // Changed, with no password typed: the one Quill had goes with
        // the account to its new name, and the old name is let go of.
        let saved = rig
            .service
            .handle(AccountRequest::Change {
                account: "Work".to_owned(),
                draft: changed_draft(),
                password: None,
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        let text = config_text(&rig);
        // Everything else the file said is still said.
        assert!(text.starts_with("# My accounts.\n"), "{text}");
        assert!(text.contains("signature = \"Robin\" # as I sign"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        let config = rig.config();
        assert_eq!(config.accounts.len(), 1);
        let account = &config.accounts[0];
        let reading = account.imap.as_ref().unwrap();
        assert_eq!(
            (reading.host.as_str(), reading.username.as_deref()),
            ("localhost", Some("robin"))
        );
        let sending = account.smtp.as_ref().unwrap();
        assert_eq!(
            (sending.host.as_str(), sending.port),
            ("127.0.0.1", Some(2))
        );
        assert_eq!(
            rig.secrets.get("imap:robin@localhost").unwrap().as_deref(),
            Some("hunter2")
        );
        assert_eq!(
            rig.secrets.get("imap:robin@example.com@127.0.0.1").unwrap(),
            None
        );
        // It is the account the store had, not a second one.
        assert_eq!(rig.store.accounts().unwrap().len(), 1);

        // Changed again with a password typed: that is the one kept. The
        // sending server is taken away again with it.
        let saved = rig
            .service
            .handle(AccountRequest::Change {
                account: "Work".to_owned(),
                draft: AccountDraft {
                    smtp: None,
                    ..changed_draft()
                },
                password: Some(Secret("correct horse".to_owned())),
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        assert!(rig.config().accounts[0].smtp.is_none());
        assert_eq!(
            rig.secrets.get("imap:robin@localhost").unwrap().as_deref(),
            Some("correct horse")
        );
    }

    #[tokio::test]
    async fn mail_that_waited_on_an_account_is_tried_again_when_it_is_changed() {
        let mut rig = Rig::new();
        write_config(&rig, CHANGEABLE);
        rig.secrets
            .set("imap:robin@example.com@127.0.0.1", "hunter2")
            .unwrap();
        let account = rig.service.register(&rig.config().accounts[0]).unwrap();
        // A message that waits, with a long time to go before it is
        // looked at again.
        let content = quill_store::DraftContent {
            to: "ana.ruiz@example.com".to_owned(),
            subject: "Waiting".to_owned(),
            ..Default::default()
        };
        let draft = rig
            .store
            .save_draft(None, account, &content, now())
            .unwrap()
            .unwrap();
        assert!(
            rig.store
                .queue_send(
                    draft,
                    &"ee".repeat(32),
                    "robin@example.com",
                    &["ana.ruiz@example.com".to_owned()],
                    now(),
                    now(),
                )
                .unwrap()
        );
        rig.store.claim_send(account, now()).unwrap().unwrap();
        rig.store
            .send_failed(draft, "no server before", Some(now() + 86_400))
            .unwrap();

        let saved = rig
            .service
            .handle(AccountRequest::Change {
                account: "Work".to_owned(),
                draft: changed_draft(),
                password: None,
                trusting: Trusting::System,
            })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        // The engine has it at once, with the account's new settings,
        // and says how that went: not what was said a day's wait ago.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let row = rig.store.draft(draft).unwrap().unwrap();
            if row.error.as_deref() != Some("no server before") {
                break;
            }
            let event = tokio::time::timeout_at(deadline, rig.events.recv()).await;
            assert!(event.is_ok(), "the waiting message was never tried again");
        }
    }

    #[tokio::test]
    async fn an_account_the_form_cannot_describe_is_not_changed_and_why_is_said() {
        let rig = Rig::new();
        let ask = |account: &str| {
            let service = Arc::clone(&rig.service);
            let account = account.to_owned();
            async move {
                match service.handle(AccountRequest::EditAsk { account }).await {
                    AccountEvent::EditAsked { found, .. } => found,
                    other => panic!("{other:?}"),
                }
            }
        };
        let written = "[[account]]\nname = \"Office\"\ntype = \"graph\"\n\
            address = \"robin@example.com\"\n\n\
            [[account]]\nname = \"Signed\"\ntype = \"imap\"\naddress = \"robin@example.org\"\n\
            auth = \"oauth\"\noauth_client_id = \"an-id\"\noauth_client_secret = \"a-secret\"\n\
            imap = { host = \"imap.gmail.com\" }\n\n\
            [[account]]\nname = \"Split\"\ntype = \"imap\"\naddress = \"robin@example.net\"\n\
            imap = { host = \"127.0.0.1\", port = 1, tls = \"none\" }\n\
            smtp = { host = \"127.0.0.1\", port = 2, tls = \"none\", username = \"relay\" }\n";
        write_config(&rig, written);
        let office = ask("Office").await.unwrap_err();
        assert!(office.contains("not read over IMAP"), "{office}");
        let signed = ask("Signed").await.unwrap_err();
        assert!(signed.contains("signs in with its provider"), "{signed}");
        let split = ask("Split").await.unwrap_err();
        assert!(split.contains("a name of its own"), "{split}");
        let nobody = ask("Nobody").await.unwrap_err();
        assert!(nobody.contains("no account called Nobody"), "{nobody}");

        // And none of them is changed by being asked to be.
        for account in ["Office", "Signed", "Split"] {
            match rig
                .service
                .handle(AccountRequest::Change {
                    account: account.to_owned(),
                    draft: changed_draft(),
                    password: Some(Secret("hunter2".to_owned())),
                    trusting: Trusting::System,
                })
                .await
            {
                AccountEvent::Saved(Err(_)) => {}
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(config_text(&rig), written);
    }

    #[tokio::test]
    async fn a_change_with_no_password_to_try_or_no_file_to_write_changes_nothing() {
        let rig = Rig::new();
        write_config(&rig, CHANGEABLE);
        // Nothing typed and nothing kept: no server is asked anything.
        match rig
            .service
            .handle(AccountRequest::VerifyChange {
                account: "Work".to_owned(),
                draft: changed_draft(),
                password: None,
                trusting: Trusting::System,
            })
            .await
        {
            AccountEvent::Verified(Err(VerifyProblem::Password(reason))) => {
                assert!(reason.contains("no password kept"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        match rig
            .service
            .handle(AccountRequest::Change {
                account: "Work".to_owned(),
                draft: changed_draft(),
                password: None,
                trusting: Trusting::System,
            })
            .await
        {
            AccountEvent::Saved(Err(reason)) => {
                assert!(reason.contains("no password kept"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(config_text(&rig), CHANGEABLE);

        // A file that cannot be written: the Keychain is as it was.
        rig.secrets
            .set("imap:robin@example.com@127.0.0.1", "hunter2")
            .unwrap();
        let folder = rig.service.config_file.parent().unwrap().to_owned();
        let mut closed = std::fs::metadata(&folder).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut closed, 0o500);
        std::fs::set_permissions(&folder, closed.clone()).unwrap();
        let refused = rig
            .service
            .handle(AccountRequest::Change {
                account: "Work".to_owned(),
                draft: changed_draft(),
                password: Some(Secret("correct horse".to_owned())),
                trusting: Trusting::System,
            })
            .await;
        std::os::unix::fs::PermissionsExt::set_mode(&mut closed, 0o700);
        std::fs::set_permissions(&folder, closed).unwrap();
        match refused {
            AccountEvent::Saved(Err(reason)) => {
                assert!(reason.contains("cannot be written"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(config_text(&rig), CHANGEABLE);
        assert_eq!(
            rig.secrets
                .get("imap:robin@example.com@127.0.0.1")
                .unwrap()
                .as_deref(),
            Some("hunter2")
        );
        assert_eq!(rig.secrets.get("imap:robin@localhost").unwrap(), None);
    }

    #[tokio::test]
    async fn an_account_is_not_removed_from_a_file_that_cannot_be_read_or_written() {
        let rig = Rig::new();
        let remove = |account: &str| {
            let service = Arc::clone(&rig.service);
            let account = account.to_owned();
            async move {
                match service.handle(AccountRequest::Remove { account }).await {
                    AccountEvent::Removed { result, .. } => result,
                    other => panic!("{other:?}"),
                }
            }
        };
        // A file with a mistake in it is not edited.
        let mistaken = "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \n";
        write_config(&rig, mistaken);
        assert!(
            remove("Work")
                .await
                .unwrap_err()
                .contains("mistake to fix first")
        );
        assert_eq!(config_text(&rig), mistaken);
        // One that cannot be written is left, and so are the store and
        // the Keychain: the account is as it was.
        let written = "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"robin@example.com\"\n\
            imap = { host = \"mail.example.com\" }\n";
        write_config(&rig, written);
        let config = rig.config();
        rig.service.register(&config.accounts[0]).unwrap();
        rig.secrets
            .set("imap:robin@example.com@mail.example.com", "hunter2")
            .unwrap();
        let folder = rig.service.config_file.parent().unwrap().to_owned();
        let mut closed = std::fs::metadata(&folder).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut closed, 0o500);
        std::fs::set_permissions(&folder, closed.clone()).unwrap();
        let refused = remove("Work").await;
        std::os::unix::fs::PermissionsExt::set_mode(&mut closed, 0o700);
        std::fs::set_permissions(&folder, closed).unwrap();
        assert!(refused.unwrap_err().contains("cannot be written"));
        assert_eq!(config_text(&rig), written);
        assert_eq!(rig.store.accounts().unwrap().len(), 1);
        assert!(
            rig.secrets
                .get("imap:robin@example.com@mail.example.com")
                .unwrap()
                .is_some()
        );
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
                google: None,
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

    /// An account that signs in with Google, whose mail servers are a
    /// port on this Mac where nothing listens: no test reaches Google's.
    fn google(name: &str, address: &str) -> AccountRequest {
        let server = |port| ServerDraft {
            host: "127.0.0.1".to_owned(),
            port,
            tls: TlsMode::None,
            username: address.to_owned(),
        };
        AccountRequest::SaveSignIn {
            draft: SignInDraft {
                name: name.to_owned(),
                address: address.to_owned(),
                application_id: "123-abc.apps.googleusercontent.com".to_owned(),
                tenant: None,
                google: Some(quill_tui::setup::GoogleDraft {
                    client_secret: Some("GOCSPX-not-a-real-one".to_owned()),
                    imap: server(1),
                    smtp: Some(server(2)),
                }),
            },
        }
    }

    #[tokio::test]
    async fn a_google_account_is_written_down_with_its_application_and_offered_again() {
        let rig = Rig::new();
        // With no account that names an application, the form is offered
        // Quill's own for Google, by its id alone and marked as Quill's,
        // in a build that was given one; and none at all in a build from
        // the source alone, where the form then asks for one.
        let look = AccountRequest::Discover {
            address: "sam@example.com".to_owned(),
        };
        let AccountEvent::Discovered { found, .. } = rig.service.handle(look.clone()).await else {
            panic!("not a discovery");
        };
        let quills = oauth::GOOGLE_APPLICATION.map(|(id, _)| id.to_owned());
        assert_eq!(
            found.google_application,
            quills.clone().map(|client_id| Box::new(GoogleApplication {
                client_id,
                client_secret: None,
            }))
        );
        assert_eq!(found.google_own, quills.is_some());

        let saved = rig
            .service
            .handle(google("Personal", "robin@gmail.com"))
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        let config = rig.config();
        let account = &config.accounts[0];
        assert_eq!(account.kind, AccountType::Imap);
        assert!(oauth::wanted(account));
        assert_eq!(
            account.oauth_client_id.as_deref(),
            Some("123-abc.apps.googleusercontent.com")
        );
        assert_eq!(
            account.oauth_client_secret.as_deref(),
            Some("GOCSPX-not-a-real-one")
        );
        // It is in the store, to be listed, and nothing is in the
        // Keychain: nobody has signed in.
        assert_eq!(rig.store.accounts().unwrap().len(), 1);
        assert_eq!(
            rig.secrets.get("oauth:google:robin@gmail.com").unwrap(),
            None
        );
        // The next account is offered the same application, which is the
        // user's own and not Quill's.
        let AccountEvent::Discovered { found, .. } = rig.service.handle(look).await else {
            panic!("not a discovery");
        };
        assert_eq!(
            found.google_application,
            Some(Box::new(GoogleApplication {
                client_id: "123-abc.apps.googleusercontent.com".to_owned(),
                client_secret: Some("GOCSPX-not-a-real-one".to_owned()),
            }))
        );
        assert!(!found.google_own);

        // The same address a second time would be the first one twice.
        match rig.service.handle(google("Other", "Robin@Gmail.com")).await {
            AccountEvent::Saved(Err(reason)) => assert!(
                reason.contains("Personal is already the Google account for"),
                "{reason}"
            ),
            other => panic!("{other:?}"),
        }
        // A Microsoft account at that address is another matter.
        let saved = rig
            .service
            .handle(microsoft("Work", "robin@gmail.com", None))
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Work".to_owned())));
        assert_eq!(rig.config().accounts.len(), 2);

        // In a build that was given Quill's own application, an account
        // that signs in as Quill has neither half of it written down: it
        // is Quill's to know.
        let Some(quills) = quills else {
            return;
        };
        let rig = Rig::new();
        let AccountRequest::SaveSignIn { mut draft } = google("Personal", "robin@gmail.com") else {
            unreachable!()
        };
        draft.application_id = quills.clone();
        if let Some(google) = &mut draft.google {
            google.client_secret = None;
        }
        let saved = rig
            .service
            .handle(AccountRequest::SaveSignIn { draft })
            .await;
        assert_eq!(saved, AccountEvent::Saved(Ok("Personal".to_owned())));
        let text = config_text(&rig);
        assert!(text.contains("auth = \"oauth\""), "{text}");
        assert!(!text.contains("oauth_client"), "{text}");
        // And it is still an account that signs in with Google, as Quill.
        let config = rig.config();
        let setup = oauth::setup(&config.accounts[0]).unwrap().unwrap();
        assert_eq!(setup.provider.name, "google");
        assert_eq!(setup.client.id, quills);
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
                        google: None,
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
        assert!(found.microsoft_own);

        // Only an application registered with Microsoft is offered: not
        // the one a Gmail account signs in to Google as.
        write_config(
            &rig,
            "[[account]]\nname = \"Mail\"\ntype = \"imap\"\naddress = \"robin@example.org\"\nauth = \"oauth\"\noauth_client_id = \"google-app\"\nimap = { host = \"mail.example.org\" }\n",
        );
        let found = discovered(rig.service.handle(look.clone()).await);
        assert_eq!(found.microsoft_application.as_deref(), quills);
        assert!(found.microsoft_own);

        // A Microsoft account's is, trimmed; one with none is passed over.
        write_config(
            &rig,
            "[[account]]\nname = \"Old\"\ntype = \"graph\"\naddress = \"old@example.com\"\n\n\
             [[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \" app-1 \"\n",
        );
        let found = discovered(rig.service.handle(look.clone()).await);
        assert_eq!(found.microsoft_application.as_deref(), Some("app-1"));
        // That one is the account's own, and is shown to be changed;
        // Quill's own is not.
        assert!(!found.microsoft_own);
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

    #[tokio::test]
    async fn a_choice_made_in_the_ui_is_written_into_the_config_and_the_rest_is_kept() {
        let rig = Rig::new();
        let before = "# My settings\n[ui]\ntheme = \"light\"   # easier on the eyes\n\n\
                      [[account]]\nname = \"Personal\"   # home\ntype = \"imap\"\naddress = \"robin@example.org\"\n\
                      imap = { host = \"mail.example.org\" }\n";
        write_config(&rig, before);
        let remember = |on| AccountRequest::Remember(Preference::WebReading(on));
        assert_eq!(
            rig.service.handle(remember(true)).await,
            AccountEvent::Remembered(Ok(()))
        );
        assert_eq!(
            config_text(&rig),
            before.replace(
                "# easier on the eyes\n",
                "# easier on the eyes\nweb_reading = true\n"
            )
        );
        assert!(rig.config().ui.web_reading);
        assert_eq!(
            rig.service.handle(remember(false)).await,
            AccountEvent::Remembered(Ok(()))
        );
        assert!(!rig.config().ui.web_reading);
        assert_eq!(rig.config().accounts.len(), 1);

        // Hiding the accounts pane is kept the same way, on a line of
        // its own, and showing it again changes that line.
        assert!(rig.config().ui.sidebar);
        for shown in [false, true] {
            assert_eq!(
                rig.service
                    .handle(AccountRequest::Remember(Preference::Sidebar(shown)))
                    .await,
                AccountEvent::Remembered(Ok(()))
            );
            assert_eq!(rig.config().ui.sidebar, shown);
            assert!(config_text(&rig).contains(&format!("\nsidebar = {shown}\n")));
        }
        assert!(config_text(&rig).contains("theme = \"light\"   # easier on the eyes"));

        // And that an editor's keys have been explained, under the
        // section it belongs to.
        assert!(rig.config().compose.editor_tips);
        assert_eq!(
            rig.service
                .handle(AccountRequest::Remember(Preference::EditorTips(false)))
                .await,
            AccountEvent::Remembered(Ok(()))
        );
        assert!(!rig.config().compose.editor_tips);
        assert!(config_text(&rig).contains("[compose]\neditor_tips = false\n"));

        // And where the edges between the panes were left.
        for (preference, line) in [
            (Preference::SidebarWidth(Some(34)), "\nsidebar_width = 34\n"),
            (
                Preference::ReaderWidth(Some(538)),
                "\nreader_width = 0.538\n",
            ),
        ] {
            assert_eq!(
                rig.service
                    .handle(AccountRequest::Remember(preference))
                    .await,
                AccountEvent::Remembered(Ok(()))
            );
            assert!(config_text(&rig).contains(line), "{}", config_text(&rig));
        }
        assert_eq!(rig.config().ui.sidebar_width, Some(34));
        assert_eq!(rig.config().ui.reader_width, 0.538);
        // Put back, each line is taken out and nothing else is.
        let with_both = config_text(&rig);
        for preference in [
            Preference::SidebarWidth(None),
            Preference::ReaderWidth(None),
        ] {
            assert_eq!(
                rig.service
                    .handle(AccountRequest::Remember(preference))
                    .await,
                AccountEvent::Remembered(Ok(()))
            );
        }
        assert_eq!(
            config_text(&rig),
            with_both
                .replace("sidebar_width = 34\n", "")
                .replace("reader_width = 0.538\n", "")
        );
        assert_eq!(rig.config().ui.sidebar_width, None);
        assert_eq!(rig.config().ui.reader_width, 0.5);

        // A config Quill cannot read is left exactly as it is, and the
        // reason comes back.
        write_config(&rig, "[ui\ntheme = ");
        assert!(matches!(
            rig.service.handle(remember(true)).await,
            AccountEvent::Remembered(Err(reason)) if reason.contains("config.toml cannot be read")
        ));
        assert_eq!(config_text(&rig), "[ui\ntheme = ");
    }

    #[tokio::test]
    async fn the_demo_mailbox_remembers_a_choice_too_and_answers_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        // No config yet, and no folder for one.
        let file = dir.path().join("config").join("config.toml");
        let AccountsLink {
            requests,
            mut events,
        } = spawn_remembering(file.clone());
        let requests = requests.unwrap();
        // Not this service's to do: nothing comes of it.
        requests
            .send(AccountRequest::Discover {
                address: "robin@example.com".to_owned(),
            })
            .unwrap();
        requests
            .send(AccountRequest::Remember(Preference::WebReading(true)))
            .unwrap();
        assert_eq!(next(&mut events).await, AccountEvent::Remembered(Ok(())));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[ui]\nweb_reading = true\n"
        );
        requests
            .send(AccountRequest::Remember(Preference::WebReading(false)))
            .unwrap();
        assert_eq!(next(&mut events).await, AccountEvent::Remembered(Ok(())));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[ui]\nweb_reading = false\n"
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
