//! The host's settings window, as the state holds it.
//!
//! Quill's app has a window for the settings; a terminal has the file.
//! While the window is open the state holds what the settings file was
//! last read as saying, and [`State::settings_shown`] describes the window
//! from that ([`crate::settings::describe`]). What the user does there
//! arrives as an [`Action::Settings`](super::Action::Settings): a change
//! is checked against what was described and sent to the accounts service
//! to be written, and the service's answer, the file read afresh, is what
//! the window shows next. So the window never shows a value the file does
//! not have.
//!
//! The settings that hold at once are handed to the run loop to put into
//! [`Env`] ([`Effects::settings`]), every time the file is read: opening
//! the window brings Quill in line with the file as it stands, whoever
//! wrote it last.

use quill_host::bridge::{BUTTON_PRESS, SettingsDid, SettingsShow};

use super::{Effects, Env, HostRequest, Notice, SettingsAct, State};
use crate::command::CommandId;
use crate::settings::{self, Applied, Can, Change, SettingsRead};
use crate::setup::{AccountRequest, SignIn, SignInStage};
use crate::text;

/// The settings window, while it is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsWindow {
    /// What the settings file was last read as saying, or why it could
    /// not be read for what it says. `None` until the first answer.
    read: Option<Result<Box<SettingsRead>, String>>,
    /// Why the last change was not made.
    refused: Option<String>,
    /// How many times the host has been owed the settings again: once
    /// for every answer from the service and for everything done in the
    /// window that the service is not asked about. The host is told the
    /// settings whenever this moves, changed or not, so that a control the
    /// user moved to a value that was refused is put back.
    answers: u64,
}

impl State {
    /// What the host's settings window is to show, while it is open and
    /// the settings file has been read: every pane with its settings as
    /// the file has them, or what is wrong with the file. With it, a
    /// count that moves whenever the host is owed the settings again.
    pub fn settings_shown(&self, env: &Env) -> Option<(u64, SettingsShow)> {
        let window = self.settings.as_ref()?;
        let shown = match window.read.as_ref()? {
            Ok(read) => settings::describe(
                read,
                env,
                Can {
                    accounts: env.accounts,
                    add: self.is_enabled(env, CommandId::AccountAdd),
                    sign_in: self.is_enabled(env, CommandId::AccountSignIn)
                        && self.sign_in.is_none(),
                    remove: self.can_remove_account(env),
                },
                window.refused.as_deref(),
            ),
            Err(problem) => settings::describe_unreadable(problem),
        }?;
        Some((window.answers, shown))
    }

    /// The user opened, closed or did something in the settings window.
    pub(super) fn on_settings(&mut self, env: &Env, act: SettingsAct, out: &mut Effects) {
        match act {
            // Opened, or come back to: the file is read afresh, since it
            // may have been edited by hand meanwhile. What was read before
            // stays on show until the answer.
            SettingsAct::Opened => {
                self.settings.get_or_insert_default().refused = None;
                out.accounts.push(AccountRequest::Settings(None));
            }
            SettingsAct::Closed => self.settings = None,
            SettingsAct::Used(used) => {
                // Only what is on show can be used.
                let Some((_, described)) = self.settings_shown(env) else {
                    return;
                };
                let Some(Ok(read)) = self
                    .settings
                    .as_ref()
                    .and_then(|window| window.read.clone())
                else {
                    // The file has a mistake in it: there is nothing here
                    // to change.
                    self.settings_answered(None);
                    return;
                };
                match used.did() {
                    SettingsDid::Set(value) => {
                        match settings::change(&described, &read, used.pane(), used.field(), value)
                        {
                            // Answered when the service has written it.
                            Ok(change) => {
                                out.accounts.push(AccountRequest::Settings(Some(change)));
                            }
                            Err(reason) => self.settings_answered(Some(reason)),
                        }
                    }
                    // The button that imports a theme: the host has the
                    // chooser, and says what was chosen when something is.
                    SettingsDid::Action { action, row: None }
                        if used.pane() == settings::Pane::General.id()
                            && used.field() == settings::IMPORT_THEME
                            && action == BUTTON_PRESS =>
                    {
                        out.host.push(HostRequest::PickTheme);
                        self.settings_answered(None);
                    }
                    SettingsDid::Action { action, row } => {
                        let known = used.pane() == settings::Pane::Accounts.id()
                            && used.field() == settings::ACCOUNTS;
                        let account = row.as_deref().and_then(|row| {
                            read.accounts
                                .iter()
                                .find(|account| settings::account_id(&account.name) == row)
                        });
                        match (known, action.as_str(), account) {
                            // As the command does, from the menu or by its
                            // key: the form opens, in the host's sheet.
                            (true, settings::ADD, _) => self.run(env, CommandId::AccountAdd, out),
                            // As the command does, for the account picked
                            // in the window and no other.
                            (true, settings::SIGN_IN, Some(account))
                                if account.signs_in
                                    && self.sign_in.is_none()
                                    && self.is_enabled(env, CommandId::AccountSignIn) =>
                            {
                                out.accounts.push(AccountRequest::SignIn {
                                    account: Some(account.name.clone()),
                                });
                                self.sign_in = Some(SignIn {
                                    account: String::new(),
                                    stage: SignInStage::Starting,
                                    fresh: false,
                                });
                            }
                            // As the command does, for the account picked
                            // in the window: the question comes first.
                            (true, settings::REMOVE, Some(account))
                                if self.can_remove_account(env) =>
                            {
                                let name = account.name.clone();
                                self.ask_remove(env, &name, out);
                            }
                            _ => {}
                        }
                        self.settings_answered(None);
                    }
                }
            }
        }
    }

