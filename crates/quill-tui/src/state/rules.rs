//! Rules, as the user works them from the window.
//!
//! The rules themselves are files, read by the accounts service, and run
//! by the sync engine as mail arrives. Here is what the window adds: the
//! switch for all of them; running one, or all, by hand; a new rule
//! started from a message; and what rules have done.
//!
//! Running by hand always shows first. Choosing a rule puts what it would
//! match in the list, as a search does, with what the rule would do said
//! in the list's title ([`RuleTrial`]). Nothing has changed until Run the
//! Rule Shown; leaving the list, as a search is left, changes nothing.

use std::sync::Arc;

use quill_core::types::{FolderRole, LocalMessageId};
use quill_store::{AgentEntry, DraftId};
use quill_sync::RuleSet;

use super::{
    Effects, Env, HookPicker, HostRequest, MessageList, Notice, Pane, Search, SidebarEntry, State,
};
use crate::data::DataRequest;
use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Mods};
use crate::setup::{AccountRequest, NewRule, Preference, RulesAsk, RulesNow};
use crate::text;

/// What the list's title says while every rule is being tried.
const ALL_RULES: &str = "All rules";

/// A rule, or all of them, being tried: the list shows what would be
/// changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuleTrial {
    /// The rule, by name. `None`: every rule that is switched on.
    pub which: Option<String>,
    /// What would be done, in words, once the store has said.
    pub says: Option<String>,
    /// How many messages would be changed.
    pub count: usize,
}

/// A list drawn over the panes to choose from, that is not the hooks: the
/// rules, or what rules did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickList {
    pub title: String,
    /// What to say when what was typed narrows the list to nothing.
    pub none: String,
    pub lines: Vec<PickLine>,
    pub what: PickFor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickLine {
    pub name: String,
    pub detail: String,
}

/// What choosing a line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// Tries the rule: shows what it would match.
    Rule,
    /// Takes back what a rule did: the log's line of this number, for
    /// each line of the list.
    TakeBack(Vec<i64>),
    /// Saves this message as the kind of file the line names.
    SaveAs(LocalMessageId),
}

/// What a new rule does with the mail it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleAction {
    #[default]
    Move,
    Delete,
    MarkRead,
}

impl RuleAction {
    pub const ALL: [RuleAction; 3] = [Self::Move, Self::Delete, Self::MarkRead];

    pub fn words(self) -> &'static str {
        match self {
            Self::Move => "Move to a folder",
            Self::Delete => "Delete (move to the Trash)",
            Self::MarkRead => "Mark as read",
        }
    }

    /// Its name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Delete => "delete",
            Self::MarkRead => "mark_read",
        }
    }

    pub fn named(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    fn next(self, forward: bool) -> Self {
        let at = Self::ALL
            .iter()
            .position(|action| *action == self)
            .unwrap_or(0);
        let count = Self::ALL.len();
        Self::ALL[if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        }]
    }
}

/// The parts of the form for a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleField {
    #[default]
    Name,
    /// Which account's mail, where there is more than one.
    Account,
    From,
    To,
    Subject,
    /// Words in the message's text.
    Text,
    Action,
    Folder,
}

/// An account a rule can be for, with the folders it can file mail in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuleAccount {
    pub name: String,
    /// Its folders, by name, with what each is for.
    pub folders: Vec<(String, FolderRole)>,
}

impl RuleAccount {
    /// The folder `wanted` names: by its name, whatever the case, or by
    /// what a folder is for (`archive`, `trash`, `junk`), as a rule's
    /// `move_to` is read.
    pub fn folder(&self, wanted: &str) -> Option<&str> {
        let wanted = wanted.trim().to_lowercase();
        let role = match wanted.as_str() {
            "archive" => Some(FolderRole::Archive),
            "trash" => Some(FolderRole::Trash),
            "junk" | "spam" => Some(FolderRole::Junk),
            "sent" => Some(FolderRole::Sent),
            "drafts" => Some(FolderRole::Drafts),
            _ => None,
        };
        self.folders
            .iter()
            .find(|(name, _)| name.to_lowercase() == wanted)
            .or_else(|| {
                self.folders
                    .iter()
                    .find(|(_, is)| role.is_some_and(|role| *is == role))
            })
            .map(|(name, _)| name.as_str())
    }

