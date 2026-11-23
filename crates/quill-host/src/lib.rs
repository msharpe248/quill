//! What hosts the core.
//!
//! Inside Quill.app the host is the Swift shell, reached over the bridge
//! ([`ShellHost`]). Run by hand in a terminal there is no shell, and
//! [`StandaloneHost`] stands in with what the system offers. The UI talks to
//! the [`Host`] trait and does not know which it has.
//!
//! macOS-specific behaviour in the core belongs in this crate.

pub mod bridge;
mod shell;
mod standalone;

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::bridge::{
    AbsolutePath, CommandInfo, CoreToShell, NoParams, PaneFile, PanePreview, StateUpdate,
};
pub use crate::shell::ShellHost;
pub use crate::standalone::StandaloneHost;

/// The environment variable in which the shell passes its socket's path.
pub const SOCKET_VARIABLE: &str = "QUILL_SHELL_SOCKET";

/// Something the host wants of the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Run the command with this name: a menu item or toolbar button was used.
    RunCommand(String),
    /// Files the user chose or dropped on the window. `temporary`: they
    /// are copies made for the core, to remove once read.
    Files { paths: Vec<String>, temporary: bool },
    /// The answer to [`Host::thumbnail`]: the picture is at the path asked
    /// for, or could not be made.
    Thumbnail { id: u64, ok: bool },
    /// The answer to [`Host::save_file`].
    Saved { name: String, saved: bool },
    /// The user clicked a notification (`action` is `None`) or one of its
    /// actions.
    NotificationUsed { id: String, action: Option<String> },
    /// The Mac asked Quill to open a `mailto:` address.
    OpenMailto(String),
    /// The answer to [`Host::spell_check`]: the misspelt words, each as
    /// the byte offsets of its start and end in the text that was sent,
    /// in order.
    Spelling { id: u64, words: Vec<(usize, usize)> },
    /// The answer to [`Host::spell_suggest`].
    Suggestions { id: u64, words: Vec<String> },
    /// The answer to [`Host::edit_draft`]: the editor ended with this
    /// status, or (`ran` false) could not be started.
    EditorExited { ran: bool, status: i32 },
    /// The user pressed a button of the form the host is showing
    /// ([`Host::show_form`]), or changed one of its choices.
    FormUsed(bridge::FormUsed),
    /// How many columns the terminal would have with the host's window
    /// to itself: what there is room for, whatever stands beside the
    /// terminal now.
    WindowColumns(u16),
    /// The user moved the divider beside the reading pane to where the
    /// terminal keeps this many columns; with none, they put it back
    /// where Quill has it by itself.
    ReadingDragged(Option<u16>),
    /// The user opened the host's settings window: it wants to be told
    /// what to show ([`Host::show_settings`]).
    SettingsOpened,
    /// The settings window was closed.
    SettingsClosed,
    /// The user changed a setting in the host's settings window, or asked
    /// for something from one of its lists.
    SettingsUsed(bridge::SettingsUsed),
    /// The Mac is in its dark appearance, or its light one: said once
    /// the host knows, and whenever it changes.
    SystemAppearance { dark: bool },
    /// The host has put a theme file of this name in the `themes` folder
    /// beside the settings file.
    ThemeImported(String),
    /// The app is quitting: flush and exit cleanly.
    WillQuit,
    /// The shell has gone. There is nothing left to draw for.
    Disconnected,
    /// Something went wrong that the user should see.
    Problem(String),
}

/// Where the core sends what only a host can do. Sending never blocks.
pub trait Host: Send + Sync {
    fn send(&self, message: CoreToShell);

    /// A shell is attached, with menus, a toolbar and native panes.
    fn is_shell(&self) -> bool;

    /// Announces every command, once at startup, in menu order.
    fn commands(&self, commands: Vec<CommandInfo>) {
        self.send(CoreToShell::CommandsList(bridge::CommandsList { commands }));
    }

