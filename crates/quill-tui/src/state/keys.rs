//! Changing a command's keys, from the settings window.
//!
//! The window lists every command with the keys it has
//! (`settings::keys`). Changing one is a question in a sheet
//! ([`KeysPrompt`]): the new key is pressed, not typed, into a field the
//! host records keys in, and what to do with it is chosen. Whether the key
//! can be had, and what it would be taken from, is worked out here, by the
//! same rules the keys are looked up by; the host records and shows, and
//! decides nothing.
//!
//! The change itself is the accounts service's to make
//! ([`AccountRequest::Keys`]): it writes `keys.toml` and reads the keys
//! afresh, and they hold from its answer. Only a host with sheets has
//! this: in a terminal the keys are changed in the file.

use quill_host::bridge::{
    FormButton, FormField, FormOption, FormRole, FormShow, FormShowParams, FormUsed,
};

use super::{Effects, Env, State};
use crate::command::{CommandId, CommandSpec};
use crate::keymap::KeySeq;
use crate::settings::{self, KeysChange};
use crate::setup::{AccountRequest, Press, headline};
use crate::text;

/// What the question is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysAsk {
    /// The keys of this command.
    Change(CommandId),
    /// Putting every key back as the preset has it.
    ResetAll,
}

/// What is to be done with the key that was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysHow {
    /// It becomes the command's one key.
    Replace,
    /// It is one more key for the command.
    Add,
    /// The command is left with no key at all.
    Clear,
}

impl KeysHow {
    const ALL: [KeysHow; 3] = [KeysHow::Replace, KeysHow::Add, KeysHow::Clear];

    fn id(self) -> &'static str {
        match self {
            KeysHow::Replace => "replace",
            KeysHow::Add => "add",
            KeysHow::Clear => "clear",
        }
    }

    fn title(self) -> &'static str {
        match self {
            KeysHow::Replace => "Use it in place of the keys it has",
            KeysHow::Add => "Add it to the keys it has",
            KeysHow::Clear => "Leave it with no key",
        }
    }
}

/// The question in front of the settings window: which key a command is
/// to have, or whether every key is to go back to the preset's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysPrompt {
    pub ask: KeysAsk,
    /// The keys pressed, as they are written: `ctrl-k`, `g g`. Empty
    /// until one is.
    pub recorded: String,
    pub how: KeysHow,
}

/// The steps of the two questions, and the fields of the first.
const CHANGE: &str = "keys_change";
const RESET: &str = "keys_reset";
const KEYS: &str = "keys";
const HOW: &str = "how";
const CONFIRM: &str = "confirm";

/// What a key that was pressed comes to for a command.
#[derive(Debug, Default, PartialEq, Eq)]
struct Verdict {
    /// The key cannot be had, and why.
    refused: Option<String>,
    /// The commands that have the key now where this command is used, by
    /// name and title: they lose it if it is taken.
    taken_from: Vec<(&'static str, &'static str)>,
}

fn share_a_place(one: &CommandSpec, other: &CommandSpec) -> bool {
    one.contexts
        .iter()
        .any(|context| other.contexts.contains(context))
}

/// A command's title as part of a sentence: without the dots that say a
/// question follows.
fn named(spec: &CommandSpec) -> &'static str {
    spec.title.trim_end_matches('\u{2026}')
}

impl KeysPrompt {
    /// What the key recorded would mean for `command`: whether it can be
    /// had, and what has it now.
    fn verdict(&self, env: &Env, command: &CommandSpec) -> Verdict {
        let mut verdict = Verdict::default();
        if self.how == KeysHow::Clear || self.recorded.trim().is_empty() {
            return verdict;
        }
        let pressed: KeySeq = match self.recorded.parse() {
            Ok(pressed) => pressed,
            Err(why) => {
                verdict.refused = Some(format!(
                    "That is not a key Quill can use here: {why}. Press another."
                ));
                return verdict;
            }
        };
        for other in env.registry.specs() {
            if other.id == command.id || !share_a_place(command, other) {
                continue;
            }
            for key in env.keymap.keys_for(other.id) {
                if *key == pressed {
                    if !verdict
                        .taken_from
                        .iter()
                        .any(|(name, _)| *name == other.name)
                    {
                        verdict.taken_from.push((other.name, named(other)));
                    }
                } else if key.0.starts_with(&pressed.0) {
                    // Pressed, it would act at once, and the longer key
                    // would never be finished.
                    verdict.refused = Some(format!(
                        "{pressed} cannot be a key by itself here: it is how {key} begins, \
                         which is the key for {}.",
                        named(other)
                    ));
                } else if pressed.0.starts_with(&key.0) {
                    verdict.refused = Some(format!(
                        "{pressed} would never be reached: it begins with {key}, which is \
                         the key for {}.",
                        named(other)
                    ));
                }
            }
        }
        verdict
    }