    /// The folders a rule would file mail in: not the inbox, which is
    /// where the mail already is, and not the folders for mail the user
    /// is writing or has sent.
    pub fn filing(&self) -> Vec<&str> {
        self.folders
            .iter()
            .filter(|(_, role)| {
                !matches!(
                    role,
                    FolderRole::Inbox | FolderRole::Drafts | FolderRole::Sent
                )
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// The folder to offer before one is chosen: the archive, where
    /// there is one, and else the first there is.
    pub fn usual(&self) -> Option<&str> {
        self.folders
            .iter()
            .find(|(_, role)| *role == FolderRole::Archive)
            .map(|(name, _)| name.as_str())
            .or_else(|| self.filing().first().copied())
    }
}

/// The form for a new rule: begun with nothing in it, or from a message,
/// with who sent it filled in.
///
/// Which mail the rule is for is asked a part at a time: who it is from,
/// who it is to, its subject, its text. Whatever is filled in must be in
/// the message. What is written into the file is a search made of them,
/// which can say more there than this form asks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RulePrompt {
    pub name: LineInput,
    pub from: LineInput,
    pub to: LineInput,
    pub subject: LineInput,
    /// Words in the message's text.
    pub text: LineInput,
    pub action: RuleAction,
    /// The folder to move to, by name.
    pub folder: LineInput,
    /// The accounts there are, with their folders, as the form was
    /// opened.
    pub accounts: Vec<RuleAccount>,
    /// The account the rule is for, as a place in `accounts`. `None`:
    /// every account's mail.
    pub account: Option<usize>,
    pub focus: RuleField,
    /// Why what was typed would not do.
    pub problem: Option<String>,
    /// It has been sent to be written, and the answer has not come.
    pub busy: bool,
}

/// What the choice of every account is called.
pub const EVERY_ACCOUNT: &str = "Every account";

impl RulePrompt {
    /// The fields there are now: an account is chosen only where there
    /// are several, and a folder is asked for only by a rule that moves.
    fn fields(&self) -> Vec<RuleField> {
        let mut fields = vec![RuleField::Name];
        if self.accounts.len() > 1 {
            fields.push(RuleField::Account);
        }
        fields.extend([
            RuleField::From,
            RuleField::To,
            RuleField::Subject,
            RuleField::Text,
            RuleField::Action,
        ]);
        if self.action == RuleAction::Move {
            fields.push(RuleField::Folder);
        }
        fields
    }

    fn step(&mut self, forward: bool) {
        let fields = self.fields();
        let at = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        let count = fields.len();
        self.focus = fields[if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        }];
    }

    /// The line being typed on, when the keys are on one.
    pub fn typing(&mut self) -> Option<&mut LineInput> {
        match self.focus {
            RuleField::Name => Some(&mut self.name),
            RuleField::From => Some(&mut self.from),
            RuleField::To => Some(&mut self.to),
            RuleField::Subject => Some(&mut self.subject),
            RuleField::Text => Some(&mut self.text),
            RuleField::Folder => Some(&mut self.folder),
            RuleField::Action | RuleField::Account => None,
        }
    }

    /// Changes what the rule does, and keeps the keys on a field that is
    /// still there.
    pub fn set_action(&mut self, action: RuleAction) {
        self.action = action;
        if self.focus == RuleField::Folder && action != RuleAction::Move {
            self.focus = RuleField::Action;
        }
    }

    /// The account the rule is for, by name; or that it is for all.
    pub fn account_name(&self) -> &str {
        self.chosen().map_or(EVERY_ACCOUNT, |account| &account.name)
    }

    /// The one account the rule is about: the one chosen, or the only
    /// one there is.
    pub fn chosen(&self) -> Option<&RuleAccount> {
        match (self.account, self.accounts.as_slice()) {
            (Some(index), accounts) => accounts.get(index),
            (None, [only]) => Some(only),
            (None, _) => None,
        }
    }

    /// The folders to choose from, when the rule is about one account:
    /// that account's. About every account there is no one list, and the
    /// folder is typed.
    pub fn folder_choices(&self) -> Option<Vec<&str>> {
        self.chosen()
            .map(RuleAccount::filing)
            .filter(|folders| !folders.is_empty())
    }

    /// Every account, then each: the next of them, or the one before.
    fn next_account(&mut self, forward: bool) {
        let count = self.accounts.len() + 1;
        let at = self.account.map_or(0, |index| index + 1);
        let next = if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        self.account = next.checked_sub(1);
    }

    /// Takes a key: `tab` and the arrows go between the fields, `space`
    /// and the side arrows change a choice, and anything else is typing.
    /// Returns whether the key was the form's.
    pub fn key(&mut self, chord: KeyChord) -> bool {
        if self.busy {
            return false;
        }
        let plain = chord.mods == Mods::default();
        let choosing = matches!(self.focus, RuleField::Action | RuleField::Account);
        match chord.key {
            Key::Tab if plain => self.step(true),
            Key::Tab
                if chord.mods
                    == (Mods {
                        shift: true,
                        ..Mods::default()
                    }) =>
            {
                self.step(false);
            }
            Key::Down if plain => self.step(true),
            Key::Up if plain => self.step(false),
            Key::Left | Key::Right | Key::Char(' ') if plain && choosing => {
                let forward = chord.key != Key::Left;
                if self.focus == RuleField::Action {
                    self.set_action(self.action.next(forward));
                } else {
                    self.next_account(forward);
                }
                self.problem = None;
            }
            _ => {
                let typed = self.typing().is_some_and(|input| input.key(chord));
                if !typed {
                    return false;
                }
                self.problem = None;
            }
        }
        true
    }

    /// Which mail the rule is for, as a search: each part that is filled
    /// in, in the place it names.
    pub fn matching(&self) -> String {
        [
            ("from", &self.from),
            ("to", &self.to),
            ("subject", &self.subject),
            ("body", &self.text),
        ]
        .into_iter()
        .filter_map(|(filter, input)| {
            let value = filter_value(input.text());
            (!value.is_empty()).then(|| format!("{filter}:{value}"))
        })
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// The folder the rule files mail in, as it is to be written, or why
    /// what was typed is no folder the rule could use.
    fn filing_folder(&self) -> Result<String, String> {
        let typed = self.folder.text().trim();
        if typed.is_empty() {
            return Err("Name the folder to move the mail to.".to_owned());
        }
        let such_as = |account: &RuleAccount| {
            let folders = account.filing();
            if folders.is_empty() {
                String::new()
            } else {
                format!(" Its folders are: {}.", folders.join(", "))
            }
        };
        match self.chosen() {
            // Written as the account names it, whatever case was typed.
            Some(account) => account
                .folder(typed)
                .filter(|name| account.filing().contains(name))
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "{} has no folder called \"{typed}\".{}",
                        account.name,
                        such_as(account)
                    )
                }),
            None => {
                let without: Vec<&str> = self
                    .accounts
                    .iter()
                    .filter(|account| account.folder(typed).is_none())
                    .map(|account| account.name.as_str())
                    .collect();
                if without.is_empty() {
                    Ok(typed.to_owned())
                } else {
                    Err(format!(
                        "There is no folder called \"{typed}\" in {}. Choose the account the \
                         rule is for, or a folder every account has, such as archive.",
                        without.join(" or ")
                    ))
                }
            }
        }
    }

    /// The rule as it is to be written, or what is missing from it.
    pub fn rule(&self) -> Result<NewRule, String> {
        let name = self.name.text().trim();
        if name.is_empty() {
            return Err("Give the rule a name.".to_owned());
        }
        let matching = self.matching();
        if matching.is_empty() {
            return Err(
                "Say which mail the rule is for: fill in who it is from, who it is to, its \
                 subject or its text."
                    .to_owned(),
            );
        }
        let move_to = match self.action {
            RuleAction::Move => Some(self.filing_folder()?),
            RuleAction::Delete | RuleAction::MarkRead => None,
        };
        Ok(NewRule {
            name: name.to_owned(),
            matching,
            // Said only where it narrows anything.
            account: self
                .account
                .and_then(|index| self.accounts.get(index))
                .filter(|_| self.accounts.len() > 1)
                .map(|account| account.name.clone()),
            move_to,
            delete: self.action == RuleAction::Delete,
            mark_read: self.action == RuleAction::MarkRead,
        })
    }
}

