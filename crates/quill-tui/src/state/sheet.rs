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

use super::{
    EVERY_ACCOUNT, Effects, Env, FolderAsk, FolderPrompt, INSIDE_ANOTHER, LetterPrompt,
    ON_THE_SERVER, ON_THIS_MAC, RemovePrompt, RuleAction, RuleField, RulePrompt, State,
    UnsubscribePrompt, WhenFor, WhenPrompt,
};
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
/// The step of the question about removing an account.
const REMOVE_ACCOUNT: &str = "remove_account";
/// The step of the question about an account's letter, and its field.
const ACCOUNT_LETTER: &str = "account_letter";
const LETTER: &str = "letter";
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
            | Context::NewFolder
            | Context::AccountLetter
            | Context::NewRule
            | Context::RecordKeys
            | Context::EditTheme
            | Context::Unsubscribe
            | Context::RemoveAccount
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

/// The step of the question about a new folder, and its fields.
const NEW_FOLDER: &str = "new_folder";
const RENAME_FOLDER: &str = "rename_folder";
const DELETE_FOLDER: &str = "delete_folder";
const FOLDER_NAME: &str = "name";
const FOLDER_PLACE: &str = "place";
const PLACE_SERVER: &str = "server";
const PLACE_HERE: &str = "here";

impl FolderPrompt {
    /// The question as a host shows it: the name to type, where the
    /// folder is to be, and what that means for the mail put in it. With
    /// no choice of place there is none to make, and the place is said.
    ///
    /// Before a folder is deleted there is nothing to type: what is asked
    /// is the title, what becomes of the folder and its mail is said
    /// under it, and no button is the one `return` presses.
    pub fn form(&self) -> Option<FormShow> {
        let step = match self.ask {
            FolderAsk::New => NEW_FOLDER,
            FolderAsk::Rename { .. } => RENAME_FOLDER,
            FolderAsk::Delete { .. } => DELETE_FOLDER,
        };
        if !self.typed() {
            return FormShow::new(FormShowParams {
                step: step.to_owned(),
                title: self.title(),
                note: paragraphs(&[self.means()]),
                error: None,
                busy: None,
                fields: Vec::new(),
                buttons: vec![
                    Press::Cancel.button(Some(FormRole::Cancel), true),
                    FormButton {
                        id: CONFIRM.to_owned(),
                        title: self.answer().to_owned(),
                        role: None,
                        enabled: true,
                    },
                ],
            });
        }
        let help = format!("{} {INSIDE_ANOTHER}", self.means());
        let mut fields = vec![typed(FOLDER_NAME, "Name:", &self.input, true, None)];
        if self.either && self.ask == FolderAsk::New {
            fields.push(FormField::Choice {
                id: FOLDER_PLACE.to_owned(),
                label: "Where:".to_owned(),
                value: if self.here { PLACE_HERE } else { PLACE_SERVER }.to_owned(),
                options: vec![
                    FormOption {
                        id: PLACE_SERVER.to_owned(),
                        title: ON_THE_SERVER.to_owned(),
                    },
                    FormOption {
                        id: PLACE_HERE.to_owned(),
                        title: ON_THIS_MAC.to_owned(),
                    },
                ],
                focus: false,
                help: Some(help),
            });
        } else {
            let mut place = shown(FOLDER_PLACE, "Where:", self.place().to_owned());
            if let FormField::Static { help: under, .. } = &mut place {
                *under = Some(help);
            }
            fields.push(place);
        }
        FormShow::new(FormShowParams {
            step: step.to_owned(),
            title: headline(&self.title()),
            note: None,
            error: self.problem.as_deref().map(text::sanitize_line),
            busy: None,
            fields,
            buttons: vec![
                Press::Cancel.button(Some(FormRole::Cancel), true),
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: self.answer().to_owned(),
                    role: Some(FormRole::Default),
                    enabled: true,
                },
            ],
        })
    }

    /// Puts what the host's fields held into the question's own.
    fn fill(&mut self, used: &FormUsed) {
        if !self.typed() {
            return;
        }
        put(&mut self.input, used.value(FOLDER_NAME));
        if self.either && self.ask == FolderAsk::New {
            match used.value(FOLDER_PLACE) {
                Some(PLACE_HERE) => self.here = true,
                Some(PLACE_SERVER) => self.here = false,
                _ => {}
            }
        }
    }
}

