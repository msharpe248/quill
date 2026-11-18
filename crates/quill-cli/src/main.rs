//! The `quill` binary. `quill tui` is what the app shell runs inside its
//! terminal; run by hand in Ghostty or Kitty it works standalone.

mod accounts;
mod config_edit;
mod discover;
mod door;
mod headless;
mod keys_edit;
mod link;
mod mcp;
mod oauth;
mod password;
mod paths;
mod rules;
mod service;
mod settings;
mod setup;
mod signin;
mod skill;
mod theme_edit;
mod themes;

/// The IMAP crate's own test server, for tests here that need a real one: a
/// Dovecot started per test, with TLS in front of it under a certificate no
/// system trusts.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../quill-imap/tests/common/mod.rs"]
mod imap_test_server;

/// The POP3 crate's test server, likewise.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../quill-pop3/tests/common/mod.rs"]
mod pop3_test_server;

#[cfg(test)]
mod pop3_tests;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use jiff::tz::TimeZone;
use quill_auth::{Keychain, Remembered, SecretStore};
use quill_core::config::{AccountConfig, Config};
use quill_store::{BlobStore, Store};
use quill_sync::SyncOptions;
use quill_tui::agent::{Call, Caller, Mark, Which};
use quill_tui::app::{AgentLink, SyncLink};
use quill_tui::theme::ColorMode;
use quill_tui::{app, demo};

use crate::paths::Paths;

