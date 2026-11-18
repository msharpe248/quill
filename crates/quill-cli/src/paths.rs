//! Where Quill keeps its files.
//!
//! The same layout serves `quill` in a terminal and the core inside Quill's
//! app, and since the app is not sandboxed it is the same place: the two
//! are one mailbox, which only one of them may have open at a time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const CONFIG_DIR_VAR: &str = "QUILL_CONFIG_DIR";
pub const DATA_DIR_VAR: &str = "QUILL_DATA_DIR";
const APP_SUPPORT: &str = "Library/Application Support/Quill";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `config.toml`, `keys.toml`, `themes/` and `templates/`.
    pub config_dir: PathBuf,
    /// `quill.db` and, later, raw message blobs.
    pub data_dir: PathBuf,
}

impl Paths {
    /// A command-line flag beats the environment variable, which beats the
    /// default under the home directory. `env` looks up one variable.
    pub fn resolve(
        config_flag: Option<PathBuf>,
        data_flag: Option<PathBuf>,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> anyhow::Result<Self> {
        let choose = |flag: Option<PathBuf>, var: &str, leaf: &str| -> anyhow::Result<PathBuf> {
            if let Some(path) = flag {
                return Ok(path);
            }
            if let Some(path) = env(var).filter(|value| !value.is_empty()) {
                return Ok(PathBuf::from(path));
            }
            match env("HOME").filter(|value| !value.is_empty()) {
                Some(home) => Ok(Path::new(&home).join(APP_SUPPORT).join(leaf)),
                None => {
                    anyhow::bail!("HOME is not set; pass --config-dir and --data-dir, or set {var}")
                }
            }
        };
        Ok(Self {
            config_dir: choose(config_flag, CONFIG_DIR_VAR, "config")?,
            data_dir: choose(data_flag, DATA_DIR_VAR, "data")?,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// The folder of text kept ready to put into messages: each `.md` or
    /// `.txt` file in it is a template.
    pub fn templates_dir(&self) -> PathBuf {
        self.config_dir.join("templates")
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("quill.db")
    }
}

/// `keys.toml` in the config folder `config_dir`.
pub fn keys_file_in(config_dir: &Path) -> PathBuf {
    config_dir.join("keys.toml")
}

/// The folder of the user's own themes in the config folder `config_dir`.
pub fn themes_dir_in(config_dir: &Path) -> PathBuf {
    config_dir.join("themes")
}

/// Whether `name` could be the name of a theme of the user's own: the
/// name of a file in `themes/`, and nothing that could point outside it.
pub fn is_theme_name(name: &str) -> bool {
    quill_core::config::is_theme_name(name)
}

/// The files the user theme called `name` may be, in the config folder
/// `config_dir`, in the order they are looked for, each with whether it
/// is one of Quill's own, in TOML: `themes/NAME.toml`, then `themes/NAME`,
/// which is a theme written for Ghostty. None when the name could not be
/// a file's (so a theme name can never point outside `themes/`).
pub fn theme_files_in(config_dir: &Path, name: &str) -> Vec<(PathBuf, bool)> {
    if !is_theme_name(name) {
        return Vec::new();
    }
    let themes = themes_dir_in(config_dir);
    vec![
        (themes.join(format!("{name}.toml")), true),
        (themes.join(name), false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_default_is_application_support_under_home() {
        let paths = Paths::resolve(None, None, env(&[("HOME", "/Users/robin")])).unwrap();
        assert_eq!(
            paths.config_dir,
            Path::new("/Users/robin/Library/Application Support/Quill/config")
        );
        assert_eq!(
            paths.database(),
            Path::new("/Users/robin/Library/Application Support/Quill/data/quill.db")
        );
    }

    #[test]
    fn a_flag_beats_the_environment_which_beats_the_default() {
        let vars = [
            ("HOME", "/Users/robin"),
            ("QUILL_CONFIG_DIR", "/env/config"),
            ("QUILL_DATA_DIR", "/env/data"),
        ];
        let paths = Paths::resolve(Some("/flag/config".into()), None, env(&vars)).unwrap();
        assert_eq!(paths.config_dir, Path::new("/flag/config"));
        assert_eq!(paths.data_dir, Path::new("/env/data"));
        assert_eq!(paths.config_file(), Path::new("/flag/config/config.toml"));
        assert_eq!(
            keys_file_in(&paths.config_dir),
            Path::new("/flag/config/keys.toml")
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let vars = [("HOME", "/Users/robin"), ("QUILL_DATA_DIR", "")];
        let paths = Paths::resolve(None, None, env(&vars)).unwrap();
        assert!(paths.data_dir.starts_with("/Users/robin"));
    }

    #[test]
    fn without_home_the_directories_must_be_given() {
        assert!(Paths::resolve(None, None, env(&[])).is_err());
        let paths = Paths::resolve(Some("/c".into()), Some("/d".into()), env(&[])).unwrap();
        assert_eq!(paths.data_dir, Path::new("/d"));
    }

    #[test]
    fn theme_names_cannot_leave_the_themes_directory() {
        let paths = Paths::resolve(Some("/c".into()), Some("/d".into()), env(&[])).unwrap();
        assert_eq!(
            theme_files_in(&paths.config_dir, "solarized-dark_2"),
            [
                (PathBuf::from("/c/themes/solarized-dark_2.toml"), true),
                (PathBuf::from("/c/themes/solarized-dark_2"), false)
            ]
        );
        // As people name themes made for Ghostty.
        assert_eq!(
            theme_files_in(&paths.config_dir, "Catppuccin Mocha")[1],
            (PathBuf::from("/c/themes/Catppuccin Mocha"), false)
        );
        for name in [
            "",
            "../secret",
            "..",
            "a/b",
            "/etc/passwd",
            "x.toml",
            ".hidden",
            " a",
        ] {
            assert_eq!(theme_files_in(&paths.config_dir, name), [], "{name}");
            assert!(!is_theme_name(name), "{name}");
        }
    }
}