    /// The host has put a theme in the user's `themes` folder, chosen in
    /// its chooser: it becomes the theme in force, so that what was chosen
    /// is what is seen. For a settings file with one theme for both
    /// appearances that is the one theme; for one that has them apart, the
    /// dark one while Quill is dark and the light one while it is light.
    /// The service knows which the file is. It also checks that the file
    /// copied in is a theme before the settings are made to name it, and
    /// reads the folder again either way.
    pub(super) fn on_theme_imported(&mut self, env: &Env, name: String, out: &mut Effects) {
        let change = Change::ThemeInForce {
            name,
            dark: env.look.is_dark(),
        };
        out.accounts.push(AccountRequest::Settings(Some(change)));
    }

    /// The host is owed the settings again, with why the last change was
    /// not made if it was not.
    pub(super) fn settings_answered(&mut self, refused: Option<String>) {
        if let Some(window) = &mut self.settings {
            window.refused = refused;
            window.answers += 1;
        }
    }

    /// The service has read the settings file, having made the change it
    /// was asked for if it could.
    ///
    /// Whatever the file says of the settings that hold at once holds from
    /// now, and the run loop is handed them for [`Env`]. `env` here is
    /// still as it was, so what changes is seen by comparing the two.
    pub(super) fn on_settings_read(
        &mut self,
        env: &Env,
        read: Result<Box<SettingsRead>, String>,
        changed: Option<(Change, Result<(), String>)>,
        out: &mut Effects,
    ) {
        // An answer that comes after the window was closed is still acted
        // on if it made a change: the file was written.
        let made = matches!(changed, Some((_, Ok(()))));
        if self.settings.is_none() && !made {
            // With no window to say it in, a change that was not made is
            // said where everything else is: a theme imported from the
            // menu that turned out not to be one.
            if let Some((_, Err(reason))) = &changed {
                self.notices.push_back(Notice::error(format!(
                    "That was not changed. {}",
                    text::sanitize_line(reason)
                )));
            }
            return;
        }
        let refused = match &changed {
            Some((_, Err(reason))) => Some(format!(
                "That was not changed. {}",
                text::sanitize_line(reason)
            )),
            _ => None,
        };
        if let Ok(read) = &read {
            let asked = changed.as_ref().map(|(change, _)| change);
            self.settings_hold(env, read, asked, out);
        }
        if let Some(window) = &mut self.settings {
            window.read = Some(read);
        }
        self.settings_answered(refused);
    }

    /// Makes the settings that hold at once hold: the state's own part of
    /// it here, and [`Env`]'s handed to the run loop.
    fn settings_hold(
        &mut self,
        env: &Env,
        read: &SettingsRead,
        asked: Option<&Change>,
        out: &mut Effects,
    ) {
        let now = &read.settings;
        // Conversations. What was switched by hand for this run gives way
        // when the setting itself is changed, in the window or in the file:
        // the list is then as the setting says.
        if now.threads != env.threads || matches!(asked, Some(Change::Threads(_))) {
            let grouped = env.threads != self.ungrouped;
            self.ungrouped = false;
            if grouped != now.threads {
                self.reload(env, out);
            }
        }
        // Web reading. The file always has what was last switched to (the
        // command writes it), so it is what the file says, and no longer
        // that turned round.
        let reading = self.web_reading;
        self.web_flipped = false;
        self.web_reading = env.web_pane && now.web_reading;
        if self.web_reading != reading {
            self.web_reading_switched(env, out);
        }
        // Spelling. Switched off, the marks go; switched on, what is being
        // written is checked from scratch.
        if now.spell_check != env.compose.spell_check {
            self.spell.sent = None;
            if !now.spell_check
                && let Some(compose) = &mut self.compose
            {
                compose.body.set_misspellings(Vec::new());
            }
        }
        // Another look: everything is drawn again.
        if now.appearance != env.look.appearance
            || read.theme_light != env.look.light
            || read.theme_dark != env.look.dark
        {
            self.dirty = true;
        }
        // Other keys: half a sequence typed in the old ones means nothing
        // in the new.
        if read.keymap != env.keymap {
            self.pending.clear();
        }
        out.settings = Some(Box::new(Applied::of(read)));
    }
}
