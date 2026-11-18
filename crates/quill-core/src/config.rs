//! `config.toml`: parsing, defaults and validation.
//!
//! Secrets never live here. Passwords and tokens belong in the Keychain.

use std::path::Path;

use serde::Deserialize;

/// The whole of `config.toml`. Every section is optional and falls back to
/// its defaults.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: UiConfig,
    pub compose: ComposeConfig,
    pub privacy: PrivacyConfig,
    pub storage: StorageConfig,
    #[serde(rename = "account")]
    pub accounts: Vec<AccountConfig>,
    /// Searches kept in the sidebar, each a mailbox of whatever it finds.
    #[serde(rename = "search")]
    pub searches: Vec<SavedSearch>,
    /// What to do with mail as it arrives, in the order written.
    #[serde(rename = "rule")]
    pub rules: Vec<RuleConfig>,
    /// What programs outside Quill's window, agents among them, may do
    /// with the mail.
    pub agent: AgentConfig,
}

/// Something to do with arriving mail that matches a search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    /// What the rule is called when Quill has something to say about it.
    #[serde(default)]
    pub name: Option<String>,
    /// Which mail it is for, in the search box's words: `from:buildbot`.
    #[serde(rename = "match")]
    pub matching: String,
    /// Only mail of this account. Every account's, without it.
    #[serde(default)]
    pub account: Option<String>,
    /// File it in this folder of its account: a folder's name, or what a
    /// folder is for (`archive`, `trash`, `junk`).
    #[serde(default)]
    pub move_to: Option<String>,
    #[serde(default)]
    pub mark_read: bool,
    #[serde(default)]
    pub star: bool,
    /// Whether to announce it. Left out: announced if it stays in the
    /// inbox, not if the rule files it elsewhere.
    #[serde(default)]
    pub notify: Option<bool>,
}

/// A search with a name, shown in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSearch {
    pub name: String,
    /// What to find, as typed in the search box.
    pub query: String,
}

/// The fewest columns the accounts pane can be given: room for the
/// marker before a name and a few letters of it.
pub const SIDEBAR_NARROWEST: u16 = 12;
/// The most columns the accounts pane can be given.
pub const SIDEBAR_WIDEST: u16 = 60;
/// The least and the most of the room right of the accounts that the
/// message has beside the list: what `reader_width` may be.
pub const READER_LEAST: f32 = 0.2;
pub const READER_MOST: f32 = 0.8;
/// What `reader_width` is when the file does not say: half each.
pub const READER_USUAL: f32 = 0.5;

/// The smallest and the largest the font of the app's terminals may be,
/// in points.
pub const FONT_SMALLEST: u32 = 6;
pub const FONT_LARGEST: u32 = 72;
/// The most bytes the name of a font's family comes to.
pub const MAX_FONT_NAME: usize = 128;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// A built-in theme name, or the name of a file in `themes/`. The one
    /// theme there is until `theme_light` or `theme_dark` says otherwise:
    /// each of those that is left out is this.
    pub theme: String,
    /// Whether Quill is light, dark, or as the Mac is.
    pub appearance: Appearance,
    /// The theme while Quill is light. Left out, it is `theme`.
    pub theme_light: Option<String>,
    /// The theme while Quill is dark. Left out, it is `theme`.
    pub theme_dark: Option<String>,
    pub keymap: KeymapPreset,
    /// The reader's share of the space right of the sidebar, in wide layouts.
    /// Quill writes this itself when the user drags the edge between the
    /// list and the message.
    pub reader_width: f32,
    /// How many columns the accounts pane has beside the others. Left
    /// out, it goes by the window's width. Quill writes this itself when
    /// the user drags the pane's edge.
    pub sidebar_width: Option<u16>,
    pub toolbar: bool,
    /// Terminal font for the app shell. `None` leaves the shell's default.
    pub font: Option<String>,
    pub font_size: f32,
    /// Take the mouse: click to select and open, wheel to scroll. While it
    /// is on, selecting text in the terminal needs shift held.
    pub mouse: bool,
    /// Say when new mail arrives, where the host can (Quill's own window).
    pub notifications: bool,
    /// List the messages of a conversation together.
    pub threads: bool,
    /// Read mail in a web view pane that follows the message list, in
    /// place of the text reader, where the host has one (Quill's own
    /// window). Quill writes this itself when the user switches.
    pub web_reading: bool,
    /// Show the accounts pane beside the others. Hidden, it comes up over
    /// them when the keys are taken to it. Quill writes this itself when
    /// the user hides it or puts it back.
    pub sidebar: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_owned(),
            appearance: Appearance::System,
            theme_light: None,
            theme_dark: None,
            keymap: KeymapPreset::Vim,
            reader_width: READER_USUAL,
            sidebar_width: None,
            toolbar: true,
            font: None,
            font_size: 13.0,
            mouse: true,
            notifications: true,
            threads: true,
            web_reading: false,
            sidebar: true,
        }
    }
}

