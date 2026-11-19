//! The settings, as a settings window shows them.
//!
//! In a terminal the settings are a file. Quill's app has a window for the
//! ones a person changes most, and this is where that window is decided:
//! which settings it has, in which panes, what each is called, what it may
//! be set to and what is said beside it ([`describe`]), and what a thing
//! done in the window means ([`change`]). The app draws what is described
//! and reports what the user did. It knows a field by its kind and never
//! by what it is for, and it reads no file.
//!
//! Nothing here reads or writes a file either. Whoever starts the UI
//! supplies a service that does (`AccountRequest::Settings`): it makes the
//! change it is asked for, through the one place that edits `config.toml`,
//! and answers with what the file then says ([`SettingsRead`]).
//!
//! A setting takes effect at once where what the UI goes by can simply be
//! given the new value ([`Applied`]), and otherwise when Quill is next
//! started: the window says which, beside the setting ([`Setting::at_once`]).

use std::hash::{Hash, Hasher};

use quill_core::config::{
    AccountType, AgentConfig, AgentNotify, Appearance, ComposeEditor, ComposeFormat, Config,
    KeymapPreset, RemoteImages, ReplyPosition, SEND_GAP_LONGEST,
};
use quill_host::bridge::{
    FormAction, FormField, FormOption, FormRow, FormValue, MAX_FONT_NAME, MAX_FORM_ROWS, Number,
    SettingsPane, SettingsShow, SettingsShowParams, valid_form_id,
};

use quill_store::AgentEntry;

use crate::keymap::{KeySeq, Keymap};
use crate::state::Env;
use crate::text;
use crate::theme::Theme;
use crate::vim::ExternalEditor;

/// The settings the window has, as `config.toml` says them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether Quill is light, dark, or as the Mac is.
    pub appearance: Appearance,
    /// The theme while Quill is light, by name: `theme_light`, or `theme`
    /// where the file does not say.
    pub theme_light: String,
    /// The theme while Quill is dark, likewise.
    pub theme_dark: String,
    /// The font of the app's terminals, by its family's name, where the
    /// file names one.
    pub font: Option<String>,
    /// And its size, in points.
    pub font_size: Number,
    pub keymap: KeymapPreset,
    pub notifications: bool,
    pub threads: bool,
    pub web_reading: bool,
    pub editor: ComposeEditor,
    /// The program run as that editor, where the file names one.
    pub editor_path: Option<String>,
    pub format: ComposeFormat,
    pub reply_position: ReplyPosition,
    pub spell_check: bool,
    pub undo_send_seconds: u32,
    pub remote_images: RemoteImages,
    pub cache_gb: Number,
    /// What may be done to the mail from outside Quill's window.
    pub agent: AgentConfig,
    /// Whether rules run by themselves.
    pub rules_enabled: bool,
    /// How far back a rule goes over mail already here, as written.
    pub rules_look_back: String,
}

impl Settings {
    pub fn of(config: &Config) -> Self {
        Self {
            appearance: config.ui.appearance,
            theme_light: config.ui.theme_for(false).to_owned(),
            theme_dark: config.ui.theme_for(true).to_owned(),
            font: config.ui.font.clone(),
            font_size: font_size(config.ui.font_size),
            keymap: config.ui.keymap,
            notifications: config.ui.notifications,
            threads: config.ui.threads,
            web_reading: config.ui.web_reading,
            editor: config.compose.editor,
            editor_path: config.compose.editor_path.clone(),
            format: config.compose.format,
            reply_position: config.compose.reply_position,
            spell_check: config.compose.spell_check,
            undo_send_seconds: config.compose.undo_send_seconds,
            remote_images: config.privacy.remote_images,
            // A config that was read has a number here.
            cache_gb: Number::new(config.storage.cache_gb).unwrap_or(Number::from(0)),
            agent: config.agent.clone(),
            rules_enabled: config.ruling.enabled,
            rules_look_back: config.ruling.look_back.trim().to_lowercase(),
        }
    }
}

/// A size of font as the window has it: to one place after the point, as
/// the file's number is read.
pub fn font_size(points: f32) -> Number {
    Number::new(f64::from(points))
        .unwrap_or(Number::from(13))
        .rounded(FONT_DECIMALS)
}

/// The smallest and the largest a font may be, in points: what the
/// settings file is held to (`quill_core::config`).
pub const FONT_SMALLEST: u32 = quill_core::config::FONT_SMALLEST;
pub const FONT_LARGEST: u32 = quill_core::config::FONT_LARGEST;
const FONT_DECIMALS: u8 = 1;

/// One of the settings the window has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Appearance,
    ThemeLight,
    ThemeDark,
    Font,
    FontSize,
    Keymap,
    Notifications,
    Threads,
    WebReading,
    Editor,
    EditorPath,
    Format,
    ReplyPosition,
    SpellCheck,
    UndoSendSeconds,
    RemoteImages,
    CacheGb,
    AgentCommands,
    AgentMcp,
    AgentOrganize,
    AgentDelete,
    AgentDraft,
    AgentSend,
    AgentSendGap,
    AgentHoldSends,
    AgentNotify,
    RulesEnabled,
    RulesLookBack,
}

impl Setting {
    pub const ALL: [Setting; 28] = [
        Setting::Appearance,
        Setting::ThemeLight,
        Setting::ThemeDark,
        Setting::Font,
        Setting::FontSize,
        Setting::Keymap,
        Setting::Notifications,
        Setting::Threads,
        Setting::WebReading,
        Setting::Editor,
        Setting::EditorPath,
        Setting::Format,
        Setting::ReplyPosition,
        Setting::SpellCheck,
        Setting::UndoSendSeconds,
        Setting::RemoteImages,
        Setting::CacheGb,
        Setting::AgentCommands,
        Setting::AgentMcp,
        Setting::AgentOrganize,
        Setting::AgentDelete,
        Setting::AgentDraft,
        Setting::AgentSend,
        Setting::AgentSendGap,
        Setting::AgentHoldSends,
        Setting::AgentNotify,
        Setting::RulesEnabled,
        Setting::RulesLookBack,
    ];

    /// The key the file has it under, in its section. It is also the
    /// setting's name on the bridge, in its pane.
    pub fn key(self) -> &'static str {
        match self {
            Setting::Appearance => "appearance",
            Setting::ThemeLight => "theme_light",
            Setting::ThemeDark => "theme_dark",
            Setting::Font => "font",
            Setting::FontSize => "font_size",
            Setting::Keymap => "keymap",
            Setting::Notifications => "notifications",
            Setting::Threads => "threads",
            Setting::WebReading => "web_reading",
            Setting::Editor => "editor",
            Setting::EditorPath => "editor_path",
            Setting::Format => "format",
            Setting::ReplyPosition => "reply_position",
            Setting::SpellCheck => "spell_check",
            Setting::UndoSendSeconds => "undo_send_seconds",
            Setting::RemoteImages => "remote_images",
            Setting::CacheGb => "cache_gb",
            Setting::AgentCommands => "commands",
            Setting::AgentMcp => "mcp",
            Setting::AgentOrganize => "organize",
            Setting::AgentDelete => "delete",
            Setting::AgentDraft => "draft",
            Setting::AgentSend => "send",
            Setting::AgentSendGap => "send_gap",
            Setting::AgentHoldSends => "hold_sends",
            Setting::AgentNotify => "notify",
            Setting::RulesEnabled => "enabled",
            Setting::RulesLookBack => "look_back",
        }
    }

    /// The section of the file it is in.
    pub fn section(self) -> &'static str {
        match self {
            Setting::Appearance
            | Setting::ThemeLight
            | Setting::ThemeDark
            | Setting::Font
            | Setting::FontSize
            | Setting::Keymap
            | Setting::Notifications
            | Setting::Threads
            | Setting::WebReading => "ui",
            Setting::Editor
            | Setting::EditorPath
            | Setting::Format
            | Setting::ReplyPosition
            | Setting::SpellCheck
            | Setting::UndoSendSeconds => "compose",
            Setting::RemoteImages => "privacy",
            Setting::CacheGb => "storage",
            Setting::AgentCommands
            | Setting::AgentMcp
            | Setting::AgentOrganize
            | Setting::AgentDelete
            | Setting::AgentDraft
            | Setting::AgentSend
            | Setting::AgentSendGap
            | Setting::AgentHoldSends
            | Setting::AgentNotify => "agent",
            Setting::RulesEnabled | Setting::RulesLookBack => "rules",
        }
    }

    /// The pane of the window it is in.
    pub fn pane(self) -> Pane {
        match self {
            Setting::Appearance
            | Setting::ThemeLight
            | Setting::ThemeDark
            | Setting::Font
            | Setting::FontSize => Pane::Appearance,
            Setting::Keymap
            | Setting::Notifications
            | Setting::Threads
            | Setting::WebReading
            | Setting::RulesEnabled
            | Setting::RulesLookBack => Pane::General,
            Setting::Editor
            | Setting::EditorPath
            | Setting::Format
            | Setting::ReplyPosition
            | Setting::SpellCheck
            | Setting::UndoSendSeconds => Pane::Composing,
            Setting::RemoteImages => Pane::Privacy,
            Setting::CacheGb => Pane::Storage,
            Setting::AgentCommands
            | Setting::AgentMcp
            | Setting::AgentOrganize
            | Setting::AgentDelete
            | Setting::AgentDraft
            | Setting::AgentSend
            | Setting::AgentSendGap
            | Setting::AgentHoldSends
            | Setting::AgentNotify => Pane::Agents,
        }
    }

    /// Whether a change holds from the moment it is made. The others
    /// hold from when Quill is next started, and the window says so
    /// beside them.
    ///
    /// What a message is written in is read by every step of handing a
    /// message to an editor and taking it back, so it is not changed
    /// under a message that is out. The room for downloaded mail is given
    /// to the sync engine once, as it starts.
    pub fn at_once(self) -> bool {
        !matches!(
            self,
            Setting::Editor | Setting::EditorPath | Setting::CacheGb
        )
    }

    fn named(pane: &str, key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|setting| setting.pane().id() == pane && setting.key() == key)
    }
}

