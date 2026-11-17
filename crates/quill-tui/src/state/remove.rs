//! Removing an account.
//!
//! An account is removed in two steps, and the first changes nothing. The
//! accounts service is asked what would go with the account
//! ([`AccountRequest::RemoveAsk`]): besides the copy of its mail, which
//! can be fetched again, there may be messages being written, messages
//! waiting to be sent and changes its server has not heard of, which
//! cannot. The user is then asked, with all of that said
//! ([`RemovePrompt`]), in a sheet of the host's or over the panes. Only a
//! yes sends [`AccountRequest::Remove`], and the service does the whole of
//! it: the settings file, the Keychain, the engine and the store. Nothing
//! is removed here; the UI lets go of the account when it hears it is gone.

use quill_store::Mailbox;

use super::{Effects, Env, Notice, SidebarEntry, State};
use crate::data::DataRequest;
use crate::keymap::{Key, KeyChord, Mods};
use crate::setup::{AccountRequest, Leaving};
use crate::text;

/// The question asked before an account is removed: which, and what goes
/// with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePrompt {
    /// The account's name, as the settings file has it.
    pub account: String,
    pub leaving: Leaving,
    /// The user said yes, and the service is at it: there is nothing more
    /// to answer, and no going back.
    pub busy: bool,
}

/// The letter that says yes where the question is drawn over the panes.
/// Not `enter`: that is too easily pressed for something that cannot be
/// undone.
pub const YES: char = 'y';

/// What is said while the service removes the account.
pub const REMOVING: &str = "Removing\u{2026}";

fn counted(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

impl RemovePrompt {
    /// What the question is called: the whole of what is asked.
    pub fn title(&self) -> String {
        format!("Remove {}?", text::sanitize_line(&self.account))
    }

    /// What saying yes does, and what it leaves alone.
    pub fn does(&self) -> String {
        let address = text::sanitize_line(&self.leaving.address);
        let whose = if address.is_empty() {
            "The account".to_owned()
        } else {
            format!("The account for {address}")
        };
        format!(
            "{whose} is taken out of Quill: its settings out of the settings file, its \
             password or sign-in out of the Keychain, and the copy of its mail off this Mac."
        )
    }

    /// What is said of the mail itself.
    pub fn keeps(&self) -> &'static str {
        if self.leaving.only_copy {
            "This account's mail is taken off its server as it is fetched (POP3), so the \
             copy on this Mac may be the only one. Mail that is no longer on the server \
             will be gone for good."
        } else {
            "Mail on its server is not touched, and the account can be added again."
        }
    }

    /// What exists only here and goes with the account, each in a line of
    /// its own. Empty when nothing does.
    pub fn losses(&self) -> Vec<String> {
        let leaving = &self.leaving;
        let mut losses = Vec::new();
        if leaving.outbox > 0 {
            losses.push(format!(
                "{} waiting in the Outbox will not be sent.",
                counted(leaving.outbox, "message", "messages")
            ));
        }
        if leaving.drafts > 0 {
            losses.push(format!(
                "{} will be deleted.",
                counted(leaving.drafts, "draft", "drafts")
            ));
        }
        if leaving.queued > 0 {
            losses.push(format!(
                "{} made here {} not reached the server yet and will be lost.",
                counted(leaving.queued, "change", "changes"),
                if leaving.queued == 1 { "has" } else { "have" }
            ));
        }
        losses
    }

    /// Whether anything goes that cannot be had back.
    pub fn loses_something(&self) -> bool {
        self.leaving.only_copy || !self.losses().is_empty()
    }
}

impl State {
    /// Whether an account can be asked to be removed now. Not in the
    /// middle of anything that has an account in hand: a message being
    /// written, a sign-in, the form that adds one, or another removal.
    pub fn can_remove_account(&self, env: &Env) -> bool {
        env.accounts
            && self.compose.is_none()
            && self.sign_in.is_none()
            && self.setup.is_none()
            && self.remove.is_none()
            && !self.remove_asked
    }