impl UiConfig {
    /// The name of the theme for when Quill is dark, or for when it is
    /// light: the one said for that, and otherwise `theme`. So a config
    /// that says only `theme`, as every config did before there was an
    /// appearance to choose, has that one theme whatever the appearance.
    pub fn theme_for(&self, dark: bool) -> &str {
        let own = if dark {
            &self.theme_dark
        } else {
            &self.theme_light
        };
        own.as_deref().unwrap_or(&self.theme)
    }
}

/// Whether Quill is light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the Mac is, and changing with it. Only Quill's own app is told
    /// what the Mac is; a terminal is not, and there this is dark.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Whether Quill is dark, given what the host says the system is
    /// (`None` when there is nobody to say, as in a terminal).
    pub fn is_dark(self, system_dark: Option<bool>) -> bool {
        match self {
            Self::System => system_dark.unwrap_or(true),
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

/// The longest name a theme may have, in bytes.
pub const MAX_THEME_NAME: usize = 100;

/// Whether `name` could be the name of a theme of the user's own: the
/// name of a file in `themes/`, as it stands or with `.toml` after it, and
/// nothing that could point outside that folder or be hidden in it.
///
/// Themes made for Ghostty are named as people write ("Catppuccin Mocha",
/// "Dracula+", "Jack O' Lantern"), so spaces and punctuation are part of a
/// name. A path separator is not, nor a control character, a dot to start
/// with, or space at either end; and no name ends in `.toml`, which would
/// make one file answer to two names.
pub fn is_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_THEME_NAME
        && !name.starts_with('.')
        && name.trim() == name
        && !name.to_ascii_lowercase().ends_with(".toml")
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c == ':' || c.is_control())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeymapPreset {
    #[default]
    Vim,
    Mutt,
}

impl KeymapPreset {
    pub const ALL: [Self; 2] = [Self::Vim, Self::Mutt];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vim => "vim",
            Self::Mutt => "mutt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComposeConfig {
    pub editor: ComposeEditor,
    /// The program to run as that editor, where it is not the one of its
    /// name found on the `PATH`: a whole path (`/opt/homebrew/bin/nvim`),
    /// one from the home folder (`~/bin/vim`), or another name to look
    /// for (`nvim`). It is run as the kind of editor `editor` says, so a
    /// program named for `vim` is given Vim's arguments and Quill's
    /// plugin. Means nothing with Quill's own editor.
    pub editor_path: Option<String>,
    pub format: ComposeFormat,
    pub reply_position: ReplyPosition,
    pub spell_check: bool,
    pub undo_send_seconds: u32,
    /// Say what an editor's own keys do above it, where it runs in the
    /// message's pane (Quill's app). Quill switches this off itself once
    /// it has been shown for one message.
    pub editor_tips: bool,
}

impl Default for ComposeConfig {
    fn default() -> Self {
        Self {
            editor: ComposeEditor::Builtin,
            editor_path: None,
            format: ComposeFormat::Markdown,
            reply_position: ReplyPosition::Above,
            spell_check: true,
            undo_send_seconds: 10,
            editor_tips: true,
        }
    }
}

/// What a message is written in.
///
/// Quill's own editor, or one outside it. An outside editor is the user's
/// own, in a terminal and in Quill's app alike: the program of that name
/// on the `PATH`, or the one `editor_path` names, with the user's own
/// settings for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeEditor {
    /// Quill's own.
    #[default]
    Builtin,
    /// Vim. `script` is read as this too: it was how the app reached the
    /// user's own Vim while the app was sandboxed, and a settings file
    /// that still says it means the same thing now.
    #[serde(alias = "script")]
    Vim,
    /// nano. On a Mac that is Pico under another name.
    Nano,
    /// Pico.
    Pico,
    /// mg, a small Emacs.
    Mg,
}

