//! Reads the config, theme and key files once, before the UI starts.
//!
//! Nothing here is fatal. A file that cannot be used is reported as a notice
//! for the status bar and the defaults take its place, so a bad edit never
//! locks anyone out of their mail.

use std::io::ErrorKind;
use std::path::Path;

use jiff::tz::TimeZone;
use quill_core::config::Config;
use quill_core::config::{ComposeEditor, ComposeFormat, KeymapPreset, ReplyPosition};
use quill_tui::command::Registry;
use quill_tui::keymap::Keymap;
use quill_tui::state::{ComposeSettings, Env, Identity, Notice};
use quill_tui::theme::{ColorMode, Look, Styles, Theme};
use quill_tui::vim::ExternalEditor;

mod templates;

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

    let (look, problems) = look_of(&paths.config_dir, &config.ui);
    notices.extend(problems.into_iter().flatten().map(Notice::error));

    let registry = Registry::builtin();
    let (keymap, problem) = keymap_for(&paths.config_dir, config.ui.keymap, &registry);
    notices.extend(problem.map(Notice::error));

    let templates = templates::load(&paths.templates_dir(), &mut notices);
    let hooks = load_hooks(&paths.config_dir.join(HOOKS_FILE), &mut notices);

    let env = Env {
        registry,
        keymap,
        // Nobody has said what the system's appearance is yet: Quill's
        // own app does, once it is attached, and a terminal never does.
        styles: Styles::new(look.theme().clone(), mode),
        look,
        tz,
        reader_width: config.ui.reader_width,
        // Decided by the caller, once it knows what hosts the core.
        images: false,
        accounts: false,
        cmd_keys: false,
        mouse: config.ui.mouse,
        file_panel: false,
        thumbnails: false,
        // The usual place for what is downloaded. Inside the app's sandbox
        // this is not the user's own folder, and the app asks with a panel.
        downloads: std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| std::path::Path::new(&home).join("Downloads")),
        compose: ComposeSettings {
            markdown: config.compose.format == ComposeFormat::Markdown,
            reply_above: config.compose.reply_position == ReplyPosition::Above,
            undo_send_seconds: config.compose.undo_send_seconds,
            spell_check: config.compose.spell_check,
            vim: config.compose.editor != ComposeEditor::Builtin,
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
        remote_never: config.privacy.remote_images == quill_core::config::RemoteImages::Never,
        notifications: config.ui.notifications,
        threads: config.ui.threads,
        searches: config
            .searches
            .iter()
            .map(|search| (search.name.clone(), search.query.clone()))
            .collect(),
        templates,
        // Decided by the caller too: only Quill's own window has a checker.
        spell_check: false,
        // And only it has a sheet to show the add-an-account form in.
        sheets: false,
        // And only it has a Vim of its own; anywhere else the user's is run.
        editor_pane: false,
        // Asked for in the config, and meant only where there is a pane:
        // a terminal runs the user's own Vim whatever this says.
        // With Quill's own editor as the usual one, the editor a message
        // can still be handed to is Vim.
        external: match config.compose.editor {
            ComposeEditor::Builtin | ComposeEditor::Vim => ExternalEditor::Vim,
            ComposeEditor::Nano => ExternalEditor::Nano,
            ComposeEditor::Pico => ExternalEditor::Pico,
            ComposeEditor::Mg => ExternalEditor::Mg,
            ComposeEditor::Script => ExternalEditor::Script,
        },
        vim_program: None,
        hooks,
        // And whether programs may be run at all: not in Quill's own app.
        hooks_enabled: false,
        // And only it has a web view pane to read mail in. What the config
        // says is kept for it.
        web_pane: false,
        web_reading: config.ui.web_reading,
        // What the sync engine is told once, as it starts.
        cache_gb: config.storage.cache_gb,
    };
    Loaded {
        config,
        config_ok,
        env,
        notices,
    }
}