#[derive(Parser)]
#[command(name = "quill", version, about = "A keyboard-first mail client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the mail UI in this terminal
    Tui(TuiArgs),
    /// Manage the saved passwords and sign-ins of the accounts in config.toml
    Account {
        #[command(subcommand)]
        action: AccountAction,
        #[command(flatten)]
        dirs: Dirs,
    },
    /// List messages: the unified inbox, or one folder
    List {
        /// A folder, as after `in:` in a search: `work/sent`, `archive`
        #[arg(long = "in", value_name = "FOLDER")]
        within: Option<String>,
        /// Only unread messages
        #[arg(long)]
        unread: bool,
        /// At most this many, newest first
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Search mail, with the same words and filters as the search box
    Search {
        /// What to find: `from:ana is:unread "exact phrase"`. A `-` before
        /// a word or a filter means not, and may come first: `-from:ana`
        #[arg(value_name = "WORDS", allow_hyphen_values = true)]
        first: String,
        /// More of what to find. One that starts with `-` goes inside
        /// quotes with the rest: 'dinner -from:ana'
        #[arg(value_name = "WORDS")]
        words: Vec<String>,
        /// At most this many, newest first
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Print one message, by the id `list` and `search` show
    Read {
        id: i64,
        /// The message exactly as it arrived, headers and all
        #[arg(long, conflicts_with = "json")]
        raw: bool,
        /// Every header line as it arrived, in place of the few
        #[arg(long)]
        headers: bool,
        /// At most this much of the text (20000 unless said; 0 for all)
        #[arg(long, value_name = "CHARACTERS")]
        max_chars: Option<usize>,
        /// Start this far into the text, to read a long message on
        #[arg(long, value_name = "CHARACTERS", default_value_t = 0)]
        from_char: usize,
        #[command(flatten)]
        output: Output,
    },
    /// How many messages a search finds
    Count {
        /// What to count: `from:news older:30d`. A `-` before a word or a
        /// filter means not, and may come first
        #[arg(value_name = "WORDS", allow_hyphen_values = true)]
        first: String,
        /// More of what to count
        #[arg(value_name = "WORDS")]
        words: Vec<String>,
        #[command(flatten)]
        output: Output,
    },
    /// The conversation a message is in, oldest first
    Thread {
        id: i64,
        #[command(flatten)]
        output: Output,
    },
    /// The accounts there are
    Accounts {
        #[command(flatten)]
        output: Output,
    },
    /// The folders of every account, or of one, with how much is in each
    Folders {
        /// Only this account's folders
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// The files attached to a message
    Attachments {
        id: i64,
        #[command(flatten)]
        output: Output,
    },
    /// Write one of a message's files out
    Attachment {
        #[command(subcommand)]
        action: AttachmentAction,
    },
    /// People Quill knows from the mail, by the start of a name or address
    Contacts {
        #[arg(required = true)]
        words: Vec<String>,
        /// At most this many
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// How things stand: unread mail, drafts, what is waiting to go
    Status {
        #[command(flatten)]
        output: Output,
    },
    /// What commands and agents have done to the mail, newest first
    Log {
        /// At most this many
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// What was looked at, as well as what was changed
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Mark messages read, unread, starred or unstarred
    Mark {
        #[arg(value_enum)]
        what: MarkAs,
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Move messages to another folder of their account
    Move {
        /// The folder, by its name or what it is for: `archive`, `work/receipts`
        #[arg(value_name = "FOLDER")]
        to: String,
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Put a second copy of messages into another folder of their account
    Copy {
        #[arg(value_name = "FOLDER")]
        to: String,
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Move messages to their account's archive
    Archive {
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Move messages to the trash. Never for good: `restore` puts them back
    Delete {
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Put messages that are in the trash back where they were
    Restore {
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Put messages aside until a time: `2h`, `tomorrow`, "fri 14:30"
    Snooze {
        #[arg(value_name = "WHEN")]
        until: String,
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Bring snoozed messages back now
    Unsnooze {
        #[command(flatten)]
        which: Selection,
        #[command(flatten)]
        output: Output,
    },
    /// Make a folder
    Folder {
        #[command(subcommand)]
        action: FolderAction,
    },
    /// The drafts there are
    Drafts {
        #[command(flatten)]
        output: Output,
    },
    /// Write, show, change, send or discard a draft
    Draft {
        #[command(subcommand)]
        action: DraftAction,
    },
    /// What is on its way out: waiting, held for your OK, or refused
    Outbox {
        #[command(subcommand)]
        action: Option<OutboxAction>,
        #[command(flatten)]
        output: Output,
    },
    /// Take back a change a command or an agent made: the last, or the
    /// one with this number in `quill log`
    Undo {
        #[arg(value_name = "NUMBER")]
        entry: Option<i64>,
        #[command(flatten)]
        output: Output,
    },
    /// Put a message in the outbox. The body is read from standard input
    Send {
        /// A recipient. Repeat for more
        #[arg(long, value_name = "ADDRESS")]
        to: Vec<String>,
        #[arg(long, value_name = "ADDRESS")]
        cc: Vec<String>,
        #[arg(long, value_name = "ADDRESS")]
        bcc: Vec<String>,
        #[arg(long, default_value = "")]
        subject: String,
        /// The account to send from, by its name. The first one otherwise
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        /// A file to attach. Repeat for more
        #[arg(long, value_name = "FILE")]
        attach: Vec<String>,
        /// When to send it: `2h`, "tomorrow 9". Now, unless said
        #[arg(long, value_name = "WHEN")]
        at: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Put a link to this quill where a shell looks for programs, so that
    /// `quill` works at a prompt: what Install Command Line Tool does
    Link {
        /// The folder to link into. Without it: /usr/local/bin, or
        /// /opt/homebrew/bin, or ~/.local/bin, whichever can be written to
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// The skill that teaches an agent these commands: print it, or put it
    /// where agents' programs look for skills
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Serve an agent's program over MCP, on standard input and output. It
    /// carries what is asked to the Quill that is running, and does
    /// nothing itself
    Mcp {
        #[command(flatten)]
        dirs: Dirs,
    },
    /// Fetch new mail, carry out what is queued and send what is waiting,
    /// once, and stop
    Sync {
        #[command(flatten)]
        output: Output,
    },
    /// List the themes there are to choose from, or import one made for
    /// Ghostty
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
    /// Print every key Quill has and what it does, with your own from
    /// keys.toml
    Keys {
        #[command(flatten)]
        dirs: Dirs,
    },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// List the themes Quill has: its own, and the ones in themes/
    List {
        /// Also the themes made for Ghostty that are on this Mac to import
        #[arg(long)]
        ghostty: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Copy a theme into themes/: one made for Ghostty, by its name, or a
    /// theme file, by its path
    Import {
        /// The name of a theme made for Ghostty (`quill theme list
        /// --ghostty` lists them), or the path of a theme file
        #[arg(value_name = "NAME-or-PATH")]
        what: String,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// Print the skill
    Show,
    /// Print the folder the skill is in, beside this Quill, to link to
    Path,
    /// Put the skill where agents' programs find it: as a link to the one
    /// that ships with Quill, so that it is always this Quill's
    Install {
        /// A folder an agent reads skills from. Repeat for more. Without
        /// it: ~/.claude/skills (Claude Code) and ~/.agents/skills (Codex,
        /// Gemini CLI, Cursor, Copilot CLI, OpenCode, Goose)
        #[arg(long, value_name = "DIR")]
        to: Vec<PathBuf>,
        /// Write a copy, not a link. A copy is this Quill's skill until
        /// Quill changes: install again then
        #[arg(long)]
        copy: bool,
    },
}

#[derive(Subcommand)]
enum AttachmentAction {
    /// Write one file of a message into a folder, and print where it is
    Save {
        id: i64,
        /// Which file, by its number from `quill attachments`
        part: u32,
        /// The folder to write into. This one, if not said
        #[arg(long, value_name = "DIR", default_value = ".")]
        to: String,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Subcommand)]
enum FolderAction {
    /// Make a folder on this Mac. Mail moved into it is taken off the
    /// server, and is then only on this Mac
    Create {
        name: String,
        /// The account it is for. Needed when there is more than one
        #[arg(long, value_name = "NAME")]
        account: Option<String>,
        #[command(flatten)]
        output: Output,
    },
}

/// Who a message is to and what it is about. Its text is read from
/// standard input.
#[derive(Args)]
struct Letter {
    /// A recipient. Repeat for more
    #[arg(long, value_name = "ADDRESS")]
    to: Vec<String>,
    #[arg(long, value_name = "ADDRESS")]
    cc: Vec<String>,
    #[arg(long, value_name = "ADDRESS")]
    bcc: Vec<String>,
    #[arg(long, default_value = "")]
    subject: String,
    /// The account it is from, by its name. The first one otherwise
    #[arg(long, value_name = "NAME")]
    account: Option<String>,
    /// A file to attach. Repeat for more
    #[arg(long, value_name = "FILE")]
    attach: Vec<String>,
}

#[derive(Subcommand)]
enum DraftAction {
    /// Write a new draft. The text is read from standard input
    New {
        #[command(flatten)]
        letter: Letter,
        #[command(flatten)]
        output: Output,
    },
    /// Write a reply to a message as a draft. The text is read from
    /// standard input
    Reply {
        /// The message to answer, by its id
        id: i64,
        /// Answer everyone it went to
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Print a draft, or a message in the outbox, with its text
    Show {
        id: i64,
        #[command(flatten)]
        output: Output,
    },
    /// Change a draft that a command or an agent wrote. With --body, the
    /// new text is read from standard input
    Edit {
        id: i64,
        /// Who it is to, in place of who it was to. Repeat for more
        #[arg(long, value_name = "ADDRESS")]
        to: Option<Vec<String>>,
        #[arg(long, value_name = "ADDRESS")]
        cc: Option<Vec<String>>,
        #[arg(long, value_name = "ADDRESS")]
        bcc: Option<Vec<String>>,
        #[arg(long)]
        subject: Option<String>,
        /// Read a new text from standard input
        #[arg(long)]
        body: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Send a draft
    Send {
        id: i64,
        /// When to send it: `2h`, "tomorrow 9". Now, unless said
        #[arg(long, value_name = "WHEN")]
        at: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Discard a draft that a command or an agent wrote
    Discard {
        id: i64,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Subcommand)]
enum OutboxAction {
    /// Take a message that has not gone back out of the outbox
    Cancel {
        id: i64,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum MarkAs {
    Read,
    Unread,
    Starred,
    Unstarred,
}

/// Which messages a command is for.
#[derive(Args)]
struct Selection {
    /// Messages by id, as `list` and `search` print them
    #[arg(value_name = "ID")]
    ids: Vec<i64>,
    /// A message by its Message-ID header, which does not change when it
    /// moves. Repeat for more
    #[arg(long, value_name = "MESSAGE-ID")]
    msgid: Vec<String>,
    /// Messages by a search, in the search box's words: 'from:news older:30d'
    #[arg(long = "where", value_name = "SEARCH", allow_hyphen_values = true)]
    query: Option<String>,
    /// Change nothing; say what would be changed
    #[arg(long)]
    dry_run: bool,
    /// Go on although the search finds more messages than one change may
    /// touch without being told to
    #[arg(long)]
    yes: bool,
}

impl Selection {
    fn which(self) -> Which {
        Which {
            ids: self.ids,
            msgids: self.msgid,
            query: self.query,
            dry_run: self.dry_run,
            confirm: self.yes,
        }
    }
}

#[derive(Args)]
struct Output {
    /// Print JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    dirs: Dirs,
}

#[derive(Subcommand)]
enum AccountAction {
    /// List the configured accounts and whether each has a saved password
    /// or sign-in
    List,
    /// Sign an account in to Microsoft or Google, in the browser
    SignIn {
        /// The account's `name` in config.toml
        name: String,
        /// Sign in with a code typed on another device, not a browser here
        #[arg(long)]
        code: bool,
    },
    /// Save an account's password in the Keychain
    Password {
        /// The account's `name` in config.toml
        name: String,
    },
    /// Remove an account's password or sign-in from the Keychain
    Forget {
        /// The account's `name` in config.toml
        name: String,
    },
}

#[derive(Args)]
struct Dirs {
    /// Where config.toml, keys.toml and themes/ live [env: QUILL_CONFIG_DIR]
    #[arg(long, value_name = "DIR", global = true)]
    config_dir: Option<PathBuf>,
    /// Where the mail database lives [env: QUILL_DATA_DIR]
    #[arg(long, value_name = "DIR", global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Args)]
struct TuiArgs {
    /// Browse a built-in demo mailbox. Your own mail and data are not touched
    #[arg(long)]
    demo: bool,
    #[command(flatten)]
    dirs: Dirs,
}

/// What Quill exits with when the mailbox it was to open is open already.
/// Quill's app goes by it to say so in words of its own: keep it the same
/// as `CoreLocator.alreadyOpenStatus` there.
const ALREADY_OPEN_STATUS: u8 = 75;

/// The mailbox is open in another Quill: its app, or another terminal.
#[derive(Debug)]
struct AlreadyOpen;

impl std::fmt::Display for AlreadyOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "Quill is already open on this mailbox, in its app or in another terminal. \
             Two at once would fetch and send the same mail twice: close that one first",
        )
    }
}

impl std::error::Error for AlreadyOpen {}

/// Takes the mailbox in `data_dir` for this run, or says another Quill
/// has it. The app and a terminal's Quill keep their mail in the same
/// place, and each has an engine that fetches and sends: two of them on
/// one mailbox would do everything twice. Held for as long as what is
/// returned is kept; the system lets go of it when Quill ends, however it
/// ends. The commands that do one thing and exit do not take it.
fn hold_mailbox(data_dir: &std::path::Path) -> anyhow::Result<std::fs::File> {
    let path = data_dir.join("open.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(AlreadyOpen.into()),
        Err(std::fs::TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("cannot lock {}", path.display()))
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.downcast_ref::<AlreadyOpen>().is_some() => {
            eprintln!("quill: {err}.");
            ExitCode::from(ALREADY_OPEN_STATUS)
        }
        Err(err) if err.downcast_ref::<headless::Turned>().is_some() => {
            eprintln!("quill: {err}.");
            ExitCode::from(
                err.downcast_ref::<headless::Turned>()
                    .map_or(1, headless::Turned::status),
            )
        }
        Err(err) => {
            eprintln!("quill: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    match cli.command {
        Command::Tui(args) => runtime.block_on(tui(args)),
        Command::Account { action, dirs } => runtime.block_on(account(
            action,
            dirs,
            Arc::new(Remembered::new(Keychain::new())),
        )),
        Command::List {
            within,
            unread,
            limit,
            output,
        } => act(
            output,
            Call::List {
                within,
                unread,
                limit: Some(limit),
            },
        ),
        Command::Search {
            first,
            words,
            limit,
            output,
        } => act(
            output,
            Call::Search {
                query: one_search(first, words),
                limit: Some(limit),
            },
        ),
        Command::Count {
            first,
            words,
            output,
        } => act(
            output,
            Call::Count {
                query: one_search(first, words),
            },
        ),
        Command::Read {
            id,
            raw: true,
            output,
            ..
        } => {
            let (mut store, paths) = mail(output.dirs)?;
            let config = Config::load(&paths.config_file())?;
            headless::raw(
                &mut std::io::stdout(),
                &mut store,
                &headless::settings(&config),
                id,
            )
        }
        Command::Read {
            id,
            raw: false,
            headers,
            max_chars,
            from_char,
            output,
        } => act(
            output,
            Call::Read {
                id,
                headers,
                max_chars,
                from_char,
            },
        ),
        Command::Thread { id, output } => act(output, Call::Thread { id }),
        Command::Accounts { output } => act(output, Call::Accounts),
        Command::Folders { account, output } => act(output, Call::Folders { account }),
        Command::Attachments { id, output } => act(output, Call::Attachments { id }),
        Command::Attachment {
            action:
                AttachmentAction::Save {
                    id,
                    part,
                    to,
                    output,
                },
        } => act(
            output,
            Call::AttachmentSave {
                id,
                part,
                to: Some(to),
            },
        ),
        Command::Contacts {
            words,
            limit,
            output,
        } => act(
            output,
            Call::Contacts {
                words: words.join(" "),
                limit: Some(limit),
            },
        ),
        Command::Status { output } => act(output, Call::Status),
        Command::Log { limit, all, output } => act(
            output,
            Call::Log {
                limit: Some(limit),
                all,
            },
        ),
        Command::Mark {
            what,
            which,
            output,
        } => act(
            output,
            Call::Mark {
                which: which.which(),
                mark: match what {
                    MarkAs::Read => Mark::Read,
                    MarkAs::Unread => Mark::Unread,
                    MarkAs::Starred => Mark::Starred,
                    MarkAs::Unstarred => Mark::Unstarred,
                },
            },
        ),
        Command::Move { to, which, output } => act(
            output,
            Call::Move {
                which: which.which(),
                to,
            },
        ),
        Command::Copy { to, which, output } => act(
            output,
            Call::Copy {
                which: which.which(),
                to,
            },
        ),
        Command::Archive { which, output } => act(
            output,
            Call::Archive {
                which: which.which(),
            },
        ),
        Command::Delete { which, output } => act(
            output,
            Call::Delete {
                which: which.which(),
            },
        ),
        Command::Restore { which, output } => act(
            output,
            Call::Restore {
                which: which.which(),
            },
        ),
        Command::Snooze {
            until,
            which,
            output,
        } => act(
            output,
            Call::Snooze {
                which: which.which(),
                until,
            },
        ),
        Command::Unsnooze { which, output } => act(
            output,
            Call::Unsnooze {
                which: which.which(),
            },
        ),
        Command::Folder {
            action:
                FolderAction::Create {
                    name,
                    account,
                    output,
                },
        } => act(output, Call::FolderCreate { name, account }),
        Command::Undo { entry, output } => act(output, Call::Undo { entry }),
        Command::Send {
            to,
            cc,
            bcc,
            subject,
            account,
            attach,
            at,
            output,
        } => act(
            output,
            Call::Send {
                to,
                cc,
                bcc,
                subject,
                body: typed_in()?,
                account,
                attach: whole_paths(attach),
                at,
            },
        ),
        Command::Drafts { output } => act(output, Call::Drafts),
        Command::Draft { action } => match action {
            DraftAction::New { letter, output } => act(
                output,
                Call::DraftNew {
                    to: letter.to,
                    cc: letter.cc,
                    bcc: letter.bcc,
                    subject: letter.subject,
                    body: typed_in()?,
                    account: letter.account,
                    attach: whole_paths(letter.attach),
                },
            ),
            DraftAction::Reply { id, all, output } => act(
                output,
                Call::DraftReply {
                    id,
                    all,
                    body: typed_in()?,
                },
            ),
            DraftAction::Show { id, output } => act(output, Call::DraftShow { id }),
            DraftAction::Edit {
                id,
                to,
                cc,
                bcc,
                subject,
                body,
                output,
            } => act(
                output,
                Call::DraftEdit {
                    id,
                    to,
                    cc,
                    bcc,
                    subject,
                    body: if body { Some(typed_in()?) } else { None },
                },
            ),
            DraftAction::Send { id, at, output } => act(output, Call::DraftSend { id, at }),
            DraftAction::Discard { id, output } => act(output, Call::DraftDiscard { id }),
        },
        Command::Outbox {
            action: Some(OutboxAction::Cancel { id, output }),
            ..
        } => act(output, Call::OutboxCancel { id }),
        Command::Outbox {
            action: None,
            output,
        } => act(output, Call::Outbox),
        Command::Link { to } => {
            let said = link::install_this(to).map_err(|why| anyhow::anyhow!(why))?;
            println!("{said}");
            Ok(())
        }
        Command::Skill { action } => match action {
            SkillAction::Show => {
                print!("{}", skill::SKILL);
                Ok(())
            }
            SkillAction::Path => {
                let folder = skill::shipped().context(
                    "this quill has no skill folder beside it to link to. `quill skill install \
                     --copy` writes one out",
                )?;
                println!("{}", folder.display());
                Ok(())
            }
            SkillAction::Install { to, copy } => {
                let places = if to.is_empty() {
                    let home = std::env::var_os("HOME").context("there is no home folder")?;
                    skill::usual_places(std::path::Path::new(&home))
                } else {
                    to
                };
                let shipped = skill::shipped();
                let mut failed = false;
                for place in &places {
                    match skill::install(place, shipped.as_deref(), copy) {
                        Ok(put) => println!("{put}"),
                        Err(why) => {
                            eprintln!("quill: {why}.");
                            failed = true;
                        }
                    }
                }
                anyhow::ensure!(!failed, "the skill was not put everywhere");
                Ok(())
            }
        },
        Command::Mcp { dirs } => {
            let paths = Paths::resolve(dirs.config_dir, dirs.data_dir, |name| {
                std::env::var_os(name)
            })?;
            mcp::run(paths.data_dir.clone(), paths.config_file())
        }
        Command::Sync { output } => {
            let paths = Paths::resolve(output.dirs.config_dir, output.dirs.data_dir, |name| {
                std::env::var_os(name)
            })?;
            let (config, config_ok) = match Config::load(&paths.config_file()) {
                Ok(config) => (config, true),
                Err(err) => anyhow::bail!("{err}. Nothing was synced"),
            };
            // With Quill running on the mailbox, it is asked to look: a
            // second engine beside its own would fetch and send twice.
            if let Some(mut quill) = door::connect(&paths.data_dir) {
                let result = quill
                    .call(&Caller::shell(), &Call::Sync { account: None })
                    .context("Quill is running on this mailbox and did not answer")?;
                let fine = result.as_ref().is_ok_and(|answer| {
                    answer.value["problems"]
                        .as_array()
                        .is_none_or(Vec::is_empty)
                });
                headless::print(&mut std::io::stdout(), result, output.json)?;
                anyhow::ensure!(fine, "not everything synced");
                return Ok(());
            }
            headless::may_run(&config)?;
            create_private_dir(&paths.data_dir)
                .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
            // And none may start while this one is at work.
            let _held = hold_mailbox(&paths.data_dir)?;
            let fine = runtime.block_on(headless::sync(
                &mut std::io::stdout(),
                &config,
                config_ok,
                &paths.database(),
                paths.data_dir.join("blobs"),
                Arc::new(Remembered::new(Keychain::new())),
                output.json,
            ))?;
            anyhow::ensure!(fine, "not everything synced");
            Ok(())
        }
        Command::Theme {
            action: ThemeAction::List { ghostty, output },
        } => {
            let paths = themes::paths(output.dirs.config_dir, output.dirs.data_dir)?;
            themes::list(
                &mut std::io::stdout(),
                &paths,
                &themes::ghostty_folders_here(),
                ghostty,
                output.json,
            )
        }
        Command::Keys { dirs } => {
            let paths = themes::paths(dirs.config_dir, dirs.data_dir)?;
            let (text, problems) = setup::keys_listing(&paths);
            for problem in problems {
                eprintln!("quill: {problem}");
            }
            print!("{text}");
            Ok(())
        }
        Command::Theme {
            action: ThemeAction::Import { what, output },
        } => {
            let paths = themes::paths(output.dirs.config_dir, output.dirs.data_dir)?;
            themes::import_command(
                &mut std::io::stdout(),
                &paths,
                &themes::ghostty_folders_here(),
                &what,
                output.json,
            )
        }
    }
}

async fn tui(args: TuiArgs) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("`quill tui` needs a terminal to draw in");
    }
    // Whether this run was told where its mail is, and is not simply
    // where everyone's is.
    let data_chosen =
        args.dirs.data_dir.is_some() || std::env::var_os(paths::DATA_DIR_VAR).is_some();
    let paths = Paths::resolve(args.dirs.config_dir, args.dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let mode = ColorMode::detect(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
    );
    let loaded = setup::load(&paths, mode, TimeZone::system());
    let mut notices = loaded.notices;

    // The real mailbox is this run's alone for as long as this is held.
    let mut mailbox_held = None;
    let (store, sync, accounts) = if args.demo {
        let mut store = Store::open_in_memory().context("cannot create the demo mailbox")?;
        demo::seed(&mut store, app::now())
            .await
            .context("cannot fill the demo mailbox")?;
        // No accounts to add or sign in to. What the user chooses in the UI
        // is still remembered, in the config this run was pointed at.
        (
            store,
            SyncLink::none(),
            service::spawn_remembering(paths.config_file()),
        )
    } else {
        create_private_dir(&paths.data_dir)
            .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
        mailbox_held = Some(hold_mailbox(&paths.data_dir)?);
        let database = paths.database();
        let open = || {
            Store::open(&database).with_context(|| format!("cannot open {}", database.display()))
        };
        // The UI reads raw messages too: the reader draws a message's HTML
        // from the message as it arrived.
        let mut store = open()?.with_blobs(BlobStore::new(paths.data_dir.join("blobs")));
        let secrets: Arc<dyn SecretStore> = Arc::new(Remembered::new(Keychain::new()));
        let prepared = accounts::prepare(&loaded.config, &mut store, &secrets, loaded.config_ok)
            .context("cannot register the accounts")?;
        notices.extend(prepared.notices);
        let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        // The engine runs even with no accounts, so that one added from the
        // setup flow starts syncing at once. It has a connection of its
        // own; the UI keeps this one.
        let handle = Arc::new(quill_sync::spawn_with_rules(
            open()?,
            Some(paths.data_dir.join("blobs")),
            prepared.accounts,
            SyncOptions::default(),
            rules::from_config(&loaded.config),
            events_tx,
        ));
        // Downloaded mail is kept within the room the config gives it.
        if let Some(bytes) = loaded.config.storage.cache_bytes() {
            handle.keep_cache_within(bytes);
        }
        let accounts = service::spawn(service::Service {
            config_file: paths.config_file(),
            secrets,
            store: std::sync::Mutex::new(open()?),
            sync: Arc::clone(&handle),
            dns: Box::new(discover::SystemDns::new()),
            prober: Box::new(discover::NetworkProber),
            trusted_once: std::sync::Mutex::default(),
            remote: over_ssh(),
        });
        (
            store,
            SyncLink {
                handle: Some(handle),
                events,
            },
            accounts,
        )
    };

    // The door for the user's own commands and agents: open for as long
    // as this Quill has the mailbox. The demo mailbox opens one only in a
    // data folder it was given: beside the user's real mail, a command
    // meant for that mail would be answered from the demo.
    let (door, agents) = if !args.demo || data_chosen {
        let opened = create_private_dir(&paths.data_dir).and_then(|()| door::open(&paths.data_dir));
        match opened {
            Ok((door, agents)) => (Some(door), agents),
            Err(err) => {
                notices.push(quill_tui::state::Notice::error(format!(
                    "Commands from a shell and agents cannot reach this Quill: {err}. They work \
                     on the mail directly, and what they change shows here after the next sync."
                )));
                (None, AgentLink::none())
            }
        }
    } else {
        (None, AgentLink::none())
    };

    let attached = quill_host::attach(std::env::var_os(quill_host::SOCKET_VARIABLE).as_deref());
    let mut env = loaded.env;
    // Quill's own shell always draws images. Elsewhere, go by what the
    // terminal says it is.
    // The demo mailbox has no config to add an account to.
    env.accounts = !args.demo;
    suit_host(
        &mut env,
        attached.host.is_shell(),
        std::env::var_os("QUILL_VIM"),
    );
    env.images = attached.host.is_shell()
        || quill_media::kitty::supported_by_environment(|name| std::env::var(name).ok());
    // Quill's app says what the Mac looks like as it starts the core, so
    // that the first frame is drawn with the theme for it. It says so again
    // over the bridge, and whenever the Mac changes. Nobody says in a
    // terminal, whatever its environment has in it.
    if attached.host.is_shell() {
        env.look.system_dark =
            system_appearance(std::env::var("QUILL_SYSTEM_APPEARANCE").ok().as_deref());
        env.restyle();
    }
    let ended = app::run(store, env, notices, attached, sync, accounts, agents)
        .await
        .context("the terminal UI stopped");
    // Closed before the mailbox is let go of, so that no command finds a
    // door with nobody behind it.
    drop(door);
    drop(mailbox_held);
    ended
}

/// Sets what depends on where Quill is running: inside its own app
/// (`shell`), or in a terminal. `vim` is what `QUILL_VIM` says.
fn suit_host(env: &mut quill_tui::state::Env, shell: bool, vim: Option<std::ffi::OsString>) {
    // A terminal program keeps cmd for itself; the shell passes it on.
    env.cmd_keys = shell;
    // And it has a panel to choose files in.
    env.file_panel = shell;
    // And the system's own readers for pictures Quill cannot decode.
    env.thumbnails = shell;
    // And the system's spelling checker.
    env.spell_check = shell;
    // And a sheet on its window for the form that adds an account and for
    // the questions Quill asks (a sign-in, a certificate, a time,
    // unsubscribing), which a terminal has drawn over its panes.
    env.sheets = shell;
    // And a terminal of its own to run an editor in, over the text's
    // place. Either way the editor is the user's own: the program the
    // settings name, or for Vim the one QUILL_VIM names, or the one of
    // the editor's name on the PATH.
    env.editor_pane = shell;
    if env.editor_program.is_none() && env.external.is_vim() {
        env.editor_program = vim.filter(|name| !name.is_empty());
    }
    // Hooks run only in a terminal: running the user's programs from the
    // app has not been tried yet.
    env.hooks_enabled = !shell;
    // And a web view pane that can stand in the reader's place. A
    // terminal has none, whatever the config says about reading in one.
    env.web_pane = shell;
    // And a window for the list of every key, which stays up.
    env.keys_window = shell;
    // And a window of its own, which can be closed with Quill left
    // running: `q` closes it there, and quits only in a terminal.
    env.window = shell;
}

/// What the app said the Mac looks like as it started the core: `dark` or
/// `light`, and nothing known for anything else.
fn system_appearance(said: Option<&str>) -> Option<bool> {
    match said {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    }
}

/// `quill account ...`: the saved passwords and sign-ins of the configured
/// accounts.
async fn account(
    action: AccountAction,
    dirs: Dirs,
    secrets: Arc<dyn SecretStore>,
) -> anyhow::Result<()> {
    let paths = Paths::resolve(dirs.config_dir, dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    let config = Config::load(&paths.config_file())?;
    let find = |name: &str| -> anyhow::Result<&AccountConfig> {
        config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .with_context(|| {
                format!(
                    "there is no account named \"{name}\" in {}",
                    paths.config_file().display()
                )
            })
    };
    let reading = |account: &AccountConfig| {
        accounts::reading(account)
            .map_err(|reason| anyhow::anyhow!("{} cannot sync: {reason}", account.name))
    };
    let oauth = |account: &AccountConfig| {
        oauth::setup(account)
            .map_err(|reason| anyhow::anyhow!("{} cannot sign in: {reason}", account.name))
    };
    match action {
        AccountAction::List => {
            if config.accounts.is_empty() {
                println!("No accounts in {}.", paths.config_file().display());
            }
            for account in &config.accounts {
                let status = if oauth::wanted(account) {
                    match oauth::setup(account) {
                        Ok(Some(setup)) => match secrets.get(&setup.secret_name(account))? {
                            Some(_) => "signed in".to_owned(),
                            None => "not signed in".to_owned(),
                        },
                        Ok(None) => "not signed in".to_owned(),
                        Err(reason) => format!("cannot sign in: {reason}"),
                    }
                } else {
                    match accounts::reading(account) {
                        Ok(reading) => match secrets.get(&reading.secret)? {
                            Some(_) => "password saved".to_owned(),
                            None => "no password saved".to_owned(),
                        },
                        Err(reason) => format!("cannot sync: {reason}"),
                    }
                };
                println!(
                    "{}  {}  {}  {status}",
                    account.name,
                    account.kind.as_str(),
                    account.address
                );
            }
        }
        AccountAction::SignIn { name, code } => {
            let terminal = signin::Terminal {
                say: Arc::new(|line| println!("{line}")),
                open: Box::new(|url| {
                    // Not sandboxed here: the system's own opener.
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(url)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }),
                remote: over_ssh(),
            };
            signin::sign_in(find(&name)?, secrets, code, terminal).await?;
        }
        AccountAction::Password { name } => {
            let account = find(&name)?;
            anyhow::ensure!(
                !oauth::wanted(account),
                "{name} signs in with its provider, not a password: quill account sign-in \"{name}\""
            );
            let reading = reading(account)?;
            let prompt = format!("Password for {} on {}: ", reading.username, reading.host);
            let secret = password::read(&prompt)?;
            anyhow::ensure!(!secret.is_empty(), "no password given; nothing was saved");
            secrets.set(&reading.secret, &secret)?;
            println!("Saved in the Keychain.");
        }
        AccountAction::Forget { name } => {
            let account = find(&name)?;
            if let Some(setup) = oauth(account)? {
                secrets.delete(&setup.secret_name(account))?;
            } else {
                secrets.delete(&reading(account)?.secret)?;
            }
            println!("Removed from the Keychain.");
        }
    }
    Ok(())
}

/// A search as it was typed, in one piece or several.
fn one_search(first: String, rest: Vec<String>) -> String {
    std::iter::once(first)
        .chain(rest)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A message's text, from standard input when something is piped in.
/// Typed at a terminal, there is none: a prompt is not waited at.
fn typed_in() -> anyhow::Result<String> {
    let mut body = String::new();
    if !std::io::stdin().is_terminal() {
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
            .context("cannot read the message from standard input")?;
    }
    Ok(body)
}

/// Files to attach, by their whole paths: the Quill that is running does
/// not stand where this command was typed.
fn whole_paths(files: Vec<String>) -> Vec<String> {
    files
        .into_iter()
        .map(|file| match std::path::absolute(&file) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => file,
        })
        .collect()
}

/// Does one thing to the mail, as the user's settings for what may be
/// done from outside Quill's window allow, and prints what came of it.
///
/// With Quill running on the mailbox, the request is handed to it: there
/// is one writer, and its window shows the change as it is made. With no
/// Quill running, it is done here on the store, and reaches a server when
/// Quill next runs or at `quill sync`. A settings file that cannot be
/// read is no licence: nothing is done.
fn act(output: Output, call: Call) -> anyhow::Result<()> {
    let paths = Paths::resolve(output.dirs.config_dir, output.dirs.data_dir, |name| {
        std::env::var_os(name)
    })?;
    if let Some(mut quill) = door::connect(&paths.data_dir) {
        // Asked once. If Quill goes away before it answers, the request
        // is not made again here: it may have been done.
        let result = quill
            .call(&Caller::shell(), &call)
            .context("Quill is running on this mailbox and did not answer")?;
        return headless::print(&mut std::io::stdout(), result, output.json);
    }
    let (mut store, paths) = mail_at(paths)?;
    let config = Config::load(&paths.config_file())?;
    headless::ask(
        &mut std::io::stdout(),
        &mut store,
        &headless::settings(&config),
        &call,
        output.json,
    )
}

/// Opens the mail there is, for the commands that only look at it or add
/// to the outbox. They make nothing: with no mail yet, they say so.
fn mail(dirs: Dirs) -> anyhow::Result<(Store, Paths)> {
    mail_at(Paths::resolve(dirs.config_dir, dirs.data_dir, |name| {
        std::env::var_os(name)
    })?)
}

/// As [`mail`], for a place already worked out.
fn mail_at(paths: Paths) -> anyhow::Result<(Store, Paths)> {
    let database = paths.database();
    anyhow::ensure!(
        database.is_file(),
        "there is no mail in {} yet. Run `quill tui` or `quill sync` first",
        paths.data_dir.display()
    );
    let store = Store::open(&database)
        .with_context(|| format!("cannot open {}", database.display()))?
        .with_blobs(BlobStore::new(paths.data_dir.join("blobs")));
    Ok((store, paths))
}

/// Whether whoever is typing is at another computer.
fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Mail is private: the data directory is readable by its owner only.
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn tui_flags_parse() {
        let cli = Cli::try_parse_from(["quill", "tui", "--demo", "--config-dir", "/c"]).unwrap();
        let Command::Tui(args) = cli.command else {
            panic!("not the tui subcommand");
        };
        assert!(args.demo);
        assert_eq!(args.dirs.config_dir, Some(PathBuf::from("/c")));
        assert_eq!(args.dirs.data_dir, None);
        assert!(Cli::try_parse_from(["quill"]).is_err());
        assert!(Cli::try_parse_from(["quill", "tui", "--bogus"]).is_err());
    }

    #[test]
    fn theme_commands_parse() {
        let cli = Cli::try_parse_from(["quill", "keys", "--config-dir", "/c"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Keys { dirs } if dirs.config_dir.as_deref() == Some(std::path::Path::new("/c"))
        ));
        let cli = Cli::try_parse_from(["quill", "theme", "list", "--ghostty", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Theme {
                action: ThemeAction::List { ghostty: true, output }
            } if output.json
        ));
        let cli = Cli::try_parse_from([
            "quill",
            "theme",
            "import",
            "Catppuccin Mocha",
            "--config-dir",
            "/c",
        ])
        .unwrap();
        let Command::Theme {
            action: ThemeAction::Import { what, output },
        } = cli.command
        else {
            panic!("not theme import");
        };
        assert_eq!(what, "Catppuccin Mocha");
        assert_eq!(output.dirs.config_dir, Some(PathBuf::from("/c")));
        assert!(Cli::try_parse_from(["quill", "theme"]).is_err());
        assert!(Cli::try_parse_from(["quill", "theme", "import"]).is_err());
    }

    #[test]
    fn the_app_runs_no_programs_and_a_terminal_has_no_panels() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
        };
        let load = || setup::load(&paths, ColorMode::None, TimeZone::UTC).env;

        // In Quill's own app: its panels and a terminal for the editor,
        // and no hooks. The editor is the user's own there too.
        let mut env = load();
        suit_host(&mut env, true, Some("/opt/homebrew/bin/nvim".into()));
        assert!(env.cmd_keys && env.file_panel && env.thumbnails && env.spell_check);
        assert!(env.editor_pane && env.sheets);
        assert!(!env.hooks_enabled);
        assert!(env.web_pane && env.window);
        assert_eq!(env.editor_program, Some("/opt/homebrew/bin/nvim".into()));

        // In a terminal: the user's own Vim, and their hooks.
        let mut env = load();
        suit_host(&mut env, false, Some("/opt/homebrew/bin/nvim".into()));
        assert!(!env.cmd_keys && !env.file_panel && !env.thumbnails && !env.spell_check);
        assert!(!env.editor_pane && !env.sheets);
        assert!(env.hooks_enabled);
        assert!(!env.web_pane && !env.window);
        assert_eq!(env.editor_program, Some("/opt/homebrew/bin/nvim".into()));
        let mut env = load();
        suit_host(&mut env, false, Some("".into()));
        assert_eq!(env.editor_program, None);
        suit_host(&mut env, false, None);
        assert_eq!(env.editor_program, None);
        // What the settings name comes first, and QUILL_VIM is Vim's
        // alone: it names no other editor.
        let mut env = load();
        env.editor_program = Some("/usr/local/bin/vim".into());
        suit_host(&mut env, false, Some("/opt/homebrew/bin/nvim".into()));
        assert_eq!(env.editor_program, Some("/usr/local/bin/vim".into()));
        let mut env = load();
        env.external = quill_tui::vim::ExternalEditor::Nano;
        suit_host(&mut env, false, Some("/opt/homebrew/bin/nvim".into()));
        assert_eq!(env.editor_program, None);
    }

    #[test]
    fn a_mailbox_is_one_quills_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let held = hold_mailbox(dir.path()).unwrap();
        // A second Quill is told so, in words, and by a status of its own.
        let refused = hold_mailbox(dir.path()).unwrap_err();
        assert!(refused.downcast_ref::<AlreadyOpen>().is_some());
        assert!(refused.to_string().contains("already open"));
        // Another mailbox is another matter.
        let other = tempfile::tempdir().unwrap();
        assert!(hold_mailbox(other.path()).is_ok());
        // And when the first has gone, the mailbox is free again. Given a
        // moment: another test starting a program at this instant has a
        // copy of every open file until the program takes its place, and
        // the lock is held for as long as any copy is open.
        drop(held);
        let free = (0..200).any(|_| {
            hold_mailbox(dir.path()).is_ok() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        });
        assert!(free);
        // A folder that is not there is a mistake of another kind.
        let missing = hold_mailbox(&dir.path().join("nowhere")).unwrap_err();
        assert!(missing.downcast_ref::<AlreadyOpen>().is_none());
    }

    #[test]
    fn the_app_says_what_the_mac_looks_like_as_it_starts_the_core() {
        assert_eq!(system_appearance(Some("dark")), Some(true));
        assert_eq!(system_appearance(Some("light")), Some(false));
        for unknown in [None, Some(""), Some("Dark"), Some("auto"), Some("1")] {
            assert_eq!(system_appearance(unknown), None, "{unknown:?}");
        }
        // With a theme for each appearance, the first frame is drawn with
        // the one for what the Mac is, and not with the dark one.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            paths.config_file(),
            "[ui]\ntheme_light = \"light\"\ntheme_dark = \"dark\"\n",
        )
        .unwrap();
        let mut env = setup::load(&paths, ColorMode::TrueColor, TimeZone::UTC).env;
        assert_eq!(env.styles.theme_name(), "dark");
        env.look.system_dark = system_appearance(Some("light"));
        env.restyle();
        assert_eq!(env.styles.theme_name(), "light");
    }

    #[cfg(unix)]
    #[test]
    fn the_data_directory_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Quill").join("data");
        create_private_dir(&data).unwrap();
        create_private_dir(&data).unwrap();
        let mode = std::fs::metadata(&data).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
