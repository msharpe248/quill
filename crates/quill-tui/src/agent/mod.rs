//! What can be done to the mail from outside Quill's window.
//!
//! The commands for a shell (`quill search`, `quill move`, ...) and the
//! tools an agent's program is given over MCP are two fronts over the
//! actions here: the same names, the same arguments, the same answers and
//! the same refusals. An action is a [`Call`]; [`perform`] checks it
//! against the user's settings, carries it out through the queue the UI's
//! own actions use, and writes it in the log.
//!
//! Three things hold for everything in this module:
//!
//! - **Mail is untrusted.** What a message says is returned as data, in
//!   fields of its own, and is never read as an instruction. Nothing is
//!   fetched or opened because a message asked.
//! - **Nothing is lost for good.** No action here deletes mail
//!   permanently: `delete` moves to a trash folder and refuses where the
//!   UI's delete would be for good.
//! - **Nothing here reads a secret**, changes an account, writes the
//!   config, or touches rules or hooks.

mod change;
mod look;
mod select;
mod show;
pub mod tools;
mod write;

pub use write::stop;

/// What taking a change back came to, for the UI to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenBack {
    /// What to say of it.
    pub said: String,
    /// The user's undo would put it back as it was, for a while.
    pub undoable: bool,
    /// Something was queued for a server.
    pub queued: bool,
}

/// Takes back the change in the log numbered `entry`, for the user: what
/// a rule did, chosen from the list of what rules did. The same as
/// `quill undo`, without a line of its own in the log.
pub fn take_back(store: &mut Store, entry: i64, now: i64) -> Result<TakenBack, String> {
    change::undo_entry(store, Some(entry), now)
        .map(|done| TakenBack {
            said: done
                .lines
                .first()
                .cloned()
                .unwrap_or_else(|| "Taken back.".to_owned()),
            undoable: done.undoable,
            queued: done.queued,
        })
        .map_err(|refusal| refusal.message)
}

use std::path::PathBuf;

use jiff::tz::TimeZone;
use quill_core::config::AgentConfig;
use quill_core::types::{LocalAccountId, LocalFolderId, LocalMessageId};
use quill_store::{NewAgentEntry, Store, StoreError};
use serde_json::{Value, json};

use crate::data::DataEvent;
pub use look::arrived;
pub use tools::{Call, Hint, Kind, Level, Mark, Param, Tool, Which};

/// The version of the answers' shapes (`schema/agent.json`). A field is
/// added freely; one is renamed or removed only with a new number.
pub const SCHEMA: u32 = 1;

/// How long a line stays in the log.
const LOG_DAYS: i64 = 90;

/// Which way in a request came by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// A command typed at a shell, or run by a skill.
    Shell,
    /// An agent's program, over MCP.
    Mcp,
}

impl Way {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Mcp => "mcp",
        }
    }
}

/// Who is asking: the way in, and the name they gave. The name is their
/// own claim and is only ever shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub way: Way,
    pub name: String,
}

impl Caller {
    /// A command for a shell.
    pub fn shell() -> Self {
        Self {
            way: Way::Shell,
            name: "A shell command".to_owned(),
        }
    }

    /// An MCP client, by the name it gave, made safe to show and kept to
    /// a line.
    pub fn mcp(name: &str) -> Self {
        let name = crate::text::sanitize_line(name);
        let name = crate::text::truncate(name.trim(), 40);
        Self {
            way: Way::Mcp,
            name: if name.is_empty() {
                "An agent".to_owned()
            } else {
                name
            },
        }
    }
}

/// What an action needs besides the store: the user's settings for
/// agents, and the few things of theirs an answer is made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub agent: AgentConfig,
    /// The user's time zone: for dates typed in a search, and for times
    /// said in words.
    pub tz: TimeZone,
    /// A private folder for files an agent asks to have written out. It is
    /// the caller's to make. `None`: there is nowhere.
    pub files: Option<PathBuf>,
    /// Who the user is on each account, by the account's name: the name
    /// that goes with its address on mail sent.
    pub from_names: Vec<(String, String)>,
    /// Mail is written as Markdown, and sent with an HTML version beside
    /// the plain text.
    pub markdown: bool,
    /// An answer is written above what it answers.
    pub reply_above: bool,
    /// The Quill that is running is asking: there is a window to show
    /// things in. A command working on the store by itself has none.
    pub running: bool,
    /// And it has an engine and a service that reach the accounts'
    /// servers and the network. The demo mailbox has neither.
    pub online: bool,
}

impl Settings {
    pub fn new(agent: AgentConfig, tz: TimeZone) -> Self {
        Self {
            agent,
            tz,
            files: None,
            from_names: Vec::new(),
            markdown: true,
            reply_above: true,
            running: false,
            online: false,
        }
    }
}