/// A pane of the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    General,
    Appearance,
    Accounts,
    Composing,
    Keys,
    Privacy,
    Agents,
    Storage,
}

impl Pane {
    pub const ALL: [Pane; 8] = [
        Pane::General,
        Pane::Appearance,
        Pane::Accounts,
        Pane::Composing,
        Pane::Keys,
        Pane::Privacy,
        Pane::Agents,
        Pane::Storage,
    ];

    /// The pane's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Pane::General => "general",
            Pane::Appearance => "appearance",
            Pane::Accounts => "accounts",
            Pane::Composing => "composing",
            Pane::Keys => "keys",
            Pane::Privacy => "privacy",
            Pane::Agents => "agents",
            Pane::Storage => "storage",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Appearance => "Appearance",
            Pane::Accounts => "Accounts",
            Pane::Composing => "Composing",
            Pane::Keys => "Keys",
            Pane::Privacy => "Privacy",
            Pane::Agents => "Agents",
            Pane::Storage => "Storage",
        }
    }

    /// The SF Symbol on the pane's toolbar button.
    pub fn symbol(self) -> &'static str {
        match self {
            Pane::General => "gearshape",
            Pane::Appearance => "paintpalette",
            Pane::Accounts => "at",
            Pane::Composing => "square.and.pencil",
            Pane::Keys => "keyboard",
            Pane::Privacy => "hand.raised",
            Pane::Agents => "sparkles",
            Pane::Storage => "internaldrive",
        }
    }
}

/// One setting given another value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Appearance(Appearance),
    /// The theme while Quill is light, by name.
    ThemeLight(String),
    /// The theme while Quill is dark, by name.
    ThemeDark(String),
    /// A theme to draw with from now on, by name: the one just imported.
    /// `dark` is whether Quill is dark as this is asked. Which key of the
    /// file that comes to is decided where the file is: `theme`, the one
    /// theme there is, for a file that has never named a theme for one
    /// appearance, and otherwise the key of the appearance in force.
    ThemeInForce {
        name: String,
        dark: bool,
    },
    /// The font of the app's terminals, by its family's name, or none
    /// named: the app's own.
    Font(Option<String>),
    /// Its size, in points.
    FontSize(Number),
    Keymap(KeymapPreset),
    Notifications(bool),
    Threads(bool),
    WebReading(bool),
    Editor(ComposeEditor),
    /// The program run as the editor, or none named: the one of the
    /// editor's name on the `PATH`.
    EditorPath(Option<String>),
    Format(ComposeFormat),
    ReplyPosition(ReplyPosition),
    SpellCheck(bool),
    UndoSendSeconds(u32),
    RemoteImages(RemoteImages),
    CacheGb(Number),
    /// The commands for a shell that touch mail.
    AgentCommands(bool),
    AgentMcp(bool),
    AgentOrganize(bool),
    AgentDelete(bool),
    AgentDraft(bool),
    AgentSend(bool),
    /// Seconds from one message agents send to the next.
    AgentSendGap(u32),
    AgentHoldSends(bool),
    AgentNotify(AgentNotify),
    /// Whether rules run by themselves.
    RulesEnabled(bool),
    /// How far back a rule goes over mail already here: `1d`, `1w`.
    RulesLookBack(String),
}

/// A setting's value as the file has it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Written<'a> {
    /// Nothing: the key is taken out of the file.
    Gone,
    Word(&'a str),
    Flag(bool),
    Whole(i64),
    Decimal(f64),
}

impl Change {
    pub fn setting(&self) -> Setting {
        match self {
            Change::Appearance(_) => Setting::Appearance,
            Change::ThemeLight(_) | Change::ThemeInForce { dark: false, .. } => Setting::ThemeLight,
            Change::ThemeDark(_) | Change::ThemeInForce { dark: true, .. } => Setting::ThemeDark,
            Change::Font(_) => Setting::Font,
            Change::FontSize(_) => Setting::FontSize,
            Change::Keymap(_) => Setting::Keymap,
            Change::Notifications(_) => Setting::Notifications,
            Change::Threads(_) => Setting::Threads,
            Change::WebReading(_) => Setting::WebReading,
            Change::Editor(_) => Setting::Editor,
            Change::EditorPath(_) => Setting::EditorPath,
            Change::Format(_) => Setting::Format,
            Change::ReplyPosition(_) => Setting::ReplyPosition,
            Change::SpellCheck(_) => Setting::SpellCheck,
            Change::UndoSendSeconds(_) => Setting::UndoSendSeconds,
            Change::RemoteImages(_) => Setting::RemoteImages,
            Change::CacheGb(_) => Setting::CacheGb,
            Change::AgentCommands(_) => Setting::AgentCommands,
            Change::AgentMcp(_) => Setting::AgentMcp,
            Change::AgentOrganize(_) => Setting::AgentOrganize,
            Change::AgentDelete(_) => Setting::AgentDelete,
            Change::AgentDraft(_) => Setting::AgentDraft,
            Change::AgentSend(_) => Setting::AgentSend,
            Change::AgentSendGap(_) => Setting::AgentSendGap,
            Change::AgentHoldSends(_) => Setting::AgentHoldSends,
            Change::AgentNotify(_) => Setting::AgentNotify,
            Change::RulesEnabled(_) => Setting::RulesEnabled,
            Change::RulesLookBack(_) => Setting::RulesLookBack,
        }
    }

    /// What is written in the file for it. A number that is a whole one
    /// is written as one: `cache_gb = 5`, as a person writes it.
    pub fn written(&self) -> Written<'_> {
        match self {
            Change::Appearance(appearance) => Written::Word(appearance.as_str()),
            Change::ThemeLight(name)
            | Change::ThemeDark(name)
            | Change::ThemeInForce { name, .. } => Written::Word(name),
            Change::Keymap(preset) => Written::Word(preset.as_str()),
            Change::Editor(editor) => Written::Word(editor.as_str()),
            Change::EditorPath(Some(path)) => Written::Word(path),
            Change::EditorPath(None) | Change::Font(None) => Written::Gone,
            Change::Font(Some(family)) => Written::Word(family),
            Change::Format(format) => Written::Word(format.as_str()),
            Change::ReplyPosition(position) => Written::Word(position.as_str()),
            Change::RemoteImages(remote) => Written::Word(remote.as_str()),
            Change::AgentNotify(notify) => Written::Word(notify.as_str()),
            Change::RulesLookBack(age) => Written::Word(age),
            Change::Notifications(on)
            | Change::Threads(on)
            | Change::WebReading(on)
            | Change::SpellCheck(on)
            | Change::AgentCommands(on)
            | Change::AgentMcp(on)
            | Change::AgentOrganize(on)
            | Change::AgentDelete(on)
            | Change::AgentDraft(on)
            | Change::AgentSend(on)
            | Change::AgentHoldSends(on)
            | Change::RulesEnabled(on) => Written::Flag(*on),
            Change::UndoSendSeconds(seconds) | Change::AgentSendGap(seconds) => {
                Written::Whole(i64::from(*seconds))
            }
            Change::CacheGb(number) | Change::FontSize(number) => match number.whole() {
                Some(whole) => Written::Whole(whole),
                None => Written::Decimal(number.get()),
            },
        }
    }
}

