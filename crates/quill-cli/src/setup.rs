//! Reads the config, theme and key files once, before the UI starts.
//!
//! Nothing here is fatal. A file that cannot be used is reported as a notice
//! for the status bar and the defaults take its place, so a bad edit never
//! locks anyone out of their mail.

use std::io::ErrorKind;
use std::path::Path;

use jiff::tz::TimeZone;
use quill_core::config::Config;
use quill_core::config::{ComposeFormat, ReplyPosition};
use quill_tui::command::Registry;
use quill_tui::keymap::Keymap;
use quill_tui::state::{ComposeSettings, Env, Identity, Notice};
use quill_tui::theme::{ColorMode, Styles, Theme};

pub struct Loaded {
    pub config: Config,
    /// `config` is what the file says, not the defaults standing in for a
    /// file that could not be read.
    pub config_ok: bool,
    pub env: Env,
    pub notices: Vec<Notice>,
}

pub fn load(paths: &crate::paths::Paths, mode: ColorMode, tz: TimeZone) -> Loaded {
    let mut notices = Vec::new();

    let (config, config_ok) = match Config::load(&paths.config_file()) {
        Ok(config) => (config, true),
        Err(err) => {
            notices.push(Notice::error(format!("{err}. Using the default settings.")));
            (Config::default(), false)
        }
    };

    let theme = load_theme(paths, &config.ui.theme, &mut notices);

    let registry = Registry::builtin();
    let preset = Keymap::preset(config.ui.keymap);
    let keymap = match read_optional(&paths.keys_file()) {
        Ok(Some(text)) => preset
            .clone()
            .with_overrides(&text, &registry)
            .unwrap_or_else(|err| {
                notices.push(Notice::error(format!("{err}. Using the preset keys.")));
                preset
            }),
        Ok(None) => preset,
        Err(err) => {
            notices.push(Notice::error(format!("{err}. Using the preset keys.")));
            preset
        }
    };

    let env = Env {
        registry,
        keymap,
        styles: Styles::new(theme, mode),
        tz,
        reader_width: config.ui.reader_width,
        // Decided by the caller, once it knows what hosts the core.
        images: false,
        accounts: false,
        cmd_keys: false,
        compose: ComposeSettings {
            markdown: config.compose.format == ComposeFormat::Markdown,
            reply_above: config.compose.reply_position == ReplyPosition::Above,
            undo_send_seconds: config.compose.undo_send_seconds,
        },
        identities: config
            .accounts
            .iter()
            .map(|account| {
                (
                    account.name.clone(),
                    Identity {
                        full_name: account.full_name.clone(),
                        signature: account.signature.clone(),
                    },
                )
            })
            .collect(),
    };
    Loaded {
        config,
        config_ok,
        env,
        notices,
    }
}

/// A theme file in `themes/` wins over a built-in of the same name, so the
/// shipped themes can be customised by copying them.
fn load_theme(paths: &crate::paths::Paths, name: &str, notices: &mut Vec<Notice>) -> Theme {
    let fallback = || {
        Theme::builtin(name)
            .or_else(|| Theme::builtin("dark"))
            .unwrap_or_else(Theme::terminal)
    };
    let Some(file) = paths.theme_file(name) else {
        notices.push(Notice::error(format!(
            "\"{name}\" is not a theme name. Using the dark theme."
        )));
        return fallback();
    };
    match read_optional(&file) {
        Ok(Some(text)) => Theme::parse(&text).unwrap_or_else(|err| {
            notices.push(Notice::error(format!(
                "themes/{name}.toml: {err}. Using a built-in theme."
            )));
            fallback()
        }),
        Ok(None) => Theme::builtin(name).unwrap_or_else(|| {
            notices.push(Notice::error(format!(
                "There is no theme called \"{name}\". Using the dark theme."
            )));
            fallback()
        }),
        Err(err) => {
            notices.push(Notice::error(format!("{err}. Using a built-in theme.")));
            fallback()
        }
    }
}

