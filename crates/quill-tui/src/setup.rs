//! Adding an account from inside Quill, and deciding about a server
//! certificate the system does not trust.
//!
//! The flow: type an address, let Quill find the server, confirm or correct
//! what it found, give the password, and sign in. Only when sign-in has
//! worked is anything saved.
//!
//! A Microsoft mailbox takes no password. Its account is saved with the id
//! of the application it signs in as, and the form hands over to the
//! sign-in that goes on in the browser.
//!
//! Nothing here touches the network, the Keychain or a file. Each step asks
//! for work with an [`AccountRequest`] and hears the answer as an
//! [`AccountEvent`]; whoever runs the UI supplies the service that does the
//! work.

use quill_core::CertificateProblem;
use quill_core::config::TlsMode;

use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Mods};
use crate::text;

mod sheet;

pub use sheet::{Press, Used};
pub(crate) use sheet::{certificate_asked, headline, put, shown, typed};

/// What the form says at its first step.
pub const ADDRESS_NOTE: &str =
    "Type the email address of the account. Quill will look for its mail server.";

/// What the form says of a sending server left empty.
pub const NO_SENDING: &str = "Empty: the account will read mail and not send it.";

/// What the form says while a server is to be reached with no encryption.
pub const NO_ENCRYPTION: &str =
    "With no encryption, your password and mail cross the network readable by anyone on the way.";

/// What the form says of a Microsoft account that signs in as Quill
/// itself: there is nothing to fill in but its name.
pub const MICROSOFT_OWN_NOTE: &str =
    "Continue, and Microsoft's sign-in page opens in your browser.";

/// What the form says under the fields of a Microsoft account that names
/// an application of its own.
pub const MICROSOFT_NOTE: &str = "Register Quill with Microsoft once to get the application id: see docs/oauth.md. Tenant may be left empty.";

/// What the form says of a Google account that signs in as Quill itself.
pub const GOOGLE_OWN_NOTE: &str = "Continue, and Google's sign-in page opens in your browser.";

/// What the form says under the fields of a Google account that names an
/// application of its own.
pub const GOOGLE_NOTE: &str = "Register Quill with Google once to get these: a Desktop app OAuth client, in the Google Cloud console. See docs/oauth.md.";

/// What is said of a certificate that could not even be read.
pub const UNREADABLE_CERTIFICATE: &str =
    "The certificate could not be read, so it cannot be accepted.";

/// A kind of connection as the form names it.
pub fn security_label(tls: TlsMode) -> &'static str {
    match tls {
        TlsMode::Implicit => "TLS",
        TlsMode::Starttls => "STARTTLS",
        TlsMode::None => "None",
    }
}

/// What is said of a certificate the server at `host` presented.
pub fn certificate_note(host: &str) -> String {
    format!("{host} presented a certificate that was not accepted. Nothing has been sent to it.")
}

/// What is said of a certificate of the server an existing account sends
/// mail through, before the rest.
pub const SENDING_CERTIFICATE: &str =
    "This is the server the account sends mail through. A message is waiting in the Outbox for it.";

/// What is said while an answer about a certificate is being acted on.
pub const CERTIFICATE_WORKING: &str = "Working…";

/// What a sign-in says before the service has said how it is to be done.
pub const SIGN_IN_STARTING: &str = "Getting ready to sign in...";

/// What a sign-in that goes on in the browser says: where it has gone,
/// what to do there, and where the password does and does not go.
pub const SIGN_IN_BROWSER: [&str; 3] = [
    "Your browser has been opened at the provider's sign-in page.",
    "Sign in there. When the page says you can close it, come back: Quill is waiting, and takes it from there.",
    "Your password goes to the provider, never to Quill. Quill keeps only the sign-in the provider hands back, in the Keychain.",
];

/// What a sign-in with a code says above the page and the code, and under
/// them.
pub const SIGN_IN_CODE: &str = "On any device, open this page and enter the code:";
pub const SIGN_IN_WAITING: &str = "Quill is waiting, and takes it from there.";

/// How long a sign-in's code is good for, as it is said.
pub fn sign_in_code_lasts(minutes: u64) -> String {
    format!("The code is good for {minutes} minutes.")
}

/// Text that must not be shown or logged. `Debug` prints a placeholder.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// Where one of an account's servers is and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDraft {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
}

/// An account as far as the setup flow has got with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDraft {
    pub name: String,
    pub address: String,
    pub imap: ServerDraft,
    /// The server mail is sent through, when discovery found one.
    pub smtp: Option<ServerDraft>,
}

/// An account that signs in with its provider (Microsoft, or Google), as
/// far as the setup flow has got with it. It has no password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInDraft {
    pub name: String,
    pub address: String,
    /// The id of the application registered for Quill with the provider.
    pub application_id: String,
    /// Microsoft: the one organisation to sign in to, when the application
    /// was registered for one. `None`: any, which leaves `oauth_tenant`
    /// out.
    pub tenant: Option<String>,
    /// What a Google account has besides: `None` for a Microsoft one,
    /// which has no mail servers to name.
    pub google: Option<GoogleDraft>,
}

/// What an account that signs in with Google has that a Microsoft one has
/// not: its mail is read and sent through mail servers, with the sign-in
/// in place of a password, and Google asks a desktop application for the
/// "client secret" it was given with its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleDraft {
    /// The application's client secret. Not a secret in any real sense:
    /// every copy of a desktop application carries it, and Google says so.
    /// `None` where the application is Quill's own, which has its own.
    pub client_secret: Option<String>,
    pub imap: ServerDraft,
    pub smtp: Option<ServerDraft>,
}

/// The application an account signs in to Google as, offered to the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleApplication {
    pub client_id: String,
    pub client_secret: Option<String>,
}

/// The kinds of account the form can add.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Kind {
    /// A mail server that takes a password.
    #[default]
    Imap,
    /// A Microsoft mailbox: it signs in with Microsoft, in the browser.
    Microsoft,
    /// A Gmail or Google Workspace mailbox: it signs in with Google, in
    /// the browser, and its mail is then read and sent as any other's is.
    Google,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Imap, Kind::Microsoft, Kind::Google];

    /// Whether an account of this kind signs in with its provider, in the
    /// browser, and is asked for no password.
    pub fn signs_in(self) -> bool {
        matches!(self, Kind::Microsoft | Kind::Google)
    }

    /// The kind as the form names it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Imap => "IMAP (password)",
            Kind::Microsoft => "Microsoft (sign in)",
            Kind::Google => "Google (sign in)",
        }
    }
}

/// Which certificates to accept from the account's server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Trusting {
    /// Only what the system trusts.
    #[default]
    System,
    /// This certificate (its SHA-256, in hex) until Quill quits.
    Once(String),
    /// This certificate from now on, written into the config.
    Pinned(String),
}

/// Work the setup flow asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRequest {
    /// Find the servers for an address.
    Discover { address: String },
    /// Sign in, to check the settings and the password. Saves nothing.
    Verify {
        draft: AccountDraft,
        password: Secret,
        trusting: Trusting,
    },
    /// Write the account to the config, its password to the Keychain, and
    /// start syncing it.
    Save {
        draft: AccountDraft,
        password: Secret,
        trusting: Trusting,
    },
    /// Say how the account called this is set up, so that it can be
    /// changed. Changes nothing. Answered with
    /// [`AccountEvent::EditAsked`].
    EditAsk { account: String },
    /// As [`AccountRequest::Verify`], for an account that is there
    /// already and is being changed. With no password, the one Quill has
    /// for it is what is tried.
    VerifyChange {
        account: String,
        draft: AccountDraft,
        password: Option<Secret>,
        trusting: Trusting,
    },
    /// Write the account called this to the config as the draft has it,
    /// keep its password under the name its new settings give it, and
    /// sync it afresh. With no password, the one Quill has is kept.
    /// Answered with [`AccountEvent::Saved`].
    Change {
        account: String,
        draft: AccountDraft,
        password: Option<Secret>,
        trusting: Trusting,
    },
    /// Write an account that signs in with Microsoft to the config and
    /// start syncing it. Nothing is asked of Microsoft and nothing goes in
    /// the Keychain: signing in is the next thing, and is asked for
    /// separately. Answered with [`AccountEvent::Saved`].
    SaveSignIn { draft: SignInDraft },
    /// Accept an existing account's certificate, once or from now on.
    Trust {
        account: String,
        fingerprint: String,
        pin: bool,
        /// The certificate is the sending server's.
        sending: bool,
    },
    /// Sign an account in to its provider (Microsoft, Google). Answered
    /// first with how ([`AccountEvent::SignInBrowser`] or
    /// [`AccountEvent::SignInCode`]) and in the end with
    /// [`AccountEvent::SignedIn`].
    ///
    /// `account` is the one the user is looking at, if any. The service
    /// signs in that one if it is an account that signs in this way, and
    /// otherwise the first that needs it.
    SignIn { account: Option<String> },
    /// Give up on the sign-in in progress.
    CancelSignIn,
    /// Say what removing the account called this would lose, so that it
    /// can be asked about. Changes nothing. Answered with
    /// [`AccountEvent::RemoveAsked`].
    RemoveAsk { account: String },
    /// Remove the account called this: out of the config, out of the
    /// Keychain, and its mail out of the store, with its syncing stopped.
    /// The user has just agreed to it. Answered with
    /// [`AccountEvent::Removed`].
    Remove { account: String },
    /// Tell a mailing list's server to unsubscribe the user: one POST of
    /// `List-Unsubscribe=One-Click` to this `https` address (RFC 8058),
    /// which the user has just agreed to. Answered with
    /// [`AccountEvent::Unsubscribed`].
    Unsubscribe { url: String },
    /// Write a choice the user made in the UI into the config, so that it
    /// is how Quill starts next time. Answered with
    /// [`AccountEvent::Remembered`].
    Remember(Preference),
    /// Read the settings from the config for the host's settings window,
    /// having first made this change to it, if there is one. The file is
    /// read afresh every time, so a change is always made to the file as
    /// it stands, by whoever last wrote it. Answered with
    /// [`AccountEvent::Settings`].
    Settings(Option<crate::settings::Change>),
}

/// A choice made in the UI that is kept between runs, in `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// `web_reading` under `[ui]`.
    WebReading(bool),
    /// `sidebar` under `[ui]`: whether the accounts pane is on show.
    Sidebar(bool),
    /// `editor_tips` under `[compose]`: whether what an editor's keys do
    /// is said above it.
    EditorTips(bool),
}