/// The keys of `preset` with the user's own from `keys.toml` on top, and
/// why theirs were left out if they were: then the preset is used whole.
pub fn keymap_for(
    config_dir: &Path,
    preset: KeymapPreset,
    registry: &Registry,
) -> (Keymap, Option<String>) {
    let preset = Keymap::preset(preset);
    match read_optional(&crate::paths::keys_file_in(config_dir)) {
        Ok(Some(text)) => match preset.clone().with_overrides(&text, registry) {
            Ok(keymap) => (keymap, None),
            Err(err) => (preset, Some(format!("{err}. Using the preset keys."))),
        },
        Ok(None) => (preset, None),
        Err(err) => (preset, Some(format!("{err}. Using the preset keys."))),
    }
}

/// Every key Quill has and what it does, as `quill keys` prints it: the
/// keys of the preset the config names with the user's own from
/// `keys.toml` on top, as the UI would have them. With it, whatever was
/// wrong with either file: the list is then of the keys Quill falls back
/// on, as the UI does.
pub fn keys_listing(paths: &crate::paths::Paths) -> (String, Vec<String>) {
    let mut problems = Vec::new();
    let config = Config::load(&paths.config_file()).unwrap_or_else(|err| {
        problems.push(format!("{err}. Using the default settings."));
        Config::default()
    });
    let registry = Registry::builtin();
    let (keymap, problem) = keymap_for(&paths.config_dir, config.ui.keymap, &registry);
    problems.extend(problem);
    let mut text = format!(
        "{} ({} keys)\n\n",
        quill_tui::view::help::ALL_KEYS_TITLE,
        config.ui.keymap.as_str()
    );
    // A terminal has no web pane, and this is a terminal's list.
    text.push_str(&quill_tui::view::help::all_text(&registry, &keymap, false));
    // How to see the same in Quill, by the keys as they are now.
    let key = |id| {
        keymap
            .keys_for(id)
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "the command palette".to_owned())
    };
    text.push_str(&format!(
        "\nIn Quill, {} lists the keys for where you are and {} lists them all.\n\
         Your own keys go in keys.toml, beside config.toml.\n",
        key(quill_tui::command::CommandId::HelpToggle),
        key(quill_tui::command::CommandId::HelpKeys),
    ));
    (text, problems)
}

/// Whether Quill is light or dark and the theme for each, as the config
/// says, with why another theme stands in for the light one and for the
/// dark one, if one does. A config that names one theme for both, as every
/// config did before there was an appearance to choose, has it read once
/// and whatever is wrong with it said once, under the dark one.
pub fn look_of(
    config_dir: &Path,
    ui: &quill_core::config::UiConfig,
) -> (Look, [Option<String>; 2]) {
    let (dark, dark_problem) = theme_named(config_dir, ui.theme_for(true));
    let (light, light_problem) = if ui.theme_for(false) == ui.theme_for(true) {
        (dark.clone(), None)
    } else {
        theme_named(config_dir, ui.theme_for(false))
    };
    (
        Look {
            appearance: ui.appearance,
            system_dark: None,
            light,
            dark,
        },
        [dark_problem, light_problem],
    )
}

/// The theme called `name`, and why another stands in for it if one does.
/// A theme file in `themes/` wins over a built-in of the same name, so the
/// shipped themes can be customised by copying them. The file is
/// `NAME.toml`, one of Quill's own, or plain `NAME`, one written for
/// Ghostty; the first of those there is.
pub fn theme_named(config_dir: &Path, name: &str) -> (Theme, Option<String>) {
    match load_theme(config_dir, name) {
        Ok(theme) => (theme, None),
        Err(NoTheme { why, instead }) => (
            Theme::builtin(name)
                .or_else(|| Theme::builtin("dark"))
                .unwrap_or_else(Theme::terminal),
            Some(format!("{why} {instead}")),
        ),
    }
}

/// Why there is no theme to be had under a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoTheme {
    /// What is wrong, as a sentence.
    pub why: String,
    /// What is said after it where another theme stands in.
    instead: &'static str,
}