/// What was typed in one part of the form, as the value of a search
/// filter: quoted when it is more than a word, and never with a quote of
/// its own or anything that would end it. A leading `-`, which a search
/// reads as "not", is dropped: the form asks what the mail has.
fn filter_value(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| *c != '"' && !c.is_control())
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('-').trim();
    if cleaned.is_empty() {
        String::new()
    } else if cleaned.contains(char::is_whitespace) || cleaned.contains(':') {
        format!("\"{cleaned}\"")
    } else {
        cleaned.to_owned()
    }
}

impl State {
    /// Whether rules run by themselves.
    pub fn rules_on(&self) -> bool {
        self.rules.on
    }

    /// The rule being tried, if one is.
    pub fn rule_trial(&self) -> Option<&RuleTrial> {
        self.search.as_ref()?.rule.as_ref()
    }

    /// What the list's title says while a rule is tried: the rule, and
    /// what it would do.
    pub fn trial_title(&self) -> Option<String> {
        let trial = self.rule_trial()?;
        let name = trial.which.as_deref().unwrap_or(ALL_RULES);
        Some(match &trial.says {
            Some(says) => format!("{} {says}", text::sanitize_line(name)),
            None => format!("{}: looking\u{2026}", text::sanitize_line(name)),
        })
    }

    /// Whether there is a rule to choose and run.
    pub fn can_run_a_rule(&self) -> bool {
        self.compose.is_none() && !self.rules.rules.is_empty()
    }

