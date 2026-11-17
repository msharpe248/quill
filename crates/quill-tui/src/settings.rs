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
    AccountType, Appearance, ComposeEditor, ComposeFormat, Config, KeymapPreset, RemoteImages,
    ReplyPosition,
};
use quill_host::bridge::{
    FormAction, FormField, FormOption, FormRow, FormValue, Number, SettingsPane, SettingsShow,
    SettingsShowParams, valid_form_id,
};

use crate::keymap::Keymap;
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
}

impl Settings {
    pub fn of(config: &Config) -> Self {
        Self {
            appearance: config.ui.appearance,
            theme_light: config.ui.theme_for(false).to_owned(),
            theme_dark: config.ui.theme_for(true).to_owned(),
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
        }
    }
}

/// One of the settings the window has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Appearance,
    ThemeLight,
    ThemeDark,
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
}

impl Setting {
    pub const ALL: [Setting; 15] = [
        Setting::Appearance,
        Setting::ThemeLight,
        Setting::ThemeDark,
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
    ];

    /// The key the file has it under, in its section. It is also the
    /// setting's name on the bridge, in its pane.
    pub fn key(self) -> &'static str {
        match self {
            Setting::Appearance => "appearance",
            Setting::ThemeLight => "theme_light",
            Setting::ThemeDark => "theme_dark",
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
        }
    }

    /// The section of the file it is in.
    pub fn section(self) -> &'static str {
        match self {
            Setting::Appearance
            | Setting::ThemeLight
            | Setting::ThemeDark
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
        }
    }

    /// The pane of the window it is in.
    pub fn pane(self) -> Pane {
        match self {
            Setting::Appearance
            | Setting::ThemeLight
            | Setting::ThemeDark
            | Setting::Keymap
            | Setting::Notifications
            | Setting::Threads
            | Setting::WebReading => Pane::General,
            Setting::Editor
            | Setting::EditorPath
            | Setting::Format
            | Setting::ReplyPosition
            | Setting::SpellCheck
            | Setting::UndoSendSeconds => Pane::Composing,
            Setting::RemoteImages => Pane::Privacy,
            Setting::CacheGb => Pane::Storage,
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
    Accounts,
    Composing,
    Privacy,
    Storage,
}

impl Pane {
    pub const ALL: [Pane; 5] = [
        Pane::General,
        Pane::Accounts,
        Pane::Composing,
        Pane::Privacy,
        Pane::Storage,
    ];

    /// The pane's name on the bridge.
    pub fn id(self) -> &'static str {
        match self {
            Pane::General => "general",
            Pane::Accounts => "accounts",
            Pane::Composing => "composing",
            Pane::Privacy => "privacy",
            Pane::Storage => "storage",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Accounts => "Accounts",
            Pane::Composing => "Composing",
            Pane::Privacy => "Privacy",
            Pane::Storage => "Storage",
        }
    }

    /// The SF Symbol on the pane's toolbar button.
    pub fn symbol(self) -> &'static str {
        match self {
            Pane::General => "gearshape",
            Pane::Accounts => "at",
            Pane::Composing => "square.and.pencil",
            Pane::Privacy => "hand.raised",
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
            Change::EditorPath(None) => Written::Gone,
            Change::Format(format) => Written::Word(format.as_str()),
            Change::ReplyPosition(position) => Written::Word(position.as_str()),
            Change::RemoteImages(remote) => Written::Word(remote.as_str()),
            Change::Notifications(on)
            | Change::Threads(on)
            | Change::WebReading(on)
            | Change::SpellCheck(on) => Written::Flag(*on),
            Change::UndoSendSeconds(seconds) => Written::Whole(i64::from(*seconds)),
            Change::CacheGb(gigabytes) => match gigabytes.whole() {
                Some(whole) => Written::Whole(whole),
                None => Written::Decimal(gigabytes.get()),
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

/// What the settings file says, read a moment ago: the answer to
/// `AccountRequest::Settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRead {
    pub settings: Settings,
    pub accounts: Vec<AccountLine>,
    /// The themes there are to choose from, by name: the ones Quill comes
    /// with, then the user's own.
    pub themes: Vec<String>,
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
    pub keymap: Keymap,
    pub notifications: bool,
    pub threads: bool,
    pub web_reading: bool,
    pub markdown: bool,
    pub reply_above: bool,
    pub undo_send_seconds: u32,
    pub spell_check: bool,
    pub remote_never: bool,
}

impl Applied {
    pub fn of(read: &SettingsRead) -> Self {
        let settings = &read.settings;
        Self {
            appearance: settings.appearance,
            theme_light: read.theme_light.clone(),
            theme_dark: read.theme_dark.clone(),
            keymap: read.keymap.clone(),
            notifications: settings.notifications,
            threads: settings.threads,
            web_reading: settings.web_reading,
            markdown: settings.format == ComposeFormat::Markdown,
            reply_above: settings.reply_position == ReplyPosition::Above,
            undo_send_seconds: settings.undo_send_seconds,
            spell_check: settings.spell_check,
            remote_never: settings.remote_images == RemoteImages::Never,
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
        env.keymap = self.keymap;
        env.notifications = self.notifications;
        env.threads = self.threads;
        env.web_reading = self.web_reading;
        env.compose.markdown = self.markdown;
        env.compose.reply_above = self.reply_above;
        env.compose.undo_send_seconds = self.undo_send_seconds;
        env.compose.spell_check = self.spell_check;
        env.remote_never = self.remote_never;
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
}

/// The name of the button that imports a theme, in the general pane.
pub const IMPORT_THEME: &str = "import_theme";
/// The name of the list of accounts, in its pane.
pub const ACCOUNTS: &str = "accounts";
/// The actions of that list.
pub const ADD: &str = "add";
pub const SIGN_IN: &str = "sign_in";
pub const REMOVE: &str = "remove";

/// Said at the foot of every pane, beside the button that opens the
/// settings file.
const MORE: &str = "Rules, saved searches and each account\u{2019}s servers are set in the settings \
                    file. Hooks and templates are files beside it.";
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
fn theme_title(name: &str) -> String {
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
        .map(|name| option(theme_id(name), theme_title(name)))
        .collect();
    if !read.themes.iter().any(|name| name == named) {
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

fn general(read: &SettingsRead, env: &Env) -> Vec<FormField> {
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
        FormField::Button {
            id: IMPORT_THEME.to_owned(),
            label: String::new(),
            title: "Import Theme\u{2026}".to_owned(),
            enabled: true,
            help: Some(
                "Themes made for Ghostty can be imported. Themes of your own are files in \
                 the themes folder beside the settings file."
                    .to_owned(),
            ),
        },
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
    ]
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
            actions: if account.signs_in {
                vec![SIGN_IN.to_owned(), REMOVE.to_owned()]
            } else {
                vec![REMOVE.to_owned()]
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
                id: REMOVE.to_owned(),
                title: "Remove\u{2026}".to_owned(),
                row: true,
                enabled: can.accounts && can.remove,
            },
        ],
        empty: Some("No accounts yet.".to_owned()),
        help: Some(
            "Removing an account takes it out of the settings file and the Keychain, and \
             deletes the copy of its mail kept on this Mac. Mail on its server is not \
             touched."
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

/// How many places after the point the room for downloaded mail is shown
/// and taken with.
const CACHE_DECIMALS: u8 = 2;

/// The settings window, described: every pane with its settings as the
/// file has them. `problem` is why the last change was not made.
pub fn describe(
    read: &SettingsRead,
    env: &Env,
    can: Can,
    problem: Option<&str>,
) -> Option<SettingsShow> {
    let panes = Pane::ALL
        .into_iter()
        .map(|pane| SettingsPane {
            id: pane.id().to_owned(),
            title: pane.title().to_owned(),
            symbol: pane.symbol().to_owned(),
            note: (pane == Pane::Accounts && !can.accounts).then(|| {
                "This is the demo mailbox. Accounts are not added or signed in to here.".to_owned()
            }),
            fields: match pane {
                Pane::General => general(read, env),
                Pane::Accounts => accounts(read, can),
                Pane::Composing => composing(read, env),
                Pane::Privacy => privacy(read),
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
                    .find(|name| &theme_id(name) == chosen)
                    .map(|name| {
                        if setting == Setting::ThemeLight {
                            Change::ThemeLight(name.clone())
                        } else {
                            Change::ThemeDark(name.clone())
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
        (FormField::Toggle { .. }, FormValue::Flag(on)) => match setting {
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
                Setting::CacheGb => Ok(Change::CacheGb(number)),
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
        for setting in Setting::ALL {
            let change = match setting {
                Setting::Appearance => Change::Appearance(Appearance::Dark),
                Setting::ThemeLight => Change::ThemeLight("light".to_owned()),
                Setting::ThemeDark => Change::ThemeDark("dark".to_owned()),
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
