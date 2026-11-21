//! The add-an-account form as a host shows it.
//!
//! Quill's app shows the form in a sheet on its window instead of having it
//! drawn in the terminal. It is the same form: [`Setup`] is the one state
//! machine, and this is its second view. [`Setup::form`] describes what to
//! show, and [`Setup::used`] takes what the user did with it, which ends in
//! the same steps the keys take in a terminal. Nothing about accounts is
//! decided on the host's side.
//!
//! The other questions Quill asks are shown the same way, and are
//! described beside the state that holds them (`state/sheet.rs`), with the
//! pieces here: how a title is written, how a field is described, and what
//! a certificate question says, which is the same whether it comes up
//! while an account is added or for one that is already there.

use jiff::tz::TimeZone;
use quill_core::CertificateProblem;
use quill_core::config::TlsMode;
use quill_host::bridge::{
    FormButton, FormField, FormOption, FormRole, FormShow, FormShowParams, FormUsed,
};

use super::{
    ADDRESS_NOTE, AccountRequest, GOOGLE_NOTE, GOOGLE_OWN_NOTE, Kind, LETTER_HELP, MICROSOFT_NOTE,
    MICROSOFT_OWN_NOTE, NO_ENCRYPTION, NO_SENDING, SECURITIES, ServerField, Setup, Step,
    UNREADABLE_CERTIFICATE, certificate_note, certificate_reason, grouped_fingerprint,
    security_label,
};
use crate::input::LineInput;
use crate::text;

/// The name of the address step's one field.
const ADDRESS: &str = "address";
/// The name of the password step's one field.
const PASSWORD: &str = "password";

impl Step {
    /// The step's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Step::Address => "address",
            Step::Server => "server",
            Step::Password => "password",
            Step::Certificate => "certificate",
        }
    }
}

impl Kind {
    /// The kind's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Imap => "imap",
            Kind::Microsoft => "microsoft",
            Kind::Google => "google",
        }
    }

    fn named(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
}

impl ServerField {
    /// The field's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            ServerField::Type => "type",
            ServerField::Name => "name",
            ServerField::Letter => "letter",
            ServerField::Host => "host",
            ServerField::Port => "port",
            ServerField::Security => "security",
            ServerField::Username => "username",
            ServerField::SmtpHost => "smtp_host",
            ServerField::SmtpPort => "smtp_port",
            ServerField::SmtpSecurity => "smtp_security",
            ServerField::Application => "application",
            ServerField::Tenant => "tenant",
            ServerField::GoogleClient => "google_client",
            ServerField::GoogleSecret => "google_secret",
        }
    }
}

/// A kind of connection's name on the bridge.
fn security_id(tls: TlsMode) -> &'static str {
    match tls {
        TlsMode::Implicit => "tls",
        TlsMode::Starttls => "starttls",
        TlsMode::None => "none",
    }
}

fn security_named(id: &str) -> Option<TlsMode> {
    SECURITIES.into_iter().find(|tls| security_id(*tls) == id)
}

/// The buttons the form has, at one step or another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// On to the next step: `enter`, in a terminal.
    Continue,
    /// Back a step: `esc`.
    Back,
    /// Put the form away, from whichever step.
    Cancel,
    /// Of a certificate: `r`.
    Reject,
    /// `o`.
    TrustOnce,
    /// `p`.
    TrustAlways,
}

impl Press {
    pub const ALL: [Press; 6] = [
        Press::Continue,
        Press::Back,
        Press::Cancel,
        Press::Reject,
        Press::TrustOnce,
        Press::TrustAlways,
    ];

    /// The button's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Press::Continue => "continue",
            Press::Back => "back",
            Press::Cancel => "cancel",
            Press::Reject => "reject",
            Press::TrustOnce => "trust_once",
            Press::TrustAlways => "trust_always",
        }
    }

    /// What the button says.
    pub fn title(self) -> &'static str {
        match self {
            Press::Continue => "Continue",
            Press::Back => "Back",
            Press::Cancel => "Cancel",
            Press::Reject => "Reject",
            Press::TrustOnce => "Trust Once",
            Press::TrustAlways => "Always Trust",
        }
    }

    pub(crate) fn named(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|press| press.id() == id)
    }

    pub(crate) fn button(self, role: Option<FormRole>, enabled: bool) -> FormButton {
        FormButton {
            id: self.id().to_owned(),
            title: self.title().to_owned(),
            role,
            enabled,
        }
    }
}