impl ComposeEditor {
    pub const ALL: [Self; 5] = [Self::Builtin, Self::Vim, Self::Nano, Self::Pico, Self::Mg];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Vim => "vim",
            Self::Nano => "nano",
            Self::Pico => "pico",
            Self::Mg => "mg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeFormat {
    #[default]
    Markdown,
    Plain,
}

impl ComposeFormat {
    pub const ALL: [Self; 2] = [Self::Markdown, Self::Plain];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Plain => "plain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyPosition {
    #[default]
    Above,
    Below,
}

impl ReplyPosition {
    pub const ALL: [Self; 2] = [Self::Above, Self::Below];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

/// How much of this Mac's disk downloaded mail may take.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// The most downloaded messages may come to, in gigabytes. Past it
    /// the oldest are let go and fetched again when next opened. 0 is no
    /// limit.
    pub cache_gb: f64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { cache_gb: 5.0 }
    }
}

impl StorageConfig {
    /// The limit in bytes; `None` for no limit.
    pub fn cache_bytes(&self) -> Option<u64> {
        // Whole megabytes are plenty exact, and keep the arithmetic in
        // integers.
        let megabytes = (self.cache_gb * 1024.0).round();
        (megabytes >= 1.0 && megabytes.is_finite()).then(|| megabytes as u64 * 1024 * 1024)
    }
}

/// The longest wait there can be between two messages agents send: a day.
pub const SEND_GAP_LONGEST: u32 = 24 * 60 * 60;
/// The most messages one change may be asked to touch, whatever is set.
pub const BULK_CEILING: u32 = 1000;

/// What may be done to the mail from outside Quill's window: by the
/// commands for a shell, and by an agent's program over MCP. One set of
/// switches for both ways in and for every agent: Quill cannot tell a
/// skill running `quill send` from its user doing so, nor one agent from
/// another by the name it gives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// The commands for a shell that touch mail.
    pub commands: bool,
    /// MCP, served by the Quill that is running. Off as Quill comes.
    pub mcp: bool,
    /// Marks, moves, copies, snoozing, putting back, making a folder.
    pub organize: bool,
    /// Moving to a trash folder. Never more than that.
    pub delete: bool,
    /// Making, changing and discarding drafts.
    pub draft: bool,
    /// Sending. Off as Quill comes.
    pub send: bool,
    /// What is sent waits in the outbox for the user to let it go.
    pub hold_sends: bool,
    /// Seconds from one message sent this way to the next. 0: no wait.
    pub send_gap: u32,
    /// What the user is told of as a notification.
    pub notify: AgentNotify,
    /// The accounts that can be seen, by name. `None`: every account.
    pub accounts: Option<Vec<String>>,
    /// The most messages a search may name for one change without being
    /// told to go on.
    pub bulk_limit: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            commands: true,
            mcp: false,
            organize: true,
            delete: true,
            draft: true,
            send: false,
            hold_sends: false,
            send_gap: 30,
            notify: AgentNotify::Changes,
            accounts: None,
            bulk_limit: 50,
        }
    }
}

impl AgentConfig {
    /// Whether an account of this name can be seen.
    pub fn sees(&self, account: &str) -> bool {
        self.accounts
            .as_ref()
            .is_none_or(|names| names.iter().any(|name| name.eq_ignore_ascii_case(account)))
    }
}

/// What agents do that the user is notified of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentNotify {
    /// Every change to the mail, and every message sent.
    #[default]
    Changes,
    /// Messages sent, or waiting to be let go.
    Sends,
    /// Nothing of what they do. A connection is still said.
    None,
}

