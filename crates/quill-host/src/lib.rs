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

    /// Shows an HTML file that has already been sanitized. `remote`: the
    /// user has allowed it to load remote content.
    fn web(&self, path: &Path, title: &str, remote: bool) {
        if let Some(path) = AbsolutePath::new(path) {
            self.send(CoreToShell::PaneWeb(bridge::PaneWeb {
                path,
                title: title.to_owned(),
                remote,
            }));
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
