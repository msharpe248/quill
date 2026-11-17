//! Themes saved and deleted from the settings window.
//!
//! A theme of the user's own is a file in the `themes` folder beside the
//! settings file. One saved from the window is written as one of Quill's
//! own, `NAME.toml`: over the file it was made from where that is one of
//! Quill's own too, touching only the lines of the colours that changed,
//! so that what the user wrote beside them is kept; and from nothing
//! otherwise, with every colour written out. Nothing is written that Quill
//! would not read back as the theme it was given.

use std::path::Path;

use quill_tui::theme::{ColorSpec, Colors, Slot, Theme};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::{config_edit, paths, setup, themes};

/// Said at the head of a theme file made from nothing.
const HEAD: &str = "# A theme for Quill, saved from its settings window.\n#\n\
                    # A colour is \"#rrggbb\", an ANSI name (\"blue\", \"bright-black\"), a \
                    256-colour\n# index, or \"default\" for the terminal's own colour.\n\n";

/// A colour as a theme file has it.
fn value_of(spec: ColorSpec) -> Value {
    match spec {
        ColorSpec::Default => Value::from("default"),
        ColorSpec::Rgb(red, green, blue) => Value::from(format!("#{red:02x}{green:02x}{blue:02x}")),
        ColorSpec::Indexed(index) => match ColorSpec::ansi_name(index) {
            Some(name) => Value::from(name),
            None => Value::from(i64::from(index)),
        },
    }
}

/// Puts `new` under `key`, where the value under it stands if there is
/// one, with whatever is written around it kept.
fn put(table: &mut Table, key: &str, new: Value) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = new;
            *existing.decor_mut() = decor;
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
}

/// The text of the file for `theme`. `base` is the text of the file it
/// was made from, where that is one of Quill's own: the lines of the
/// colours that are as they were are left exactly as they are.
pub fn written(base: Option<&str>, theme: &Theme) -> Result<String, String> {
    // What the base says, to tell what has changed: a base that cannot
    // be read is no base.
    let before = base.and_then(|text| Theme::parse(text).ok().map(|theme| (text, theme.colors)));
    // A file made from nothing starts as its heading and a name to fill
    // in, which the heading is written above.
    let empty = format!("{HEAD}name = \"\"\n");
    let (text, before) = match before {
        Some((text, colors)) => (text, Some(colors)),
        None => (empty.as_str(), None),
    };
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("the theme\u{2019}s file cannot be read: {err}"))?;
    put(
        document.as_table_mut(),
        "name",
        Value::from(theme.name.as_str()),
    );
    let fresh = !document.contains_key("colors");
    let colors = document
        .entry("colors")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("`colors` in the theme\u{2019}s file is not a table")?;
    if fresh {
        colors.decor_mut().set_prefix("\n");
    }
    for slot in Slot::FIXED {
        let was = before.as_ref().map(|colors| field(colors, slot));
        let now = field(&theme.colors, slot);
        if was == Some(now) {
            continue;
        }
        match now {
            Some(spec) => put(colors, slot.key(), value_of(spec)),
            None => {
                colors.remove(slot.key());
            }
        }
    }
    if before.as_ref().map(|colors| &colors.accounts) != Some(&theme.colors.accounts) {
        let accounts: Array = theme
            .colors
            .accounts
            .iter()
            .copied()
            .map(value_of)
            .collect();
        put(colors, "accounts", Value::Array(accounts));
    }
    let text = document.to_string();
    match Theme::parse(&text) {
        Ok(read) if read == *theme => Ok(text),
        Ok(_) => Err("the theme would not read back as it was made".to_owned()),
        Err(err) => Err(format!("the theme would not read back: {err}")),
    }
}

/// What a file says of a slot: the colour, or nothing for the one colour
/// a theme may leave out and mean something by.
fn field(colors: &Colors, slot: Slot) -> Option<ColorSpec> {
    match slot {
        Slot::SelectionInactiveBg => colors.selection_inactive_bg,
        other => colors.get(other),
    }
}

/// Saves `theme` into the themes folder of `config_dir` under the name it
/// has. It was made from the theme called `from`: under that name it
/// takes its place, and under another it is a new theme, which no theme
/// may have the name of already.
pub fn save(config_dir: &Path, from: &str, theme: &Theme) -> Result<(), String> {
    let name = theme.name.as_str();
    if !paths::is_theme_name(name) {
        return Err(format!("\"{name}\" cannot be a theme\u{2019}s name."));
    }
    let own = |name: &str| {
        paths::theme_files_in(config_dir, name)
            .iter()
            .any(|(file, _)| file.exists())
    };
    if name != from
        && setup::themes_in(config_dir)
            .iter()
            .any(|other| other.to_lowercase() == name.to_lowercase())
    {
        return Err(format!("There is already a theme called \"{name}\"."));
    }
    // One of Quill's own is saved over only where a file of the user's
    // already stands in its place.
    if Theme::builtin_names().any(|builtin| builtin == name) && !own(name) {
        return Err(format!(
            "\"{name}\" is a theme Quill comes with, and those stay as they are."
        ));
    }
    let themes_dir = paths::themes_dir_in(config_dir);
    // The file it was made from, where that is one of Quill's own.
    let base = paths::theme_files_in(config_dir, from)
        .into_iter()
        .find(|(file, _)| file.exists())
        .filter(|(_, toml)| *toml)
        .and_then(|(file, _)| setup::read_theme(&file).ok().flatten());
    let text = written(base.as_deref(), theme)?;
    let file = themes_dir.join(format!("{name}.toml"));
    themes::write_private(&file, &text)
        .map_err(|err| format!("{} cannot be written: {err}.", file.display()))?;
    // A theme made for Ghostty that was changed under its own name is
    // one of Quill's from now: the file it was is in the way of nothing,
    // since `NAME.toml` is found first, and is taken away so that deleting
    // the theme later leaves none behind.
    if name == from {
        let _ = std::fs::remove_file(themes_dir.join(name));
    }
    Ok(())
}