/// What came of the form being used: work for the accounts service, and
/// whether the form is to be put away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Used {
    pub requests: Vec<AccountRequest>,
    pub close: bool,
}

/// A title of the form's in the capitals a Mac gives the title of a sheet:
/// every word but the small ones, and those too at the start, at the end
/// and after a colon. The words are the terminal's own.
pub(crate) fn headline(title: &str) -> String {
    const SMALL: [&str; 9] = ["a", "an", "and", "for", "in", "of", "or", "the", "to"];
    let words: Vec<&str> = title.split(' ').collect();
    let last = words.len().saturating_sub(1);
    let mut opening = true;
    let mut out = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let small = SMALL.contains(word) && !opening && index != last;
        let mut letters = word.chars();
        out.push(match letters.next() {
            Some(first) if !small => first.to_uppercase().chain(letters).collect(),
            _ => (*word).to_owned(),
        });
        opening = word.ends_with(':');
    }
    out.join(" ")
}

/// A text field as the host shows it. What it holds may have come from a
/// server (a name found for the account), so it is cleaned on the way.
pub(crate) fn typed(
    id: &str,
    label: &str,
    input: &LineInput,
    focus: bool,
    help: Option<&str>,
) -> FormField {
    FormField::Text {
        id: id.to_owned(),
        label: label.to_owned(),
        value: text::sanitize_line(input.text()),
        focus,
        help: help.map(str::to_owned),
    }
}

pub(crate) fn shown(id: &str, label: &str, value: String) -> FormField {
    FormField::Static {
        id: id.to_owned(),
        label: label.to_owned(),
        value,
        help: None,
    }
}

fn chosen(
    field: ServerField,
    label: &str,
    value: &str,
    options: Vec<FormOption>,
    focus: bool,
) -> FormField {
    FormField::Choice {
        id: field.id().to_owned(),
        label: label.to_owned(),
        value: value.to_owned(),
        options,
        focus,
        help: None,
    }
}

fn securities() -> Vec<FormOption> {
    SECURITIES
        .into_iter()
        .map(|tls| FormOption {
            id: security_id(tls).to_owned(),
            title: security_label(tls).to_owned(),
        })
        .collect()
}

/// Puts what a field of the host's held into the form's own, as typing or
/// pasting it would have: with nothing in it that a line cannot hold.
pub(crate) fn put(input: &mut LineInput, value: Option<&str>) {
    let Some(value) = value else { return };
    let mut line = LineInput::default();
    line.insert(value);
    if line.text() != input.text() {
        input.set(line.text().to_owned());
    }
}

/// A certificate question as a host shows it: what it says above the
/// certificate, what is wrong with it, the certificate in its own words,
/// and the three things that can be done about it. The same wherever the
/// question is asked: while an account is added, and of an account that
/// is already there.
pub(crate) struct CertificateAsked {
    pub note: String,
    /// What is drawn as a warning in a terminal: under the fields.
    pub wrong: Vec<String>,
    pub fields: Vec<FormField>,
    pub buttons: Vec<FormButton>,
}

