//! The host when Quill.app is running the core: messages go over the bridge.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::{self, CoreToShell, ShellToCore};
use crate::{Host, HostEvent};

/// The app shell at the other end of a Unix socket.
///
/// Sending never blocks the caller: messages are queued for a writer thread.
/// A second thread reads what the shell sends and turns it into
/// [`HostEvent`]s.
pub struct ShellHost {
    outgoing: mpsc::Sender<CoreToShell>,
    events: UnboundedSender<HostEvent>,
}

impl ShellHost {
    /// Connects to the socket the shell is listening on.
    pub fn connect(socket: &Path, events: UnboundedSender<HostEvent>) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        let reader = stream.try_clone()?;
        let (outgoing, queue) = mpsc::channel::<CoreToShell>();

        thread::Builder::new()
            .name("quill-bridge-write".to_owned())
            .spawn(move || write_loop(stream, queue))?;
        let heard = events.clone();
        thread::Builder::new()
            .name("quill-bridge-read".to_owned())
            .spawn(move || read_loop(reader, heard))?;
        Ok(Self { outgoing, events })
    }
}

impl Host for ShellHost {
    fn send(&self, message: CoreToShell) {
        // If the writer has gone the shell has too, and the reader says so.
        let _ = self.outgoing.send(message);
    }

    fn is_shell(&self) -> bool {
        true
    }

    fn send_back(&self, event: HostEvent) {
        let _ = self.events.send(event);
    }

    fn edit_draft(
        &self,
        dir: &Path,
        title: &str,
        editor: bridge::EditorKind,
        argv: Vec<String>,
    ) -> bool {
        match (
            bridge::AbsolutePath::new(dir),
            bridge::EditorArgv::new(argv),
        ) {
            (Some(dir), Some(argv)) => {
                self.send(CoreToShell::PaneEditor(bridge::PaneEditor {
                    dir,
                    title: title.to_owned(),
                    editor,
                    argv,
                }));
                true
            }
            _ => false,
        }
    }

    fn spell_check(&self, id: u64, text: &str) -> bool {
        match bridge::SpellCheck::new(id, text) {
            Some(check) => {
                self.send(CoreToShell::SpellCheck(check));
                true
            }
            None => false,
        }
    }

    fn spell_suggest(&self, id: u64, word: &str) -> bool {
        match bridge::SpellSuggest::new(id, word) {
            Some(suggest) => {
                self.send(CoreToShell::SpellSuggest(suggest));
                true
            }
            None => false,
        }
    }

    fn save_file(&self, path: &Path, name: &str) -> bool {
        let save =
            bridge::AbsolutePath::new(path).and_then(|path| bridge::FilesSave::new(path, name));
        match save {
            Some(save) => self.send(CoreToShell::FilesSave(save)),
            // Nothing to ask about: said at once, as a save that failed.
            None => self.send_back(HostEvent::Saved {
                name: name.to_owned(),
                saved: false,
            }),
        }
        true
    }

    fn copy_text(&self, text: &str) -> bool {
        self.send(CoreToShell::ClipboardWrite(bridge::ClipboardWrite {
            text: text.to_owned(),
        }));
        true
    }
}

fn write_loop(mut stream: UnixStream, queue: mpsc::Receiver<CoreToShell>) {
    for message in queue {
        let Ok(mut line) = bridge::encode(&message) else {
            continue;
        };
        line.push('\n');
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
    }
}

