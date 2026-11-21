//! The letter that stands for an account.
//!
//! Where mail of several accounts is listed together, each message is
//! marked with one letter for its account. That is the first letter of the
//! account's name unless the user chooses another: accounts named by
//! their addresses often start alike. The choice is `letter` in the
//! account's settings, and the question for it ([`LetterPrompt`]) is asked
//! from the accounts pane and from the list of accounts in the host's
//! settings window. The accounts service writes it down
//! ([`AccountRequest::Letter`]); nothing is changed here until it says so.

use super::{Effects, Env, Notice, State};
use crate::data::DataRequest;
use crate::input::LineInput;
use crate::keymap::KeyChord;
use crate::setup::{AccountRequest, typed_letter};
use crate::text;

/// The question: which letter or number is to stand for an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetterPrompt {
    /// The account's name, as the settings file has it.
    pub account: String,
    /// The letter, typed. It starts as the one the account has of its
    /// own, if it has one.
    pub input: LineInput,
    /// What stands for the account with no letter of its own: the first
    /// of its name.
    pub usual: Option<char>,
    /// Why what was typed would not do.
    pub problem: Option<String>,
}

impl LetterPrompt {
    pub fn title(&self) -> String {
        format!("Letter for {}", text::sanitize_line(&self.account))
    }

    /// What the letter is for, and what leaving it empty comes to.
    pub fn says(&self) -> String {
        let usual = match self.usual.filter(|letter| !letter.is_control()) {
            Some(letter) => {
                format!(
                    "Left empty, it is {letter}, the first letter of the account\u{2019}s name."
                )
            }
            None => "Left empty, it is the first letter of the account\u{2019}s name.".to_owned(),
        };
        format!(
            "One letter or number. It marks this account\u{2019}s mail where every \
             account\u{2019}s is listed together. {usual}"
        )
    }

    /// Takes a key as typing. Returns whether the key was the question's.
    pub fn key(&mut self, chord: KeyChord) -> bool {
        if self.input.key(chord) {
            self.problem = None;
            return true;
        }
        false
    }

    /// The letter typed: none, where the field is empty, or the one
    /// letter or number in it. Where it holds anything else, what to say
    /// of it.
    pub fn chosen(&self) -> Result<Option<char>, &'static str> {
        typed_letter(self.input.text())
    }
}

impl State {
    /// Whether an account's letter can be asked for now: where there are
    /// settings to write it in, and not in the middle of anything that
    /// has an account in hand.
    pub fn can_set_letter(&self, env: &Env) -> bool {
        self.can_remove_account(env) && self.letter_prompt.is_none()
    }

    /// Opens the question for the account called `account`, whose own
    /// letter is `own` if it has one.
    pub(super) fn ask_letter(&mut self, env: &Env, account: &str, own: Option<String>) {
        if !self.can_set_letter(env) {
            return;
        }
        self.letter_prompt = Some(LetterPrompt {
            account: account.to_owned(),
            input: LineInput::new(own.unwrap_or_default()),
            usual: account.chars().next(),
            problem: None,
        });
    }

    /// Has the service give the account the letter typed, or says what
    /// is wrong with it.
    pub(super) fn answer_letter(&mut self, out: &mut Effects) {
        let Some(prompt) = &mut self.letter_prompt else {
            return;
        };
        let letter = match prompt.chosen() {
            Ok(letter) => letter,
            Err(problem) => {
                prompt.problem = Some(problem.to_owned());
                return;
            }
        };
        let Some(prompt) = self.letter_prompt.take() else {
            return;
        };
        self.letter_asked = true;
        out.accounts.push(AccountRequest::Letter {
            account: prompt.account,
            letter,
        });
    }

    /// The service has given an account its letter, or could not.
    ///
    /// A letter given as an account was added is not announced: adding
    /// it was, and the letter is seen in the list.
    pub(super) fn on_letter_set(
        &mut self,
        account: &str,
        letter: Option<char>,
        result: Result<(), String>,
        out: &mut Effects,
    ) {
        let asked = std::mem::take(&mut self.letter_asked);
        let name = text::sanitize_line(account);
        match result {
            Ok(()) => {
                // The accounts are read again, with the letter.
                out.data.push(DataRequest::LoadSidebar);
                if asked {
                    self.notices.push_back(Notice::info(match letter {
                        Some(letter) => format!("Mail of {name} is marked {letter} in lists."),
                        None => format!(
                            "Mail of {name} is marked with the first letter of its name in lists."
                        ),
                    }));
                }
                // The settings window lists the accounts as the file has
                // them.
                if self.settings.is_some() {
                    out.accounts.push(AccountRequest::Settings(None));
                }
            }
            Err(reason) => {
                let said = format!(
                    "The letter for {name} was not changed. {}",
                    text::sanitize_line(&reason)
                );
                self.notices.push_back(Notice::error(said.clone()));
                self.settings_answered(Some(said));
            }
        }
    }
}