/// What is still to be done once a request has been checked, done as far
/// as the store goes and written in the log: by whoever has what it
/// needs. The window, the engine and the network are not the store's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Show this message in the window, in its folder's list.
    Show {
        message: LocalMessageId,
        folder: LocalFolderId,
    },
    /// Make a folder of this name on the account's server.
    MakeFolder {
        account: LocalAccountId,
        account_name: String,
        name: String,
    },
    /// Tell a mailing list's server to unsubscribe the user: one request
    /// to this address, which the list gave for it.
    Unsubscribe { url: String, host: String },
    /// Go on telling whoever asked of the mail that arrives.
    Watch { query: String },
}

/// Why something was not done. `code` does not change between versions;
/// `message` is for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub message: String,
}

/// The kinds of refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// The user's settings do not allow it.
    SwitchedOff,
    /// More messages than one change may touch.
    OverLimit,
    /// What was named is not there.
    NotFound,
    /// The request cannot be read: a missing or mistaken argument.
    Invalid,
    /// It needs the Quill that is running, and none is.
    NotRunning,
    /// It cannot be taken back.
    CannotUndo,
    /// Quill tried and could not.
    Failed,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SwitchedOff => "switched_off",
            Self::OverLimit => "over_limit",
            Self::NotFound => "not_found",
            Self::Invalid => "invalid",
            Self::NotRunning => "not_running",
            Self::CannotUndo => "cannot_undo",
            Self::Failed => "failed",
        }
    }

    /// The code a word names.
    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::SwitchedOff,
            Self::OverLimit,
            Self::NotFound,
            Self::Invalid,
            Self::NotRunning,
            Self::CannotUndo,
            Self::Failed,
        ]
        .into_iter()
        .find(|code| code.as_str() == word)
    }
}

impl Refusal {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Code::Invalid, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Code::NotFound, message)
    }

    /// As an answer: `{"schema": 1, "refused": {"code", "message"}}`.
    pub fn value(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "refused": { "code": self.code.as_str(), "message": self.message },
        })
    }
}

impl From<StoreError> for Refusal {
    fn from(err: StoreError) -> Self {
        Self::new(Code::Failed, err.to_string())
    }
}

/// What was done, or found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The answer as data: an object, with `schema` in it.
    pub value: Value,
    /// The answer for reading in a terminal, a line at a time, made safe
    /// to print.
    pub lines: Vec<String>,
}

/// Something done that the user should hear of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// What was done, in a line, with who did it.
    pub text: String,
    /// Mail was sent, or is waiting to be let go.
    pub send: bool,
}

/// All that comes of one request.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub result: Result<Answer, Refusal>,
    /// For the UI of the Quill that is running: views to reload, the
    /// engine to wake, words for the bottom bar.
    pub events: Vec<DataEvent>,
    /// For a notification, when the mail was changed or sent.
    pub notice: Option<Notice>,
    /// What whoever holds the window, the engine and the network is to
    /// do before the answer is given.
    pub then: Option<Then>,
}

impl Outcome {
    pub fn refused(refusal: Refusal) -> Self {
        Self {
            result: Err(refusal),
            events: Vec::new(),
            notice: None,
            then: None,
        }
    }
}

/// What one action came to, before it is logged and wrapped.
pub(crate) struct Done {
    /// The answer's fields.
    pub value: Value,
    pub lines: Vec<String>,
    /// How many messages were changed, or came back.
    pub count: usize,
    /// What was asked and what came of it, in a line for the log. Never
    /// what a message said.
    pub summary: String,
    /// A change: what is needed to take it back.
    pub undo: Option<Value>,
    /// What the UI is to say of it in its bottom bar, when anything was
    /// changed: the caller's name goes before it.
    pub said: Option<String>,
    /// Whether `U` in Quill would take it back, for a while.
    pub undoable: bool,
    /// Changes were queued for a server.
    pub queued: bool,
    /// A message sent: when it is due to go.
    pub due: Option<i64>,
    /// What is still to be done, by whoever can.
    pub then: Option<Then>,
}

impl Done {
    /// Something looked at.
    pub fn look(value: Value, lines: Vec<String>, count: usize, summary: String) -> Self {
        Self {
            value,
            lines,
            count,
            summary,
            undo: None,
            said: None,
            undoable: false,
            queued: false,
            due: None,
            then: None,
        }
    }
}

/// Whether this way in is open at all.
fn way_refused(settings: &Settings, way: Way) -> Option<Refusal> {
    match way {
        Way::Shell if !settings.agent.commands => Some(Refusal::new(
            Code::SwitchedOff,
            "Commands from a shell are switched off in Quill's settings (Agents).",
        )),
        Way::Mcp if !settings.agent.mcp => Some(Refusal::new(
            Code::SwitchedOff,
            "MCP is switched off in Quill's settings (Agents).",
        )),
        _ => None,
    }
}

