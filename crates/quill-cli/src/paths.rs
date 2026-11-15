//! Where Quill keeps its files.
//!
//! Inside the app sandbox `HOME` is the app's container, so the same layout
//! serves the standalone binary and the bundled one. This moves behind
//! `quill-host` when that crate arrives in M1.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

const CONFIG_DIR_VAR: &str = "QUILL_CONFIG_DIR";
const DATA_DIR_VAR: &str = "QUILL_DATA_DIR";
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

    pub fn keys_file(&self) -> PathBuf {
        self.config_dir.join("keys.toml")
    }

    /// The folder of text kept ready to put into messages: each `.md` or
    /// `.txt` file in it is a template.
    pub fn templates_dir(&self) -> PathBuf {
        self.config_dir.join("templates")
    }

    /// The file of a user theme, or `None` when the name could not be a file
    /// stem (so a theme name can never point outside `themes/`).
    pub fn theme_file(&self, name: &str) -> Option<PathBuf> {
        let plain = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        plain.then(|| self.config_dir.join("themes").join(format!("{name}.toml")))
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("quill.db")
    }
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
        assert_eq!(paths.keys_file(), Path::new("/flag/config/keys.toml"));
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
            paths.theme_file("solarized-dark_2"),
            Some(PathBuf::from("/c/themes/solarized-dark_2.toml"))
        );
        for name in ["", "../secret", "a/b", "/etc/passwd", "x.toml", "a b"] {
            assert_eq!(paths.theme_file(name), None, "{name}");
        }
    }
}