    /// Reports the state the shell mirrors. Always the whole of it.
    fn state(&self, state: StateUpdate) {
        self.send(CoreToShell::StateUpdate(state));
    }

    /// Shows files in Quick Look, starting with the one at `index`. Each
    /// is a path and what to call it. Nothing is shown for a path that is
    /// not absolute or an index past the end.
    fn preview(&self, files: &[(&Path, &str)], index: usize) {
        let items: Option<Vec<PaneFile>> = files
            .iter()
            .map(|(path, title)| pane_file(path, title))
            .collect();
        if let Some(preview) = items.and_then(|items| PanePreview::new(items, index)) {
            self.send(CoreToShell::PanePreview(preview));
        }
    }

    /// Plays a video or audio file.
    fn play(&self, path: &Path, title: &str) {
        if let Some(file) = pane_file(path, title) {
            self.send(CoreToShell::PanePlay(file));
        }
    }

    /// Asks for a PNG picture of `path` to be written to `out`. The answer
    /// is [`HostEvent::Thumbnail`] with the same `id`, always: a host that
    /// cannot make one says so.
    fn thumbnail(&self, id: u64, path: &Path, out: &Path, size: u32) {
        match (AbsolutePath::new(path), AbsolutePath::new(out)) {
            (Some(path), Some(out)) => self.send(CoreToShell::ThumbnailMake(
                bridge::ThumbnailMake::new(id, path, out, size),
            )),
            _ => self.send_back(HostEvent::Thumbnail { id, ok: false }),
        }
    }

    /// Opens a file in the application the Mac opens its kind with.
    fn open_file(&self, path: &Path) {
        if let Some(path) = AbsolutePath::new(path) {
            self.send(CoreToShell::FileOpen(bridge::FileOpen { path }));
        }
    }

    /// Asks the user where to save a copy of `path`, offering `name`, if
    /// this host has a panel for that. Returns whether it has: when not,
    /// the caller asks for a place itself. The answer is
    /// [`HostEvent::Saved`].
    fn save_file(&self, path: &Path, name: &str) -> bool {
        let _ = (path, name);
        false
    }

    /// Shows a notification, where the host has them.
    fn notify(&self, notification: bridge::Notification) {
        self.send(CoreToShell::NotifyPost(notification));
    }

    /// Takes notifications down by their ids.
    fn clear_notifications(&self, ids: Vec<String>) {
        if let Some(clear) = bridge::NotifyClear::new(ids) {
            self.send(CoreToShell::NotifyClear(clear));
        }
    }

    /// Asks for the spelling of `text` to be checked, if this host has a
    /// checker. Returns whether it has (and the text was short enough to
    /// send): when not, there is no answer to wait for. The answer is
    /// [`HostEvent::Spelling`] with the same `id`.
    fn spell_check(&self, id: u64, text: &str) -> bool {
        let _ = (id, text);
        false
    }

    /// Asks what `word` might have been meant to be, on the same terms.
    /// The answer is [`HostEvent::Suggestions`].
    fn spell_suggest(&self, id: u64, word: &str) -> bool {
        let _ = (id, word);
        false
    }

    /// Runs an editor on a draft, in a terminal of the host's own, if it
    /// has one. `argv` is the whole command, the program by its absolute
    /// path first; `dir` is the draft's folder, where the editor and the
    /// core leave each other notes; `editor` says what kind it is, which
    /// is how the host asks it to leave. Returns whether the host took it
    /// on: when not, the caller says the editor did not run. The answer
    /// is [`HostEvent::EditorExited`].
    fn edit_draft(
        &self,
        dir: &Path,
        title: &str,
        editor: bridge::EditorKind,
        argv: Vec<String>,
    ) -> bool {
        let _ = (dir, title, editor, argv);
        false
    }

