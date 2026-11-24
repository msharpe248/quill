//! The run loop: terminal events, store answers and host events in; frames,
//! store requests and host messages out.
//!
//! This task owns the terminal and never waits on disk or the network. The
//! store lives on its own thread (see [`crate::data`]), the host has its own
//! (see `quill-host`); everything here is `select`, `update`, draw.

use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use quill_core::config::Appearance;
use quill_host::bridge::{self, CommandInfo, StateUpdate};
use quill_host::{Attached, Host, HostEvent};
use quill_media::kitty;
use quill_store::Store;
use quill_sync::{SyncEvent, SyncHandle, SyncRequest};
use ratatui::backend::IntoCrossterm;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::event::{Event, EventStream, MouseButton, MouseEventKind};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Attribute, ContentStyle, Print, SetAttribute, SetStyle};
use ratatui::crossterm::terminal::{Clear, ClearType};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::agent::{Answer, Call, Caller, Refusal, Way};
use crate::command::{Context, Menu};
use crate::data::{self, DataEvent, DataHandle, DataRequest};
use crate::files::Files;
use crate::keymap::KeyChord;
use crate::setup::{AccountEvent, AccountRequest};
use crate::state::{
    Action, Effects, Env, HostRequest, HostState, Mouse, MouseKind, Notice, SettingsAct, State,
};
use crate::view::{ImagePlacement, LinkPlacement};
use crate::{hook, hyperlink, terminal, view, vim};

/// The sync engine, as the UI holds it: where to send requests, and where
/// its news arrives. With no engine (the demo mailbox) there is only the
/// store.
pub struct SyncLink {
    /// Shared with the accounts service, which adds accounts to the engine.
    pub handle: Option<Arc<SyncHandle>>,
    pub events: UnboundedReceiver<SyncEvent>,
}

/// The accounts service, as the UI holds it: where the setup flow's
/// requests go and where the answers arrive. Whoever starts the UI supplies
/// the service; the UI itself reaches no server, Keychain or config file.
pub struct AccountsLink {
    pub requests: Option<UnboundedSender<AccountRequest>>,
    pub events: UnboundedReceiver<AccountEvent>,
}

impl AccountsLink {
    /// No service: accounts cannot be added from the UI.
    pub fn none() -> Self {
        let (_, events) = unbounded_channel();
        Self {
            requests: None,
            events,
        }
    }
}

/// Something asked of the Quill that is running from outside its window:
/// by a command for a shell, or by an agent over MCP. Whoever starts the
/// UI supplies the door these come in by; the UI itself listens nowhere.
#[derive(Debug)]
pub enum AgentAsk {
    /// Do this, and answer.
    Call {
        caller: Caller,
        call: Box<Call>,
        reply: oneshot::Sender<Result<Answer, Refusal>>,
    },
    /// The tools this way in may use now, as an MCP client is told of
    /// them: none when the way is switched off.
    Tools {
        way: Way,
        reply: oneshot::Sender<Vec<serde_json::Value>>,
    },
    /// An MCP client has connected.
    Hello { caller: Caller },
    /// And has gone: its connection closed.
    Bye { caller: Caller },
    /// Go on saying what mail arrives that a search finds, a line for
    /// each message into `lines`, until whoever asked lets go of the
    /// other end. `reply` says whether it was begun.
    Watch {
        caller: Caller,
        query: String,
        lines: UnboundedSender<serde_json::Value>,
        reply: oneshot::Sender<Result<Answer, Refusal>>,
    },
}

/// Where requests from outside the window arrive.
pub struct AgentLink {
    pub asks: UnboundedReceiver<AgentAsk>,
    /// Hangs up on every program that is connected, when the user stops
    /// the agents.
    pub hang_up: Option<Box<dyn Fn() + Send>>,
}

impl AgentLink {
    /// No door: nothing is ever asked.
    pub fn none() -> Self {
        let (_, asks) = unbounded_channel();
        Self {
            asks,
            hang_up: None,
        }
    }
}

impl SyncLink {
    /// No engine: nothing syncs and nothing is ever downloaded.
    pub fn none() -> Self {
        // The sender is dropped at once, so the receiver simply stays quiet.
        let (_, events) = unbounded_channel();
        Self {
            handle: None,
            events,
        }
    }
}

/// Runs the UI over `store` until the user quits. `notices` are shown in the
/// status bar at startup, for problems found while loading configuration.
pub async fn run(
    store: Store,
    mut env: Env,
    mut notices: Vec<Notice>,
    attached: Attached,
    sync: SyncLink,
    accounts: AccountsLink,
    agents: AgentLink,
) -> io::Result<()> {
    let (events_tx, events) = unbounded_channel();
    // Files taken out of messages for viewing go in a folder of this run's
    // own, made when the first is written and removed at the end.
    let view_dir = std::env::temp_dir().join(format!("quill-view-{}", std::process::id()));
    let files = Files {
        view_dir: Some(view_dir.clone()),
        downloads: env.downloads.clone(),
    };
    let data = step(
        "cannot start the store worker",
        data::spawn(store, files, events_tx),
    )?;
    let Attached {
        host,
        events: host_events,
        problem,
    } = attached;
    notices.extend(problem.map(Notice::error));

    let mut screen = step("cannot take over the terminal", terminal::enter(env.mouse))?;
    let (injected_tx, injected) = unbounded_channel();
    let SyncLink {
        handle: sync_handle,
        events: sync_events,
    } = sync;
    let AccountsLink {
        requests: account_requests,
        events: account_events,
    } = accounts;
    let mut io = Io {
        data: &data,
        host: &host,
        sync: sync_handle.as_deref(),
        accounts: account_requests,
        injected: injected_tx,
        preview_dir: view_dir,
        last_host_state: None,
        last_form: None,
        last_settings: None,
        last_place: None,
        last_editor: None,
        images: Images::default(),
        thumbnails: std::collections::HashMap::new(),
        editor_wanted: None,
        closing: Arc::new(AtomicBool::new(false)),
        agents: Agents {
            hang_up: agents.hang_up,
            ..Agents::default()
        },
    };
    let mut inbound = Inbound {
        data: events,
        host: host_events,
        sync: sync_events,
        accounts: account_events,
        injected,
        agents: agents.asks,
    };
    let result = event_loop(&mut screen, &mut env, notices, &mut io, &mut inbound).await;
    // A hook still running is stopped: nothing started here outlives Quill.
    io.closing.store(true, Ordering::SeqCst);
    io.images.forget();
    terminal::leave();
    // Files written for viewing do not outlive the run. The store worker
    // has finished by now: the loop waits for it on the way out.
    let _ = std::fs::remove_dir_all(&io.preview_dir);
    result
}

/// The loop's ways out to the world, all of them non-blocking.
struct Io<'a> {
    data: &'a DataHandle,
    host: &'a Arc<dyn Host>,
    sync: Option<&'a SyncHandle>,
    accounts: Option<UnboundedSender<AccountRequest>>,
    /// Actions that background work feeds back into the loop.
    injected: UnboundedSender<Action>,
    /// A private directory for files handed to the host for viewing.
    preview_dir: PathBuf,
    last_host_state: Option<HostState>,
    /// The form or question the host was last told to show in its sheet.
    last_form: Option<bridge::FormShow>,
    /// What the host was last told its settings window shows, and the
    /// count of answers it was owed then.
    last_settings: Option<(u64, bridge::SettingsShow)>,
    /// The place the host was last told its reading pane has.
    last_place: Option<(u16, u16)>,
    /// Where the host was last told the editor that has a message's text
    /// goes.
    last_editor: Option<crate::state::EditorSpot>,
    images: Images,
    /// Pictures the host has been asked to make, by the id asked under,
    /// and where each will be written.
    thumbnails: std::collections::HashMap<u64, PathBuf>,
    /// A draft that an editor is to be run on. The loop does it: it
    /// finds the program, and in a terminal it owns the terminal.
    editor_wanted: Option<EditorWanted>,
    /// Set as the UI closes, for work started here that must end with it.
    closing: Arc<AtomicBool>,
    /// What has been asked from outside the window and not yet answered.
    agents: Agents,
}

/// Requests from outside the window that are waiting on the store worker
/// or on the engine.
#[derive(Default)]
struct Agents {
    next: u64,
    /// Waiting on the store worker, by the ticket each was sent under.
    asked: std::collections::HashMap<u64, oneshot::Sender<Result<Answer, Refusal>>>,
    /// Waiting on the engine to have looked at these accounts.
    checking: Vec<Checking>,
    /// Waiting on a server to have made a folder, or on a mailing list's
    /// server to have been asked.
    waiting: Vec<Waiting>,
    /// Commands that asked to watch, until the store worker has said they
    /// may: by the ticket each was sent under.
    watch_wanted: std::collections::HashMap<u64, UnboundedSender<serde_json::Value>>,
    /// Commands that are told of the mail that arrives.
    watchers: Vec<Watcher>,
    /// Hangs up on every program that is connected.
    hang_up: Option<Box<dyn Fn() + Send>>,
}

/// A check for mail that someone outside is waiting on.
struct Checking {
    /// The accounts still being looked at.
    waiting: Vec<quill_core::types::LocalAccountId>,
    asked: usize,
    arrived: usize,
    problems: Vec<String>,
    until: tokio::time::Instant,
    reply: oneshot::Sender<Result<Answer, Refusal>>,
}

