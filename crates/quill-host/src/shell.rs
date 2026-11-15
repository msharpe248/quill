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
            Ok(ShellToCore::AppWillQuit(_)) => HostEvent::WillQuit,
            Err(err) => HostEvent::Problem(format!("bridge: {err}")),
        };
        if events.send(event).is_err() {
            return;
        }
    }
    let _ = events.send(HostEvent::Disconnected);
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
            unread: 12,
            title: "Inbox".to_owned(),
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
            r#"{"jsonrpc":"2.0","method":"state.update","params":{"context":"list","enabled":["view.zoom"],"unread":12,"title":"Inbox"}}"#
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
                  {\"jsonrpc\":\"2.0\",\"method\":\"app.will_quit\",\"params\":{}}\n",
            )
            .unwrap();
        assert_eq!(
            next(&mut events).await,
            HostEvent::RunCommand("view.zoom".to_owned())
        );
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
        host.web(png, "a message", true);
        assert!(matches!(sent(&mut shell), CoreToShell::PaneWeb(web) if web.remote));
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
