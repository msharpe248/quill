//! Adding an account from inside Quill, and deciding about a server
//! certificate the system does not trust.
//!
//! The flow: type an address, let Quill find the server, confirm or correct
//! what it found, give the password, and sign in. Only when sign-in has
//! worked is anything saved.
//!
//! Nothing here touches the network, the Keychain or a file. Each step asks
//! for work with an [`AccountRequest`] and hears the answer as an
//! [`AccountEvent`]; whoever runs the UI supplies the service that does the
//! work.

use quill_core::CertificateProblem;
use quill_core::config::TlsMode;

use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Mods};

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
    /// Accept an existing account's certificate, once or from now on.
    Trust {
        account: String,
        fingerprint: String,
        pin: bool,
        /// The certificate is the sending server's.
        sending: bool,
    },
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyProblem {
    /// The server refused the name and password.
    Auth(String),
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

/// The fields of the server step, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerField {
    Name,
    Host,
    Port,
    Security,
    Username,
    /// The server mail is sent through. Empty: the account only reads.
    SmtpHost,
    SmtpPort,
    SmtpSecurity,
}

impl ServerField {
    pub const ALL: [ServerField; 8] = [
        ServerField::Name,
        ServerField::Host,
        ServerField::Port,
        ServerField::Security,
        ServerField::Username,
        ServerField::SmtpHost,
        ServerField::SmtpPort,
        ServerField::SmtpSecurity,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub step: Step,
    pub address: LineInput,
    pub name: LineInput,
    pub host: LineInput,
    pub port: LineInput,
    pub tls: TlsMode,
    pub username: LineInput,
    pub smtp_host: LineInput,
    pub smtp_port: LineInput,
    pub smtp_tls: TlsMode,
    pub password: LineInput,
    /// Which field of the server step has the cursor.
    pub field: usize,
    /// What is being waited for, in words. Keys other than `esc` wait too.
    pub busy: Option<String>,
    pub error: Option<String>,
    pub note: Option<String>,
    /// The certificate being asked about.
    pub problem: Option<Box<CertificateProblem>>,
    trusting: Trusting,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            step: Step::Address,
            address: LineInput::default(),
            name: LineInput::default(),
            host: LineInput::default(),
            port: LineInput::default(),
            tls: TlsMode::Implicit,
            username: LineInput::default(),
            smtp_host: LineInput::default(),
            smtp_port: LineInput::new("465".to_owned()),
            smtp_tls: TlsMode::Implicit,
            password: LineInput::masked(),
            field: 0,
            busy: None,
            error: None,
            note: None,
            problem: None,
            trusting: Trusting::System,
        }
    }
}

impl Setup {
    pub fn server_field(&self) -> ServerField {
        ServerField::ALL[self.field.min(ServerField::ALL.len() - 1)]
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
                ServerField::Security | ServerField::SmtpSecurity => None,
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
                ServerField::Security | ServerField::SmtpSecurity => None,
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
        let choosing = match self.server_field() {
            ServerField::Security => Some(false),
            ServerField::SmtpSecurity => Some(true),
            _ => None,
        };
        if self.step == Step::Server
            && let Some(sending) = choosing
        {
            let order = [TlsMode::Implicit, TlsMode::Starttls, TlsMode::None];
            let (tls, port) = if sending {
                (&mut self.smtp_tls, &mut self.smtp_port)
            } else {
                (&mut self.tls, &mut self.port)
            };
            let at = order.iter().position(|mode| mode == tls).unwrap_or(0);
            let next = match chord.key {
                Key::Right | Key::Char(' ') if plain => (at + 1) % order.len(),
                Key::Left if plain => (at + order.len() - 1) % order.len(),
                Key::Char(_) => return Some(Vec::new()),
                _ => return None,
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
                port.set(usual(order[next]));
            }
            *tls = order[next];
            self.error = None;
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
                // With no sending server named, its port and security are
                // not shown and not stopped at.
                let hidden = self.smtp_host.is_empty()
                    && matches!(
                        self.server_field(),
                        ServerField::SmtpPort | ServerField::SmtpSecurity
                    );
                if !hidden {
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
            Step::Server => {
                if let Err(problem) = self.draft() {
                    self.error = Some(problem);
                    return Vec::new();
                }
                self.step = Step::Password;
                Vec::new()
            }
            Step::Password => {
                if self.password.is_empty() {
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

    fn verify(&mut self) -> Vec<AccountRequest> {
        match self.draft() {
            Ok(draft) => {
                self.busy = Some(format!("Signing in to {}…", draft.imap.host));
                vec![AccountRequest::Verify {
                    draft,
                    password: Secret(self.password.text().to_owned()),
                    trusting: self.trusting.clone(),
                }]
            }
            Err(problem) => {
                self.step = Step::Server;
                self.error = Some(problem);
                Vec::new()
            }
        }
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
                // Found: the cursor waits on the name, the one thing only
                // the user can choose. Not found: on the server to fill in.
                self.field = usize::from(!found.found);
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
                            let save = AccountRequest::Save {
                                draft,
                                password: Secret(self.password.text().to_owned()),
                                trusting: self.trusting.clone(),
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
                        self.error = Some(format!(
                            "The server did not accept that name and password. It said: {reason}"
                        ));
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
                        self.error = Some(format!(
                            "Mail can be read, but not sent through {}: {reason} Correct it, or clear it to add the account without sending.",
                            self.smtp_host.text().trim()
                        ));
                        (Vec::new(), Outcome::Open)
                    }
                    Err(VerifyProblem::Other(reason)) => {
                        // Most likely the server's details: go back to them.
                        self.step = Step::Server;
                        self.field = 1;
                        self.error = Some(reason);
                        (Vec::new(), Outcome::Open)
                    }
                }
            }
            AccountEvent::Saved(result) => {
                self.busy = None;
                match result {
                    Ok(name) => (Vec::new(), Outcome::Added(name)),
                    Err(reason) => {
                        self.error = Some(format!("The account could not be saved: {reason}"));
                        (Vec::new(), Outcome::Open)
                    }
                }
            }
            AccountEvent::Trusted { .. } => (Vec::new(), Outcome::Open),
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

    fn keys(setup: &mut Setup, keys: &str) -> Vec<AccountRequest> {
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

    fn found(address: &str) -> AccountEvent {
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
            },
        }
    }

    fn problem() -> Box<CertificateProblem> {
        Box::new(CertificateProblem {
            host: "imap.example.com".to_owned(),
            reason: "UnknownIssuer".to_owned(),
            fingerprint: Some("ab".repeat(32)),
            certificate: None,
            details: None,
        })
    }

    fn at_password() -> Setup {
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
        assert_eq!(setup.server_field(), ServerField::Name);
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