impl Checking {
    fn answer(self) {
        let Checking {
            asked,
            arrived,
            problems,
            reply,
            ..
        } = self;
        let mut lines = vec![format!(
            "{asked} account{} looked at, {arrived} new message{}.",
            if asked == 1 { "" } else { "s" },
            if arrived == 1 { "" } else { "s" },
        )];
        lines.extend(problems.iter().map(|line| crate::text::sanitize_line(line)));
        let _ = reply.send(Ok(Answer {
            value: serde_json::json!({
                "schema": crate::agent::SCHEMA,
                "accounts": asked,
                "new": arrived,
                "problems": problems,
            }),
            lines,
        }));
    }
}

/// How long a check for mail asked for from outside is waited on.
const CHECK_WAIT: Duration = Duration::from_secs(90);

/// How long a server is waited on for a folder, or a mailing list's
/// server for its answer, before whoever asked is told it is not known.
const SERVER_WAIT: Duration = Duration::from_secs(60);

/// What someone outside is waiting to hear of.
enum WaitingFor {
    /// A folder of this name made on this account's server.
    Folder { account: String, name: String },
    /// A mailing list's server, by its name, asked to unsubscribe.
    Unsubscribed { host: String },
}

/// Something asked from outside that the store worker has done its part
/// of, and that is answered when a server has.
struct Waiting {
    what: WaitingFor,
    /// What is answered when it went well.
    answer: Answer,
    until: tokio::time::Instant,
    reply: oneshot::Sender<Result<Answer, Refusal>>,
}

/// A command that is told of the mail that arrives.
struct Watcher {
    id: u64,
    query: String,
    lines: UnboundedSender<serde_json::Value>,
}

impl Io<'_> {
    /// What the settings are for a request from outside, now.
    fn agent_settings(env: &Env) -> crate::agent::Settings {
        crate::agent::Settings {
            from_names: env
                .identities
                .iter()
                .filter_map(|(account, identity)| {
                    Some((account.clone(), identity.full_name.clone()?))
                })
                .collect(),
            markdown: env.compose.markdown,
            reply_above: env.compose.reply_above,
            running: true,
            // The demo mailbox has no engine and no service.
            online: env.accounts,
            ..crate::agent::Settings::new(env.agent.clone(), env.tz.clone())
        }
    }

    /// Takes a request from outside the window: answers what can be
    /// answered here, and hands the rest to the store worker or the
    /// engine.
    fn agent_ask(&mut self, env: &Env, state: &State, ask: AgentAsk) -> Option<Action> {
        match ask {
            AgentAsk::Tools { reply, .. } => {
                let settings = Self::agent_settings(env);
                let _ = reply.send(crate::agent::tools_for(&settings));
            }
            // Always said: it is how the user learns that something has
            // come in.
            AgentAsk::Hello { caller } => return Some(Action::AgentConnected(caller.name)),
            AgentAsk::Bye { caller } => return Some(Action::AgentGone(caller.name)),
            AgentAsk::Watch {
                caller,
                query,
                lines,
                reply,
            } => {
                // Checked and written down as any request is; what it
                // came to says whether the watching begins.
                self.agents.next += 1;
                let ticket = self.agents.next;
                self.agents.asked.insert(ticket, reply);
                self.agents.watch_wanted.insert(ticket, lines);
                let sent = self.data.send(DataRequest::Agent {
                    ticket,
                    caller: Box::new(caller),
                    call: Box::new(Call::Watch { query }),
                    settings: Box::new(Self::agent_settings(env)),
                });
                if !sent {
                    self.agents.watch_wanted.remove(&ticket);
                    if let Some(reply) = self.agents.asked.remove(&ticket) {
                        let _ = reply.send(Err(Refusal::new(
                            crate::agent::Code::Failed,
                            "Quill is closing.",
                        )));
                    }
                }
            }
            AgentAsk::Call {
                caller,
                call,
                reply,
            } => {
                let settings = Self::agent_settings(env);
                if let Call::Sync { account } = call.as_ref() {
                    if let Some(refusal) = crate::agent::refused(&settings, caller.way, &call) {
                        let _ = reply.send(Err(refusal));
                        return None;
                    }
                    self.agent_check(state, account.as_deref(), reply);
                    return None;
                }
                self.agents.next += 1;
                let ticket = self.agents.next;
                self.agents.asked.insert(ticket, reply);
                let sent = self.data.send(DataRequest::Agent {
                    ticket,
                    caller: Box::new(caller),
                    call,
                    settings: Box::new(settings),
                });
                if !sent && let Some(reply) = self.agents.asked.remove(&ticket) {
                    let _ = reply.send(Err(Refusal::new(
                        crate::agent::Code::Failed,
                        "Quill is closing.",
                    )));
                }
            }
        }
        None
    }

    /// Checks for mail because someone outside asked, and keeps their
    /// question until the accounts have been looked at.
    fn agent_check(
        &mut self,
        state: &State,
        account: Option<&str>,
        reply: oneshot::Sender<Result<Answer, Refusal>>,
    ) {
        let Some(sync) = self.sync else {
            let _ = reply.send(Err(Refusal::new(
                crate::agent::Code::Failed,
                "This mailbox has no accounts that sync.",
            )));
            return;
        };
        let accounts: Vec<_> = state
            .accounts
            .iter()
            .filter(|row| env_sees(account, &row.name))
            .map(|row| row.id)
            .collect();
        if accounts.is_empty() {
            let _ = reply.send(Err(Refusal::not_found(match account {
                Some(name) => format!(
                    "There is no account named \"{}\".",
                    crate::text::sanitize_line(name)
                ),
                None => "There is no account to check.".to_owned(),
            })));
            return;
        }
        match account {
            Some(_) => {
                for id in &accounts {
                    sync.send(SyncRequest::SyncOne(*id));
                }
            }
            None => {
                sync.send(SyncRequest::SyncNow);
            }
        }
        // And what is waiting in the queue is carried out.
        sync.send(SyncRequest::Queue);
        self.agents.checking.push(Checking {
            asked: accounts.len(),
            waiting: accounts,
            arrived: 0,
            problems: Vec::new(),
            until: tokio::time::Instant::now() + CHECK_WAIT,
            reply,
        });
    }

    /// Notes what the engine said for whoever outside is waiting on a
    /// check for mail, and answers those whose accounts have all been
    /// looked at.
    fn agent_heard(&mut self, env: &Env, event: &SyncEvent) {
        self.agents.heard_engine(event);
        // Mail that has arrived is told to the commands watching for it,
        // once the store worker has said which of it each looks for.
        if let SyncEvent::Arrived { messages, .. } = event {
            self.agents
                .watchers
                .retain(|watcher| !watcher.lines.is_closed());
            if !self.agents.watchers.is_empty() {
                self.data.send(DataRequest::AgentArrived {
                    watchers: self
                        .agents
                        .watchers
                        .iter()
                        .map(|watcher| (watcher.id, watcher.query.clone()))
                        .collect(),
                    messages: messages.clone(),
                    settings: Box::new(Self::agent_settings(env)),
                });
            }
        }
        if self.agents.checking.is_empty() {
            return;
        }
        for check in &mut self.agents.checking {
            match event {
                SyncEvent::Syncing {
                    account,
                    active: false,
                } => check.waiting.retain(|id| id != account),
                SyncEvent::Arrived { messages, .. } => check.arrived += messages.len(),
                SyncEvent::Problem { account, message } => {
                    check.problems.push(format!("{account}: {message}"));
                }
                _ => {}
            }
        }
        self.agent_settle(false);
    }

    /// Answers the checks for mail that are done, or with `late` those
    /// that have been waited on long enough.
    fn agent_settle(&mut self, late: bool) {
        let now = tokio::time::Instant::now();
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agents.checking)
            .into_iter()
            .partition(|check| check.waiting.is_empty() || (late && check.until <= now));
        self.agents.checking = waiting;
        for mut check in done {
            if !check.waiting.is_empty() {
                check
                    .problems
                    .push("Gave up waiting: not every account answered in time.".to_owned());
            }
            check.answer();
        }
        if late {
            self.agents.give_up(now);
        }
    }

    /// Gives a request from outside its answer, when the store worker has
    /// one for it: at once, or when whatever is still to be done with the
    /// window, the engine or the network has been.
    fn agent_done(
        &mut self,
        ticket: u64,
        result: Result<Answer, Refusal>,
        then: Option<crate::agent::Then>,
    ) -> Option<Action> {
        use crate::agent::Then;
        let watch = self.agents.watch_wanted.remove(&ticket);
        let reply = self.agents.asked.remove(&ticket)?;
        let answer = match result {
            Ok(answer) => answer,
            Err(refusal) => {
                let _ = reply.send(Err(refusal));
                return None;
            }
        };
        let wait = |what, answer, reply| Waiting {
            what,
            answer,
            until: tokio::time::Instant::now() + SERVER_WAIT,
            reply,
        };
        match then {
            None => {
                let _ = reply.send(Ok(answer));
            }
            Some(Then::Show { message, folder }) => {
                let _ = reply.send(Ok(answer));
                self.host.show_window();
                return Some(Action::ShowMessage { message, folder });
            }
            Some(Then::Watch { query }) => {
                if let Some(lines) = watch {
                    self.agents.watchers.push(Watcher {
                        id: ticket,
                        query,
                        lines,
                    });
                }
                let _ = reply.send(Ok(answer));
            }
            Some(Then::MakeFolder {
                account,
                account_name,
                name,
            }) => match self.sync {
                Some(engine) => {
                    engine.send(SyncRequest::CreateFolder {
                        account,
                        name: name.clone(),
                    });
                    self.agents.waiting.push(wait(
                        WaitingFor::Folder {
                            account: account_name,
                            name,
                        },
                        answer,
                        reply,
                    ));
                }
                None => {
                    let _ = reply.send(Err(Refusal::new(
                        crate::agent::Code::Failed,
                        "There is no server to ask here. Nothing was made.",
                    )));
                }
            },
            Some(Then::Unsubscribe { url, host }) => match &self.accounts {
                Some(service) if service.send(AccountRequest::Unsubscribe { url }).is_ok() => {
                    self.agents.waiting.push(wait(
                        WaitingFor::Unsubscribed { host },
                        answer,
                        reply,
                    ));
                }
                _ => {
                    let _ = reply.send(Err(Refusal::new(
                        crate::agent::Code::Failed,
                        "There is nothing here to make the request with. Nothing was sent.",
                    )));
                }
            },
        }
        None
    }
}