    /// Shows a form in a sheet of the host's own, in place of whatever
    /// form it shows now. What the user does with it arrives as
    /// [`HostEvent::FormUsed`]. Only a shell has a sheet: a caller asks
    /// this of a host that said it was one.
    fn show_form(&self, form: bridge::FormShow) {
        self.send(CoreToShell::FormShow(form));
    }

    /// Takes the host's form down.
    fn close_form(&self) {
        self.send(CoreToShell::FormClose(NoParams {}));
    }

    /// Shows a sanitized HTML file in the reading pane, the web view that
    /// stands in the reader's place while web reading is on, with `header`
    /// as lines of plain text above it. Only a shell has one: a caller
    /// asks this of a host that said it was one. Nothing is shown for a
    /// path that is not absolute or a header the pane has no room for.
    /// `notice` is what to tell the user about the page, in a bar above
    /// it, with buttons that run commands.
    fn reading_show(
        &self,
        path: &Path,
        title: &str,
        remote: bool,
        header: Vec<String>,
        notice: Option<bridge::ReadingNotice>,
    ) {
        if let Some(show) = AbsolutePath::new(path)
            .and_then(|path| bridge::ReadingShow::new(path, title, remote, reading_header(header)))
        {
            self.send(CoreToShell::ReadingShow(show.with_notice(notice)));
        }
    }

    /// Scrolls what the reading pane shows.
    fn reading_scroll(&self, by: bridge::ScrollBy) {
        self.send(CoreToShell::ReadingScroll(bridge::ReadingScroll { by }));
    }

    /// Takes the reading pane away.
    fn reading_close(&self) {
        self.send(CoreToShell::ReadingClose(NoParams {}));
    }

    /// Says where the reading pane stands: the terminal keeps `columns`
    /// columns, and the pane has the rest of the window, which has room
    /// for `window` columns in all.
    fn reading_place(&self, columns: u16, window: u16) {
        if let Some(place) = bridge::ReadingPlace::new(columns, window) {
            self.send(CoreToShell::ReadingPlace(place));
        }
    }

    /// Says where the editor that has a message's text goes on the
    /// terminal's screen, and whether it has the keys.
    fn editor_place(&self, place: bridge::EditorPlace) {
        self.send(CoreToShell::EditorPlace(place));
    }

    /// Tells the host what its settings window shows: every pane, with
    /// its settings as they stand. What the user does there arrives as
    /// [`HostEvent::SettingsUsed`]. Only a shell has such a window: in a
    /// terminal the settings are the file.
    fn show_settings(&self, settings: bridge::SettingsShow) {
        self.send(CoreToShell::SettingsShow(settings));
    }

    /// Hands an event straight back to the core, for a host that can
    /// answer a request without anyone else's help.
    fn send_back(&self, event: HostEvent);

    fn close_pane(&self) {
        self.send(CoreToShell::PaneClose(NoParams {}));
    }

    /// Asks the user to choose files with the native open panel. Their
    /// choice arrives as [`HostEvent::Files`]. Only a shell has a panel.
    fn pick_files(&self) {
        self.send(CoreToShell::FilesPick(NoParams {}));
    }

    /// Asks the user to choose a theme to import, with the host's own
    /// chooser. One that is imported arrives as
    /// [`HostEvent::ThemeImported`]. Only a shell has a chooser.
    fn pick_theme(&self) {
        self.send(CoreToShell::ThemesPick(NoParams {}));
    }

    /// Tells the host every key there is, for its window of them: at
    /// startup, and whenever the keys change.
    fn keys(&self, keys: bridge::KeysList) {
        self.send(CoreToShell::KeysList(keys));
    }

    /// Brings up the host's window of keys. Only a shell has one: in a
    /// terminal the list is drawn over the panes.
    fn show_keys(&self) {
        self.send(CoreToShell::KeysShow(NoParams {}));
    }

    /// Opens the file the user's rules are written in, where the host
    /// opens files: a shell does, by the settings folder it knows. No
    /// path is given: a host opens nothing the core names outside the
    /// folder of files the core wrote out for viewing.
    fn edit_rules(&self) {
        self.send(CoreToShell::RulesEdit(NoParams {}));
    }