/// What goes with an account that is removed, beyond the copy of its mail:
/// what exists nowhere but on this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Leaving {
    /// The account's address.
    pub address: String,
    /// The account's mail is taken off its server as it is fetched
    /// (POP3), so the copy here may be the only one.
    pub only_copy: bool,
    /// Messages being written and not sent.
    pub drafts: usize,
    /// Messages waiting to be sent, or refused by a server.
    pub outbox: usize,
    /// Changes to mail that have not reached the server yet.
    pub queued: usize,
    /// Messages in the account's folders on this Mac, which no server
    /// has.
    pub local: usize,
}

/// The answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountEvent {
    Discovered {
        address: String,
        found: Discovery,
    },
    Verified(Result<(), VerifyProblem>),
    /// The saved account's name, or why it could not be saved.
    Saved(Result<String, String>),
    Trusted {
        account: String,
        /// Which of the account's servers the answer was about.
        sending: bool,
        result: Result<(), String>,
    },
    /// The sign-in goes on in the browser, at this address.
    SignInBrowser {
        account: String,
        url: String,
    },
    /// The sign-in goes on on any device: at `address`, with `code`.
    SignInCode {
        account: String,
        address: String,
        code: String,
        /// How long the code is good for.
        minutes: u64,
    },
    /// The sign-in ended: kept, or why not.
    SignedIn {
        account: String,
        result: Result<(), String>,
    },
    /// The list's server at `host` took the request to unsubscribe, or why
    /// the request did not get that far.
    Unsubscribed {
        host: String,
        result: Result<(), String>,
    },
    /// What removing the account would lose, or why it cannot be removed
    /// from here at all.
    RemoveAsked {
        account: String,
        found: Result<Leaving, String>,
    },
    /// How the account is set up now, to be changed, or why it cannot be
    /// changed from here.
    EditAsked {
        account: String,
        found: Result<Box<AccountDraft>, String>,
    },
    /// The account is removed, or why not: then nothing was changed. With
    /// it removed, anything that could not be cleared up after it, such as
    /// a password still in the Keychain.
    Removed {
        account: String,
        result: Result<Option<String>, String>,
    },
    /// The choice is in the config, or why it could not be written there.
    /// It holds for this run either way.
    Remembered(Result<(), String>),
    /// What the config says now, for the host's settings window: or, when
    /// it cannot be read for what it says, why not. `changed` is the
    /// change that was asked for with it, and whether it was made; a
    /// change that was not made has left the file as it was.
    Settings {
        read: Result<Box<crate::settings::SettingsRead>, String>,
        changed: Option<(crate::settings::Change, Result<(), String>)>,
    },
}

/// A sign-in to an account's provider that is under way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    /// Which account, once the service has said. Empty until then.
    pub account: String,
    pub stage: SignInStage,
    /// The account was added by the form a moment ago. Nobody can have
    /// signed in to it yet, and it need not be heard saying so.
    pub fresh: bool,
}

impl SignIn {
    /// What the sign-in is called: whose it is, once that is known.
    pub fn title(&self) -> String {
        if self.account.is_empty() {
            "Sign in".to_owned()
        } else {
            format!("Sign in to {}", self.account)
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SignInStage {
    /// Asked for; nothing heard yet.
    Starting,
    /// The browser has been sent to the provider.
    Browser,
    /// A code to type in at the provider's page, on any device.
    Code {
        address: String,
        code: String,
        minutes: u64,
    },
}

/// The page and the code are for the screen, and for nothing that prints
/// the state.
impl std::fmt::Debug for SignInStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => f.write_str("Starting"),
            Self::Browser => f.write_str("Browser"),
            Self::Code { minutes, .. } => f
                .debug_struct("Code")
                .field("minutes", minutes)
                .finish_non_exhaustive(),
        }
    }
}

/// What discovery came back with. There is always a draft: when nothing was
/// found it holds the usual guesses for the user to correct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub draft: AccountDraft,
    /// Whether a server was actually found, as opposed to guessed.
    pub found: bool,
    /// Something the user should know: where the settings came from, or
    /// that this provider needs another kind of sign-in.
    pub note: Option<String>,
    /// The application id an account in the config already signs in to
    /// Microsoft with, to offer again. The form reads no config itself.
    pub microsoft_application: Option<String>,
    /// That id is Quill's own, the one every Microsoft account uses unless
    /// its config names another. The form then has no need to show it.
    pub microsoft_own: bool,
    /// The application an account signs in to Google as, to offer: the one
    /// an account in the config already uses, or Quill's own if it has one.
    /// Boxed: it is seldom there, and an answer is passed about by value.
    pub google_application: Option<Box<GoogleApplication>>,
    /// That application is Quill's own, and the form need not show it.
    pub google_own: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyProblem {
    /// The server refused the name and password.
    Auth(String),
    /// There is no password to try: none was typed for an account being
    /// changed, and Quill has none kept for it. Nothing was asked of a
    /// server.
    Password(String),
    /// The server's certificate was not accepted.
    Certificate(Box<CertificateProblem>),
    /// Reading works; the server mail is sent through does not, for this
    /// reason.
    Sending(String),
    /// Anything else: unreachable, no STARTTLS, a protocol error.
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Address,
    Server,
    Password,
    Certificate,
}

/// The kinds of connection the form offers, in order.
pub const SECURITIES: [TlsMode; 3] = [TlsMode::Implicit, TlsMode::Starttls, TlsMode::None];

/// The fields of the server step, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerField {
    /// What kind of account this is. The fields after the name depend on it.
    Type,
    Name,
    Host,
    Port,
    Security,
    Username,
    /// The server mail is sent through. Empty: the account only reads.
    SmtpHost,
    SmtpPort,
    SmtpSecurity,
    /// For a Microsoft account: the id of the application it signs in as.
    Application,
    /// For a Microsoft account: its organisation. Empty for any.
    Tenant,
    /// For a Google account: the client id and the client secret of the
    /// application it signs in as.
    GoogleClient,
    GoogleSecret,
}

impl ServerField {
    pub const ALL: [ServerField; 13] = [
        ServerField::Type,
        ServerField::Name,
        ServerField::Host,
        ServerField::Port,
        ServerField::Security,
        ServerField::Username,
        ServerField::SmtpHost,
        ServerField::SmtpPort,
        ServerField::SmtpSecurity,
        ServerField::Application,
        ServerField::Tenant,
        ServerField::GoogleClient,
        ServerField::GoogleSecret,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0)
    }
}

/// What became of the flow after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Open,
    /// The account was saved under this name; the flow is over.
    Added(String),
    /// The changes to the account called this were saved; the flow is
    /// over.
    Changed(String),
    /// The account was saved under this name and is being signed in to
    /// with its provider: the form is over and the sign-in takes its place.
    SigningIn(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub step: Step,
    pub address: LineInput,
    /// What kind of account is being added. Chosen for the user from the
    /// server that was found, and theirs to change.
    pub kind: Kind,
    pub name: LineInput,
    pub host: LineInput,
    pub port: LineInput,
    pub tls: TlsMode,
    pub username: LineInput,
    pub smtp_host: LineInput,
    pub smtp_port: LineInput,
    pub smtp_tls: TlsMode,
    pub password: LineInput,
    /// For a Microsoft account: the application id, and the organisation.
    pub application: LineInput,
    pub tenant: LineInput,
    /// The application id is Quill's own, offered by the service and not
    /// typed. Its field and the tenant's are then not shown: an account
    /// that wants another application names it in the config.
    pub own_application: bool,
    /// For a Google account: the application's client id and secret, and
    /// whether they are Quill's own and so not shown.
    pub google_client: LineInput,
    pub google_secret: LineInput,
    pub own_google: bool,
    /// Which field of the server step has the cursor.
    pub field: usize,
    /// What is being waited for, in words. Keys other than `esc` wait too.
    pub busy: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
    /// The certificate being asked about.
    pub problem: Option<Box<CertificateProblem>>,
    /// The form is changing an account that is there already: this one,
    /// by its name. Its name, its address and what kind of account it is
    /// stay as they are, so the form starts at its servers.
    pub editing: Option<String>,
    trusting: Trusting,
}

/// What the form says where the password of an account being changed is
/// asked for.
pub const KEEP_PASSWORD: &str = "Leave it empty to keep the password Quill has.";

impl Default for Setup {
    fn default() -> Self {
        Self {
            step: Step::Address,
            address: LineInput::default(),
            kind: Kind::Imap,
            name: LineInput::default(),
            host: LineInput::default(),
            port: LineInput::default(),
            tls: TlsMode::Implicit,
            username: LineInput::default(),
            smtp_host: LineInput::default(),
            smtp_port: LineInput::new("465".to_owned()),
            smtp_tls: TlsMode::Implicit,
            password: LineInput::masked(),
            application: LineInput::default(),
            tenant: LineInput::default(),
            own_application: false,
            google_client: LineInput::default(),
            google_secret: LineInput::default(),
            own_google: false,
            field: ServerField::Name.index(),
            busy: None,
            error: None,
            note: None,
            problem: None,
            editing: None,
            trusting: Trusting::System,
        }
    }
}

impl Setup {
    /// The form for changing the account called `account`, with its
    /// settings as `draft` has them: at its servers, since that is all
    /// there is to change.
    pub fn editing(account: String, draft: AccountDraft) -> Self {
        let mut setup = Self {
            step: Step::Server,
            kind: Kind::Imap,
            field: ServerField::Host.index(),
            editing: Some(account),
            ..Self::default()
        };
        setup.address.set(draft.address);
        setup.name.set(draft.name);
        setup.host.set(draft.imap.host);
        setup.port.set(draft.imap.port.to_string());
        setup.tls = draft.imap.tls;
        setup.username.set(draft.imap.username);
        if let Some(smtp) = draft.smtp {
            setup.smtp_host.set(smtp.host);
            setup.smtp_port.set(smtp.port.to_string());
            setup.smtp_tls = smtp.tls;
        }
        setup
    }

    /// Which account is being changed, in words, for the top of the form.
    pub fn editing_note(&self) -> Option<String> {
        let account = self.editing.as_deref()?;
        Some(format!(
            "{account}, {}. Its name and address stay as they are.",
            self.address.text()
        ))
    }

