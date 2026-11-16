//! The forms and questions Quill asks, as a host shows them.
//!
//! In a terminal the form to add an account, a sign-in, a certificate, a
//! time to name and unsubscribing are each drawn over the panes. Quill's app
//! shows them in a sheet on its window instead ([`Env::sheets`]). They are
//! the same questions: each is still held in [`State`], which is the one
//! state machine, and this is its second view of them. [`State::form`]
//! describes the one in front, and [`State::on_form`] takes what the user
//! did with it, which ends in exactly what the keys do in a terminal.
//!
//! One sheet at a time: the question a terminal would have in front is the
//! one described, and an answer is taken only for the form on show, at the
//! step it names. What is chosen from with the keyboard (the palette, the
//! pickers, the help) is no sheet: it is drawn in the terminal whatever the
//! host.

use std::hash::{Hash, Hasher};

use quill_host::bridge::{
    FormButton, FormField, FormOption, FormRole, FormShow, FormShowParams, FormUsed, Secret,
};

use super::{Effects, Env, State, UnsubscribePrompt, WhenFor, WhenPrompt};
use crate::command::{CommandId, Context};
use crate::setup::{
    AccountRequest, CERTIFICATE_WORKING, CertificatePrompt, Press, SENDING_CERTIFICATE,
    SIGN_IN_BROWSER, SIGN_IN_CODE, SIGN_IN_STARTING, SIGN_IN_WAITING, SignIn, SignInStage, Used,
    certificate_asked, headline, put, shown, sign_in_code_lasts, typed,
};
use crate::{text, when};

/// The step of the sign-in's form. It has one: what it says changes, and
/// what can be done about it does not.
const SIGN_IN: &str = "sign_in";
/// The steps of the two questions that ask for a time.
const SNOOZE: &str = "snooze";
const SEND_LATER: &str = "send_later";
/// The step of the question about unsubscribing.
const UNSUBSCRIBE: &str = "unsubscribe";
/// What the step of a question about an existing account's certificate
/// starts with. The rest is the certificate's own: see
/// [`CertificatePrompt::step`].
const ACCOUNT_CERTIFICATE: &str = "account_certificate";

/// The button that goes ahead with what a question asked: `enter`, in a
/// terminal.
const CONFIRM: &str = "confirm";
/// The field a time is typed in, and the choice of ready-made ones.
const WHEN: &str = "when";
const OFFERED: &str = "offered";
/// The choice that is none of the ready-made times: the time is typed.
const NOT_OFFERED: &str = "typed";

/// What a sign-in in the browser says beside its progress indicator.
const SIGN_IN_IN_BROWSER: &str = "Waiting for you to sign in…";

/// Whether what is asked in `context` is a form or a question that a host
/// with sheets shows, and a terminal draws over the panes.
fn asked_in_a_sheet(context: Context) -> bool {
    matches!(
        context,
        Context::Setup
            | Context::Certificate
            | Context::SignIn
            | Context::When
            | Context::Unsubscribe
    )
}

/// The name on the bridge of a ready-made time, from the words it is typed
/// as: a name starts with a letter.
fn offered_id(typed: &str) -> String {
    if typed.starts_with(|first: char| first.is_ascii_digit()) {
        format!("in_{typed}")
    } else {
        typed.to_owned()
    }
}

/// The words a ready-made time is typed as, from its name on the bridge.
fn offered_named(id: &str) -> Option<&'static str> {
    when::OFFERED
        .into_iter()
        .map(|(typed, _)| typed)
        .find(|typed| offered_id(typed) == id)
}

/// Several paragraphs of a form's note or error, each made safe to show.
fn paragraphs(words: &[&str]) -> Option<String> {
    let words: Vec<String> = words.iter().map(|line| text::sanitize_line(line)).collect();
    (!words.is_empty()).then(|| words.join("\n\n"))
}

fn button(id: &str, title: &str, role: FormRole) -> FormButton {
    FormButton {
        id: id.to_owned(),
        title: title.to_owned(),
        role: Some(role),
        enabled: true,
    }
}