/// The step of the form for a new rule, and its fields.
const NEW_RULE: &str = "new_rule";
const RULE_NAME: &str = "name";
const RULE_ACCOUNT: &str = "account";
const RULE_FROM: &str = "from";
const RULE_TO: &str = "to";
const RULE_SUBJECT: &str = "subject";
const RULE_TEXT: &str = "text";
const RULE_DOES: &str = "does";
const RULE_FOLDER: &str = "folder";
/// The choice of every account, and what the name of an account's or a
/// folder's choice starts with: its place in the list follows.
const EVERY: &str = "all";
const ACCOUNT_AT: &str = "a";
const FOLDER_AT: &str = "f";

/// The place in a list that a choice's name gives: `f3` is the fourth.
fn place(id: &str, starts: &str) -> Option<usize> {
    id.strip_prefix(starts)?.parse().ok()
}

impl RulePrompt {
    /// The form as a host shows it: the rule's name, whose mail it is
    /// for, what the mail has, what to do with it, and the folder when
    /// it moves mail. A rule about one account chooses among that
    /// account's folders; about every account, the folder is typed.
    pub fn form(&self) -> Option<FormShow> {
        let on = |field| self.focus == field;
        let mut fields = vec![typed(
            RULE_NAME,
            "Name:",
            &self.name,
            on(RuleField::Name),
            None,
        )];
        if self.accounts.len() > 1 {
            let mut options = vec![FormOption {
                id: EVERY.to_owned(),
                title: EVERY_ACCOUNT.to_owned(),
            }];
            options.extend(
                self.accounts
                    .iter()
                    .enumerate()
                    .map(|(index, account)| FormOption {
                        id: format!("{ACCOUNT_AT}{index}"),
                        title: text::sanitize_line(&account.name),
                    }),
            );
            fields.push(FormField::Choice {
                id: RULE_ACCOUNT.to_owned(),
                label: "For Mail In:".to_owned(),
                value: self
                    .account
                    .map_or_else(|| EVERY.to_owned(), |index| format!("{ACCOUNT_AT}{index}")),
                options,
                focus: on(RuleField::Account),
                help: None,
            });
        }
        fields.extend([
            typed(
                RULE_FROM,
                "From Contains:",
                &self.from,
                on(RuleField::From),
                None,
            ),
            typed(RULE_TO, "To Contains:", &self.to, on(RuleField::To), None),
            typed(
                RULE_SUBJECT,
                "Subject Contains:",
                &self.subject,
                on(RuleField::Subject),
                None,
            ),
            typed(
                RULE_TEXT,
                "Text Contains:",
                &self.text,
                on(RuleField::Text),
                Some(
                    "Fill in any of these and leave the rest empty. The rule is for mail \
                     that has all of what is filled in.",
                ),
            ),
            FormField::Choice {
                id: RULE_DOES.to_owned(),
                label: "Do:".to_owned(),
                value: self.action.id().to_owned(),
                options: RuleAction::ALL
                    .into_iter()
                    .map(|action| FormOption {
                        id: action.id().to_owned(),
                        title: action.words().to_owned(),
                    })
                    .collect(),
                focus: on(RuleField::Action),
                help: None,
            },
        ]);
        if self.action == RuleAction::Move {
            fields.push(match self.folder_choices() {
                Some(folders) => {
                    // The folder the form has, or the one an account's
                    // mail is usually filed in: a choice always shows
                    // one.
                    let account = self.chosen();
                    let chosen = account
                        .and_then(|account| account.folder(self.folder.text()))
                        .or_else(|| account.and_then(|account| account.usual()))
                        .and_then(|name| folders.iter().position(|folder| *folder == name))
                        .unwrap_or(0);
                    FormField::Choice {
                        id: RULE_FOLDER.to_owned(),
                        label: "Folder:".to_owned(),
                        value: format!("{FOLDER_AT}{chosen}"),
                        options: folders
                            .iter()
                            .enumerate()
                            .map(|(index, folder)| FormOption {
                                id: format!("{FOLDER_AT}{index}"),
                                title: text::sanitize_line(folder),
                            })
                            .collect(),
                        focus: on(RuleField::Folder),
                        help: None,
                    }
                }
                None => typed(
                    RULE_FOLDER,
                    "Folder:",
                    &self.folder,
                    on(RuleField::Folder),
                    Some(
                        "A folder every account has: its name, or archive. Choose an account \
                         above to pick from its folders.",
                    ),
                ),
            });
        }
        FormShow::new(FormShowParams {
            step: NEW_RULE.to_owned(),
            title: "New Rule".to_owned(),
            note: Some(
                "Saving shows what the rule would match. Nothing is changed until you run it."
                    .to_owned(),
            ),
            error: self.problem.as_deref().map(text::sanitize_line),
            busy: self.busy.then(|| "Saving\u{2026}".to_owned()),
            fields,
            buttons: vec![
                Press::Cancel.button(Some(FormRole::Cancel), true),
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: "Save".to_owned(),
                    role: Some(FormRole::Default),
                    enabled: !self.busy,
                },
            ],
        })
    }

    /// Puts what the host's fields held into the form's own.
    fn fill(&mut self, used: &FormUsed) {
        put(&mut self.name, used.value(RULE_NAME));
        put(&mut self.from, used.value(RULE_FROM));
        put(&mut self.to, used.value(RULE_TO));
        put(&mut self.subject, used.value(RULE_SUBJECT));
        put(&mut self.text, used.value(RULE_TEXT));
        // The folder first, read against the list it was chosen from:
        // the account's as it was shown, before any change to it here.
        if let Some(value) = used.value(RULE_FOLDER) {
            match self.folder_choices() {
                Some(folders) => {
                    if let Some(folder) = place(value, FOLDER_AT).and_then(|at| folders.get(at)) {
                        let folder = (*folder).to_owned();
                        self.folder.set(folder);
                    }
                }
                None => put(&mut self.folder, Some(value)),
            }
        }
        match used.value(RULE_ACCOUNT) {
            Some(EVERY) => self.account = None,
            Some(value) => {
                if let Some(at) = place(value, ACCOUNT_AT).filter(|at| *at < self.accounts.len()) {
                    self.account = Some(at);
                }
            }
            None => {}
        }
        if let Some(action) = used.value(RULE_DOES).and_then(RuleAction::named) {
            self.set_action(action);
        }
        // A choice of folders always shows one of them: the form holds
        // the one shown, so that what is saved is what was seen.
        if self.action == RuleAction::Move
            && let Some(folders) = self.folder_choices()
        {
            let account = self.chosen();
            let held = account
                .and_then(|account| account.folder(self.folder.text()))
                .filter(|name| folders.contains(name));
            let folder = held
                .or_else(|| account.and_then(|account| account.usual()))
                .or(folders.first().copied())
                .map(str::to_owned);
            if let Some(folder) = folder {
                self.folder.set(folder);
            }
        }
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

impl LetterPrompt {
    /// The question as a host shows it: the letter to type, and under it
    /// what the letter is for.
    pub fn form(&self) -> Option<FormShow> {
        FormShow::new(FormShowParams {
            step: ACCOUNT_LETTER.to_owned(),
            // As it is: the account's name is the user's, and is not
            // given capitals it does not have.
            title: self.title(),
            note: None,
            error: self.problem.as_deref().map(text::sanitize_line),
            busy: None,
            fields: vec![typed(
                LETTER,
                "Letter:",
                &self.input,
                true,
                Some(&self.says()),
            )],
            buttons: vec![
                Press::Cancel.button(Some(FormRole::Cancel), true),
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: "Use It".to_owned(),
                    role: Some(FormRole::Default),
                    enabled: true,
                },
            ],
        })
    }
}

