//! The door to the Quill that is running: a socket file in the data
//! folder, which only the user's own programs can reach.
//!
//! The commands for a shell and `quill mcp` come in by it, so that there
//! is one writer on the mailbox and the window shows a change as it is
//! made. Nothing listens on the network. A program reaches Quill only by
//! running as the same user: the file is the user's alone, in a folder
//! that is the user's alone, and whoever connects is asked of the system
//! and turned away if they are anyone else. There is no password, because
//! being the user is the proof.
//!
//! What crosses it is a line of JSON each way, and is Quill's own: the
//! two ends are the same program.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use quill_tui::agent::{Answer, Call, Caller, Code, Refusal, Way};
use quill_tui::app::{AgentAsk, AgentLink};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{Semaphore, oneshot, watch};

/// The socket's name in the data folder.
pub const SOCKET: &str = "agent.sock";

/// The longest line either end reads. An answer describes fifty messages
/// at most and a message's text comes in pieces, so this is room to
/// spare, and a line past it is not Quill's.
const LONGEST_LINE: usize = 8 * 1024 * 1024;

/// How many programs may be connected at once.
const MOST_CONNECTIONS: usize = 32;

/// How long a command waits for the running Quill to answer. A check for
/// mail is the slowest thing asked, and Quill gives up on one before
/// this.
const ANSWER_WAIT: Duration = Duration::from_secs(150);

/// What is asked, on one line.
#[derive(Debug, Serialize, Deserialize)]
struct Request {
    id: u64,
    /// `call`, `tools` or `hello`.
    method: String,
    /// `shell` or `mcp`.
    way: String,
    /// The name an MCP client gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Value>,
    /// From a shell: the folder a file is to be written into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    save_to: Option<String>,
}

/// What is answered, on one line. One of the three is there.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Response {
    id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<Done>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused: Option<Turned>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Done {
    value: Value,
    lines: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Turned {
    code: String,
    message: String,
}

fn way_word(way: Way) -> &'static str {
    way.as_str()
}

fn way_from(word: &str) -> Option<Way> {
    match word {
        "shell" => Some(Way::Shell),
        "mcp" => Some(Way::Mcp),
        _ => None,
    }
}

/// What the door is told to do to the programs connected to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Word {
    /// Raised to hang up on everyone.
    pub hang_up: u64,
}

/// The door, open. Dropping it closes it and takes the file away.
pub struct Door {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
    /// What the programs connected are told: kept here so that it lasts
    /// as long as the door does, whoever else has let go of it.
    word: std::sync::Arc<watch::Sender<Word>>,
}

impl Drop for Door {
    fn drop(&mut self) {
        self.task.abort();
        // Nobody is left talking to a door that has closed.
        self.word.send_modify(|word| word.hang_up += 1);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Opens the door in `data_dir`. Whoever calls this holds the mailbox, so
/// a socket file that is there already was left by a Quill that is gone,
/// and is replaced. An error is a reason to say so and go on without:
/// most likely a data folder whose path is too long for a socket.
pub fn open(data_dir: &Path) -> std::io::Result<(Door, AgentLink)> {
    let path = data_dir.join(SOCKET);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let (asks_tx, asks) = tokio::sync::mpsc::unbounded_channel();
    let (word, heard) = watch::channel(Word::default());
    let task = tokio::spawn(serve(listener, asks_tx, heard));
    // Hangs up on every program that is connected. The door stays open:
    // whether anything is let in again is the settings' to say.
    let word = std::sync::Arc::new(word);
    let hang_up = {
        let word = std::sync::Arc::clone(&word);
        Box::new(move || word.send_modify(|word| word.hang_up += 1)) as Box<dyn Fn() + Send>
    };
    Ok((
        Door { path, task, word },
        AgentLink {
            asks,
            hang_up: Some(hang_up),
        },
    ))
}

/// Whether whoever is at the other end is this user.
fn is_user(stream: &UnixStream) -> bool {
    stream
        .peer_cred()
        .is_ok_and(|peer| peer.uid() == rustix::process::geteuid().as_raw())
}

async fn serve(
    listener: UnixListener,
    asks: UnboundedSender<AgentAsk>,
    word: watch::Receiver<Word>,
) {
    let room = std::sync::Arc::new(Semaphore::new(MOST_CONNECTIONS));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            // The listener has gone wrong: wait, so as not to spin.
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        if !is_user(&stream) {
            continue;
        }
        // With no room, the connection is closed: the command says that
        // Quill did not answer.
        let Ok(permit) = std::sync::Arc::clone(&room).try_acquire_owned() else {
            continue;
        };
        let asks = asks.clone();
        let mut word = word.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let hung_up = word.borrow().hang_up;
            tokio::select! {
                () = connection(stream, asks) => {}
                () = async {
                    while word.changed().await.is_ok() {
                        if word.borrow().hang_up != hung_up {
                            return;
                        }
                    }
                    // The door has gone.
                } => {}
            }
        });
    }
}