    /// The question as a host shows it.
    pub fn form(&self, env: &Env) -> Option<FormShow> {
        let cancel = Press::Cancel.button(Some(FormRole::Cancel), true);
        let command = match self.ask {
            KeysAsk::ResetAll => {
                return FormShow::new(FormShowParams {
                    step: RESET.to_owned(),
                    title: "Reset All Keys?".to_owned(),
                    note: Some(
                        "Every key goes back to what Quill comes with. Your own keys are \
                         taken out of keys.toml; nothing else in that file is touched."
                            .to_owned(),
                    ),
                    error: None,
                    busy: None,
                    fields: Vec::new(),
                    buttons: vec![
                        cancel,
                        // No key presses it: it undoes what was set up by
                        // hand.
                        FormButton {
                            id: CONFIRM.to_owned(),
                            title: "Reset All".to_owned(),
                            role: None,
                            enabled: true,
                        },
                    ],
                });
            }
            KeysAsk::Change(command) => env.registry.get(command)?,
        };
        let verdict = self.verdict(env, command);
        let mut note = format!(
            "It has: {}.",
            settings::keys_written(env.keymap.keys_for(command.id))
        );
        if !verdict.taken_from.is_empty() && verdict.refused.is_none() {
            let others: Vec<&str> = verdict.taken_from.iter().map(|(_, title)| *title).collect();
            note.push_str(&format!(
                " {} is the key for {} in the same place now. Saving takes it from there.",
                self.recorded.trim(),
                others.join(" and for ")
            ));
        }
        let ready = verdict.refused.is_none()
            && (self.how == KeysHow::Clear || !self.recorded.trim().is_empty());
        FormShow::new(FormShowParams {
            step: CHANGE.to_owned(),
            title: headline(&format!("Keys for {}", named(command))),
            note: Some(note),
            error: verdict.refused.as_deref().map(text::sanitize_line),
            busy: None,
            fields: vec![
                FormField::Keys {
                    id: KEYS.to_owned(),
                    label: "New Key:".to_owned(),
                    value: self.recorded.clone(),
                    focus: true,
                    help: Some(
                        "Press the key, or two keys one after the other for a pair such as \
                         g g. esc, return and tab work this sheet and are not taken: bind \
                         those in keys.toml."
                            .to_owned(),
                    ),
                },
                FormField::Choice {
                    id: HOW.to_owned(),
                    label: "Do:".to_owned(),
                    value: self.how.id().to_owned(),
                    options: KeysHow::ALL
                        .into_iter()
                        .map(|how| FormOption {
                            id: how.id().to_owned(),
                            title: how.title().to_owned(),
                        })
                        .collect(),
                    focus: false,
                    help: None,
                },
            ],
            buttons: vec![
                cancel,
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: "Save".to_owned(),
                    role: Some(FormRole::Default),
                    enabled: ready,
                },
            ],
        })
    }

    /// Puts what the host's fields held into the question's own.
    fn fill(&mut self, used: &FormUsed) {
        if let Some(keys) = used.value(KEYS) {
            // One line, as a key is written.
            self.recorded = text::sanitize_line(keys).trim().to_owned();
        }
        if let Some(how) = used
            .value(HOW)
            .and_then(|id| KeysHow::ALL.into_iter().find(|how| how.id() == id))
        {
            self.how = how;
        }
    }

    /// The change the question comes to, if it comes to one.
    fn change(&self, env: &Env) -> Option<KeysChange> {
        let command = match self.ask {
            KeysAsk::ResetAll => return Some(KeysChange::ResetAll),
            KeysAsk::Change(command) => env.registry.get(command)?,
        };
        if self.how == KeysHow::Clear {
            return Some(KeysChange::Set(vec![(command.name.to_owned(), Vec::new())]));
        }
        let verdict = self.verdict(env, command);
        if verdict.refused.is_some() {
            return None;
        }
        let pressed: KeySeq = self.recorded.parse().ok()?;
        let written = |keys: &[KeySeq]| keys.iter().map(ToString::to_string).collect::<Vec<_>>();
        let mut keys = match self.how {
            KeysHow::Add => written(env.keymap.keys_for(command.id)),
            _ => Vec::new(),
        };
        if !keys.contains(&pressed.to_string()) {
            keys.push(pressed.to_string());
        }
        let mut sets = vec![(command.name.to_owned(), keys)];
        // Whatever had the key in the same place gives it up: a key means
        // one thing in one place.
        for (name, _) in &verdict.taken_from {
            let Some(other) = env.registry.by_name(name) else {
                continue;
            };
            let left: Vec<KeySeq> = env
                .keymap
                .keys_for(other.id)
                .iter()
                .filter(|key| **key != pressed)
                .cloned()
                .collect();
            sets.push(((*name).to_owned(), written(&left)));
        }
        Some(KeysChange::Set(sets))
    }
}

