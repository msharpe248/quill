//! Themes from a shell: `quill theme list` and `quill theme import`.
//!
//! Quill's own app imports a theme from a chooser in its settings window.
//! This is the same for a terminal: it finds the themes of a Ghostty
//! installed on this Mac, copies one into the `themes` folder beside the
//! settings file, and says how to select it. Which theme is in force is the
//! settings file's to say, and nothing here writes that file.
//!
//! A theme file is someone else's. It is read to a limit, read as a list of
//! colours and nothing more, and copied only when it is a theme Quill can
//! use; the copy is the user's alone to read and write.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Context;
use quill_core::config::Config;
use quill_tui::text;
use quill_tui::theme::{Theme, ghostty};
use serde_json::json;

use crate::paths::{self, Paths};
use crate::setup;

/// Where Ghostty keeps the themes it comes with, inside its application.
const INSIDE_GHOSTTY: &str = "Ghostty.app/Contents/Resources/ghostty/themes";

/// The most files of one folder that are looked at for themes.
const MOST_FILES: usize = 5_000;

/// Where themes made for Ghostty are looked for on this Mac, in order: the
/// Ghostty in `/Applications`, one in the user's own `Applications`, and
/// the themes the user keeps for Ghostty themselves. A name found in an
/// earlier folder is not taken from a later one.
pub fn ghostty_folders(home: Option<&Path>) -> Vec<PathBuf> {
    let mut folders = vec![Path::new("/Applications").join(INSIDE_GHOSTTY)];
    if let Some(home) = home {
        folders.push(home.join("Applications").join(INSIDE_GHOSTTY));
        folders.push(home.join(".config/ghostty/themes"));
    }
    folders
}

/// A theme made for Ghostty that is there to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Importable {
    pub name: String,
    pub file: PathBuf,
    /// Whether it is a dark theme.
    pub dark: bool,
}

/// The themes made for Ghostty in `folders`, by name, whatever case the
/// names are in: every file there that is one. A folder that is not there
/// has none.
pub fn importable(folders: &[PathBuf]) -> Vec<Importable> {
    let mut found: Vec<Importable> = Vec::new();
    for folder in folders {
        let files = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(MOST_FILES);
        let mut here = Vec::new();
        for entry in files {
            let file = entry.path();
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !paths::is_theme_name(name) || found.iter().any(|theme| theme.name == name) {
                continue;
            }
            let Some(theme) = setup::read_theme(&file)
                .ok()
                .flatten()
                .and_then(|text| ghostty::parse(&text).ok())
            else {
                continue;
            };
            here.push(Importable {
                name: name.to_owned(),
                dark: theme.is_dark(),
                file,
            });
        }
        found.extend(here);
    }
    found.sort_by(|one, other| {
        (one.name.to_lowercase(), &one.name).cmp(&(other.name.to_lowercase(), &other.name))
    });
    found
}

/// A theme that was copied into the `themes` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// What the theme is called from now on: what the settings name it by.
    pub name: String,
    /// The copy.
    pub file: PathBuf,
    /// There was a theme of that name already, and this is in its place.
    pub replaced: bool,
    /// Whether it is a dark theme, where it fixes its own background.
    pub dark: Option<bool>,
}