/// The button that puts a question away, with nothing done: `esc`.
fn cancel() -> FormButton {
    Press::Cancel.button(Some(FormRole::Cancel), true)
}

/// Text shown to be read off and typed somewhere else. It is kept out of
/// whatever prints the form.
fn code(id: &str, label: &str, value: &str, help: Option<String>) -> FormField {
    FormField::Code {
        id: id.to_owned(),
        label: label.to_owned(),
        value: Secret::new(text::sanitize_line(value)),
        help,
    }
}

impl SignIn {
    /// The sign-in as a host shows it: whose it is, what is happening and
    /// what the user is to do about it, and the one thing that can be done
    /// here, which is to give up. It is waiting from start to end, and
    /// says on what.
    ///
    /// The words are the terminal's. The address the browser was sent to
    /// is no part of it: it went to the host to be opened, and is shown
    /// nowhere. A code and the page to type it at are shown, and are kept
    /// out of anything that prints the form.
    pub fn form(&self) -> Option<FormShow> {
        let mut fields = Vec::new();
        let (note, busy) = match &self.stage {
            SignInStage::Starting => (None, SIGN_IN_STARTING),
            SignInStage::Browser => (paragraphs(&SIGN_IN_BROWSER), SIGN_IN_IN_BROWSER),
            SignInStage::Code {
                address,
                code: digits,
                minutes,
            } => {
                fields.push(code("page", "Page:", address, None));
                fields.push(code(
                    "code",
                    "Code:",
                    digits,
                    Some(sign_in_code_lasts(*minutes)),
                ));
                (paragraphs(&[SIGN_IN_CODE]), SIGN_IN_WAITING)
            }
        };
        // The terminal's title in a sheet's capitals. The account's name
        // is the user's own, and is left as they wrote it.
        let title = if self.account.is_empty() {
            "Sign In".to_owned()
        } else {
            format!("Sign In to {}", text::sanitize_line(&self.account))
        };
        FormShow::new(FormShowParams {
            step: SIGN_IN.to_owned(),
            title,
            note,
            error: None,
            busy: Some(busy.to_owned()),
            fields,
            buttons: vec![cancel()],
        })
    }
}

impl CertificatePrompt {
    /// The step of this question's form: one no other certificate's
    /// question has. One such question can follow another at once, so an
    /// answer on its way to the first (a second click) must not be taken
    /// for the answer to the next, which nobody has read yet.
    pub fn step(&self) -> String {
        let mut hasher = std::hash::DefaultHasher::new();
        self.account.hash(&mut hasher);
        self.sending.hash(&mut hasher);
        self.problem.host.hash(&mut hasher);
        self.problem.reason.hash(&mut hasher);
        self.problem.fingerprint.hash(&mut hasher);
        format!("{ACCOUNT_CERTIFICATE}_{:016x}", hasher.finish())
    }

    /// The question as a host shows it: the same description of the
    /// certificate and the same three buttons as when an account is being
    /// added, under the account's name. `tz` is for its dates.
    pub fn form(&self, tz: &jiff::tz::TimeZone) -> Option<FormShow> {
        let asked = certificate_asked(&self.problem, self.busy, tz);
        // The account's name is the user's own, and is left as they wrote
        // it.
        let title = format!(
            "{}: {}",
            text::sanitize_line(&self.account),
            headline(if self.sending {
                "certificate for sending"
            } else {
                "certificate"
            })
        );
        let note = if self.sending {
            paragraphs(&[SENDING_CERTIFICATE, &asked.note])
        } else {
            paragraphs(&[&asked.note])
        };
        let wrong: Vec<String> = asked
            .wrong
            .iter()
            .map(|line| text::sanitize_line(line))
            .collect();
        FormShow::new(FormShowParams {
            step: self.step(),
            title,
            note,
            error: (!wrong.is_empty()).then(|| wrong.join("\n")),
            busy: self.busy.then(|| CERTIFICATE_WORKING.to_owned()),
            fields: asked.fields,
            buttons: asked.buttons,
        })
    }
}