impl State {
    /// Whether the keys can be changed from here: a host with sheets has
    /// the settings window open, the file has been read, and the user's
    /// own keys are in a state to be written to.
    fn keys_can_change(&self, env: &Env) -> bool {
        env.sheets
            && self.keys.is_none()
            && self
                .settings_read()
                .is_some_and(|read| read.keys_problem.is_none())
    }

    /// Something was asked of the list of keys in the settings window:
    /// one of its actions, of the row picked if of any.
    pub(super) fn on_keys_action(
        &mut self,
        env: &Env,
        action: &str,
        row: Option<&str>,
        out: &mut Effects,
    ) {
        if !self.keys_can_change(env) {
            return;
        }
        let command = row.and_then(|row| {
            env.registry
                .specs()
                .iter()
                .find(|spec| settings::binding_id(spec.name) == row)
        });
        match (action, command) {
            (settings::CHANGE_KEY, Some(command)) => {
                self.keys = Some(KeysPrompt {
                    ask: KeysAsk::Change(command.id),
                    recorded: String::new(),
                    how: KeysHow::Replace,
                });
            }
            // Back to the preset's keys: nothing is lost that is not one
            // line of a file, so it is not asked about.
            (settings::DEFAULT_KEY, Some(command)) => {
                out.accounts
                    .push(AccountRequest::Keys(KeysChange::Reset(vec![
                        command.name.to_owned(),
                    ])));
            }
            (settings::RESET_KEYS, _) => {
                self.keys = Some(KeysPrompt {
                    ask: KeysAsk::ResetAll,
                    recorded: String::new(),
                    how: KeysHow::Replace,
                });
            }
            _ => {}
        }
    }

    /// Takes what was done with the question in the host's sheet: what its
    /// fields held goes into the question's own, and then the button does
    /// its work. With no button, a key was pressed or the choice changed,
    /// and what is said of it changes with it.
    pub(super) fn answer_keys_in_sheet(&mut self, env: &Env, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = &mut self.keys else {
            return;
        };
        let step = match prompt.ask {
            KeysAsk::Change(_) => CHANGE,
            KeysAsk::ResetAll => RESET,
        };
        if used.step() != step {
            return;
        }
        match used.button() {
            Some(id) if id == Press::Cancel.id() => self.keys = None,
            Some(CONFIRM) => {
                prompt.fill(used);
                // A key that cannot be had leaves the question up, saying
                // why.
                if let Some(change) = prompt.change(env) {
                    out.accounts.push(AccountRequest::Keys(change));
                    self.keys = None;
                }
            }
            Some(_) => {}
            None => prompt.fill(used),
        }
    }
}
