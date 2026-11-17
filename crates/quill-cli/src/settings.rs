//! The settings window's side of the config: reading `config.toml` for it,
//! and making the changes made in it.
//!
//! The UI describes the window and decides what a thing done there means
//! (`quill_tui::settings`); it reads and writes no file. This does: it is
//! asked through the accounts service, makes the one change it is given
//! through [`config_edit`](crate::config_edit), and answers with what the
//! file then says, read afresh.
//!
//! The file is read from disk for every request and written back whole, so
//! a change is always made to the file as it stands. What was edited by
//! hand a moment ago is kept, and a file that has a mistake in it is not
//! written to at all.

use std::path::{Path, PathBuf};

use quill_core::config::Config;
use quill_tui::command::Registry;
use quill_tui::settings::{
    AccountLine, Change, KeysChange, Settings, SettingsRead, ThemeLine, ThemesChange,
};
use quill_tui::setup::AccountEvent;

use crate::{config_edit, oauth, setup};

/// [`answer`], off the async threads: the config file blocks.
pub async fn answer_off_thread(config_file: PathBuf, change: Option<Change>) -> AccountEvent {
    tokio::task::spawn_blocking(move || answer(&config_file, change))
        .await
        .unwrap_or_else(|err| AccountEvent::Settings {
            read: Err(err.to_string()),
            changed: None,
        })
}

/// Makes `change` to the config, if there is one to make, and reads the
/// settings from the file as it then is.
pub fn answer(config_file: &Path, change: Option<Change>) -> AccountEvent {
    let changed = change.map(|change| {
        let made = make(config_file, &change);
        (change, made)
    });
    AccountEvent::Settings {
        read: read(config_file).map(Box::new),
        changed,
    }
}

/// [`change_keys`], off the async threads: the files block.
pub async fn keys_off_thread(config_file: PathBuf, change: KeysChange) -> AccountEvent {
    tokio::task::spawn_blocking(move || change_keys(&config_file, &change))
        .await
        .unwrap_or_else(|err| AccountEvent::KeysChanged {
            read: Err(err.to_string()),
            result: Err(err.to_string()),
        })
}

/// Makes `change` to the user's own keys, in `keys.toml` beside the
/// config, and reads the settings as they then are: the keys with them.
pub fn change_keys(config_file: &Path, change: &KeysChange) -> AccountEvent {
    let result = write_keys(config_file, change);
    AccountEvent::KeysChanged {
        read: read(config_file).map(Box::new),
        result,
    }
}

/// Writes `change` into `keys.toml` as it stands now. A file that is not
/// there yet is made by the first change.
fn write_keys(config_file: &Path, change: &KeysChange) -> Result<(), String> {
    let config = Config::parse(&text_of(config_file)?).map_err(|err| err.to_string())?;
    let path = crate::paths::keys_file_in(config_dir(config_file));
    let before = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("keys.toml cannot be read: {err}")),
    };
    let after = crate::keys_edit::apply(
        &before,
        change,
        &quill_tui::keymap::Keymap::preset(config.ui.keymap),
        &Registry::builtin(),
    )?;
    if after == before {
        return Ok(());
    }
    config_edit::write(&path, &after).map_err(|err| format!("keys.toml cannot be written: {err}"))
}

/// [`change_themes`], off the async threads: the files block.
pub async fn themes_off_thread(config_file: PathBuf, change: ThemesChange) -> AccountEvent {
    tokio::task::spawn_blocking(move || change_themes(&config_file, &change))
        .await
        .unwrap_or_else(|err| AccountEvent::ThemesChanged {
            read: Err(err.to_string()),
            result: Err(err.to_string()),
        })
}

/// Saves or deletes a theme in the `themes` folder beside the config, and
/// reads the settings as they then are: the themes with them.
pub fn change_themes(config_file: &Path, change: &ThemesChange) -> AccountEvent {
    let dir = config_dir(config_file);
    let result = match change {
        ThemesChange::Save {
            from,
            theme,
            select,
        } => crate::theme_edit::save(dir, from, theme).and_then(|()| match select {
            // A new theme is the one in use from now, as one imported is.
            Some(dark) => make(
                config_file,
                &Change::ThemeInForce {
                    name: theme.name.clone(),
                    dark: *dark,
                },
            ),
            None => Ok(()),
        }),
        ThemesChange::Delete { name } => crate::theme_edit::delete(config_file, dir, name),
    };
    AccountEvent::ThemesChanged {
        read: read(config_file).map(Box::new),
        result,
    }
}

/// The folder the config is in, where the themes and the keys are too.
fn config_dir(config_file: &Path) -> &Path {
    config_file.parent().unwrap_or(Path::new("."))
}

fn text_of(config_file: &Path) -> Result<String, String> {
    match std::fs::read_to_string(config_file) {
        Ok(text) => Ok(text),
        // No config yet is an empty one.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("config.toml cannot be read: {err}")),
    }
}

/// Writes one setting into the file as it stands now.
fn make(config_file: &Path, change: &Change) -> Result<(), String> {
    let before = text_of(config_file)?;
    // A theme is one there is, and one that can be drawn with: the file
    // is not made to name another. This is also where a theme the app has
    // just copied into the folder is checked: what its file is worth is
    // decided here, and not by whoever copied it.
    if let Change::ThemeLight(name) | Change::ThemeDark(name) | Change::ThemeInForce { name, .. } =
        change
    {
        if !crate::paths::is_theme_name(name) {
            return Err(format!("There is no theme called \"{name}\"."));
        }
        setup::load_theme(config_dir(config_file), name).map_err(|none| none.why)?;
    }
    let after = config_edit::set(&before, change)?;
    if after == before {
        return Ok(());
    }
    config_edit::write(config_file, &after)
        .map_err(|err| format!("config.toml cannot be written: {err}"))
}