impl AgentNotify {
    pub const ALL: [Self; 3] = [Self::Changes, Self::Sends, Self::None];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Sends => "sends",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacyConfig {
    pub remote_images: RemoteImages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteImages {
    #[default]
    Ask,
    Never,
    Allowlist,
}

impl RemoteImages {
    pub const ALL: [Self; 3] = [Self::Ask, Self::Never, Self::Allowlist];

    /// The word the config says it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Never => "never",
            Self::Allowlist => "allowlist",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: AccountType,
    pub address: String,
    /// The user's name as it appears on mail sent from this account. Without
    /// it mail goes out from the bare address.
    #[serde(default)]
    pub full_name: Option<String>,
    /// Text added to the end of new mail from this account.
    #[serde(default)]
    pub signature: Option<String>,
    /// How the account proves who it is. A `graph` account always signs
    /// in with OAuth, whatever this says.
    #[serde(default)]
    pub auth: AuthMethod,
    /// The id of the application registered for Quill with the provider
    /// (Microsoft Entra, or a Google Cloud project). OAuth needs one.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// What Google calls the client secret of a desktop application. It
    /// is not a secret in any useful sense, as Google says itself: every
    /// copy of a desktop application carries it. Microsoft has none.
    #[serde(default)]
    pub oauth_client_secret: Option<String>,
    /// Microsoft only: the organisation to sign in to, by id or domain
    /// name. Without it any organisation's accounts and personal ones are
    /// accepted.
    #[serde(default)]
    pub oauth_tenant: Option<String>,
    /// Other addresses to sign in at and to reach Microsoft Graph at, for
    /// a national cloud.
    #[serde(default)]
    pub oauth: Option<OAuthAddresses>,
    #[serde(default)]
    pub imap: Option<ServerConfig>,
    #[serde(default)]
    pub pop3: Option<ServerConfig>,
    /// For an `ews` account: where the server's Exchange Web Services are.
    #[serde(default)]
    pub ews: Option<EwsServerConfig>,
    /// For a `pop3` account, whose mail is kept on this Mac: when the
    /// server's copy of a message is deleted. `"never"`, `"downloaded"`,
    /// or a number of days after it was first seen.
    #[serde(default)]
    pub delete_from_server: DeleteFromServer,
    #[serde(default)]
    pub smtp: Option<ServerConfig>,
}

/// An on-premises Exchange server's web services.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EwsServerConfig {
    /// The address of the service: `https://mail.example.com/EWS/Exchange.asmx`.
    pub url: String,
    /// The name to sign in with: often `DOMAIN\user` or the address.
    /// `None` means the account's address.
    #[serde(default)]
    pub username: Option<String>,
}

/// When the server's copy of a message is deleted, for an account whose
/// mail is kept here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "DeleteFromServerText")]
pub enum DeleteFromServer {
    /// Left on the server, for other mail programs to see too.
    #[default]
    Never,
    /// Once Quill has the whole message.
    Downloaded,
    /// This many days after Quill first saw it.
    AfterDays(u32),
}

/// `delete_from_server` as written: a word, or a number of days.
#[derive(Deserialize)]
#[serde(untagged)]
enum DeleteFromServerText {
    Word(String),
    Days(i64),
}

impl TryFrom<DeleteFromServerText> for DeleteFromServer {
    type Error = String;

