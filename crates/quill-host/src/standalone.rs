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
    /// The program that opens an address in the user's browser, and a
    /// file in whatever the user's Mac opens its kind with.
    opener: PathBuf,
    /// The program that shows a file in Quick Look.
    previewer: PathBuf,
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
            previewer: PathBuf::from("/usr/bin/qlmanage"),
        }
    }

    /// As [`StandaloneHost::with_opener`], with a stand-in for Quick Look
    /// too.
    pub fn with_programs(
        events: UnboundedSender<HostEvent>,
        opener: impl Into<PathBuf>,
        previewer: impl Into<PathBuf>,
    ) -> Self {
        Self {
            events,
            opener: opener.into(),
            previewer: previewer.into(),
        }
    }
}

impl StandaloneHost {
    /// Starts `program` on `arguments`, off the UI task, and reports it if
    /// that goes wrong. Each argument is passed as itself, never through a
    /// shell.
    fn run(&self, program: PathBuf, arguments: Vec<String>, what: &'static str) {
        let events = self.events.clone();
        let spawned = thread::Builder::new()
            .name("quill-open".to_owned())
            .spawn(move || {
                let result = Command::new(program)
                    .args(arguments)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let problem = match result {
                    Ok(status) if status.success() => return,
                    Ok(_) => format!("The system could not {what}."),
                    Err(err) => format!("Cannot {what}: {err}"),
                };
                let _ = events.send(HostEvent::Problem(problem));
            });
        if let Err(err) = spawned {
            let _ = self
                .events
                .send(HostEvent::Problem(format!("Cannot {what}: {err}")));
        }
    }
}

impl Host for StandaloneHost {
    fn send(&self, message: CoreToShell) {
        match message {
            // There are no menus, toolbar or Dock badge to keep up to date,
            // and no pane to close: `qlmanage` owns its own window. Nor a
            // file panel: the UI asks for a path instead. The clipboard is
            // the terminal's, and is asked for there. Nor notifications:
            // a terminal has no place for them that every terminal shares.
            // Nor a spelling checker: these are never sent here, since
            // this host says it has none.
            // Nor an editor pane: in a terminal the editor is run in the
            // terminal. Nor a sheet for a form: the form is drawn in the
            // terminal, and these are never sent here.
            CoreToShell::PaneEditor(_)
            | CoreToShell::FormShow(_)
            | CoreToShell::FormClose(_)
            // There is no reading pane to show a page in, scroll, place
            // or close: web reading is not on offer here.
            | CoreToShell::ReadingShow(_)
            | CoreToShell::ReadingScroll(_)
            | CoreToShell::ReadingClose(_)
            | CoreToShell::ReadingPlace(_)
            // Nor a terminal of its own to lay an editor over this one:
            // here an editor has the whole terminal.
            | CoreToShell::EditorPlace(_)
            // Nor a settings window: in a terminal the settings are the
            // file, and this is never sent here.
            | CoreToShell::SettingsShow(_)
            // Nor a chooser of themes: `quill theme import` is how one is
            // imported from a terminal.
            | CoreToShell::ThemesPick(_)
            // Nor a vimrc to take in: in a terminal the user's own Vim
            // reads the user's own.
            | CoreToShell::EditorSetup(_)
            // Nor a window of keys: the list is drawn over the panes.
            | CoreToShell::KeysList(_)
            | CoreToShell::KeysShow(_)
            | CoreToShell::SpellCheck(_)
            | CoreToShell::SpellSuggest(_)
            | CoreToShell::NotifyPost(_)
            | CoreToShell::NotifyClear(_)
            | CoreToShell::CommandsList(_)
            | CoreToShell::StateUpdate(_)
            | CoreToShell::PaneClose(_)
            | CoreToShell::FilesPick(_)
            | CoreToShell::FilesSave(_)
            | CoreToShell::ClipboardWrite(_) => {}
            // `open` hands the address to the default browser or mail
            // program. It can only be http, https or mailto.
            CoreToShell::LinkOpen(open) => self.run(
                self.opener.clone(),
                vec![open.url.as_str().to_owned()],
                "open that link",
            ),
            // Quick Look in a window of its own, on the file asked for
            // first. There is no stepping through the others.
            CoreToShell::PanePreview(preview) => {
                if let Some(file) = preview.items().get(preview.index()) {
                    self.run(
                        self.previewer.clone(),
                        vec!["-p".to_owned(), file.path.as_str().to_owned()],
                        "preview that file",
                    );
                }
            }
            // With no pane to play in, the program the user's Mac has for
            // the file does it.
            CoreToShell::PanePlay(file) => self.run(
                self.opener.clone(),
                vec![file.path.as_str().to_owned()],
                "play that file",
            ),
            CoreToShell::FileOpen(open) => self.run(
                self.opener.clone(),
                vec![open.path.as_str().to_owned()],
                "open that file",
            ),
            // Nothing here makes thumbnails. The answer is still owed.
            CoreToShell::ThumbnailMake(make) => self.send_back(HostEvent::Thumbnail {
                id: make.id(),
                ok: false,
            }),
        }
    }

    fn send_back(&self, event: HostEvent) {
        let _ = self.events.send(event);
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
    /// `record`, all of them in one go: whoever waits for the record must
    /// never find it with only the first of two written.
    fn recorder(dir: &std::path::Path) -> (PathBuf, PathBuf) {
        let record = dir.join("opened.txt");
        let script = dir.join("opener.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n",
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
                    matches!(&event, HostEvent::Problem(text) if text.starts_with("Cannot open that link")),
                    "{event:?}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "no problem was reported");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn files_go_to_quick_look_and_to_the_systems_own_programs() {
        let dir = tempfile::tempdir().unwrap();
        let (script, record) = recorder(dir.path());
        let (events, mut received) = unbounded_channel();
        let host = StandaloneHost::with_programs(events, script.clone(), script);
        let pdf = dir.path().join("road map.pdf");
        let png = dir.path().join("chart.png");

        // Quick Look is given the one asked for first, as one argument
        // whatever is in its name.
        host.preview(
            &[
                (pdf.as_path(), "road map.pdf"),
                (png.as_path(), "chart.png"),
            ],
            1,
        );
        assert_eq!(wait_for(&record), format!("-p\n{}\n", png.display()));
        std::fs::remove_file(&record).unwrap();

        // Playing and opening a file, a message's page among them, both
        // fall to whatever the Mac opens that kind of file with.
        host.play(&pdf, "a film");
        assert_eq!(wait_for(&record), format!("{}\n", pdf.display()));
        std::fs::remove_file(&record).unwrap();
        host.open_file(&pdf);
        assert_eq!(wait_for(&record), format!("{}\n", pdf.display()));
        std::fs::remove_file(&record).unwrap();

        // A path that is not absolute, or an index past the end, goes
        // nowhere at all.
        host.open_file(std::path::Path::new("notes.txt"));
        host.preview(&[(pdf.as_path(), "road map.pdf")], 3);
        // There is no panel to save with, and nothing that makes
        // thumbnails: the first is said by the return value, the second by
        // an answer.
        assert!(!host.save_file(&pdf, "road map.pdf"));
        host.thumbnail(7, &pdf, &dir.path().join("thumb.png"), 320);
        assert_eq!(
            received.try_recv().ok(),
            Some(HostEvent::Thumbnail { id: 7, ok: false })
        );
        thread::sleep(Duration::from_millis(200));
        assert!(!record.exists(), "{:?}", std::fs::read_to_string(&record));
    }
}
