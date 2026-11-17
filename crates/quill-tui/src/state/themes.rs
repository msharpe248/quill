//! Changing a theme's colours, and deleting a theme, from the settings
//! window.
//!
//! The window lists the themes there are (`settings::appearance`). Editing
//! one is a question in a sheet ([`ThemePrompt`]): a name, and a well for
//! each colour, which the host shows and has colours picked in as its
//! system picks them. Every colour picked comes here at once, and the
//! theme as it then stands is drawn with while the question is up
//! ([`Look::trying`](crate::theme::Look)), so that what is chosen is seen
//! on the mail itself. Saved under the name it had, the theme is changed;
//! under another, it is a new theme and the one it was made from is left
//! alone. What a theme may be called, and whether a name is taken, is
//! decided here.
//!
//! A colour the theme leaves to the terminal (its own background, one of
//! its sixteen) has no red, green and blue to show: the well has what a
//! terminal usually has for it ([`ColorSpec::roughly`]), and the theme
//! keeps what it said unless another colour is picked there.
//!
//! The writing and the deleting are the accounts service's
//! ([`AccountRequest::Themes`]). Only a host with sheets has any of this:
//! in a terminal a theme is a file.

use quill_core::config::is_theme_name;
use quill_host::bridge::{
    FormButton, FormField, FormRole, FormShow, FormShowParams, FormUsed, TerminalColor,
};

use super::{Effects, Env, HostRequest, State};
use crate::settings::{self, SettingsRead, ThemeLine, ThemesChange};
use crate::setup::{AccountRequest, Press, headline};
use crate::text;
use crate::theme::{ColorSpec, Slot, Theme, contrast};

/// What the question is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeAsk {
    /// The colours of the theme listed under this name, and the name it
    /// is to be saved under. `own` says it is a file of the user's, which
    /// can be saved over; one Quill comes with cannot.
    Edit { from: String, own: bool },
    /// Deleting the theme listed under this name.
    Delete { name: String },
}

/// The question in front of the settings window: what a theme's colours
/// are to be and what it is to be called, or whether one is to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePrompt {
    pub ask: ThemeAsk,
    /// The name to save under, as typed.
    pub name: String,
    /// The theme as it stands: drawn with while the question is up.
    pub draft: Theme,
    /// Whether Quill was dark as the question was asked: what a colour
    /// left to the terminal is shown as goes by it, and stays put.
    dark: bool,
    /// The names of the themes there are, to tell a name that is taken.
    names: Vec<String>,
    /// Which of the two themes in the settings the one to delete is, said
    /// in the question: the light one, the dark one.
    in_use: (bool, bool),
    /// Why the last answer was not taken.
    problem: Option<String>,
}

/// The steps of the two questions, and the names of their parts.
const EDIT: &str = "theme_edit";
const DELETE: &str = "theme_delete";
const NAME: &str = "name";
const CONFIRM: &str = "confirm";

/// The name on the bridge of the well of a colour.
fn slot_id(slot: Slot) -> String {
    match slot {
        Slot::Account(index) => format!("account_{}", index + 1),
        fixed => fixed.key().to_owned(),
    }
}

fn builtin(name: &str) -> bool {
    Theme::builtin_names().any(|builtin| builtin.eq_ignore_ascii_case(name))
}

/// A name as it is said in a sentence.
fn quoted(name: &str) -> String {
    format!("\u{201c}{}\u{201d}", text::sanitize_line(name))
}

/// A name for a copy of the theme called `name` that no theme has yet.
fn copy_name(name: &str, taken: &[String]) -> String {
    let free = |candidate: &str| {
        !builtin(candidate)
            && !taken
                .iter()
                .any(|other| other.to_lowercase() == candidate.to_lowercase())
    };
    let base = format!("{name} copy");
    if free(&base) {
        return base;
    }
    (2..100)
        .map(|count| format!("{base} {count}"))
        .find(|candidate| free(candidate))
        .unwrap_or(base)
}

impl ThemePrompt {
    fn edit(line: &ThemeLine, theme: &Theme, read: &SettingsRead, dark: bool) -> Self {
        let names: Vec<String> = read.themes.iter().map(|line| line.name.clone()).collect();
        let mut draft = theme.clone();
        // A theme with no colour for any account has none to choose.
        draft.name = line.name.clone();
        Self {
            ask: ThemeAsk::Edit {
                from: line.name.clone(),
                own: line.own,
            },
            // One of the user's own keeps its name until they give it
            // another; one of Quill's is saved as a copy.
            name: if line.own {
                line.name.clone()
            } else {
                copy_name(&settings::theme_title(&line.name), &names)
            },
            draft,
            // A colour the theme leaves to the terminal is shown as one
            // that goes with the theme's own background, where it has one.
            dark: theme.is_dark().unwrap_or(dark),
            names,
            in_use: (false, false),
            problem: None,
        }
    }