impl RemovePrompt {
    /// The question as a host shows it: what is asked is its title, and
    /// under it what saying yes does, what it leaves alone, and each thing
    /// that goes for good. No button is the one `return` presses: removing
    /// an account is not to be done by a stray key.
    pub fn form(&self) -> Option<FormShow> {
        let mut said = vec![self.does(), self.keeps().to_owned()];
        said.extend(self.losses());
        let said: Vec<&str> = said.iter().map(String::as_str).collect();
        FormShow::new(FormShowParams {
            step: REMOVE_ACCOUNT.to_owned(),
            title: self.title(),
            note: paragraphs(&said),
            error: None,
            busy: self.busy.then(|| super::REMOVING.to_owned()),
            fields: Vec::new(),
            buttons: vec![
                Press::Cancel.button(Some(FormRole::Cancel), !self.busy),
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: "Remove".to_owned(),
                    role: None,
                    enabled: !self.busy,
                },
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
            Context::NewFolder => self.new_folder.as_ref()?.form(),
            Context::AccountLetter => self.letter_prompt.as_ref()?.form(),
            Context::NewRule => self.new_rule.as_ref()?.form(),
            Context::RecordKeys => self.keys.as_ref()?.form(env),
            Context::EditTheme => self.theme_prompt.as_ref()?.form(),
            Context::Unsubscribe => self.unsubscribe.as_ref()?.form(),
            Context::RemoveAccount => self.remove.as_ref()?.form(),
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
            Context::NewFolder => self.answer_new_folder_in_sheet(env, used, out),
            Context::AccountLetter => self.answer_letter_in_sheet(used, out),
            Context::NewRule => self.answer_new_rule_in_sheet(used, out),
            Context::RecordKeys => self.answer_keys_in_sheet(env, used, out),
            Context::EditTheme => self.answer_theme_in_sheet(env, used, out),
            Context::Unsubscribe => match used.button() {
                // As `enter`: the one thing the question described.
                Some(CONFIRM) => self.answer_unsubscribe(env, out),
                // As `esc`: nothing was asked of the list, written or
                // opened.
                Some(id) if id == Press::Cancel.id() => self.unsubscribe = None,
                _ => {}
            },
            Context::RemoveAccount => match used.button() {
                Some(CONFIRM) => self.answer_remove(out),
                Some(id) if id == Press::Cancel.id() => self.cancel_remove(),
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

    /// Takes what was done with the question about an account's letter:
    /// what its field held goes into the question's own, and then the
    /// button does what its key does.
    fn answer_letter_in_sheet(&mut self, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = &mut self.letter_prompt else {
            return;
        };
        match used.button() {
            // As `esc`: the letter is as it was.
            Some(id) if id == Press::Cancel.id() => self.letter_prompt = None,
            Some(CONFIRM) => {
                put(&mut prompt.input, used.value(LETTER));
                self.answer_letter(out);
            }
            Some(_) => {}
            None => {
                put(&mut prompt.input, used.value(LETTER));
                prompt.problem = None;
            }
        }
    }

    /// Takes what was done with the question about a new folder: what its
    /// fields held goes into the question's own, and then the button does
    /// what its key does. With no button, the place was changed, and what
    /// is said of it changes with it.
    fn answer_new_folder_in_sheet(&mut self, env: &Env, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = &mut self.new_folder else {
            return;
        };
        match used.button() {
            // As `esc`: no folder was made.
            Some(id) if id == Press::Cancel.id() => self.new_folder = None,
            Some(CONFIRM) => {
                prompt.fill(used);
                self.answer_new_folder(env, out);
            }
            Some(_) => {}
            None => {
                prompt.fill(used);
                prompt.problem = None;
            }
        }
    }

    /// Takes what was done with the form for a new rule: what its fields
    /// held goes into the form's own, and then the button does what its
    /// key does. With no button, what the rule does was changed, and the
    /// fields change with it.
    fn answer_new_rule_in_sheet(&mut self, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = &mut self.new_rule else {
            return;
        };
        match used.button() {
            // As `esc`: no rule was written.
            Some(id) if id == Press::Cancel.id() => self.new_rule = None,
            Some(CONFIRM) => {
                prompt.fill(used);
                self.answer_new_rule(out);
            }
            Some(_) => {}
            None => {
                prompt.fill(used);
                prompt.problem = None;
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