/// The theme called `name`, or why there is none: no such theme, a name
/// that could not be one, or a file that cannot be read as a theme.
pub fn load_theme(config_dir: &Path, name: &str) -> Result<Theme, NoTheme> {
    const DARK: &str = "Using the dark theme.";
    const BUILTIN: &str = "Using a built-in theme.";
    let files = crate::paths::theme_files_in(config_dir, name);
    if files.is_empty() {
        return Err(NoTheme {
            why: format!("\"{name}\" is not a theme name."),
            instead: DARK,
        });
    }
    for (file, toml) in files {
        // As it is said in the folder: `themes/mine.toml`, `themes/Dracula`.
        let said = if toml {
            format!("themes/{name}.toml:")
        } else {
            format!("themes/{name}")
        };
        let unusable = |err: String| NoTheme {
            why: format!("{said} {err}."),
            instead: BUILTIN,
        };
        return match read_theme(&file) {
            Ok(Some(text)) => Theme::from_file(name, &text, toml).map_err(unusable),
            Ok(None) => continue,
            Err(err) => Err(unusable(err)),
        };
    }
    Theme::builtin(name).ok_or_else(|| NoTheme {
        why: format!("There is no theme called \"{name}\"."),
        instead: DARK,
    })
}

/// The themes there are to choose from, by name: the ones Quill comes
/// with, then the user's own, which are the files in `themes/`. A file
/// called `NAME.toml` is a theme called NAME, whatever is in it: one with
/// a mistake in it is still listed, and says what is wrong when it is
/// chosen. Any other file is listed under its whole name if it is a theme
/// written for Ghostty, and not otherwise: a folder of themes may have
/// notes in it. A file of theirs with a built-in's name is that theme,
/// and is not listed twice.
pub fn themes_in(config_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Theme::builtin_names().map(str::to_owned).collect();
    let mut own: Vec<String> = std::fs::read_dir(crate::paths::themes_dir_in(config_dir))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let file = path.file_name()?.to_str()?;
            let name = match file.strip_suffix(".toml") {
                Some(stem) => stem.to_owned(),
                None => {
                    let text = read_theme(&path).ok().flatten()?;
                    quill_tui::theme::ghostty::parse(&text).ok()?;
                    file.to_owned()
                }
            };
            (crate::paths::is_theme_name(&name) && !names.contains(&name)).then_some(name)
        })
        .collect();
    own.sort();
    own.dedup();
    // More than anyone has, and more than a pop-up is any use for.
    own.truncate(MOST_THEMES);
    names.extend(own);
    names
}

/// The most themes of the user's own that are offered to choose from.
const MOST_THEMES: usize = 500;

/// The text of a theme file, `None` when there is no such file, or what
/// is wrong with it, in words that follow its name: "is not a file". No
/// more of it is read than a theme can be ([`MAX_THEME_BYTES`]).
pub fn read_theme(path: &Path) -> Result<Option<String>, String> {
    use std::io::Read;

    // Looked at before it is opened: opening a pipe would wait for
    // whoever writes to it.
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot be read: {err}")),
    };
    if !metadata.is_file() {
        return Err("is not a file".to_owned());
    }
    let too_large = || format!("is larger than {} KiB", MAX_THEME_BYTES / 1024);
    if metadata.len() > MAX_THEME_BYTES {
        return Err(too_large());
    }
    // Read to the limit and no further, whatever the size was a moment ago.
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_THEME_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|err| format!("cannot be read: {err}"))?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err(too_large());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "is not UTF-8 text".to_owned())
}

/// A theme file larger than this is not read: the same for one of
/// Quill's own as for one written for Ghostty.
pub const MAX_THEME_BYTES: u64 = quill_tui::theme::ghostty::MAX_BYTES as u64;

/// The file beside `config.toml` that lists the user's hooks.
const HOOKS_FILE: &str = "hooks.toml";

/// A hooks file larger than this is not read. A few dozen programs with
/// their arguments are a fraction of it.
const MAX_HOOKS_BYTES: u64 = 64 * 1024;