    fn delete(line: &ThemeLine, read: &SettingsRead) -> Self {
        Self {
            ask: ThemeAsk::Delete {
                name: line.name.clone(),
            },
            name: line.name.clone(),
            draft: Theme::terminal(),
            dark: false,
            names: Vec::new(),
            in_use: (
                read.settings.theme_light == line.name,
                read.settings.theme_dark == line.name,
            ),
            problem: None,
        }
    }

    /// The theme to draw with while the question is up, if it is one
    /// about a theme's colours.
    pub fn trying(&self) -> Option<&Theme> {
        matches!(self.ask, ThemeAsk::Edit { .. }).then_some(&self.draft)
    }

    /// What a slot's well shows: the colour as red, green and blue.
    fn shown(&self, slot: Slot) -> Option<(u8, u8, u8)> {
        let spec = self.draft.colors.get(slot)?;
        Some(spec.roughly(slot.is_ground(), self.dark))
    }

    /// Why the theme cannot be saved under the name typed, if it cannot.
    fn refused(&self) -> Option<String> {
        let ThemeAsk::Edit { from, own } = &self.ask else {
            return None;
        };
        let name = self.name.trim();
        if name.is_empty() {
            return Some("A theme needs a name.".to_owned());
        }
        if !is_theme_name(name) {
            return Some(
                "A theme\u{2019}s name is the name of its file: it cannot have /, \\ or : in \
                 it, start with a dot, end in .toml, or be longer than 100 letters."
                    .to_owned(),
            );
        }
        // One of Quill's own is saved over only where a file of the
        // user's already stands in its place, and it is that one.
        if builtin(name) && !(*own && name == from) {
            return Some(format!(
                "{} is a theme Quill comes with, and those stay as they are. Give this one \
                 another name.",
                quoted(name)
            ));
        }
        // Another theme has the name, or one that differs only in its
        // capitals, which one folder cannot hold both of.
        let taken = self.names.iter().find(|other| {
            other.to_lowercase() == name.to_lowercase() && !(*other == from && name == from)
        });
        if let Some(other) = taken {
            return Some(format!(
                "There is already a theme called {}. Edit that one, or give this one another \
                 name.",
                quoted(other)
            ));
        }
        None
    }

    /// The question as a host shows it.
    pub fn form(&self) -> Option<FormShow> {
        let cancel = Press::Cancel.button(Some(FormRole::Cancel), true);
        let from = match &self.ask {
            ThemeAsk::Delete { name } => {
                let mut note = "Its file is taken out of the themes folder, and that cannot \
                                be undone."
                    .to_owned();
                if builtin(name) {
                    note.push_str(" The theme of that name that Quill comes with is used again.");
                } else {
                    match self.in_use {
                        (true, true) => note.push_str(
                            " It is the theme in use: Quill goes back to the one it comes with.",
                        ),
                        (true, false) => note.push_str(
                            " It is the light theme: Quill goes back to the one it had for \
                             that.",
                        ),
                        (false, true) => note.push_str(
                            " It is the dark theme: Quill goes back to the one it had for that.",
                        ),
                        (false, false) => {}
                    }
                }
                return FormShow::new(FormShowParams {
                    step: DELETE.to_owned(),
                    title: format!("Delete the Theme {}?", quoted(&settings::theme_title(name))),
                    note: Some(note),
                    error: None,
                    busy: None,
                    fields: Vec::new(),
                    buttons: vec![
                        cancel,
                        // No key presses it: it cannot be taken back.
                        FormButton {
                            id: CONFIRM.to_owned(),
                            title: "Delete".to_owned(),
                            role: None,
                            enabled: true,
                        },
                    ],
                });
            }
            ThemeAsk::Edit { from, .. } => from,
        };
        let mut note = "Quill is drawn in these colours as they are chosen. Saved under \
                        another name this is a new theme, and the one it was made from stays \
                        as it is."
            .to_owned();
        // What is read has to be readable: said, and left to the user.
        if let (Some(ground), Some(ink)) = (self.shown(Slot::Background), self.shown(Slot::Text)) {
            let ratio = contrast::contrast(ground, ink);
            if ratio < contrast::BODY {
                note.push_str(&format!(
                    " The text is hard to read on this background: their contrast is {ratio:.1}, \
                     and under {:.1} is hard on the eyes.",
                    contrast::BODY
                ));
            }
        }
        let mut fields = vec![FormField::Text {
            id: NAME.to_owned(),
            label: "Name:".to_owned(),
            value: self.name.clone(),
            focus: true,
            help: None,
        }];
        for slot in self.draft.colors.slots() {
            let Some((red, green, blue)) = self.shown(slot) else {
                continue;
            };
            fields.push(FormField::Color {
                id: slot_id(slot),
                label: format!("{}:", slot.title()),
                value: TerminalColor::rgb(red, green, blue),
                help: None,
            });
        }
        FormShow::new(FormShowParams {
            step: EDIT.to_owned(),
            title: headline(&format!(
                "The Theme {}",
                quoted(&settings::theme_title(from))
            )),
            note: Some(note),
            error: self.problem.clone(),
            busy: None,
            fields,
            buttons: vec![
                cancel,
                FormButton {
                    id: CONFIRM.to_owned(),
                    title: "Save".to_owned(),
                    role: Some(FormRole::Default),
                    enabled: true,
                },
            ],
        })
    }

