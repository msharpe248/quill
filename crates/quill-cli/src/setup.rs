//! Reads the config, theme and key files once, before the UI starts.
//!
//! Nothing here is fatal. A file that cannot be used is reported as a notice
//! for the status bar and the defaults take its place, so a bad edit never
//! locks anyone out of their mail.

use std::io::ErrorKind;
use std::path::Path;

use jiff::tz::TimeZone;
use quill_core::config::Config;
use quill_core::config::{ComposeEditor, ComposeFormat, ReplyPosition};
use quill_tui::command::Registry;
use quill_tui::keymap::Keymap;
use quill_tui::state::{ComposeSettings, Env, Identity, Notice};
use quill_tui::theme::{ColorMode, Styles, Theme};

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

    let templates = templates::load(&paths.templates_dir(), &mut notices);
    let hooks = load_hooks(&paths.config_dir.join(HOOKS_FILE), &mut notices);

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
            vim: config.compose.editor == ComposeEditor::Vim,
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
        // And only it has a Vim of its own; anywhere else the user's is run.
        editor_pane: false,
        vim_program: None,
        hooks,
        // And whether programs may be run at all: not in Quill's own app.
        hooks_enabled: false,
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

    #[test]
    fn the_config_says_whether_messages_are_written_in_vim() {
        let dir = TempDir::new().unwrap();
        assert!(!load_from(&dir).env.compose.vim);
        write(&dir, "config.toml", "[compose]\neditor = \"vim\"\n");
        let loaded = load_from(&dir);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert!(loaded.env.compose.vim);
        write(&dir, "config.toml", "[compose]\neditor = \"builtin\"\n");
        assert!(!load_from(&dir).env.compose.vim);
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