/// Copies a theme into `themes_dir`: the theme made for Ghostty called
/// `what` in one of `folders`, or the file at the path `what`, which may
/// also be one of Quill's own themes. `Err` says why nothing was copied.
pub fn import(what: &str, folders: &[PathBuf], themes_dir: &Path) -> Result<Imported, String> {
    let source = source_of(what, folders)?;
    // A path is whatever was typed, and a terminal obeys what it is sent.
    let said = text::sanitize_line(&source.display().to_string());
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{said} has no name a theme could have"))?;
    let text = match setup::read_theme(&source) {
        Ok(Some(text)) => text,
        Ok(None) => return Err(format!("There is no file at {said}.")),
        Err(why) => return Err(format!("{said} {why}.")),
    };
    // One of Quill's own keeps the name it is filed under, and its
    // `.toml`; anything else is a theme made for Ghostty or it is nothing.
    let (stem, toml) = match file_name.strip_suffix(".toml") {
        Some(stem) => (stem, true),
        None => (file_name, false),
    };
    let quill = Theme::parse(&text).is_ok();
    if !paths::is_theme_name(stem) {
        return Err(format!(
            "\"{}\" cannot be a theme\u{2019}s name. Copy the file under a plainer name and \
             import that.",
            text::sanitize_line(stem)
        ));
    }
    let theme = Theme::from_file(stem, &text, toml).map_err(|why| {
        if toml {
            format!("{said}: {why}.")
        } else {
            format!("{said} {why}.")
        }
    })?;
    if Theme::builtin_names().any(|builtin| builtin == stem) {
        return Err(format!(
            "\"{stem}\" is the name of a theme Quill comes with. Copy the file under another \
             name and import that."
        ));
    }
    let file = if quill {
        themes_dir.join(format!("{stem}.toml"))
    } else {
        // A theme of Quill's own under the same name would be found
        // first, and this one never seen.
        if themes_dir.join(format!("{stem}.toml")).exists() {
            return Err(format!(
                "There is already a theme called \"{stem}\" in {}, in {stem}.toml. Remove it \
                 or rename it first.",
                themes_dir.display()
            ));
        }
        themes_dir.join(stem)
    };
    let replaced = file.exists();
    write_private(&file, &text)
        .map_err(|err| format!("{} cannot be written: {err}.", file.display()))?;
    Ok(Imported {
        name: stem.to_owned(),
        file,
        replaced,
        dark: theme.is_dark(),
    })
}

/// The file `what` means: a path, if it has the look of one or is one, and
/// otherwise the theme of that name in `folders`, in whatever case when
/// only one has it.
fn source_of(what: &str, folders: &[PathBuf]) -> Result<PathBuf, String> {
    let path = Path::new(what);
    if what.contains('/') || path.is_file() {
        return Ok(path.to_owned());
    }
    let themes = importable(folders);
    if let Some(exact) = themes.iter().find(|theme| theme.name == what) {
        return Ok(exact.file.clone());
    }
    let lower = what.to_lowercase();
    let mut alike = themes
        .iter()
        .filter(|theme| theme.name.to_lowercase() == lower);
    match (alike.next(), alike.next()) {
        (Some(only), None) => Ok(only.file.clone()),
        (Some(first), Some(second)) => Err(format!(
            "\"{first}\" and \"{second}\" are both there. Say which, exactly.",
            first = first.name,
            second = second.name
        )),
        _ => Err(format!(
            "There is no theme made for Ghostty called \"{}\" on this Mac, and no file of \
             that name here. `quill theme list --ghostty` lists the ones there are.",
            text::sanitize_line(what)
        )),
    }
}