/// Describes the question about `problem`. `waiting`: an answer has been
/// given and is being acted on, so only giving up can be pressed. `tz` is
/// for the certificate's dates.
pub(crate) fn certificate_asked(
    problem: &CertificateProblem,
    waiting: bool,
    tz: &TimeZone,
) -> CertificateAsked {
    let mut wrong = vec![certificate_reason(&problem.reason)];
    let mut fields = Vec::new();
    if let Some(details) = &problem.details {
        let date = |seconds| text::long_date(seconds, tz);
        fields.push(shown(
            "issued_to",
            "Issued To:",
            text::sanitize_line(&details.subject),
        ));
        fields.push(shown(
            "issued_by",
            "Issued By:",
            text::sanitize_line(&details.issuer),
        ));
        fields.push(shown(
            "valid",
            "Valid:",
            format!(
                "{} to {}",
                date(details.not_before),
                date(details.not_after)
            ),
        ));
    }
    let mut buttons = vec![Press::Reject.button(Some(FormRole::Cancel), true)];
    match &problem.fingerprint {
        Some(fingerprint) => {
            // Two rows of sixteen pairs, to compare with the server's own.
            let grouped = text::sanitize_line(&grouped_fingerprint(fingerprint));
            let (first, second) = grouped.split_at(grouped.len().min(48));
            let mut value = first.trim_end_matches(':').to_owned();
            if !second.is_empty() {
                value.push('\n');
                value.push_str(second);
            }
            fields.push(shown("sha256", "SHA-256:", value));
            buttons.push(Press::TrustOnce.button(None, !waiting));
            buttons.push(Press::TrustAlways.button(None, !waiting));
        }
        // What could not be read cannot be accepted.
        None => wrong.push(UNREADABLE_CERTIFICATE.to_owned()),
    }
    CertificateAsked {
        note: certificate_note(&problem.host),
        wrong,
        fields,
        buttons,
    }
}

