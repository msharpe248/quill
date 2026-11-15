//! The host when the core runs by hand in a terminal, with no app shell.
//!
//! Standalone mode exists for development and tests. It is not sandboxed, so
//! it may hand files to the system's own tools.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::CoreToShell;
use crate::{Host, HostEvent};

pub struct StandaloneHost {
    /// Reports what goes wrong, and keeps the event channel open so the UI
    /// can wait on it forever.
    events: UnboundedSender<HostEvent>,
    /// The program that opens an address in the user's browser.
    opener: PathBuf,
}

impl StandaloneHost {
    pub fn new(events: UnboundedSender<HostEvent>) -> Self {
        Self::with_opener(events, "/usr/bin/open")
    }

    /// A host that opens addresses with `opener`, so a test can see what
    /// would be opened without opening it.
    pub fn with_opener(events: UnboundedSender<HostEvent>, opener: impl Into<PathBuf>) -> Self {
        Self {
            events,
            opener: opener.into(),
        }
    }
}

impl Host for StandaloneHost {
    fn send(&self, message: CoreToShell) {
        match message {
            // There are no menus, toolbar or Dock badge to keep up to date,
            // and no pane to close: `qlmanage` owns its own window.
            // And no file panel: the UI asks for a path instead. The
            // clipboard is the terminal's, and is asked for there.
            CoreToShell::CommandsList(_)
            | CoreToShell::StateUpdate(_)
            | CoreToShell::PaneClose(_)
            | CoreToShell::FilesPick(_)
            | CoreToShell::ClipboardWrite(_) => {}
            CoreToShell::LinkOpen(open) => {
                let events = self.events.clone();
                let opener = self.opener.clone();
                // `open` hands the address to the default browser or mail
                // program. The address is one argument, never a shell
                // command, and can only be http, https or mailto.
                let spawned =
                    thread::Builder::new()
                        .name("quill-open".to_owned())
                        .spawn(move || {
                            let result = Command::new(opener)
                                .arg(open.url.as_str())
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status();
                            match result {
                                Ok(status) if status.success() => {}
                                Ok(_) => {
                                    let _ = events.send(HostEvent::Problem(
                                        "The system could not open that link.".to_owned(),
                                    ));
                                }
                                Err(err) => {
                                    let _ = events.send(HostEvent::Problem(format!(
                                        "Cannot open the link: {err}"
                                    )));
                                }
                            }
                        });
                if let Err(err) = spawned {
                    let _ = self
                        .events
                        .send(HostEvent::Problem(format!("Cannot open the link: {err}")));
                }
            }
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    /// A stand-in for `open` that writes each of its arguments on a line of
    /// `record`.
    fn recorder(dir: &std::path::Path) -> (PathBuf, PathBuf) {
        let record = dir.join("opened.txt");
        let script = dir.join("opener.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        (script, record)
    }

    fn wait_for(record: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = std::fs::read_to_string(record)
                && text.ends_with('\n')
            {
                return text;
            }
            assert!(Instant::now() < deadline, "the opener was never run");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn an_address_reaches_the_opener_as_one_argument() {
        let dir = tempfile::tempdir().unwrap();
        let (script, record) = recorder(dir.path());
        let (events, _kept) = unbounded_channel();
        let host = StandaloneHost::with_opener(events, script);
        // Characters a shell would act on arrive untouched: no shell is involved.
        host.open_url("https://example.com/a?b=1&c=$(id);d=`id`#e");
        assert_eq!(
            wait_for(&record),
            "https://example.com/a?b=1&c=$(id);d=`id`#e\n"
        );
    }

    #[test]
    fn what_quill_does_not_open_never_reaches_the_opener() {
        let dir = tempfile::tempdir().unwrap();
        let (script, record) = recorder(dir.path());
        let (events, _kept) = unbounded_channel();
        let host = StandaloneHost::with_opener(events, script);
        for url in [
            "file:///Applications/Calculator.app",
            "-a Calculator",
            "x-evil://run",
            "https://example.com/a b",
            "",
        ] {
            host.open_url(url);
        }
        // An address that is opened comes after them and is the only one seen.
        host.open_url("mailto:ana.ruiz@example.com");
        assert_eq!(wait_for(&record), "mailto:ana.ruiz@example.com\n");
    }

    #[test]
    fn a_missing_opener_is_reported() {
        let (events, mut received) = unbounded_channel();
        let host = StandaloneHost::with_opener(events, "/nonexistent/quill-opener");
        host.open_url("https://example.com/");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(event) = received.try_recv() {
                assert!(
                    matches!(&event, HostEvent::Problem(text) if text.starts_with("Cannot open the link")),
                    "{event:?}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "no problem was reported");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