    /// What the form is called at the step it is at.
    pub fn title(&self) -> &'static str {
        if self.editing.is_some() {
            return match self.step {
                Step::Address | Step::Server => "Edit an account: the server",
                Step::Password => "Edit an account: sign in",
                Step::Certificate => "Edit an account: certificate",
            };
        }
        match self.step {
            Step::Address => "Add an account",
            Step::Server if self.kind == Kind::Microsoft => "Add an account: Microsoft",
            Step::Server if self.kind == Kind::Google => "Add an account: Google",
            Step::Server => "Add an account: the server",
            Step::Password => "Add an account: sign in",
            Step::Certificate => "Add an account: certificate",
        }
    }

    /// What the form says where the password is asked for.
    pub fn password_note(&self) -> String {
        let asked = format!(
            "The password for {} at {}. It is kept in your Keychain and nowhere else.",
            self.username.text(),
            self.host.text()
        );
        // An account that is there already has a password already.
        let asked = match self.editing {
            Some(_) => format!("{asked} {KEEP_PASSWORD}"),
            None => asked,
        };
        // A provider that does not take the user's normal password is
        // said so here, where it is about to be typed.
        match quill_core::providers::password_advice(self.host.text()) {
            Some(advice) => format!("{asked} {advice}"),
            None => asked,
        }
    }

    /// Whether one of the servers named is to be reached with no
    /// encryption.
    pub fn unencrypted(&self) -> bool {
        self.tls == TlsMode::None || (self.smtp_tls == TlsMode::None && !self.smtp_host.is_empty())
    }

    /// Makes the account of another kind. What was said about the old
    /// kind is taken back.
    fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.error = None;
    }

    /// Changes the kind of connection to the reading server, or to the
    /// `sending` one.
    fn set_security(&mut self, sending: bool, mode: TlsMode) {
        let (tls, port) = if sending {
            (&mut self.smtp_tls, &mut self.smtp_port)
        } else {
            (&mut self.tls, &mut self.port)
        };
        // The port follows the kind of connection while it is still the
        // usual one for the old kind.
        let usual = |mode| match (sending, mode) {
            (false, TlsMode::Implicit) => "993",
            (false, TlsMode::Starttls | TlsMode::None) => "143",
            (true, TlsMode::Implicit) => "465",
            (true, TlsMode::Starttls | TlsMode::None) => "587",
        };
        if port.text() == usual(*tls) {
            port.set(usual(mode));
        }
        *tls = mode;
        self.error = None;
    }

    pub fn server_field(&self) -> ServerField {
        ServerField::ALL[self.field.min(ServerField::ALL.len() - 1)]
    }

    /// Whether the server step shows `field` for the kind of account
    /// chosen. A field that is not shown is not stopped at.
    pub fn shows(&self, field: ServerField) -> bool {
        match field {
            // An account that is there keeps its kind and its name.
            ServerField::Type | ServerField::Name => self.editing.is_none(),
            ServerField::Host
            | ServerField::Port
            | ServerField::Security
            | ServerField::Username
            | ServerField::SmtpHost => self.kind == Kind::Imap,
            // With no sending server named, its port and security are not
            // shown either.
            ServerField::SmtpPort | ServerField::SmtpSecurity => {
                self.kind == Kind::Imap && !self.smtp_host.is_empty()
            }
            ServerField::Application | ServerField::Tenant => {
                self.kind == Kind::Microsoft && !self.own_application
            }
            ServerField::GoogleClient | ServerField::GoogleSecret => {
                self.kind == Kind::Google && !self.own_google
            }
        }
    }

    /// The text field the cursor is in, if it is in one.
    pub fn focused_input(&self) -> Option<&LineInput> {
        if self.busy.is_some() {
            return None;
        }
        match self.step {
            Step::Address => Some(&self.address),
            Step::Password => Some(&self.password),
            Step::Certificate => None,
            Step::Server => match self.server_field() {
                ServerField::Name => Some(&self.name),
                ServerField::Host => Some(&self.host),
                ServerField::Port => Some(&self.port),
                ServerField::Username => Some(&self.username),
                ServerField::SmtpHost => Some(&self.smtp_host),
                ServerField::SmtpPort => Some(&self.smtp_port),
                ServerField::Application => Some(&self.application),
                ServerField::Tenant => Some(&self.tenant),
                ServerField::GoogleClient => Some(&self.google_client),
                ServerField::GoogleSecret => Some(&self.google_secret),
                ServerField::Type | ServerField::Security | ServerField::SmtpSecurity => None,
            },
        }
    }

    fn focused_input_mut(&mut self) -> Option<&mut LineInput> {
        if self.busy.is_some() {
            return None;
        }
        match self.step {
            Step::Address => Some(&mut self.address),
            Step::Password => Some(&mut self.password),
            Step::Certificate => None,
            Step::Server => match self.server_field() {
                ServerField::Name => Some(&mut self.name),
                ServerField::Host => Some(&mut self.host),
                ServerField::Port => Some(&mut self.port),
                ServerField::Username => Some(&mut self.username),
                ServerField::SmtpHost => Some(&mut self.smtp_host),
                ServerField::SmtpPort => Some(&mut self.smtp_port),
                ServerField::Application => Some(&mut self.application),
                ServerField::Tenant => Some(&mut self.tenant),
                ServerField::GoogleClient => Some(&mut self.google_client),
                ServerField::GoogleSecret => Some(&mut self.google_secret),
                ServerField::Type | ServerField::Security | ServerField::SmtpSecurity => None,
            },
        }
    }

    /// Pasted text goes into the field with the cursor.
    pub fn paste(&mut self, text: &str) {
        if let Some(input) = self.focused_input_mut() {
            input.insert(text);
            self.error = None;
        }
    }

    /// Takes a key as typing or as a choice. Returns the work it calls for,
    /// or `None` when the key is not the flow's and should go to a command.
    pub fn key(&mut self, chord: KeyChord) -> Option<Vec<AccountRequest>> {
        if self.busy.is_some() {
            // Typing during a wait would change what the answer is for.
            return matches!(chord.key, Key::Char(_)).then(Vec::new);
        }
        let plain = chord.mods == Mods::default();
        if self.step == Step::Certificate {
            return match chord.key {
                Key::Char('r') if plain => {
                    self.reject_certificate();
                    Some(Vec::new())
                }
                Key::Char('o') if plain => Some(self.accept_certificate(false)),
                Key::Char('p') if plain => Some(self.accept_certificate(true)),
                Key::Char(_) => Some(Vec::new()),
                _ => None,
            };
        }
        if self.step == Step::Server && self.server_field() == ServerField::Type {
            let order = Kind::ALL;
            let at = order
                .iter()
                .position(|kind| *kind == self.kind)
                .unwrap_or(0);
            let next = match chord.key {
                Key::Right | Key::Char(' ') if plain => (at + 1) % order.len(),
                Key::Left if plain => (at + order.len() - 1) % order.len(),
                Key::Char(_) => return Some(Vec::new()),
                _ => return None,
            };
            self.set_kind(order[next]);
            return Some(Vec::new());
        }
        let choosing = match self.server_field() {
            ServerField::Security => Some(false),
            ServerField::SmtpSecurity => Some(true),
            _ => None,
        };
        if self.step == Step::Server
            && let Some(sending) = choosing
        {
            let order = SECURITIES;
            let tls = if sending { self.smtp_tls } else { self.tls };
            let at = order.iter().position(|mode| *mode == tls).unwrap_or(0);
            let next = match chord.key {
                Key::Right | Key::Char(' ') if plain => (at + 1) % order.len(),
                Key::Left if plain => (at + order.len() - 1) % order.len(),
                Key::Char(_) => return Some(Vec::new()),
                _ => return None,
            };
            self.set_security(sending, order[next]);
            return Some(Vec::new());
        }
        let input = self.focused_input_mut()?;
        if input.key(chord) {
            self.error = None;
            Some(Vec::new())
        } else {
            None
        }
    }

    /// `tab` and `shift-tab`: the fields of the server step go round.
    pub fn move_field(&mut self, forward: bool) {
        if self.step == Step::Server && self.busy.is_none() {
            let count = ServerField::ALL.len();
            loop {
                self.field = if forward {
                    (self.field + 1) % count
                } else {
                    (self.field + count - 1) % count
                };
                // The server's name is always shown, so this ends.
                if self.shows(self.server_field()) {
                    break;
                }
            }
        }
    }

    /// `enter`: go on to the next step, asking for whatever it needs.
    pub fn advance(&mut self) -> Vec<AccountRequest> {
        if self.busy.is_some() {
            return Vec::new();
        }
        self.error = None;
        match self.step {
            Step::Address => {
                let address = self.address.text().trim().to_owned();
                let valid = address.split_once('@').is_some_and(|(user, domain)| {
                    !user.is_empty() && domain.contains('.') && !domain.ends_with('.')
                }) && !address.contains(char::is_whitespace);
                if !valid {
                    self.error = Some("That does not look like an email address.".to_owned());
                    return Vec::new();
                }
                self.address.set(address.clone());
                let domain = address.rsplit('@').next().unwrap_or_default();
                self.busy = Some(format!("Looking for the mail server of {domain}…"));
                vec![AccountRequest::Discover { address }]
            }
            Step::Server if self.kind.signs_in() => match self.sign_in_draft() {
                // No password to ask for and nothing to check with a
                // server: the account is saved, and then signed in to.
                Ok(draft) => {
                    self.busy = Some(format!("Saving {}…", draft.name));
                    vec![AccountRequest::SaveSignIn { draft }]
                }
                Err((field, problem)) => {
                    self.field = field.index();
                    self.error = Some(problem);
                    Vec::new()
                }
            },
            Step::Server => {
                if let Err(problem) = self.draft() {
                    self.error = Some(problem);
                    return Vec::new();
                }
                // A server that takes no password from a mail program is
                // not asked for one: the answer is known, and it is no.
                // The cursor goes to the choice that adds the account.
                if let Some(whose) = quill_core::providers::takes_no_password(self.host.text()) {
                    self.error = Some(no_password(whose));
                    self.field = ServerField::Type.index();
                    return Vec::new();
                }
                self.step = Step::Password;
                Vec::new()
            }
            Step::Password => {
                // An account being changed may keep the password it has.
                if self.password.is_empty() && self.editing.is_none() {
                    self.error = Some("Enter the account's password.".to_owned());
                    return Vec::new();
                }
                self.verify()
            }
            Step::Certificate => Vec::new(),
        }
    }

    /// `esc`: back one step. Returns `true` when there is no step to go
    /// back to and the flow should close.
    pub fn back(&mut self) -> bool {
        self.error = None;
        if self.busy.take().is_some() {
            // Whatever answer arrives now is for a question no longer asked.
            return false;
        }
        match self.step {
            Step::Address => return true,
            // An account being changed has no address to go back to.
            Step::Server if self.editing.is_some() => return true,
            Step::Server => self.step = Step::Address,
            Step::Password => self.step = Step::Server,
            Step::Certificate => self.reject_certificate(),
        }
        false
    }

    /// The account as the fields describe it, or what is wrong with them.
    fn draft(&self) -> Result<AccountDraft, String> {
        let host = self.host.text().trim();
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err("Enter the mail server's name, such as imap.example.com.".to_owned());
        }
        let port: u16 = self
            .port
            .text()
            .trim()
            .parse()
            .ok()
            .filter(|port| *port != 0)
            .ok_or("The port is a number from 1 to 65535.")?;
        let address = self.address.text().trim().to_owned();
        let name = match self.name.text().trim() {
            "" => address.clone(),
            name => name.to_owned(),
        };
        let username = match self.username.text().trim() {
            "" => address.clone(),
            username => username.to_owned(),
        };
        // No sending server named: the account reads mail and no more.
        let smtp = match self.smtp_host.text().trim() {
            "" => None,
            host if host.contains(char::is_whitespace) => {
                return Err(
                    "Enter the name of the server mail is sent through, such as smtp.example.com, or leave it empty."
                        .to_owned(),
                );
            }
            host => Some(ServerDraft {
                host: host.to_owned(),
                port: self
                    .smtp_port
                    .text()
                    .trim()
                    .parse()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or("The sending server's port is a number from 1 to 65535.")?,
                tls: self.smtp_tls,
                username: username.clone(),
            }),
        };
        Ok(AccountDraft {
            name,
            imap: ServerDraft {
                host: host.to_owned(),
                port,
                tls: self.tls,
                username: username.clone(),
            },
            smtp,
            address,
        })
    }

    /// The account that signs in with Google as the fields describe it,
    /// or which field is wrong and how. Its mail servers are the ones that
    /// were found for the address, which the form does not show.
    fn google_draft(&self) -> Result<SignInDraft, (ServerField, String)> {
        let found = self
            .draft()
            .map_err(|problem| (ServerField::Type, problem))?;
        if !quill_core::providers::signs_in_with_google(&found.imap.host) {
            return Err((
                ServerField::Type,
                format!(
                    "Signing in with Google is for Gmail and Google Workspace mailboxes. \
                     The server found for this address is {}: choose another type.",
                    found.imap.host
                ),
            ));
        }
        let client = self.google_client.text().trim();
        if client.is_empty() {
            return Err((
                ServerField::GoogleClient,
                "Enter the client ID Google shows for the application: it ends in \
                 .apps.googleusercontent.com."
                    .to_owned(),
            ));
        }
        if client.contains(char::is_whitespace) {
            return Err((
                ServerField::GoogleClient,
                "A client ID has no spaces in it.".to_owned(),
            ));
        }
        let client_secret = match self.google_secret.text().trim() {
            // Quill's own application has its own.
            "" if self.own_google => None,
            "" => {
                return Err((
                    ServerField::GoogleSecret,
                    "Enter the client secret Google shows beside the client ID.".to_owned(),
                ));
            }
            secret if secret.contains(char::is_whitespace) => {
                return Err((
                    ServerField::GoogleSecret,
                    "A client secret has no spaces in it.".to_owned(),
                ));
            }
            secret => Some(secret.to_owned()),
        };
        Ok(SignInDraft {
            name: found.name,
            address: found.address,
            application_id: client.to_owned(),
            tenant: None,
            google: Some(GoogleDraft {
                client_secret,
                imap: found.imap,
                smtp: found.smtp,
            }),
        })
    }

    /// The account that signs in with its provider as the fields describe
    /// it, or which field is wrong and how.
    fn sign_in_draft(&self) -> Result<SignInDraft, (ServerField, String)> {
        if self.kind == Kind::Google {
            return self.google_draft();
        }
        let address = self.address.text().trim().to_owned();
        let name = match self.name.text().trim() {
            "" => address.clone(),
            name => name.to_owned(),
        };
        let application_id = self.application.text().trim();
        if application_id.is_empty() {
            return Err((
                ServerField::Application,
                "Enter the application id: the Application (client) ID Microsoft shows for Quill."
                    .to_owned(),
            ));
        }
        if application_id.contains(char::is_whitespace) {
            return Err((
                ServerField::Application,
                "An application id has no spaces in it.".to_owned(),
            ));
        }
        let tenant = match self.tenant.text().trim() {
            "" => None,
            tenant if tenant.contains(char::is_whitespace) => {
                return Err((
                    ServerField::Tenant,
                    "A tenant is an id or a domain name, with no spaces. Leave it empty for any."
                        .to_owned(),
                ));
            }
            tenant => Some(tenant.to_owned()),
        };
        Ok(SignInDraft {
            name,
            address,
            application_id: application_id.to_owned(),
            tenant,
            google: None,
        })
    }

    fn verify(&mut self) -> Vec<AccountRequest> {
        match self.draft() {
            Ok(draft) => {
                self.busy = Some(format!("Signing in to {}…", draft.imap.host));
                vec![match self.editing.clone() {
                    Some(account) => AccountRequest::VerifyChange {
                        account,
                        draft,
                        password: self.typed_password(),
                        trusting: self.trusting.clone(),
                    },
                    None => AccountRequest::Verify {
                        draft,
                        password: Secret(self.password.text().to_owned()),
                        trusting: self.trusting.clone(),
                    },
                }]
            }
            Err(problem) => {
                self.step = Step::Server;
                self.error = Some(problem);
                Vec::new()
            }
        }
    }

    /// The password typed, if one was: an account being changed may be
    /// left with the one it has.
    fn typed_password(&self) -> Option<Secret> {
        (!self.password.is_empty()).then(|| Secret(self.password.text().to_owned()))
    }

    fn reject_certificate(&mut self) {
        self.problem = None;
        self.trusting = Trusting::System;
        self.step = Step::Server;
        self.error = Some(
            "The server's certificate was not accepted, so nothing was sent to it.".to_owned(),
        );
    }

    fn accept_certificate(&mut self, pin: bool) -> Vec<AccountRequest> {
        let Some(fingerprint) = self
            .problem
            .as_ref()
            .and_then(|problem| problem.fingerprint.clone())
        else {
            // Nothing to hold the server to: it cannot be accepted.
            return Vec::new();
        };
        self.trusting = if pin {
            Trusting::Pinned(fingerprint)
        } else {
            Trusting::Once(fingerprint)
        };
        self.step = Step::Password;
        self.verify()
    }

    /// Takes an answer from the service.
    pub fn on_event(&mut self, event: AccountEvent) -> (Vec<AccountRequest>, Outcome) {
        // An answer to a question that was abandoned with `esc` is dropped.
        if self.busy.is_none() {
            return (Vec::new(), Outcome::Open);
        }
        match event {
            AccountEvent::Discovered { address, found } => {
                if self.step != Step::Address || address != self.address.text() {
                    return (Vec::new(), Outcome::Open);
                }
                self.busy = None;
                let draft = found.draft;
                self.name.set(draft.name);
                self.host.set(draft.imap.host);
                self.port.set(draft.imap.port.to_string());
                self.tls = draft.imap.tls;
                self.username.set(draft.imap.username);
                match draft.smtp {
                    Some(smtp) => {
                        self.smtp_host.set(smtp.host);
                        self.smtp_port.set(smtp.port.to_string());
                        self.smtp_tls = smtp.tls;
                    }
                    None => {
                        self.smtp_host.set(String::new());
                        self.smtp_port.set("465");
                        self.smtp_tls = TlsMode::Implicit;
                    }
                }
                self.note = found.note;
                self.trusting = Trusting::System;
                self.step = Step::Server;
                // A Microsoft server takes no password, so the account is
                // offered as the kind that needs none.
                self.kind = match quill_core::providers::takes_no_password(self.host.text()) {
                    Some(_) => Kind::Microsoft,
                    None => Kind::Imap,
                };
                // An id typed before going back to the address is kept.
                if self.application.is_empty()
                    && let Some(id) = found.microsoft_application
                {
                    self.application.set(id);
                    self.own_application = found.microsoft_own;
                }
                if self.google_client.is_empty()
                    && let Some(application) = found.google_application
                {
                    let application = *application;
                    self.google_client.set(application.client_id);
                    self.google_secret
                        .set(application.client_secret.unwrap_or_default());
                    self.own_google = found.google_own;
                }
                // A Google mailbox signs in with Google where there is an
                // application to sign in as. Where there is none yet, it
                // is offered with a password, which is an app password,
                // and the other type is the user's to choose.
                if quill_core::providers::signs_in_with_google(self.host.text())
                    && !self.google_client.is_empty()
                {
                    self.kind = Kind::Google;
                }
                // Found: the cursor waits on the name, the one thing only
                // the user can choose. Not found: on the server to fill in.
                // A Microsoft account with no application id yet: on that.
                self.field = match self.kind {
                    Kind::Microsoft if self.application.is_empty() => ServerField::Application,
                    Kind::Google if self.google_client.is_empty() => ServerField::GoogleClient,
                    Kind::Imap if !found.found => ServerField::Host,
                    _ => ServerField::Name,
                }
                .index();
                (Vec::new(), Outcome::Open)
            }
            AccountEvent::Verified(result) => {
                if self.step != Step::Password {
                    return (Vec::new(), Outcome::Open);
                }
                self.busy = None;
                match result {
                    Ok(()) => match self.draft() {
                        Ok(draft) => {
                            self.busy = Some(format!("Saving {}…", draft.name));
                            let save = match self.editing.clone() {
                                Some(account) => AccountRequest::Change {
                                    account,
                                    draft,
                                    password: self.typed_password(),
                                    trusting: self.trusting.clone(),
                                },
                                None => AccountRequest::Save {
                                    draft,
                                    password: Secret(self.password.text().to_owned()),
                                    trusting: self.trusting.clone(),
                                },
                            };
                            (vec![save], Outcome::Open)
                        }
                        Err(problem) => {
                            self.step = Step::Server;
                            self.error = Some(problem);
                            (Vec::new(), Outcome::Open)
                        }
                    },
                    Err(VerifyProblem::Auth(reason)) => {
                        let reason = text::sanitize_line(&reason);
                        // At a provider that wants a password of its own
                        // making, the normal one typed correctly is what
                        // was refused: say what to type instead.
                        self.error = Some(
                            match quill_core::providers::password_advice(self.host.text()) {
                                Some(advice) => format!(
                                    "The server did not accept that name and password. \
                                     {advice} It said: {reason}"
                                ),
                                None => format!(
                                    "The server did not accept that name and password. It \
                                     said: {reason}"
                                ),
                            },
                        );
                        (Vec::new(), Outcome::Open)
                    }
                    Err(VerifyProblem::Password(reason)) => {
                        self.error = Some(text::sanitize_line(&reason));
                        (Vec::new(), Outcome::Open)
                    }
                    Err(VerifyProblem::Certificate(problem)) => {
                        self.problem = Some(problem);
                        self.step = Step::Certificate;
                        (Vec::new(), Outcome::Open)
                    }
                    Err(VerifyProblem::Sending(reason)) => {
                        // Reading works. Put the sending server right, or
                        // do without one.
                        self.step = Step::Server;
                        self.field = ServerField::SmtpHost.index();
                        let without = if self.editing.is_some() {
                            "clear it to leave the account without sending"
                        } else {
                            "clear it to add the account without sending"
                        };
                        self.error = Some(format!(
                            "Mail can be read, but not sent through {}: {reason} Correct it, or {without}.",
                            self.smtp_host.text().trim()
                        ));
                        (Vec::new(), Outcome::Open)
                    }
                    Err(VerifyProblem::Other(reason)) => {
                        // Most likely the server's details: go back to them.
                        self.step = Step::Server;
                        self.field = ServerField::Host.index();
                        self.error = Some(reason);
                        (Vec::new(), Outcome::Open)
                    }
                }
            }
            AccountEvent::Saved(result) => {
                self.busy = None;
                match result {
                    // An account that signs in with its provider is saved
                    // from the server step, and has still to be signed in
                    // to. That is asked for at once, for the account just
                    // made.
                    Ok(name) if self.kind.signs_in() && self.step == Step::Server => {
                        let sign_in = AccountRequest::SignIn {
                            account: Some(name.clone()),
                        };
                        (vec![sign_in], Outcome::SigningIn(name))
                    }
                    Ok(name) if self.editing.is_some() => (Vec::new(), Outcome::Changed(name)),
                    Ok(name) => (Vec::new(), Outcome::Added(name)),
                    Err(reason) => {
                        self.error = Some(format!("The account could not be saved: {reason}"));
                        (Vec::new(), Outcome::Open)
                    }
                }
            }
            // Not this form's business.
            AccountEvent::Trusted { .. }
            | AccountEvent::SignInBrowser { .. }
            | AccountEvent::SignInCode { .. }
            | AccountEvent::SignedIn { .. }
            | AccountEvent::Unsubscribed { .. }
            | AccountEvent::RemoveAsked { .. }
            | AccountEvent::Removed { .. }
            | AccountEvent::EditAsked { .. }
            | AccountEvent::Remembered(_)
            | AccountEvent::Settings { .. } => (Vec::new(), Outcome::Open),
        }
    }
}