    fn try_from(text: DeleteFromServerText) -> Result<Self, String> {
        const HELP: &str =
            "delete_from_server is \"never\", \"downloaded\", or a number of days such as 14";
        match text {
            DeleteFromServerText::Word(word) => match word.as_str() {
                "never" => Ok(Self::Never),
                "downloaded" => Ok(Self::Downloaded),
                _ => Err(HELP.to_owned()),
            },
            DeleteFromServerText::Days(days) => match u32::try_from(days) {
                Ok(0) => Ok(Self::Downloaded),
                Ok(days) => Ok(Self::AfterDays(days)),
                Err(_) => Err(HELP.to_owned()),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Password,
    Oauth,
}

/// Where to sign in and where Microsoft Graph is, when not at the
/// provider's usual addresses. Each must be `https`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OAuthAddresses {
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub device_url: Option<String>,
    pub graph_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    Imap,
    Pop3,
    Graph,
    Ews,
}

impl AccountType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Imap => "imap",
            Self::Pop3 => "pop3",
            Self::Graph => "graph",
            Self::Ews => "ews",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    /// `None` means the default port for the protocol and TLS mode.
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub tls: TlsMode,
    /// The name to sign in with. `None` means the account's address.
    #[serde(default)]
    pub username: Option<String>,
    /// The SHA-256 fingerprint of the one certificate to accept, for a
    /// self-signed server: 64 hex digits, colons and a `sha256:` prefix
    /// allowed.
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    #[default]
    Implicit,
    Starttls,
    /// Plaintext. Only ever by explicit per-account opt-in.
    None,
}

/// Why a config could not be used. The message fits on one line so it can be
/// shown in the status bar.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("config line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error("config: {0}")]
    Invalid(String),
}

impl Config {
    /// Parses and validates the text of a `config.toml`.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|err| ConfigError::Parse {
            line: err
                .span()
                .map_or(1, |span| line_of_offset(text, span.start)),
            message: err.message().to_owned(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Reads `path`. A missing file is not an error: it yields the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !(READER_LEAST..=READER_MOST).contains(&self.ui.reader_width) {
            return Err(ConfigError::Invalid(format!(
                "ui.reader_width must be between 0.2 and 0.8, got {}",
                self.ui.reader_width
            )));
        }
        if let Some(columns) = self.ui.sidebar_width
            && !(SIDEBAR_NARROWEST..=SIDEBAR_WIDEST).contains(&columns)
        {
            return Err(ConfigError::Invalid(format!(
                "ui.sidebar_width must be between {SIDEBAR_NARROWEST} and {SIDEBAR_WIDEST} \
                 columns, got {columns}"
            )));
        }
        let sizes = FONT_SMALLEST as f32..=FONT_LARGEST as f32;
        if !sizes.contains(&self.ui.font_size) {
            return Err(ConfigError::Invalid(format!(
                "ui.font_size must be between {FONT_SMALLEST} and {FONT_LARGEST}, got {}",
                self.ui.font_size
            )));
        }
        if let Some(font) = &self.ui.font
            && (font.trim().is_empty()
                || font.len() > MAX_FONT_NAME
                || font.chars().any(char::is_control))
        {
            return Err(ConfigError::Invalid(
                "ui.font must be the name of a font, on one line".to_owned(),
            ));
        }
        if self.agent.send_gap > SEND_GAP_LONGEST {
            return Err(ConfigError::Invalid(format!(
                "agent.send_gap is in seconds and must be {SEND_GAP_LONGEST} (a day) or less, \
                 got {}",
                self.agent.send_gap
            )));
        }
        if !(1..=BULK_CEILING).contains(&self.agent.bulk_limit) {
            return Err(ConfigError::Invalid(format!(
                "agent.bulk_limit must be between 1 and {BULK_CEILING}, got {}",
                self.agent.bulk_limit
            )));
        }
        if !self.storage.cache_gb.is_finite() || self.storage.cache_gb < 0.0 {
            return Err(ConfigError::Invalid(
                "storage.cache_gb must be 0 (no limit) or more".to_owned(),
            ));
        }
        if let Some(path) = &self.compose.editor_path {
            let whole = path.starts_with('/') || path.starts_with("~/");
            if path.trim().is_empty() || path.contains('\0') || (!whole && path.contains('/')) {
                return Err(ConfigError::Invalid(format!(
                    "compose.editor_path must be a whole path (\"/opt/homebrew/bin/nvim\", \
                     \"~/bin/vim\") or a program's name (\"nvim\"), got {path:?}"
                )));
            }
        }
        for (index, rule) in self.rules.iter().enumerate() {
            let named = rule
                .name
                .clone()
                .unwrap_or_else(|| format!("rule {}", index + 1));
            if rule.matching.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "{named} has nothing to match. Give it `match = \"from:someone\"`"
                )));
            }
            if rule.move_to.is_none() && !rule.mark_read && !rule.star && rule.notify.is_none() {
                return Err(ConfigError::Invalid(format!(
                    "{named} does nothing. Give it one of move_to, mark_read, star or notify"
                )));
            }
        }
        for search in &self.searches {
            if search.name.trim().is_empty() || search.query.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "a [[search]] needs a name and a query".to_owned(),
                ));
            }
        }
        for (index, account) in self.accounts.iter().enumerate() {
            if account.name.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "account {} has an empty name",
                    index + 1
                )));
            }
            if account.delete_from_server != DeleteFromServer::Never
                && account.kind != AccountType::Pop3
            {
                return Err(ConfigError::Invalid(format!(
                    "{}: delete_from_server is for pop3 accounts, whose mail is kept on this Mac. Other accounts keep theirs on the server",
                    account.name
                )));
            }
            if self.accounts[..index]
                .iter()
                .any(|earlier| earlier.name == account.name)
            {
                return Err(ConfigError::Invalid(format!(
                    "two accounts are named \"{}\"",
                    account.name
                )));
            }
        }
        Ok(())
    }
}

