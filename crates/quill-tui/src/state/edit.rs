//! Changing an account that is there already.
//!
//! The form that adds an account is also the form that changes one: its
//! servers, the name it signs in with and its password. The accounts
//! service is asked how the account is set up now
//! ([`AccountRequest::EditAsk`]), which changes nothing, and the form
//! opens with that in its fields ([`Setup::editing`]). From there it is
//! the form it always was: the settings are tried before they are kept,
//! and the service writes them down ([`AccountRequest::Change`]). An
//! account the form cannot describe (one that signs in with its provider,
//! one whose mail is fetched another way) is said so by the service, and
//! no form opens.

use super::{Effects, Env, Notice, State};
use crate::data::DataRequest;
use crate::setup::{AccountDraft, AccountRequest, Setup};
use crate::text;

impl State {
    /// Whether an account can be asked to be changed now. Not in the
    /// middle of anything that has an account in hand.
    pub fn can_edit_account(&self, env: &Env) -> bool {
        self.can_remove_account(env) && !self.edit_asked
    }

    /// Has the service say how `account` is set up. The form follows its
    /// answer.
    pub(super) fn ask_edit(&mut self, env: &Env, account: &str, out: &mut Effects) {
        if !self.can_edit_account(env) {
            return;
        }
        self.edit_asked = true;
        out.accounts.push(AccountRequest::EditAsk {
            account: account.to_owned(),
        });
    }

    /// The service has said how the account is set up: the form opens
    /// with it, or why there is no form for this account is said.
    pub(super) fn on_edit_asked(
        &mut self,
        account: String,
        found: Result<Box<AccountDraft>, String>,
    ) {
        if !std::mem::take(&mut self.edit_asked) {
            return;
        }
        // Something else has taken the screen while the service looked.
        if self.setup.is_some() || self.compose.is_some() || self.remove.is_some() {
            return;
        }
        match found {
            Ok(draft) => {
                self.setup = Some(Setup::editing(account, *draft));
                self.settings_answered(None);
            }
            Err(reason) => {
                let said = format!(
                    "{} cannot be changed here. {}",
                    text::sanitize_line(&account),
                    text::sanitize_line(&reason)
                );
                self.notices.push_back(Notice::error(said.clone()));
                self.settings_answered(Some(said));
            }
        }
    }

    /// The changes to an account were saved, and it is syncing afresh.
    pub(super) fn on_account_changed(&mut self, name: &str, out: &mut Effects) {
        self.notices.push_back(Notice::info(format!(
            "Saved the changes to {}.",
            text::sanitize_line(name)
        )));
        out.data.push(DataRequest::LoadSidebar);
        out.data.push(DataRequest::LoadMessages(self.mailbox));
        // The settings window lists the accounts as the file has them.
        if self.settings.is_some() {
            out.accounts.push(AccountRequest::Settings(None));
        }
    }
}