impl Agents {
    /// Answers those outside who were waiting on a server for what
    /// `heard` says has come of it: well, or with the reason it did not.
    fn waited(&mut self, heard: impl Fn(&WaitingFor) -> Option<Result<(), String>>) {
        // One thing heard answers one who waited: two who asked the same
        // are each answered by their own.
        let Some((index, came)) = self
            .waiting
            .iter()
            .enumerate()
            .find_map(|(index, waiting)| Some((index, heard(&waiting.what)?)))
        else {
            return;
        };
        let waiting = self.waiting.remove(index);
        let _ = waiting.reply.send(match came {
            Ok(()) => Ok(waiting.answer),
            Err(why) => Err(Refusal::new(
                crate::agent::Code::Failed,
                crate::text::sanitize_line(&why),
            )),
        });
    }

    /// Notes what the engine said for whoever outside is waiting on a
    /// folder: that it was made, or the problem that says it was not.
    fn heard_engine(&mut self, event: &SyncEvent) {
        self.waited(|what| match (what, event) {
            (
                WaitingFor::Folder { account, name },
                SyncEvent::FolderMade {
                    account: made_for,
                    name: made,
                },
            ) if account == made_for && name == made => Some(Ok(())),
            (WaitingFor::Folder { name, .. }, SyncEvent::Problem { message, .. })
                if quill_sync::is_folder_trouble(message, name) =>
            {
                Some(Err(message.clone()))
            }
            _ => None,
        });
    }

    /// Notes what the accounts service said for whoever outside is
    /// waiting on a mailing list's server.
    fn heard_service(&mut self, event: &AccountEvent) {
        if let AccountEvent::Unsubscribed { host, result } = event {
            self.waited(|what| match what {
                WaitingFor::Unsubscribed { host: asked } if asked == host => Some(
                    result
                        .clone()
                        .map_err(|why| format!("{host} was asked, and it did not go well: {why}")),
                ),
                _ => None,
            });
        }
    }

    /// Tells those who have waited on a server long enough that what
    /// came of it is not known.
    fn give_up(&mut self, now: tokio::time::Instant) {
        let (late, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|waiting| waiting.until <= now);
        self.waiting = waiting;
        for waiting in late {
            let _ = waiting.reply.send(Err(Refusal::new(
                crate::agent::Code::Failed,
                match waiting.what {
                    WaitingFor::Folder { .. } => {
                        "The server was asked for the folder and has not answered yet. \
                         Whether it was made is not known: look at the folders before asking \
                         again."
                    }
                    WaitingFor::Unsubscribed { .. } => {
                        "The list's server was asked and has not answered yet. Whether it \
                         unsubscribed you is not known."
                    }
                },
            )));
        }
    }

    /// Tells the commands that are watching of the mail that arrived and
    /// that each looks for. `None`: commands from a shell have been
    /// switched off, and every watcher is let go.
    fn arrived(&mut self, found: Option<Vec<(u64, Vec<serde_json::Value>)>>) {
        let Some(found) = found else {
            self.watchers.clear();
            return;
        };
        for (id, messages) in found {
            if let Some(watcher) = self.watchers.iter().find(|watcher| watcher.id == id) {
                for message in messages {
                    let _ = watcher.lines.send(message);
                }
            }
        }
    }
}

/// Whether an account of this name is the one asked for, or any is.
fn env_sees(wanted: Option<&str>, name: &str) -> bool {
    wanted.is_none_or(|wanted| wanted.trim().eq_ignore_ascii_case(name))
}

/// A draft file in its own folder, and the editor to run on it.
struct EditorWanted {
    path: PathBuf,
    dir: PathBuf,
    /// What the message is called, for a host that shows the editor.
    title: String,
    editor: vim::ExternalEditor,
    /// The line of the file the body starts on, counting from 1.
    body_line: u32,
}

/// The command an editor is started with on a draft: the user's own
/// program, found by the name the settings or the environment give it or
/// by the editor's own name on the `PATH`. `None` when there is no such
/// program, which costs nothing but a line saying so.
fn editor_command(env: &Env, wanted: &EditorWanted) -> Option<std::process::Command> {
    let program = vim::find_program(
        vim::program_name(wanted.editor, env.editor_program.as_ref()),
        std::env::var_os("PATH").as_deref(),
    )?;
    Some(vim::command(
        wanted.editor,
        &program,
        &wanted.path,
        &wanted.dir,
        wanted.body_line,
    ))
}

/// What the host is to be told about its sheet.
#[derive(Debug, PartialEq, Eq)]
enum SheetChange<'a> {
    /// Show this form: in a sheet that is opened for it, or in place of
    /// the form the sheet has.
    Show(&'a bridge::FormShow),
    /// Take the sheet down.
    Close,
}

/// What to tell the host, given the form it was last told to show and the
/// one in front now. Nothing while they are the same. One form giving way
/// to another, whatever each is asking, is the new one shown and never the
/// sheet taken down first.
fn sheet_change<'a>(
    last: Option<&bridge::FormShow>,
    current: Option<&'a bridge::FormShow>,
) -> Option<SheetChange<'a>> {
    if last == current {
        return None;
    }
    Some(current.map_or(SheetChange::Close, SheetChange::Show))
}

/// The most pictures the terminal is left holding at once. Each is a few
/// megabytes of its memory; beyond this the oldest is taken back.
const PICTURES_KEPT: usize = 6;

/// The largest a picture is kept at, in pixels: enough to fill the reader
/// on a sharp display, and small enough to send in one go.
const PICTURE_PIXELS: (u32, u32) = (800, 500);

/// Pictures decoded and waiting to be sent to the terminal, by the id
/// they will be known by. Filled by the tasks that decode, emptied by the
/// run loop as it sends.
type Decoded = Arc<std::sync::Mutex<std::collections::HashMap<u32, quill_media::Image>>>;

/// What the terminal has been asked to draw over the frame.
#[derive(Default)]
struct Images {
    decoded: Decoded,
    /// The pictures the terminal holds, oldest first, with their sizes in
    /// pixels.
    sent: Vec<(u32, (u32, u32))>,
    placed: Vec<ImagePlacement>,
    /// The terminal size the placements were made at. A resize clears the
    /// screen, and the images with it.
    size: (u16, u16),
}

impl Images {
    /// Makes the terminal show exactly `wanted`. Costs nothing when that is
    /// what it shows already.
    fn sync(&mut self, wanted: Vec<ImagePlacement>, size: (u16, u16)) -> io::Result<()> {
        if wanted == self.placed && (size == self.size || wanted.is_empty()) {
            return Ok(());
        }
        let mut out = stdout();
        for old in &self.placed {
            queue!(out, Print(kitty::clear(old.id)))?;
        }
        let mut shown = Vec::with_capacity(wanted.len());
        for image in &wanted {
            if !self.sent.iter().any(|(id, _)| *id == image.id) {
                // The pixels go once; placing the picture again is a few
                // bytes. They are let go of here: the terminal has them.
                let pixels = self
                    .decoded
                    .lock()
                    .ok()
                    .and_then(|mut decoded| decoded.remove(&image.id));
                let Some(pixels) = pixels else {
                    // Nothing to send for it: it is not drawn.
                    continue;
                };
                queue!(out, Print(kitty::transmit(image.id, &pixels)))?;
                self.sent.push((image.id, (pixels.width, pixels.height)));
            }
            queue!(
                out,
                MoveTo(image.area.x, image.area.y),
                Print(kitty::place(image.id, image.area.width, image.area.height))
            )?;
            shown.push(*image);
        }
        // The terminal is not left holding every picture ever looked at.
        while self.sent.len() > PICTURES_KEPT {
            let (oldest, _) = self.sent.remove(0);
            if shown.iter().any(|image| image.id == oldest) {
                // On screen now: keep it, and stop looking.
                self.sent.insert(0, (oldest, (0, 0)));
                break;
            }
            queue!(out, Print(kitty::forget(oldest)))?;
        }
        out.flush()?;
        self.placed = shown;
        self.size = size;
        Ok(())
    }

    /// The size of a picture the terminal already holds.
    fn held(&self, id: u32) -> Option<(u32, u32)> {
        self.sent
            .iter()
            .find(|(sent, size)| *sent == id && *size != (0, 0))
            .map(|(_, size)| *size)
    }

    /// Frees what the terminal is holding for us.
    fn forget(&mut self) {
        if !self.sent.is_empty() {
            let mut out = stdout();
            for (id, _) in self.sent.drain(..) {
                let _ = queue!(out, Print(kitty::forget(id)));
            }
            let _ = out.flush();
            self.placed.clear();
        }
    }
}