/// Writes `text` to `file`, all or nothing, for the user alone to read
/// and write: beside the old one, and moved into its place. The folder is
/// made if it is not there, the user's alone too.
fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    let folder = file
        .parent()
        .ok_or_else(|| std::io::Error::other("the theme has no folder to go in"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    let partial = folder.join(format!(".import.{}", std::process::id()));
    let _ = std::fs::remove_file(&partial);
    let result = (|| {
        let mut staged = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)?;
        // Said outright as well: the mode asked for is cut down by the
        // umask.
        staged.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        staged.write_all(text.as_bytes())?;
        staged.sync_all()?;
        std::fs::rename(&partial, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn appearance_word(dark: Option<bool>) -> &'static str {
    match dark {
        Some(true) => "dark",
        Some(false) => "light",
        None => "the terminal\u{2019}s own",
    }
}

/// `quill theme list`: the themes there are to choose from, which of them
/// the settings name, and with `ghostty` the themes made for Ghostty that
/// are on this Mac to import.
pub fn list(
    out: &mut dyn Write,
    paths: &Paths,
    folders: &[PathBuf],
    ghostty: bool,
    json: bool,
) -> anyhow::Result<()> {
    let config = Config::load(&paths.config_file())?;
    let names = setup::themes_in(&paths.config_dir);
    let (light, dark) = (config.ui.theme_for(false), config.ui.theme_for(true));
    let builtin = |name: &str| Theme::builtin_names().any(|builtin| builtin == name);
    let importable = if ghostty {
        importable(folders)
    } else {
        Vec::new()
    };
    if json {
        let themes: Vec<_> = names
            .iter()
            .map(|name| {
                let (theme, problem) = setup::theme_named(&paths.config_dir, name);
                json!({
                    "name": name,
                    "builtin": builtin(name),
                    "appearance": match theme.is_dark().filter(|_| problem.is_none()) {
                        Some(true) => json!("dark"),
                        Some(false) => json!("light"),
                        None => serde_json::Value::Null,
                    },
                    "problem": problem,
                    "theme_light": name == light,
                    "theme_dark": name == dark,
                })
            })
            .collect();
        let mut value = json!({
            "appearance": config.ui.appearance.as_str(),
            "themes": themes,
        });
        if ghostty {
            value["ghostty"] = importable
                .iter()
                .map(|theme| {
                    json!({
                        "name": theme.name,
                        "file": theme.file,
                        "appearance": if theme.dark { "dark" } else { "light" },
                    })
                })
                .collect();
        }
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }
    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    for name in &names {
        let (theme, problem) = setup::theme_named(&paths.config_dir, name);
        let mut said = vec![if builtin(name) {
            "comes with Quill".to_owned()
        } else {
            "in the themes folder".to_owned()
        }];
        match problem {
            Some(problem) => said.push(format!("cannot be used: {problem}")),
            None => said.push(appearance_word(theme.is_dark()).to_owned()),
        }
        if name == light && name == dark {
            said.push("the theme in the settings".to_owned());
        } else if name == light {
            said.push("the light theme in the settings".to_owned());
        } else if name == dark {
            said.push("the dark theme in the settings".to_owned());
        }
        let name = text::sanitize_line(name);
        let pad = " ".repeat(width.saturating_sub(name.chars().count()));
        writeln!(
            out,
            "{name}{pad}  {}",
            text::sanitize_line(&said.join(", "))
        )?;
    }
    if ghostty {
        writeln!(out)?;
        if importable.is_empty() {
            writeln!(
                out,
                "No themes made for Ghostty were found on this Mac. They are looked for in:"
            )?;
            for folder in folders {
                writeln!(out, "  {}", folder.display())?;
            }
        } else {
            writeln!(
                out,
                "Themes made for Ghostty on this Mac ({}). Import one with `quill theme import NAME`:",
                importable.len()
            )?;
            let width = importable
                .iter()
                .map(|theme| theme.name.chars().count())
                .max()
                .unwrap_or(0);
            for theme in &importable {
                let name = text::sanitize_line(&theme.name);
                let pad = " ".repeat(width.saturating_sub(name.chars().count()));
                writeln!(
                    out,
                    "{name}{pad}  {}",
                    if theme.dark { "dark" } else { "light" }
                )?;
            }
        }
    }
    Ok(())
}

/// `quill theme import`: copies a theme into the `themes` folder beside
/// the settings file and says how to select it.
pub fn import_command(
    out: &mut dyn Write,
    paths: &Paths,
    folders: &[PathBuf],
    what: &str,
    json: bool,
) -> anyhow::Result<()> {
    let themes_dir = paths::themes_dir_in(&paths.config_dir);
    let imported = import(what, folders, &themes_dir).map_err(anyhow::Error::msg)?;
    if json {
        let value = json!({
            "name": imported.name,
            "file": imported.file,
            "replaced": imported.replaced,
            "appearance": match imported.dark {
                Some(true) => json!("dark"),
                Some(false) => json!("light"),
                None => serde_json::Value::Null,
            },
        });
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }
    let name = text::sanitize_line(&imported.name);
    writeln!(
        out,
        "{} \"{name}\" {} {}.",
        if imported.replaced {
            "Replaced"
        } else {
            "Imported"
        },
        if imported.replaced { "in" } else { "into" },
        themes_dir.display()
    )?;
    // Written as TOML writes it, so that a name with a quote in it is
    // still a line that can be pasted.
    let quoted = toml_edit::Value::from(imported.name.as_str()).to_string();
    let (key, while_it_is) = match imported.dark {
        Some(false) => ("theme_light", "light"),
        _ => ("theme_dark", "dark"),
    };
    writeln!(
        out,
        "It is {} theme. To use it, put one of these under [ui] in {}:",
        match imported.dark {
            Some(true) => "a dark",
            Some(false) => "a light",
            None => "a",
        },
        paths.config_file().display()
    )?;
    writeln!(out)?;
    writeln!(out, "    theme = {quoted}")?;
    writeln!(out, "    {key} = {quoted}")?;
    writeln!(out)?;
    writeln!(
        out,
        "`theme` is the one theme, whenever theme_light and theme_dark do not say otherwise. \
         `{key}` is the theme only while Quill is {while_it_is}: `appearance` says which it \
         is, \"system\", \"light\" or \"dark\", and in a terminal \"system\" is dark."
    )?;
    Ok(())
}

/// The folders of themes made for Ghostty, for the user whose home the
/// environment names.
pub fn ghostty_folders_here() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    ghostty_folders(home.as_deref().map(Path::new))
}

/// The paths the theme commands go by, from the flags and the environment.
pub fn paths(config_dir: Option<PathBuf>, data_dir: Option<PathBuf>) -> anyhow::Result<Paths> {
    Paths::resolve(config_dir, data_dir, |name| std::env::var_os(name))
        .context("cannot tell where the settings are")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// A dark theme in Ghostty's format, written here.
    const NIGHT: &str = "palette = 1=#f0606a\npalette = 4=#7aa0f0\nbackground = #1e2030\n\
                         foreground = #d0d4ea\nselection-background = #3a3f5c\n\
                         selection-foreground = #ffffff\n";
    /// A light one.
    const PAPER: &str = "background = #fbf6e8\nforeground = #5a6068\npalette = 4=#2a5db0\n";
    /// One of Quill's own.
    const OWN: &str = "name = \"mine\"\n[colors]\nbackground = \"#101010\"\ntext = \"#e0e0e0\"\n";

    struct Rig {
        dir: TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let rig = Self {
                dir: TempDir::new().unwrap(),
            };
            // Two folders of themes, as Ghostty's own and the user's are.
            rig.write("ghostty/Night Owl+", NIGHT);
            rig.write("ghostty/Paper", PAPER);
            rig.write("ghostty/notes.txt", "Remember the milk.\n");
            rig.write("ghostty/.hidden", NIGHT);
            fs::create_dir_all(rig.dir.path().join("ghostty/A Folder")).unwrap();
            rig.write("user/Mine", NIGHT);
            // The same name again: the first folder's is the one.
            rig.write("user/Paper", NIGHT);
            rig
        }

        fn write(&self, name: &str, text: &str) {
            let path = self.dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn folders(&self) -> Vec<PathBuf> {
            vec![
                self.dir.path().join("ghostty"),
                self.dir.path().join("user"),
                self.dir.path().join("not there"),
            ]
        }

        fn paths(&self) -> Paths {
            Paths {
                config_dir: self.dir.path().join("config"),
                data_dir: self.dir.path().join("data"),
            }
        }

        fn themes(&self) -> PathBuf {
            self.dir.path().join("config/themes")
        }

        fn import(&self, what: &str) -> Result<Imported, String> {
            import(what, &self.folders(), &self.themes())
        }

        fn list(&self, ghostty: bool, json: bool) -> String {
            let mut out = Vec::new();
            list(&mut out, &self.paths(), &self.folders(), ghostty, json).unwrap();
            String::from_utf8(out).unwrap()
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn ghosttys_themes_are_looked_for_in_three_places_in_order() {
        assert_eq!(
            ghostty_folders(Some(Path::new("/Users/robin"))),
            [
                PathBuf::from("/Applications/Ghostty.app/Contents/Resources/ghostty/themes"),
                PathBuf::from(
                    "/Users/robin/Applications/Ghostty.app/Contents/Resources/ghostty/themes"
                ),
                PathBuf::from("/Users/robin/.config/ghostty/themes"),
            ]
        );
        // With no home to look in, the one place every user shares.
        assert_eq!(ghostty_folders(None).len(), 1);
    }

    #[test]
    fn the_themes_there_are_to_import_are_the_files_that_are_themes() {
        let rig = Rig::new();
        let found = importable(&rig.folders());
        let names: Vec<(&str, bool)> = found
            .iter()
            .map(|theme| (theme.name.as_str(), theme.dark))
            .collect();
        // Not the notes, the hidden file or the folder; and Paper once,
        // from the first folder, where it is a light theme.
        assert_eq!(
            names,
            [("Mine", true), ("Night Owl+", true), ("Paper", false)]
        );
        assert!(found[2].file.starts_with(rig.dir.path().join("ghostty")));
        assert_eq!(importable(&[rig.dir.path().join("not there")]), []);
        assert_eq!(importable(&[]), []);
    }

    #[test]
    fn a_theme_is_imported_by_name_and_is_then_one_to_choose() {
        let rig = Rig::new();
        let imported = rig.import("Night Owl+").unwrap();
        assert_eq!(imported.name, "Night Owl+");
        assert_eq!(imported.file, rig.themes().join("Night Owl+"));
        assert_eq!((imported.replaced, imported.dark), (false, Some(true)));
        // The copy is the file, byte for byte, and the user's alone.
        assert_eq!(fs::read_to_string(&imported.file).unwrap(), NIGHT);
        assert_eq!(mode(&imported.file), 0o600);
        assert_eq!(mode(&rig.themes()), 0o700);
        // Nothing else is left in the folder.
        let left: Vec<_> = fs::read_dir(rig.themes())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Night Owl+"]);
        // Quill lists it and loads it under that name.
        let config = rig.paths().config_dir;
        assert_eq!(
            setup::themes_in(&config),
            ["dark", "light", "terminal", "Night Owl+"]
        );
        let (theme, problem) = setup::theme_named(&config, "Night Owl+");
        assert_eq!((theme.name.as_str(), problem), ("Night Owl+", None));
        assert_eq!(theme.is_dark(), Some(true));

        // In whatever case, when only one theme has the name.
        let imported = rig.import("paper").unwrap();
        assert_eq!(
            (imported.name.as_str(), imported.dark),
            ("Paper", Some(false))
        );
        // Again: the copy is replaced, and that is said.
        assert!(rig.import("Paper").unwrap().replaced);
        // The source is as it was.
        assert_eq!(
            fs::read_to_string(rig.dir.path().join("ghostty/Paper")).unwrap(),
            PAPER
        );
    }

    #[test]
    fn a_theme_is_imported_from_a_file_anywhere() {
        let rig = Rig::new();
        rig.write("elsewhere/Dusk", NIGHT);
        rig.write("elsewhere/mine.toml", OWN);
        let dusk = rig.dir.path().join("elsewhere/Dusk");
        let imported = rig.import(dusk.to_str().unwrap()).unwrap();
        assert_eq!(imported.file, rig.themes().join("Dusk"));
        // One of Quill's own keeps its `.toml`, and is named by its file.
        let own = rig.dir.path().join("elsewhere/mine.toml");
        let imported = rig.import(own.to_str().unwrap()).unwrap();
        assert_eq!(imported.name, "mine");
        assert_eq!(imported.file, rig.themes().join("mine.toml"));
        assert_eq!(imported.dark, Some(true));
        // A theme made for Ghostty that somebody called `.toml` is still
        // that theme, filed under its name.
        rig.write("elsewhere/Dawn.toml", PAPER);
        let dawn = rig.dir.path().join("elsewhere/Dawn.toml");
        let imported = rig.import(dawn.to_str().unwrap()).unwrap();
        assert_eq!(imported.file, rig.themes().join("Dawn"));
        assert_eq!(
            setup::themes_in(&rig.paths().config_dir),
            ["dark", "light", "terminal", "Dawn", "Dusk", "mine"]
        );
    }

    #[test]
    fn what_is_not_a_theme_is_not_copied_and_why_is_said() {
        let rig = Rig::new();
        rig.write("elsewhere/notes", "Remember the milk.\n");
        rig.write("elsewhere/dark", NIGHT);
        rig.write(
            "elsewhere/broken.toml",
            "name = \"broken\"\n[colors]\ntext = \"mauve\"\n",
        );
        rig.write(
            "elsewhere/huge",
            &format!("{NIGHT}{}", "#\n".repeat(40_000)),
        );
        rig.write("elsewhere/bad\u{7}name", NIGHT);
        rig.write("config/themes/Mine.toml", OWN);
        let at = |name: &str| rig.dir.path().join("elsewhere").join(name);
        for (what, why) in [
            (
                "Nothing Of The Kind".to_owned(),
                "There is no theme made for Ghostty called",
            ),
            (at("gone").display().to_string(), "There is no file at"),
            (
                at("notes").display().to_string(),
                "is not a theme in Ghostty",
            ),
            (
                at("dark").display().to_string(),
                "is the name of a theme Quill comes with",
            ),
            (at("broken.toml").display().to_string(), "theme line 3"),
            (at("huge").display().to_string(), "is larger than 64 KiB"),
            (
                at("bad\u{7}name").display().to_string(),
                "cannot be a theme\u{2019}s name",
            ),
            (at("").display().to_string(), "is not a file"),
            // A theme of Quill's own has that name already.
            (
                "Mine".to_owned(),
                "There is already a theme called \"Mine\"",
            ),
        ] {
            let refused = rig.import(&what).unwrap_err();
            assert!(refused.contains(why), "{what}: {refused}");
            assert!(!refused.contains('\u{7}'), "{refused:?}");
        }
        // Nothing was copied.
        let left: Vec<_> = fs::read_dir(rig.themes())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Mine.toml"]);

        // Two themes that differ only in case: which is to be said.
        rig.write("ghostty/Dusk", NIGHT);
        rig.write("user/DUSK", PAPER);
        assert!(rig.import("dusk").unwrap_err().contains("are both there"));
        assert_eq!(rig.import("DUSK").unwrap().dark, Some(false));
    }

    #[test]
    fn the_list_says_what_there_is_and_what_the_settings_name() {
        let rig = Rig::new();
        assert_eq!(
            rig.list(false, false),
            "dark      comes with Quill, dark, the theme in the settings\n\
             light     comes with Quill, light\n\
             terminal  comes with Quill, the terminal\u{2019}s own\n"
        );
        rig.import("Night Owl+").unwrap();
        rig.write(
            "config/themes/broken.toml",
            "name = \"broken\"\n[colors]\ntext = \"mauve\"\n",
        );
        rig.write(
            "config/config.toml",
            "[ui]\nappearance = \"light\"\ntheme_light = \"light\"\ntheme_dark = \"Night Owl+\"\n",
        );
        let listed = rig.list(true, false);
        assert!(
            listed.starts_with(
                "dark        comes with Quill, dark\n\
                 light       comes with Quill, light, the light theme in the settings\n\
                 terminal    comes with Quill, the terminal\u{2019}s own\n\
                 Night Owl+  in the themes folder, dark, the dark theme in the settings\n\
                 broken      in the themes folder, cannot be used: themes/broken.toml: theme line 3"
            ),
            "{listed}"
        );
        assert!(
            listed.ends_with(
                "\nThemes made for Ghostty on this Mac (3). Import one with `quill theme import \
                 NAME`:\nMine        dark\nNight Owl+  dark\nPaper       light\n"
            ),
            "{listed}"
        );

        let value: serde_json::Value = serde_json::from_str(&rig.list(true, true)).unwrap();
        assert_eq!(value["appearance"], "light");
        let themes = value["themes"].as_array().unwrap();
        assert_eq!(themes.len(), 5);
        assert_eq!(themes[3]["name"], "Night Owl+");
        assert_eq!(themes[3]["builtin"], false);
        assert_eq!(themes[3]["appearance"], "dark");
        assert_eq!(themes[3]["theme_dark"], true);
        assert_eq!(themes[3]["theme_light"], false);
        assert_eq!(themes[1]["theme_light"], true);
        assert_eq!(themes[2]["appearance"], serde_json::Value::Null);
        assert!(
            themes[4]["problem"]
                .as_str()
                .unwrap()
                .contains("theme line 3")
        );
        let ghostty = value["ghostty"].as_array().unwrap();
        assert_eq!(ghostty.len(), 3);
        assert_eq!(ghostty[2]["name"], "Paper");
        assert_eq!(ghostty[2]["appearance"], "light");
        // Without the flag, Ghostty's are not looked for at all.
        let value: serde_json::Value = serde_json::from_str(&rig.list(false, true)).unwrap();
        assert!(value.get("ghostty").is_none());

        // With none anywhere, where they were looked for is said.
        let mut out = Vec::new();
        list(
            &mut out,
            &rig.paths(),
            &[rig.dir.path().join("not there")],
            true,
            false,
        )
        .unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("No themes made for Ghostty were found on this Mac."),
            "{said}"
        );
        assert!(said.contains("not there"), "{said}");
    }

    #[test]
    fn importing_says_how_to_select_the_theme_and_writes_no_settings() {
        let rig = Rig::new();
        let run = |what: &str, json: bool| {
            let mut out = Vec::new();
            import_command(&mut out, &rig.paths(), &rig.folders(), what, json)
                .map(|()| String::from_utf8(out).unwrap())
        };
        let said = run("Night Owl+", false).unwrap();
        assert!(
            said.starts_with(&format!(
                "Imported \"Night Owl+\" into {}.\nIt is a dark theme. To use it, put one of \
                 these under [ui] in {}:\n\n    theme = \"Night Owl+\"\n    theme_dark = \
                 \"Night Owl+\"\n\n",
                rig.themes().display(),
                rig.paths().config_file().display()
            )),
            "{said}"
        );
        assert!(said.contains("in a terminal \"system\" is dark."), "{said}");
        let line = said
            .lines()
            .find(|line| line.starts_with("    theme ="))
            .unwrap();
        let config = Config::parse(&format!("[ui]\n{line}\n")).unwrap();
        assert_eq!(
            (config.ui.theme_for(false), config.ui.theme_for(true)),
            ("Night Owl+", "Night Owl+")
        );
        // The settings file is the user's to change: none was made.
        assert!(!rig.paths().config_file().exists());
        // What it says to write is a line Quill reads back as that theme.
        let line = said
            .lines()
            .find(|line| line.starts_with("    theme_dark"))
            .unwrap();
        let config = Config::parse(&format!("[ui]\n{line}\n")).unwrap();
        assert_eq!(config.ui.theme_for(true), "Night Owl+");

        let said = run("Paper", false).unwrap();
        assert!(said.contains("It is a light theme."), "{said}");
        assert!(said.contains("\n    theme_light = \"Paper\"\n"), "{said}");
        let said = run("Paper", false).unwrap();
        assert!(said.starts_with("Replaced \"Paper\" in "), "{said}");

        let value: serde_json::Value = serde_json::from_str(&run("Mine", true).unwrap()).unwrap();
        assert_eq!(value["name"], "Mine");
        assert_eq!(value["replaced"], false);
        assert_eq!(value["appearance"], "dark");
        assert!(
            value["file"]
                .as_str()
                .unwrap()
                .ends_with("config/themes/Mine")
        );

        let refused = run("Nothing Of The Kind", false).unwrap_err().to_string();
        assert!(refused.contains("quill theme list --ghostty"), "{refused}");
    }

    /// Against the themes Ghostty really ships, where it is installed:
    /// read where they are, and copied only into a folder of the test's
    /// own. Skipped on a Mac without Ghostty.
    #[test]
    fn a_theme_ghostty_ships_is_imported_from_where_it_is_installed() {
        let folders = ghostty_folders(None);
        let found = importable(&folders);
        if found.is_empty() {
            eprintln!("skipped: no Ghostty themes at {}", folders[0].display());
            return;
        }
        let dir = TempDir::new().unwrap();
        let themes = dir.path().join("themes");
        let first = &found[0];
        let imported = import(&first.name, &folders, &themes).unwrap();
        assert_eq!(imported.name, first.name);
        assert_eq!(
            fs::read(&imported.file).unwrap(),
            fs::read(&first.file).unwrap()
        );
        assert_eq!(imported.dark, Some(first.dark));
    }
}