impl Setup {
    /// The form as a host is to show it: the step it is at, what it says,
    /// its fields in order with what each holds, and its buttons from left
    /// to right. `tz` is for a certificate's dates.
    ///
    /// The fields and what is said of them are the terminal's. The labels
    /// are in the words and capitals a Mac's own forms use, since that is
    /// where these are read.
    ///
    /// The password is never part of it: a secret field has no value.
    pub fn form(&self, tz: &TimeZone) -> Option<FormShow> {
        let waiting = self.busy.is_some();
        let at = self.server_field();
        let mut fields = Vec::new();
        let mut note = None;
        // What is drawn as a warning in a terminal: under the fields.
        let mut wrong: Vec<String> = Vec::new();
        // The buttons of a certificate question, which come with it.
        let mut asked_buttons = None;
        match self.step {
            Step::Address => {
                note = Some(ADDRESS_NOTE.to_owned());
                fields.push(typed(ADDRESS, "Email Address:", &self.address, true, None));
            }
            Step::Server => {
                // What was said of the address when it was found is about
                // its password, which a Google sign-in has none of.
                if self.kind != Kind::Google {
                    note.clone_from(&self.note);
                }
                let server = |field: ServerField, label, input, help| {
                    typed(field.id(), label, input, at == field, help)
                };
                // An account that is there keeps its kind and its name:
                // which one it is is said, and neither is asked for.
                if self.editing.is_some() {
                    note = self.editing_note();
                } else {
                    fields.push(chosen(
                        ServerField::Type,
                        "Type:",
                        self.kind.id(),
                        Kind::ALL
                            .into_iter()
                            .map(|kind| FormOption {
                                id: kind.id().to_owned(),
                                title: kind.label().to_owned(),
                            })
                            .collect(),
                        at == ServerField::Type,
                    ));
                    fields.push(server(ServerField::Name, "Name:", &self.name, None));
                    fields.push(server(
                        ServerField::Letter,
                        "Letter:",
                        &self.letter,
                        Some(LETTER_HELP),
                    ));
                }
                match self.kind {
                    // Signing in as Quill itself: nothing more to fill in.
                    Kind::Microsoft if self.own_application => {
                        let mut address = shown(
                            ADDRESS,
                            "Address:",
                            text::sanitize_line(self.address.text()),
                        );
                        if let FormField::Static { help, .. } = &mut address {
                            *help = Some(MICROSOFT_OWN_NOTE.to_owned());
                        }
                        fields.push(address);
                    }
                    Kind::Microsoft => {
                        fields.push(shown(
                            ADDRESS,
                            "Address:",
                            text::sanitize_line(self.address.text()),
                        ));
                        fields.push(server(
                            ServerField::Application,
                            "Application ID:",
                            &self.application,
                            None,
                        ));
                        fields.push(server(
                            ServerField::Tenant,
                            "Tenant:",
                            &self.tenant,
                            Some(MICROSOFT_NOTE),
                        ));
                    }
                    // Signing in as Quill itself: nothing more to fill in.
                    Kind::Google if self.own_google => {
                        let mut address = shown(
                            ADDRESS,
                            "Address:",
                            text::sanitize_line(self.address.text()),
                        );
                        if let FormField::Static { help, .. } = &mut address {
                            *help = Some(GOOGLE_OWN_NOTE.to_owned());
                        }
                        fields.push(address);
                    }
                    Kind::Google => {
                        fields.push(shown(
                            ADDRESS,
                            "Address:",
                            text::sanitize_line(self.address.text()),
                        ));
                        fields.push(server(
                            ServerField::GoogleClient,
                            "Client ID:",
                            &self.google_client,
                            None,
                        ));
                        fields.push(server(
                            ServerField::GoogleSecret,
                            "Client Secret:",
                            &self.google_secret,
                            Some(GOOGLE_NOTE),
                        ));
                    }
                    Kind::Imap => {
                        fields.push(server(
                            ServerField::Host,
                            "Incoming Server:",
                            &self.host,
                            None,
                        ));
                        fields.push(server(ServerField::Port, "Port:", &self.port, None));
                        fields.push(chosen(
                            ServerField::Security,
                            "Security:",
                            security_id(self.tls),
                            securities(),
                            at == ServerField::Security,
                        ));
                        fields.push(server(
                            ServerField::Username,
                            "User Name:",
                            &self.username,
                            None,
                        ));
                        // A terminal hides the sending server's port and
                        // security while no server is named, as each key
                        // is typed. A sheet hears of what was typed only
                        // when a button is pressed, so there they are
                        // always to be seen.
                        fields.push(server(
                            ServerField::SmtpHost,
                            "Outgoing Server:",
                            &self.smtp_host,
                            Some(NO_SENDING),
                        ));
                        fields.push(server(
                            ServerField::SmtpPort,
                            "Port:",
                            &self.smtp_port,
                            None,
                        ));
                        fields.push(chosen(
                            ServerField::SmtpSecurity,
                            "Security:",
                            security_id(self.smtp_tls),
                            securities(),
                            at == ServerField::SmtpSecurity,
                        ));
                        if self.unencrypted() {
                            wrong.push(NO_ENCRYPTION.to_owned());
                        }
                    }
                }
            }
            Step::Password => {
                note = Some(self.password_note());
                fields.push(FormField::Secret {
                    id: PASSWORD.to_owned(),
                    label: "Password:".to_owned(),
                    focus: true,
                    help: None,
                });
            }
            Step::Certificate => {
                if let Some(problem) = &self.problem {
                    let asked = certificate_asked(problem, waiting, tz);
                    note = Some(asked.note);
                    wrong.extend(asked.wrong);
                    fields.extend(asked.fields);
                    asked_buttons = Some(asked.buttons);
                }
            }
        }
        wrong.extend(self.error.clone());
        let buttons = match self.step {
            Step::Certificate => asked_buttons
                .unwrap_or_else(|| vec![Press::Reject.button(Some(FormRole::Cancel), true)]),
            step => {
                let mut buttons = vec![Press::Cancel.button(Some(FormRole::Cancel), true)];
                // Not at the first step, which for an account being
                // changed is its servers.
                let first = match self.editing {
                    Some(_) => Step::Server,
                    None => Step::Address,
                };
                if step != first {
                    buttons.push(Press::Back.button(None, !waiting));
                }
                buttons.push(Press::Continue.button(Some(FormRole::Default), !waiting));
                buttons
            }
        };
        let lines = |words: Vec<String>| {
            let words: Vec<String> = words.iter().map(|line| text::sanitize_line(line)).collect();
            (!words.is_empty()).then(|| words.join("\n"))
        };
        FormShow::new(FormShowParams {
            step: self.step.id().to_owned(),
            title: headline(self.title()),
            note: note.map(|note| text::sanitize_line(&note)),
            error: lines(wrong),
            busy: self.busy.as_deref().map(text::sanitize_line),
            fields,
            buttons,
        })
    }