    /// Puts what the host's fields held into the question's own. A well
    /// that holds what it was shown has not been touched, and its colour
    /// stays as the theme wrote it.
    fn fill(&mut self, used: &FormUsed) {
        if let Some(name) = used.value(NAME) {
            self.name = text::sanitize_line(name);
        }
        for slot in self.draft.colors.slots() {
            let picked = used
                .value(&slot_id(slot))
                .and_then(|value| TerminalColor::try_from(value.to_ascii_lowercase()).ok())
                .and_then(|color| rgb_of(color.as_str()));
            if let Some(picked) = picked
                && Some(picked) != self.shown(slot)
            {
                let (red, green, blue) = picked;
                self.draft
                    .colors
                    .set(slot, ColorSpec::Rgb(red, green, blue));
            }
        }
    }

    /// The change the question comes to, if it comes to one. `dark` is
    /// whether Quill is dark now: a new theme becomes the one in use.
    fn change(&self, dark: bool) -> Option<ThemesChange> {
        match &self.ask {
            ThemeAsk::Delete { name } => Some(ThemesChange::Delete { name: name.clone() }),
            ThemeAsk::Edit { from, .. } => {
                if self.refused().is_some() {
                    return None;
                }
                let name = self.name.trim().to_owned();
                let mut theme = self.draft.clone();
                theme.name.clone_from(&name);
                Some(ThemesChange::Save {
                    select: (name != *from).then_some(dark),
                    from: from.clone(),
                    theme,
                })
            }
        }
    }
}

/// The red, green and blue of a colour written `#rrggbb`.
fn rgb_of(color: &str) -> Option<(u8, u8, u8)> {
    let digits = color.strip_prefix('#')?;
    let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

impl State {
    /// Whether a theme can be changed or deleted from here: a host with
    /// sheets has the settings window open, and no such question is up.
    pub(super) fn themes_can_change(&self, env: &Env) -> bool {
        env.sheets && self.theme_prompt.is_none() && self.keys.is_none()
    }

    /// Something was asked of the list of themes in the settings window:
    /// one of its actions, of the row picked if of any.
    pub(super) fn on_theme_action(
        &mut self,
        env: &Env,
        read: &SettingsRead,
        action: &str,
        row: Option<&str>,
        out: &mut Effects,
    ) {
        // The host has the chooser, and says what was chosen when
        // something is.
        if action == settings::IMPORT_THEME {
            out.host.push(HostRequest::PickTheme);
            return;
        }
        if !self.themes_can_change(env) {
            return;
        }
        let Some(line) = row.and_then(|row| {
            read.themes
                .iter()
                .find(|line| settings::theme_id(&line.name) == row)
        }) else {
            return;
        };
        match (action, &line.theme) {
            (settings::EDIT_THEME, Ok(theme)) => {
                self.theme_prompt = Some(ThemePrompt::edit(line, theme, read, env.look.is_dark()));
            }
            (settings::DELETE_THEME, _) if line.own => {
                self.theme_prompt = Some(ThemePrompt::delete(line, read));
            }
            _ => {}
        }
    }

    /// Takes what was done with the question in the host's sheet: what its
    /// fields held goes into the question's own, and then the button does
    /// its work. With no button, a colour was picked, and the theme being
    /// made changes with it.
    pub(super) fn answer_theme_in_sheet(&mut self, env: &Env, used: &FormUsed, out: &mut Effects) {
        let Some(prompt) = &mut self.theme_prompt else {
            return;
        };
        let step = match prompt.ask {
            ThemeAsk::Edit { .. } => EDIT,
            ThemeAsk::Delete { .. } => DELETE,
        };
        if used.step() != step {
            return;
        }
        match used.button() {
            Some(id) if id == Press::Cancel.id() => self.theme_prompt = None,
            Some(CONFIRM) => {
                prompt.fill(used);
                // A name that cannot be had leaves the question up, saying
                // why.
                prompt.problem = prompt.refused();
                if let Some(change) = prompt.change(env.look.is_dark()) {
                    out.accounts.push(AccountRequest::Themes(change));
                    self.theme_prompt = None;
                }
            }
            Some(_) => {}
            None => prompt.fill(used),
        }
    }

    /// Has the theme being made drawn with, and the theme in force again
    /// when none is: asked of the run loop whenever the two differ.
    pub(super) fn follow_theme_draft(&self, env: &Env, out: &mut Effects) {
        let wanted = self.theme_prompt.as_ref().and_then(ThemePrompt::trying);
        if wanted != env.look.trying.as_ref() {
            out.trying = Some(wanted.cloned());
        }
    }
}