/// Marks the link text of the frame just drawn as hyperlinks, by writing it
/// again in place with its address attached. Every piece of link text is
/// written on every frame, so each cell's link is always the current one.
fn mark_links(links: &[LinkPlacement]) -> io::Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    let mut out = stdout();
    for link in links {
        let style: ContentStyle = link.style.into_crossterm();
        let open = link.url.as_deref().and_then(hyperlink::open);
        queue!(out, MoveTo(link.x, link.y))?;
        if let Some(open) = &open {
            queue!(out, Print(open))?;
        }
        queue!(
            out,
            SetStyle(style),
            Print(&link.text),
            // The next frame starts from a terminal with no style set.
            SetAttribute(Attribute::Reset)
        )?;
        if open.is_some() {
            queue!(out, Print(hyperlink::CLOSE))?;
        }
    }
    out.flush()
}

impl Io<'_> {
    fn perform(&mut self, effects: Effects) {
        for request in effects.data {
            self.data.send(request);
        }
        for request in effects.host {
            match request {
                HostRequest::OpenUrl(url) => self.host.open_url(&url),
                HostRequest::PickFiles => self.host.pick_files(),
                HostRequest::PickTheme => self.host.pick_theme(),
                HostRequest::ShowKeys => self.host.show_keys(),
                HostRequest::CloseWindow => self.host.close_window(),
                HostRequest::ShowSettings(pane) => self.host.show_settings_pane(pane),
                HostRequest::Preview { files, index } => {
                    let files: Vec<(&std::path::Path, &str)> = files
                        .iter()
                        .map(|(path, title)| (path.as_path(), title.as_str()))
                        .collect();
                    self.host.preview(&files, index);
                }
                HostRequest::Play { path, title } => self.host.play(&path, &title),
                HostRequest::OpenFile(path) => self.host.open_file(&path),
                HostRequest::EditRules => self.host.edit_rules(),
                // Only ever asked for where the host has the pane.
                HostRequest::Reading {
                    path,
                    title,
                    remote,
                    header,
                    notice,
                } => {
                    // A notice that cannot be said is not; the page is
                    // shown all the same.
                    let notice = notice.and_then(|notice| {
                        let actions = notice
                            .actions
                            .into_iter()
                            .map(|(command, title)| bridge::ReadingAction {
                                command: command.to_owned(),
                                title: title.to_owned(),
                            })
                            .collect();
                        bridge::ReadingNotice::new(&notice.text, actions)
                    });
                    self.host
                        .reading_show(&path, &title, remote, header, notice);
                }
                HostRequest::ScrollReading { down } => self.host.reading_scroll(if down {
                    bridge::ScrollBy::PageDown
                } else {
                    bridge::ScrollBy::PageUp
                }),
                HostRequest::CloseReading => self.host.reading_close(),
                HostRequest::Notify {
                    id,
                    title,
                    subtitle,
                    body,
                    actions,
                } => {
                    // One that cannot be said (an empty title) is not.
                    if let Some(notification) = quill_host::bridge::Notification::new(
                        &id,
                        &title,
                        subtitle.as_deref(),
                        &body,
                        &actions,
                    ) {
                        self.host.notify(notification);
                    }
                }
                HostRequest::ClearNotifications(ids) => self.host.clear_notifications(ids),
                HostRequest::LoadPicture {
                    id,
                    path,
                    content_type,
                } => self.load_picture(id, path, &content_type),
                // A host with no checker gives no answer to wait for: the
                // answer is that nothing is misspelt, or suggested.
                HostRequest::SpellCheck { id, text } => {
                    if !self.host.spell_check(id, &text) {
                        let _ = self.injected.send(Action::Spelling {
                            id,
                            words: Vec::new(),
                        });
                    }
                }
                HostRequest::SpellSuggest { id, word } => {
                    if !self.host.spell_suggest(id, &word) {
                        let _ = self.injected.send(Action::Suggestions {
                            id,
                            words: Vec::new(),
                        });
                    }
                }
                // The loop starts it: it knows which program the editor
                // is, and in a terminal it owns the terminal.
                HostRequest::EditDraft {
                    path,
                    dir,
                    title,
                    editor,
                    body_line,
                } => {
                    self.editor_wanted = Some(EditorWanted {
                        path,
                        dir,
                        title,
                        editor,
                        body_line,
                    });
                }
                HostRequest::ClosePane => self.host.close_pane(),
                HostRequest::RunHook(run) => self.run_hook(*run),
                HostRequest::SaveFile { path, name } => {
                    // Only asked for where the host has a panel. Should it
                    // not, say so rather than nothing.
                    if !self.host.save_file(&path, &name) {
                        let _ = self.injected.send(Action::FileSaved { name, saved: false });
                    }
                }
                // Quill's own shell takes the text itself. Anywhere else
                // the terminal owns the clipboard and is asked to take it
                // (OSC 52), which the terminals Quill is meant for do.
                HostRequest::Copy(text) if self.host.copy_text(&text) => {}
                HostRequest::Copy(text) => {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
                    let mut out = stdout();
                    let _ = queue!(out, Print(format!("\x1b]52;c;{encoded}\x1b\\")));
                    let _ = out.flush();
                }
            }
        }
        for request in effects.accounts {
            if let Some(service) = &self.accounts {
                let _ = service.send(request);
            }
        }
        for request in effects.sync {
            match (self.sync, request) {
                (Some(engine), request) => {
                    engine.send(request);
                }
                // With no engine a body that is not in the store never will
                // be. Say so instead of waiting forever.
                (None, SyncRequest::FetchBody(id)) => {
                    let _ = self.injected.send(Action::Sync(SyncEvent::BodyFailed {
                        id,
                        reason: "The body of this message has not been downloaded.".to_owned(),
                    }));
                }
                // With no engine there is no server to ask: said, not left
                // to look as if it were on its way.
                (None, SyncRequest::CreateFolder { name, .. }) => {
                    let _ = self.injected.send(Action::Sync(SyncEvent::Problem {
                        account: "Folders".to_owned(),
                        message: format!(
                            "The folder {name} was not made: there is no server to ask here."
                        ),
                    }));
                }
                (None, SyncRequest::RenameFolder { .. } | SyncRequest::DeleteFolder(_)) => {
                    let _ = self.injected.send(Action::Sync(SyncEvent::Problem {
                        account: "Folders".to_owned(),
                        message: "The folder was not changed: there is no server to ask here."
                            .to_owned(),
                    }));
                }
                (
                    None,
                    SyncRequest::SyncNow
                    | SyncRequest::SyncOne(_)
                    | SyncRequest::Queue
                    | SyncRequest::FetchNear(_),
                ) => {}
            }
        }
        // With no engine there is nothing to download from.
        if let (Some(engine), Some(near)) = (self.sync, effects.near) {
            engine.send(SyncRequest::FetchNear(near));
        }
    }

    /// Tells the host about the state it mirrors, when that has changed.
    fn mirror(&mut self, state: &State, env: &Env) {
        let current = state.host_state(env);
        if self.last_host_state.as_ref() == Some(&current) {
            return;
        }
        self.host.state(StateUpdate {
            context: match current.context {
                Context::Sidebar => bridge::Context::Sidebar,
                Context::List => bridge::Context::List,
                Context::Reader | Context::Select => bridge::Context::Reader,
                Context::Help => bridge::Context::Help,
                Context::Links => bridge::Context::Links,
                // A list to choose from over the reader, as the links are.
                // The shell has no use yet for telling the two apart.
                Context::Attachments => bridge::Context::Links,
                // The same whether the form is drawn here or shown by the
                // host in a sheet: it is what has the keyboard.
                Context::Setup => bridge::Context::Setup,
                Context::Certificate => bridge::Context::Certificate,
                // A question about an account, as far as the shell cares.
                Context::SignIn => bridge::Context::Setup,
                // A box to type in over the panes, as the folder picker is.
                Context::When => bridge::Context::Folders,
                // Another box to type in over the panes.
                Context::NewFolder => bridge::Context::Folders,
                // And the one an account's letter is typed in.
                Context::AccountLetter => bridge::Context::Folders,
                // And another: a rule being written.
                Context::NewRule => bridge::Context::Folders,
                // A question in the host's sheet, as a certificate is.
                Context::RecordKeys => bridge::Context::Certificate,
                Context::EditTheme => bridge::Context::Certificate,
                // A question to answer before anything is sent, as a
                // certificate is.
                Context::Unsubscribe => bridge::Context::Certificate,
                // And so is one to answer before an account is removed.
                Context::RemoveAccount | Context::LetAgentsIn => bridge::Context::Certificate,
                Context::Folders => bridge::Context::Folders,
                // A list to narrow by typing, over the message.
                Context::Templates => bridge::Context::Folders,
                // A list to choose from, over the message.
                Context::Suggestions => bridge::Context::Folders,
                // A list to narrow by typing, over the message.
                Context::Hooks => bridge::Context::Folders,
                Context::Compose => bridge::Context::Compose,
                Context::Search => bridge::Context::Search,
                Context::Palette => bridge::Context::Palette,
            },
            enabled: current
                .enabled
                .iter()
                .filter_map(|id| env.registry.get(*id))
                .map(|spec| spec.name.to_owned())
                .collect(),
            checked: current
                .checked
                .iter()
                .filter_map(|id| env.registry.get(*id))
                .map(|spec| spec.name.to_owned())
                .collect(),
            unread: current.unread,
            title: current.title.clone(),
            background: current.background.map(terminal_color),
            foreground: current.foreground.map(terminal_color),
            appearance: match current.appearance {
                Appearance::System => bridge::Appearance::System,
                Appearance::Light => bridge::Appearance::Light,
                Appearance::Dark => bridge::Appearance::Dark,
            },
            font: current.font.clone(),
            font_size: current.font_size,
        });
        self.last_host_state = Some(current);
    }

    /// Tells the host every key there is, for its window of them, where
    /// it has one: as Quill starts, and whenever the keys change.
    fn send_keys(&self, env: &Env) {
        if env.keys_window {
            self.host.keys(keys_list(env));
        }
    }

    /// Tells the host where its reading pane stands, when that has
    /// changed: how many columns the terminal keeps beside it, and how
    /// many the window had room for when that was worked out, so that
    /// the host can tell a place for the window as it is from one for the
    /// window as it was.
    fn mirror_place(&mut self, state: &State, env: &Env) {
        let current = state.reading_place(env).zip(state.window_columns);
        if current == self.last_place {
            return;
        }
        if let Some((columns, window)) = current {
            self.host.reading_place(columns, window);
        }
        self.last_place = current;
    }

    /// Tells the host where the editor that has the text of the message
    /// being written goes, and whether it has the keys, when either has
    /// changed. Only a host that runs editors itself is told.
    fn mirror_editor(&mut self, state: &State, env: &Env) {
        if !env.editor_pane {
            return;
        }
        let current = state.editor_spot(env);
        if self.last_editor == Some(current) {
            return;
        }
        // Nothing has been said yet and there is nothing to say.
        if self.last_editor.is_none() && current.place.is_none() {
            return;
        }
        let place = current
            .place
            .and_then(|rect| {
                bridge::EditorPlace::at(
                    bridge::EditorCells {
                        column: rect.x,
                        row: rect.y,
                        columns: rect.width,
                        rows: rect.height,
                        covered: current.covered,
                    },
                    current.keys,
                )
            })
            .unwrap_or_else(bridge::EditorPlace::hidden);
        self.host.editor_place(place);
        self.last_editor = Some(current);
    }

    /// Has the host show the form or question in front as it now is,
    /// where the host is the one that shows them: the whole form whenever
    /// any of it has changed, and word to put it away when it is over.
    /// One question giving way to another is one message: the sheet is
    /// given the new form where it stands, and is not taken down between.
    fn mirror_form(&mut self, state: &State, env: &Env) {
        let current = state.form(env);
        match sheet_change(self.last_form.as_ref(), current.as_ref()) {
            Some(SheetChange::Show(form)) => self.host.show_form(form.clone()),
            Some(SheetChange::Close) => self.host.close_form(),
            None => return,
        }
        self.last_form = current;
    }

    /// Tells the host what its settings window shows, while it is open:
    /// whenever any of it has changed, and whenever the host is owed it
    /// again because something was done there, changed or not.
    fn mirror_settings(&mut self, state: &State, env: &Env) {
        let current = state.settings_shown(env);
        if current == self.last_settings {
            return;
        }
        if let Some((_, settings)) = &current {
            self.host.show_settings(settings.clone());
        }
        self.last_settings = current;
    }

    /// Runs one of the user's programs on a message, off the UI task, and
    /// says how it went. Never in Quill's own app, whatever asked.
    fn run_hook(&self, run: hook::HookRun) {
        let injected = self.injected.clone();
        if self.host.is_shell() {
            let _ = injected.send(Action::HookDone {
                name: run.name,
                outcome: hook::HookOutcome::not_allowed(),
            });
            return;
        }
        let closing = Arc::clone(&self.closing);
        tokio::task::spawn_blocking(move || {
            let outcome = hook::run(&run, hook::Limits::default(), &closing);
            let _ = injected.send(Action::HookDone {
                name: run.name,
                outcome,
            });
        });
    }

    /// Gets a picture ready to draw: from the terminal's own memory if it
    /// has it, by decoding it, or by asking the host for a picture of it.
    fn load_picture(&mut self, id: u32, path: PathBuf, content_type: &str) {
        if let Some(size) = self.images.held(id) {
            let _ = self.injected.send(Action::PictureLoaded {
                id,
                size: Some(size),
            });
        } else if quill_media::decode::can_decode(content_type) {
            self.decode_picture(id, path);
        } else if self.host.is_shell() {
            // A kind only the system can read. The host writes a PNG of it
            // beside the file, and that is decoded like any other.
            let out = path.with_file_name(format!("picture-{id}.png"));
            self.thumbnails.insert(u64::from(id), out.clone());
            self.host
                .thumbnail(u64::from(id), &path, &out, PICTURE_PIXELS.0);
        } else {
            let _ = self.injected.send(Action::PictureLoaded { id, size: None });
        }
    }

    /// The host has answered a request for a picture of a file.
    fn thumbnail_made(&mut self, id: u64, ok: bool) {
        let Some(path) = self.thumbnails.remove(&id) else {
            return;
        };
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        if ok {
            self.decode_picture(id, path);
        } else {
            let _ = self.injected.send(Action::PictureLoaded { id, size: None });
        }
    }

    /// Reads and decodes a picture file off the UI task, and says when the
    /// pixels are ready to send.
    fn decode_picture(&self, id: u32, path: PathBuf) {
        let decoded = Arc::clone(&self.images.decoded);
        let injected = self.injected.clone();
        tokio::task::spawn_blocking(move || {
            let image = std::fs::read(&path).ok().and_then(|bytes| {
                quill_media::decode::decode(&bytes, PICTURE_PIXELS.0, PICTURE_PIXELS.1)
            });
            let size = image.map(|image| {
                let size = (image.width, image.height);
                if let Ok(mut decoded) = decoded.lock() {
                    decoded.insert(id, image);
                }
                size
            });
            let _ = injected.send(Action::PictureLoaded { id, size });
        });
    }
}

