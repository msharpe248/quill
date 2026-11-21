//! The settings window of Quill's own app, from the core's side: what the
//! host is told to show, what a thing done in the window comes to, and
//! which settings hold from the moment they are changed.
//!
//! The host here is the harness, and the service that reads and writes the
//! settings file is a stand-in that keeps the settings in memory. The file
//! itself, comments and all, is the business of `quill-cli`, whose tests
//! run this same state against the real file.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::config::{
    AccountType, Appearance, ComposeEditor, Config, KeymapPreset, RemoteImages,
};
use quill_core::types::{
    Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_host::bridge::{
    self, CoreToShell, FormField, FormValue, Number, SettingsShow, SettingsUsed,
};
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataEvent;
use quill_tui::keymap::Keymap;
use quill_tui::settings::{AccountLine, Change, Settings, SettingsRead};
use quill_tui::setup::{AccountEvent, AccountRequest, Preference};
use quill_tui::state::{Action, HostRequest, SettingsAct};
use quill_tui::theme::Theme;

/// The settings file, as the stand-in service keeps it.
struct File {
    settings: Settings,
    accounts: Vec<AccountLine>,
    /// Set to make the file one with a mistake in it.
    mistake: Option<String>,
    /// Why the next change cannot be written, if it cannot.
    unwritable: Option<String>,
    /// The file names a theme for one appearance or the other, and not
    /// only the one theme for both.
    apart: bool,
}

impl File {
    fn of(config: &str) -> Self {
        let config = Config::parse(config).unwrap();
        Self {
            settings: Settings::of(&config),
            accounts: config
                .accounts
                .iter()
                .map(|account| AccountLine {
                    name: account.name.clone(),
                    kind: account.kind,
                    address: account.address.clone(),
                    signs_in: account.kind == AccountType::Graph,
                    letter: None,
                })
                .collect(),
            mistake: None,
            unwritable: None,
            apart: config.ui.theme_light.is_some() || config.ui.theme_dark.is_some(),
        }
    }

    fn read(&self) -> Result<Box<SettingsRead>, String> {
        if let Some(mistake) = &self.mistake {
            return Err(mistake.clone());
        }
        Ok(Box::new(SettingsRead {
            settings: self.settings.clone(),
            accounts: self.accounts.clone(),
            themes: quill_tui::settings::ThemeLine::builtins(),
            theme_light: Theme::builtin(&self.settings.theme_light).unwrap(),
            theme_light_problem: None,
            theme_dark: Theme::builtin(&self.settings.theme_dark).unwrap(),
            theme_dark_problem: None,
            keymap: Keymap::preset(self.settings.keymap),
            keys_problem: None,
        }))
    }

    fn make(&mut self, change: &Change) -> Result<(), String> {
        if self.mistake.is_some() {
            return Err("config.toml has a mistake to fix first".to_owned());
        }
        if let Some(reason) = self.unwritable.take() {
            return Err(reason);
        }
        let settings = &mut self.settings;
        match change.clone() {
            Change::Appearance(appearance) => settings.appearance = appearance,
            Change::ThemeLight(name) => {
                settings.theme_light = name;
                self.apart = true;
            }
            Change::ThemeDark(name) => {
                settings.theme_dark = name;
                self.apart = true;
            }
            // As the real service does it: the one theme, in a file that
            // has one; the theme of the appearance in force otherwise.
            Change::ThemeInForce { name, dark } => match (self.apart, dark) {
                (false, _) => {
                    settings.theme_light = name.clone();
                    settings.theme_dark = name;
                }
                (true, true) => settings.theme_dark = name,
                (true, false) => settings.theme_light = name,
            },
            Change::Font(family) => settings.font = family,
            Change::FontSize(points) => settings.font_size = points,
            Change::Keymap(preset) => settings.keymap = preset,
            Change::Notifications(on) => settings.notifications = on,
            Change::Threads(on) => settings.threads = on,
            Change::WebReading(on) => settings.web_reading = on,
            Change::Editor(editor) => settings.editor = editor,
            Change::EditorPath(path) => settings.editor_path = path,
            Change::Format(format) => settings.format = format,
            Change::ReplyPosition(position) => settings.reply_position = position,
            Change::SpellCheck(on) => settings.spell_check = on,
            Change::UndoSendSeconds(seconds) => settings.undo_send_seconds = seconds,
            Change::RemoteImages(remote) => settings.remote_images = remote,
            Change::CacheGb(gigabytes) => settings.cache_gb = gigabytes,
            Change::AgentCommands(on) => settings.agent.commands = on,
            Change::AgentMcp(on) => settings.agent.mcp = on,
            Change::AgentOrganize(on) => settings.agent.organize = on,
            Change::AgentDelete(on) => settings.agent.delete = on,
            Change::AgentDraft(on) => settings.agent.draft = on,
            Change::AgentSend(on) => settings.agent.send = on,
            Change::AgentSendGap(seconds) => settings.agent.send_gap = seconds,
            Change::AgentHoldSends(on) => settings.agent.hold_sends = on,
            Change::AgentNotify(notify) => settings.agent.notify = notify,
            Change::RulesEnabled(on) => settings.rules_enabled = on,
            Change::RulesLookBack(age) => settings.rules_look_back = age,
        }
        Ok(())
    }
}

/// A config with two accounts, one that signs in with Microsoft.
const CONFIG: &str = "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\n\n\
                      [[account]]\nname = \"Home\"\ntype = \"imap\"\naddress = \"robin@example.org\"\n\
                      imap = { host = \"mail.example.org\" }\n";

/// The service's turn: every request about the settings that the UI has
/// made and not had answered is answered, in order, and whatever those
/// answers lead it to ask is answered too.
fn serve(harness: &mut Harness, file: &mut File, answered: &mut usize) {
    while *answered < harness.account_requests.len() {
        let request = harness.account_requests[*answered].clone();
        *answered += 1;
        match request {
            AccountRequest::Settings(change) => {
                let changed = change.map(|change| {
                    let made = file.make(&change);
                    (change, made)
                });
                harness.act(Action::Account(AccountEvent::Settings {
                    read: file.read(),
                    changed,
                }));
            }
            // What the web reading command has remembered is in the file
            // from then on.
            AccountRequest::Remember(Preference::WebReading(on)) => {
                file.settings.web_reading = on;
                harness.act(Action::Account(AccountEvent::Remembered(Ok(()))));
            }
            // And that the agents were stopped.
            AccountRequest::Remember(Preference::AgentsOff) => {
                file.settings.agent.commands = false;
                file.settings.agent.mcp = false;
                harness.act(Action::Account(AccountEvent::Remembered(Ok(()))));
            }
            _ => {}
        }
    }
}

/// The app with its settings window, and the file behind it.
struct Window {
    harness: Harness,
    file: File,
    answered: usize,
}

impl Window {
    /// Over the demo mailbox in Quill's own app, with a service to add
    /// accounts through.
    fn new(config: &str) -> Self {
        Self::over(Harness::with_accounts(200, 60), config)
    }

    fn over(mut harness: Harness, config: &str) -> Self {
        harness.env.sheets = true;
        harness.env.editor_pane = true;
        let file = File::of(config);
        let answered = harness.account_requests.len();
        Self {
            harness,
            file,
            answered,
        }
    }

    fn serve(&mut self) {
        serve(&mut self.harness, &mut self.file, &mut self.answered);
    }

    fn open(&mut self) {
        self.harness.act(Action::Settings(SettingsAct::Opened));
        self.serve();
    }

    fn set(&mut self, pane: &str, field: &str, value: FormValue) {
        let used = SettingsUsed::set(pane, field, value).unwrap();
        self.harness.act(Action::Settings(SettingsAct::Used(used)));
        self.serve();
    }

    fn choose(&mut self, pane: &str, field: &str, option: &str) {
        self.set(pane, field, FormValue::Text(option.to_owned()));
    }

    fn press(&mut self, action: &str, row: Option<&str>) {
        let used = SettingsUsed::action("accounts", "accounts", action, row).unwrap();
        self.harness.act(Action::Settings(SettingsAct::Used(used)));
        self.serve();
    }

    fn shown(&self) -> SettingsShow {
        self.harness
            .state
            .settings_shown(&self.harness.env)
            .expect("the window has nothing to show")
            .1
    }

    fn answers(&self) -> u64 {
        self.harness
            .state
            .settings_shown(&self.harness.env)
            .unwrap()
            .0
    }

    fn field(&self, pane: &str, field: &str) -> FormField {
        self.shown().field(pane, field).unwrap().clone()
    }

    /// The names of the accounts' rows, and of the list's actions that can
    /// be pressed at all.
    fn accounts(&self) -> (Vec<(String, Vec<String>)>, Vec<String>) {
        match self.field("accounts", "accounts") {
            FormField::List { rows, actions, .. } => (
                rows.into_iter()
                    .map(|row| (row.title, row.actions))
                    .collect(),
                actions
                    .into_iter()
                    .filter(|action| action.enabled)
                    .map(|action| action.id)
                    .collect(),
            ),
            other => panic!("{other:?}"),
        }
    }

    fn row(&self, title: &str) -> String {
        match self.field("accounts", "accounts") {
            FormField::List { rows, .. } => {
                rows.into_iter().find(|row| row.title == title).unwrap().id
            }
            other => panic!("{other:?}"),
        }
    }
}

/// The window as lines of text: each pane, and each field with its kind,
/// what it holds and what is said beside it.
fn outline(shown: &SettingsShow) -> String {
    let mut lines = Vec::new();
    if let Some(problem) = shown.problem() {
        lines.push(format!("problem: {problem}"));
    }
    for pane in shown.panes() {
        lines.push(format!("== {} ({}, {})", pane.title, pane.id, pane.symbol));
        if let Some(note) = &pane.note {
            lines.push(format!("   note: {note}"));
        }
        for field in &pane.fields {
            let holds = match field {
                FormField::Choice { value, options, .. } => format!(
                    "choice = {value} of {}",
                    options
                        .iter()
                        .map(|option| format!("{} \"{}\"", option.id, option.title))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
                FormField::Toggle { title, value, .. } => format!("toggle \"{title}\" = {value}"),
                FormField::Number {
                    value,
                    min,
                    max,
                    step,
                    decimals,
                    unit,
                    ..
                } => format!(
                    "number = {} from {} to {} by {}, {decimals} places, {}",
                    value.get(),
                    min.get(),
                    max.map_or_else(|| "any".to_owned(), |max| max.get().to_string()),
                    step.get(),
                    unit.as_deref().unwrap_or("no unit"),
                ),
                FormField::List {
                    rows,
                    actions,
                    empty,
                    ..
                } => {
                    let mut said = vec!["list".to_owned()];
                    for row in rows {
                        said.push(format!(
                            "     row \"{}\": {} [{}]",
                            row.title,
                            row.detail.as_deref().unwrap_or(""),
                            row.actions.join(", ")
                        ));
                    }
                    for action in actions {
                        said.push(format!(
                            "     action {} \"{}\"{}{}",
                            action.id,
                            action.title,
                            if action.row { ", of a row" } else { "" },
                            if action.enabled { "" } else { ", off" },
                        ));
                    }
                    said.push(format!("     empty: {}", empty.as_deref().unwrap_or("")));
                    said.join("\n")
                }
                FormField::Button { title, enabled, .. } => {
                    format!("button \"{title}\"{}", if *enabled { "" } else { ", off" })
                }
                FormField::Text { value, .. } => format!("text = {value:?}"),
                FormField::Font { value, .. } => format!("font = {value:?}"),
                other => format!("{other:?}"),
            };
            lines.push(format!("   {} \"{}\": {holds}", field.id(), field.label()));
            if let Some(help) = field.help() {
                lines.push(format!("     help: {help}"));
            }
        }
        if let Some(more) = &pane.more {
            lines.push(format!("   more: {more}"));
        }
    }
    lines.join("\n")
}

#[test]
fn nothing_is_shown_until_the_window_is_opened_and_the_file_has_been_read() {
    let mut window = Window::new(CONFIG);
    let shown = |window: &Window| window.harness.state.settings_shown(&window.harness.env);
    assert!(shown(&window).is_none());
    // Opened: the file is asked for, and until the answer there is still
    // nothing to show.
    window.harness.act(Action::Settings(SettingsAct::Opened));
    assert_eq!(
        window.harness.account_requests.last(),
        Some(&AccountRequest::Settings(None))
    );
    assert!(shown(&window).is_none());
    window.serve();
    assert_eq!(window.shown().panes().len(), 8);
    // It is no question drawn over the panes: the terminal is as it was,
    // and the keys are still the list's.
    assert_eq!(window.harness.state.context(), Context::List);
    assert!(window.harness.state.form(&window.harness.env).is_none());
    // Closed: nothing more is shown, and an answer still on its way is
    // not taken for a window that is open.
    window.harness.act(Action::Settings(SettingsAct::Closed));
    assert!(shown(&window).is_none());
    let late = AccountEvent::Settings {
        read: window.file.read(),
        changed: None,
    };
    window.harness.act(Action::Account(late));
    assert!(shown(&window).is_none());
}

#[test]
fn the_window_is_described_pane_by_pane_from_what_the_file_says() {
    // The defaults, and nobody to sign in.
    let mut window = Window::new("");
    window.open();
    insta::assert_snapshot!("settings_by_default", outline(&window.shown()));

    // A file that says something of every setting.
    let mut window = Window::new(&format!(
        "[ui]\ntheme = \"light\"\nkeymap = \"mutt\"\nnotifications = false\nthreads = false\n\
         web_reading = true\n\n[compose]\neditor = \"nano\"\neditor_path = \"/opt/homebrew/bin/nano\"\n\
         format = \"plain\"\n\
         reply_position = \"below\"\nspell_check = false\nundo_send_seconds = 30\n\n\
         [privacy]\nremote_images = \"never\"\n\n[storage]\ncache_gb = 0.5\n\n{CONFIG}"
    ));
    window.open();
    let shown = window.shown();
    insta::assert_snapshot!("settings_as_the_file_says", outline(&shown));

    // What is described is a message the bridge carries, whole.
    let message = CoreToShell::SettingsShow(shown);
    let line = bridge::encode(&message).unwrap();
    assert_eq!(bridge::decode::<CoreToShell>(&line).unwrap(), message);
}

#[test]
fn a_change_goes_to_the_service_and_the_window_shows_what_the_file_then_says() {
    let mut window = Window::new(CONFIG);
    window.open();
    let before = window.answers();
    window.choose("appearance", "theme_dark", "light");
    assert_eq!(
        window.harness.account_requests.last(),
        Some(&AccountRequest::Settings(Some(Change::ThemeDark(
            "light".to_owned()
        ))))
    );
    assert_eq!(window.file.settings.theme_dark, "light");
    assert!(matches!(
        window.field("appearance", "theme_dark"),
        FormField::Choice { value, .. } if value == "light"
    ));
    // The host is told the settings once for the change, when it is made.
    assert_eq!(window.answers(), before + 1);

    // A change the file could not take: said, and the window shows what
    // the file still says.
    window.file.unwritable = Some("config.toml cannot be written: Permission denied".to_owned());
    window.set("general", "notifications", FormValue::Flag(false));
    assert!(window.file.settings.notifications);
    assert_eq!(
        window.shown().problem(),
        Some("That was not changed. config.toml cannot be written: Permission denied")
    );
    assert!(matches!(
        window.field("general", "notifications"),
        FormField::Toggle { value: true, .. }
    ));
    assert!(window.harness.env.notifications);
    assert_eq!(window.answers(), before + 2);
    // The next change that is made says nothing of the last.
    window.set("general", "notifications", FormValue::Flag(false));
    assert_eq!(window.shown().problem(), None);
    assert!(!window.harness.env.notifications);

    // What the window never offered is not asked of the service at all.
    let asked = window.harness.account_requests.len();
    window.choose("appearance", "theme_dark", "solarized");
    window.choose("appearance", "theme_light", "solarized");
    window.choose("appearance", "appearance", "sepia");
    // The old one pop-up of themes is gone from the window.
    window.choose("appearance", "theme", "light");
    window.set(
        "composing",
        "undo_send_seconds",
        FormValue::Number(Number::new(-5.0).unwrap()),
    );
    window.choose("composing", "editor", "emacs");
    assert_eq!(window.harness.account_requests.len(), asked);
    assert!(window.shown().problem().is_some());
}

#[test]
fn the_settings_that_hold_at_once_do_and_the_two_that_wait_do_not() {
    let mut window = Window::new(CONFIG);
    window.open();
    let env = |window: &Window| {
        let env = &window.harness.env;
        (
            env.styles.theme_name().to_owned(),
            env.keymap.keys_for(CommandId::MessageToggleUnread)[0].to_string(),
            env.notifications,
            env.threads,
            (
                env.compose.markdown,
                env.compose.reply_above,
                env.compose.spell_check,
                env.compose.undo_send_seconds,
            ),
            env.remote_never,
        )
    };
    assert_eq!(
        env(&window),
        (
            "dark".to_owned(),
            "u".to_owned(),
            true,
            true,
            (true, true, true, 10),
            false
        )
    );
    window.choose("appearance", "theme_dark", "terminal");
    window.choose("general", "keymap", "mutt");
    window.set("general", "notifications", FormValue::Flag(false));
    window.set("general", "threads", FormValue::Flag(false));
    window.choose("composing", "format", "plain");
    window.choose("composing", "reply_position", "below");
    window.set("composing", "spell_check", FormValue::Flag(false));
    window.set(
        "composing",
        "undo_send_seconds",
        FormValue::Number(Number::from(45)),
    );
    window.choose("privacy", "remote_images", "never");
    assert_eq!(
        env(&window),
        (
            "terminal".to_owned(),
            "N".to_owned(),
            false,
            false,
            (false, false, false, 45),
            true
        )
    );
    // The app is told every command's keys again when they change, and
    // only then.
    assert_eq!(window.harness.keys_announced, 1);
    // The new keys are the ones that work, from the next key on.
    let unread = window.harness.state.list.rows[0].flags.seen;
    window.harness.keys("N");
    assert_ne!(window.harness.state.list.rows[0].flags.seen, unread);

    // What a message is written in, and the room for downloaded mail:
    // written, and not what Quill goes by until it is started again.
    window.choose("composing", "editor", "nano");
    window.set("storage", "cache_gb", FormValue::Number(Number::from(1)));
    assert_eq!(window.file.settings.editor, ComposeEditor::Nano);
    assert_eq!(window.file.settings.cache_gb, Number::from(1));
    let env = &window.harness.env;
    assert!(!env.compose.vim);
    assert_eq!(env.external, quill_tui::vim::ExternalEditor::Vim);
    assert_eq!(env.cache_gb, 5.0);
    // A new message is still written here, in Quill's own editor.
    window
        .harness
        .act(Action::Command(CommandId::MessageCompose));
    assert_eq!(window.harness.state.context(), Context::Compose);
    assert!(window.harness.in_vim().is_none());
}

/// The demo mailbox with a conversation of three at the top of the Work
/// inbox, a message from someone else arriving in the middle of it.
#[test]
fn the_host_is_told_the_colours_of_the_theme_from_the_moment_it_is_changed() {
    // What the app gives its terminals to match: the editor's pane beside
    // Quill's own, and the cells Quill does not paint.
    let mut window = Window::new(CONFIG);
    window.open();
    let colours = |window: &Window| {
        let state = window.harness.state.host_state(&window.harness.env);
        (state.background, state.foreground)
    };
    assert_eq!(
        colours(&window),
        (Some((0x1a, 0x1b, 0x26)), Some((0xc0, 0xca, 0xf5)))
    );
    window.choose("appearance", "theme_dark", "light");
    assert_eq!(
        colours(&window),
        (Some((0xfa, 0xfa, 0xfa)), Some((0x38, 0x3a, 0x42)))
    );
    // The terminal's own colours: nothing is said, and the app's
    // terminals keep theirs.
    window.choose("appearance", "theme_dark", "terminal");
    assert_eq!(colours(&window), (None, None));
    window.choose("appearance", "theme_dark", "dark");
    assert_eq!(
        colours(&window),
        (Some((0x1a, 0x1b, 0x26)), Some((0xc0, 0xca, 0xf5)))
    );
}

/// The app with a theme for each appearance: the light one and the dark
/// one that come with Quill.
fn with_a_theme_for_each() -> Window {
    let mut window = Window::new(&format!(
        "[ui]\ntheme_light = \"light\"\ntheme_dark = \"dark\"\n\n{CONFIG}"
    ));
    window.open();
    window
}

fn theme_in_force(window: &Window) -> (String, Option<(u8, u8, u8)>, Appearance) {
    let state = window.harness.state.host_state(&window.harness.env);
    (
        window.harness.env.styles.theme_name().to_owned(),
        state.background,
        state.appearance,
    )
}

const DARK: Option<(u8, u8, u8)> = Some((0x1a, 0x1b, 0x26));
const LIGHT: Option<(u8, u8, u8)> = Some((0xfa, 0xfa, 0xfa));

#[test]
fn light_or_dark_by_the_settings_and_the_host_is_told_which() {
    let mut window = with_a_theme_for_each();
    // As the system is, and nobody has said what that is: dark.
    assert_eq!(
        theme_in_force(&window),
        ("dark".to_owned(), DARK, Appearance::System)
    );
    assert!(matches!(
        window.field("appearance", "appearance"),
        FormField::Choice { value, options, .. }
            if value == "system" && options.iter().map(|option| option.id.as_str()).eq(["system", "light", "dark"])
    ));

    // Light: the light theme at once, and the host is told to be light,
    // with the colours of that theme for its terminals.
    window.choose("appearance", "appearance", "light");
    assert_eq!(window.file.settings.appearance, Appearance::Light);
    assert_eq!(
        theme_in_force(&window),
        ("light".to_owned(), LIGHT, Appearance::Light)
    );
    // The theme for the appearance that is not in force can be changed,
    // and nothing is drawn differently.
    window.choose("appearance", "theme_dark", "terminal");
    assert_eq!(theme_in_force(&window).0, "light");
    // Dark: that one.
    window.choose("appearance", "appearance", "dark");
    assert_eq!(
        theme_in_force(&window),
        ("terminal".to_owned(), None, Appearance::Dark)
    );
    window.choose("appearance", "theme_dark", "dark");
    assert_eq!(
        theme_in_force(&window),
        ("dark".to_owned(), DARK, Appearance::Dark)
    );
    // What the system is makes no difference while the settings say.
    window.harness.act(Action::SystemAppearance { dark: false });
    assert_eq!(theme_in_force(&window).0, "dark");
    // Back to the system's: light, since that is what it was last said
    // to be.
    window.choose("appearance", "appearance", "system");
    assert_eq!(
        theme_in_force(&window),
        ("light".to_owned(), LIGHT, Appearance::System)
    );
}

#[test]
fn as_the_system_is_follows_the_system_as_the_host_says_it_changes() {
    let mut window = with_a_theme_for_each();
    assert_eq!(theme_in_force(&window).0, "dark");
    // The app says what the Mac is as it starts, and whenever it changes.
    window.harness.act(Action::SystemAppearance { dark: false });
    assert_eq!(
        theme_in_force(&window),
        ("light".to_owned(), LIGHT, Appearance::System)
    );
    // Said again, nothing changes.
    window.harness.act(Action::SystemAppearance { dark: false });
    assert_eq!(theme_in_force(&window).0, "light");
    window.harness.act(Action::SystemAppearance { dark: true });
    assert_eq!(
        theme_in_force(&window),
        ("dark".to_owned(), DARK, Appearance::System)
    );
    // The window says what the Mac is now, beside the choice.
    let help = |window: &Window| {
        window
            .field("appearance", "appearance")
            .help()
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        help(&window),
        "System follows the Mac, and changes with it. The Mac is dark now."
    );
    window.harness.act(Action::SystemAppearance { dark: false });
    assert!(help(&window).ends_with("The Mac is light now."));

    // A config that names one theme, as every config did, has that theme
    // whatever the Mac does.
    let mut old = Window::new(&format!("[ui]\ntheme = \"light\"\n\n{CONFIG}"));
    old.open();
    for dark in [true, false, true] {
        old.harness.act(Action::SystemAppearance { dark });
        assert_eq!(
            theme_in_force(&old),
            ("light".to_owned(), LIGHT, Appearance::System)
        );
    }
    // And both pop-ups show it.
    for field in ["theme_light", "theme_dark"] {
        assert!(matches!(
            old.field("appearance", field),
            FormField::Choice { value, .. } if value == "light"
        ));
    }
}

#[test]
fn in_a_terminal_nobody_says_what_the_system_is_and_it_is_dark() {
    // The harness is told nothing of the system, as Quill in a terminal
    // is not: with a theme for each, the dark one is drawn with.
    let window = with_a_theme_for_each();
    assert_eq!(window.harness.env.look.system_dark, None);
    assert!(window.harness.env.look.is_dark());
    assert_eq!(window.harness.env.styles.theme_name(), "dark");
    // The window is not told the Mac is anything.
    assert_eq!(
        window.field("appearance", "appearance").help(),
        Some("System follows the Mac, and changes with it.")
    );
}

#[test]
fn the_button_that_imports_a_theme_has_the_host_show_its_chooser() {
    let mut window = Window::new(CONFIG);
    window.open();
    assert!(matches!(
        window.field("appearance", "themes"),
        FormField::List { actions, .. }
            if actions.iter().any(|action| {
                action.id == "import" && action.title == "Import\u{2026}" && action.enabled && !action.row
            })
    ));
    let asked = window.harness.account_requests.len();
    let before = window.answers();
    let press = |window: &mut Window, field: &str, action: &str, row: Option<&str>| {
        let used = SettingsUsed::action("appearance", field, action, row).unwrap();
        window
            .harness
            .act(Action::Settings(SettingsAct::Used(used)));
        window.serve();
    };
    press(&mut window, "themes", "import", None);
    assert_eq!(
        window.harness.host_requests.last(),
        Some(&HostRequest::PickTheme)
    );
    // Nothing is asked of the service until a theme has been chosen, and
    // the window is told the settings again, as for anything done in it.
    assert_eq!(window.harness.account_requests.len(), asked);
    assert_eq!(window.answers(), before + 1);
    // Anything else done to it is nothing.
    let chooser = |window: &Window| {
        window
            .harness
            .host_requests
            .iter()
            .filter(|request| **request == HostRequest::PickTheme)
            .count()
    };
    press(&mut window, "themes", "open", None);
    press(&mut window, "theme_dark", "import", None);
    window.set("appearance", "themes", FormValue::Flag(true));
    assert_eq!(chooser(&window), 1);
}

#[test]
fn under_the_editor_is_the_program_it_is_for_any_but_quills_own() {
    let mut window = Window::new(CONFIG);
    window.open();
    let program = |window: &Window| -> Option<(usize, String, String)> {
        let shown = window.shown();
        let pane = shown
            .panes()
            .iter()
            .find(|pane| pane.id == "composing")
            .unwrap();
        pane.fields
            .iter()
            .enumerate()
            .find_map(|(index, field)| match field {
                FormField::Text {
                    id, value, help, ..
                } if id == "editor_path" => {
                    Some((index, value.clone(), help.clone().unwrap_or_default()))
                }
                _ => None,
            })
    };
    // Quill's own editor is no program, and nothing is asked. Nor is
    // there anything to import or set up for any editor: each is the
    // user's own, with their own settings for it.
    assert_eq!(program(&window), None);
    let shown = window.shown();
    assert!(shown.panes().iter().all(|pane| {
        pane.id == "general"
            || pane
                .fields
                .iter()
                .all(|field| !matches!(field, FormField::Button { .. }))
    }));
    window.set(
        "composing",
        "editor_path",
        FormValue::Text("/opt/homebrew/bin/nvim".to_owned()),
    );
    assert_eq!(window.file.settings.editor_path, None);

    // Any other editor has it, right under the editor, saying which
    // program is run when none is named and where the app looks for it.
    for (editor, name) in [
        ("vim", "vim"),
        ("nano", "nano"),
        ("pico", "pico"),
        ("mg", "mg"),
    ] {
        window.choose("composing", "editor", editor);
        let (index, value, help) = program(&window).unwrap();
        assert_eq!((index, value.as_str()), (1, ""), "{editor}");
        assert!(
            help.contains(&format!("runs the {name} it finds")),
            "{help}"
        );
        assert!(help.contains("Homebrew"), "{help}");
        assert!(help.contains("next opened"), "{help}");
    }
    // In a terminal the PATH is the user's own. There is no window there,
    // but what would be said is not about the app.
    let mut terminal = Window::new(CONFIG);
    terminal.harness.env.editor_pane = false;
    terminal.open();
    terminal.choose("composing", "editor", "vim");
    assert!(program(&terminal).unwrap().2.contains("on your PATH"));

    // A program named is kept as typed, less the spaces around it.
    window.choose("composing", "editor", "vim");
    window.set(
        "composing",
        "editor_path",
        FormValue::Text(" /opt/homebrew/bin/nvim ".to_owned()),
    );
    assert_eq!(
        window.file.settings.editor_path.as_deref(),
        Some("/opt/homebrew/bin/nvim")
    );
    assert_eq!(program(&window).unwrap().1, "/opt/homebrew/bin/nvim");
    // What is neither a whole path nor a name is refused, and said.
    window.set(
        "composing",
        "editor_path",
        FormValue::Text("bin/vim".to_owned()),
    );
    assert!(
        window
            .shown()
            .problem()
            .is_some_and(|said| said.contains("whole path")),
        "{:?}",
        window.shown().problem()
    );
    assert_eq!(
        window.file.settings.editor_path.as_deref(),
        Some("/opt/homebrew/bin/nvim")
    );
    // Emptied, none is named.
    window.set("composing", "editor_path", FormValue::Text("  ".to_owned()));
    assert_eq!(window.file.settings.editor_path, None);
    // It is typed, and nothing else.
    window.set("composing", "editor_path", FormValue::Flag(true));
    assert_eq!(window.file.settings.editor_path, None);
}

#[test]
fn an_imported_theme_becomes_the_theme_in_force() {
    // While Quill is dark it is the dark theme that is changed.
    let mut window = with_a_theme_for_each();
    window.file.settings.theme_dark = "dark".to_owned();
    window
        .harness
        .act(Action::ThemeImported("terminal".to_owned()));
    assert_eq!(
        window.harness.account_requests.last(),
        Some(&AccountRequest::Settings(Some(Change::ThemeInForce {
            name: "terminal".to_owned(),
            dark: true
        })))
    );
    window.serve();
    assert_eq!(window.file.settings.theme_dark, "terminal");
    assert_eq!(window.file.settings.theme_light, "light");
    assert_eq!(window.harness.env.styles.theme_name(), "terminal");
    // While it is light, the light one.
    window.harness.act(Action::SystemAppearance { dark: false });
    window.harness.act(Action::ThemeImported("dark".to_owned()));
    window.serve();
    assert_eq!(window.file.settings.theme_light, "dark");
    assert_eq!(window.file.settings.theme_dark, "terminal");
    assert_eq!(window.harness.env.styles.theme_name(), "dark");

    // A file with one theme for both appearances has that one changed.
    let mut one = Window::new(CONFIG);
    one.open();
    one.harness.act(Action::ThemeImported("light".to_owned()));
    one.serve();
    assert_eq!(
        (
            one.file.settings.theme_light.as_str(),
            one.file.settings.theme_dark.as_str()
        ),
        ("light", "light")
    );
    assert_eq!(one.harness.env.styles.theme_name(), "light");

    // With the window closed, as when the chooser was opened from the
    // menu: the theme still takes effect, and one that turns out not to
    // be a theme is said in the bottom bar, there being no window to say
    // it in.
    window.harness.act(Action::Settings(SettingsAct::Closed));
    window
        .harness
        .act(Action::ThemeImported("light".to_owned()));
    window.serve();
    assert_eq!(window.harness.env.styles.theme_name(), "light");
    window.file.unwritable = Some("There is no theme called \"notes\".".to_owned());
    window
        .harness
        .act(Action::ThemeImported("notes".to_owned()));
    window.serve();
    assert_eq!(window.harness.env.styles.theme_name(), "light");
    assert!(
        window
            .harness
            .screen()
            .contains("There is no theme called \"notes\""),
        "{}",
        window.harness.screen()
    );
}

fn with_a_conversation() -> Harness {
    let mut harness = Harness::with_accounts(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let message = |name: &str, subject: &str, minutes: i64, follows: &[&str]| RemoteMessage {
        id: RemoteId(name.to_owned()),
        headers: Headers {
            message_id: Some(format!("<{name}@example.com>")),
            subject: subject.to_owned(),
            from: vec![Address::new("Kit Moss", "kit.moss@example.com")],
            to: Vec::new(),
            cc: Vec::new(),
            date: NOW + minutes * 60,
        },
        flags: Flags::default(),
        size: 1,
        has_attachments: false,
        thread: follows
            .iter()
            .map(|name| format!("<{name}@example.com>"))
            .collect(),
    };
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![
                    message("plan-1", "Plan", 1, &[]),
                    message("other", "Unrelated", 2, &[]),
                    message("plan-2", "Re: Plan", 3, &["plan-1"]),
                ],
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.keys("2 g g");
    harness
}

fn top(harness: &Harness) -> Vec<String> {
    harness
        .state
        .list
        .rows
        .iter()
        .take(3)
        .map(|row| row.headers.subject.clone())
        .collect()
}

#[test]
fn conversations_are_listed_as_the_setting_says_from_the_moment_it_is_changed() {
    let together = ["Plan", "Re: Plan", "Unrelated"];
    let one_by_one = ["Re: Plan", "Unrelated", "Plan"];
    let mut window = Window::over(with_a_conversation(), CONFIG);
    window.open();
    assert_eq!(top(&window.harness), together);
    window.set("general", "threads", FormValue::Flag(false));
    assert_eq!(top(&window.harness), one_by_one);
    window.set("general", "threads", FormValue::Flag(true));
    assert_eq!(top(&window.harness), together);

    // Switched by hand for the run, the list is the other way and the
    // setting is as it was. Coming back to the window leaves that alone.
    window.harness.keys("T");
    assert_eq!(top(&window.harness), one_by_one);
    window.open();
    assert_eq!(top(&window.harness), one_by_one);
    assert!(matches!(
        window.field("general", "threads"),
        FormField::Toggle { value: true, .. }
    ));
    // Changing the setting is the last word: the list is as it says.
    window.set("general", "threads", FormValue::Flag(false));
    assert_eq!(top(&window.harness), one_by_one);
    window.set("general", "threads", FormValue::Flag(true));
    assert_eq!(top(&window.harness), together);
    // And so is a change made to the file by hand, when the window is
    // come back to.
    window.harness.keys("T");
    assert_eq!(top(&window.harness), one_by_one);
    window.file.settings.threads = false;
    window.open();
    assert_eq!(top(&window.harness), one_by_one);
    window.harness.keys("T");
    assert_eq!(top(&window.harness), together);
}

#[test]
fn web_reading_is_switched_from_the_window_as_its_command_switches_it() {
    let mut window = Window::over(Harness::in_the_app(200, 60, false), "");
    // The demo mailbox: no accounts to add, and a config all the same.
    window.harness.settle();
    window.open();
    let pages = |window: &Window| {
        window
            .harness
            .host_requests
            .iter()
            .filter(|request| matches!(request, HostRequest::Reading { .. }))
            .count()
    };
    let closed = |window: &Window| {
        window
            .harness
            .host_requests
            .iter()
            .filter(|request| matches!(request, HostRequest::CloseReading))
            .count()
    };
    assert!(!window.harness.state.web_reading);
    window.set("general", "web_reading", FormValue::Flag(true));
    // On, and the message under the cursor is in the pane, as with `w`.
    assert!(window.harness.state.web_reading && window.harness.env.web_reading);
    assert_eq!((pages(&window), closed(&window)), (1, 0));
    assert_eq!(
        window.harness.state.host_state(&window.harness.env).checked,
        [
            CommandId::ViewSidebar,
            CommandId::ViewWebReading,
            CommandId::ViewDarkPages,
            CommandId::RulesToggle
        ]
    );
    assert!(!window.harness.screen().contains("[3]"));

    // Switched off with its key: the file has that, and the window, which
    // is open, shows it.
    window.harness.keys("w");
    window.serve();
    assert!(!window.harness.state.web_reading);
    assert!(!window.file.settings.web_reading);
    assert!(matches!(
        window.field("general", "web_reading"),
        FormField::Toggle { value: false, .. }
    ));
    assert_eq!(closed(&window), 1);
    assert!(window.harness.screen().contains("[3] Reader"));

    // On with the key and off from the window: each undoes the other,
    // whichever was used last.
    window.harness.keys("w");
    window.serve();
    assert!(window.harness.state.web_reading && window.file.settings.web_reading);
    window.set("general", "web_reading", FormValue::Flag(false));
    assert!(!window.harness.state.web_reading && !window.harness.env.web_reading);
    assert_eq!(closed(&window), 2);
    window.harness.keys("w");
    window.serve();
    assert!(window.harness.state.web_reading);
    assert!(matches!(
        window.field("general", "web_reading"),
        FormField::Toggle { value: true, .. }
    ));
}

#[test]
fn spelling_marks_go_when_checking_is_switched_off_in_the_window() {
    let mut harness = Harness::with_accounts(120, 40);
    harness.env.spell_check = true;
    let mut window = Window::over(harness, CONFIG);
    window.harness.keys("c");
    window
        .harness
        .act(Action::Paste("sam.lee@example.com, ".to_owned()));
    window.harness.keys("tab tab tab");
    window.harness.type_text("Lunhc tomorrow?");
    window.harness.act(Action::TypingPaused);
    let (id, text) = window
        .harness
        .host_requests
        .iter()
        .rev()
        .find_map(|request| match request {
            HostRequest::SpellCheck { id, text } => Some((*id, text.clone())),
            _ => None,
        })
        .expect("nothing was sent to check");
    let start = text.find("Lunhc").unwrap();
    window.harness.act(Action::Spelling {
        id,
        words: vec![(start, start + 5)],
    });
    let marks = |window: &Window| {
        window
            .harness
            .state
            .compose
            .as_ref()
            .unwrap()
            .body
            .misspellings()
            .len()
    };
    assert_eq!(marks(&window), 1);

    window.open();
    assert_eq!(marks(&window), 1);
    window.set("composing", "spell_check", FormValue::Flag(false));
    assert_eq!(marks(&window), 0);
    assert!(!window.harness.state.spell_check_due(&window.harness.env));
    // Switched on again, what is being written is checked afresh.
    window.set("composing", "spell_check", FormValue::Flag(true));
    assert!(window.harness.state.spell_check_due(&window.harness.env));
}

#[test]
fn an_account_is_added_and_signed_in_to_from_the_window_as_by_its_commands() {
    let mut window = Window::new(CONFIG);
    window.open();
    let (rows, can) = window.accounts();
    assert_eq!(
        rows,
        [
            (
                "Work".to_owned(),
                vec![
                    "sign_in".to_owned(),
                    "letter".to_owned(),
                    "remove".to_owned()
                ]
            ),
            (
                "Home".to_owned(),
                vec!["edit".to_owned(), "letter".to_owned(), "remove".to_owned()]
            )
        ]
    );
    assert_eq!(can, ["add", "sign_in", "edit", "letter", "remove"]);
    // And an account picked can be taken away, which is asked about
    // first: `remove.rs` has that.
    match window.field("accounts", "accounts") {
        FormField::List { actions, .. } => {
            let ids: Vec<&str> = actions.iter().map(|action| action.id.as_str()).collect();
            assert_eq!(ids, ["add", "sign_in", "edit", "letter", "remove"]);
        }
        other => panic!("{other:?}"),
    }

    // Sign In, of the account picked: that account and no other is asked
    // for, and the sign-in is the question in the host's sheet.
    let asked = window.harness.account_requests.len();
    let home = window.row("Home");
    window.press("sign_in", Some(&home));
    window.press("sign_in", Some("account_0000000000000000"));
    window.press("sign_in", None);
    window.press("delete", Some(&home));
    assert_eq!(window.harness.account_requests.len(), asked);
    assert!(window.harness.state.sign_in.is_none());
    let work = window.row("Work");
    window.press("sign_in", Some(&work));
    assert_eq!(
        window.harness.account_requests.last(),
        Some(&AccountRequest::SignIn {
            account: Some("Work".to_owned())
        })
    );
    assert_eq!(window.harness.state.context(), Context::SignIn);
    let sheet = window.harness.state.form(&window.harness.env).unwrap();
    assert_eq!(sheet.step(), "sign_in");
    // While it is under way there is no second one to start, and no
    // account to add: the sheet has a question in it.
    assert_eq!(window.accounts().1, Vec::<String>::new());
    let asked = window.harness.account_requests.len();
    window.press("sign_in", Some(&work));
    assert_eq!(window.harness.account_requests.len(), asked);
    // Over, both are back.
    window.harness.act(Action::Account(AccountEvent::SignedIn {
        account: "Work".to_owned(),
        result: Err("nobody signed in".to_owned()),
    }));
    assert_eq!(
        window.accounts().1,
        ["add", "sign_in", "edit", "letter", "remove"]
    );

    // Add an Account: the form that adds one, in the host's sheet.
    assert!(window.harness.state.setup.is_none());
    window.press("add", None);
    assert_eq!(window.harness.state.context(), Context::Setup);
    let sheet = window.harness.state.form(&window.harness.env).unwrap();
    assert_eq!(sheet.step(), "address");
    assert_eq!(sheet.title(), "Add an Account");

    // The account it adds is one the config then has, so the window is
    // told the accounts afresh.
    let asked = window.harness.account_requests.len();
    window
        .harness
        .act(Action::Account(AccountEvent::Saved(Ok("Other".to_owned()))));
    assert_eq!(
        window.harness.account_requests[asked..],
        [AccountRequest::Settings(None)]
    );
    window.file.accounts.push(AccountLine {
        name: "Other".to_owned(),
        kind: AccountType::Imap,
        address: "robin@example.net".to_owned(),
        signs_in: false,
        letter: None,
    });
    window.serve();
    assert_eq!(window.accounts().0.len(), 3);

    // The demo mailbox has no accounts to add or sign in to, and says so.
    let mut window = Window::over(Harness::demo(200, 60), CONFIG);
    window.open();
    assert_eq!(window.accounts().1, Vec::<String>::new());
    let shown = window.shown();
    let pane = shown
        .panes()
        .iter()
        .find(|pane| pane.id == "accounts")
        .unwrap();
    assert!(
        pane.note
            .as_deref()
            .is_some_and(|note| note.starts_with("This is the demo mailbox.")),
        "{:?}",
        pane.note
    );
    let work = window.row("Work");
    window.press("add", None);
    window.press("sign_in", Some(&work));
    assert!(window.harness.state.setup.is_none() && window.harness.state.sign_in.is_none());
}

#[test]
fn a_file_with_a_mistake_in_it_shows_no_settings_and_changes_nothing() {
    let mut window = Window::new(CONFIG);
    window.file.mistake = Some("config line 3: expected a value".to_owned());
    window.open();
    let shown = window.shown();
    assert!(shown.panes().is_empty());
    assert_eq!(
        shown.problem(),
        Some(
            "The settings file has a mistake in it, so nothing can be changed here until it \
             is put right. Config line 3: expected a value."
        )
    );
    // Whatever the window sends, nothing is asked of the service.
    let asked = window.harness.account_requests.len();
    window.set("general", "notifications", FormValue::Flag(false));
    window.press("add", None);
    assert_eq!(window.harness.account_requests.len(), asked);
    assert!(window.harness.state.setup.is_none());
    assert!(window.harness.env.notifications);

    // A file that goes wrong while the window is open: the change is
    // refused by the service, and the window then shows the mistake.
    let mut window = Window::new(CONFIG);
    window.open();
    window.file.mistake = Some("config line 9: unknown field `themee`".to_owned());
    window.set("general", "notifications", FormValue::Flag(false));
    assert!(window.shown().panes().is_empty());
    assert!(window.harness.env.notifications);
    // Put right, and the window come back to.
    window.file.mistake = None;
    window.open();
    assert_eq!(window.shown().panes().len(), 8);
    assert_eq!(window.shown().problem(), None);
}

#[test]
fn a_file_that_says_what_the_window_does_not_offer_is_shown_as_it_is() {
    // An allowlist of senders is read from the file, and a theme that
    // does not exist can be named in it. Each is shown as the choice made,
    // since it is, and is not one to change to.
    let mut window = Window::new("[privacy]\nremote_images = \"allowlist\"\n");
    window.file.settings.theme_dark = "Solarized".to_owned();
    window.file.settings.remote_images = RemoteImages::Allowlist;
    let read = SettingsRead {
        theme_dark: Theme::builtin("dark").unwrap(),
        theme_dark_problem: Some(
            "There is no theme called \"Solarized\". Using the dark theme.".to_owned(),
        ),
        ..*File::of("").read().unwrap()
    };
    let read = SettingsRead {
        settings: window.file.settings.clone(),
        ..read
    };
    window.harness.act(Action::Settings(SettingsAct::Opened));
    window.answered = window.harness.account_requests.len();
    window.harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(Box::new(read)),
        changed: None,
    }));
    // The light theme is one there is, and says nothing.
    assert_eq!(window.field("appearance", "theme_light").help(), None);
    match window.field("appearance", "theme_dark") {
        FormField::Choice {
            value,
            options,
            help,
            ..
        } => {
            let chosen = options.iter().find(|option| option.id == value).unwrap();
            assert_eq!(chosen.title, "Solarized (not found)");
            assert_eq!(options.len(), 4);
            assert_eq!(
                help.as_deref(),
                Some("There is no theme called \"Solarized\". Using the dark theme.")
            );
        }
        other => panic!("{other:?}"),
    }
    match window.field("privacy", "remote_images") {
        FormField::Choice { value, options, .. } => {
            assert_eq!(value, "allowlist");
            let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
            assert_eq!(ids, ["ask", "never", "allowlist"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(window.harness.env.styles.theme_name(), "dark");
    assert_eq!(KeymapPreset::ALL.len(), 2);
}

fn flag(window: &Window, field: &str) -> bool {
    match window.field("agents", field) {
        FormField::Toggle { value, .. } => value,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_agents_pane_has_the_ways_in_what_they_may_do_and_how_sending_goes() {
    let mut window = Window::new(CONFIG);
    window.open();
    // As Quill comes: shell commands on, MCP off, sending off.
    assert!(flag(&window, "commands"));
    assert!(!flag(&window, "mcp"));
    assert!(flag(&window, "organize") && flag(&window, "delete") && flag(&window, "draft"));
    assert!(!flag(&window, "send"));
    // The command an agent's program is given is said beside MCP.
    match window.field("agents", "mcp") {
        FormField::Toggle { help, .. } => assert!(help.unwrap().ends_with("quill mcp")),
        other => panic!("{other:?}"),
    }
    // How sending goes is decided when it is switched on, and is not in
    // the way before.
    assert!(window.shown().field("agents", "send_gap").is_none());
    assert!(window.shown().field("agents", "hold_sends").is_none());

    window.set("agents", "send", FormValue::Flag(true));
    assert!(window.harness.env.agent.send, "it holds at once");
    match window.field("agents", "send_gap") {
        FormField::Number {
            value, min, max, ..
        } => {
            assert_eq!(value, Number::from(30));
            assert_eq!(min, Number::from(0));
            assert_eq!(max, Some(Number::from(86_400)));
        }
        other => panic!("{other:?}"),
    }
    assert!(!flag(&window, "hold_sends"));

    window.set("agents", "send_gap", FormValue::Number(Number::from(90)));
    assert_eq!(window.harness.env.agent.send_gap, 90);
    window.set("agents", "send_gap", FormValue::Number(Number::from(0)));
    assert_eq!(window.harness.env.agent.send_gap, 0);
    // More than a day is not a wait.
    window.set(
        "agents",
        "send_gap",
        FormValue::Number(Number::from(100_000)),
    );
    assert!(window.shown().problem().is_some());
    assert_eq!(window.harness.env.agent.send_gap, 0);

    window.set("agents", "hold_sends", FormValue::Flag(true));
    assert!(window.harness.env.agent.hold_sends);
    window.choose("agents", "notify", "sends");
    assert_eq!(
        window.harness.env.agent.notify,
        quill_core::config::AgentNotify::Sends
    );
    window.choose("agents", "notify", "loudly");
    assert!(window.shown().problem().is_some());
    window.set("agents", "mcp", FormValue::Flag(true));
    assert!(window.harness.env.agent.mcp);
    window.set("agents", "delete", FormValue::Flag(false));
    assert!(!window.harness.env.agent.delete);
    // And each is what the file is asked to say.
    assert!(window.file.settings.agent.mcp && !window.file.settings.agent.delete);
    assert!(window.file.settings.agent.send && window.file.settings.agent.hold_sends);
}

#[test]
fn stop_agents_switches_both_ways_in_off_at_once_and_in_the_file() {
    let mut window = Window::new(CONFIG);
    window.open();
    window.set("agents", "mcp", FormValue::Flag(true));
    let stop = |window: &Window| match window.field("agents", "recent") {
        FormField::List { actions, .. } => {
            actions
                .into_iter()
                .find(|action| action.id == "stop")
                .unwrap()
                .enabled
        }
        other => panic!("{other:?}"),
    };
    assert!(stop(&window));

    let used = SettingsUsed::action("agents", "recent", "stop", None).unwrap();
    window
        .harness
        .act(Action::Settings(SettingsAct::Used(used)));
    // From this moment, before the file has been written.
    assert!(!window.harness.env.agent.commands && !window.harness.env.agent.mcp);
    assert_eq!(window.harness.hung_up, 1);
    assert!(
        window
            .harness
            .account_requests
            .contains(&AccountRequest::Remember(Preference::AgentsOff))
    );
    window.serve();
    assert!(!flag(&window, "commands") && !flag(&window, "mcp"));
    assert!(!stop(&window));
    assert!(window.harness.screen().contains("Agents are stopped"));
    // Stopped once: the command has nothing left to do.
    assert!(
        !window
            .harness
            .state
            .is_enabled(&window.harness.env, CommandId::AgentsStop)
    );
    // They are let back in here, and nowhere else.
    window.set("agents", "commands", FormValue::Flag(true));
    assert!(window.harness.env.agent.commands);
    assert!(stop(&window));
}

#[test]
fn the_last_things_agents_changed_are_listed_in_the_pane() {
    let mut window = Window::new(CONFIG);
    let settings = quill_tui::agent::Settings::new(
        window.harness.env.agent.clone(),
        window.harness.env.tz.clone(),
    );
    let call = quill_tui::agent::Call::Archive {
        which: quill_tui::agent::Which {
            query: Some("from:peter dinner".to_owned()),
            ..Default::default()
        },
    };
    let outcome = quill_tui::agent::perform(
        &mut window.harness.store,
        &settings,
        &quill_tui::agent::Caller::shell(),
        &call,
        1_794_648_600,
    );
    assert!(outcome.result.is_ok());
    window.open();
    match window.field("agents", "recent") {
        FormField::List { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].title,
                "archived 1 message (where from:peter dinner)"
            );
            assert!(
                rows[0]
                    .detail
                    .as_deref()
                    .unwrap()
                    .starts_with("A shell command, ")
            );
        }
        other => panic!("{other:?}"),
    }
}

fn notifications(harness: &mut Harness) -> Vec<(String, String, String)> {
    std::mem::take(&mut harness.host_requests)
        .into_iter()
        .filter_map(|request| match request {
            HostRequest::Notify {
                id, title, body, ..
            } => Some((id, title, body)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_user_is_told_what_agents_do_as_far_as_they_asked_and_always_of_a_connection() {
    use quill_core::config::AgentNotify;
    use quill_tui::agent::Notice as Told;
    let mut harness = Harness::in_the_app(200, 60, false);
    let changed = || Told {
        text: "Claude Code: archived 12 messages.".to_owned(),
        send: false,
    };
    let sent = || Told {
        text: "Claude Code: sent a message to ana.ruiz@example.com.".to_owned(),
        send: true,
    };
    harness.host_requests.clear();

    // Every change, as Quill comes: gathered under one name, so that
    // forty changes are one notification and not forty.
    harness.act(Action::AgentNotice(changed()));
    harness.act(Action::AgentNotice(changed()));
    let told = notifications(&mut harness);
    assert_eq!(told.len(), 2);
    assert_eq!(told[0].0, told[1].0);
    assert_eq!(told[0].1, "An agent changed your mail");
    assert_eq!(told[0].2, "Claude Code: archived 12 messages.");
    // A message sent is always said by itself.
    harness.act(Action::AgentNotice(sent()));
    harness.act(Action::AgentNotice(sent()));
    let told = notifications(&mut harness);
    assert_eq!(told.len(), 2);
    assert_ne!(told[0].0, told[1].0);
    assert_eq!(told[0].1, "An agent sent mail");

    harness.env.agent.notify = AgentNotify::Sends;
    harness.act(Action::AgentNotice(changed()));
    assert!(notifications(&mut harness).is_empty());
    harness.act(Action::AgentNotice(sent()));
    assert_eq!(notifications(&mut harness).len(), 1);

    harness.env.agent.notify = AgentNotify::None;
    harness.act(Action::AgentNotice(sent()));
    assert!(notifications(&mut harness).is_empty());

    // A connection is said whatever the setting, and in the window too.
    harness.env.agent.mcp = true;
    harness.act(Action::AgentConnected("Claude Code".to_owned()));
    let told = notifications(&mut harness);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].1, "An agent connected");
    assert_eq!(told[0].2, "Claude Code connected to Quill over MCP.");
    assert!(
        harness
            .screen()
            .contains("Claude Code connected to Quill over MCP.")
    );
    // With MCP off, that it tried is said.
    harness.env.agent.mcp = false;
    harness.act(Action::AgentConnected("Claude Code".to_owned()));
    let told = notifications(&mut harness);
    assert!(told[0].2.contains("tried to connect"), "{}", told[0].2);

    // The button on a notification stops them.
    harness.env.agent.mcp = true;
    harness.act(Action::Notification {
        id: "agent.changes".to_owned(),
        action: Some("stop".to_owned()),
    });
    assert!(!harness.env.agent.mcp && !harness.env.agent.commands);
    assert_eq!(harness.hung_up, 1);
}