/// The user's hooks: programs a message can be given to. No file is no
/// hooks. A file that cannot be used whole is not used at all, and says
/// so: it names programs to run, and half of it is not to be guessed at.
fn load_hooks(path: &Path, notices: &mut Vec<Notice>) -> Vec<quill_tui::hook::Hook> {
    use std::io::Read;

    let mut refuse = |why: String| {
        notices.push(Notice::error(format!(
            "{HOOKS_FILE}: {why}. No hooks are loaded."
        )));
        Vec::new()
    };
    // Looked at before it is opened: opening a pipe would wait for
    // whoever writes to it.
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => return refuse(format!("cannot be read: {err}")),
    };
    if !metadata.is_file() {
        return refuse("is not a file".to_owned());
    }
    let too_large = || format!("is larger than {} KiB", MAX_HOOKS_BYTES / 1024);
    if metadata.len() > MAX_HOOKS_BYTES {
        return refuse(too_large());
    }
    // Read to the limit and no further, whatever the size was a moment ago.
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(MAX_HOOKS_BYTES + 1).read_to_end(&mut bytes));
    if let Err(err) = read {
        return refuse(format!("cannot be read: {err}"));
    }
    if bytes.len() as u64 > MAX_HOOKS_BYTES {
        return refuse(too_large());
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return refuse("is not UTF-8 text".to_owned());
    };
    match quill_tui::hook::parse(&text) {
        Ok(hooks) => hooks,
        Err(why) => refuse(why),
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
    fn web_reading_starts_as_the_config_says_and_off_without_it() {
        let dir = TempDir::new().unwrap();
        let loaded = load_from(&dir);
        assert!(!loaded.env.web_reading);
        // Never by itself: only Quill's own window has the pane.
        assert!(!loaded.env.web_pane);
        write(&dir, "config.toml", "[ui]\nweb_reading = true\n");
        let loaded = load_from(&dir);
        assert!(loaded.env.web_reading && !loaded.env.web_pane);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
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
    fn a_theme_made_for_ghostty_is_found_by_its_files_name() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", "[ui]\ntheme = \"Night Owl+\"\n");
        write(
            &dir,
            "themes/Night Owl+",
            "# mine\nbackground = #1e2030\nforeground = #d0d4ea\npalette = 4=#7aa0f0\n",
        );
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert_eq!(loaded.env.styles.theme_name(), "Night Owl+");
        assert_eq!(loaded.env.styles.background_rgb(), Some((0x1e, 0x20, 0x30)));
        // One of Quill's own under the same name is the one found first.
        write(
            &dir,
            "themes/Night Owl+.toml",
            "name = \"owl\"\n[colors]\nbackground = \"#000000\"\n",
        );
        assert_eq!(load_from(&dir).env.styles.theme_name(), "owl");
        assert_eq!(
            themes_in(&dir.path().join("config")),
            ["dark", "light", "terminal", "Night Owl+"]
        );

        // A file that is no theme says so in Ghostty's terms, and the
        // dark theme stands in.
        write(&dir, "config.toml", "[ui]\ntheme = \"Notes\"\n");
        write(&dir, "themes/Notes", "Remember the milk.\n");
        let loaded = load_from(&dir);
        assert_eq!(loaded.env.styles.theme_name(), "dark");
        assert!(
            loaded.notices[0]
                .text
                .starts_with("themes/Notes is not a theme in Ghostty\u{2019}s format"),
            "{:?}",
            loaded.notices
        );
        // One too large to be a theme is not read at all.
        write(
            &dir,
            "themes/Notes",
            &format!(
                "background = #000000\nforeground = #ffffff\n{}",
                "#\n".repeat(40_000)
            ),
        );
        let loaded = load_from(&dir);
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0]
                .text
                .starts_with("themes/Notes is larger than 64 KiB"),
            "{:?}",
            loaded.notices
        );
        // Nor a folder.
        fs::remove_file(dir.path().join("config/themes/Notes")).unwrap();
        fs::create_dir_all(dir.path().join("config/themes/Notes")).unwrap();
        let loaded = load_from(&dir);
        assert!(
            loaded.notices[0]
                .text
                .starts_with("themes/Notes is not a file"),
            "{:?}",
            loaded.notices
        );
    }

    #[test]
    fn the_config_names_a_theme_for_each_appearance_and_theme_stands_for_the_rest() {
        use quill_core::config::Appearance;

        let dir = TempDir::new().unwrap();
        let look = |text: &str| {
            write(&dir, "config.toml", text);
            let loaded = load_from(&dir);
            assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
            let look = loaded.env.look;
            (
                look.appearance,
                look.light.name.clone(),
                look.dark.name.clone(),
                loaded.env.styles.theme_name().to_owned(),
            )
        };
        let all = |appearance, light: &str, dark: &str, drawn: &str| {
            (
                appearance,
                light.to_owned(),
                dark.to_owned(),
                drawn.to_owned(),
            )
        };
        // Nothing said, and `theme` alone, as every config was: one theme.
        assert_eq!(look(""), all(Appearance::System, "dark", "dark", "dark"));
        assert_eq!(
            look("[ui]\ntheme = \"light\"\n"),
            all(Appearance::System, "light", "light", "light")
        );
        // One of the two said: `theme` is still the other.
        assert_eq!(
            look("[ui]\ntheme = \"terminal\"\ntheme_light = \"light\"\n"),
            all(Appearance::System, "light", "terminal", "terminal")
        );
        // Both said, and which of them Quill is. With nobody to say what
        // the system is, as here and in a terminal, "system" is dark.
        let both = "theme_light = \"light\"\ntheme_dark = \"terminal\"\n";
        assert_eq!(
            look(&format!("[ui]\n{both}")),
            all(Appearance::System, "light", "terminal", "terminal")
        );
        assert_eq!(
            look(&format!("[ui]\nappearance = \"light\"\n{both}")),
            all(Appearance::Light, "light", "terminal", "light")
        );
        assert_eq!(
            look(&format!("[ui]\nappearance = \"dark\"\n{both}")),
            all(Appearance::Dark, "light", "terminal", "terminal")
        );

        // A theme there is none of is said once when it is named once,
        // and for each appearance when each names its own.
        write(&dir, "config.toml", "[ui]\ntheme = \"nope\"\n");
        assert_eq!(load_from(&dir).notices.len(), 1);
        write(
            &dir,
            "config.toml",
            "[ui]\ntheme_light = \"nope\"\ntheme_dark = \"neither\"\n",
        );
        let notices = load_from(&dir).notices;
        assert_eq!(notices.len(), 2);
        assert!(notices[0].text.contains("\"neither\""), "{notices:?}");
        assert!(notices[1].text.contains("\"nope\""), "{notices:?}");
    }

    #[test]
    fn the_printed_keys_are_the_presets_with_the_users_own_on_top() {
        let dir = TempDir::new().unwrap();
        let paths = paths(&dir);
        // No files at all: the default preset, and nothing to complain of.
        let (text, problems) = keys_listing(&paths);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            text.starts_with("Keyboard shortcuts (vim keys)\n\nNavigation\n"),
            "{text}"
        );
        assert!(text.contains("  u  "), "{text}");
        assert!(text.contains("Mark read or unread"));
        assert!(text.contains("\nWriting a message\n"));
        assert!(
            text.contains("In Quill, ? lists the keys for where you are and K lists them all.")
        );
        // A terminal has no web pane to read in.
        assert!(!text.contains("Web reading pane"));

        // The preset the config names, and a key of the user's own.
        write(&dir, "config.toml", "[ui]\nkeymap = \"mutt\"\n");
        write(
            &dir,
            "keys.toml",
            "\"help.keys\" = \"f2\"\n\"message.toggle_unread\" = \"ctrl-n\"\n",
        );
        let (text, problems) = keys_listing(&paths);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(text.starts_with("Keyboard shortcuts (mutt keys)"), "{text}");
        assert!(line_of(&text, "Mark read or unread").starts_with("ctrl-n "));
        assert!(line_of(&text, "Keyboard shortcuts").starts_with("f2 "));
        assert!(text.contains("and f2 lists them all."));

        // A keys.toml that is wrong is said, and the preset is listed whole.
        write(&dir, "keys.toml", "\"no.such.command\" = \"x\"\n");
        let (text, problems) = keys_listing(&paths);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].ends_with("Using the preset keys."),
            "{problems:?}"
        );
        assert!(line_of(&text, "Keyboard shortcuts").starts_with("K, f1 "));
    }

    /// The line of a printed list that ends with `title`, trimmed.
    fn line_of(text: &str, title: &str) -> String {
        text.lines()
            .skip(1)
            .find(|line| line.ends_with(title))
            .unwrap_or_else(|| panic!("{title} is not listed:\n{text}"))
            .trim()
            .to_owned()
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

    #[test]
    fn the_config_says_what_messages_are_written_in() {
        let dir = TempDir::new().unwrap();
        // With nothing said: Quill's own editor, and Vim for a message
        // that is handed over all the same.
        let loaded = load_from(&dir);
        assert!(!loaded.env.compose.vim);
        assert_eq!(loaded.env.external, ExternalEditor::Vim);
        for (word, editor) in [
            ("vim", ExternalEditor::Vim),
            ("nano", ExternalEditor::Nano),
            ("pico", ExternalEditor::Pico),
            ("mg", ExternalEditor::Mg),
            ("script", ExternalEditor::Script),
        ] {
            write(
                &dir,
                "config.toml",
                &format!("[compose]\neditor = \"{word}\"\n"),
            );
            let loaded = load_from(&dir);
            assert!(loaded.notices.is_empty(), "{word}: {:?}", loaded.notices);
            // Every message goes there as it opens.
            assert!(loaded.env.compose.vim, "{word}");
            assert_eq!(loaded.env.external, editor, "{word}");
        }
        write(&dir, "config.toml", "[compose]\neditor = \"builtin\"\n");
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert!(!loaded.env.compose.vim);
        assert_eq!(loaded.env.external, ExternalEditor::Vim);
        // It is one of those words, never a program: a path is a mistake
        // in the config, said as one, and not an editor to run. So are an
        // editor macOS does not come with, and the settings this one
        // replaced.
        for mistaken in [
            "editor = \"/opt/homebrew/bin/nvim\"",
            "editor = \"emacs\"",
            "editor = \"bundled\"",
            "vim = \"system\"",
            "vim = \"script\"",
        ] {
            write(&dir, "config.toml", &format!("[compose]\n{mistaken}\n"));
            let loaded = load_from(&dir);
            assert!(!loaded.notices.is_empty(), "{mistaken}");
            assert!(!loaded.env.compose.vim, "{mistaken}");
            assert_eq!(loaded.env.external, ExternalEditor::Vim, "{mistaken}");
        }
    }

    #[test]
    fn hooks_beside_the_config_are_handed_to_the_ui() {
        let dir = TempDir::new().unwrap();
        // With no file there are none, and that is not worth a notice.
        let loaded = load_from(&dir);
        assert!(loaded.env.hooks.is_empty() && loaded.notices.is_empty());

        write(
            &dir,
            "hooks.toml",
            "[[hook]]\nname = \"Save to notes\"\ncommand = [\"/usr/local/bin/save-note\", \"--from-mail\"]\n\n\
             [[hook]]\nname = \"Word count\"\ncommand = [\"/usr/bin/wc\", \"-w\"]\ninput = \"text\"\n",
        );
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        let hooks = &loaded.env.hooks;
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].name, "Save to notes");
        assert_eq!(
            hooks[0].command,
            ["/usr/local/bin/save-note", "--from-mail"]
        );
        assert_eq!(hooks[0].input, quill_tui::hook::HookInput::Raw);
        assert_eq!(hooks[1].input, quill_tui::hook::HookInput::Text);
        // Whether they may run is not this file's to say: that is
        // decided once it is known what hosts the UI.
        assert!(!loaded.env.hooks_enabled);
    }

    #[test]
    fn a_hooks_file_that_cannot_be_used_whole_is_not_used_at_all() {
        let good = "[[hook]]\nname = \"Good\"\ncommand = [\"/bin/cat\"]\n";
        let cases: [(String, &str); 5] = [
            // A program found by searching, not by where it is.
            (
                format!("{good}[[hook]]\nname = \"Notes\"\ncommand = [\"save-note\"]\n"),
                "hooks.toml: \"Notes\": the program must be an absolute path",
            ),
            // A line for a shell.
            (
                "[[hook]]\nname = \"Notes\"\ncommand = \"/bin/sh -c save-note\"\n".to_owned(),
                "hooks.toml: line 3: ",
            ),
            // Not TOML at all.
            (format!("{good}[[hook\n"), "hooks.toml: line 4: "),
            // More than anyone means to have.
            (
                (0..51)
                    .map(|number| {
                        format!("[[hook]]\nname = \"Hook {number}\"\ncommand = [\"/bin/cat\"]\n")
                    })
                    .collect(),
                "hooks.toml: there are 51 hooks, and the most is 50",
            ),
            // More than is read.
            (
                format!("{good}# {}\n", "x".repeat(MAX_HOOKS_BYTES as usize)),
                "hooks.toml: is larger than 64 KiB",
            ),
        ];
        for (text, complaint) in cases {
            let dir = TempDir::new().unwrap();
            write(&dir, "hooks.toml", &text);
            let loaded = load_from(&dir);
            // Not even the good hook that came first.
            assert!(loaded.env.hooks.is_empty(), "{complaint}");
            assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
            let notice = &loaded.notices[0];
            assert_eq!(notice.kind, NoticeKind::Error);
            assert!(notice.text.starts_with(complaint), "{}", notice.text);
            assert!(
                notice.text.ends_with(". No hooks are loaded."),
                "{}",
                notice.text
            );
            assert!(!notice.text.contains('\n'));
        }

        // Something that is not a file, or not text.
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("config/hooks.toml")).unwrap();
        let loaded = load_from(&dir);
        assert!(loaded.env.hooks.is_empty());
        assert!(
            loaded.notices[0]
                .text
                .starts_with("hooks.toml: is not a file")
        );
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/hooks.toml"),
            b"[[hook]]\nname = \"\xff\"\n",
        )
        .unwrap();
        let loaded = load_from(&dir);
        assert!(loaded.env.hooks.is_empty());
        assert!(
            loaded.notices[0]
                .text
                .starts_with("hooks.toml: is not UTF-8 text")
        );
    }

    #[test]
    fn templates_beside_the_config_are_handed_to_the_ui() {
        let dir = TempDir::new().unwrap();
        // With no folder there are none, and that is not worth a notice.
        assert!(load_from(&dir).env.templates.is_empty());

        write(
            &dir,
            "templates/Out of office.md",
            "Subject: Away until {{back}}\n\nHi {{first_name}},\n\nI am away.\n",
        );
        write(&dir, "templates/thanks.txt", "Thanks,\n{{from_name}}\n");
        // A template that cannot be used is said so, and the rest load.
        write(
            &dir,
            "templates/huge.md",
            &"x".repeat(templates::MAX_FILE_BYTES as usize + 1),
        );
        let loaded = load_from(&dir);
        let names: Vec<&str> = loaded
            .env
            .templates
            .iter()
            .map(|template| template.name.as_str())
            .collect();
        assert_eq!(names, ["Out of office", "thanks"]);
        assert_eq!(
            loaded.env.templates[0].subject.as_deref(),
            Some("Away until {{back}}")
        );
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(
            loaded.notices[0]
                .text
                .contains("huge.md (is larger than 64 KiB)")
        );
    }
}