    /// Closes the host's window and leaves Quill running behind it. Only
    /// a shell has one: in a terminal closing Quill is quitting it, and
    /// this is never asked.
    fn close_window(&self) {
        self.send(CoreToShell::WindowClose(NoParams {}));
    }

    /// Puts text on the system clipboard, if this host can. Returns
    /// whether it took it; a terminal is asked instead when it did not.
    fn copy_text(&self, text: &str) -> bool {
        let _ = text;
        false
    }

    /// Opens a web or mail address in the user's browser or mail program.
    /// An address Quill does not open is ignored.
    fn open_url(&self, url: &str) {
        if let Some(url) = bridge::OpenableUrl::new(url) {
            self.send(CoreToShell::LinkOpen(bridge::LinkOpen { url }));
        }
    }
}

/// The lines for above a page in the reading pane, made to fit what the
/// bridge carries: no line empty, too long or with a control character in
/// it, and no more lines than there is room for. A page is never kept
/// back for what is said above it.
fn reading_header(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            let mut line: String = line.chars().filter(|c| !c.is_control()).collect();
            let mut end = line.len().min(bridge::MAX_READING_HEADER_BYTES);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line
        })
        .filter(|line| !line.is_empty())
        .take(bridge::MAX_READING_HEADER_LINES)
        .collect()
}

fn pane_file(path: &Path, title: &str) -> Option<PaneFile> {
    Some(PaneFile {
        path: AbsolutePath::new(path)?,
        title: title.to_owned(),
    })
}

/// The host for this run, the events it will send, and a problem to show the
/// user if a shell was promised but could not be reached.
pub struct Attached {
    pub host: Arc<dyn Host>,
    pub events: UnboundedReceiver<HostEvent>,
    pub problem: Option<String>,
}

/// Connects to the shell when `socket` (the value of [`SOCKET_VARIABLE`]) is
/// set, and otherwise runs standalone. A shell that cannot be reached is not
/// fatal: the core runs standalone and says why.
pub fn attach(socket: Option<&OsStr>) -> Attached {
    let (events_tx, events) = unbounded_channel();
    let Some(socket) = socket.filter(|value| !value.is_empty()) else {
        return Attached {
            host: Arc::new(StandaloneHost::new(events_tx)),
            events,
            problem: None,
        };
    };
    match ShellHost::connect(Path::new(socket), events_tx.clone()) {
        Ok(host) => Attached {
            host: Arc::new(host),
            events,
            problem: None,
        },
        Err(err) => Attached {
            host: Arc::new(StandaloneHost::new(events_tx)),
            events,
            problem: Some(format!(
                "Cannot reach the app shell ({err}). Menus and panes are unavailable."
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    #[test]
    fn with_no_socket_the_core_runs_standalone() {
        for socket in [None, Some(OsStr::new(""))] {
            let attached = attach(socket);
            assert!(!attached.host.is_shell());
            assert_eq!(attached.problem, None);
        }
    }

    #[test]
    fn a_listening_socket_gives_a_shell_host() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("bridge.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let attached = attach(Some(socket.as_os_str()));
        assert!(attached.host.is_shell());
        assert_eq!(attached.problem, None);
    }

    #[test]
    fn an_unreachable_shell_falls_back_to_standalone_with_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let attached = attach(Some(socket.as_os_str()));
        assert!(!attached.host.is_shell());
        let problem = attached.problem.unwrap();
        assert!(
            problem.starts_with("Cannot reach the app shell"),
            "{problem}"
        );
    }

    #[tokio::test]
    async fn a_standalone_host_never_sends_events_and_never_closes_the_channel() {
        let mut attached = attach(None);
        attached.host.close_pane();
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(50), attached.events.recv())
                .await;
        assert!(waited.is_err(), "expected to keep waiting, got {waited:?}");
    }
}
