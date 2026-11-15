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
    #[serde(rename = "account")]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// A built-in theme name, or the stem of a file in `themes/`.
    pub theme: String,
    pub keymap: KeymapPreset,
    /// The reader's share of the space right of the sidebar, in wide layouts.
    pub reader_width: f32,
    pub toolbar: bool,
    /// Terminal font for the app shell. `None` leaves the shell's default.
    pub font: Option<String>,
    pub font_size: f32,
    /// Take the mouse: click to select and open, wheel to scroll. While it
    /// is on, selecting text in the terminal needs shift held.
    pub mouse: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_owned(),
            keymap: KeymapPreset::Vim,
            reader_width: 0.5,
            toolbar: true,
            font: None,
            font_size: 13.0,
            mouse: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeymapPreset {
    #[default]
    Vim,
    Mutt,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComposeConfig {
    pub editor: ComposeEditor,
    pub format: ComposeFormat,
    pub reply_position: ReplyPosition,
    pub spell_check: bool,
    pub undo_send_seconds: u32,
}

impl Default for ComposeConfig {
    fn default() -> Self {
        Self {
            editor: ComposeEditor::Builtin,
            format: ComposeFormat::Markdown,
            reply_position: ReplyPosition::Above,
            spell_check: true,
            undo_send_seconds: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeEditor {
    #[default]
    Builtin,
    Vim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeFormat {
    #[default]
    Markdown,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyPosition {
    #[default]
    Above,
    Below,
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
    /// For a `pop3` account, whose mail is kept on this Mac: when the
    /// server's copy of a message is deleted. `"never"`, `"downloaded"`,
    /// or a number of days after it was first seen.
    #[serde(default)]
    pub delete_from_server: DeleteFromServer,
    #[serde(default)]
    pub smtp: Option<ServerConfig>,
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
        if !(0.2..=0.8).contains(&self.ui.reader_width) {
            return Err(ConfigError::Invalid(format!(
                "ui.reader_width must be between 0.2 and 0.8, got {}",
                self.ui.reader_width
            )));
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
editor = "builtin"        # "vim" once the bundled Vim milestone lands
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
        assert_eq!(policy("delete_from_server = \"never\""), DeleteFromServer::Never);
        assert_eq!(
            policy("delete_from_server = \"downloaded\""),
            DeleteFromServer::Downloaded
        );
        assert_eq!(
            policy("delete_from_server = 14"),
            DeleteFromServer::AfterDays(14)
        );
        assert_eq!(policy("delete_from_server = 0"), DeleteFromServer::Downloaded);
        for bad in ["delete_from_server = \"sometimes\"", "delete_from_server = -1"] {
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