    /// Whether there is any rule switched on to run.
    pub fn can_run_all_rules(&self) -> bool {
        self.compose.is_none() && self.rules.rules.iter().any(|rule| rule.enabled)
    }

    /// Whether a rule is on show that would change something.
    pub fn can_apply_rule(&self) -> bool {
        self.rule_trial().is_some_and(|trial| trial.count > 0)
    }

    /// The message a rule would be started from: the one the user is on,
    /// when it is mail with a sender.
    fn rule_message(&self) -> Option<(String, String, quill_core::types::LocalAccountId)> {
        let row = self.target_row()?;
        if DraftId::from_message_id(row.id).is_some() {
            return None;
        }
        let from = row.headers.from.first()?;
        let address = from.email.trim();
        if address.is_empty() {
            return None;
        }
        let name = from
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(address);
        Some((name.to_owned(), address.to_owned(), row.account))
    }

    /// Whether a rule can be started from where the user is.
    pub fn can_start_rule(&self) -> bool {
        self.compose.is_none() && self.new_rule.is_none() && self.rule_message().is_some()
    }

    /// Switches every rule on or off. On, the engine catches up on what
    /// came meanwhile. It holds at once here, and is written to the
    /// settings for the engine and for next time.
    pub(super) fn toggle_rules(&mut self, out: &mut Effects) {
        let on = !self.rules.on;
        self.rules = Arc::new(RuleSet {
            on,
            ..(*self.rules).clone()
        });
        out.accounts
            .push(AccountRequest::Remember(Preference::Rules(on)));
        self.flash = Some((
            if on {
                "Rules are on.".to_owned()
            } else {
                "Rules are off: mail stays where it arrives.".to_owned()
            },
            false,
        ));
    }

    /// Opens the list of rules, to choose one to try.
    pub(super) fn pick_rule(&mut self) {
        if self.rules.rules.is_empty() {
            return;
        }
        let lines: Vec<PickLine> = self
            .rules
            .rules
            .iter()
            .map(|rule| PickLine {
                name: text::sanitize_line(&rule.name),
                detail: if rule.enabled {
                    text::sanitize_line(&rule.does())
                } else {
                    format!("{} (switched off)", text::sanitize_line(&rule.does()))
                },
            })
            .collect();
        self.open_pick_list(PickList {
            title: " Run a rule: enter shows what it would change ".to_owned(),
            none: "No rule matches.".to_owned(),
            lines,
            what: PickFor::Rule,
        });
    }

    pub(super) fn open_pick_list(&mut self, list: PickList) {
        self.hooks = Some(HookPicker {
            message: LocalMessageId(0),
            filter: LineInput::default(),
            selected: 0,
            offset: 0,
            total: list.lines.len(),
            list: Some(list),
        });
    }