    /// Has the service say what removing `account` would lose. The
    /// question follows its answer.
    pub(super) fn ask_remove(&mut self, env: &Env, account: &str, out: &mut Effects) {
        if !self.can_remove_account(env) {
            return;
        }
        self.remove_asked = true;
        out.accounts.push(AccountRequest::RemoveAsk {
            account: account.to_owned(),
        });
    }

    /// The service has said what would go with the account: ask.
    pub(super) fn on_remove_asked(&mut self, account: String, found: Result<Leaving, String>) {
        if !std::mem::take(&mut self.remove_asked) {
            return;
        }
        match found {
            Ok(leaving) => {
                self.remove = Some(RemovePrompt {
                    account,
                    leaving,
                    busy: false,
                });
            }
            Err(reason) => self.remove_refused(&account, &reason),
        }
    }

    /// A key at the question, where it is drawn over the panes: the one
    /// letter that says yes. Returns whether the key was the question's.
    pub(super) fn answer_remove_key(&mut self, chord: KeyChord, out: &mut Effects) -> bool {
        let Key::Char(letter) = chord.key else {
            return false;
        };
        if letter == YES && chord.mods == Mods::default() {
            self.answer_remove(out);
        }
        // Any other letter is no answer, and runs nothing behind it.
        true
    }

    /// The user said yes: the service removes the account.
    pub(super) fn answer_remove(&mut self, out: &mut Effects) {
        let Some(prompt) = &mut self.remove else {
            return;
        };
        if prompt.busy {
            return;
        }
        prompt.busy = true;
        out.accounts.push(AccountRequest::Remove {
            account: prompt.account.clone(),
        });
    }

    /// The user said no. Nothing was changed. Once the service is at it
    /// there is no saying no.
    pub(super) fn cancel_remove(&mut self) {
        if self.remove.as_ref().is_some_and(|prompt| !prompt.busy) {
            self.remove = None;
        }
    }

    /// The service has removed the account, or could not.
    pub(super) fn on_removed(
        &mut self,
        account: String,
        result: Result<Option<String>, String>,
        out: &mut Effects,
    ) {
        if self
            .remove
            .as_ref()
            .is_some_and(|prompt| prompt.account == account)
        {
            self.remove = None;
        }
        let name = text::sanitize_line(&account);
        match result {
            Ok(left) => {
                // Whatever was on show of the account goes with it: its
                // inbox or one of its folders gives way to every inbox.
                let here = match self.mailbox {
                    Mailbox::Account(id) => self
                        .accounts
                        .iter()
                        .any(|row| row.id == id && row.name == account),
                    Mailbox::Folder(folder) => self.sidebar.entries.iter().any(|entry| {
                        matches!(entry, SidebarEntry::Folder { id, account: index, .. }
                            if *id == folder
                                && self.accounts.get(*index).is_some_and(|row| row.name == account))
                    }),
                    _ => false,
                };
                if here {
                    self.mailbox = Mailbox::Unified;
                }
                self.search = None;
                self.list = super::MessageList::default();
                out.data.push(DataRequest::LoadSidebar);
                out.data.push(DataRequest::LoadMessages(self.mailbox));
                self.notices.push_back(Notice::info(match left {
                    None => format!("Removed {name}."),
                    Some(left) => format!("Removed {name}. {}", text::sanitize_line(&left)),
                }));
                // The settings window lists the accounts the file has.
                if self.settings.is_some() {
                    out.accounts.push(AccountRequest::Settings(None));
                }
            }
            Err(reason) => self.remove_refused(&account, &reason),
        }
    }

    /// Says why an account was not removed: where everything else is said,
    /// and in the settings window if that is where it was asked.
    fn remove_refused(&mut self, account: &str, reason: &str) {
        let said = format!(
            "{} was not removed. {}",
            text::sanitize_line(account),
            text::sanitize_line(reason)
        );
        self.notices.push_back(Notice::error(said.clone()));
        self.settings_answered(Some(said));
    }
}