/// A colour as the bridge says it.
fn terminal_color((red, green, blue): (u8, u8, u8)) -> bridge::TerminalColor {
    bridge::TerminalColor::rgb(red, green, blue)
}

/// Every key as the host's window of them shows it: what the list drawn
/// over the panes has, under the same headings and in the same order.
pub fn keys_list(env: &Env) -> bridge::KeysList {
    let mut sections: Vec<bridge::KeySection> = Vec::new();
    for line in view::help::all_lines(&env.registry, &env.keymap, env.web_pane) {
        match line {
            view::help::HelpLine::Heading(title) => sections.push(bridge::KeySection {
                title: title.to_owned(),
                keys: Vec::new(),
            }),
            view::help::HelpLine::Entry { id, title, .. } => {
                if let Some(section) = sections.last_mut() {
                    section.keys.push(bridge::KeyLine {
                        title: title.to_owned(),
                        keys: env
                            .keymap
                            .keys_for(id)
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                    });
                }
            }
            view::help::HelpLine::Blank => {}
        }
    }
    bridge::KeysList {
        title: view::help::ALL_KEYS_TITLE.to_owned(),
        sections,
    }
}

/// Every command as the host sees it: its name, title, keys and placement.
pub fn command_infos(env: &Env) -> Vec<CommandInfo> {
    env.registry
        .specs()
        .iter()
        .map(|spec| CommandInfo {
            name: spec.name.to_owned(),
            title: spec.title.to_owned(),
            keys: env
                .keymap
                .keys_for(spec.id)
                .iter()
                .map(ToString::to_string)
                .collect(),
            menu: spec.menu.map(|menu| match menu {
                Menu::App => bridge::Menu::App,
                Menu::File => bridge::Menu::File,
                Menu::Edit => bridge::Menu::Edit,
                Menu::Mailbox => bridge::Menu::Mailbox,
                Menu::View => bridge::Menu::View,
                Menu::Message => bridge::Menu::Message,
                Menu::Rules => bridge::Menu::Rules,
                Menu::Help => bridge::Menu::Help,
            }),
            symbol: spec.symbol.map(str::to_owned),
        })
        .collect()
}

/// Everything the loop waits on besides the keyboard.
struct Inbound {
    /// Answers from the store worker.
    data: UnboundedReceiver<DataEvent>,
    host: UnboundedReceiver<HostEvent>,
    sync: UnboundedReceiver<SyncEvent>,
    accounts: UnboundedReceiver<AccountEvent>,
    /// Actions that background work feeds back into the loop.
    injected: UnboundedReceiver<Action>,
    /// What is asked from outside the window.
    agents: UnboundedReceiver<AgentAsk>,
}