/// A certificate an existing account's server presented, waiting for the
/// user to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificatePrompt {
    pub account: String,
    pub problem: Box<CertificateProblem>,
    /// It is the server the account sends mail through.
    pub sending: bool,
    /// The answer has been given and is being acted on.
    pub busy: bool,
}

impl CertificatePrompt {
    /// What the question is called: whose certificate, and of which of
    /// the account's servers.
    pub fn title(&self) -> String {
        if self.sending {
            format!("{}: certificate for sending", self.account)
        } else {
            format!("{}: certificate", self.account)
        }
    }
}

/// What the form says when a password account is asked for at a server
/// that takes no password from a mail program: how the account is added
/// instead. One short line, because that is the room the form can count on
/// under its fields; the note above them has already said why.
fn no_password(whose: &str) -> String {
    format!("{whose} takes no password. Change Type to {whose}.")
}

/// Why a certificate was refused, in words for someone who has never read a
/// TLS error. Falls back to the verifier's own text.
pub fn certificate_reason(reason: &str) -> String {
    let known = [
        (
            "UnknownIssuer",
            "It was issued by an authority this Mac does not trust. A self-signed certificate reads this way.",
        ),
        ("Expired", "It has expired."),
        ("NotValidYet", "It is not valid yet."),
        (
            "NotValidForName",
            "It was issued for a different server name.",
        ),
        ("BadSignature", "Its signature is not valid."),
        ("Revoked", "It has been revoked by its issuer."),
        (
            "InvalidPurpose",
            "It was not issued for identifying a server.",
        ),
    ];
    known
        .iter()
        .find(|(code, _)| reason.contains(code))
        .map_or_else(|| reason.to_owned(), |(_, words)| (*words).to_owned())
}