/// An account, as the file has it and as far as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLine {
    pub name: String,
    pub kind: AccountType,
    pub address: String,
    /// It signs in with its provider (Microsoft, Google) and not with a
    /// password.
    pub signs_in: bool,
}

/// A theme there is to choose, as the window lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeLine {
    pub name: String,
    /// It is a file in the user's `themes` folder: theirs to change and
    /// to delete. One Quill comes with is not, unless a file of theirs
    /// has its name and stands in its place.
    pub own: bool,
    /// The theme, ready to draw with, or why its file cannot be used.
    pub theme: Result<Theme, String>,
}

impl ThemeLine {
    /// The themes Quill comes with, as they are listed where the user has
    /// none of their own.
    pub fn builtins() -> Vec<Self> {
        Theme::builtin_names()
            .filter_map(|name| {
                Some(Self {
                    name: name.to_owned(),
                    own: false,
                    theme: Ok(Theme::builtin(name)?),
                })
            })
            .collect()
    }
}

impl SettingsRead {
    /// The names of the themes there are to choose from.
    pub fn theme_names(&self) -> Vec<&str> {
        self.themes.iter().map(|line| line.name.as_str()).collect()
    }
}

/// What the settings file says, read a moment ago: the answer to
/// `AccountRequest::Settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRead {
    pub settings: Settings,
    pub accounts: Vec<AccountLine>,
    /// The themes there are to choose from: the ones Quill comes with,
    /// then the user's own.
    pub themes: Vec<ThemeLine>,
    /// The theme the file names for when Quill is light, ready to draw
    /// with, or the one that stands in for it.
    pub theme_light: Theme,
    /// Why another theme stands in for the one named, if one does.
    pub theme_light_problem: Option<String>,
    /// The same for when Quill is dark.
    pub theme_dark: Theme,
    pub theme_dark_problem: Option<String>,
    /// The keys of the preset the file names, with the user's own on top.
    pub keymap: Keymap,
    /// Why the user's own keys were left out, if they were.
    pub keys_problem: Option<String>,
}

/// The settings that hold from the moment they are read, for the run loop
/// to put into [`Env`]: `update` decides, and changes nothing but the
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub appearance: Appearance,
    pub theme_light: Theme,
    pub theme_dark: Theme,
    pub font: Option<String>,
    pub font_size: Number,
    pub keymap: Keymap,
    pub notifications: bool,
    pub threads: bool,
    pub web_reading: bool,
    pub markdown: bool,
    pub reply_above: bool,
    pub undo_send_seconds: u32,
    pub spell_check: bool,
    pub remote_never: bool,
    pub agent: AgentConfig,
}

impl Applied {
    pub fn of(read: &SettingsRead) -> Self {
        let settings = &read.settings;
        Self {
            appearance: settings.appearance,
            theme_light: read.theme_light.clone(),
            theme_dark: read.theme_dark.clone(),
            font: settings.font.clone(),
            font_size: settings.font_size,
            keymap: read.keymap.clone(),
            notifications: settings.notifications,
            threads: settings.threads,
            web_reading: settings.web_reading,
            markdown: settings.format == ComposeFormat::Markdown,
            reply_above: settings.reply_position == ReplyPosition::Above,
            undo_send_seconds: settings.undo_send_seconds,
            spell_check: settings.spell_check,
            remote_never: settings.remote_images == RemoteImages::Never,
            agent: settings.agent.clone(),
        }
    }

    /// Puts them into `env`. Returns whether the keys changed: the host
    /// is then told every command's keys again, for its menus.
    ///
    /// What a message is written in and the room for downloaded mail are
    /// no part of this: they hold from when Quill is next started.
    pub fn apply(self, env: &mut Env) -> bool {
        let keys_changed = env.keymap != self.keymap;
        // What the host says the system is stays as it was said.
        env.look.appearance = self.appearance;
        env.look.light = self.theme_light;
        env.look.dark = self.theme_dark;
        env.restyle();
        env.font = self.font;
        env.font_size = Some(self.font_size);
        env.keymap = self.keymap;
        env.notifications = self.notifications;
        env.threads = self.threads;
        env.web_reading = self.web_reading;
        env.compose.markdown = self.markdown;
        env.compose.reply_above = self.reply_above;
        env.compose.undo_send_seconds = self.undo_send_seconds;
        env.compose.spell_check = self.spell_check;
        env.remote_never = self.remote_never;
        env.agent = self.agent;
        keys_changed
    }
}

/// What can be done from the accounts pane right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Can {
    /// There is a service to add an account through, and to sign in with:
    /// not so for the demo mailbox.
    pub accounts: bool,
    /// An account can be added now: what the command for it says.
    pub add: bool,
    /// An account can be signed in to now, and no sign-in is under way.
    pub sign_in: bool,
    /// An account can be asked to be removed now: nothing that has an
    /// account in hand is under way.
    pub remove: bool,
    /// An account can be asked to be changed now.
    pub edit: bool,
    /// A theme can be changed or deleted now: the host asks in a sheet,
    /// and no such question is up.
    pub themes: bool,
}

/// How many themes are listed before the list is one to search.
const MANY_THEMES: usize = 8;

/// The name of the list of themes, in the appearance pane.
pub const THEMES: &str = "themes";
/// The actions of that list.
pub const EDIT_THEME: &str = "edit";
pub const DELETE_THEME: &str = "delete";
pub const IMPORT_THEME: &str = "import";
/// The name of the list of accounts, in its pane.
pub const ACCOUNTS: &str = "accounts";
/// The actions of that list.
pub const ADD: &str = "add";
pub const SIGN_IN: &str = "sign_in";
pub const REMOVE: &str = "remove";
pub const EDIT: &str = "edit";

/// Said at the foot of every pane, beside the button that opens the
/// settings file.
const MORE: &str = "Saved searches are set in the settings file. Rules, hooks and templates \
                    are files beside it.";
/// Said beside a setting that holds from the next start.
const NEXT_START: &str = "Takes effect when Quill is next opened.";

/// A name on the bridge for something the user named: the name itself
/// where it can be one, and otherwise one made from it.
fn name_id(prefix: &str, name: &str) -> String {
    if prefix.is_empty() && valid_form_id(name) && !name.starts_with("x_") {
        return name.to_owned();
    }
    let mut hasher = std::hash::DefaultHasher::new();
    name.hash(&mut hasher);
    format!(
        "{}_{:016x}",
        if prefix.is_empty() { "x" } else { prefix },
        hasher.finish()
    )
}

/// The name on the bridge of the theme called `name`.
pub fn theme_id(name: &str) -> String {
    name_id("", name)
}

/// The name on the bridge of the row of the account called `name`.
pub fn account_id(name: &str) -> String {
    name_id("account", name)
}

/// A theme's name as a person reads it: the ones Quill comes with are
/// given a capital, and the user's own are left as they named them.
pub(crate) fn theme_title(name: &str) -> String {
    if Theme::builtin_names().any(|builtin| builtin == name) {
        let mut letters = name.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default()
    } else {
        text::sanitize_line(name)
    }
}

fn option(id: impl Into<String>, title: impl Into<String>) -> FormOption {
    FormOption {
        id: id.into(),
        title: title.into(),
    }
}

fn choice(
    setting: Setting,
    label: &str,
    value: impl Into<String>,
    options: Vec<FormOption>,
    help: Option<String>,
) -> FormField {
    FormField::Choice {
        id: setting.key().to_owned(),
        label: label.to_owned(),
        value: value.into(),
        options,
        focus: false,
        help,
    }
}

fn toggle(setting: Setting, label: &str, title: &str, value: bool) -> FormField {
    FormField::Toggle {
        id: setting.key().to_owned(),
        label: label.to_owned(),
        title: title.to_owned(),
        value,
        help: None,
    }
}

/// What an editor is called in the window.
fn editor_title(editor: ComposeEditor) -> &'static str {
    match editor {
        ComposeEditor::Builtin => "Quill\u{2019}s own editor",
        ComposeEditor::Vim => "Vim",
        ComposeEditor::Nano => "nano",
        ComposeEditor::Pico => "Pico",
        ComposeEditor::Mg => "mg",
    }
}

/// The editors on offer: the same wherever Quill is running.
pub fn editors_offered() -> Vec<ComposeEditor> {
    ComposeEditor::ALL.to_vec()
}