/// What the file says of the settings the window has, with what the UI
/// needs to make the ones that hold at once hold: the theme and the keys,
/// loaded as they are when Quill starts.
fn read(config_file: &Path) -> Result<SettingsRead, String> {
    let config = Config::parse(&text_of(config_file)?).map_err(|err| err.to_string())?;
    let dir = config_dir(config_file);
    let (look, [theme_dark_problem, theme_light_problem]) = setup::look_of(dir, &config.ui);
    let (keymap, keys_problem) = setup::keymap_for(dir, config.ui.keymap, &Registry::builtin());
    Ok(SettingsRead {
        settings: Settings::of(&config),
        accounts: config
            .accounts
            .iter()
            .map(|account| AccountLine {
                name: account.name.clone(),
                kind: account.kind,
                address: account.address.clone(),
                signs_in: oauth::wanted(account),
            })
            .collect(),
        themes: setup::themes_in(dir)
            .into_iter()
            .map(|name| ThemeLine {
                own: crate::paths::theme_files_in(dir, &name)
                    .iter()
                    .any(|(file, _)| file.is_file()),
                theme: setup::load_theme(dir, &name).map_err(|none| none.why),
                name,
            })
            .collect(),
        theme_light: look.light,
        theme_light_problem,
        theme_dark: look.dark,
        theme_dark_problem,
        keymap,
        keys_problem,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jiff::tz::TimeZone;
    use quill_core::config::{Appearance, KeymapPreset, RemoteImages};
    use quill_host::bridge::{FormField, FormValue, Number, SettingsShow, SettingsUsed};
    use quill_tui::app::AccountsLink;
    use quill_tui::command::CommandId;
    use quill_tui::setup::AccountRequest;
    use quill_tui::state::{Action, Env, SettingsAct, State};
    use quill_tui::theme::ColorMode;
    use quill_tui::vim::ExternalEditor;
    use tempfile::TempDir;

    use super::*;
    use crate::paths::Paths;

    #[test]
    fn a_key_changed_in_the_window_is_in_keys_toml_and_in_what_is_read_back() {
        let dir = TempDir::new().unwrap();
        let config_file = dir.path().join("config.toml");
        fs::write(&config_file, "[ui]\nkeymap = \"vim\"\n").unwrap();
        let keys_file = dir.path().join("keys.toml");
        let keys_of = |event: &AccountEvent, id: CommandId| -> Vec<String> {
            match event {
                AccountEvent::KeysChanged { read: Ok(read), .. } => read
                    .keymap
                    .keys_for(id)
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                other => panic!("{other:?}"),
            }
        };
        let set = |name: &str, keys: &[&str]| {
            KeysChange::Set(vec![(
                name.to_owned(),
                keys.iter().map(|key| (*key).to_owned()).collect(),
            )])
        };

        // There is no file yet: the first change makes it.
        let answer = change_keys(&config_file, &set("message.move", &["ctrl-b"]));
        assert!(matches!(
            &answer,
            AccountEvent::KeysChanged { result: Ok(()), .. }
        ));
        assert_eq!(keys_of(&answer, CommandId::MessageMove), ["ctrl-b"]);
        assert_eq!(
            fs::read_to_string(&keys_file).unwrap(),
            "\"message.move\" = \"ctrl-b\"\n"
        );

        // A change that cannot be made says why, and the file and the
        // keys are as they were.
        let answer = change_keys(&config_file, &set("no.such.command", &["x"]));
        match &answer {
            AccountEvent::KeysChanged {
                result: Err(why), ..
            } => assert!(why.contains("no command called"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(keys_of(&answer, CommandId::MessageMove), ["ctrl-b"]);

        // The user's own edit by hand, with a mistake in it, is not
        // written over: it is theirs to put right.
        let mistaken = "\"message.move\" = \"ctrl-b\"\n\"no.such.command\" = \"x\"\n";
        fs::write(&keys_file, mistaken).unwrap();
        let answer = change_keys(&config_file, &KeysChange::ResetAll);
        match &answer {
            AccountEvent::KeysChanged {
                read: Ok(read),
                result: Err(why),
            } => {
                assert!(why.contains("a mistake to put right first"), "{why}");
                assert!(read.keys_problem.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read_to_string(&keys_file).unwrap(), mistaken);

        // Put right, everything can be put back.
        fs::write(&keys_file, "\"message.move\" = \"ctrl-b\"\n").unwrap();
        let answer = change_keys(&config_file, &KeysChange::ResetAll);
        assert!(matches!(
            &answer,
            AccountEvent::KeysChanged { result: Ok(()), .. }
        ));
        assert_eq!(keys_of(&answer, CommandId::MessageMove), ["m"]);
        assert_eq!(fs::read_to_string(&keys_file).unwrap(), "");
    }

    /// A config a person has written, with their comments.
    const WRITTEN: &str = "# My settings\n\
        [ui]\n\
        theme = \"light\"   # easier on the eyes\n\
        \n\
        [[account]]\n\
        name = \"Work\"   # the office\n\
        type = \"graph\"\n\
        address = \"robin@example.com\"\n\
        \n\
        [[account]]\n\
        name = \"Home\"\n\
        type = \"imap\"\n\
        address = \"robin@example.org\"\n\
        imap = { host = \"mail.example.org\" }\n";

    fn file(dir: &TempDir) -> PathBuf {
        dir.path().join("config").join("config.toml")
    }

    fn write(dir: &TempDir, name: &str, text: &str) {
        let path = dir.path().join("config").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn text(dir: &TempDir) -> String {
        fs::read_to_string(file(dir)).unwrap()
    }

    fn read_of(event: AccountEvent) -> SettingsRead {
        match event {
            AccountEvent::Settings { read: Ok(read), .. } => *read,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn with_no_file_the_settings_are_the_defaults_and_nothing_is_made() {
        let dir = TempDir::new().unwrap();
        let read = read_of(answer(&file(&dir), None));
        assert_eq!(read.settings, Settings::of(&Config::default()));
        assert_eq!(read.theme_names(), ["dark", "light", "terminal"]);
        assert!(read.accounts.is_empty());
        assert_eq!(
            (
                read.theme_light_problem,
                read.theme_dark_problem,
                read.keys_problem
            ),
            (None, None, None)
        );
        // Reading makes nothing on disk.
        assert!(!dir.path().join("config").exists());
    }

    #[test]
    fn the_settings_are_read_from_the_file_with_its_themes_keys_and_accounts() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", WRITTEN);
        write(
            &dir,
            "themes/mine.toml",
            "name = \"mine\"\n[colors]\ntext = \"#112233\"\n",
        );
        write(&dir, "themes/Solarized-2.toml", "name = \"solarized\"\n");
        // One made for Ghostty, under the name its file has.
        write(
            &dir,
            "themes/Night Owl+",
            "background = #1e2030\nforeground = #d0d4ea\npalette = 4=#7aa0f0\n",
        );
        // Not themes: a file that is neither kind, names no theme can
        // have, and one that is a built-in's own, which is that theme.
        write(&dir, "themes/notes.txt", "Remember the milk.\n");
        write(&dir, "themes/.hidden.toml", "");
        write(
            &dir,
            "themes/.DS_Store",
            "background = #000000\nforeground = #ffffff\n",
        );
        write(&dir, "themes/dark.toml", "name = \"my dark\"\n");
        write(&dir, "keys.toml", "\"message.toggle_unread\" = \"m\"\n");

        let read = read_of(answer(&file(&dir), None));
        // The one theme the file names is the theme for both appearances.
        assert_eq!(
            (
                read.settings.theme_light.as_str(),
                read.settings.theme_dark.as_str()
            ),
            ("light", "light")
        );
        assert_eq!(read.settings.appearance, Appearance::System);
        assert_eq!(
            (
                read.theme_light.name.as_str(),
                read.theme_dark.name.as_str()
            ),
            ("light", "light")
        );
        assert_eq!(read.settings.keymap, KeymapPreset::Vim);
        assert_eq!(read.settings.undo_send_seconds, 10);
        assert_eq!(
            read.theme_names(),
            [
                "dark",
                "light",
                "terminal",
                "Night Owl+",
                "Solarized-2",
                "mine"
            ]
        );
        let accounts: Vec<(&str, &str, bool)> = read
            .accounts
            .iter()
            .map(|account| {
                (
                    account.name.as_str(),
                    account.address.as_str(),
                    account.signs_in,
                )
            })
            .collect();
        assert_eq!(
            accounts,
            [
                ("Work", "robin@example.com", true),
                ("Home", "robin@example.org", false)
            ]
        );
        // The keys are the preset's with the user's own on top, as when
        // Quill starts.
        assert_eq!(
            read.keymap.keys_for(CommandId::MessageToggleUnread)[0].to_string(),
            "m"
        );
        assert_eq!(read.keys_problem, None);

        // A theme of the user's own is loaded from its file; one that
        // cannot be is stood in for, and that is said.
        let read = read_of(answer(
            &file(&dir),
            Some(Change::ThemeDark("mine".to_owned())),
        ));
        assert_eq!(read.settings.theme_dark, "mine");
        assert_eq!(read.theme_dark.name, "mine");
        assert_eq!(read.theme_dark_problem, None);
        // The other appearance still has the theme the file named.
        assert_eq!(read.settings.theme_light, "light");
        assert_eq!(read.theme_light.name, "light");
        // One made for Ghostty is read from its file too, and is the
        // colours worked out from it.
        let read = read_of(answer(
            &file(&dir),
            Some(Change::ThemeLight("Night Owl+".to_owned())),
        ));
        assert_eq!(read.theme_light.name, "Night Owl+");
        assert_eq!(read.theme_light_problem, None);
        assert_eq!(read.theme_light.is_dark(), Some(true));
        assert!(text(&dir).contains("theme_light = \"Night Owl+\"\n"));
        write(
            &dir,
            "themes/mine.toml",
            "name = \"mine\"\n[colors]\ntext = \"mauve\"\n",
        );
        let read = read_of(answer(&file(&dir), None));
        assert_eq!(read.theme_dark.name, "dark");
        assert!(
            read.theme_dark_problem
                .as_deref()
                .is_some_and(|problem| problem.starts_with("themes/mine.toml: ")),
            "{:?}",
            read.theme_dark_problem
        );
        assert_eq!(read.theme_light_problem, None);
        // A Ghostty theme that stops being one is no longer listed, and
        // what is wrong with it is said for as long as the file names it.
        write(&dir, "themes/Night Owl+", "background = #1e2030\n");
        let read = read_of(answer(&file(&dir), None));
        assert!(!read.theme_names().contains(&"Night Owl+"));
        assert_eq!(
            read.theme_light_problem.as_deref(),
            Some(
                "themes/Night Owl+ is not a theme in Ghostty\u{2019}s format: it names no \
                 background and foreground colour. Using a built-in theme."
            )
        );
        write(&dir, "keys.toml", "\"no.such.command\" = \"x\"\n");
        let read = read_of(answer(&file(&dir), None));
        assert!(
            read.keys_problem
                .as_deref()
                .is_some_and(|problem| problem.contains("no.such.command")),
            "{:?}",
            read.keys_problem
        );
        assert_eq!(
            read.keymap.keys_for(CommandId::MessageToggleUnread)[0].to_string(),
            "u"
        );
    }

    #[test]
    fn a_change_is_written_and_the_answer_is_the_file_as_it_then_is() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", WRITTEN);
        let change = Change::UndoSendSeconds(30);
        let event = answer(&file(&dir), Some(change.clone()));
        let AccountEvent::Settings {
            read: Ok(read),
            changed,
        } = event
        else {
            panic!("the file was not read");
        };
        assert_eq!(changed, Some((change, Ok(()))));
        assert_eq!(read.settings.undo_send_seconds, 30);
        assert_eq!(
            text(&dir),
            format!("{WRITTEN}\n[compose]\nundo_send_seconds = 30\n")
        );
        // Nothing but the config is left in its folder.
        let left: Vec<_> = fs::read_dir(dir.path().join("config"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["config.toml"]);

        // With no file yet, the change makes one that says only that.
        let fresh = TempDir::new().unwrap();
        let read = read_of(answer(
            &file(&fresh),
            Some(Change::RemoteImages(RemoteImages::Never)),
        ));
        assert_eq!(read.settings.remote_images, RemoteImages::Never);
        assert_eq!(text(&fresh), "[privacy]\nremote_images = \"never\"\n");
    }

    #[test]
    fn what_was_edited_by_hand_between_two_changes_is_kept() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", WRITTEN);
        answer(&file(&dir), Some(Change::Notifications(false)));
        // By hand, in an editor, while the window is open: another theme,
        // a comment, and a rule.
        let by_hand = text(&dir).replace(
            "theme = \"light\"   # easier on the eyes\n",
            "theme = \"terminal\"   # trying this one\nreader_width = 0.4\n",
        ) + "\n[[rule]]\nmatch = \"from:buildbot\"\nmark_read = true\n";
        write(&dir, "config.toml", &by_hand);
        // The next change is made to that, not to what the window last
        // read.
        let read = read_of(answer(&file(&dir), Some(Change::Threads(false))));
        assert_eq!(read.settings.theme_light, "terminal");
        assert_eq!(read.settings.theme_dark, "terminal");
        assert!(!read.settings.notifications && !read.settings.threads);
        assert_eq!(
            text(&dir),
            by_hand.replace(
                "notifications = false\n",
                "notifications = false\nthreads = false\n"
            )
        );
        let config = Config::parse(&text(&dir)).unwrap();
        assert_eq!(config.ui.reader_width, 0.4);
        assert_eq!(config.rules.len(), 1);
        assert!(text(&dir).contains("# trying this one"));
    }

    #[test]
    fn a_file_with_a_mistake_in_it_is_said_to_have_one_and_is_never_written() {
        let dir = TempDir::new().unwrap();
        for mistaken in [
            "[ui]\ntheme = \"light\"\nkeymap = \n",
            "[ui]\nthemee = \"dark\"   # a slip\n",
            "# Mine\n[storage]\ncache_gb = -1\n",
        ] {
            write(&dir, "config.toml", mistaken);
            for change in [
                None,
                Some(Change::Notifications(false)),
                Some(Change::CacheGb(Number::from(5))),
            ] {
                let event = answer(&file(&dir), change.clone());
                let AccountEvent::Settings {
                    read: Err(problem),
                    changed,
                } = event
                else {
                    panic!("{mistaken:?} was read");
                };
                assert!(problem.starts_with("config"), "{problem}");
                assert!(!problem.contains('\n'));
                match (change, changed) {
                    (None, None) => {}
                    (Some(asked), Some((made, Err(reason)))) => {
                        assert_eq!(asked, made);
                        assert!(reason.contains("has a mistake to fix first"), "{reason}");
                    }
                    other => panic!("{other:?}"),
                }
                // Byte for byte what it was.
                assert_eq!(text(&dir), mistaken);
            }
        }
    }

    #[test]
    fn a_theme_there_is_none_of_is_not_written_into_the_file() {
        let dir = TempDir::new().unwrap();
        write(&dir, "config.toml", WRITTEN);
        // Nor is a file in the folder that is not a theme.
        write(&dir, "themes/notes", "Remember the milk.\n");
        for name in ["nope", "../../etc/passwd", "", "notes"] {
            let change = if name.is_empty() {
                Change::ThemeLight(name.to_owned())
            } else {
                Change::ThemeDark(name.to_owned())
            };
            let event = answer(&file(&dir), Some(change));
            let AccountEvent::Settings {
                read: Ok(read),
                changed: Some((_, Err(reason))),
            } = event
            else {
                panic!("{name:?} was written");
            };
            assert!(
                reason.contains(if name == "notes" {
                    "themes/notes is not a theme in Ghostty\u{2019}s format"
                } else {
                    "There is no theme called"
                }),
                "{reason}"
            );
            assert_eq!(read.settings.theme_dark, "light");
            assert_eq!(read.settings.theme_light, "light");
            assert_eq!(text(&dir), WRITTEN);
        }
    }

    #[tokio::test]
    async fn the_demo_mailbox_answers_for_the_settings_window_too() {
        let dir = TempDir::new().unwrap();
        let AccountsLink {
            requests,
            mut events,
        } = crate::service::spawn_remembering(file(&dir));
        let requests = requests.unwrap();
        let mut next = async || {
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .expect("timed out waiting for the service")
                .expect("the service stopped")
        };
        requests.send(AccountRequest::Settings(None)).unwrap();
        let read = read_of(next().await);
        assert_eq!(read.settings.keymap, KeymapPreset::Vim);
        assert!(!file(&dir).exists());
        requests
            .send(AccountRequest::Settings(Some(Change::Keymap(
                KeymapPreset::Mutt,
            ))))
            .unwrap();
        let read = read_of(next().await);
        assert_eq!(read.settings.keymap, KeymapPreset::Mutt);
        assert_eq!(text(&dir), "[ui]\nkeymap = \"mutt\"\n");
    }

    /// Quill's app with its settings window, over a config in a folder of
    /// the test's own: the state, the environment it goes by, and this
    /// service answering what the state asks of it, as the run loop has
    /// them answer one another.
    struct App {
        state: State,
        env: Env,
        dir: TempDir,
        /// How many times the keys changed with the settings: the times
        /// the app would have been told every command's keys again.
        keys_announced: usize,
    }

    impl App {
        fn over(config: Option<&str>) -> Self {
            let dir = TempDir::new().unwrap();
            if let Some(config) = config {
                write(&dir, "config.toml", config);
            }
            Self::in_dir(dir)
        }

        /// Over a folder that already has what the test put in it.
        fn in_dir(dir: TempDir) -> Self {
            let paths = Paths {
                config_dir: dir.path().join("config"),
                data_dir: dir.path().join("data"),
            };
            let mut env = setup::load(&paths, ColorMode::TrueColor, TimeZone::UTC).env;
            env.accounts = true;
            crate::suit_host(&mut env, true, None);
            let mut app = Self {
                state: State::new((120, 40), 1_794_648_600),
                env,
                dir,
                keys_announced: 0,
            };
            app.act(Action::Resize(120, 40));
            app
        }

        fn act(&mut self, action: Action) {
            let mut waiting = std::collections::VecDeque::from([action]);
            while let Some(action) = waiting.pop_front() {
                let mut effects = self.state.update(&self.env, action);
                if let Some(settings) = effects.settings.take()
                    && settings.apply(&mut self.env)
                {
                    self.keys_announced += 1;
                }
                if let Some(dark) = effects.system_dark.take() {
                    self.env.look.system_dark = Some(dark);
                    self.env.restyle();
                }
                if let Some(trying) = effects.trying.take() {
                    self.env.look.trying = trying;
                    self.env.restyle();
                }
                for request in effects.accounts {
                    match request {
                        AccountRequest::Settings(change) => {
                            waiting.push_back(Action::Account(answer(&file(&self.dir), change)));
                        }
                        AccountRequest::Themes(change) => {
                            waiting.push_back(Action::Account(change_themes(
                                &file(&self.dir),
                                &change,
                            )));
                        }
                        AccountRequest::Remember(_) => {}
                        other => panic!("{other:?}"),
                    }
                }
            }
        }

        fn open(&mut self) {
            self.act(Action::Settings(SettingsAct::Opened));
        }

        fn set(&mut self, pane: &str, field: &str, value: FormValue) {
            let used = SettingsUsed::set(pane, field, value).unwrap();
            self.act(Action::Settings(SettingsAct::Used(used)));
        }

        fn choose(&mut self, pane: &str, field: &str, option: &str) {
            self.set(pane, field, FormValue::Text(option.to_owned()));
        }

        fn shown(&self) -> SettingsShow {
            self.state.settings_shown(&self.env).unwrap().1
        }

        fn answers(&self) -> u64 {
            self.state.settings_shown(&self.env).unwrap().0
        }

        fn field(&self, pane: &str, field: &str) -> FormField {
            self.shown().field(pane, field).unwrap().clone()
        }

        fn chosen(&self, pane: &str, field: &str) -> String {
            match self.field(pane, field) {
                FormField::Choice { value, .. } => value,
                other => panic!("{other:?}"),
            }
        }

        fn text(&self) -> String {
            text(&self.dir)
        }
    }

    #[test]
    fn a_setting_changed_in_the_window_is_in_the_file_and_holds_at_once() {
        let mut app = App::over(Some(WRITTEN));
        assert_eq!(app.env.styles.theme_name(), "light");
        // Until it is opened there is nothing to show.
        assert!(app.state.settings_shown(&app.env).is_none());
        app.open();
        assert_eq!(app.shown().panes().len(), 7);
        assert_eq!(app.shown().problem(), None);
        // The one theme the file names is shown for both appearances.
        assert_eq!(app.chosen("appearance", "theme_light"), "light");
        assert_eq!(app.chosen("appearance", "theme_dark"), "light");
        assert_eq!(app.chosen("appearance", "appearance"), "system");
        // Opening wrote nothing.
        assert_eq!(app.text(), WRITTEN);

        // The theme for the appearance Quill has: drawn with from now on.
        // It is written under a key of its own, and the line the user
        // wrote is left as they wrote it.
        app.choose("appearance", "theme_dark", "dark");
        assert_eq!(app.chosen("appearance", "theme_dark"), "dark");
        assert_eq!(app.chosen("appearance", "theme_light"), "light");
        assert_eq!(app.env.styles.theme_name(), "dark");
        assert_eq!(
            app.text(),
            WRITTEN.replace(
                "# easier on the eyes\n",
                "# easier on the eyes\ntheme_dark = \"dark\"\n"
            )
        );

        // The keys: the other preset's, and the app is told them.
        let unread =
            |app: &App| app.env.keymap.keys_for(CommandId::MessageToggleUnread)[0].to_string();
        assert_eq!((unread(&app), app.keys_announced), ("u".to_owned(), 0));
        app.choose("general", "keymap", "mutt");
        assert_eq!((unread(&app), app.keys_announced), ("N".to_owned(), 1));
        // Nothing else that is changed tells the app the keys again.
        app.set("general", "notifications", FormValue::Flag(false));
        assert!(!app.env.notifications);
        app.set("general", "threads", FormValue::Flag(false));
        assert!(!app.env.threads);
        app.set("general", "web_reading", FormValue::Flag(true));
        assert!(app.env.web_reading && app.state.web_reading);
        app.choose("composing", "format", "plain");
        assert!(!app.env.compose.markdown);
        app.choose("composing", "reply_position", "below");
        assert!(!app.env.compose.reply_above);
        app.set("composing", "spell_check", FormValue::Flag(false));
        assert!(!app.env.compose.spell_check);
        app.set(
            "composing",
            "undo_send_seconds",
            FormValue::Number(Number::from(0)),
        );
        assert_eq!(app.env.compose.undo_send_seconds, 0);
        app.choose("privacy", "remote_images", "never");
        assert!(app.env.remote_never);
        assert_eq!(app.keys_announced, 1);

        // All of it is in the file, with what the user wrote around it.
        let config = Config::parse(&app.text()).unwrap();
        assert_eq!(config.ui.theme_for(true), "dark");
        assert_eq!(config.ui.theme, "light");
        assert_eq!(config.ui.keymap, KeymapPreset::Mutt);
        assert!(!config.ui.notifications && !config.ui.threads && config.ui.web_reading);
        assert_eq!(config.compose.format.as_str(), "plain");
        assert_eq!(config.compose.reply_position.as_str(), "below");
        assert!(!config.compose.spell_check);
        assert_eq!(config.compose.undo_send_seconds, 0);
        assert_eq!(config.privacy.remote_images, RemoteImages::Never);
        assert_eq!(config.accounts.len(), 2);
        for kept in [
            "# My settings\n",
            "# easier on the eyes\n",
            "# the office\n",
        ] {
            assert!(app.text().contains(kept), "{kept:?}");
        }
        // And the window shows what the file says.
        assert_eq!(app.chosen("general", "keymap"), "mutt");
        assert_eq!(app.chosen("privacy", "remote_images"), "never");
        assert!(matches!(
            app.field("general", "web_reading"),
            FormField::Toggle { value: true, .. }
        ));

        // Switched back, each holds again.
        app.set("general", "web_reading", FormValue::Flag(false));
        assert!(!app.env.web_reading && !app.state.web_reading);
        app.choose("privacy", "remote_images", "ask");
        assert!(!app.env.remote_never);
        app.choose("general", "keymap", "vim");
        assert_eq!((unread(&app), app.keys_announced), ("u".to_owned(), 2));
    }

    #[test]
    fn what_waits_for_the_next_start_is_written_and_said_to_wait() {
        let mut app = App::over(Some(WRITTEN));
        app.open();
        let help = |app: &App, pane: &str, field: &str| {
            app.field(pane, field).help().unwrap_or_default().to_owned()
        };
        // Said beside each of the two, and beside no other.
        for pane in app.shown().panes() {
            for field in &pane.fields {
                let waits = field
                    .help()
                    .is_some_and(|help| help.contains("Takes effect when Quill is next opened."));
                assert_eq!(
                    waits,
                    matches!(field.id(), "editor" | "cache_gb"),
                    "{}",
                    field.id()
                );
            }
        }
        assert!(!help(&app, "composing", "editor").contains("Until then"));
        assert!(!help(&app, "storage", "cache_gb").contains("Until then"));

        app.choose("composing", "editor", "nano");
        assert_eq!(app.chosen("composing", "editor"), "nano");
        assert!(app.text().ends_with("\n[compose]\neditor = \"nano\"\n"));
        // Messages are still written as they were when Quill started.
        assert!(!app.env.compose.vim);
        assert_eq!(app.env.external, ExternalEditor::Vim);
        assert!(
            help(&app, "composing", "editor")
                .ends_with("Until then messages are written in Quill\u{2019}s own editor."),
            "{}",
            help(&app, "composing", "editor")
        );

        app.set(
            "storage",
            "cache_gb",
            FormValue::Number(Number::new(0.5).unwrap()),
        );
        assert!(app.text().ends_with("\n[storage]\ncache_gb = 0.5\n"));
        assert_eq!(app.env.cache_gb, 5.0);
        assert!(
            help(&app, "storage", "cache_gb").ends_with("Until then Quill keeps to 5 GB."),
            "{}",
            help(&app, "storage", "cache_gb")
        );
        // Put back to what is running, there is no "until then" to say.
        app.choose("composing", "editor", "builtin");
        app.set("storage", "cache_gb", FormValue::Number(Number::from(5)));
        assert!(!help(&app, "composing", "editor").contains("Until then"));
        assert!(!help(&app, "storage", "cache_gb").contains("Until then"));
        assert!(app.text().contains("cache_gb = 5\n"));

        // Started with another editor, that is the one that is running.
        let mut app = App::over(Some("[compose]\neditor = \"nano\"\n"));
        app.open();
        app.choose("composing", "editor", "vim");
        assert_eq!(app.env.external, ExternalEditor::Nano);
        assert!(
            help(&app, "composing", "editor").ends_with("Until then messages are written in nano.")
        );
    }

    #[test]
    fn what_a_setting_cannot_be_is_refused_and_the_file_is_left_alone() {
        let mut app = App::over(Some(WRITTEN));
        app.open();
        let whole = |value: f64| FormValue::Number(Number::new(value).unwrap());
        let refused: Vec<(&str, &str, FormValue)> = vec![
            // Outside its bounds.
            ("composing", "undo_send_seconds", whole(-1.0)),
            ("composing", "undo_send_seconds", whole(4_294_967_296.0)),
            ("storage", "cache_gb", whole(-0.5)),
            // Not one of the choices.
            (
                "general",
                "theme_dark",
                FormValue::Text("solarized".to_owned()),
            ),
            ("appearance", "theme", FormValue::Text("dark".to_owned())),
            (
                "appearance",
                "appearance",
                FormValue::Text("dim".to_owned()),
            ),
            ("general", "keymap", FormValue::Text("emacs".to_owned())),
            ("composing", "editor", FormValue::Text("emacs".to_owned())),
            (
                "privacy",
                "remote_images",
                FormValue::Text("always".to_owned()),
            ),
            // Read from the file, and not offered here.
            (
                "privacy",
                "remote_images",
                FormValue::Text("allowlist".to_owned()),
            ),
            // What the app once called the user's own Vim is read from a
            // file as Vim, and is not a word to choose.
            ("composing", "editor", FormValue::Text("script".to_owned())),
            // Nor is a program anything but a whole path or a name.
            (
                "composing",
                "editor_path",
                FormValue::Text("bin/vim".to_owned()),
            ),
            // The wrong kind of thing altogether.
            (
                "general",
                "notifications",
                FormValue::Text("yes".to_owned()),
            ),
            ("appearance", "theme_light", FormValue::Flag(true)),
            (
                "composing",
                "undo_send_seconds",
                FormValue::Text("ten".to_owned()),
            ),
            // No such setting, or not in that pane.
            ("general", "editor", FormValue::Text("vim".to_owned())),
            ("general", "font", FormValue::Text("Menlo".to_owned())),
            ("accounts", "accounts", FormValue::Flag(true)),
        ];
        for (pane, field, value) in refused {
            let before = app.answers();
            app.set(pane, field, value.clone());
            assert_eq!(app.text(), WRITTEN, "{pane}.{field} = {value:?}");
            // Said in the window, which is told the settings again so
            // that the control goes back to what is so.
            assert!(app.shown().problem().is_some(), "{pane}.{field}");
            assert_eq!(app.answers(), before + 1, "{pane}.{field}");
        }
        // Half a second is a whole number of seconds, rounded as the
        // field shows it.
        app.set("composing", "undo_send_seconds", whole(7.4));
        assert_eq!(app.env.compose.undo_send_seconds, 7);
        // What went wrong before is no longer said.
        assert_eq!(app.shown().problem(), None);
        // The room is kept to the places it is shown with.
        app.set("storage", "cache_gb", whole(1.23456));
        assert!(app.text().ends_with("cache_gb = 1.23\n"), "{}", app.text());
    }

    #[test]
    fn the_editors_are_the_same_in_the_app_as_in_a_terminal_and_the_program_may_be_named() {
        let options = |app: &App| match app.field("composing", "editor") {
            FormField::Choice { options, .. } => options
                .into_iter()
                .map(|option| option.id)
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        let program = |app: &App| match app.shown().field("composing", "editor_path") {
            Some(FormField::Text { value, .. }) => Some(value.clone()),
            Some(other) => panic!("{other:?}"),
            None => None,
        };
        let mut app = App::over(Some(WRITTEN));
        app.open();
        assert_eq!(options(&app), ["builtin", "vim", "nano", "pico", "mg"]);
        // Quill's own editor is no program to name.
        assert_eq!(program(&app), None);

        // What the app called the user's own Vim while it was sandboxed is
        // Vim, and is shown as Vim.
        let mut app = App::over(Some("[compose]\neditor = \"script\"\n"));
        app.open();
        assert_eq!(app.chosen("composing", "editor"), "vim");
        assert_eq!(options(&app), ["builtin", "vim", "nano", "pico", "mg"]);
        app.choose("composing", "editor", "mg");
        assert_eq!(app.text(), "[compose]\neditor = \"mg\"\n");

        // The program an editor is can be named, right under the editor:
        // written as typed, and taken out again when the field is emptied.
        app.choose("composing", "editor", "vim");
        assert_eq!(program(&app).as_deref(), Some(""));
        app.set(
            "composing",
            "editor_path",
            FormValue::Text("  /opt/homebrew/bin/nvim ".to_owned()),
        );
        assert_eq!(
            app.text(),
            "[compose]\neditor = \"vim\"\neditor_path = \"/opt/homebrew/bin/nvim\"\n"
        );
        assert_eq!(program(&app).as_deref(), Some("/opt/homebrew/bin/nvim"));
        // Neither a whole path nor a name: refused, and the file is as it was.
        app.set(
            "composing",
            "editor_path",
            FormValue::Text("bin/vim".to_owned()),
        );
        assert!(
            app.shown()
                .problem()
                .is_some_and(|said| said.contains("whole path"))
        );
        assert_eq!(program(&app).as_deref(), Some("/opt/homebrew/bin/nvim"));
        app.set(
            "composing",
            "editor_path",
            FormValue::Text("nvim".to_owned()),
        );
        assert_eq!(program(&app).as_deref(), Some("nvim"));
        app.set("composing", "editor_path", FormValue::Text(String::new()));
        assert_eq!(app.text(), "[compose]\neditor = \"vim\"\n");
        assert_eq!(program(&app).as_deref(), Some(""));
        // Emptied when it was empty, nothing is written.
        app.set(
            "composing",
            "editor_path",
            FormValue::Text("   ".to_owned()),
        );
        assert_eq!(app.text(), "[compose]\neditor = \"vim\"\n");
    }

    #[test]
    fn a_file_with_a_mistake_is_shown_as_one_and_nothing_is_pretended() {
        let mistaken = "[ui]\ntheme = \"light\"   # mine\nkeymap = \n";
        let mut app = App::over(Some(mistaken));
        app.open();
        let shown = app.shown();
        assert!(shown.panes().is_empty());
        let problem = shown.problem().unwrap();
        assert!(
            problem.starts_with("The settings file has a mistake in it")
                && problem.contains("Config line 3: "),
            "{problem}"
        );
        // Nothing can be changed, whatever is sent.
        app.set("general", "notifications", FormValue::Flag(false));
        assert_eq!(app.text(), mistaken);
        assert!(app.shown().panes().is_empty());

        // Put right by hand, and the window come back to: it is all there.
        write(
            &app.dir,
            "config.toml",
            "[ui]\ntheme = \"light\"   # mine\n",
        );
        app.open();
        assert_eq!(app.shown().panes().len(), 7);
        assert_eq!(app.shown().problem(), None);
        app.set("general", "notifications", FormValue::Flag(false));
        assert_eq!(
            app.text(),
            "[ui]\ntheme = \"light\"   # mine\nnotifications = false\n"
        );

        // Broken while the window is open: the next change is refused,
        // the file is as the user left it, and the window says so.
        let broken = "[ui]\ntheme = \"light\"   # mine\nnotifications = \n";
        write(&app.dir, "config.toml", broken);
        app.set("general", "threads", FormValue::Flag(false));
        assert_eq!(app.text(), broken);
        assert!(app.shown().panes().is_empty());
        assert!(app.env.threads);
    }

    #[test]
    fn coming_back_to_the_window_brings_quill_in_line_with_a_file_edited_by_hand() {
        let mut app = App::over(Some(WRITTEN));
        app.open();
        assert_eq!(app.env.styles.theme_name(), "light");
        write(
            &app.dir,
            "config.toml",
            &WRITTEN.replace("\"light\"", "\"terminal\""),
        );
        // Nothing happens by itself: the file is read when the window is
        // opened or come back to.
        assert_eq!(app.chosen("appearance", "theme_dark"), "light");
        app.open();
        assert_eq!(app.chosen("appearance", "theme_dark"), "terminal");
        assert_eq!(app.chosen("appearance", "theme_light"), "terminal");
        assert_eq!(app.env.styles.theme_name(), "terminal");
        // Closed, the window is told nothing more.
        app.act(Action::Settings(SettingsAct::Closed));
        assert!(app.state.settings_shown(&app.env).is_none());
    }

    /// A dark theme and a light one, as Ghostty writes them.
    const NIGHT: &str = "palette = 1=#f0606a\npalette = 4=#7aa0f0\nbackground = #1e2030\n\
                         foreground = #d0d4ea\nselection-background = #3a3f5c\n\
                         selection-foreground = #ffffff\n";
    const PAPER: &str = "background = #fbf6e8\nforeground = #5a6068\npalette = 4=#2a5db0\n";

    /// A colour as red, green and blue.
    type Rgb = Option<(u8, u8, u8)>;

    /// What the app is told of the look: the theme drawn with, and the
    /// colours and the appearance sent to the shell with the state.
    fn look(app: &App) -> (String, Rgb, Rgb, Appearance) {
        let host = app.state.host_state(&app.env);
        (
            app.env.styles.theme_name().to_owned(),
            host.background,
            host.foreground,
            host.appearance,
        )
    }

    #[test]
    fn an_old_config_is_drawn_as_it_always_was_whatever_the_mac_does() {
        // `theme` and nothing newer, as every config was.
        for (config, theme) in [
            (Some("[ui]\ntheme = \"light\"\n"), "light"),
            (Some("[ui]\ntheme = \"terminal\"\n"), "terminal"),
            (Some("[ui]\nkeymap = \"mutt\"\n"), "dark"),
            (Some(""), "dark"),
            (None, "dark"),
        ] {
            let mut app = App::over(config);
            assert_eq!(app.env.styles.theme_name(), theme, "{config:?}");
            for dark in [false, true, false] {
                app.act(Action::SystemAppearance { dark });
                assert_eq!(app.env.styles.theme_name(), theme, "{config:?}");
                assert_eq!(look(&app).3, Appearance::System);
            }
            // Nothing was written for any of it.
            assert_eq!(
                fs::read_to_string(file(&app.dir)).ok().as_deref(),
                config,
                "{config:?}"
            );
        }
    }

    #[test]
    fn the_three_appearances_each_have_their_theme_and_the_shell_is_told_its_colours() {
        let config = "[ui]\ntheme = \"terminal\"   # was this for years\n\
                      theme_light = \"Paper\"\ntheme_dark = \"Night Owl+\"\n";
        let prepared = TempDir::new().unwrap();
        write(&prepared, "themes/Night Owl+", NIGHT);
        write(&prepared, "themes/Paper", PAPER);
        write(&prepared, "config.toml", config);
        let mut app = App::in_dir(prepared);
        let night = (Some((0x1e, 0x20, 0x30)), Some((0xd0, 0xd4, 0xea)));
        let paper = (Some((0xfb, 0xf6, 0xe8)), Some((0x5a, 0x60, 0x68)));

        // As the system is, and the app has not said what that is: dark.
        assert_eq!(
            look(&app),
            (
                "Night Owl+".to_owned(),
                night.0,
                night.1,
                Appearance::System
            )
        );
        // The Mac is light: the light theme at once, and its colours for
        // the app's terminals.
        app.act(Action::SystemAppearance { dark: false });
        assert_eq!(
            look(&app),
            ("Paper".to_owned(), paper.0, paper.1, Appearance::System)
        );
        // The Mac goes dark, as at sunset.
        app.act(Action::SystemAppearance { dark: true });
        assert_eq!(look(&app).0, "Night Owl+");

        // Light, in the settings window: light whatever the Mac is, and
        // the shell is told to be light too.
        app.open();
        app.choose("appearance", "appearance", "light");
        assert_eq!(
            look(&app),
            ("Paper".to_owned(), paper.0, paper.1, Appearance::Light)
        );
        assert!(
            app.text()
                .ends_with("theme_dark = \"Night Owl+\"\nappearance = \"light\"\n")
        );
        app.act(Action::SystemAppearance { dark: true });
        assert_eq!(look(&app).0, "Paper");
        // Dark.
        app.choose("appearance", "appearance", "dark");
        assert_eq!(
            look(&app),
            ("Night Owl+".to_owned(), night.0, night.1, Appearance::Dark)
        );
        app.act(Action::SystemAppearance { dark: false });
        assert_eq!(look(&app).0, "Night Owl+");
        // And back to the system's, which is light now.
        app.choose("appearance", "appearance", "system");
        assert_eq!(
            look(&app),
            ("Paper".to_owned(), paper.0, paper.1, Appearance::System)
        );
        assert_eq!(
            app.text(),
            format!("{config}appearance = \"system\"\n"),
            "the comment and the old key are as they were written"
        );

        // The theme of the appearance in force is changed and drawn with
        // at once; the other's is changed and waits its turn.
        app.choose("appearance", "theme_light", "light");
        assert_eq!(look(&app).0, "light");
        app.choose("appearance", "theme_dark", "dark");
        assert_eq!(look(&app).0, "light");
        app.act(Action::SystemAppearance { dark: true });
        assert_eq!(look(&app).0, "dark");
        // `terminal` is a theme like any other: nothing is said of its
        // colours, and the app's terminals keep their own.
        app.choose("appearance", "theme_dark", "terminal");
        assert_eq!(
            look(&app),
            ("terminal".to_owned(), None, None, Appearance::System)
        );
    }

    #[test]
    fn in_a_terminal_the_systems_appearance_is_dark() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "config.toml",
            "[ui]\ntheme_light = \"light\"\ntheme_dark = \"terminal\"\n",
        );
        let paths = Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
        };
        let theme = |paths: &Paths| {
            let mut env = setup::load(paths, ColorMode::TrueColor, TimeZone::UTC).env;
            crate::suit_host(&mut env, false, None);
            (env.styles.theme_name().to_owned(), env.look.system_dark)
        };
        // Nobody says what the system is, and "system" is dark.
        assert_eq!(theme(&paths), ("terminal".to_owned(), None));
        // Said in the file, light is light and dark is dark.
        write(
            &dir,
            "config.toml",
            "[ui]\nappearance = \"light\"\ntheme_light = \"light\"\ntheme_dark = \"terminal\"\n",
        );
        assert_eq!(theme(&paths).0, "light");
        write(
            &dir,
            "config.toml",
            "[ui]\nappearance = \"dark\"\ntheme_light = \"light\"\ntheme_dark = \"terminal\"\n",
        );
        assert_eq!(theme(&paths).0, "terminal");
    }

    #[test]
    fn a_theme_the_shell_imported_is_read_from_the_folder_and_selected() {
        let mut app = App::over(Some(WRITTEN));
        app.open();
        let themes = |app: &App| match app.field("appearance", "theme_dark") {
            FormField::Choice { options, .. } => options
                .into_iter()
                .map(|option| option.title)
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(themes(&app), ["Dark", "Light", "Terminal"]);

        // The shell copies the file in and says so. The file has one theme
        // for both appearances, so that is the one that changes: where it
        // stands, and whatever the Mac is, now or later.
        write(&app.dir, "themes/Night Owl+", NIGHT);
        app.act(Action::ThemeImported("Night Owl+".to_owned()));
        assert_eq!(themes(&app), ["Dark", "Light", "Terminal", "Night Owl+"]);
        assert_eq!(app.env.styles.theme_name(), "Night Owl+");
        assert_eq!(look(&app).1, Some((0x1e, 0x20, 0x30)));
        assert_eq!(
            app.text(),
            WRITTEN.replace("theme = \"light\"   #", "theme = \"Night Owl+\"   #")
        );
        assert_eq!(app.shown().problem(), None);
        // Both pop-ups show it, by a name the bridge can carry.
        assert!(app.chosen("appearance", "theme_dark").starts_with("x_"));
        assert_eq!(
            app.chosen("appearance", "theme_light"),
            app.chosen("appearance", "theme_dark")
        );
        app.act(Action::SystemAppearance { dark: false });
        assert_eq!(app.env.styles.theme_name(), "Night Owl+");

        // Once the two appearances have themes of their own, an imported
        // theme is the theme of the one in force: the light one while the
        // Mac is light.
        app.choose("appearance", "theme_dark", "dark");
        write(&app.dir, "themes/Paper", PAPER);
        app.act(Action::ThemeImported("Paper".to_owned()));
        assert_eq!(app.env.styles.theme_name(), "Paper");
        assert!(app.text().contains("theme_light = \"Paper\"\n"));
        assert!(app.text().contains("theme_dark = \"dark\"\n"));
        assert!(
            app.text()
                .contains("theme = \"Night Owl+\"   # easier on the eyes\n")
        );
        // And the dark one while it is dark.
        app.act(Action::SystemAppearance { dark: true });
        assert_eq!(app.env.styles.theme_name(), "dark");
        app.act(Action::ThemeImported("Night Owl+".to_owned()));
        assert_eq!(app.env.styles.theme_name(), "Night Owl+");
        assert!(app.text().contains("theme_dark = \"Night Owl+\"\n"));
        assert!(app.text().contains("theme_light = \"Paper\"\n"));
        app.act(Action::SystemAppearance { dark: false });

        // A file that turns out not to be a theme is not selected, is not
        // listed, and that is said; the settings file is as it was.
        let before = app.text();
        write(&app.dir, "themes/Notes", "Remember the milk.\n");
        app.act(Action::ThemeImported("Notes".to_owned()));
        assert_eq!(app.text(), before);
        assert_eq!(app.env.styles.theme_name(), "Paper");
        assert_eq!(
            app.shown().problem(),
            Some(
                "That was not changed. themes/Notes is not a theme in Ghostty\u{2019}s format: it \
                 names no background and foreground colour."
            )
        );
        assert!(!themes(&app).contains(&"Notes".to_owned()));
        // Nor one that was never copied at all.
        app.act(Action::ThemeImported("Nowhere".to_owned()));
        assert_eq!(app.text(), before);
        assert_eq!(
            app.shown().problem(),
            Some("That was not changed. There is no theme called \"Nowhere\".")
        );
        // Nor one of the user's own that has a mistake in it: it is listed,
        // so that what is wrong can be said, and is not changed to.
        write(
            &app.dir,
            "themes/broken.toml",
            "name = \"broken\"\n[colors]\ntext = \"mauve\"\n",
        );
        app.open();
        assert!(themes(&app).contains(&"broken".to_owned()));
        app.choose("appearance", "theme_dark", "broken");
        assert_eq!(app.text(), before);
        assert!(
            app.shown().problem().is_some_and(|problem| problem
                .starts_with("That was not changed. themes/broken.toml: theme line 3: ")),
            "{:?}",
            app.shown().problem()
        );
    }
    fn press_theme(app: &mut App, action: &str, name: Option<&str>) {
        let row = name.map(quill_tui::settings::theme_id);
        let used = SettingsUsed::action("appearance", "themes", action, row.as_deref()).unwrap();
        app.act(Action::Settings(SettingsAct::Used(used)));
    }

    /// What the sheet's fields hold, with these put in their place, told
    /// to the core as the app tells it: with a button, or with none when
    /// a colour was picked.
    fn tell_sheet(app: &mut App, changed: &[(&str, &str)], button: Option<&str>) {
        let form = app.state.form(&app.env).unwrap();
        let mut values: Vec<(String, String)> = form
            .fields()
            .iter()
            .filter_map(|field| Some((field.id().to_owned(), field.value()?.to_owned())))
            .collect();
        for (id, value) in changed {
            let held = values.iter_mut().find(|(field, _)| field == id).unwrap();
            held.1 = (*value).to_owned();
        }
        let values: Vec<(&str, &str)> = values
            .iter()
            .map(|(id, value)| (id.as_str(), value.as_str()))
            .collect();
        app.act(Action::FormUsed(
            quill_host::bridge::FormUsed::new(form.step(), button, &values, &[]).unwrap(),
        ));
    }

    #[test]
    fn a_theme_made_in_the_window_is_a_file_in_the_folder_and_the_theme_in_use() {
        let mut app = App::over(None);
        write(&app.dir, "themes/Night Owl+", NIGHT);
        app.open();
        press_theme(&mut app, "edit", Some("Night Owl+"));
        // Drawn with while it is being changed, colour by colour.
        assert_eq!(app.env.styles.theme_name(), "Night Owl+");
        tell_sheet(&mut app, &[("background", "#102030")], None);
        assert_eq!(look(&app).1, Some((0x10, 0x20, 0x30)));
        tell_sheet(&mut app, &[("name", "Midnight")], Some("confirm"));

        // A file of Quill's own, and the one it was made from as it was.
        let themes = app.dir.path().join("config/themes");
        let text = fs::read_to_string(themes.join("Midnight.toml")).unwrap();
        assert!(text.contains("\nname = \"Midnight\"\n"), "{text}");
        assert!(text.contains("\nbackground = \"#102030\"\n"), "{text}");
        assert_eq!(
            fs::read_to_string(themes.join("Night Owl+")).unwrap(),
            NIGHT
        );
        // It is the theme, in the settings file and on the screen.
        assert_eq!(app.text(), "[ui]\ntheme = \"Midnight\"\n");
        assert_eq!(app.env.styles.theme_name(), "Midnight");
        assert_eq!(app.env.look.trying, None);
        assert_eq!(look(&app).1, Some((0x10, 0x20, 0x30)));
        assert_eq!(
            app.chosen("appearance", "theme_dark"),
            quill_tui::settings::theme_id("Midnight")
        );

        // Changed again under its own name: the same file, and seen.
        press_theme(&mut app, "edit", Some("Midnight"));
        tell_sheet(&mut app, &[("text", "#ffffff")], Some("confirm"));
        assert_eq!(look(&app).2, Some((0xff, 0xff, 0xff)));
        assert_eq!(app.text(), "[ui]\ntheme = \"Midnight\"\n");

        // Deleted: the file goes, the settings stop naming it, and Quill
        // is drawn in the theme it comes with.
        press_theme(&mut app, "delete", Some("Midnight"));
        tell_sheet(&mut app, &[], Some("confirm"));
        assert!(!themes.join("Midnight.toml").exists());
        assert_eq!(app.text(), "[ui]\n");
        assert_eq!(app.env.styles.theme_name(), "dark");
        assert!(themes.join("Night Owl+").exists());
    }

    #[test]
    fn the_font_named_in_the_window_is_in_the_file_and_the_shell_is_told_at_once() {
        let mut app = App::over(None);
        app.open();
        assert!(matches!(
            app.field("appearance", "font"),
            FormField::Font { value, .. } if value.is_empty()
        ));
        let told = |app: &App| {
            let state = app.state.host_state(&app.env);
            (state.font, state.font_size.map(Number::get))
        };
        assert_eq!(told(&app), (None, Some(13.0)));
        app.set(
            "appearance",
            "font",
            FormValue::Text("JetBrains Mono".to_owned()),
        );
        app.set(
            "appearance",
            "font_size",
            FormValue::Number(Number::new(14.5).unwrap()),
        );
        assert_eq!(
            app.text(),
            "[ui]\nfont = \"JetBrains Mono\"\nfont_size = 14.5\n"
        );
        assert_eq!(told(&app), (Some("JetBrains Mono".to_owned()), Some(14.5)));
        // The app's own font again: the line is taken out.
        app.set("appearance", "font", FormValue::Text(String::new()));
        app.set(
            "appearance",
            "font_size",
            FormValue::Number(Number::from(12)),
        );
        assert_eq!(app.text(), "[ui]\nfont_size = 12\n");
        assert_eq!(told(&app), (None, Some(12.0)));
        // A size no terminal is given is refused, and the file left alone.
        app.set(
            "appearance",
            "font_size",
            FormValue::Number(Number::from(200)),
        );
        assert_eq!(app.text(), "[ui]\nfont_size = 12\n");
        assert!(app.shown().problem().unwrap().contains("from 6 to 72"));
    }
}