/// Deletes the theme called `name`: every file in the themes folder of
/// `config_dir` that it is. The settings file is made to stop naming it
/// first, and if that cannot be done nothing is deleted. A theme Quill
/// comes with has no file to delete, unless one of the user's stands in
/// its place: then that goes, and the settings go on naming the theme.
pub fn delete(config_file: &Path, config_dir: &Path, name: &str) -> Result<(), String> {
    let files: Vec<_> = paths::theme_files_in(config_dir, name)
        .into_iter()
        .map(|(file, _)| file)
        .filter(|file| file.is_file())
        .collect();
    if files.is_empty() {
        return Err(format!(
            "There is no file for a theme called \"{name}\" in the themes folder."
        ));
    }
    if !Theme::builtin_names().any(|builtin| builtin == name) {
        let before = match std::fs::read_to_string(config_file) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("config.toml cannot be read: {err}")),
        };
        let after = config_edit::forget_theme(&before, name)?;
        if after != before {
            config_edit::write(config_file, &after)
                .map_err(|err| format!("config.toml cannot be written: {err}"))?;
        }
    }
    for file in files {
        std::fs::remove_file(&file)
            .map_err(|err| format!("{} cannot be deleted: {err}.", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const OWN: &str = "# Mine, for the evenings.\nname = \"mine\"\n\n[colors]\n\
                       background = \"#101010\"  # nearly black\ntext = \"#E0E0E0\"\n\
                       dim = \"bright-black\"\nborder = 244\n";
    const NIGHT: &str = "palette = 1=#f0606a\npalette = 4=#7aa0f0\nbackground = #1e2030\n\
                         foreground = #d0d4ea\n";

    fn rig() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir_all(config.join("themes")).unwrap();
        (dir, config)
    }

    #[test]
    fn a_theme_made_from_nothing_has_every_colour_written_and_reads_back() {
        for name in Theme::builtin_names() {
            let mut theme = Theme::builtin(name).unwrap();
            theme.name = "Evening".to_owned();
            let text = written(None, &theme).unwrap();
            assert_eq!(Theme::parse(&text).unwrap(), theme, "{name}");
            assert!(text.starts_with("# A theme for Quill"), "{text}");
            assert!(
                text.contains("\nname = \"Evening\"\n\n[colors]\n"),
                "{text}"
            );
        }
        // The terminal's own colours are written as a person writes them.
        let mut theme = Theme::terminal();
        theme.name = "t".to_owned();
        let text = written(None, &theme).unwrap();
        assert!(text.contains("background = \"default\"\n"), "{text}");
        assert!(text.contains("dim = \"bright-black\"\n"), "{text}");
        // The one colour a theme may leave out is left out.
        assert!(!text.contains("selection_inactive_bg"), "{text}");
        theme.colors.selection_inactive_bg = Some(ColorSpec::Indexed(236));
        let text = written(None, &theme).unwrap();
        assert!(text.contains("selection_inactive_bg = 236\n"), "{text}");
    }

    #[test]
    fn a_theme_changed_keeps_the_lines_that_did_not_change_as_they_were_written() {
        let mut theme = Theme::parse(OWN).unwrap();
        theme.colors.text = ColorSpec::Rgb(0xff, 0xee, 0xdd);
        theme.colors.error = ColorSpec::Rgb(0xff, 0, 0);
        let text = written(Some(OWN), &theme).unwrap();
        assert_eq!(
            text,
            "# Mine, for the evenings.\nname = \"mine\"\n\n[colors]\n\
             background = \"#101010\"  # nearly black\ntext = \"#ffeedd\"\n\
             dim = \"bright-black\"\nborder = 244\nerror = \"#ff0000\"\n"
        );
        // Under another name it is the same file but for the name.
        theme.name = "mine too".to_owned();
        let text = written(Some(OWN), &theme).unwrap();
        assert!(
            text.starts_with("# Mine, for the evenings.\nname = \"mine too\"\n"),
            "{text}"
        );
        assert!(text.contains("# nearly black"), "{text}");
        // With nothing changed, the file is as it was.
        assert_eq!(
            written(Some(OWN), &Theme::parse(OWN).unwrap()).unwrap(),
            OWN
        );
        // A base that is not a theme of Quill's is no base.
        let text = written(Some(NIGHT), &theme).unwrap();
        assert!(text.starts_with("# A theme for Quill"), "{text}");
        assert_eq!(Theme::parse(&text).unwrap(), theme);
    }

    #[test]
    fn a_theme_is_saved_over_itself_or_under_a_new_name_and_never_over_another() {
        let (_dir, config) = rig();
        let themes = config.join("themes");
        std::fs::write(themes.join("mine.toml"), OWN).unwrap();
        std::fs::write(themes.join("Night"), NIGHT).unwrap();

        // Over itself.
        let mut mine = Theme::parse(OWN).unwrap();
        mine.colors.accent = ColorSpec::Rgb(1, 2, 3);
        save(&config, "mine", &mine).unwrap();
        let (read, problem) = setup::theme_named(&config, "mine");
        assert_eq!((read, problem), (mine.clone(), None));
        assert!(
            std::fs::read_to_string(themes.join("mine.toml"))
                .unwrap()
                .contains("# nearly black")
        );

        // Under a new name: a new file, and the old one as it was.
        let mut evening = mine.clone();
        evening.name = "Evening".to_owned();
        save(&config, "mine", &evening).unwrap();
        assert_eq!(setup::theme_named(&config, "Evening").0, evening);
        assert_eq!(setup::theme_named(&config, "mine").0, mine);

        // One made for Ghostty, changed under its own name, is one of
        // Quill's from then, and the file it was is gone.
        let (mut night, _) = setup::theme_named(&config, "Night");
        night.colors.text = ColorSpec::Rgb(9, 9, 9);
        save(&config, "Night", &night).unwrap();
        assert!(!themes.join("Night").exists());
        assert_eq!(setup::theme_named(&config, "Night"), (night.clone(), None));
        assert_eq!(
            setup::themes_in(&config),
            ["dark", "light", "terminal", "Evening", "Night", "mine"]
        );

        // Never over another theme, whatever its capitals, nor over one
        // Quill comes with, nor under a name that is none.
        let mut clash = night.clone();
        for (name, why) in [
            ("mine", "already a theme called"),
            ("EVENING", "already a theme called"),
            ("dark", "already a theme called"),
            ("a/b", "cannot be a theme"),
        ] {
            clash.name = name.to_owned();
            let refused = save(&config, "Night", &clash).unwrap_err();
            assert!(refused.contains(why), "{name}: {refused}");
        }
        let mut dark = Theme::builtin("dark").unwrap();
        dark.colors.text = ColorSpec::Rgb(1, 1, 1);
        let refused = save(&config, "dark", &dark).unwrap_err();
        assert!(refused.contains("Quill comes with"), "{refused}");
        // Unless a file of the user's already stands in its place.
        std::fs::write(themes.join("dark.toml"), "name = \"dark\"\n").unwrap();
        save(&config, "dark", &dark).unwrap();
        assert_eq!(setup::theme_named(&config, "dark").0, dark);
    }

    #[test]
    fn deleting_a_theme_takes_its_file_and_its_name_out_of_the_settings() {
        let (_dir, config) = rig();
        let themes = config.join("themes");
        let file = config.join("config.toml");
        std::fs::write(themes.join("mine.toml"), OWN).unwrap();
        std::fs::write(themes.join("Night"), NIGHT).unwrap();
        std::fs::write(
            &file,
            "# Mine.\n[ui]\ntheme = \"mine\"  # the one\ntheme_dark = \"Night\"\nkeymap = \"mutt\"\n",
        )
        .unwrap();

        delete(&file, &config, "Night").unwrap();
        assert!(!themes.join("Night").exists());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Mine.\n[ui]\ntheme = \"mine\"  # the one\nkeymap = \"mutt\"\n"
        );
        delete(&file, &config, "mine").unwrap();
        assert!(!themes.join("mine.toml").exists());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Mine.\n[ui]\nkeymap = \"mutt\"\n"
        );

        // What has no file is not deleted, and one Quill comes with has
        // none; a name that could point elsewhere is no theme's.
        for name in ["mine", "dark", "../config.toml", ""] {
            let refused = delete(&file, &config, name).unwrap_err();
            assert!(refused.contains("There is no file"), "{name}: {refused}");
        }
        assert!(file.exists());

        // A file of the user's in the place of one of Quill's goes, and
        // the settings go on naming the theme.
        std::fs::write(themes.join("light.toml"), "name = \"light\"\n").unwrap();
        std::fs::write(&file, "[ui]\ntheme = \"light\"\n").unwrap();
        delete(&file, &config, "light").unwrap();
        assert!(!themes.join("light.toml").exists());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "[ui]\ntheme = \"light\"\n"
        );

        // With a settings file that cannot be read, nothing is deleted.
        std::fs::write(themes.join("mine.toml"), OWN).unwrap();
        std::fs::write(&file, "[ui]\ntheme = \"mine\"\nkeymap = \n").unwrap();
        let refused = delete(&file, &config, "mine").unwrap_err();
        assert!(refused.contains("mistake"), "{refused}");
        assert!(themes.join("mine.toml").exists());
    }
}