/// The name on the `PATH` of the program an editor is, when none is named.
fn editor_program(editor: ComposeEditor) -> Option<&'static str> {
    match editor {
        ComposeEditor::Builtin => None,
        ComposeEditor::Vim => Some(ExternalEditor::Vim.program()),
        ComposeEditor::Nano => Some(ExternalEditor::Nano.program()),
        ComposeEditor::Pico => Some(ExternalEditor::Pico.program()),
        ComposeEditor::Mg => Some(ExternalEditor::Mg.program()),
    }
}

/// Whether `path` can name the program an editor is: a whole path, one
/// from the home folder, or a name to look for. As the settings file is
/// read ([`quill_core::config`]).
fn names_a_program(path: &str) -> bool {
    let whole = path.starts_with('/') || path.starts_with("~/");
    !path.contains('\0') && (whole || !path.contains('/'))
}

/// What messages are written in for as long as Quill runs: what the file
/// said as it started.
fn editor_running(env: &Env) -> ComposeEditor {
    if !env.compose.vim {
        return ComposeEditor::Builtin;
    }
    match env.external {
        ExternalEditor::Vim => ComposeEditor::Vim,
        ExternalEditor::Nano => ComposeEditor::Nano,
        ExternalEditor::Pico => ComposeEditor::Pico,
        ExternalEditor::Mg => ComposeEditor::Mg,
    }
}

/// A number of gigabytes as a person writes it: `5`, `0.5`.
fn gigabytes(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    format!("{rounded}")
}

fn kind_title(kind: AccountType) -> &'static str {
    match kind {
        AccountType::Imap => "IMAP",
        AccountType::Pop3 => "POP3",
        AccountType::Graph => "Microsoft 365",
        AccountType::Ews => "Exchange",
    }
}

/// The pop-up of themes for one appearance: every theme there is, with
/// the one the file names chosen. Under it, why another stands in for
/// that one if one does, and otherwise `help`.
fn theme_choice(
    setting: Setting,
    label: &str,
    read: &SettingsRead,
    named: &str,
    problem: Option<&String>,
    help: Option<&str>,
) -> FormField {
    // The theme the file names is always among the choices, so that what
    // is shown as chosen is what the file says, whether or not there is
    // such a theme.
    let mut themes: Vec<FormOption> = read
        .themes
        .iter()
        .map(|line| option(theme_id(&line.name), theme_title(&line.name)))
        .collect();
    if !read.themes.iter().any(|line| line.name == named) {
        themes.push(option(
            theme_id(named),
            format!("{} (not found)", text::sanitize_line(named)),
        ));
    }
    choice(
        setting,
        label,
        theme_id(named),
        themes,
        problem
            .map(|problem| text::sanitize_line(problem))
            .or_else(|| help.map(str::to_owned)),
    )
}

/// What is said of a theme under its name in the list of them.
fn theme_detail(line: &ThemeLine, settings: &Settings) -> String {
    let theme = match &line.theme {
        Ok(theme) => theme,
        Err(problem) => {
            return format!(
                "Cannot be used. {}",
                sentence(&text::sanitize_line(problem))
            );
        }
    };
    let mut said = vec![match theme.is_dark() {
        Some(true) => "Dark",
        Some(false) => "Light",
        None => "The terminal\u{2019}s own colours",
    }];
    let builtin = Theme::builtin_names().any(|builtin| builtin == line.name);
    match (builtin, line.own) {
        (true, false) => said.push("comes with Quill"),
        (true, true) => said.push("Quill\u{2019}s, changed by you"),
        (false, _) => {}
    }
    match (
        line.name == settings.theme_light,
        line.name == settings.theme_dark,
    ) {
        (true, true) => said.push("the theme in use"),
        (true, false) => said.push("the light theme"),
        (false, true) => said.push("the dark theme"),
        (false, false) => {}
    }
    said.join(" \u{b7} ")
}

fn appearance(read: &SettingsRead, env: &Env, can: Can) -> Vec<FormField> {
    let settings = &read.settings;
    // What "as the Mac is" comes to now, where the host has said.
    let system = match env.look.system_dark {
        Some(true) => " The Mac is dark now.",
        Some(false) => " The Mac is light now.",
        None => "",
    };
    vec![
        choice(
            Setting::Appearance,
            "Appearance:",
            settings.appearance.as_str(),
            vec![
                option(Appearance::System.as_str(), "System"),
                option(Appearance::Light.as_str(), "Light"),
                option(Appearance::Dark.as_str(), "Dark"),
            ],
            Some(format!(
                "System follows the Mac, and changes with it.{system}"
            )),
        ),
        theme_choice(
            Setting::ThemeLight,
            "Light theme:",
            read,
            &settings.theme_light,
            read.theme_light_problem.as_ref(),
            None,
        ),
        theme_choice(
            Setting::ThemeDark,
            "Dark theme:",
            read,
            &settings.theme_dark,
            read.theme_dark_problem.as_ref(),
            None,
        ),
        FormField::Font {
            id: Setting::Font.key().to_owned(),
            label: "Font:".to_owned(),
            value: settings.font.clone().unwrap_or_default(),
            help: None,
        },
        FormField::Number {
            id: Setting::FontSize.key().to_owned(),
            label: "Size:".to_owned(),
            value: settings.font_size,
            min: Number::from(FONT_SMALLEST),
            max: Some(Number::from(FONT_LARGEST)),
            step: Number::from(1),
            decimals: FONT_DECIMALS,
            unit: Some("points".to_owned()),
            help: Some(
                "The font of Quill\u{2019}s own window, and of an editor in it. In a terminal \
                 Quill has the terminal\u{2019}s font."
                    .to_owned(),
            ),
        },
        FormField::List {
            id: THEMES.to_owned(),
            label: "Themes:".to_owned(),
            // The window lists so many and no more; the pop-ups above
            // have every one.
            rows: read
                .themes
                .iter()
                .take(MAX_FORM_ROWS)
                .map(|line| {
                    let mut actions = Vec::new();
                    if line.theme.is_ok() {
                        actions.push(EDIT_THEME.to_owned());
                    }
                    if line.own {
                        actions.push(DELETE_THEME.to_owned());
                    }
                    FormRow {
                        id: theme_id(&line.name),
                        title: theme_title(&line.name),
                        detail: Some(theme_detail(line, settings)),
                        actions,
                    }
                })
                .collect(),
            actions: vec![
                FormAction {
                    id: EDIT_THEME.to_owned(),
                    title: "Edit\u{2026}".to_owned(),
                    row: true,
                    enabled: can.themes,
                },
                FormAction {
                    id: DELETE_THEME.to_owned(),
                    title: "Delete\u{2026}".to_owned(),
                    row: true,
                    enabled: can.themes,
                },
                FormAction {
                    id: IMPORT_THEME.to_owned(),
                    title: "Import\u{2026}".to_owned(),
                    row: false,
                    enabled: true,
                },
            ],
            // Past a handful there is a line to type on over them, and
            // more room.
            searchable: read.themes.len() > MANY_THEMES,
            empty: None,
            help: Some(
                "Edit\u{2026} changes a theme\u{2019}s colours, and Quill is drawn in them as they \
                 are chosen. Saved under another name it is a new theme, and the one it was \
                 made from is left as it was. Themes made for Ghostty can be imported. Your \
                 own themes are files in the themes folder beside the settings file."
                    .to_owned(),
            ),
        },
    ]
}

fn general(read: &SettingsRead) -> Vec<FormField> {
    let settings = &read.settings;
    vec![
        choice(
            Setting::Keymap,
            "Keys:",
            settings.keymap.as_str(),
            vec![
                option(KeymapPreset::Vim.as_str(), "Vim"),
                option(KeymapPreset::Mutt.as_str(), "Mutt"),
            ],
            Some(match &read.keys_problem {
                Some(problem) => text::sanitize_line(problem),
                None => "Keys of your own in keys.toml stay on top of either.".to_owned(),
            }),
        ),
        toggle(
            Setting::Notifications,
            "New mail:",
            "Show a notification when mail arrives",
            settings.notifications,
        ),
        toggle(
            Setting::Threads,
            "Message list:",
            "List the messages of a conversation together",
            settings.threads,
        ),
        toggle(
            Setting::WebReading,
            "Reading:",
            "Read mail in a web pane beside the list",
            settings.web_reading,
        ),
        FormField::Toggle {
            id: Setting::RulesEnabled.key().to_owned(),
            label: "Rules:".to_owned(),
            title: "Run rules on mail as it arrives".to_owned(),
            value: settings.rules_enabled,
            help: Some(
                "Rules are written in rules.toml: Rules > Edit Rules opens it. Switching \
                 them on catches up on the mail that came while they were off."
                    .to_owned(),
            ),
        },
        choice(
            Setting::RulesLookBack,
            "Look back:",
            look_back_id(&settings.rules_look_back),
            look_backs(&settings.rules_look_back),
            Some(
                "How far back a rule goes over mail already in the inbox, when rules \
                 catch up or are run by hand. A rule can say otherwise for itself."
                    .to_owned(),
            ),
        ),
    ]
}