    /// Takes what the user did with the form a host is showing: what its
    /// fields held goes into the form's own, and then the button does what
    /// its key does in a terminal. With no button, a choice was changed,
    /// and the fields that follow from it change with it.
    ///
    /// An answer about a step the form has since left is dropped: it was
    /// given to a form that is no longer on screen.
    pub fn used(&mut self, used: &FormUsed) -> Used {
        let mut out = Used::default();
        if used.step() != self.step.id() {
            return out;
        }
        let press = match used.button() {
            None => None,
            Some(id) => match Press::named(id) {
                Some(press) => Some(press),
                // Not a button of this form.
                None => return out,
            },
        };
        match press {
            // Nothing typed is kept, and nothing is asked for. An answer
            // still on its way finds no form to give itself to.
            Some(Press::Cancel) => out.close = true,
            // As `esc`: back a step, out of a certificate question, or
            // out of a wait.
            Some(Press::Back) => {
                if self.busy.is_none() {
                    self.fill(used);
                }
                out.close = self.back();
            }
            // Typing waits while Quill is busy, and so does going on.
            Some(Press::Continue) if self.busy.is_none() => {
                self.fill(used);
                out.requests = self.advance();
            }
            None if self.busy.is_none() => self.fill(used),
            Some(Press::Reject) if self.asking_about_certificate() => self.reject_certificate(),
            Some(Press::TrustOnce) if self.asking_about_certificate() => {
                out.requests = self.accept_certificate(false);
            }
            Some(Press::TrustAlways) if self.asking_about_certificate() => {
                out.requests = self.accept_certificate(true);
            }
            _ => {}
        }
        out
    }

    fn asking_about_certificate(&self) -> bool {
        self.step == Step::Certificate && self.busy.is_none()
    }

    /// Puts what the host's fields held into the form's own. Only the
    /// fields of the step the form is at, and of the kind of account it
    /// was showing: anything else in the answer is not this form's.
    fn fill(&mut self, used: &FormUsed) {
        match self.step {
            Step::Address => put(&mut self.address, used.value(ADDRESS)),
            Step::Password => {
                if let Some(secret) = used.secret(PASSWORD) {
                    put(&mut self.password, Some(secret.reveal()));
                }
            }
            Step::Certificate => {}
            Step::Server => {
                // Only what the form showed: an account being changed
                // was shown neither its name nor its kind.
                let editing = self.editing.is_some();
                let value = |field: ServerField| match field {
                    ServerField::Name | ServerField::Type | ServerField::Letter if editing => None,
                    field => used.value(field.id()),
                };
                put(&mut self.name, value(ServerField::Name));
                put(&mut self.letter, value(ServerField::Letter));
                match self.kind {
                    Kind::Microsoft => {
                        put(&mut self.application, value(ServerField::Application));
                        put(&mut self.tenant, value(ServerField::Tenant));
                    }
                    Kind::Google => {
                        put(&mut self.google_client, value(ServerField::GoogleClient));
                        put(&mut self.google_secret, value(ServerField::GoogleSecret));
                    }
                    Kind::Imap => {
                        put(&mut self.host, value(ServerField::Host));
                        put(&mut self.port, value(ServerField::Port));
                        put(&mut self.username, value(ServerField::Username));
                        put(&mut self.smtp_host, value(ServerField::SmtpHost));
                        put(&mut self.smtp_port, value(ServerField::SmtpPort));
                        // After the ports, which follow the kind of
                        // connection while they are the usual ones.
                        for (field, sending) in [
                            (ServerField::Security, false),
                            (ServerField::SmtpSecurity, true),
                        ] {
                            let now = if sending { self.smtp_tls } else { self.tls };
                            if let Some(mode) = value(field).and_then(security_named)
                                && mode != now
                            {
                                self.set_security(sending, mode);
                            }
                        }
                    }
                }
                // Last: the fields above were those of the old kind.
                if let Some(kind) = value(ServerField::Type).and_then(Kind::named)
                    && kind != self.kind
                {
                    self.set_kind(kind);
                    // The cursor is not left in a field that has gone.
                    if !self.shows(self.server_field()) {
                        self.field = ServerField::Type.index();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
