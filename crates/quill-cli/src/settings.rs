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
use quill_tui::settings::{AccountLine, Change, Settings, SettingsRead};
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
    // A theme is one there is: the file is not made to name another.
    if let Change::Theme(name) = change
        && !setup::themes_in(config_dir(config_file)).contains(name)
    {
        return Err(format!("There is no theme called \"{name}\"."));
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
    let (theme, theme_problem) = setup::theme_named(dir, &config.ui.theme);
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
        themes: setup::themes_in(dir),
        theme,
        theme_problem,
        keymap,
        keys_problem,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jiff::tz::TimeZone;
    use quill_core::config::{ComposeEditor, KeymapPreset, RemoteImages};
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
        assert_eq!(read.themes, ["dark", "light", "terminal"]);
        assert!(read.accounts.is_empty());
        assert_eq!((read.theme_problem, read.keys_problem), (None, None));
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
        // Not themes: not a `.toml`, a name no theme can have, and one
        // that is a built-in's own, which is that theme.
        write(&dir, "themes/notes.txt", "");
        write(&dir, "themes/my theme.toml", "");
        write(&dir, "themes/dark.toml", "name = \"my dark\"\n");
        write(&dir, "keys.toml", "\"message.toggle_unread\" = \"m\"\n");

        let read = read_of(answer(&file(&dir), None));
        assert_eq!(read.settings.theme, "light");
        assert_eq!(read.settings.keymap, KeymapPreset::Vim);
        assert_eq!(read.settings.undo_send_seconds, 10);
        assert_eq!(
            read.themes,
            ["dark", "light", "terminal", "Solarized-2", "mine"]
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
        let read = read_of(answer(&file(&dir), Some(Change::Theme("mine".to_owned()))));
        assert_eq!(read.settings.theme, "mine");
        assert_eq!(read.theme.name, "mine");
        assert_eq!(read.theme_problem, None);
        write(
            &dir,
            "themes/mine.toml",
            "name = \"mine\"\n[colors]\ntext = \"mauve\"\n",
        );
        let read = read_of(answer(&file(&dir), None));
        assert_eq!(read.theme.name, "dark");
        assert!(
            read.theme_problem
                .as_deref()
                .is_some_and(|problem| problem.starts_with("themes/mine.toml: ")),
            "{:?}",
            read.theme_problem
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
        assert_eq!(read.settings.theme, "terminal");
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
        for name in ["nope", "../../etc/passwd", ""] {
            let event = answer(&file(&dir), Some(Change::Theme(name.to_owned())));
            let AccountEvent::Settings {
                read: Ok(read),
                changed: Some((_, Err(reason))),
            } = event
            else {
                panic!("{name:?} was written");
            };
            assert!(reason.contains("There is no theme called"), "{reason}");
            assert_eq!(read.settings.theme, "light");
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
                for request in effects.accounts {
                    match request {
                        AccountRequest::Settings(change) => {
                            waiting.push_back(Action::Account(answer(&file(&self.dir), change)));
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
        assert_eq!(app.shown().panes().len(), 5);
        assert_eq!(app.shown().problem(), None);
        assert_eq!(app.chosen("general", "theme"), "light");
        // Opening wrote nothing.
        assert_eq!(app.text(), WRITTEN);

        // The theme: drawn with from now on.
        app.choose("general", "theme", "dark");
        assert_eq!(app.chosen("general", "theme"), "dark");
        assert_eq!(app.env.styles.theme_name(), "dark");
        assert_eq!(
            app.text(),
            WRITTEN.replace("theme = \"light\"   #", "theme = \"dark\"   #")
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
        assert_eq!(config.ui.theme, "dark");
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
        let mut app = App::over(Some("[compose]\neditor = \"script\"\n"));
        app.open();
        app.choose("composing", "editor", "vim");
        assert_eq!(app.env.external, ExternalEditor::Script);
        assert!(
            help(&app, "composing", "editor")
                .ends_with("Until then messages are written in your own Vim.")
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
            ("general", "theme", FormValue::Text("solarized".to_owned())),
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
            // mg does not run in the app, and is not to be chosen in it.
            ("composing", "editor", FormValue::Text("mg".to_owned())),
            // The wrong kind of thing altogether.
            (
                "general",
                "notifications",
                FormValue::Text("yes".to_owned()),
            ),
            ("general", "theme", FormValue::Flag(true)),
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
    fn mg_is_not_offered_in_the_app_and_is_still_shown_when_the_file_says_it() {
        let options = |app: &App| match app.field("composing", "editor") {
            FormField::Choice { options, .. } => options
                .into_iter()
                .map(|option| option.id)
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        let mut app = App::over(Some(WRITTEN));
        app.open();
        assert_eq!(options(&app), ["builtin", "vim", "nano", "pico", "script"]);

        // A file that says mg, as it may for Quill in a terminal: the
        // window shows what the file says, and offers no way back to it.
        let mut app = App::over(Some("[compose]\neditor = \"mg\"\n"));
        app.open();
        assert_eq!(app.chosen("composing", "editor"), "mg");
        assert_eq!(
            options(&app),
            ["builtin", "vim", "nano", "pico", "script", "mg"]
        );
        app.choose("composing", "editor", "pico");
        assert_eq!(options(&app), ["builtin", "vim", "nano", "pico", "script"]);
        assert_eq!(app.text(), "[compose]\neditor = \"pico\"\n");
        app.choose("composing", "editor", "mg");
        assert_eq!(app.text(), "[compose]\neditor = \"pico\"\n");

        // In a terminal it is one of the editors, though there is no
        // window there to offer it in.
        let mut env = App::over(None).env;
        crate::suit_host(&mut env, false, None);
        assert!(quill_tui::settings::editors_offered(&env).contains(&ComposeEditor::Mg));
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
        assert_eq!(app.shown().panes().len(), 5);
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
        assert_eq!(app.chosen("general", "theme"), "light");
        app.open();
        assert_eq!(app.chosen("general", "theme"), "terminal");
        assert_eq!(app.env.styles.theme_name(), "terminal");
        // Closed, the window is told nothing more.
        app.act(Action::Settings(SettingsAct::Closed));
        assert!(app.state.settings_shown(&app.env).is_none());
    }
}