async fn event_loop(
    terminal: &mut terminal::Screen,
    env: &mut Env,
    notices: Vec<Notice>,
    io: &mut Io<'_>,
    inbound: &mut Inbound,
) -> io::Result<()> {
    let size = step("cannot read the terminal size", terminal.size())?;
    let mut state = State::new((size.width, size.height), now());
    // Before the first frame: what the host has decides what is drawn.
    state.update(env, Action::Resize(size.width, size.height));
    for notice in notices {
        state.update(env, Action::Notify(notice));
    }
    io.host.commands(command_infos(env));
    io.send_keys(env);
    for request in state.startup_requests() {
        io.data.send(request);
    }
    // The rules are files, and the service reads them: what they say is
    // asked for once, and told from then on whenever it changes.
    if let Some(service) = &io.accounts {
        let _ = service.send(crate::setup::AccountRequest::Rules(
            crate::setup::RulesAsk::Read,
        ));
    }

    // `None` while Vim has the terminal: its keys are not Quill's to read.
    let mut input = Some(EventStream::new());
    // Vim, while it runs on this terminal in Quill's place.
    let mut away: Option<EditorWait> = None;
    // Often enough that a draft being written is saved every few seconds.
    let mut clock = tokio::time::interval(Duration::from_secs(5));
    // When a key last arrived. Spelling is checked once the keys have
    // been quiet for a moment, never as each one is typed.
    let mut last_typed = tokio::time::Instant::now();
    // The move of the list's cursor that the web reading pane is waiting
    // out, and when it was made. A page is asked for once the cursor has
    // rested, never for each message passed on the way.
    let mut resting: Option<(u64, tokio::time::Instant)> = None;
    // Until when nothing is drawn, because the host is about to give the
    // terminal another width: the reading pane is coming or going beside
    // it. Drawing at the old width in the arrangement meant for the new
    // one would show the list at a width it has neither before nor after.
    // As Quill starts with web reading on, the wait is for the host to
    // say how wide its window is.
    let mut width_due = state
        .web_reading
        .then(|| tokio::time::Instant::now() + WIDTH_WAIT);
    loop {
        if width_due.is_some_and(|due| tokio::time::Instant::now() >= due) {
            width_due = None;
        }
        // Redraw only on change, never over Vim, and not at a width that
        // is about to change.
        if state.dirty && away.is_none() && width_due.is_none() {
            let mut overlays = view::Overlays::default();
            let frame = step(
                "cannot draw",
                terminal.draw(|frame| overlays = view::render(frame, &state, env)),
            )?;
            let size = (frame.area.width, frame.area.height);
            step("cannot mark a link", mark_links(&overlays.links))?;
            step(
                "cannot draw an image",
                io.images.sync(overlays.images, size),
            )?;
            state.dirty = false;
        }
        io.mirror(&state, env);
        io.mirror_form(&state, env);
        io.mirror_settings(&state, env);
        io.mirror_place(&state, env);
        io.mirror_editor(&state, env);

        let spelling_due = state
            .spell_check_due(env)
            .then(|| last_typed + TYPING_PAUSE);
        resting = state.settle_due().map(|moves| match resting {
            Some((timed, since)) if timed == moves => (moves, since),
            _ => (moves, tokio::time::Instant::now()),
        });
        let settle_due = resting.map(|(_, since)| since + SETTLE_PAUSE);
        // A pointer held still says nothing, so while one is held past
        // the edge of the text it is asked about at a steady pace.
        let drag_due = state
            .drag_scrolls(env)
            .then(|| tokio::time::Instant::now() + DRAG_PACE);
        // The mark beside a syncing account turns while there is one to
        // turn and a pane to turn it in.
        let spin_due = state
            .spinning(env)
            .then(|| tokio::time::Instant::now() + SPIN_PACE);
        let action = tokio::select! {
            event = next_input(&mut input) => match event {
                Some(Ok(event)) => translate(event),
                Some(Err(err)) => return step("cannot read input", Err(err)),
                None => break,
            },
            ended = editor_ended(&mut away) => {
                away = None;
                // The terminal is Quill's again: as at the start, and with
                // everything drawn afresh.
                *terminal = step("cannot take the terminal back", retake(env.mouse))?;
                input = Some(EventStream::new());
                let size = step("cannot read the terminal size", terminal.size())?;
                state.update(env, Action::Resize(size.width, size.height));
                state.dirty = true;
                Some(match ended {
                    Ok(status) => Action::EditorExited {
                        ran: true,
                        // Killed by a signal is not a clean exit.
                        status: status.code().unwrap_or(-1),
                    },
                    Err(_) => Action::EditorExited { ran: true, status: -1 },
                })
            }
            Some(event) = inbound.data.recv() => match event {
                // An answer for someone outside the window: theirs, and
                // nothing for the UI.
                DataEvent::AgentDone { ticket, result, notice, then } => {
                    // Shown in the window, when that is what was asked.
                    if let Some(show) = io.agent_done(ticket, *result, then) {
                        let _ = io.injected.send(show);
                    }
                    // What was changed is told to the user, as far as
                    // they asked to be.
                    notice.map(Action::AgentNotice)
                }
                DataEvent::AgentArrived(found) => {
                    io.agents.arrived(found);
                    None
                }
                event => Some(Action::Data(event)),
            },
            Some(ask) = inbound.agents.recv() => io.agent_ask(env, &state, ask),
            Some(event) = inbound.host.recv() => match event {
                HostEvent::RunCommand(name) => Some(match env.registry.by_name(&name) {
                    Some(spec) => Action::Command(spec.id),
                    None => Action::Notify(Notice::error(format!(
                        "The app asked for an unknown command \"{name}\"."
                    ))),
                }),
                HostEvent::Files { paths, temporary } => {
                    Some(Action::Files { paths, temporary })
                }
                HostEvent::Saved { name, saved } => Some(Action::FileSaved { name, saved }),
                HostEvent::OpenMailto(url) => Some(Action::Mailto(url)),
                HostEvent::Spelling { id, words } => Some(Action::Spelling { id, words }),
                HostEvent::Suggestions { id, words } => {
                    Some(Action::Suggestions { id, words })
                }
                HostEvent::EditorExited { ran, status } => {
                    Some(Action::EditorExited { ran, status })
                }
                HostEvent::NotificationUsed { id, action } => {
                    Some(Action::Notification { id, action })
                }
                HostEvent::FormUsed(used) => Some(Action::FormUsed(used)),
                HostEvent::WindowColumns(columns) => Some(Action::WindowColumns(columns)),
                HostEvent::ReadingDragged(columns) => Some(Action::ReadingDragged(columns)),
                HostEvent::SettingsOpened => Some(Action::Settings(SettingsAct::Opened)),
                HostEvent::SettingsClosed => Some(Action::Settings(SettingsAct::Closed)),
                HostEvent::SettingsUsed(used) => Some(Action::Settings(SettingsAct::Used(used))),
                HostEvent::SystemAppearance { dark } => Some(Action::SystemAppearance { dark }),
                HostEvent::ThemeImported(name) => Some(Action::ThemeImported(name)),
                HostEvent::Thumbnail { id, ok } => {
                    io.thumbnail_made(id, ok);
                    None
                }
                // Either way there is nothing left to draw for.
                HostEvent::WillQuit | HostEvent::Disconnected => break,
                HostEvent::Problem(text) => Some(Action::Notify(Notice::error(text))),
            },
            Some(event) = inbound.sync.recv() => {
                io.agent_heard(env, &event);
                Some(Action::Sync(event))
            }
            Some(event) = inbound.accounts.recv() => {
                io.agents.heard_service(&event);
                Some(Action::Account(event))
            }
            Some(action) = inbound.injected.recv() => Some(action),
            _ = clock.tick() => {
                io.agent_settle(true);
                Some(Action::ClockTick(now()))
            }
            () = quiet_until(spelling_due) => Some(Action::TypingPaused),
            () = quiet_until(settle_due) => Some(Action::SelectionSettled),
            () = quiet_until(drag_due) => Some(Action::DragHeld),
            () = quiet_until(spin_due) => Some(Action::Spin),
            // The new width did not come: draw at the one there is.
            () = quiet_until(width_due) => None,
        };
        if matches!(action, Some(Action::Resize(..) | Action::WindowColumns(_))) {
            width_due = None;
        }
        if matches!(action, Some(Action::Key(_) | Action::Paste(_))) {
            last_typed = tokio::time::Instant::now();
        }
        if let Some(action) = action {
            let web = state.web_reading;
            let (accounts_hidden, place) = (state.sidebar_hidden, state.reading_place(env));
            let mut effects = state.update(env, action);
            // Settings that hold from now are put where the next update
            // and the next frame find them. With other keys, the host is
            // told every command's keys again, for its menus.
            if let Some(settings) = effects.settings.take() {
                if settings.apply(env) {
                    io.host.commands(command_infos(env));
                    io.send_keys(env);
                }
                state.dirty = true;
            }
            // And what the host says the system's appearance is: with the
            // settings saying "as the system is", another theme is drawn
            // with from the next frame.
            if let Some(dark) = effects.system_dark.take() {
                env.look.system_dark = Some(dark);
                env.restyle();
                state.dirty = true;
            }
            // And a theme being made, which is drawn with as its colours
            // are chosen, until it is saved or given up.
            // And the agents stopped: both ways in are off from this
            // moment, and whoever is connected is hung up on.
            if std::mem::take(&mut effects.agents_stopped) {
                env.agent.commands = false;
                env.agent.mcp = false;
                if let Some(hang_up) = &io.agents.hang_up {
                    hang_up();
                }
            }
            if let Some(trying) = effects.trying.take() {
                env.look.trying = trying;
                env.restyle();
                state.dirty = true;
            }
            // The host asked where its reading pane goes: what it was
            // last told is forgotten, so that it is told again.
            if std::mem::take(&mut effects.place_again) {
                io.last_place = None;
            }
            io.perform(effects);
            if state.web_reading != web {
                width_due = state
                    .resize_expected()
                    .then(|| tokio::time::Instant::now() + WIDTH_WAIT);
            } else if state.sidebar_hidden != accounts_hidden && state.reading_place(env) != place {
                // The accounts have come or gone beside the host's pane,
                // and the terminal is about to be that much wider or
                // narrower: wait for it, as above.
                width_due = Some(tokio::time::Instant::now() + WIDTH_WAIT);
            }
        }
        if let Some(wanted) = io.editor_wanted.take() {
            let command = editor_command(env, &wanted);
            let started = if io.host.is_shell() {
                // The app runs it, in a terminal of its own over the
                // text's place: the same command, handed over whole.
                command.as_ref().and_then(vim::argv).is_some_and(|argv| {
                    io.host
                        .edit_draft(&wanted.dir, &wanted.title, wanted.editor.kind(), argv)
                })
            } else {
                match command {
                    Some(command) => match run_editor(terminal, env, io, &mut input, command)? {
                        Some(wait) => {
                            away = Some(wait);
                            true
                        }
                        None => false,
                    },
                    None => false,
                }
            };
            if !started {
                // Nothing ran, and the terminal was never given up.
                {
                    let effects = state.update(
                        env,
                        Action::EditorExited {
                            ran: false,
                            status: 0,
                        },
                    );
                    io.perform(effects);
                }
            }
        }
        if state.should_quit {
            break;
        }
    }
    // A message being written is not lost by quitting: it is saved as a
    // draft, and the store is given time to write it down.
    for request in state.parting_requests() {
        io.data.send(request);
    }
    io.data.finish();
    Ok(())
}