/// The file's text, `None` when there is no such file, or a one-line error.
fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("cannot read {}: {err}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use quill_core::config::KeymapPreset;
    use quill_tui::command::CommandId;
    use quill_tui::state::NoticeKind;
    use tempfile::TempDir;

    use super::*;
    use crate::paths::Paths;

    fn paths(dir: &TempDir) -> Paths {
        Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
        }
    }

    fn write(dir: &TempDir, file: &str, text: &str) {
        let path = dir.path().join("config").join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn load_from(dir: &TempDir) -> Loaded {
        load(&paths(dir), ColorMode::TrueColor, TimeZone::UTC)
    }

    fn first_key(loaded: &Loaded, id: CommandId) -> String {
        loaded.env.keymap.keys_for(id)[0].to_string()
    }

    #[test]
    fn with_no_files_everything_is_default_and_nothing_is_reported() {
        let dir = TempDir::new().unwrap();
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        // No file at all is a config that was read: it says "the defaults".
        assert!(loaded.config_ok);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.env.styles.theme_name(), "dark");
        assert_eq!(first_key(&loaded, CommandId::MessageToggleUnread), "u");
        // Loading creates nothing on disk.
        assert!(!dir.path().join("config").exists());
        assert!(!dir.path().join("data").exists());
    }

    #[test]
    fn the_config_selects_the_theme_keymap_and_reader_width() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "config.toml",
            "[ui]\ntheme = \"light\"\nkeymap = \"mutt\"\nreader_width = 0.3\n",
        );
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert_eq!(loaded.config.ui.keymap, KeymapPreset::Mutt);
        assert_eq!(loaded.env.styles.theme_name(), "light");
        assert_eq!(first_key(&loaded, CommandId::MessageToggleUnread), "N");
        assert_eq!(loaded.env.reader_width, 0.3);
    }

    #[test]
    fn a_broken_config_is_reported_and_replaced_by_the_defaults() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", "[ui]\ntheme = \"light\"\nkeymap = \n");
        let loaded = load_from(&dir);
        assert_eq!(loaded.config, Config::default());
        assert!(!loaded.config_ok);
        assert_eq!(loaded.env.styles.theme_name(), "dark");
        assert_eq!(loaded.notices.len(), 1);
        assert_eq!(loaded.notices[0].kind, NoticeKind::Error);
        assert!(loaded.notices[0].text.starts_with("config line 3: "));
        assert!(!loaded.notices[0].text.contains('\n'));
    }

    #[test]
    fn a_user_theme_file_is_found_by_name() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", "[ui]\ntheme = \"mine\"\n");
        write(
            &dir,
            "themes/mine.toml",
            "name = \"mine\"\n[colors]\ntext = \"#112233\"\n",
        );
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert_eq!(loaded.env.styles.theme_name(), "mine");
    }

    #[test]
    fn a_user_theme_file_overrides_a_builtin_of_the_same_name() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "themes/dark.toml",
            "name = \"my dark\"\n[colors]\ntext = \"white\"\n",
        );
        let loaded = load_from(&dir);
        assert_eq!(loaded.env.styles.theme_name(), "my dark");
    }

    #[test]
    fn an_unknown_or_broken_theme_falls_back_with_a_notice() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", "[ui]\ntheme = \"nope\"\n");
        let loaded = load_from(&dir);
        assert_eq!(loaded.env.styles.theme_name(), "dark");
        assert!(loaded.notices[0].text.contains("no theme called \"nope\""));

        write(&dir, "config.toml", "[ui]\ntheme = \"light\"\n");
        write(
            &dir,
            "themes/light.toml",
            "name = \"light\"\n[colors]\ntext = \"mauve\"\n",
        );
        let loaded = load_from(&dir);
        // The broken file falls back to the built-in of the same name.
        assert_eq!(loaded.env.styles.theme_name(), "light");
        assert!(
            loaded.notices[0]
                .text
                .starts_with("themes/light.toml: theme line 3")
        );

        write(&dir, "config.toml", "[ui]\ntheme = \"../../etc/passwd\"\n");
        let loaded = load_from(&dir);
        assert_eq!(loaded.env.styles.theme_name(), "dark");
        assert!(loaded.notices[0].text.contains("is not a theme name"));
    }

    #[test]
    fn keys_toml_overrides_the_preset() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "keys.toml",
            "\"message.toggle_unread\" = [\"m\", \"u\"]\n",
        );
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert_eq!(first_key(&loaded, CommandId::MessageToggleUnread), "m");
    }

    #[test]
    fn a_bad_keys_toml_is_reported_and_the_preset_kept_whole() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "keys.toml",
            "\"message.toggle_unread\" = \"m\"\n\"no.such.command\" = \"x\"\n",
        );
        let loaded = load_from(&dir);
        assert_eq!(first_key(&loaded, CommandId::MessageToggleUnread), "u");
        assert_eq!(loaded.notices.len(), 1);
        assert!(loaded.notices[0].text.contains("no.such.command"));
    }
}