/// Answers one program's requests, one at a time, until it hangs up or
/// sends something that is not a request.
async fn connection(stream: UnixStream, asks: UnboundedSender<AgentAsk>) {
    let (read, mut write) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read);
    let mut line = Vec::new();
    loop {
        if !matches!(read_line(&mut lines, &mut line).await, Ok(true)) {
            return;
        }
        let Ok(request) = serde_json::from_slice::<Request>(&line) else {
            return;
        };
        let response = answer(request, &asks).await;
        let Ok(mut text) = serde_json::to_vec(&response) else {
            return;
        };
        text.push(b'\n');
        if write.write_all(&text).await.is_err() {
            return;
        }
    }
}

/// Reads one line into `line`, without its end. `false` when there is no
/// whole line to be had: the other end hung up, or sent more than a line
/// may be.
async fn read_line<R>(reader: &mut R, line: &mut Vec<u8>) -> std::io::Result<bool>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    line.clear();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return Ok(false);
        }
        let (taken, whole) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                line.extend_from_slice(&buffer[..end]);
                (end + 1, true)
            }
            None => {
                line.extend_from_slice(buffer);
                (buffer.len(), false)
            }
        };
        reader.consume(taken);
        if line.len() > LONGEST_LINE {
            return Ok(false);
        }
        if whole {
            return Ok(true);
        }
    }
}

fn turned(id: u64, refusal: &Refusal) -> Response {
    Response {
        id,
        refused: Some(Turned {
            code: refusal.code.as_str().to_owned(),
            message: refusal.message.clone(),
        }),
        ..Response::default()
    }
}

async fn answer(request: Request, asks: &UnboundedSender<AgentAsk>) -> Response {
    let id = request.id;
    let closing = || Refusal::new(Code::NotRunning, "Quill is closing.");
    let Some(way) = way_from(&request.way) else {
        return turned(id, &Refusal::invalid("That is not a way in."));
    };
    let caller = match way {
        Way::Shell => Caller::shell(),
        Way::Mcp => Caller::mcp(request.name.as_deref().unwrap_or_default()),
    };
    match request.method.as_str() {
        "hello" => {
            if asks.send(AgentAsk::Hello { caller }).is_err() {
                return turned(id, &closing());
            }
            Response {
                id,
                tools: Some(Vec::new()),
                ..Response::default()
            }
        }
        "tools" => {
            let (reply, heard) = oneshot::channel();
            if asks.send(AgentAsk::Tools { way, reply }).is_err() {
                return turned(id, &closing());
            }
            match heard.await {
                Ok(tools) => Response {
                    id,
                    tools: Some(tools),
                    ..Response::default()
                },
                Err(_) => turned(id, &closing()),
            }
        }
        "call" => {
            let parsed = Call::parse(
                request.tool.as_deref().unwrap_or_default(),
                request.arguments.as_ref().unwrap_or(&Value::Null),
            );
            let mut call = match parsed {
                Ok(call) => call,
                Err(refusal) => return turned(id, &refusal),
            };
            // Only a shell command names a folder to write a file into.
            if let (Way::Shell, Call::AttachmentSave { to, .. }) = (way, &mut call) {
                *to = request.save_to;
            }
            let (reply, heard) = oneshot::channel();
            let ask = AgentAsk::Call {
                caller,
                call: Box::new(call),
                reply,
            };
            if asks.send(ask).is_err() {
                return turned(id, &closing());
            }
            match heard.await {
                Ok(Ok(done)) => Response {
                    id,
                    ok: Some(Done {
                        value: done.value,
                        lines: done.lines,
                    }),
                    ..Response::default()
                },
                Ok(Err(refusal)) => turned(id, &refusal),
                Err(_) => turned(id, &closing()),
            }
        }
        _ => turned(id, &Refusal::invalid("That is not something to ask.")),
    }
}

/// A command's end of the door.
pub struct Client {
    stream: BufReader<StdStream>,
    next: u64,
}

/// Connects to the Quill that has the mailbox in `data_dir` open, if one
/// is there and listening. `None` is the ordinary case of no Quill
/// running, and of one that has gone and left its file behind.
pub fn connect(data_dir: &Path) -> Option<Client> {
    let stream = StdStream::connect(data_dir.join(SOCKET)).ok()?;
    stream.set_read_timeout(Some(ANSWER_WAIT)).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    Some(Client {
        stream: BufReader::new(stream),
        next: 0,
    })
}