/// A SHA-256 in hex as pairs with colons, the way certificate viewers show
/// it, for comparing by eye.
pub fn grouped_fingerprint(hex: &str) -> String {
    let pairs: Vec<&str> = hex
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| std::str::from_utf8(pair).ok())
        .collect();
    pairs.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn keys(setup: &mut Setup, keys: &str) -> Vec<AccountRequest> {
        let mut requests = Vec::new();
        for key in keys.split_whitespace() {
            let chord: KeyChord = key.parse().unwrap();
            match chord.key {
                Key::Enter => requests.extend(setup.advance()),
                Key::Tab => setup.move_field(!chord.mods.shift),
                _ => requests.extend(setup.key(chord).unwrap_or_default()),
            }
        }
        requests
    }

    pub(super) fn found(address: &str) -> AccountEvent {
        AccountEvent::Discovered {
            address: address.to_owned(),
            found: Discovery {
                draft: AccountDraft {
                    name: "Example".to_owned(),
                    address: address.to_owned(),
                    imap: ServerDraft {
                        host: "imap.example.com".to_owned(),
                        port: 993,
                        tls: TlsMode::Implicit,
                        username: address.to_owned(),
                    },
                    smtp: Some(ServerDraft {
                        host: "smtp.example.com".to_owned(),
                        port: 465,
                        tls: TlsMode::Implicit,
                        username: address.to_owned(),
                    }),
                },
                found: true,
                note: Some("Found in the DNS records of example.com.".to_owned()),
                microsoft_application: None,
                microsoft_own: false,
                google_application: None,
                google_own: false,
            },
        }
    }

    /// What discovery says of an address whose mail Microsoft hosts, and
    /// the application id the config already has, if it has one.
    pub(super) fn found_at_microsoft(address: &str, application: Option<&str>) -> AccountEvent {
        let AccountEvent::Discovered { address, mut found } = found(address) else {
            unreachable!()
        };
        found.draft.imap.host = "outlook.office365.com".to_owned();
        found.draft.smtp = Some(ServerDraft {
            host: "smtp.office365.com".to_owned(),
            port: 587,
            tls: TlsMode::Starttls,
            username: address.clone(),
        });
        found.note = Some("Mail for example.com is hosted by Microsoft.".to_owned());
        found.microsoft_application = application.map(str::to_owned);
        AccountEvent::Discovered { address, found }
    }

    /// What discovery says of a Gmail address, with the application the
    /// service has to offer for signing in to Google, if it has one, and
    /// whether that one is Quill's own.
    pub(super) fn found_at_google(
        application: Option<(&str, Option<&str>)>,
        own: bool,
    ) -> AccountEvent {
        let AccountEvent::Discovered { address, mut found } = found("robin@gmail.com") else {
            unreachable!()
        };
        found.draft.name = "Gmail".to_owned();
        found.draft.imap.host = "imap.gmail.com".to_owned();
        found.draft.smtp = Some(ServerDraft {
            host: "smtp.gmail.com".to_owned(),
            port: 465,
            tls: TlsMode::Implicit,
            username: address.clone(),
        });
        found.note = Some(
            "Gmail's usual servers. Google does not accept your normal password here.".to_owned(),
        );
        found.google_application = application.map(|(id, secret)| {
            Box::new(GoogleApplication {
                client_id: id.to_owned(),
                client_secret: secret.map(str::to_owned),
            })
        });
        found.google_own = own;
        AccountEvent::Discovered { address, found }
    }

    /// The form at the server step, for a Gmail address.
    pub(super) fn at_google(application: Option<(&str, Option<&str>)>, own: bool) -> Setup {
        let mut setup = Setup::default();
        setup.address.set("robin@gmail.com");
        setup.advance();
        setup.on_event(found_at_google(application, own));
        assert_eq!(setup.step, Step::Server);
        setup
    }

    fn gmail_servers() -> (ServerDraft, Option<ServerDraft>) {
        let server = |host: &str, port| ServerDraft {
            host: host.to_owned(),
            port,
            tls: TlsMode::Implicit,
            username: "robin@gmail.com".to_owned(),
        };
        (
            server("imap.gmail.com", 993),
            Some(server("smtp.gmail.com", 465)),
        )
    }

    #[test]
    fn with_no_google_application_a_gmail_address_is_offered_with_a_password_and_can_sign_in() {
        // Nobody has registered Quill with Google: the account is offered
        // as one with a password, which the form says is an app password.
        let mut setup = at_google(None, false);
        assert_eq!(setup.kind, Kind::Imap);
        setup.advance();
        assert_eq!(setup.step, Step::Password);
        assert!(setup.password_note().contains("app password"));
        keys(&mut setup, "esc");
        setup.back();
        assert_eq!(setup.step, Step::Server);

        // Signing in with Google is the user's to choose, and then asks
        // for the application to sign in as: both halves of it.
        setup.field = ServerField::Type.index();
        keys(&mut setup, "left");
        assert_eq!(setup.kind, Kind::Google);
        assert_eq!(setup.title(), "Add an account: Google");
        let shown: Vec<ServerField> = ServerField::ALL
            .into_iter()
            .filter(|field| setup.shows(*field))
            .collect();
        assert_eq!(
            shown,
            [
                ServerField::Type,
                ServerField::Name,
                ServerField::GoogleClient,
                ServerField::GoogleSecret
            ]
        );
        assert_eq!(keys(&mut setup, "enter"), []);
        assert_eq!(setup.server_field(), ServerField::GoogleClient);
        assert!(setup.error.as_deref().unwrap().contains("client ID"));
        setup
            .google_client
            .set(" 123-abc.apps.googleusercontent.com ");
        assert_eq!(keys(&mut setup, "enter"), []);
        assert_eq!(setup.server_field(), ServerField::GoogleSecret);
        assert!(setup.error.as_deref().unwrap().contains("client secret"));
        setup.google_secret.set("GOCSPX-not-a-real-one");

        // No password is asked for and no server is signed in to: the
        // account is saved, with the mail servers that were found.
        let (imap, smtp) = gmail_servers();
        assert_eq!(
            keys(&mut setup, "enter"),
            [AccountRequest::SaveSignIn {
                draft: SignInDraft {
                    name: "Gmail".to_owned(),
                    address: "robin@gmail.com".to_owned(),
                    application_id: "123-abc.apps.googleusercontent.com".to_owned(),
                    tenant: None,
                    google: Some(GoogleDraft {
                        client_secret: Some("GOCSPX-not-a-real-one".to_owned()),
                        imap,
                        smtp,
                    }),
                }
            }]
        );
        assert_eq!(setup.busy.as_deref(), Some("Saving Gmail…"));
        // Saved: it goes on to the sign-in, as a Microsoft account does.
        let (requests, outcome) = setup.on_event(AccountEvent::Saved(Ok("Gmail".to_owned())));
        assert_eq!(
            requests,
            [AccountRequest::SignIn {
                account: Some("Gmail".to_owned())
            }]
        );
        assert_eq!(outcome, Outcome::SigningIn("Gmail".to_owned()));
    }

    #[test]
    fn with_a_google_application_to_offer_a_gmail_address_signs_in_with_google() {
        // One an account in the config already signs in as: offered again,
        // and shown, since it is the user's own.
        let mut setup = at_google(
            Some(("mine.apps.googleusercontent.com", Some("GOCSPX-mine"))),
            false,
        );
        assert_eq!(setup.kind, Kind::Google);
        assert_eq!(setup.server_field(), ServerField::Name);
        assert!(setup.shows(ServerField::GoogleClient) && setup.shows(ServerField::GoogleSecret));
        assert_eq!(
            setup.google_client.text(),
            "mine.apps.googleusercontent.com"
        );
        assert_eq!(setup.google_secret.text(), "GOCSPX-mine");
        let saved = keys(&mut setup, "enter");
        assert!(matches!(
            saved.as_slice(),
            [AccountRequest::SaveSignIn { draft }]
                if draft.google.as_ref().and_then(|google| google.client_secret.as_deref())
                    == Some("GOCSPX-mine")
        ));

        // Quill's own: nothing to fill in but the name, and nothing of the
        // application is shown or written.
        let mut setup = at_google(Some(("quill.apps.googleusercontent.com", None)), true);
        assert_eq!(setup.kind, Kind::Google);
        assert!(!setup.shows(ServerField::GoogleClient) && !setup.shows(ServerField::GoogleSecret));
        let (imap, smtp) = gmail_servers();
        assert_eq!(
            keys(&mut setup, "enter"),
            [AccountRequest::SaveSignIn {
                draft: SignInDraft {
                    name: "Gmail".to_owned(),
                    address: "robin@gmail.com".to_owned(),
                    application_id: "quill.apps.googleusercontent.com".to_owned(),
                    tenant: None,
                    google: Some(GoogleDraft {
                        client_secret: None,
                        imap,
                        smtp,
                    }),
                }
            }]
        );
    }

    #[test]
    fn signing_in_with_google_is_for_googles_mail_servers_only() {
        // An address whose mail is somewhere else, with the type changed
        // by hand: said, and nothing saved.
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        assert_eq!(setup.kind, Kind::Imap);
        setup.field = ServerField::Type.index();
        keys(&mut setup, "left");
        assert_eq!(setup.kind, Kind::Google);
        setup.google_client.set("mine.apps.googleusercontent.com");
        setup.google_secret.set("GOCSPX-mine");
        assert_eq!(keys(&mut setup, "enter"), []);
        let said = setup.error.clone().unwrap_or_default();
        assert!(
            said.contains("Gmail and Google Workspace") && said.contains("imap.example.com"),
            "{said}"
        );
        assert_eq!(setup.server_field(), ServerField::Type);
        // An offer of an application does not make another server's
        // account a Google one.
        let AccountEvent::Discovered { address, mut found } = found("sam@example.com") else {
            unreachable!()
        };
        found.google_application = Some(Box::new(GoogleApplication {
            client_id: "mine.apps.googleusercontent.com".to_owned(),
            client_secret: None,
        }));
        let mut setup = Setup::default();
        setup.address.set("sam@example.com");
        setup.advance();
        setup.on_event(AccountEvent::Discovered { address, found });
        assert_eq!(setup.kind, Kind::Imap);
    }

    /// The form at the server step, for an address Microsoft hosts.
    pub(super) fn at_microsoft(application: Option<&str>) -> Setup {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found_at_microsoft("robin@example.com", application));
        assert_eq!(setup.step, Step::Server);
        setup
    }

    pub(super) fn problem() -> Box<CertificateProblem> {
        Box::new(CertificateProblem {
            host: "imap.example.com".to_owned(),
            reason: "UnknownIssuer".to_owned(),
            fingerprint: Some("ab".repeat(32)),
            certificate: None,
            details: None,
        })
    }

    pub(super) fn at_password() -> Setup {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        setup.advance();
        assert_eq!(setup.step, Step::Password);
        setup.password.set("hunter2");
        setup
    }

    #[test]
    fn the_type_is_chosen_from_the_server_that_was_found() {
        // A server that takes a password: an IMAP account, as ever.
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        assert_eq!(setup.kind, Kind::Imap);
        assert_eq!(setup.server_field(), ServerField::Name);
        assert!(setup.shows(ServerField::Host) && !setup.shows(ServerField::Application));

        // One of Microsoft's: a Microsoft account, with the cursor on the
        // one thing it still needs.
        let setup = at_microsoft(None);
        assert_eq!(setup.kind, Kind::Microsoft);
        assert_eq!(setup.server_field(), ServerField::Application);
        assert_eq!(setup.application.text(), "");
        assert_eq!(setup.name.text(), "Example");
        // It has no servers to name.
        for field in [
            ServerField::Host,
            ServerField::Port,
            ServerField::Security,
            ServerField::Username,
            ServerField::SmtpHost,
            ServerField::SmtpPort,
            ServerField::SmtpSecurity,
        ] {
            assert!(!setup.shows(field), "{field:?}");
        }

        // An application id the config already has is offered again, and
        // then the name is all there is left to choose.
        let mut setup = at_microsoft(Some("app-1"));
        assert_eq!(setup.application.text(), "app-1");
        assert_eq!(setup.server_field(), ServerField::Name);

        // Going back for another address chooses again, and keeps an id
        // that was typed.
        setup.application.set("app-2");
        assert!(!setup.back());
        setup.address.set("robin@example.org");
        setup.advance();
        setup.on_event(found("robin@example.org"));
        assert_eq!(setup.kind, Kind::Imap);
        assert!(!setup.back());
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found_at_microsoft("robin@example.com", Some("app-1")));
        assert_eq!(setup.kind, Kind::Microsoft);
        assert_eq!(setup.application.text(), "app-2");
    }

    #[test]
    fn the_type_changes_with_the_arrow_keys() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        // The type is the first field, before the name.
        keys(&mut setup, "shift-tab");
        assert_eq!(setup.server_field(), ServerField::Type);
        assert_eq!(setup.focused_input(), None);

        keys(&mut setup, "right");
        assert_eq!(setup.kind, Kind::Microsoft);
        keys(&mut setup, "right");
        assert_eq!(setup.kind, Kind::Google);
        keys(&mut setup, "right");
        assert_eq!(setup.kind, Kind::Imap);
        keys(&mut setup, "left");
        assert_eq!(setup.kind, Kind::Google);
        keys(&mut setup, "space");
        assert_eq!(setup.kind, Kind::Imap);
        // Letters are not typed into a choice, and change nothing.
        assert_eq!(keys(&mut setup, "m x"), []);
        assert_eq!(setup.kind, Kind::Imap);
        // Another key is not the form's: it goes on to be a command.
        assert_eq!(setup.key("down".parse().unwrap()), None);

        // Changing the type takes back what was said about the old one.
        setup.error = Some("said of the other type".to_owned());
        keys(&mut setup, "right");
        assert_eq!(
            (setup.kind, setup.error.as_deref()),
            (Kind::Microsoft, None)
        );

        // The fields that follow are the type's own, and go round.
        let mut seen = Vec::new();
        for _ in 0..4 {
            keys(&mut setup, "tab");
            seen.push(setup.server_field());
        }
        assert_eq!(
            seen,
            [
                ServerField::Name,
                ServerField::Application,
                ServerField::Tenant,
                ServerField::Type
            ]
        );
        keys(&mut setup, "shift-tab");
        assert_eq!(setup.server_field(), ServerField::Tenant);
        keys(&mut setup, "e x a m p l e . c o m");
        assert_eq!(setup.tenant.text(), "example.com");

        // Elsewhere the arrows are not the type's: in the name they move
        // the cursor, and the type stays.
        keys(&mut setup, "shift-tab shift-tab");
        assert_eq!(setup.server_field(), ServerField::Name);
        keys(&mut setup, "left right");
        assert_eq!(setup.kind, Kind::Microsoft);
    }

    #[test]
    fn a_microsoft_account_that_signs_in_as_quill_asks_for_nothing_but_its_name() {
        // The service offers Quill's own application: there is no id to
        // type and no tenant to name.
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        let AccountEvent::Discovered { address, mut found } =
            found_at_microsoft("robin@example.com", Some("quills-own-id"))
        else {
            unreachable!()
        };
        found.microsoft_own = true;
        setup.on_event(AccountEvent::Discovered { address, found });
        assert_eq!(setup.step, Step::Server);
        assert_eq!(setup.kind, Kind::Microsoft);
        assert!(setup.own_application);
        assert!(!setup.shows(ServerField::Application));
        assert!(!setup.shows(ServerField::Tenant));
        // The cursor is on the name, and tab goes round the two fields
        // there are without stopping at one that is not shown.
        assert_eq!(setup.server_field(), ServerField::Name);
        setup.move_field(true);
        assert_eq!(setup.server_field(), ServerField::Type);
        setup.move_field(true);
        assert_eq!(setup.server_field(), ServerField::Name);
        // Continuing saves it with that application and no tenant.
        assert_eq!(
            setup.advance(),
            [AccountRequest::SaveSignIn {
                draft: SignInDraft {
                    name: "Example".to_owned(),
                    address: "robin@example.com".to_owned(),
                    application_id: "quills-own-id".to_owned(),
                    tenant: None,
                    google: None,
                },
            }]
        );
        // The sheet is told the same: no field for the id, and what will
        // happen said under the address.
        use quill_host::bridge::FormField;
        let form = at_own_microsoft().form(&jiff::tz::TimeZone::UTC).unwrap();
        let ids: Vec<&str> = form.fields().iter().map(FormField::id).collect();
        assert!(
            !ids.contains(&"application") && !ids.contains(&"tenant"),
            "{ids:?}"
        );
        assert!(
            form.fields().iter().any(|field| matches!(
                field,
                FormField::Static { help: Some(help), .. } if help == MICROSOFT_OWN_NOTE
            )),
            "{:?}",
            form.fields()
        );

        // An application of the account's own, from the config, is shown
        // to be seen and changed, as before.
        let theirs = at_microsoft(Some("app-1"));
        assert!(!theirs.own_application);
        assert!(theirs.shows(ServerField::Application));
    }

    /// The form at the server step for an address Microsoft hosts, with
    /// Quill's own application on offer.
    fn at_own_microsoft() -> Setup {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        let AccountEvent::Discovered { address, mut found } =
            found_at_microsoft("robin@example.com", Some("quills-own-id"))
        else {
            unreachable!()
        };
        found.microsoft_own = true;
        setup.on_event(AccountEvent::Discovered { address, found });
        setup
    }

    #[test]
    fn a_microsoft_account_needs_its_application_id_and_may_name_a_tenant() {
        let mut setup = at_microsoft(None);
        // No id: nothing is asked for, and the cursor is where it goes.
        setup.field = ServerField::Name.index();
        assert_eq!(setup.advance(), []);
        assert_eq!(setup.step, Step::Server);
        assert_eq!(setup.server_field(), ServerField::Application);
        let said = setup.error.clone().unwrap();
        assert!(said.contains("application id"), "{said}");
        for blank in ["   ", "\t"] {
            setup.application.set(blank);
            assert_eq!(setup.advance(), [], "{blank:?}");
            assert!(setup.error.is_some(), "{blank:?}");
        }
        // An id is one word.
        setup.application.set("app 1");
        assert_eq!(setup.advance(), []);
        assert!(setup.error.as_deref().unwrap().contains("no spaces"));
        // Typing takes the complaint away.
        keys(&mut setup, "x");
        assert_eq!(setup.error, None);

        // The tenant is one word too, when there is one.
        setup.application.set("app-1");
        setup.tenant.set("not a tenant");
        assert_eq!(setup.advance(), []);
        assert_eq!(setup.server_field(), ServerField::Tenant);
        assert!(setup.error.as_deref().unwrap().contains("tenant"));

        // Each complaint fits the two lines the form has for it, at the
        // narrowest the app shows the form.
        for said in [
            {
                setup.application.set("");
                setup.advance();
                setup.error.clone().unwrap()
            },
            {
                setup.application.set("a b");
                setup.advance();
                setup.error.clone().unwrap()
            },
            {
                setup.application.set("app-1");
                setup.advance();
                setup.error.clone().unwrap()
            },
        ] {
            assert!(text::wrap(&said, 58).len() <= 2, "{said}");
        }

        // With no tenant the account is for any organisation. What was
        // typed around the id and the name is trimmed off.
        setup.application.set("  0f1e2d3c-app-1 ");
        setup.tenant.set("  ");
        setup.name.set(" Work ");
        assert_eq!(
            setup.advance(),
            [AccountRequest::SaveSignIn {
                draft: SignInDraft {
                    name: "Work".to_owned(),
                    address: "robin@example.com".to_owned(),
                    application_id: "0f1e2d3c-app-1".to_owned(),
                    tenant: None,
                    google: None,
                }
            }]
        );
        assert_eq!(setup.busy.as_deref(), Some("Saving Work…"));
        // It was never asked for a password.
        assert_eq!(setup.step, Step::Server);
        assert!(setup.password.is_empty());

        // With one, it is named. A name left empty is the address.
        let mut setup = at_microsoft(Some("app-1"));
        setup.name.set("");
        setup.tenant.set(" example.com ");
        assert_eq!(
            setup.advance(),
            [AccountRequest::SaveSignIn {
                draft: SignInDraft {
                    name: "robin@example.com".to_owned(),
                    address: "robin@example.com".to_owned(),
                    application_id: "app-1".to_owned(),
                    tenant: Some("example.com".to_owned()),
                    google: None,
                }
            }]
        );
    }

    #[test]
    fn a_saved_microsoft_account_is_signed_in_to_at_once() {
        let mut setup = at_microsoft(Some("app-1"));
        setup.name.set("Work");
        let requests = setup.advance();
        assert!(
            matches!(requests.as_slice(), [AccountRequest::SaveSignIn { .. }]),
            "{requests:?}"
        );
        // Saved: the form is over, and the sign-in is asked for, for the
        // account just made and no other.
        let (requests, outcome) = setup.on_event(AccountEvent::Saved(Ok("Work".to_owned())));
        assert_eq!(
            requests,
            [AccountRequest::SignIn {
                account: Some("Work".to_owned())
            }]
        );
        assert_eq!(outcome, Outcome::SigningIn("Work".to_owned()));

        // Not saved: the form stays where it was and says why. Nothing is
        // signed in to.
        let mut setup = at_microsoft(Some("app-1"));
        setup.advance();
        let (requests, outcome) = setup.on_event(AccountEvent::Saved(Err(
            "there is already an account called Example".to_owned(),
        )));
        assert_eq!((requests, outcome), (Vec::new(), Outcome::Open));
        assert_eq!((setup.step, setup.busy.clone()), (Step::Server, None));
        assert!(
            setup
                .error
                .as_deref()
                .unwrap()
                .contains("already an account called Example")
        );

        // A password account that is saved is not signed in to this way.
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Ok(())));
        let (requests, outcome) = setup.on_event(AccountEvent::Saved(Ok("Example".to_owned())));
        assert_eq!(
            (requests, outcome),
            (Vec::new(), Outcome::Added("Example".to_owned()))
        );
    }

    #[test]
    fn a_password_account_at_a_microsoft_server_is_never_asked_for_a_password() {
        let mut setup = at_microsoft(None);
        // The user would have a password account all the same.
        setup.field = ServerField::Type.index();
        keys(&mut setup, "left");
        assert_eq!(setup.kind, Kind::Imap);
        keys(&mut setup, "tab");
        // Microsoft's servers, however the address came to them: found
        // for the address, or typed in for a domain of the user's own.
        for host in [
            "outlook.office365.com",
            "Outlook.Office365.com.",
            "imap-mail.outlook.com",
        ] {
            setup.host.set(host);
            setup.field = ServerField::Host.index();
            // Going on stays where it is, asks nothing of the server, and
            // says which type adds the account, with the cursor on it.
            assert_eq!(setup.advance(), [], "{host}");
            assert_eq!(setup.step, Step::Server, "{host}");
            assert_eq!(setup.server_field(), ServerField::Type, "{host}");
            let said = setup.error.clone().unwrap_or_default();
            assert!(
                said.contains("Microsoft takes no password"),
                "{host}: {said}"
            );
            assert!(said.contains("Change Type to Microsoft"), "{said}");
            // One line where the app shows the form at its narrowest:
            // under all the fields of a server, longer is cut off.
            assert_eq!(text::wrap(&said, 58).len(), 1, "{said}");
        }
        // One key from there it is a Microsoft account, and the complaint
        // is gone.
        keys(&mut setup, "right");
        assert_eq!(
            (setup.kind, setup.error.as_deref()),
            (Kind::Microsoft, None)
        );

        // Another server, with a name that only looks alike, is asked.
        keys(&mut setup, "left");
        setup.host.set("imap.notoutlook.com");
        assert_eq!(setup.advance(), []);
        assert_eq!(setup.step, Step::Password);
    }

    #[test]
    fn a_provider_that_wants_an_app_password_is_said_so_where_it_is_typed_and_when_refused() {
        let mut setup = at_password();
        // A server Quill does not know: nothing more to say.
        assert!(!setup.password_note().contains("app password"));
        setup.host.set("imap.gmail.com");
        let note = setup.password_note();
        assert!(note.starts_with("The password for "), "{note}");
        assert!(
            note.ends_with(
                "Google does not accept your normal password here. Create an app password \
                 in your Google account (Security, 2-Step Verification, App passwords) and \
                 use that."
            ),
            "{note}"
        );
        // Refused, as Google refuses a normal password that is right.
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Auth(
            "Application-specific password required: \
             https://support.google.com/accounts/answer/185833 (Failure)"
                .to_owned(),
        ))));
        let said = setup.error.clone().unwrap_or_default();
        assert!(
            said.starts_with(
                "The server did not accept that name and password. Google does not accept \
                 your normal password here. Create an app password"
            ),
            "{said}"
        );
        assert!(
            said.ends_with("It said: Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)"),
            "{said}"
        );
        assert_eq!(setup.step, Step::Password);
    }

    #[test]
    fn what_the_server_said_is_shown_as_one_clean_line() {
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Auth(
            "Basic authentication\nis disabled.\u{1b}[31m".to_owned(),
        ))));
        let said = setup.error.clone().unwrap_or_default();
        assert!(said.starts_with("The server did not accept that name and password."));
        assert!(!said.contains('\n') && !said.contains('\u{1b}'), "{said:?}");
    }

    #[test]
    fn the_whole_flow_from_address_to_saved() {
        let mut setup = Setup::default();
        setup.address.set(" robin@example.com ");
        assert_eq!(
            setup.advance(),
            [AccountRequest::Discover {
                address: "robin@example.com".to_owned()
            }]
        );
        assert!(setup.busy.as_deref().unwrap().contains("example.com"));

        setup.on_event(found("robin@example.com"));
        assert_eq!(setup.step, Step::Server);
        assert_eq!(setup.busy, None);
        assert_eq!(setup.host.text(), "imap.example.com");
        assert_eq!(setup.port.text(), "993");
        assert_eq!(setup.server_field(), ServerField::Name);
        assert!(setup.note.is_some());

        assert!(setup.advance().is_empty());
        assert_eq!(setup.step, Step::Password);
        setup.password.set("hunter2");
        let requests = setup.advance();
        let [
            AccountRequest::Verify {
                draft,
                password,
                trusting,
            },
        ] = requests.as_slice()
        else {
            panic!("{requests:?}");
        };
        assert_eq!(draft.name, "Example");
        assert_eq!(draft.imap.username, "robin@example.com");
        assert_eq!(draft.smtp.as_ref().unwrap().host, "smtp.example.com");
        assert_eq!(password.0, "hunter2");
        assert_eq!(*trusting, Trusting::System);

        let (requests, outcome) = setup.on_event(AccountEvent::Verified(Ok(())));
        assert_eq!(outcome, Outcome::Open);
        assert!(matches!(requests.as_slice(), [AccountRequest::Save { .. }]));
        let (_, outcome) = setup.on_event(AccountEvent::Saved(Ok("Example".to_owned())));
        assert_eq!(outcome, Outcome::Added("Example".to_owned()));
    }

    #[test]
    fn what_is_not_an_address_goes_nowhere() {
        for address in [
            "",
            "robin",
            "robin@",
            "@example.com",
            "robin@localhost",
            "a b@example.com",
        ] {
            let mut setup = Setup::default();
            setup.address.set(address);
            assert!(setup.advance().is_empty(), "{address}");
            assert!(setup.error.is_some(), "{address}");
            assert_eq!(setup.busy, None);
        }
    }

    #[test]
    fn nothing_found_leaves_the_server_to_fill_in() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        let AccountEvent::Discovered { address, mut found } = found("robin@example.com") else {
            unreachable!()
        };
        found.found = false;
        found.draft.imap.host = String::new();
        setup.on_event(AccountEvent::Discovered { address, found });
        assert_eq!(setup.server_field(), ServerField::Host);
        assert!(setup.advance().is_empty());
        assert_eq!(setup.step, Step::Server);
        assert!(setup.error.as_deref().unwrap().contains("mail server"));

        keys(&mut setup, "m a i l . e x a m p l e . c o m tab");
        setup.port.set("70000");
        setup.advance();
        assert!(setup.error.as_deref().unwrap().contains("port"));
        setup.port.set("993");
        setup.advance();
        assert_eq!(setup.step, Step::Password);
    }

    #[test]
    fn the_kind_of_connection_cycles_and_takes_the_usual_port_with_it() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        keys(&mut setup, "tab tab tab");
        assert_eq!(setup.server_field(), ServerField::Security);
        keys(&mut setup, "right");
        assert_eq!((setup.tls, setup.port.text()), (TlsMode::Starttls, "143"));
        keys(&mut setup, "space");
        assert_eq!(setup.tls, TlsMode::None);
        keys(&mut setup, "space");
        assert_eq!((setup.tls, setup.port.text()), (TlsMode::Implicit, "993"));
        keys(&mut setup, "left left");
        assert_eq!(setup.tls, TlsMode::Starttls);
        // A port the user chose is left alone.
        setup.port.set("1143");
        keys(&mut setup, "right");
        assert_eq!(setup.port.text(), "1143");
        // Letters are not typed into a choice.
        assert_eq!(keys(&mut setup, "x"), []);
        keys(&mut setup, "tab tab shift-tab");
        assert_eq!(setup.server_field(), ServerField::Username);
    }

    #[test]
    fn the_sending_server_is_shown_to_correct_or_do_without() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("robin@example.com"));
        assert_eq!(
            (
                setup.smtp_host.text(),
                setup.smtp_port.text(),
                setup.smtp_tls
            ),
            ("smtp.example.com", "465", TlsMode::Implicit)
        );

        // Past the reading server's fields to the sending server's.
        keys(&mut setup, "tab tab tab tab tab");
        assert_eq!(setup.server_field(), ServerField::SmtpHost);
        keys(&mut setup, "tab tab");
        assert_eq!(setup.server_field(), ServerField::SmtpSecurity);
        // Its usual ports are its own.
        keys(&mut setup, "right");
        assert_eq!(
            (setup.smtp_tls, setup.smtp_port.text()),
            (TlsMode::Starttls, "587")
        );
        assert_eq!((setup.tls, setup.port.text()), (TlsMode::Implicit, "993"));
        keys(&mut setup, "shift-tab");
        setup.smtp_port.set("2525");
        let draft = setup.draft().unwrap();
        let smtp = draft.smtp.unwrap();
        assert_eq!(
            (smtp.host.as_str(), smtp.port, smtp.tls),
            ("smtp.example.com", 2525, TlsMode::Starttls)
        );
        // It is signed in to under the same name as the reading server.
        assert_eq!(smtp.username, draft.imap.username);

        // A port that is not one stops the step.
        setup.smtp_port.set("0");
        assert!(setup.advance().is_empty());
        assert!(
            setup
                .error
                .as_deref()
                .unwrap()
                .contains("sending server's port")
        );

        // With no server named the account only reads, and the fields that
        // describe the server are passed over.
        setup.smtp_host.clear();
        assert_eq!(setup.draft().unwrap().smtp, None);
        setup.field = ServerField::SmtpHost.index();
        keys(&mut setup, "tab");
        assert_eq!(setup.server_field(), ServerField::Type);
        keys(&mut setup, "shift-tab");
        assert_eq!(setup.server_field(), ServerField::SmtpHost);
        assert!(setup.advance().is_empty());
        assert_eq!(setup.step, Step::Password);
    }

    #[test]
    fn a_sending_server_that_does_not_work_is_put_right_before_saving() {
        let mut setup = at_password();
        setup.advance();
        let (requests, outcome) = setup.on_event(AccountEvent::Verified(Err(
            VerifyProblem::Sending("Cannot reach the server: connection refused.".to_owned()),
        )));
        assert!(requests.is_empty());
        assert_eq!(outcome, Outcome::Open);
        // Back at the server, on the field to correct, told what works.
        assert_eq!(setup.step, Step::Server);
        assert_eq!(setup.server_field(), ServerField::SmtpHost);
        let error = setup.error.clone().unwrap();
        assert!(error.contains("Mail can be read"), "{error}");
        assert!(error.contains("smtp.example.com"), "{error}");
        assert!(error.contains("connection refused"), "{error}");
        // The password typed is kept for the next try.
        setup.smtp_host.clear();
        setup.advance();
        assert_eq!(setup.password.text(), "hunter2");
        let requests = setup.advance();
        assert!(
            matches!(requests.as_slice(), [AccountRequest::Verify { draft, .. }] if draft.smtp.is_none()),
            "{requests:?}"
        );
    }

    #[test]
    fn a_refused_password_stays_at_the_password() {
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Auth(
            "Authentication failed.".to_owned(),
        ))));
        assert_eq!(setup.step, Step::Password);
        assert!(
            setup
                .error
                .as_deref()
                .unwrap()
                .contains("Authentication failed.")
        );
        // Typing clears the complaint.
        keys(&mut setup, "x");
        assert_eq!(setup.error, None);
    }

    #[test]
    fn an_unreachable_server_goes_back_to_its_details() {
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Other(
            "cannot reach the server".to_owned(),
        ))));
        assert_eq!(
            (setup.step, setup.server_field()),
            (Step::Server, ServerField::Host)
        );
        assert_eq!(setup.error.as_deref(), Some("cannot reach the server"));
    }

    #[test]
    fn an_untrusted_certificate_is_put_to_the_user() {
        // Reject: back to the server's details, nothing saved.
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
            problem(),
        ))));
        assert_eq!(setup.step, Step::Certificate);
        assert_eq!(keys(&mut setup, "r"), []);
        assert_eq!(setup.step, Step::Server);
        assert!(setup.error.as_deref().unwrap().contains("not accepted"));

        // Trust once: sign in again holding the server to that certificate.
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
            problem(),
        ))));
        let requests = keys(&mut setup, "o");
        assert!(matches!(
            requests.as_slice(),
            [AccountRequest::Verify { trusting: Trusting::Once(fingerprint), .. }] if *fingerprint == "ab".repeat(32)
        ));
        // And the choice is carried into what is saved.
        let (requests, _) = setup.on_event(AccountEvent::Verified(Ok(())));
        assert!(matches!(
            requests.as_slice(),
            [AccountRequest::Save {
                trusting: Trusting::Once(_),
                ..
            }]
        ));

        // Pin.
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
            problem(),
        ))));
        let requests = keys(&mut setup, "p");
        assert!(matches!(
            requests.as_slice(),
            [AccountRequest::Verify {
                trusting: Trusting::Pinned(_),
                ..
            }]
        ));

        // A certificate that could not even be read cannot be accepted.
        let mut setup = at_password();
        setup.advance();
        let mut unreadable = problem();
        unreadable.fingerprint = None;
        setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
            unreadable,
        ))));
        assert_eq!(keys(&mut setup, "o p"), []);
        assert_eq!(setup.step, Step::Certificate);
    }

    #[test]
    fn esc_goes_back_a_step_and_abandons_a_wait() {
        let mut setup = at_password();
        assert!(!setup.back());
        assert_eq!(setup.step, Step::Server);
        assert!(!setup.back());
        assert_eq!(setup.step, Step::Address);

        // During a wait, esc gives up on the answer, which is then ignored.
        setup.advance();
        assert!(setup.busy.is_some());
        assert!(!setup.back());
        assert_eq!(setup.busy, None);
        setup.on_event(found("robin@example.com"));
        assert_eq!(setup.step, Step::Address);

        // From the first step there is nowhere back to go.
        assert!(setup.back());
    }

    #[test]
    fn an_answer_for_another_address_is_ignored() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        setup.on_event(found("someone.else@example.org"));
        assert_eq!(setup.step, Step::Address);
        assert!(setup.busy.is_some());
    }

    #[test]
    fn typing_waits_while_quill_is_busy() {
        let mut setup = Setup::default();
        setup.address.set("robin@example.com");
        setup.advance();
        assert_eq!(keys(&mut setup, "x y z"), []);
        assert_eq!(setup.address.text(), "robin@example.com");
        assert!(setup.advance().is_empty());
        setup.paste("pasted");
        assert_eq!(setup.address.text(), "robin@example.com");
    }

    #[test]
    fn a_failed_save_says_why_and_keeps_the_password_step() {
        let mut setup = at_password();
        setup.advance();
        setup.on_event(AccountEvent::Verified(Ok(())));
        let (_, outcome) =
            setup.on_event(AccountEvent::Saved(Err("the Keychain refused".to_owned())));
        assert_eq!(outcome, Outcome::Open);
        assert_eq!(setup.step, Step::Password);
        assert!(
            setup
                .error
                .as_deref()
                .unwrap()
                .contains("the Keychain refused")
        );
    }

    #[test]
    fn a_secret_does_not_print() {
        let request = AccountRequest::Verify {
            draft: AccountDraft {
                name: "x".to_owned(),
                address: "robin@example.com".to_owned(),
                imap: ServerDraft {
                    host: "h".to_owned(),
                    port: 1,
                    tls: TlsMode::None,
                    username: "u".to_owned(),
                },
                smtp: None,
            },
            password: Secret("hunter2".to_owned()),
            trusting: Trusting::System,
        };
        assert!(!format!("{request:?}").contains("hunter2"));
    }

    #[test]
    fn certificate_reasons_and_fingerprints_are_made_readable() {
        assert!(certificate_reason("UnknownIssuer").contains("self-signed"));
        assert_eq!(certificate_reason("Expired"), "It has expired.");
        assert!(certificate_reason("NotValidForName").contains("different server name"));
        assert_eq!(
            certificate_reason("the pinned certificate does not match"),
            "the pinned certificate does not match"
        );
        assert_eq!(grouped_fingerprint("0a1b2c"), "0a:1b:2c");
        assert_eq!(grouped_fingerprint(&"ab".repeat(32)).len(), 95);
    }
}