fn read_loop(stream: UnixStream, events: UnboundedSender<HostEvent>) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let event = match bridge::decode::<ShellToCore>(&line) {
            Ok(ShellToCore::CommandRun(run)) => HostEvent::RunCommand(run.name),
            Ok(ShellToCore::FilesDropped(files)) => HostEvent::Files {
                paths: files.paths().to_vec(),
                temporary: files.temporary(),
            },
            Ok(ShellToCore::ThumbnailMade(made)) => HostEvent::Thumbnail {
                id: made.id,
                ok: made.ok,
            },
            Ok(ShellToCore::FilesSaved(saved)) => HostEvent::Saved {
                name: saved.name,
                saved: saved.saved,
            },
            Ok(ShellToCore::NotifyUsed(used)) => HostEvent::NotificationUsed {
                id: used.id().to_owned(),
                action: used.action().map(str::to_owned),
            },
            Ok(ShellToCore::UrlOpen(open)) => HostEvent::OpenMailto(open.url().to_owned()),
            Ok(ShellToCore::SpellChecked(checked)) => HostEvent::Spelling {
                id: checked.id(),
                words: checked
                    .words()
                    .iter()
                    .map(|word| (word.start, word.end))
                    .collect(),
            },
            Ok(ShellToCore::SpellSuggested(suggested)) => HostEvent::Suggestions {
                id: suggested.id(),
                words: suggested.words().to_vec(),
            },
            Ok(ShellToCore::EditorExited(exited)) => HostEvent::EditorExited {
                ran: exited.ran(),
                status: exited.status(),
            },
            Ok(ShellToCore::FormUsed(used)) => HostEvent::FormUsed(used),
            Ok(ShellToCore::WindowResized(resized)) => HostEvent::WindowColumns(resized.columns()),
            Ok(ShellToCore::SettingsOpened(_)) => HostEvent::SettingsOpened,
            Ok(ShellToCore::SettingsClosed(_)) => HostEvent::SettingsClosed,
            Ok(ShellToCore::SettingsUsed(used)) => HostEvent::SettingsUsed(used),
            Ok(ShellToCore::AppearanceChanged(changed)) => {
                HostEvent::SystemAppearance { dark: changed.dark }
            }
            Ok(ShellToCore::ThemesImported(imported)) => {
                HostEvent::ThemeImported(imported.name().to_owned())
            }
            Ok(ShellToCore::AppWillQuit(_)) => HostEvent::WillQuit,
            Err(err) => HostEvent::Problem(unreadable(&line, &err)),
        };
        if events.send(event).is_err() {
            return;
        }
    }
    let _ = events.send(HostEvent::Disconnected);
}