/// The look-backs the window offers, with what the file has among them
/// if it is none of these.
const LOOK_BACKS: [(&str, &str); 5] = [
    ("1d", "A day"),
    ("3d", "3 days"),
    ("1w", "A week"),
    ("2w", "2 weeks"),
    ("30d", "30 days"),
];

/// The name on the bridge of a look-back, from how it is written: a name
/// there starts with a letter, and `1w` does not.
fn look_back_id(written: &str) -> String {
    format!("back_{written}")
}

fn look_backs(written: &str) -> Vec<FormOption> {
    let mut options: Vec<FormOption> = LOOK_BACKS
        .into_iter()
        .map(|(written, title)| option(look_back_id(written), title))
        .collect();
    let id = look_back_id(written);
    if !LOOK_BACKS.iter().any(|(known, _)| *known == written) && valid_form_id(&id) {
        options.push(option(id, written));
    }
    options
}

/// The name of the list of commands and their keys, in its pane.
pub const BINDINGS: &str = "bindings";
/// The actions of that list.
pub const CHANGE_KEY: &str = "change";
pub const DEFAULT_KEY: &str = "reset";
pub const RESET_KEYS: &str = "reset_all";

/// What a command with no key is said to have.
pub const NO_KEY: &str = "No key";

/// The name on the bridge of a command's row in the list of keys: its own
/// name, with nothing in it a name there cannot have.
pub fn binding_id(command: &str) -> String {
    command.replace('.', "_")
}

/// The keys a command has, as they are written: `j, down`.
pub fn keys_written(keys: &[KeySeq]) -> String {
    if keys.is_empty() {
        return NO_KEY.to_owned();
    }
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A change to the user's own keys, for `keys.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysChange {
    /// Give each of these commands, by name, exactly these keys. A
    /// command given none is left with no key.
    Set(Vec<(String, Vec<String>)>),
    /// Take what the file says of these commands out of it: each goes
    /// back to the keys of the preset.
    Reset(Vec<String>),
    /// Take everything out of the file: every key is the preset's again.
    ResetAll,
}

/// A change to the user's own themes, in the `themes` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemesChange {
    /// Save `theme` under the name it has. It was made from the theme
    /// listed as `from`: under that name it takes its place, and under
    /// another it is a new theme, written as `from` is where that is one
    /// of Quill's own files, with what is written beside its colours
    /// kept. `select` says the theme is to be the one in use from now,
    /// and whether Quill is dark as that is asked.
    Save {
        from: String,
        theme: Theme,
        select: Option<bool>,
    },
    /// Delete the theme of this name: its file, and the lines of the
    /// settings file that name it.
    Delete { name: String },
}

/// Every command with the keys it has, to look through and change. A
/// command whose keys are not the preset's says so, and can be put back.
fn keys(read: &SettingsRead, env: &Env) -> Vec<FormField> {
    let preset = Keymap::preset(read.settings.keymap);
    // A file that could not be read is not written over: nothing here
    // can be changed until it is put right by hand.
    let editable = read.keys_problem.is_none();
    let mut any_changed = false;
    let rows = env
        .registry
        .specs()
        .iter()
        .map(|spec| {
            let now = read.keymap.keys_for(spec.id);
            let changed = now != preset.keys_for(spec.id);
            any_changed |= changed;
            let mut actions = Vec::new();
            if editable {
                actions.push(CHANGE_KEY.to_owned());
                if changed {
                    actions.push(DEFAULT_KEY.to_owned());
                }
            }
            FormRow {
                id: binding_id(spec.name),
                title: spec.title.to_owned(),
                detail: Some(if changed {
                    format!("{} \u{b7} yours", keys_written(now))
                } else {
                    keys_written(now)
                }),
                actions,
            }
        })
        .collect();
    vec![FormField::List {
        id: BINDINGS.to_owned(),
        label: String::new(),
        rows,
        actions: vec![
            FormAction {
                id: CHANGE_KEY.to_owned(),
                title: "Change\u{2026}".to_owned(),
                row: true,
                enabled: editable,
            },
            FormAction {
                id: DEFAULT_KEY.to_owned(),
                title: "Use the Default".to_owned(),
                row: true,
                enabled: editable,
            },
            FormAction {
                id: RESET_KEYS.to_owned(),
                title: "Reset All\u{2026}".to_owned(),
                row: false,
                enabled: editable && any_changed,
            },
        ],
        searchable: true,
        empty: None,
        help: Some(format!(
            "The keys are the {} keys (General), with your own on top. A key changed here \
             is written to keys.toml, beside the settings file, and holds at once. A key \
             with \u{2318} works in the app and not in a terminal, which keeps \u{2318} \
             for itself.",
            match read.settings.keymap {
                KeymapPreset::Vim => "Vim",
                KeymapPreset::Mutt => "Mutt",
            }
        )),
    }]
}

fn accounts(read: &SettingsRead, can: Can) -> Vec<FormField> {
    let rows = read
        .accounts
        .iter()
        .map(|account| FormRow {
            id: account_id(&account.name),
            title: text::sanitize_line(&account.name),
            detail: Some(format!(
                "{}, {}. {}",
                kind_title(account.kind),
                text::sanitize_line(&account.address),
                match (account.signs_in, account.kind) {
                    (true, AccountType::Graph) => "Signs in with Microsoft.",
                    (true, _) => "Signs in with its provider.",
                    (false, _) => "Signs in with a password.",
                }
            )),
            // What the form can change is an account's servers and its
            // password: one that signs in with its provider has neither
            // to show.
            actions: match (account.signs_in, account.kind) {
                (true, _) => vec![SIGN_IN.to_owned(), REMOVE.to_owned()],
                (false, AccountType::Imap) => vec![EDIT.to_owned(), REMOVE.to_owned()],
                (false, _) => vec![REMOVE.to_owned()],
            },
        })
        .collect();
    vec![FormField::List {
        id: ACCOUNTS.to_owned(),
        label: String::new(),
        rows,
        actions: vec![
            FormAction {
                id: ADD.to_owned(),
                title: "Add an Account\u{2026}".to_owned(),
                row: false,
                enabled: can.accounts && can.add,
            },
            FormAction {
                id: SIGN_IN.to_owned(),
                title: "Sign In\u{2026}".to_owned(),
                row: true,
                enabled: can.accounts && can.sign_in,
            },
            FormAction {
                id: EDIT.to_owned(),
                title: "Edit\u{2026}".to_owned(),
                row: true,
                enabled: can.accounts && can.edit,
            },
            FormAction {
                id: REMOVE.to_owned(),
                title: "Remove\u{2026}".to_owned(),
                row: true,
                enabled: can.accounts && can.remove,
            },
        ],
        searchable: false,
        empty: Some("No accounts yet.".to_owned()),
        help: Some(
            "Edit\u{2026} changes an account\u{2019}s servers, the name it signs in with and its \
             password. Removing an account takes it out of the settings file and the \
             Keychain, and deletes the copy of its mail kept on this Mac. Mail on its \
             server is not touched."
                .to_owned(),
        ),
    }]
}

