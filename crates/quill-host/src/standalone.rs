//! The host when the core runs by hand in a terminal, with no app shell.
//!
//! Standalone mode exists for development and tests. It is not sandboxed, so
//! it may hand files to the system's own tools.

use std::process::{Command, Stdio};
use std::thread;

use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::CoreToShell;
use crate::{Host, HostEvent};

pub struct StandaloneHost {
    /// Reports what goes wrong, and keeps the event channel open so the UI
    /// can wait on it forever.
    events: UnboundedSender<HostEvent>,
}

impl StandaloneHost {
    pub fn new(events: UnboundedSender<HostEvent>) -> Self {
        Self { events }
    }
}

impl Host for StandaloneHost {
    fn send(&self, message: CoreToShell) {
        match message {
            // There are no menus, toolbar or Dock badge to keep up to date,
            // and no pane to close: `qlmanage` owns its own window.
            CoreToShell::CommandsList(_)
            | CoreToShell::StateUpdate(_)
            | CoreToShell::PaneClose(_) => {}
            CoreToShell::PanePreview(preview) => {
                let events = self.events.clone();
                // Starting a process is not for the UI task.
                let spawned = thread::Builder::new()
                    .name("quill-preview".to_owned())
                    .spawn(move || {
                        let result = Command::new("/usr/bin/qlmanage")
                            .arg("-p")
                            .arg(&preview.path)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                        if let Err(err) = result {
                            let _ = events.send(HostEvent::Problem(format!(
                                "Cannot preview {}: {err}",
                                preview.title
                            )));
                        }
                    });
                if let Err(err) = spawned {
                    let _ = self
                        .events
                        .send(HostEvent::Problem(format!("Cannot preview: {err}")));
                }
            }
        }
    }

    fn is_shell(&self) -> bool {
        false
    }
}