/// What to say of a line from the shell that is not a message.
///
/// A `form.used` may carry a password. Why one could not be read is not
/// said in the parser's words, which can quote what it was reading: the
/// problem is shown to the user, on screen.
fn unreadable(line: &str, err: &bridge::BridgeError) -> String {
    if line.contains("form.used") {
        "bridge: a form.used message could not be read".to_owned()
    } else {
        format!("bridge: {err}")
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::Duration;

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;
    use crate::bridge::{CommandInfo, CommandsList, Context, NoParams, StateUpdate};

    /// A scripted stand-in for the app shell: it listens, and the test plays
    /// the shell's part by hand on the accepted connection.
    struct FakeShell {
        _dir: tempfile::TempDir,
        connection: UnixStream,
        lines: std::io::Lines<BufReader<UnixStream>>,
    }

    fn connect() -> (ShellHost, UnboundedReceiver<HostEvent>, FakeShell) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("bridge.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (events_tx, events) = unbounded_channel();
        let host = ShellHost::connect(&socket, events_tx).unwrap();
        let (connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let lines = BufReader::new(connection.try_clone().unwrap()).lines();
        (
            host,
            events,
            FakeShell {
                _dir: dir,
                connection,
                lines,
            },
        )
    }

    async fn next(events: &mut UnboundedReceiver<HostEvent>) -> HostEvent {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for the shell")
            .expect("the event channel closed")
    }

    #[tokio::test]
    async fn what_the_core_sends_arrives_as_one_json_line_each_in_order() {
        let (host, _events, mut shell) = connect();
        host.commands(vec![CommandInfo {
            name: "view.zoom".to_owned(),
            title: "Zoom the focused pane".to_owned(),
            keys: vec!["z".to_owned()],
            menu: Some(crate::bridge::Menu::View),
            symbol: None,
        }]);
        host.state(StateUpdate {
            context: Context::List,
            enabled: vec!["view.zoom".to_owned()],
            checked: Vec::new(),
            unread: 12,
            title: "Inbox".to_owned(),
            background: None,
            foreground: None,
            appearance: bridge::Appearance::System,
        });
        host.close_pane();

        let first: CoreToShell = bridge::decode(&shell.lines.next().unwrap().unwrap()).unwrap();
        assert!(matches!(
            first,
            CoreToShell::CommandsList(CommandsList { ref commands }) if commands[0].name == "view.zoom"
        ));
        let second = shell.lines.next().unwrap().unwrap();
        assert_eq!(
            second,
            r#"{"jsonrpc":"2.0","method":"state.update","params":{"context":"list","enabled":["view.zoom"],"checked":[],"unread":12,"title":"Inbox"}}"#
        );
        let third: CoreToShell = bridge::decode(&shell.lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(third, CoreToShell::PaneClose(NoParams {}));
        assert!(host.is_shell());
    }

    #[tokio::test]
    async fn what_the_shell_sends_becomes_events() {
        let (_host, mut events, mut shell) = connect();
        shell
            .connection
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"command.run\",\"params\":{\"name\":\"view.zoom\"}}\n\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"window.resized\",\"params\":{\"columns\":147}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"appearance.changed\",\"params\":{\"dark\":true}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"themes.imported\",\"params\":{\"name\":\"Catppuccin Mocha\"}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"themes.imported\",\"params\":{\"name\":\"../config\"}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"app.will_quit\",\"params\":{}}\n",
            )
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            HostEvent::RunCommand("view.zoom".to_owned())
        );
        assert_eq!(next(&mut events).await, HostEvent::WindowColumns(147));
        assert_eq!(
            next(&mut events).await,
            HostEvent::SystemAppearance { dark: true }
        );
        assert_eq!(
            next(&mut events).await,
            HostEvent::ThemeImported("Catppuccin Mocha".to_owned())
        );
        // A name that is a path is no theme's, and names nothing here.
        assert!(matches!(next(&mut events).await, HostEvent::Problem(_)));
        assert_eq!(next(&mut events).await, HostEvent::WillQuit);
    }

    #[tokio::test]
    async fn a_line_that_is_not_a_message_is_reported_and_the_next_still_works() {
        let (_host, mut events, mut shell) = connect();
        shell
            .connection
            .write_all(
                b"this is not json\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"mail.delete\",\"params\":{}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"command.run\",\"params\":{\"name\":\"nav.next\"}}\n",
            )
            .unwrap();
        assert!(
            matches!(next(&mut events).await, HostEvent::Problem(text) if text.starts_with("bridge: "))
        );
        assert!(matches!(next(&mut events).await, HostEvent::Problem(_)));
        assert_eq!(
            next(&mut events).await,
            HostEvent::RunCommand("nav.next".to_owned())
        );
    }

    #[tokio::test]
    async fn files_are_shown_pictured_opened_and_saved_through_the_shell() {
        let (host, mut events, mut shell) = connect();
        let pdf = Path::new("/tmp/quill-view/road map.pdf");
        let png = Path::new("/tmp/quill-view/chart.png");
        let sent = |shell: &mut FakeShell| -> CoreToShell {
            bridge::decode(&shell.lines.next().unwrap().unwrap()).unwrap()
        };

        host.preview(&[(pdf, "road map.pdf"), (png, "chart.png")], 1);
        match sent(&mut shell) {
            CoreToShell::PanePreview(preview) => {
                assert_eq!(preview.index(), 1);
                assert_eq!(preview.items().len(), 2);
                assert_eq!(
                    preview.items()[0].path.as_str(),
                    "/tmp/quill-view/road map.pdf"
                );
                assert_eq!(preview.items()[1].title, "chart.png");
            }
            other => panic!("{other:?}"),
        }
        host.play(pdf, "a film");
        assert!(matches!(sent(&mut shell), CoreToShell::PanePlay(file) if file.title == "a film"));
        host.reading_show(png, "a message", false, vec!["From: Ana".to_owned()], None);
        assert!(matches!(
            sent(&mut shell),
            CoreToShell::ReadingShow(show)
                if !show.remote() && show.title() == "a message" && show.header() == ["From: Ana"]
        ));
        host.reading_scroll(crate::bridge::ScrollBy::PageDown);
        assert!(matches!(
            sent(&mut shell),
            CoreToShell::ReadingScroll(scroll) if scroll.by == crate::bridge::ScrollBy::PageDown
        ));
        host.reading_close();
        assert!(matches!(sent(&mut shell), CoreToShell::ReadingClose(_)));
        host.reading_place(78);
        assert!(matches!(
            sent(&mut shell),
            CoreToShell::ReadingPlace(place) if place.columns() == 78
        ));
        // No columns is no place, and is not said.
        host.reading_place(0);
        // What is said above the page is made to fit; the page is shown.
        let mut header = vec!["line\u{7}".to_owned(); 9];
        header.insert(0, String::new());
        header.insert(1, "é".repeat(1500));
        host.reading_show(png, "a message", false, header, None);
        match sent(&mut shell) {
            CoreToShell::ReadingShow(show) => {
                assert_eq!(show.header().len(), 8);
                assert_eq!(show.header()[0], "é".repeat(1000));
                assert_eq!(show.header()[1], "line");
            }
            other => panic!("{other:?}"),
        }
        // A page that is not at an absolute path is not.
        host.reading_show(Path::new("relative.html"), "", false, Vec::new(), None);
        host.open_file(pdf);
        assert!(matches!(sent(&mut shell), CoreToShell::FileOpen(_)));
        // The shell has a save panel, and is asked.
        assert!(host.save_file(pdf, "road map.pdf"));
        assert!(matches!(sent(&mut shell), CoreToShell::FilesSave(_)));
        // A thumbnail is asked for at a size the shell will accept.
        host.thumbnail(7, pdf, Path::new("/tmp/quill-view/thumb-7.png"), 1_000_000);
        match sent(&mut shell) {
            CoreToShell::ThumbnailMake(make) => assert_eq!(make.id(), 7),
            other => panic!("{other:?}"),
        }

        // What cannot be asked is not sent, and what is owed an answer
        // gets one from the host itself.
        host.open_file(Path::new("notes.txt"));
        host.preview(&[(pdf, "road map.pdf")], 1);
        host.thumbnail(8, Path::new("a.heic"), Path::new("/tmp/t.png"), 64);
        assert_eq!(
            next(&mut events).await,
            HostEvent::Thumbnail { id: 8, ok: false }
        );
        assert!(host.save_file(pdf, ""));
        assert_eq!(
            next(&mut events).await,
            HostEvent::Saved {
                name: String::new(),
                saved: false
            }
        );

        // The shell's answers arrive as events.
        shell
            .connection
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"thumbnail.made\",\"params\":{\"id\":7,\"ok\":true}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"files.saved\",\"params\":{\"name\":\"road map.pdf\",\"saved\":true}}\n",
            )
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            HostEvent::Thumbnail { id: 7, ok: true }
        );
        assert_eq!(
            next(&mut events).await,
            HostEvent::Saved {
                name: "road map.pdf".to_owned(),
                saved: true
            }
        );
        // Nothing else was sent to the shell.
        host.close_pane();
        assert!(matches!(sent(&mut shell), CoreToShell::PaneClose(_)));
    }

    #[tokio::test]
    async fn a_form_goes_to_the_shell_and_what_is_done_with_it_comes_back() {
        let (host, mut events, mut shell) = connect();
        let form = bridge::FormShow::new(bridge::FormShowParams {
            step: "password".to_owned(),
            title: "Add an account: sign in".to_owned(),
            fields: vec![bridge::FormField::Secret {
                id: "password".to_owned(),
                label: "Password:".to_owned(),
                focus: true,
                help: None,
            }],
            buttons: Vec::new(),
            ..bridge::FormShowParams::default()
        })
        .unwrap();
        host.show_form(form.clone());
        host.close_form();
        let first: CoreToShell = bridge::decode(&shell.lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(first, CoreToShell::FormShow(form));
        assert_eq!(
            shell.lines.next().unwrap().unwrap(),
            r#"{"jsonrpc":"2.0","method":"form.close","params":{}}"#
        );

        shell
            .connection
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"form.used\",\"params\":{\"step\":\"password\",\"button\":\"continue\",\"values\":{},\"secrets\":{\"password\":\"not-a-real-password\"}}}\n",
            )
            .unwrap();
        let event = next(&mut events).await;
        // The event prints without what it carries.
        assert!(!format!("{event:?}").contains("not-a-real-password"));
        let HostEvent::FormUsed(used) = event else {
            panic!("{event:?}");
        };
        assert_eq!(
            used.secret("password").map(bridge::Secret::reveal),
            Some("not-a-real-password")
        );
    }

    #[tokio::test]
    async fn the_settings_go_to_the_shell_and_what_is_done_there_comes_back() {
        let (host, mut events, mut shell) = connect();
        let settings = bridge::SettingsShow::new(bridge::SettingsShowParams {
            panes: vec![bridge::SettingsPane {
                id: "general".to_owned(),
                title: "General".to_owned(),
                symbol: "gearshape".to_owned(),
                note: None,
                fields: vec![bridge::FormField::Toggle {
                    id: "notifications".to_owned(),
                    label: "New mail:".to_owned(),
                    title: "Show a notification".to_owned(),
                    value: true,
                    help: None,
                }],
                more: None,
            }],
            problem: None,
        })
        .unwrap();
        host.show_settings(settings.clone());
        let sent: CoreToShell = bridge::decode(&shell.lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(sent, CoreToShell::SettingsShow(settings));

        shell
            .connection
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"settings.opened\",\"params\":{}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"settings.used\",\"params\":{\"pane\":\"general\",\"field\":\"notifications\",\"value\":false}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"settings.used\",\"params\":{\"pane\":\"general\",\"field\":\"notifications\"}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"settings.closed\",\"params\":{}}\n",
            )
            .unwrap();
        assert_eq!(next(&mut events).await, HostEvent::SettingsOpened);
        let HostEvent::SettingsUsed(used) = next(&mut events).await else {
            panic!("not a setting used");
        };
        assert_eq!((used.pane(), used.field()), ("general", "notifications"));
        assert_eq!(
            used.did(),
            &bridge::SettingsDid::Set(bridge::FormValue::Flag(false))
        );
        // One that says nothing was done is not one.
        assert!(matches!(next(&mut events).await, HostEvent::Problem(_)));
        assert_eq!(next(&mut events).await, HostEvent::SettingsClosed);
    }

    #[tokio::test]
    async fn a_form_message_that_cannot_be_read_is_not_quoted() {
        let (_host, mut events, mut shell) = connect();
        // Each is wrong in a way the parser would describe by quoting
        // what it found: a secret where a name or a number belongs.
        shell
            .connection
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"form.used\",\"params\":{\"step\":\"password\",\"values\":{},\"secrets\":\"not-a-real-password\"}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"form.used\",\"params\":{\"step\":\"password\",\"values\":{},\"secrets\":{},\"not-a-real-password\":1}}\n\
                  {\"jsonrpc\":\"2.0\",\"method\":\"form.used\",\"params\":{\"step\":\"password\",\"values\":{},\"secrets\":{\"password\":\"not-a-real-password\"\n",
            )
            .unwrap();
        for _ in 0..3 {
            let event = next(&mut events).await;
            assert_eq!(
                event,
                HostEvent::Problem("bridge: a form.used message could not be read".to_owned())
            );
        }
        // Any other line is still described in full.
        shell.connection.write_all(b"not json\n").unwrap();
        assert!(
            matches!(next(&mut events).await, HostEvent::Problem(text) if text.contains("not a bridge message"))
        );
    }

    #[tokio::test]
    async fn the_shell_going_away_is_reported_once() {
        let (host, mut events, shell) = connect();
        drop(shell);
        assert_eq!(next(&mut events).await, HostEvent::Disconnected);
        // Sending into the void is harmless.
        host.close_pane();
        host.close_pane();
    }

    #[test]
    fn connecting_to_nothing_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (events_tx, _events) = unbounded_channel();
        assert!(ShellHost::connect(&dir.path().join("absent.sock"), events_tx).is_err());
    }
}