fn composing(read: &SettingsRead, env: &Env) -> Vec<FormField> {
    let settings = &read.settings;
    // What the file says is always among the choices, as with the theme.
    let editors = editors_offered();
    let running = editor_running(env);
    let mut editor_help = format!(
        "An editor other than Quill\u{2019}s own is yours, with your own settings for \
         it: Vim reads your vimrc. {NEXT_START}"
    );
    if running != settings.editor {
        editor_help.push_str(&format!(
            " Until then messages are written in {}.",
            match running {
                ComposeEditor::Builtin => "Quill\u{2019}s own editor",
                other => editor_title(other),
            }
        ));
    }
    let mut fields = vec![choice(
        Setting::Editor,
        "Write in:",
        settings.editor.as_str(),
        editors
            .into_iter()
            .map(|editor| option(editor.as_str(), editor_title(editor)))
            .collect(),
        Some(editor_help),
    )];
    // Which program that editor is, for one that is not Quill's own.
    if let Some(program) = editor_program(settings.editor) {
        // Quill's app is started by the Mac, not by a shell, and has the
        // Mac's own PATH: the app looks in Homebrew's folders after it.
        let looked = if env.editor_pane {
            "in the Mac\u{2019}s own folders and then Homebrew\u{2019}s"
        } else {
            "on your PATH"
        };
        fields.push(FormField::Text {
            id: Setting::EditorPath.key().to_owned(),
            label: "Program:".to_owned(),
            value: settings.editor_path.clone().unwrap_or_default(),
            focus: false,
            help: Some(format!(
                "Left empty, Quill runs the {program} it finds {looked}. A whole path \
                 runs that program instead, such as /opt/homebrew/bin/nvim. {NEXT_START}"
            )),
        });
    }
    fields.extend([
        choice(
            Setting::Format,
            "Format:",
            settings.format.as_str(),
            vec![
                option(
                    ComposeFormat::Markdown.as_str(),
                    "Markdown, sent as formatted text",
                ),
                option(ComposeFormat::Plain.as_str(), "Plain text"),
            ],
            None,
        ),
        choice(
            Setting::ReplyPosition,
            "Replies:",
            settings.reply_position.as_str(),
            vec![
                option(ReplyPosition::Above.as_str(), "Above the quoted message"),
                option(ReplyPosition::Below.as_str(), "Below the quoted message"),
            ],
            None,
        ),
        toggle(
            Setting::SpellCheck,
            "Spelling:",
            "Check spelling as you type",
            settings.spell_check,
        ),
        FormField::Number {
            id: Setting::UndoSendSeconds.key().to_owned(),
            label: "Undo send:".to_owned(),
            value: Number::from(settings.undo_send_seconds),
            min: Number::from(0),
            max: Some(Number::from(u32::MAX)),
            step: Number::from(1),
            decimals: 0,
            unit: Some("seconds".to_owned()),
            help: Some(
                "How long a sent message waits in the Outbox, where it can be taken back. \
                 0 sends at once."
                    .to_owned(),
            ),
        },
    ]);
    fields
}

fn privacy(read: &SettingsRead) -> Vec<FormField> {
    let remote = read.settings.remote_images;
    let mut options = vec![
        option(RemoteImages::Ask.as_str(), "Ask for each message"),
        option(RemoteImages::Never.as_str(), "Never load them"),
    ];
    // Only what the file already says: it is read, and does nothing that
    // asking does not.
    if remote == RemoteImages::Allowlist {
        options.push(option(
            RemoteImages::Allowlist.as_str(),
            "Allowlist (as the settings file says)",
        ));
    }
    vec![choice(
        Setting::RemoteImages,
        "Remote pictures:",
        remote.as_str(),
        options,
        Some(
            "Pictures and other content a message would fetch from the internet. Asked \
             for, nothing is fetched until you say so for a message or for its sender."
                .to_owned(),
        ),
    )]
}

fn storage(read: &SettingsRead, env: &Env) -> Vec<FormField> {
    let cache = read.settings.cache_gb;
    let mut help = format!(
        "Past that the oldest downloaded copies are let go, and fetched again when next \
         opened. 0 is no limit. Mail of a POP3 account is never cleared. {NEXT_START}"
    );
    if Number::new(env.cache_gb).is_some_and(|running| running != cache) {
        help.push_str(&if env.cache_gb > 0.0 {
            format!(" Until then Quill keeps to {} GB.", gigabytes(env.cache_gb))
        } else {
            " Until then there is no limit.".to_owned()
        });
    }
    vec![FormField::Number {
        id: Setting::CacheGb.key().to_owned(),
        label: "Keep up to:".to_owned(),
        value: cache,
        min: Number::from(0),
        max: None,
        step: Number::from(1),
        decimals: CACHE_DECIMALS,
        unit: Some("GB of downloaded mail".to_owned()),
        help: Some(help),
    }]
}

/// The list of what agents changed last, in its pane, and what can be
/// done from it.
pub const AGENT_LOG: &str = "recent";
/// Stop every agent at once.
pub const STOP_AGENTS: &str = "stop";
/// Copy the command that adds Quill to an agent's program.
pub const COPY_MCP: &str = "copy_mcp";

/// The command an agent's program is given to reach Quill over MCP: this
/// Quill's own program, by its whole path, and `mcp`.
pub fn mcp_command(env: &Env) -> String {
    let program = env
        .program
        .as_deref()
        .and_then(std::path::Path::to_str)
        .unwrap_or("quill");
    // A path with a space in it is quoted, as a shell wants it.
    if program.contains(' ') {
        format!("\"{program}\" mcp")
    } else {
        format!("{program} mcp")
    }
}

/// What programs outside Quill's window may do with the mail: the two
/// ways in, what may be done through them, how sending goes, and what
/// was done last.
fn agents(read: &SettingsRead, env: &Env, recent: &[AgentEntry]) -> Vec<FormField> {
    let agent = &read.settings.agent;
    let mut fields = vec![
        toggle(
            Setting::AgentCommands,
            "Shell commands:",
            "Let quill\u{2019}s commands work on the mail",
            agent.commands,
        ),
        FormField::Toggle {
            id: Setting::AgentMcp.key().to_owned(),
            label: "MCP:".to_owned(),
            title: "Serve agents over MCP while Quill is running".to_owned(),
            value: agent.mcp,
            help: Some(format!(
                "An agent\u{2019}s program is given this command to reach Quill: {}",
                mcp_command(env)
            )),
        },
        toggle(
            Setting::AgentOrganize,
            "They may:",
            "Organize mail: mark, move, archive, snooze",
            agent.organize,
        ),
        toggle(
            Setting::AgentDelete,
            "",
            "Delete mail: to the trash, never for good",
            agent.delete,
        ),
        toggle(Setting::AgentDraft, "", "Write drafts", agent.draft),
        toggle(Setting::AgentSend, "", "Send mail", agent.send),
    ];
    // How sending goes is decided when it is switched on, and is not in
    // the way before.
    if agent.send {
        fields.push(FormField::Number {
            id: Setting::AgentSendGap.key().to_owned(),
            label: "Wait:".to_owned(),
            value: Number::from(agent.send_gap),
            min: Number::from(0),
            max: Some(Number::from(SEND_GAP_LONGEST)),
            step: Number::from(5),
            decimals: 0,
            unit: Some("seconds between messages".to_owned()),
            help: Some(
                "What they send goes one message at a time, each this long after the one \
                 before. A message that has to wait is in the Outbox, where it can be taken \
                 back. 0 is no wait."
                    .to_owned(),
            ),
        });
        fields.push(FormField::Toggle {
            id: Setting::AgentHoldSends.key().to_owned(),
            label: String::new(),
            title: "Hold what they send for my OK".to_owned(),
            value: agent.hold_sends,
            help: Some(
                "Held mail waits in the Outbox and does not go until you send it from there."
                    .to_owned(),
            ),
        });
    }
    fields.push(choice(
        Setting::AgentNotify,
        "Tell me:",
        agent.notify.as_str(),
        vec![
            option(
                AgentNotify::Changes.as_str(),
                "When they change or send mail",
            ),
            option(AgentNotify::Sends.as_str(), "When they send mail"),
            option(AgentNotify::None.as_str(), "Nothing of what they do"),
        ],
        Some(
            "An agent connecting over MCP is always said. What they do is always said in \
             Quill\u{2019}s window, and kept in the log."
                .to_owned(),
        ),
    ));
    fields.push(FormField::List {
        id: AGENT_LOG.to_owned(),
        label: "Lately:".to_owned(),
        rows: recent
            .iter()
            .map(|entry| FormRow {
                id: format!("entry_{}", entry.id),
                title: text::sanitize_line(&entry.summary),
                detail: Some(format!(
                    "{}, {}{}",
                    text::sanitize_line(&entry.agent),
                    text::long_date(entry.at, &env.tz),
                    if entry.undone.is_some() {
                        ". Taken back."
                    } else {
                        ""
                    }
                )),
                actions: Vec::new(),
            })
            .collect(),
        actions: vec![
            FormAction {
                id: COPY_MCP.to_owned(),
                title: "Copy MCP Command".to_owned(),
                row: false,
                enabled: true,
            },
            FormAction {
                id: STOP_AGENTS.to_owned(),
                title: "Stop Agents".to_owned(),
                row: false,
                enabled: agent.commands || agent.mcp,
            },
        ],
        searchable: false,
        empty: Some("Nothing has been changed from outside Quill\u{2019}s window.".to_owned()),
        help: Some(
            "What commands and agents changed last; quill log in a shell has all of it, and \
             quill undo takes a change back. Stop Agents switches both ways in off at once \
             and takes back their mail that has not gone. These settings stop an agent \
             that has been misled. They are not a wall against a program that runs as you \
             and means harm: keeping an eye on your agents is still yours to do."
                .to_owned(),
        ),
    });
    fields
}