impl WhenPrompt {
    /// The step of this question's form.
    fn step(&self) -> &'static str {
        match self.what {
            WhenFor::Snooze(_) => SNOOZE,
            WhenFor::SendLater => SEND_LATER,
        }
    }

    /// The question as a host shows it: the line a time is typed on, with
    /// the terminal's examples under it, and the first of those examples
    /// to choose ready-made. Choosing one types it on the line, so what is
    /// on the line is always what will be read, and it is read here and
    /// nowhere else.
    pub fn form(&self) -> Option<FormShow> {
        let help = format!("{}\n{}", when::such_as(), self.aside());
        let typed_now = self.input.text().trim().to_lowercase();
        let chosen = when::OFFERED
            .into_iter()
            .find(|(typed, _)| *typed == typed_now)
            .map_or_else(|| NOT_OFFERED.to_owned(), |(typed, _)| offered_id(typed));
        let mut options = vec![FormOption {
            id: NOT_OFFERED.to_owned(),
            title: "None".to_owned(),
        }];
        options.extend(when::OFFERED.into_iter().map(|(typed, called)| FormOption {
            id: offered_id(typed),
            title: called.to_owned(),
        }));
        let go = match self.what {
            WhenFor::Snooze(_) => "Snooze",
            WhenFor::SendLater => "Send Later",
        };
        FormShow::new(FormShowParams {
            step: self.step().to_owned(),
            title: headline(&self.title()),
            note: None,
            error: self.problem.as_deref().map(text::sanitize_line),
            busy: None,
            fields: vec![
                typed(WHEN, "When:", &self.input, true, Some(&help)),
                FormField::Choice {
                    id: OFFERED.to_owned(),
                    label: "Preset:".to_owned(),
                    value: chosen,
                    options,
                    focus: false,
                    help: None,
                },
            ],
            buttons: vec![cancel(), button(CONFIRM, go, FormRole::Default)],
        })
    }
}

impl UnsubscribePrompt {
    /// The question as a host shows it. What saying yes does is its title,
    /// in full; what it does not do is said under that; then who the list
    /// is and the one address anything goes to. The button that says yes
    /// is named for what it does.
    pub fn form(&self) -> Option<FormShow> {
        let says = self.says();
        FormShow::new(FormShowParams {
            step: UNSUBSCRIBE.to_owned(),
            title: text::sanitize_line(&says.does),
            note: paragraphs(&[super::MAILING_LIST, says.does_not]),
            error: None,
            busy: None,
            fields: vec![
                shown("from", "From:", text::sanitize_line(&self.sender)),
                shown("address", says.label, text::sanitize_line(says.shown)),
            ],
            buttons: vec![
                cancel(),
                button(CONFIRM, &headline(says.yes), FormRole::Default),
            ],
        })
    }
}

impl State {
    /// The form or question the host is to show in its sheet, when the
    /// host is the one that shows them ([`Env::sheets`]): the one a
    /// terminal would have in front. `None`: the host shows no form.
    ///
    /// The order is [`State::context`]'s, so an earlier question wins: a
    /// certificate comes before a sign-in, that before a time, that before
    /// unsubscribing, and all of them before the form to add an account,
    /// which comes back when they are answered. The help is no sheet, and
    /// while it is open over a question the host's sheet is put away.
    pub fn form(&self, env: &Env) -> Option<FormShow> {
        if !env.sheets {
            return None;
        }
        match self.context() {
            Context::Certificate => self.certificates.front()?.form(&env.tz),
            Context::SignIn => self.sign_in.as_ref()?.form(),
            Context::When => self.when.as_ref()?.form(),
            Context::Unsubscribe => self.unsubscribe.as_ref()?.form(),
            Context::Setup => self.setup.as_ref()?.form(&env.tz),
            _ => None,
        }
    }

    /// Whether a form or a question is in front and is the host's to show,
    /// in a sheet, and not drawn here.
    pub fn form_in_sheet(&self) -> bool {
        self.form_hosted && asked_in_a_sheet(self.context())
    }