fn line_of_offset(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example from the spec, verbatim.
    const SPEC_EXAMPLE: &str = r##"
[ui]
theme = "dark"
keymap = "vim"            # navigation keys; or "mutt"
reader_width = 0.5
toolbar = true
font = "JetBrains Mono"
font_size = 13

[compose]
editor = "builtin"        # or your own "vim", "nano", "pico" or "mg"
format = "markdown"       # or "plain"
reply_position = "above"  # or "below"
spell_check = true
undo_send_seconds = 10

[privacy]
remote_images = "ask"     # "ask" | "never" | "allowlist"

[[account]]
name = "Work"
type = "graph"            # "imap" | "pop3" | "graph" | "ews"
address = "me@example.com"
oauth_client_id = "..."

[[account]]
name = "Personal"
type = "imap"
address = "me@fastmail.com"
imap = { host = "imap.fastmail.com", port = 993, tls = "implicit" }
smtp = { host = "smtp.fastmail.com", port = 465, tls = "implicit" }
"##;

    #[test]
    fn deleting_from_the_server_is_a_word_or_a_number_of_days() {
        let pop = |line: &str| {
            Config::parse(&format!(
                "[[account]]\nname = \"Old\"\ntype = \"pop3\"\naddress = \"a@example.com\"\n{line}\n"
            ))
        };
        let policy = |line: &str| pop(line).unwrap().accounts[0].delete_from_server;
        assert_eq!(policy(""), DeleteFromServer::Never);
        assert_eq!(
            policy("delete_from_server = \"never\""),
            DeleteFromServer::Never
        );
        assert_eq!(
            policy("delete_from_server = \"downloaded\""),
            DeleteFromServer::Downloaded
        );
        assert_eq!(
            policy("delete_from_server = 14"),
            DeleteFromServer::AfterDays(14)
        );
        assert_eq!(
            policy("delete_from_server = 0"),
            DeleteFromServer::Downloaded
        );
        for bad in [
            "delete_from_server = \"sometimes\"",
            "delete_from_server = -1",
        ] {
            let err = pop(bad).unwrap_err().to_string();
            assert!(err.contains("a number of days such as 14"), "{err}");
        }
        // It means nothing for an account whose mail stays on its server.
        let err = Config::parse(
            "[[account]]\nname = \"Work\"\ntype = \"imap\"\naddress = \"a@example.com\"\ndelete_from_server = \"downloaded\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("is for pop3 accounts"), "{err}");
    }

    #[test]
    fn the_cache_has_five_gigabytes_unless_told_otherwise() {
        assert_eq!(
            Config::default().storage.cache_bytes(),
            Some(5 * 1024 * 1024 * 1024)
        );
        let half = Config::parse("[storage]\ncache_gb = 0.5\n").unwrap();
        assert_eq!(half.storage.cache_bytes(), Some(512 * 1024 * 1024));
        let unlimited = Config::parse("[storage]\ncache_gb = 0\n").unwrap();
        assert_eq!(unlimited.storage.cache_bytes(), None);
        assert!(Config::parse("[storage]\ncache_gb = -1\n").is_err());
    }

    #[test]
    fn parses_the_spec_example() {
        let config = Config::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(config.ui.font.as_deref(), Some("JetBrains Mono"));
        assert_eq!(config.ui.keymap, KeymapPreset::Vim);
        assert_eq!(config.accounts.len(), 2);
        assert_eq!(config.accounts[0].kind, AccountType::Graph);
        let imap = config.accounts[1].imap.as_ref().unwrap();
        assert_eq!(imap.host, "imap.fastmail.com");
        assert_eq!(imap.port, Some(993));
        assert_eq!(imap.tls, TlsMode::Implicit);
    }

    /// Whoever writes a setting into the file for the user writes these
    /// words, so each must be the word the file is read by.
    #[test]
    fn every_word_a_setting_is_said_with_reads_back_as_that_setting() {
        for preset in KeymapPreset::ALL {
            let text = format!("[ui]\nkeymap = \"{}\"\n", preset.as_str());
            assert_eq!(Config::parse(&text).unwrap().ui.keymap, preset);
        }
        for editor in ComposeEditor::ALL {
            let text = format!("[compose]\neditor = \"{}\"\n", editor.as_str());
            assert_eq!(Config::parse(&text).unwrap().compose.editor, editor);
        }
        // What the sandboxed app called the user's own Vim is Vim.
        assert_eq!(
            Config::parse("[compose]\neditor = \"script\"\n")
                .unwrap()
                .compose
                .editor,
            ComposeEditor::Vim
        );
        for format in ComposeFormat::ALL {
            let text = format!("[compose]\nformat = \"{}\"\n", format.as_str());
            assert_eq!(Config::parse(&text).unwrap().compose.format, format);
        }
        for position in ReplyPosition::ALL {
            let text = format!("[compose]\nreply_position = \"{}\"\n", position.as_str());
            assert_eq!(
                Config::parse(&text).unwrap().compose.reply_position,
                position
            );
        }
        for remote in RemoteImages::ALL {
            let text = format!("[privacy]\nremote_images = \"{}\"\n", remote.as_str());
            assert_eq!(Config::parse(&text).unwrap().privacy.remote_images, remote);
        }
    }

    #[test]
    fn an_editor_may_be_named_by_a_whole_path_or_by_a_name_and_nothing_between() {
        let path = |text: &str| {
            Config::parse(&format!(
                "[compose]\neditor = \"vim\"\neditor_path = {text:?}\n"
            ))
            .map(|config| config.compose.editor_path)
        };
        assert_eq!(Config::parse("").unwrap().compose.editor_path, None);
        for good in ["/opt/homebrew/bin/nvim", "~/bin/my vim", "nvim"] {
            assert_eq!(path(good).unwrap().as_deref(), Some(good));
        }
        for bad in ["", "  ", "bin/vim", "./vim", "../vim"] {
            assert!(path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_config_that_says_only_theme_has_that_theme_whatever_the_appearance() {
        // As every config was before there was an appearance to choose.
        let old = Config::parse("[ui]\ntheme = \"light\"\n").unwrap().ui;
        assert_eq!(old.appearance, Appearance::System);
        assert_eq!(
            (old.theme_light.as_deref(), old.theme_dark.as_deref()),
            (None, None)
        );
        assert_eq!(
            (old.theme_for(false), old.theme_for(true)),
            ("light", "light")
        );
        // And one that says nothing has the dark theme, as it always had.
        let none = Config::default().ui;
        assert_eq!(
            (none.theme_for(false), none.theme_for(true)),
            ("dark", "dark")
        );

        // Each of the two that is said is used for its own appearance, and
        // `theme` still stands for the one that is not.
        let one = Config::parse("[ui]\ntheme = \"terminal\"\ntheme_light = \"Solarized Light\"\n")
            .unwrap()
            .ui;
        assert_eq!(
            (one.theme_for(false), one.theme_for(true)),
            ("Solarized Light", "terminal")
        );
        let both = Config::parse(
            "[ui]\nappearance = \"light\"\ntheme_light = \"light\"\ntheme_dark = \"Dracula\"\n",
        )
        .unwrap()
        .ui;
        assert_eq!(both.appearance, Appearance::Light);
        assert_eq!(
            (both.theme_for(false), both.theme_for(true)),
            ("light", "Dracula")
        );
        assert!(Config::parse("[ui]\nappearance = \"dim\"\n").is_err());
    }

    #[test]
    fn the_appearance_is_light_or_dark_or_as_the_system_is() {
        for appearance in Appearance::ALL {
            let text = format!("[ui]\nappearance = \"{}\"\n", appearance.as_str());
            assert_eq!(Config::parse(&text).unwrap().ui.appearance, appearance);
        }
        assert!(!Appearance::Light.is_dark(Some(true)));
        assert!(Appearance::Dark.is_dark(Some(false)));
        assert!(Appearance::System.is_dark(Some(true)));
        assert!(!Appearance::System.is_dark(Some(false)));
        // With nobody to say what the system is, as in a terminal: dark.
        assert!(Appearance::System.is_dark(None));
    }

    #[test]
    fn a_theme_is_named_as_people_write_and_never_by_a_path() {
        for name in [
            "dark",
            "solarized-dark_2",
            "Catppuccin Mocha",
            "Dracula+",
            "Jack O' Lantern",
            "Builtin Tango (Light)",
            "caf\u{e9} 3.5",
        ] {
            assert!(is_theme_name(name), "{name}");
        }
        let long = "x".repeat(MAX_THEME_NAME + 1);
        for name in [
            "",
            "../secret",
            "a/b",
            "/etc/passwd",
            "a\\b",
            "a:b",
            ".hidden",
            "x.toml",
            "X.TOML",
            " padded",
            "padded ",
            "two\nlines",
            "bell\u{7}",
            long.as_str(),
        ] {
            assert!(!is_theme_name(name), "{name:?}");
        }
    }

    #[test]
    fn empty_text_gives_the_defaults() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.ui.theme, "dark");
        assert_eq!(config.compose.undo_send_seconds, 10);
        assert_eq!(config.privacy.remote_images, RemoteImages::Ask);
    }

    #[test]
    fn a_partial_section_keeps_the_other_defaults() {
        let config = Config::parse("[ui]\nkeymap = \"mutt\"\n").unwrap();
        assert_eq!(config.ui.keymap, KeymapPreset::Mutt);
        assert_eq!(config.ui.theme, "dark");
    }

    #[test]
    fn a_syntax_error_reports_its_line() {
        let err = Config::parse("[ui]\ntheme = \"dark\"\nkeymap = \n").unwrap_err();
        match err {
            ConfigError::Parse { line, .. } => assert_eq!(line, 3),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        let err = Config::parse("[ui]\nthemee = \"dark\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { line: 2, .. }), "{err:?}");
        assert!(!err.to_string().contains('\n'));
    }

    #[test]
    fn an_unknown_enum_value_is_rejected() {
        assert!(Config::parse("[ui]\nkeymap = \"emacs\"\n").is_err());
        assert!(Config::parse("[privacy]\nremote_images = \"always\"\n").is_err());
    }

    #[test]
    fn the_font_is_a_name_and_a_size_a_terminal_can_have() {
        let config = Config::parse("[ui]\nfont = \"Menlo\"\nfont_size = 14.5\n").unwrap();
        assert_eq!(config.ui.font.as_deref(), Some("Menlo"));
        assert_eq!(config.ui.font_size, 14.5);
        for bad in [
            "font_size = 2",
            "font_size = 500",
            "font = \"\"",
            "font = \"a\\nb\"",
        ] {
            let err = Config::parse(&format!("[ui]\n{bad}\n")).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn the_accounts_pane_has_the_columns_it_is_given_within_reason() {
        assert_eq!(Config::parse("").unwrap().ui.sidebar_width, None);
        for columns in [SIDEBAR_NARROWEST, 24, SIDEBAR_WIDEST] {
            let config = Config::parse(&format!("[ui]\nsidebar_width = {columns}\n")).unwrap();
            assert_eq!(config.ui.sidebar_width, Some(columns));
        }
        for columns in ["0", "11", "61", "-3", "24.5", "\"wide\""] {
            assert!(
                Config::parse(&format!("[ui]\nsidebar_width = {columns}\n")).is_err(),
                "{columns}"
            );
        }
    }

    #[test]
    fn reader_width_must_be_in_range() {
        let err = Config::parse("[ui]\nreader_width = 0.95\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
    }

    #[test]
    fn duplicate_account_names_are_rejected() {
        let text = r#"
[[account]]
name = "Work"
type = "imap"
address = "a@example.com"

[[account]]
name = "Work"
type = "imap"
address = "b@example.com"
"#;
        let err = Config::parse(text).unwrap_err();
        assert!(err.to_string().contains("Work"), "{err}");
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let config = Config::load(Path::new("/nonexistent/quill/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }
}