/// How many places after the point the room for downloaded mail is shown
/// and taken with.
const CACHE_DECIMALS: u8 = 2;

/// The settings window, described: every pane with its settings as the
/// file has them. `problem` is why the last change was not made.
pub fn describe(
    read: &SettingsRead,
    env: &Env,
    can: Can,
    recent: &[AgentEntry],
    problem: Option<&str>,
) -> Option<SettingsShow> {
    let panes = Pane::ALL
        .into_iter()
        .map(|pane| SettingsPane {
            id: pane.id().to_owned(),
            title: pane.title().to_owned(),
            symbol: pane.symbol().to_owned(),
            note: match pane {
                Pane::Accounts if !can.accounts => Some(
                    "This is the demo mailbox. Accounts are not added or signed in to here."
                        .to_owned(),
                ),
                // The user's own keys could not be read: the keys in force
                // are the preset's, and theirs are not written over.
                Pane::Keys => read.keys_problem.as_deref().map(|problem| {
                    format!(
                        "Your own keys, in keys.toml, have a mistake in them, so none can be \
                         changed here until it is put right. {}",
                        sentence(&text::sanitize_line(problem))
                    )
                }),
                _ => None,
            },
            fields: match pane {
                Pane::General => general(read),
                Pane::Appearance => appearance(read, env, can),
                Pane::Accounts => accounts(read, can),
                Pane::Composing => composing(read, env),
                Pane::Keys => keys(read, env),
                Pane::Privacy => privacy(read),
                Pane::Agents => agents(read, env, recent),
                Pane::Storage => storage(read, env),
            },
            more: Some(MORE.to_owned()),
        })
        .collect();
    SettingsShow::new(SettingsShowParams {
        panes,
        problem: problem.map(text::sanitize_line),
    })
}

/// The window when the settings file cannot be read for what it says: it
/// has no panes, says what is wrong and offers the file. Nothing is
/// pretended to be editable.
pub fn describe_unreadable(problem: &str) -> Option<SettingsShow> {
    SettingsShow::new(SettingsShowParams {
        panes: Vec::new(),
        problem: Some(format!(
            "The settings file has a mistake in it, so nothing can be changed here until it \
             is put right. {}",
            sentence(&text::sanitize_line(problem))
        )),
    })
}

/// `words` as a sentence: a capital to start with and a full stop to end.
fn sentence(words: &str) -> String {
    let mut letters = words.trim().chars();
    let mut said: String = letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default();
    if !said.is_empty() && !said.ends_with(['.', '!', '?']) {
        said.push('.');
    }
    said
}