impl Client {
    fn ask(&mut self, mut request: Request) -> std::io::Result<Response> {
        self.next += 1;
        request.id = self.next;
        let mut text = serde_json::to_vec(&request)?;
        text.push(b'\n');
        self.stream.get_mut().write_all(&text)?;
        let mut line = Vec::new();
        let read = (&mut self.stream)
            .take(LONGEST_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 || line.len() > LONGEST_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Quill did not answer",
            ));
        }
        Ok(serde_json::from_slice(&line)?)
    }

    fn request(way: Way, method: &str) -> Request {
        Request {
            id: 0,
            method: method.to_owned(),
            way: way_word(way).to_owned(),
            name: None,
            tool: None,
            arguments: None,
            save_to: None,
        }
    }

    /// Asks the running Quill to do `call`. The outer error is the door's
    /// (Quill went away); the inner one is Quill's refusal.
    pub fn call(
        &mut self,
        caller: &Caller,
        call: &Call,
    ) -> std::io::Result<Result<Answer, Refusal>> {
        let save_to = match call {
            Call::AttachmentSave { to, .. } => to.clone(),
            _ => None,
        };
        let response = self.ask(Request {
            name: Some(caller.name.clone()),
            tool: Some(call.name().to_owned()),
            arguments: Some(call.arguments()),
            save_to,
            ..Self::request(caller.way, "call")
        })?;
        Ok(match (response.ok, response.refused) {
            (Some(done), _) => Ok(Answer {
                value: done.value,
                lines: done.lines,
            }),
            (None, Some(turned)) => Err(Refusal::new(
                Code::from_word(&turned.code).unwrap_or(Code::Failed),
                turned.message,
            )),
            (None, None) => Err(Refusal::new(Code::Failed, "Quill gave no answer.")),
        })
    }

    /// As [`Client::call`], for a request as an MCP client wrote it: the
    /// running Quill reads it, and refuses what it cannot read.
    pub fn call_named(
        &mut self,
        caller: &Caller,
        tool: &str,
        arguments: &Value,
    ) -> std::io::Result<Result<Answer, Refusal>> {
        let response = self.ask(Request {
            name: Some(caller.name.clone()),
            tool: Some(tool.to_owned()),
            arguments: Some(arguments.clone()),
            ..Self::request(caller.way, "call")
        })?;
        Ok(match (response.ok, response.refused) {
            (Some(done), _) => Ok(Answer {
                value: done.value,
                lines: done.lines,
            }),
            (None, Some(turned)) => Err(Refusal::new(
                Code::from_word(&turned.code).unwrap_or(Code::Failed),
                turned.message,
            )),
            (None, None) => Err(Refusal::new(Code::Failed, "Quill gave no answer.")),
        })
    }

    /// The tools this way in may use now.
    pub fn tools(&mut self, way: Way) -> std::io::Result<Vec<Value>> {
        Ok(self
            .ask(Self::request(way, "tools"))?
            .tools
            .unwrap_or_default())
    }

    /// Says that an MCP client of this name has connected.
    pub fn hello(&mut self, caller: &Caller) -> std::io::Result<()> {
        self.ask(Request {
            name: Some(caller.name.clone()),
            ..Self::request(caller.way, "hello")
        })?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use quill_core::config::AgentConfig;
    use quill_store::Store;
    use quill_tui::agent::{self, Settings};
    use serde_json::json;

    use super::*;

    /// Stands in for the Quill behind the door: answers what is asked on
    /// the demo mailbox, as the run loop and the store worker would.
    fn quill_behind(
        mut link: AgentLink,
        agent: AgentConfig,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        tokio::spawn(async move {
            let mut store = Store::open_in_memory().unwrap();
            quill_tui::demo::seed(&mut store, 1_794_648_600)
                .await
                .unwrap();
            let settings = Settings::new(agent, jiff::tz::TimeZone::UTC);
            let mut greeted = Vec::new();
            while let Some(ask) = link.asks.recv().await {
                match ask {
                    AgentAsk::Call {
                        caller,
                        call,
                        reply,
                    } => {
                        let outcome =
                            agent::perform(&mut store, &settings, &caller, &call, 1_794_648_600);
                        let _ = reply.send(outcome.result);
                    }
                    AgentAsk::Tools { reply, .. } => {
                        let _ = reply.send(agent::tools_for(&settings));
                    }
                    AgentAsk::Hello { caller } => greeted.push(caller.name),
                }
            }
            greeted
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_is_answered_through_the_door_as_it_would_be_on_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (door, link) = open(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join(SOCKET))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let behind = quill_behind(
            link,
            AgentConfig {
                mcp: true,
                ..AgentConfig::default()
            },
        );

        let data_dir = dir.path().to_path_buf();
        let (found, refused, tools, off) = tokio::task::spawn_blocking(move || {
            let mut quill = connect(&data_dir).unwrap();
            let found = quill
                .call(
                    &Caller::shell(),
                    &Call::Search {
                        query: "from:peter dinner".to_owned(),
                        limit: None,
                    },
                )
                .unwrap()
                .unwrap();
            // A refusal comes back as one, with its code.
            let refused = quill
                .call(
                    &Caller::shell(),
                    &Call::Read {
                        id: 999_999,
                        headers: false,
                        max_chars: None,
                        from_char: 0,
                    },
                )
                .unwrap()
                .unwrap_err();
            // As an MCP client writes a request, mistakes and all.
            let claude = Caller::mcp("Claude Code");
            quill.hello(&claude).unwrap();
            let tools = quill.tools(Way::Mcp).unwrap();
            let off = quill
                .call_named(&claude, "delete", &json!({ "ids": [1], "dryrun": true }))
                .unwrap()
                .unwrap_err();
            (found, refused, tools, off)
        })
        .await
        .unwrap();

        assert_eq!(found.value["messages"][0]["subject"], "Sunday dinner");
        assert_eq!(found.lines.len(), 1);
        assert_eq!(refused.code, Code::NotFound);
        // Sending is off as Quill comes, so it is not among the tools.
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"search") && names.contains(&"delete"));
        assert_eq!(off.code, Code::Invalid);
        assert!(off.message.contains("dryrun"));

        let path = dir.path().join(SOCKET);
        drop(door);
        assert!(!path.exists());
        assert_eq!(behind.await.unwrap(), ["Claude Code"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hanging_up_drops_who_is_connected_and_lets_them_come_again() {
        let dir = tempfile::tempdir().unwrap();
        let (_door, mut link) = open(dir.path()).unwrap();
        let hang_up = link.hang_up.take().unwrap();
        let _behind = quill_behind(link, AgentConfig::default());
        let data_dir = dir.path().to_path_buf();
        let (ask, mut asked) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let client = tokio::task::spawn_blocking(move || {
            let mut quill = connect(&data_dir).unwrap();
            assert!(quill.call(&Caller::shell(), &Call::Status).unwrap().is_ok());
            ask.send(()).unwrap();
            wait.recv().unwrap();
            // Hung up on: the door says so, and nothing is answered.
            let gone = quill.call(&Caller::shell(), &Call::Status);
            // And a new connection is let in.
            let again = connect(&data_dir)
                .unwrap()
                .call(&Caller::shell(), &Call::Status)
                .unwrap();
            (gone.is_err(), again.is_ok())
        });
        asked.recv().await.unwrap();
        hang_up();
        tokio::time::sleep(Duration::from_millis(100)).await;
        go.send(()).unwrap();
        assert_eq!(client.await.unwrap(), (true, true));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn what_is_not_a_request_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (_door, link) = open(dir.path()).unwrap();
        let _behind = quill_behind(link, AgentConfig::default());
        let path = dir.path().join(SOCKET);
        let ended = tokio::task::spawn_blocking(move || {
            let mut stream = StdStream::connect(&path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
            let mut heard = Vec::new();
            // Nothing is said back: the connection is closed.
            (stream.read_to_end(&mut heard).unwrap(), heard)
        })
        .await
        .unwrap();
        assert_eq!(ended, (0, Vec::new()));
        // A way in that is not one is refused in words.
        let path = dir.path().join(SOCKET);
        let said = tokio::task::spawn_blocking(move || {
            let mut stream = BufReader::new(StdStream::connect(&path).unwrap());
            stream
                .get_mut()
                .write_all(b"{\"id\":1,\"method\":\"call\",\"way\":\"web\",\"tool\":\"list\"}\n")
                .unwrap();
            let mut line = String::new();
            stream.read_line(&mut line).unwrap();
            line
        })
        .await
        .unwrap();
        assert!(said.contains("\"invalid\""), "{said}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_no_quill_there_is_no_door_and_a_file_left_behind_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        assert!(connect(dir.path()).is_none());
        // A file a Quill that is gone left behind: nobody answers at it.
        let stale = std::os::unix::net::UnixListener::bind(dir.path().join(SOCKET)).unwrap();
        drop(stale);
        assert!(dir.path().join(SOCKET).exists());
        assert!(connect(dir.path()).is_none());
        let (_door, link) = open(dir.path()).unwrap();
        let _behind = quill_behind(link, AgentConfig::default());
        let data_dir = dir.path().to_path_buf();
        let answered = tokio::task::spawn_blocking(move || {
            connect(&data_dir)
                .unwrap()
                .call(&Caller::shell(), &Call::Accounts)
                .unwrap()
                .is_ok()
        })
        .await
        .unwrap();
        assert!(answered);
    }
}