/// Vim running on this terminal: the wait for it to end, off the UI task.
type EditorWait = tokio::task::JoinHandle<io::Result<ExitStatus>>;

/// The next thing from the keyboard, or nothing for ever while the
/// terminal is not Quill's to read.
async fn next_input(input: &mut Option<EventStream>) -> Option<io::Result<Event>> {
    match input {
        Some(input) => input.next().await,
        None => std::future::pending().await,
    }
}

/// Waits for Vim to end, or for ever when it is not running.
async fn editor_ended(away: &mut Option<EditorWait>) -> io::Result<ExitStatus> {
    match away {
        Some(wait) => wait.await.map_err(io::Error::other)?,
        None => std::future::pending().await,
    }
}

/// Gives the terminal to the user's Vim, started on the draft at `path`.
/// Returns the wait for it, or `None` when there is no Vim to run or it
/// would not start: then the terminal is Quill's as before. An error is
/// the terminal not coming back.
///
/// Vim is a child on the same terminal, started from an argv and never
/// through a shell. Everything else Quill does carries on meanwhile: mail
/// is fetched and sent; only the screen and the keys are Vim's.
fn run_editor(
    terminal: &mut terminal::Screen,
    env: &Env,
    io: &mut Io<'_>,
    input: &mut Option<EventStream>,
    mut command: std::process::Command,
) -> io::Result<Option<EditorWait>> {
    // Stop reading the keyboard: what is typed from here on is the
    // editor's. Dropping the stream tells its reader thread to stop: a
    // flag, and one wake for whichever look at the keyboard comes next.
    //
    // Nothing here looks at the keyboard after that, and it matters. The
    // reader thread may have been asked to look and not have started yet.
    // A look from here would take the wake that was meant for it; the
    // thread would then start, find nothing to wake it, and wait on the
    // keyboard for ever with crossterm's reader in its hand, eating what
    // is typed for the editor. Taking the terminal back would then stop
    // dead at making a new stream, with the screen cleared and nothing
    // drawn. Left alone, the wake is the thread's whenever it starts.
    // What was typed and not read is cleared when the terminal is taken
    // back ([`settle_keyboard`]).
    *input = None;
    // Pictures are drawn over the screen being left: take them down.
    let _ = io.images.sync(Vec::new(), io.images.size);
    let _ = terminal.show_cursor();
    terminal::leave();
    match command.spawn() {
        Ok(mut child) => Ok(Some(tokio::task::spawn_blocking(move || child.wait()))),
        Err(_) => {
            // It was there and would not start. Take the terminal back,
            // once the reader that was stopped has had a moment to go:
            // no editor ran to give it one.
            std::thread::sleep(Duration::from_millis(20));
            *terminal = step("cannot take the terminal back", retake(env.mouse))?;
            *input = Some(EventStream::new());
            Ok(None)
        }
    }
}

/// Takes the terminal over again after Vim, as at the start: the same
/// modes, an empty screen, and a frame with nothing in it, so that the
/// next draw writes everything.
fn retake(mouse: bool) -> io::Result<terminal::Screen> {
    let terminal = terminal::enter(mouse)?;
    // Asked of the terminal directly: nothing here waits on an answer
    // from it.
    ratatui::crossterm::execute!(stdout(), Clear(ClearType::All))?;
    settle_keyboard();
    Ok(terminal)
}

/// Clears what is left of the keyboard from before an editor had the
/// terminal, so that a new stream starts clean: keys that were typed and
/// never read, which are not kept for later, and the wake left by the
/// reader that was stopped, if nothing took it. A stream that met that
/// wake on its first look would report it as a failure to read.
///
/// Only once the editor has gone: by then the reader thread that the wake
/// was for has long had it (see [`run_editor`]).
fn settle_keyboard() {
    // A bound, so that a keyboard that cannot be read at all is the new
    // stream's to report, not a loop here.
    for _ in 0..64 {
        match ratatui::crossterm::event::poll(Duration::ZERO) {
            Ok(true) => {
                let _ = ratatui::crossterm::event::read();
            }
            Ok(false) => break,
            // The wake: taken by looking, so look again.
            Err(_) => {}
        }
    }
}

/// How long the keys are quiet before the spelling of what was typed is
/// checked.
const TYPING_PAUSE: Duration = Duration::from_millis(400);

/// How long the list's cursor rests on a message before the web reading
/// pane shows it: longer than the gap between the repeats of a held key,
/// so that holding `j` renders nothing on the way, and short enough that a
/// single press still feels answered at once.
const SETTLE_PAUSE: Duration = Duration::from_millis(150);

/// How long the host is given to change the terminal's width, once web
/// reading has been switched, before the panes are drawn at the width
/// there is. The host does it in a few milliseconds; a page that is slow
/// to make is not waited for.
const WIDTH_WAIT: Duration = Duration::from_millis(200);

/// How long the mark beside a syncing account stays as it is before it
/// turns on: eight turns a second.
const SPIN_PACE: Duration = Duration::from_millis(125);

/// How long between two lines of the text moving under a pointer that is
/// held still past its edge.
const DRAG_PACE: Duration = Duration::from_millis(60);

/// Waits until `at`, or for ever when there is nothing to wait for.
async fn quiet_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Says which step failed. A bare "Operation not permitted" is no help when
/// the sandbox, the terminal and the disk are all candidates.
fn step<T>(what: &str, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|err| io::Error::new(err.kind(), format!("{what}: {err}")))
}

