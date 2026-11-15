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

use crate::bridge::{CommandInfo, CoreToShell, NoParams, PanePreview, StateUpdate};
pub use crate::shell::ShellHost;
pub use crate::standalone::StandaloneHost;

/// The environment variable in which the shell passes its socket's path.
pub const SOCKET_VARIABLE: &str = "QUILL_SHELL_SOCKET";

/// Something the host wants of the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Run the command with this name: a menu item or toolbar button was used.
    RunCommand(String),
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

    /// Shows a file in Quick Look.
    fn preview(&self, path: &Path, title: &str) {
        self.send(CoreToShell::PanePreview(PanePreview {
            path: path.to_string_lossy().into_owned(),
            title: title.to_owned(),
        }));
    }

    fn close_pane(&self) {
        self.send(CoreToShell::PaneClose(NoParams {}));
    }

    /// Opens a web or mail address in the user's browser or mail program.
    /// An address Quill does not open is ignored.
    fn open_url(&self, url: &str) {
        if let Some(url) = bridge::OpenableUrl::new(url) {
            self.send(CoreToShell::LinkOpen(bridge::LinkOpen { url }));
        }
    }
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