    /// Does what choosing line `index` of a list is for.
    pub(super) fn picked(&mut self, env: &Env, list: &PickList, index: usize, out: &mut Effects) {
        match &list.what {
            PickFor::Rule => {
                // By the name the rule has, not the name as it is shown.
                if let Some(rule) = self.rules.rules.get(index) {
                    let name = rule.name.clone();
                    self.try_rules(env, Some(name), out);
                }
            }
            PickFor::TakeBack(entries) => {
                if let Some(entry) = entries.get(index) {
                    out.data.push(DataRequest::RulesUndo(*entry));
                }
            }
            PickFor::SaveAs(message) => {
                if let Some(kind) = super::SaveKind::ALL.get(index) {
                    self.save_as(env, *message, *kind, out);
                }
            }
        }
    }

    /// Shows what a rule would change, in the list: the rule of this
    /// name, or every rule that is on. Nothing is changed.
    pub(super) fn try_rules(&mut self, env: &Env, which: Option<String>, out: &mut Effects) {
        let shown = which.clone().unwrap_or_else(|| ALL_RULES.to_owned());
        self.search = Some(Search {
            input: LineInput::new(text::sanitize_line(&shown)),
            typing: false,
            found: None,
            more: false,
            rule: Some(RuleTrial {
                which,
                says: None,
                count: 0,
            }),
        });
        self.list = MessageList::default();
        self.set_focus(Pane::List);
        if let Some(request) = self.search_request(env) {
            out.data.push(request);
        }
    }

    /// Runs the rule on show, and goes back to the mailbox it was tried
    /// from.
    pub(super) fn apply_rule(&mut self, out: &mut Effects) {
        let Some(trial) = self.rule_trial().cloned() else {
            return;
        };
        out.data.push(DataRequest::RulesRun {
            set: Arc::clone(&self.rules),
            which: trial.which,
        });
        self.search = None;
        self.list = MessageList::default();
        out.data.push(DataRequest::LoadMessages(self.mailbox));
        self.select_mailbox_row(self.mailbox, out);
    }

    /// The accounts there are, each with its folders, for the form.
    fn rule_accounts(&self) -> Vec<RuleAccount> {
        let mut accounts: Vec<RuleAccount> = self
            .accounts
            .iter()
            .map(|account| RuleAccount {
                name: text::sanitize_line(&account.name),
                folders: Vec::new(),
            })
            .collect();
        for entry in &self.sidebar.entries {
            if let SidebarEntry::Folder {
                account,
                name,
                role,
                ..
            } = entry
                && let Some(of) = accounts.get_mut(*account)
            {
                of.folders.push((name.clone(), *role));
            }
        }
        accounts
    }

    /// Whether a rule can be begun now.
    pub fn can_create_rule(&self) -> bool {
        self.compose.is_none() && self.new_rule.is_none() && !self.accounts.is_empty()
    }

    /// Opens the form for a new rule, with nothing in it: for every
    /// account's mail, until one is chosen.
    pub(super) fn ask_blank_rule(&mut self) {
        if !self.can_create_rule() {
            return;
        }
        self.new_rule = Some(RulePrompt {
            accounts: self.rule_accounts(),
            ..RulePrompt::default()
        });
    }

    /// Opens the form for a new rule about mail from whoever sent the
    /// message the user is on.
    pub(super) fn ask_new_rule(&mut self) {
        let Some((name, address, account)) = self.rule_message() else {
            return;
        };
        // With several accounts the rule is for the one the message is
        // in, until another is chosen: the folder it names is that
        // account's.
        let account = (self.accounts.len() > 1)
            .then(|| self.accounts.iter().position(|row| row.id == account))
            .flatten();
        self.new_rule = Some(RulePrompt {
            name: LineInput::new(text::sanitize_line(&format!("From {name}"))),
            from: LineInput::new(text::sanitize_line(&address)),
            accounts: self.rule_accounts(),
            account,
            ..RulePrompt::default()
        });
    }