fn translate(event: Event) -> Option<Action> {
    match event {
        Event::Key(key) => KeyChord::from_event(&key).map(Action::Key),
        Event::Resize(width, height) => Some(Action::Resize(width, height)),
        Event::Paste(text) => Some(Action::Paste(text)),
        Event::Mouse(mouse) => {
            let kind = match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => MouseKind::Press,
                MouseEventKind::Drag(MouseButton::Left) => MouseKind::Drag,
                MouseEventKind::Up(MouseButton::Left) => MouseKind::Release,
                MouseEventKind::ScrollUp => MouseKind::WheelUp,
                MouseEventKind::ScrollDown => MouseKind::WheelDown,
                // Other buttons and plain movement mean nothing here.
                _ => return None,
            };
            Some(Action::Mouse(Mouse {
                kind,
                column: mouse.column,
                row: mouse.row,
            }))
        }
        _ => None,
    }
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use quill_core::config::KeymapPreset;

    use super::*;
    use crate::command::Registry;
    use crate::keymap::Keymap;
    use crate::theme::{ColorMode, Styles, Theme};

    fn env() -> Env {
        Env {
            registry: Registry::builtin(),
            keymap: Keymap::preset(KeymapPreset::Vim),
            styles: Styles::new(Theme::terminal(), ColorMode::None),
            look: crate::theme::Look::one(Theme::terminal()),
            tz: TimeZone::UTC,
            reader_width: 0.5,
            program: None,
            sidebar_width: None,
            agent: quill_core::config::AgentConfig::default(),
            images: false,
            accounts: false,
            cmd_keys: true,
            mouse: false,
            file_panel: false,
            thumbnails: false,
            downloads: None,
            compose: crate::state::ComposeSettings::default(),
            identities: std::collections::HashMap::new(),
            remote_never: false,
            notifications: true,
            threads: true,
            searches: Vec::new(),
            templates: Vec::new(),
            spell_check: false,
            sheets: false,
            editor_pane: false,
            external: vim::ExternalEditor::Vim,
            editor_program: None,
            hooks: Vec::new(),
            hooks_enabled: false,
            web_pane: false,
            keys_window: false,
            window: false,
            web_reading: false,
            dark_pages: true,
            sidebar: true,
            editor_tips: true,
            cache_gb: 5.0,
            font: None,
            font_size: None,
        }
    }

    #[test]
    fn every_command_is_announced_with_its_keys_and_placement() {
        let env = env();
        let infos = command_infos(&env);
        assert_eq!(infos.len(), env.registry.specs().len());
        let find = |name: &str| infos.iter().find(|info| info.name == name).unwrap();

        let next = find("nav.next");
        assert_eq!(next.keys, ["j", "down"]);
        assert_eq!((next.menu, next.symbol.as_deref()), (None, None));

        let star = find("message.toggle_star");
        assert_eq!(star.menu, Some(bridge::Menu::Message));
        assert_eq!(star.symbol.as_deref(), Some("star"));
        assert_eq!(find("nav.top").keys[0], "g g");
        // Quit and Close Window are the shell's to offer.
        assert_eq!(find("app.quit").menu, None);
        assert_eq!(find("app.quit").keys, ["Q"]);
        assert_eq!(find("window.close").menu, None);
        assert_eq!(find("window.close").keys, ["q", "ctrl-c"]);
        assert_eq!(find("help.toggle").menu, Some(bridge::Menu::Help));
    }

    #[test]
    fn the_host_hears_of_its_sheet_only_when_it_changes_and_never_of_a_gap() {
        let form = |step: &str, busy: Option<&str>| {
            bridge::FormShow::new(bridge::FormShowParams {
                step: step.to_owned(),
                title: "Add an Account".to_owned(),
                busy: busy.map(str::to_owned),
                ..bridge::FormShowParams::default()
            })
            .unwrap()
        };
        let (saving, signing_in) = (form("server", Some("Saving…")), form("sign_in", None));
        assert_eq!(sheet_change(None, None), None);
        assert_eq!(
            sheet_change(None, Some(&saving)),
            Some(SheetChange::Show(&saving))
        );
        assert_eq!(sheet_change(Some(&saving), Some(&saving)), None);
        // Any of it changing is the whole form again.
        let saved = form("server", None);
        assert_eq!(
            sheet_change(Some(&saving), Some(&saved)),
            Some(SheetChange::Show(&saved))
        );
        // One question in another's place is one message: the new form,
        // with no word to take the sheet down in between.
        assert_eq!(
            sheet_change(Some(&saving), Some(&signing_in)),
            Some(SheetChange::Show(&signing_in))
        );
        assert_eq!(
            sheet_change(Some(&signing_in), None),
            Some(SheetChange::Close)
        );
    }

    #[test]
    fn the_hosts_list_of_keys_is_the_list_drawn_here_under_the_same_headings() {
        let env = env();
        let list = keys_list(&env);
        assert_eq!(list.title, "Keyboard shortcuts");
        // Every command the list drawn here has, once, with its keys.
        let drawn = view::help::all_lines(&env.registry, &env.keymap, env.web_pane);
        let headings: Vec<&str> = drawn
            .iter()
            .filter_map(|line| match line {
                view::help::HelpLine::Heading(title) => Some(*title),
                _ => None,
            })
            .collect();
        assert_eq!(
            list.sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            headings
        );
        assert!(headings.contains(&"Writing a message"));
        let entries = drawn
            .iter()
            .filter(|line| matches!(line, view::help::HelpLine::Entry { .. }))
            .count();
        let lines: Vec<&bridge::KeyLine> = list
            .sections
            .iter()
            .flat_map(|section| &section.keys)
            .collect();
        assert_eq!(lines.len(), entries);
        assert!(list.sections.iter().all(|section| !section.keys.is_empty()));
        let down = lines
            .iter()
            .find(|line| line.title == "Next item or line")
            .map(|line| line.keys.clone());
        assert_eq!(down, Some(vec!["j".to_owned(), "down".to_owned()]));
        // And it is a message the bridge can carry.
        let message = bridge::CoreToShell::KeysList(list);
        let line = bridge::encode(&message).unwrap();
        assert_eq!(
            bridge::decode::<bridge::CoreToShell>(&line).unwrap(),
            message
        );
    }

    #[test]
    fn announced_commands_are_valid_bridge_messages() {
        let message = bridge::CoreToShell::CommandsList(bridge::CommandsList {
            commands: command_infos(&env()),
        });
        let line = bridge::encode(&message).unwrap();
        assert_eq!(
            bridge::decode::<bridge::CoreToShell>(&line).unwrap(),
            message
        );
    }

    /// Someone outside, waiting on a server for `what`.
    fn waiting_for(
        agents: &mut Agents,
        what: WaitingFor,
        until: tokio::time::Instant,
    ) -> oneshot::Receiver<Result<Answer, Refusal>> {
        let (reply, heard) = oneshot::channel();
        agents.waiting.push(Waiting {
            what,
            answer: Answer {
                value: serde_json::json!({ "done": true }),
                lines: Vec::new(),
            },
            until,
            reply,
        });
        heard
    }

    #[tokio::test]
    async fn whoever_asked_for_a_folder_is_answered_when_the_server_has() {
        let mut agents = Agents::default();
        let later = tokio::time::Instant::now() + SERVER_WAIT;
        let folder = |name: &str| WaitingFor::Folder {
            account: "Work".to_owned(),
            name: name.to_owned(),
        };
        let mut made = waiting_for(&mut agents, folder("Projects"), later);
        let mut refused = waiting_for(&mut agents, folder("Receipts"), later);
        let mut again = waiting_for(&mut agents, folder("Projects"), later);

        // News of other things, other folders and other accounts answers
        // nobody.
        agents.heard_engine(&SyncEvent::Changed);
        agents.heard_engine(&SyncEvent::FolderMade {
            account: "Home".to_owned(),
            name: "Projects".to_owned(),
        });
        agents.heard_engine(&SyncEvent::Problem {
            account: "Work".to_owned(),
            message: "The folder Projects/2026 could not be made on the server: no".to_owned(),
        });
        assert_eq!(agents.waiting.len(), 3);

        // Made: the one who asked first is told so, and only that one.
        agents.heard_engine(&SyncEvent::FolderMade {
            account: "Work".to_owned(),
            name: "Projects".to_owned(),
        });
        assert_eq!(
            made.try_recv().unwrap().unwrap().value,
            serde_json::json!({ "done": true })
        );
        assert!(again.try_recv().is_err());
        // Not made: the reason is the answer, as a refusal.
        agents.heard_engine(&SyncEvent::Problem {
            account: "Work".to_owned(),
            message: "The folder Receipts could not be made on the server: it exists".to_owned(),
        });
        let why = refused.try_recv().unwrap().unwrap_err();
        assert_eq!(why.code, crate::agent::Code::Failed);
        assert!(why.message.ends_with("it exists"));

        // A server that never says is given up on, and that is said as
        // not known, which is what it is.
        agents.give_up(later - Duration::from_secs(1));
        assert_eq!(agents.waiting.len(), 1);
        agents.give_up(later);
        assert!(agents.waiting.is_empty());
        let unknown = again.try_recv().unwrap().unwrap_err();
        assert!(unknown.message.contains("not known"));
    }

    #[tokio::test]
    async fn whoever_asked_a_list_to_unsubscribe_is_told_how_it_went() {
        let mut agents = Agents::default();
        let later = tokio::time::Instant::now() + SERVER_WAIT;
        let list = |host: &str| WaitingFor::Unsubscribed {
            host: host.to_owned(),
        };
        let mut fine = waiting_for(&mut agents, list("lists.example.com"), later);
        let mut failed = waiting_for(&mut agents, list("news.example.com"), later);
        agents.heard_service(&AccountEvent::Unsubscribed {
            host: "other.example.com".to_owned(),
            result: Ok(()),
        });
        assert_eq!(agents.waiting.len(), 2);
        agents.heard_service(&AccountEvent::Unsubscribed {
            host: "news.example.com".to_owned(),
            result: Err("the server answered 500".to_owned()),
        });
        let why = failed.try_recv().unwrap().unwrap_err();
        assert!(why.message.contains("news.example.com"));
        assert!(why.message.contains("500"));
        agents.heard_service(&AccountEvent::Unsubscribed {
            host: "lists.example.com".to_owned(),
            result: Ok(()),
        });
        assert!(fine.try_recv().unwrap().is_ok());
        assert!(agents.waiting.is_empty());
    }

    #[tokio::test]
    async fn each_watcher_is_told_of_its_own_mail_until_commands_are_switched_off() {
        let mut agents = Agents::default();
        let mut watch = |id: u64| {
            let (lines, heard) = unbounded_channel();
            agents.watchers.push(Watcher {
                id,
                query: String::new(),
                lines,
            });
            heard
        };
        let mut first = watch(1);
        let mut second = watch(2);
        agents.arrived(Some(vec![
            (
                2,
                vec![
                    serde_json::json!({ "id": 7 }),
                    serde_json::json!({ "id": 8 }),
                ],
            ),
            // One that has gone meanwhile: nobody to tell.
            (3, vec![serde_json::json!({ "id": 9 })]),
        ]));
        assert!(first.try_recv().is_err());
        assert_eq!(second.try_recv().unwrap()["id"], 7);
        assert_eq!(second.try_recv().unwrap()["id"], 8);
        // Switched off: every watcher is let go, which each hears as its
        // line being closed.
        agents.arrived(None);
        assert!(agents.watchers.is_empty());
        assert_eq!(
            first.try_recv().unwrap_err(),
            tokio::sync::mpsc::error::TryRecvError::Disconnected
        );
    }

    #[test]
    fn the_engine_says_which_problem_is_a_folder_that_was_not_made() {
        let trouble = quill_sync::is_folder_trouble;
        assert!(trouble(
            "The folder A/B could not be made on the server: no",
            "A/B"
        ));
        assert!(trouble(
            "The folder A was not made: its account is not being synced.",
            "A"
        ));
        // Another folder whose name begins the same, and other problems.
        assert!(!trouble(
            "The folder A/B could not be made on the server: no",
            "A"
        ));
        assert!(!trouble(
            "The folder was not changed: there is no server.",
            "A"
        ));
        assert!(!trouble("Work: the connection was lost", "Work"));
    }
}