/// Whether the settings allow `call` by this way in: `None` when they do.
pub fn refused(settings: &Settings, way: Way, call: &Call) -> Option<Refusal> {
    way_refused(settings, way).or_else(|| {
        let tool = call.tool();
        if tool.shell && way != Way::Shell {
            return Some(Refusal::invalid(format!(
                "{} is a command for a shell, and no tool.",
                tool.name
            )));
        }
        let level = tool.level;
        (!level.allowed(&settings.agent))
            .then(|| Refusal::new(Code::SwitchedOff, level.off_words()))
    })
}

/// Whether a way in is open at all, for what is asked more than once of
/// a connection that stays.
pub fn way_open(settings: &Settings, way: Way) -> bool {
    way_refused(settings, way).is_none()
}

/// The tools there are to call, as an MCP client is told of them: the
/// ones the settings allow. What is switched off is not listed, so that
/// it is not reached for. With the way in itself switched off they are
/// listed all the same: calling one is how its user learns why nothing
/// happens.
pub fn tools_for(settings: &Settings) -> Vec<Value> {
    tools::TOOLS
        .iter()
        .filter(|tool| !tool.shell && tool.level.allowed(&settings.agent))
        .map(Tool::described)
        .collect()
}

/// Carries out one request against the store: checks it, does it, logs
/// it. `now` is the moment it is done, in seconds since the Unix epoch.
///
/// [`Call::Sync`] is not done here: checking for mail is the engine's,
/// and whoever holds the engine answers it.
pub fn perform(
    store: &mut Store,
    settings: &Settings,
    caller: &Caller,
    call: &Call,
    now: i64,
) -> Outcome {
    let tool = call.tool();
    if let Some(refusal) = refused(settings, caller.way, call) {
        // Kept: what was tried and turned away is worth knowing.
        let summary = format!("refused: {}", refusal.message);
        log(
            store, caller, tool.name, false, &summary, 0, None, None, now,
        );
        return Outcome::refused(refusal);
    }
    let done = match call {
        Call::Sync { .. } => Err(Refusal::new(
            Code::Failed,
            "Checking for mail is not done here.",
        )),
        Call::Undo { entry } => change::undo(store, settings, *entry, now),
        Call::Open { id } => look::open(store, settings, *id),
        call if tool.level == Level::Read => look::run(store, settings, call, now),
        call if matches!(tool.level, Level::Draft | Level::Send) => {
            write::run(store, settings, caller, call, now)
        }
        call => change::run(store, settings, call, now),
    };
    let done = match done {
        Ok(done) => done,
        Err(refusal) => {
            if tool.level != Level::Read {
                let summary = format!("refused: {}", refusal.message);
                log(
                    store, caller, tool.name, false, &summary, 0, None, None, now,
                );
            }
            return Outcome::refused(refusal);
        }
    };

    let change = tool.level != Level::Read;
    // The log is not itself looked at in the log.
    let entry = if matches!(call, Call::Log { .. }) {
        None
    } else {
        let undo = done.undo.as_ref().map(Value::to_string);
        log(
            store,
            caller,
            tool.name,
            change && done.count > 0,
            &done.summary,
            done.count,
            undo.as_deref(),
            done.due,
            now,
        )
    };

    let mut value = done.value;
    if let Value::Object(fields) = &mut value {
        fields.insert("schema".to_owned(), json!(SCHEMA));
        if change && done.count > 0 {
            fields.insert("log".to_owned(), json!(entry));
        }
    }
    let mut events = Vec::new();
    let mut notice = None;
    let then = done.then;
    if let Some(said) = done.said.filter(|_| done.count > 0) {
        let text = format!("{}: {said}", caller.name);
        events.push(DataEvent::StoreChanged);
        if done.queued {
            events.push(DataEvent::Queued);
        }
        events.push(DataEvent::Flash {
            text: text.clone(),
            undoable: done.undoable,
        });
        notice = Some(Notice {
            text,
            send: tool.level == Level::Send,
        });
    }
    Outcome {
        result: Ok(Answer {
            value,
            lines: done.lines,
        }),
        events,
        notice,
        then,
    }
}

/// Writes a line in the log. A log that cannot be written is no reason
/// to undo what was done, so a failure is passed over; the number of the
/// line is returned when there is one.
#[allow(clippy::too_many_arguments)]
fn log(
    store: &mut Store,
    caller: &Caller,
    action: &str,
    change: bool,
    summary: &str,
    count: usize,
    undo: Option<&str>,
    due: Option<i64>,
    now: i64,
) -> Option<i64> {
    let id = store
        .log_agent(&NewAgentEntry {
            at: now,
            way: caller.way.as_str(),
            agent: &caller.name,
            action,
            change,
            summary,
            count: u32::try_from(count).unwrap_or(u32::MAX),
            undo,
            due,
        })
        .ok()?;
    // Old lines are let go a little at a time, as new ones are written.
    if id % 64 == 0 {
        let _ = store.prune_agent_log(now - LOG_DAYS * 24 * 60 * 60);
    }
    Some(id)
}

/// `n message` or `n messages`.
pub(crate) fn messages(count: usize) -> String {
    if count == 1 {
        "1 message".to_owned()
    } else {
        format!("{count} messages")
    }
}