    /// The user did something with the form the host is showing.
    ///
    /// Only the form in front is answered, and only at the step it is at:
    /// an answer given to a form the host has since been told to put away,
    /// or to change for another, is dropped.
    pub(super) fn on_form(&mut self, env: &Env, used: &FormUsed, out: &mut Effects) {
        let Some(on_show) = self.form(env) else {
            return;
        };
        if on_show.step() != used.step() {
            return;
        }
        // As a key pressed at the question does: what the last key did
        // has been seen.
        self.flash = None;
        match self.context() {
            Context::Setup => {
                let Some(setup) = &mut self.setup else {
                    return;
                };
                let Used { requests, close } = setup.used(used);
                out.accounts.extend(requests);
                if close {
                    self.setup = None;
                }
            }
            Context::Certificate => self.answer_certificate_in_sheet(used, out),
            // Waiting on the browser: only giving up is on offer.
            Context::SignIn => {
                if used.button() == Some(Press::Cancel.id()) {
                    self.give_up_sign_in(out);
                }
            }
            Context::When => self.answer_when_in_sheet(env, used, out),
            Context::Unsubscribe => match used.button() {
                // As `enter`: the one thing the question described.
                Some(CONFIRM) => self.answer_unsubscribe(env, out),
                // As `esc`: nothing was asked of the list, written or
                // opened.
                Some(id) if id == Press::Cancel.id() => self.unsubscribe = None,
                _ => {}
            },
            _ => {}
        }
    }

    /// Takes a button as the answer to the certificate in front: what
    /// `r`, `o` and `p` are in a terminal.
    fn answer_certificate_in_sheet(&mut self, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = self.certificates.front_mut() else {
            return;
        };
        let pin = match used.button().and_then(Press::named) {
            // The button `esc` presses, and `esc` rejects whether or not
            // an answer is being acted on: nothing more is sent to that
            // server.
            Some(Press::Reject) => {
                self.certificates.pop_front();
                return;
            }
            Some(Press::TrustOnce) => false,
            Some(Press::TrustAlways) => true,
            _ => return,
        };
        if prompt.busy {
            return;
        }
        // A certificate with no fingerprint cannot be held to anything.
        if let Some(fingerprint) = prompt.problem.fingerprint.clone() {
            prompt.busy = true;
            out.accounts.push(AccountRequest::Trust {
                account: prompt.account.clone(),
                fingerprint,
                pin,
                sending: prompt.sending,
            });
        }
    }

    /// Takes what was done with the question that asks for a time. What
    /// the line held goes on the question's own line, as typing it does;
    /// then the button does what its key does. With no button, a
    /// ready-made time was chosen, and it is typed on the line.
    fn answer_when_in_sheet(&mut self, env: &Env, used: &FormUsed, out: &mut Effects) {
        let Some(when) = &mut self.when else {
            return;
        };
        match used.button() {
            // As `esc`: nothing was snoozed or sent.
            Some(id) if id == Press::Cancel.id() => self.when = None,
            // As `enter`, with what the sheet's line held.
            Some(CONFIRM) => {
                put(&mut when.input, used.value(WHEN));
                self.answer_when(env, out);
            }
            Some(_) => {}
            None => {
                put(&mut when.input, used.value(WHEN));
                if let Some(typed) = used.value(OFFERED).and_then(offered_named) {
                    put(&mut when.input, Some(typed));
                }
                // As typing does: what was wrong with the last time is no
                // longer said of this one.
                when.problem = None;
            }
        }
    }

    /// Whether the terminal's own way of answering the question in front
    /// is off, because the host has it in a sheet: see
    /// [`State::is_enabled`].
    pub(super) fn answered_in_sheet(&self, env: &Env, id: CommandId) -> bool {
        env.sheets
            && asked_in_a_sheet(self.context())
            && matches!(
                id,
                CommandId::NavOpen
                    | CommandId::NavNext
                    | CommandId::NavPrev
                    | CommandId::FocusNext
                    | CommandId::FocusPrev
            )
    }
}