/// What giving `value` to the field called `field` in the pane called
/// `pane` changes, checked against the settings as they were described:
/// a choice must be one of the options shown, a number within the bounds
/// shown, and so on. `Err` says why it changes nothing.
pub fn change(
    described: &SettingsShow,
    read: &SettingsRead,
    pane: &str,
    field: &str,
    value: &FormValue,
) -> Result<Change, String> {
    let setting = Setting::named(pane, field).ok_or("There is no such setting.")?;
    let shown = described
        .field(pane, field)
        .ok_or("There is no such setting.")?;
    let label = shown.label().trim_end_matches(':');
    match (shown, value) {
        (FormField::Choice { options, .. }, FormValue::Text(chosen)) => {
            if !options.iter().any(|option| &option.id == chosen) {
                return Err(format!("That is not one of the choices for {label}."));
            }
            let not_one = || format!("That is not one of the choices for {label}.");
            match setting {
                Setting::Appearance => Appearance::ALL
                    .into_iter()
                    .find(|appearance| appearance.as_str() == chosen)
                    .map(Change::Appearance)
                    .ok_or_else(not_one),
                Setting::ThemeLight | Setting::ThemeDark => read
                    .themes
                    .iter()
                    .find(|line| &theme_id(&line.name) == chosen)
                    .map(|line| {
                        if setting == Setting::ThemeLight {
                            Change::ThemeLight(line.name.clone())
                        } else {
                            Change::ThemeDark(line.name.clone())
                        }
                    })
                    // The theme the file names and nobody has: it is shown,
                    // and is not to be chosen.
                    .ok_or_else(|| "There is no theme of that name to change to.".to_owned()),
                Setting::Keymap => KeymapPreset::ALL
                    .into_iter()
                    .find(|preset| preset.as_str() == chosen)
                    .map(Change::Keymap)
                    .ok_or_else(not_one),
                Setting::Editor => editors_offered()
                    .into_iter()
                    .find(|editor| editor.as_str() == chosen)
                    .map(Change::Editor)
                    .ok_or_else(|| {
                        "mg does not run inside Quill\u{2019}s app. It can be chosen in the \
                         settings file, for Quill in a terminal."
                            .to_owned()
                    }),
                Setting::Format => ComposeFormat::ALL
                    .into_iter()
                    .find(|format| format.as_str() == chosen)
                    .map(Change::Format)
                    .ok_or_else(not_one),
                Setting::ReplyPosition => ReplyPosition::ALL
                    .into_iter()
                    .find(|position| position.as_str() == chosen)
                    .map(Change::ReplyPosition)
                    .ok_or_else(not_one),
                Setting::RemoteImages => [RemoteImages::Ask, RemoteImages::Never]
                    .into_iter()
                    .find(|remote| remote.as_str() == chosen)
                    .map(Change::RemoteImages)
                    .ok_or_else(not_one),
                Setting::AgentNotify => AgentNotify::ALL
                    .into_iter()
                    .find(|notify| notify.as_str() == chosen)
                    .map(Change::AgentNotify)
                    .ok_or_else(not_one),
                Setting::RulesLookBack => chosen
                    .strip_prefix("back_")
                    .filter(|age| quill_core::rules::look_back_seconds(age).is_some())
                    .map(|age| Change::RulesLookBack(age.to_owned()))
                    .ok_or_else(not_one),
                _ => Err(not_one()),
            }
        }
        (FormField::Text { .. }, FormValue::Text(typed)) => match setting {
            Setting::EditorPath => {
                let path = typed.trim();
                if path.is_empty() {
                    Ok(Change::EditorPath(None))
                } else if names_a_program(path) {
                    Ok(Change::EditorPath(Some(path.to_owned())))
                } else {
                    Err(format!(
                        "{label} takes a whole path, such as /opt/homebrew/bin/nvim, or a \
                         program\u{2019}s name."
                    ))
                }
            }
            _ => Err(format!("{label} is not typed.")),
        },
        (FormField::Font { .. }, FormValue::Text(family)) => {
            let family = family.trim();
            if family.is_empty() {
                Ok(Change::Font(None))
            } else if family.len() <= MAX_FONT_NAME && !family.chars().any(char::is_control) {
                Ok(Change::Font(Some(family.to_owned())))
            } else {
                Err(format!("{label} is the name of a font."))
            }
        }
        (FormField::Toggle { .. }, FormValue::Flag(on)) => match setting {
            Setting::AgentCommands => Ok(Change::AgentCommands(*on)),
            Setting::AgentMcp => Ok(Change::AgentMcp(*on)),
            Setting::AgentOrganize => Ok(Change::AgentOrganize(*on)),
            Setting::AgentDelete => Ok(Change::AgentDelete(*on)),
            Setting::AgentDraft => Ok(Change::AgentDraft(*on)),
            Setting::AgentSend => Ok(Change::AgentSend(*on)),
            Setting::AgentHoldSends => Ok(Change::AgentHoldSends(*on)),
            Setting::RulesEnabled => Ok(Change::RulesEnabled(*on)),
            Setting::Notifications => Ok(Change::Notifications(*on)),
            Setting::Threads => Ok(Change::Threads(*on)),
            Setting::WebReading => Ok(Change::WebReading(*on)),
            Setting::SpellCheck => Ok(Change::SpellCheck(*on)),
            _ => Err(format!("{label} is not something to switch on or off.")),
        },
        (
            FormField::Number {
                min, max, decimals, ..
            },
            FormValue::Number(number),
        ) => {
            let number = number.rounded(*decimals);
            let within =
                min.get() <= number.get() && max.is_none_or(|max| number.get() <= max.get());
            if !within {
                return Err(match max {
                    Some(max) => format!(
                        "{label} is a number from {} to {}.",
                        gigabytes(min.get()),
                        gigabytes(max.get())
                    ),
                    None => format!("{label} is {} or more.", gigabytes(min.get())),
                });
            }
            match setting {
                Setting::UndoSendSeconds => number
                    .whole()
                    .and_then(|seconds| u32::try_from(seconds).ok())
                    .map(Change::UndoSendSeconds)
                    .ok_or_else(|| format!("{label} is a whole number of seconds.")),
                Setting::AgentSendGap => number
                    .whole()
                    .and_then(|seconds| u32::try_from(seconds).ok())
                    .map(Change::AgentSendGap)
                    .ok_or_else(|| format!("{label} is a whole number of seconds.")),
                Setting::CacheGb => Ok(Change::CacheGb(number)),
                Setting::FontSize => Ok(Change::FontSize(number)),
                _ => Err(format!("{label} is not a number.")),
            }
        }
        _ => Err(format!("{label} cannot be set to that.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_place_in_the_file_and_in_the_window_of_its_own() {
        let mut places = std::collections::BTreeSet::new();
        let mut fields = std::collections::BTreeSet::new();
        for setting in Setting::ALL {
            assert!(valid_form_id(setting.key()), "{setting:?}");
            assert!(
                places.insert((setting.section(), setting.key())),
                "{setting:?}"
            );
            assert!(
                fields.insert((setting.pane().id(), setting.key())),
                "{setting:?}"
            );
            assert_eq!(
                Setting::named(setting.pane().id(), setting.key()),
                Some(setting)
            );
            assert_ne!(setting.pane(), Pane::Accounts);
        }
        assert_eq!(Setting::named("general", "editor"), None);
        // The ones that wait for the next start, and no others: what a
        // message is written in, and the room for downloaded mail.
        let waiting: Vec<Setting> = Setting::ALL
            .into_iter()
            .filter(|setting| !setting.at_once())
            .collect();
        assert_eq!(
            waiting,
            [Setting::Editor, Setting::EditorPath, Setting::CacheGb]
        );
    }

    #[test]
    fn a_name_of_the_users_own_is_given_a_name_the_bridge_can_carry() {
        // A plain name is its own.
        assert_eq!(theme_id("dark"), "dark");
        assert_eq!(theme_id("solarized_2"), "solarized_2");
        // One that is not is given one, the same every time, and no two
        // names share one.
        for name in [
            "Solarized",
            "solarized-dark",
            "x_1",
            "",
            "caf\u{e9}",
            "Catppuccin Mocha",
            "Dracula+",
            "Jack O' Lantern",
        ] {
            let id = theme_id(name);
            assert!(valid_form_id(&id) && id.starts_with("x_"), "{name}: {id}");
            assert_eq!(id, theme_id(name));
        }
        assert_ne!(theme_id("Solarized"), theme_id("solarized-dark"));
        for name in ["Work", "work", "Work \"main\"\n[ui]", ""] {
            let id = account_id(name);
            assert!(valid_form_id(&id) && id.starts_with("account_"), "{id}");
        }
        assert_ne!(account_id("Work"), account_id("work"));
    }

    #[test]
    fn a_change_is_written_as_a_person_would_write_it() {
        assert_eq!(
            Change::ThemeLight("light".to_owned()).written(),
            Written::Word("light")
        );
        assert_eq!(
            Change::ThemeDark("Catppuccin Mocha".to_owned()).written(),
            Written::Word("Catppuccin Mocha")
        );
        assert_eq!(
            Change::Appearance(Appearance::System).written(),
            Written::Word("system")
        );
        for dark in [false, true] {
            let imported = Change::ThemeInForce {
                name: "Dracula+".to_owned(),
                dark,
            };
            assert_eq!(imported.written(), Written::Word("Dracula+"));
            assert_eq!(
                imported.setting(),
                if dark {
                    Setting::ThemeDark
                } else {
                    Setting::ThemeLight
                }
            );
        }
        assert_eq!(
            Change::Keymap(KeymapPreset::Mutt).written(),
            Written::Word("mutt")
        );
        assert_eq!(
            Change::Editor(ComposeEditor::Mg).written(),
            Written::Word("mg")
        );
        // A program named is written; none named is the key taken out.
        let named = Change::EditorPath(Some("/opt/homebrew/bin/nvim".to_owned()));
        assert_eq!(named.written(), Written::Word("/opt/homebrew/bin/nvim"));
        assert_eq!(named.setting(), Setting::EditorPath);
        assert_eq!(Change::EditorPath(None).written(), Written::Gone);
        assert!(!Setting::EditorPath.at_once());
        for good in ["/opt/homebrew/bin/nvim", "~/bin/my vim", "nvim"] {
            assert!(names_a_program(good), "{good}");
        }
        for bad in ["bin/vim", "./vim", "../vim"] {
            assert!(!names_a_program(bad), "{bad}");
        }
        assert_eq!(
            Change::RemoteImages(RemoteImages::Never).written(),
            Written::Word("never")
        );
        assert_eq!(Change::Threads(false).written(), Written::Flag(false));
        assert_eq!(Change::UndoSendSeconds(30).written(), Written::Whole(30));
        for (gigabytes, written) in [
            (5.0, Written::Whole(5)),
            (0.0, Written::Whole(0)),
            (0.5, Written::Decimal(0.5)),
        ] {
            let change = Change::CacheGb(Number::new(gigabytes).unwrap());
            assert_eq!(change.written(), written);
        }
        // A font named is written; none named is the key taken out. A
        // size is written as a person writes it.
        assert_eq!(
            Change::Font(Some("JetBrains Mono".to_owned())).written(),
            Written::Word("JetBrains Mono")
        );
        assert_eq!(Change::Font(None).written(), Written::Gone);
        assert_eq!(
            Change::FontSize(Number::from(14)).written(),
            Written::Whole(14)
        );
        assert_eq!(
            Change::FontSize(font_size(13.5)).written(),
            Written::Decimal(13.5)
        );
        // As the file's number is read, to one place.
        assert_eq!(font_size(13.3), Number::new(13.3).unwrap());
        for setting in Setting::ALL {
            let change = match setting {
                Setting::Appearance => Change::Appearance(Appearance::Dark),
                Setting::ThemeLight => Change::ThemeLight("light".to_owned()),
                Setting::ThemeDark => Change::ThemeDark("dark".to_owned()),
                Setting::Font => Change::Font(None),
                Setting::FontSize => Change::FontSize(Number::from(13)),
                Setting::Keymap => Change::Keymap(KeymapPreset::Vim),
                Setting::Notifications => Change::Notifications(true),
                Setting::Threads => Change::Threads(true),
                Setting::WebReading => Change::WebReading(true),
                Setting::Editor => Change::Editor(ComposeEditor::Builtin),
                Setting::EditorPath => Change::EditorPath(None),
                Setting::Format => Change::Format(ComposeFormat::Plain),
                Setting::ReplyPosition => Change::ReplyPosition(ReplyPosition::Below),
                Setting::SpellCheck => Change::SpellCheck(true),
                Setting::UndoSendSeconds => Change::UndoSendSeconds(1),
                Setting::RemoteImages => Change::RemoteImages(RemoteImages::Ask),
                Setting::CacheGb => Change::CacheGb(Number::from(1)),
                Setting::AgentCommands => Change::AgentCommands(false),
                Setting::AgentMcp => Change::AgentMcp(true),
                Setting::AgentOrganize => Change::AgentOrganize(false),
                Setting::AgentDelete => Change::AgentDelete(false),
                Setting::AgentDraft => Change::AgentDraft(false),
                Setting::AgentSend => Change::AgentSend(true),
                Setting::AgentSendGap => Change::AgentSendGap(60),
                Setting::AgentHoldSends => Change::AgentHoldSends(true),
                Setting::AgentNotify => Change::AgentNotify(AgentNotify::Sends),
                Setting::RulesEnabled => Change::RulesEnabled(false),
                Setting::RulesLookBack => Change::RulesLookBack("2w".to_owned()),
            };
            assert_eq!(change.setting(), setting);
        }
    }

    #[test]
    fn the_file_having_a_mistake_is_said_as_a_sentence_and_nothing_is_shown() {
        let shown = describe_unreadable("config line 3: expected a value").unwrap();
        assert!(shown.panes().is_empty());
        assert_eq!(
            shown.problem(),
            Some(
                "The settings file has a mistake in it, so nothing can be changed here until \
                 it is put right. Config line 3: expected a value."
            )
        );
        // Whatever the file had in it is not drawn as it stands.
        let shown = describe_unreadable("bad \u{1b}[31mthing\nnext").unwrap();
        assert!(shown.problem().unwrap().ends_with("Bad thing next."));
    }
}