    /// Sends the rule the form describes to be written, or says what is
    /// missing from it.
    pub(super) fn answer_new_rule(&mut self, out: &mut Effects) {
        let Some(prompt) = &mut self.new_rule else {
            return;
        };
        if prompt.busy {
            return;
        }
        match prompt.rule() {
            Ok(rule) => {
                prompt.problem = None;
                prompt.busy = true;
                out.accounts
                    .push(AccountRequest::Rules(RulesAsk::Add(rule)));
            }
            Err(problem) => prompt.problem = Some(problem),
        }
    }

    /// The rules were read: at the start, because their file changed, or
    /// because they were switched.
    pub(super) fn on_rules(&mut self, env: &Env, now: RulesNow, out: &mut Effects) {
        if now.problem != self.rules_problem {
            if let Some(problem) = &now.problem {
                self.notices
                    .push_back(Notice::error(text::sanitize_line(problem)));
            }
            self.rules_problem = now.problem;
        }
        self.rules = now.set;
        // A rule on show is shown as it now is.
        if self.rule_trial().is_some()
            && let Some(request) = self.search_request(env)
        {
            out.data.push(request);
        }
    }

    /// A rule the form described was written, or was not.
    pub(super) fn on_rule_added(
        &mut self,
        env: &Env,
        result: Result<String, String>,
        out: &mut Effects,
    ) {
        match result {
            Ok(name) => {
                self.new_rule = None;
                // What it would match, before it is run on anything.
                self.try_rules(env, Some(name), out);
            }
            Err(reason) => match &mut self.new_rule {
                Some(prompt) => {
                    prompt.busy = false;
                    prompt.problem = Some(text::sanitize_line(&reason));
                }
                None => self.notices.push_back(Notice::error(format!(
                    "The rule was not saved: {}",
                    text::sanitize_line(&reason)
                ))),
            },
        }
    }

    /// Where the rules' file is, now that it is there to be opened; or
    /// why it cannot be. The app opens it, in whatever the Mac opens
    /// such a file with. In a terminal it is said where it is: what a
    /// file is edited with there is the user's own business.
    pub(super) fn on_rules_file(
        &mut self,
        env: &Env,
        result: Result<std::path::PathBuf, String>,
        out: &mut Effects,
    ) {
        match result {
            Ok(_) if env.window => out.host.push(HostRequest::EditRules),
            Ok(path) => self.notices.push_back(Notice::info(format!(
                "Rules are written in {}. Quill reads the file again whenever it is saved.",
                text::sanitize_line(&path.display().to_string())
            ))),
            Err(reason) => self.notices.push_back(Notice::error(format!(
                "The rules' file cannot be opened: {}",
                text::sanitize_line(&reason)
            ))),
        }
    }

    /// What a run by hand came to.
    pub(super) fn on_rules_ran(&mut self, said: &str, problems: Vec<String>, undoable: bool) {
        self.flash = Some((format!("{}.", text::sanitize_line(said)), undoable));
        for problem in problems {
            self.notices
                .push_back(Notice::error(text::sanitize_line(&problem)));
        }
    }

    /// What rules did lately: a list, from which one can be taken back.
    pub(super) fn on_rules_log(&mut self, env: &Env, entries: Vec<AgentEntry>) {
        if entries.is_empty() {
            self.flash = Some(("Rules have done nothing yet.".to_owned(), false));
            return;
        }
        let lines = entries
            .iter()
            .map(|entry| PickLine {
                name: text::sanitize_line(&entry.agent),
                // Said first where it is so: it is what matters most of
                // a line, and the end of a line may not fit.
                detail: format!(
                    "{}{}, {}",
                    if entry.undone.is_some() {
                        "(taken back) "
                    } else {
                        ""
                    },
                    text::sanitize_line(&entry.summary),
                    text::short_date(entry.at, self.now, &env.tz).trim(),
                ),
            })
            .collect();
        self.open_pick_list(PickList {
            title: " What rules did: enter takes one back ".to_owned(),
            none: "Nothing matches.".to_owned(),
            lines,
            what: PickFor::TakeBack(entries.iter().map(|entry| entry.id).collect()),
        });
    }

    /// The engine's rules would change more than they do unasked.
    pub(super) fn on_rules_waiting(&mut self, account: &str, count: usize) {
        self.notices.push_back(Notice::info(format!(
            "Rules would change {count} messages in {}'s inbox, which is more than they change \
             without asking. Run All Rules shows them first.",
            text::sanitize_line(account)
        )));
    }
}
