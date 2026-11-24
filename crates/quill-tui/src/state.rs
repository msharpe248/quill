//! The UI state and its one way of changing: `Event -> Action -> update`.
//!
//! `update` never touches the terminal, the disk or the network. It mutates
//! the state and returns the store requests the change calls for; the run
//! loop sends those to the store worker, whose answers come back as actions.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use jiff::tz::TimeZone;
use quill_core::types::{FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_mime::html::Line;
use quill_mime::{BodyFormat, ReplyPosition};
use quill_store::{AccountRow, DraftContent, DraftId, FolderRow, Mailbox, MessageRow};
use quill_sync::{SyncEvent, SyncRequest};

use crate::command::{CommandId, Context, Registry};
use crate::compose::{Away, Compose, Did, Field, Prefill};
use crate::data::{Bulk, DataEvent, DataRequest, FileUse, LoadedBody, PageUse};
use crate::editor::Editor;
use crate::files::RemoteContent;
use crate::files::{AttachedFile, ExtractedFile};
use crate::hook::{Hook, HookOutcome, HookRun};
use crate::input::LineInput;
use crate::invite::CardRow;
use crate::keymap::{Key, KeyChord, Keymap, Mods, Resolution};
use crate::layout;
use crate::outgoing::{Handed, InviteAnswer, Response, SendRequest};
use crate::palette::{self, Item, Palette, What};

mod edit;
mod folders;
mod keys;
mod letter;
mod mouse;
mod remove;
mod rules;
mod save;
mod select;
mod settings;
mod sheet;
mod themes;
use quill_host::bridge::{FormUsed, Number, SettingsUsed};

pub use self::folders::{FolderPrompt, ON_THE_SERVER, ON_THIS_MAC};
pub use self::keys::{KeysAsk, KeysHow, KeysPrompt};
pub use self::letter::LetterPrompt;
pub use self::remove::{REMOVING, RemovePrompt, YES as REMOVE_YES};
pub use self::rules::{
    EVERY_ACCOUNT, PickFor, PickLine, PickList, RuleAccount, RuleAction, RuleField, RulePrompt,
    RuleTrial,
};
pub use self::settings::SettingsWindow;
pub use self::themes::{ThemeAsk, ThemePrompt};
use crate::settings::Applied;

use crate::setup::{
    AccountEvent, AccountRequest, CertificatePrompt, Outcome, Preference, RulesAsk, Setup, SignIn,
    SignInStage,
};
use crate::template::{self, Template};
use crate::theme::{Look, Styles, Theme};
use crate::vim::{self, ExternalEditor, Verdict};
use crate::{text, view};
pub use mouse::{Mouse, MouseKind, Resizing};
pub use save::SaveKind;
pub use select::{Clicked, HeadRow, Held, LABEL_WIDTH, Part, Selection, Spot};

/// What a notification about one message is called: this and the message's
/// id in the store.
const MESSAGE_NOTIFICATION: &str = "message:";
/// What the name of every notification about agents starts with.
const AGENT_NOTIFICATION: &str = "agent.";
/// What the notification about several messages at once is called.
const SEVERAL_NOTIFICATION: &str = "arrived";
/// More new messages than this at once are announced together.
const MOST_ANNOUNCED_SINGLY: usize = 3;

/// What `update` and the views consult but never change: where Quill is
/// running, and what the config says.
///
/// Nearly all of it is fixed for the life of the run. The exception is the
/// settings that hold from the moment they are changed in the host's
/// settings window: `update` decides what they come to
/// ([`Effects::settings`]), and the run loop puts them here between one
/// update and the next ([`crate::settings::Applied::apply`]).
pub struct Env {
    pub registry: Registry,
    pub keymap: Keymap,
    /// What the views draw with: the theme in force, for this terminal.
    /// Made from [`Env::look`] by [`Env::restyle`], and by nothing else.
    pub styles: Styles,
    /// Whether Quill is light or dark, and the theme for each.
    pub look: Look,
    pub tz: TimeZone,
    /// The reader's share of the space right of the sidebar, from the config.
    pub reader_width: f32,
    /// This Quill's own program, by its whole path, for saying what an
    /// agent's program is to be given to reach it. `None` when it is not
    /// known.
    pub program: Option<std::path::PathBuf>,
    /// The accounts pane's columns, from the config: `sidebar_width`
    /// under `[ui]`. `None`: by the window's width.
    pub sidebar_width: Option<u16>,
    /// What may be done to the mail from outside Quill's window, from the
    /// config: `[agent]`.
    pub agent: quill_core::config::AgentConfig,
    /// The terminal draws images (the Kitty graphics protocol).
    pub images: bool,
    /// Accounts can be added and their certificates accepted from here.
    /// False for the demo mailbox, which has no config to write to.
    pub accounts: bool,
    /// Whether keys held with `cmd` can arrive: they do from Quill's own
    /// window, and a terminal program keeps them for itself.
    pub cmd_keys: bool,
    /// Whether the host can make a picture of a file Quill cannot decode
    /// itself: a HEIC photo, the first page of a PDF, a frame of a video.
    pub thumbnails: bool,
    /// Take the mouse: clicks select and open, the wheel scrolls.
    pub mouse: bool,
    /// Whether the host has a panel to choose files in. Without one a
    /// file to attach is named by its path.
    pub file_panel: bool,
    /// Where a saved attachment goes when the host has no panel to ask
    /// with: the user's Downloads folder.
    pub downloads: Option<std::path::PathBuf>,
    /// How mail is written and sent, from the config.
    pub compose: ComposeSettings,
    /// Who the user is on each account, by the account's name.
    pub identities: HashMap<String, Identity>,
    /// Remote content is never loaded, whoever sent the message and
    /// whatever is pressed (`remote_images = "never"`).
    pub remote_never: bool,
    /// Say when new mail arrives.
    pub notifications: bool,
    /// List the messages of a conversation together, unless the user
    /// switches that off for the run.
    pub threads: bool,
    /// Searches kept in the sidebar: a name and what to search for.
    pub searches: Vec<(String, String)>,
    /// Text kept ready to put into a message, from the user's `templates`
    /// folder.
    pub templates: Vec<Template>,
    /// Whether the host has a spelling checker to ask. A terminal has
    /// none.
    pub spell_check: bool,
    /// Whether the host shows Quill's forms and questions itself, in a
    /// sheet on its window, as Quill's app does: the form to add an
    /// account, a sign-in, a certificate, a time to name, unsubscribing.
    /// The one in front is then not drawn here: it is described to the
    /// host ([`State::form`]), and what the user does with it comes back
    /// as [`Action::FormUsed`]. In a terminal each is drawn over the
    /// panes. What is chosen from with the keyboard (the palette, the
    /// pickers, the help) is drawn here whatever the host.
    pub sheets: bool,
    /// Whether the host runs an editor itself, in a terminal of its own
    /// laid over the text's place, as Quill's app does. In a terminal
    /// Quill runs it there, and it has the whole screen. Either way it
    /// is the user's own editor, started with the same command.
    pub editor_pane: bool,
    /// The editor outside Quill that a message is handed to: the one the
    /// config names (`editor` under `[compose]`), and Vim when it names
    /// Quill's own.
    pub external: ExternalEditor,
    /// The program run as that editor, when it is not to be the one of
    /// its name on the `PATH`: `editor_path` under `[compose]`, or for
    /// Vim what `QUILL_VIM` says.
    pub editor_program: Option<std::ffi::OsString>,
    /// The user's programs that a message can be given to, from
    /// `hooks.toml`.
    pub hooks: Vec<Hook>,
    /// Whether the user's programs may be run from here at all. Not in
    /// Quill's own app, where it has not been tried.
    pub hooks_enabled: bool,
    /// Whether the host has a web view pane that can stand in the
    /// reader's place and follow the list, as Quill's app has. A terminal
    /// has none, and web reading is not on offer there.
    pub web_pane: bool,
    /// Whether the host has a window of its own for the list of every
    /// key, as Quill's app has: asked for from a menu, the list goes
    /// there and stays up beside the mail. Asked for by its key it is
    /// drawn over the panes, whatever the host.
    pub keys_window: bool,
    /// Whether Quill is in a window of the host's that can be closed and
    /// brought back while Quill goes on running, as the app's can. A
    /// terminal has none: there, closing Quill is quitting it.
    pub window: bool,
    /// Web reading is on as Quill starts: `web_reading` under `[ui]` in
    /// the config. It means nothing without [`Env::web_pane`].
    pub web_reading: bool,
    /// Mail in the web pane is shown dark while Quill is, as Quill
    /// starts: `dark_pages` under `[ui]` in the config.
    pub dark_pages: bool,
    /// The accounts pane is on show as Quill starts: `sidebar` under
    /// `[ui]` in the config.
    pub sidebar: bool,
    /// What an editor's own keys do is said above it, where it runs in a
    /// message's pane: `editor_tips` under `[compose]`. Quill switches it
    /// off itself once it has been shown for one message.
    pub editor_tips: bool,
    /// The room downloaded mail was given as Quill started, in gigabytes
    /// (`cache_gb` under `[storage]`; 0 is no limit). The sync engine was
    /// told once, so this is what holds until Quill is started again,
    /// whatever the config has come to say since.
    pub cache_gb: f64,
    /// The font the host's terminals have, by its family's name, and its
    /// size in points: `font` and `font_size` under `[ui]`. Where there
    /// is none said the host has its own. A terminal Quill runs in has
    /// the font it has, and nothing here changes it.
    pub font: Option<String>,
    pub font_size: Option<Number>,
}

impl Env {
    /// Draws with the theme in force from now on: after the look has
    /// changed, in the settings or because the system's appearance has.
    pub fn restyle(&mut self) {
        self.styles = Styles::new(self.look.theme().clone(), self.styles.mode());
    }

    /// The key to name when telling the user how to run a command: the
    /// first of its keys that can be pressed here.
    pub fn hint_key(&self, id: CommandId) -> Option<&crate::keymap::KeySeq> {
        let keys = self.keymap.keys_for(id);
        keys.iter()
            .find(|sequence| self.cmd_keys || !sequence.uses_cmd())
            .or_else(|| keys.first())
    }
}

/// The `[compose]` settings the UI acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeSettings {
    /// Write in Markdown and send HTML beside the text, or plain text only.
    pub markdown: bool,
    /// The reply goes above the quoted message, or below it.
    pub reply_above: bool,
    /// How long a sent message waits in the outbox, where it can be taken
    /// back.
    pub undo_send_seconds: u32,
    /// Check the spelling of what is written, where the host can.
    pub spell_check: bool,
    /// A message is written outside Quill from the start: the config
    /// names an editor other than Quill's own. Otherwise only when asked
    /// for.
    pub vim: bool,
}

impl Default for ComposeSettings {
    fn default() -> Self {
        Self {
            markdown: true,
            reply_above: true,
            undo_send_seconds: 10,
            spell_check: true,
            vim: false,
        }
    }
}

/// Who the user is when sending from an account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub full_name: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Key(KeyChord),
    /// Run a command directly: the path menus and the palette will use.
    Command(CommandId),
    Resize(u16, u16),
    Data(DataEvent),
    /// The wall clock, in seconds since the Unix epoch.
    ClockTick(i64),
    Notify(Notice),
    /// Something was changed, or sent, from outside the window.
    AgentNotice(crate::agent::Notice),
    /// An agent's program connected over MCP, under this name.
    AgentConnected(String),
    /// News from the sync engine.
    Sync(SyncEvent),
    /// An answer from the accounts service.
    Account(AccountEvent),
    /// Text pasted into the terminal.
    Paste(String),
    /// Files the user chose in the host's panel or dropped on its window.
    /// `temporary`: they are copies made for Quill, to remove once read.
    Files {
        paths: Vec<String>,
        temporary: bool,
    },
    /// The mouse did something over the terminal.
    Mouse(Mouse),
    /// The host's save panel has been answered.
    FileSaved {
        name: String,
        saved: bool,
    },
    /// The Mac asked Quill to write to a `mailto:` address.
    Mailto(String),
    /// The user clicked a notification (`action` is `None`) or one of its
    /// buttons.
    Notification {
        id: String,
        action: Option<String>,
    },
    /// A picture asked for with [`HostRequest::LoadPicture`] is ready to be
    /// drawn, at this many pixels, or could not be made.
    PictureLoaded {
        id: u32,
        size: Option<(u32, u32)>,
    },
    /// No key has arrived for a moment: the time to have the spelling of
    /// what was typed checked, if it wants checking.
    TypingPaused,
    /// The answer to [`HostRequest::SpellCheck`]: the misspelt words, each
    /// as the byte offsets of its start and end in the text that was sent.
    Spelling {
        id: u64,
        words: Vec<(usize, usize)>,
    },
    /// The answer to [`HostRequest::SpellSuggest`].
    Suggestions {
        id: u64,
        words: Vec<String>,
    },
    /// The answer to [`HostRequest::EditDraft`]: Vim has gone, with this
    /// exit status, or (`ran` false) could not be started at all.
    EditorExited {
        ran: bool,
        status: i32,
    },
    /// The answer to [`HostRequest::RunHook`].
    HookDone {
        name: String,
        outcome: HookOutcome,
    },
    /// The user pressed a button of the form the host is showing, or
    /// changed one of its choices: what its fields held, and which button.
    FormUsed(FormUsed),
    /// The list's cursor has rested on a message for a moment: the time
    /// to show it in the web reading pane. See [`State::settle_due`].
    SelectionSettled,
    /// The host's window has room for this many columns: what the
    /// terminal would have with the window to itself. The layout goes by
    /// it while the host's reading pane has part of the window.
    WindowColumns(u16),
    /// The user moved the divider between the terminal and the host's
    /// reading pane to where the terminal keeps this many columns; with
    /// none, they put it back where Quill has it by itself. It is the
    /// edge of the reader, dragged in the host's window.
    ReadingDragged(Option<u16>),
    /// The user opened, closed or did something in the host's settings
    /// window.
    Settings(SettingsAct),
    /// The host says the system is in its dark appearance, or its light
    /// one.
    SystemAppearance {
        dark: bool,
    },
    /// The host has put a theme file of this name in the user's `themes`
    /// folder: the user chose it in the host's chooser.
    ThemeImported(String),
}

/// What was done with the host's settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAct {
    /// It was opened, or brought forward: the settings are read afresh.
    Opened,
    Closed,
    /// A setting was given another value there, or something was asked
    /// for from one of its lists.
    Used(SettingsUsed),
}

/// What an action asks of the world outside the state: queries and writes
/// for the store worker, things only the host can do, and things only the
/// network can do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub data: Vec<DataRequest>,
    pub host: Vec<HostRequest>,
    pub sync: Vec<SyncRequest>,
    /// Work for the accounts service: discovery, sign-in, saving.
    pub accounts: Vec<AccountRequest>,
    /// Settings that hold from now on, for the run loop to put into
    /// [`Env`] before the next update.
    pub settings: Option<Box<Applied>>,
    /// What the host has just said the system's appearance is, for the
    /// run loop to put into [`Env`] likewise ([`Look::system_dark`]).
    pub system_dark: Option<bool>,
    /// A theme to draw with while it is being made, or `Some(None)` when
    /// that is over, for the run loop to put into [`Env`] likewise
    /// ([`Look::trying`]).
    pub trying: Option<Option<Theme>>,
    /// Agents are stopped: the run loop switches both ways in off in
    /// [`Env`] and hangs up on whoever is connected.
    pub agents_stopped: bool,
    /// The host asked where its reading pane goes: it is told again,
    /// though the place is what it was.
    pub place_again: bool,
    /// The mail around the list's cursor, nearest first, when that has
    /// changed: what is not here is downloaded, so that the message
    /// opened next is.
    pub near: Option<Vec<LocalMessageId>>,
}

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
            && self.host.is_empty()
            && self.sync.is_empty()
            && self.accounts.is_empty()
            && self.settings.is_none()
            && self.system_dark.is_none()
            && self.trying.is_none()
            && !self.agents_stopped
            && !self.place_again
            && self.near.is_none()
    }
}

/// How many messages after the list's cursor are downloaded ahead of
/// being opened, and how many before it.
const NEAR_AFTER: usize = 8;
const NEAR_BEFORE: usize = 3;

/// What the web reading pane says about its page in a bar above it, and
/// the buttons beside the words: each a command, by name, and what the
/// button says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageNotice {
    pub text: String,
    pub actions: Vec<(&'static str, &'static str)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequest {
    /// Open a web or mail address in the user's browser or mail program.
    /// Only ever an address `quill_mime::links::is_openable` accepts.
    OpenUrl(String),
    /// Put text on the system clipboard.
    Copy(String),
    /// Ask the user to choose files with the host's own panel.
    PickFiles,
    /// Ask the user to choose a theme to import, with the host's own
    /// chooser. Answered by [`Action::ThemeImported`], if one is.
    PickTheme,
    /// Bring up the host's window of every key.
    ShowKeys,
    /// Close the host's window and leave Quill running behind it, as the
    /// window's own close button does.
    CloseWindow,
    /// Show files in the host's viewer, starting with the one at `index`.
    /// Each is a path and what to call it.
    Preview {
        files: Vec<(PathBuf, String)>,
        index: usize,
    },
    /// Play a sound or a video.
    Play { path: PathBuf, title: String },
    /// Open a file in the application the Mac opens its kind with. Only
    /// ever on the user's own key press.
    OpenFile(PathBuf),
    /// Open the file the rules are written in, in a host that opens
    /// files for editing. It knows which file: none is named.
    EditRules,
    /// Tell the user about new mail. `actions` are a name, a title and
    /// whether using it should bring Quill forward; the name of the one
    /// chosen comes back in [`Action::Notification`].
    Notify {
        id: String,
        title: String,
        subtitle: Option<String>,
        body: String,
        actions: Vec<(&'static str, &'static str, bool)>,
    },
    /// Show a sanitized page in the web reading pane, in place of what
    /// it showed, with `header` as lines of plain text above it. `remote`:
    /// it may load remote content. `notice`: what to tell the user about
    /// the page, in a bar above it.
    Reading {
        path: PathBuf,
        title: String,
        remote: bool,
        header: Vec<String>,
        notice: Option<PageNotice>,
    },
    /// Scroll what the web reading pane shows by a page: down, or up.
    ScrollReading { down: bool },
    /// Take the web reading pane away.
    CloseReading,
    /// Take notifications down: their mail has been dealt with.
    ClearNotifications(Vec<String>),
    /// Ask the user where to save a copy of a file, with the host's panel.
    SaveFile { path: PathBuf, name: String },
    /// Get a picture ready to draw under `id`: decode the file at `path`,
    /// or have the host picture it when it is not a kind Quill decodes.
    /// Answered by [`Action::PictureLoaded`].
    LoadPicture {
        id: u32,
        path: PathBuf,
        content_type: String,
    },
    /// Have the spelling of the body of the message being written
    /// checked. Only ever that body: never a recipient, a subject or
    /// mail that was received. Answered by [`Action::Spelling`].
    SpellCheck { id: u64, text: String },
    /// Ask what a misspelt word might have been meant to be. Answered by
    /// [`Action::Suggestions`].
    SpellSuggest { id: u64, word: String },
    /// Run Vim on a draft file: `path` in the draft's own folder `dir`.
    /// Quill's app does it in a pane; in a terminal Vim takes the
    /// terminal until it is done. Answered by [`Action::EditorExited`].
    EditDraft {
        path: PathBuf,
        dir: PathBuf,
        title: String,
        editor: ExternalEditor,
        /// The line of the file the body starts on, counting from 1.
        body_line: u32,
    },
    /// Close the host's pane. With Vim in it, that is asking Vim to save
    /// and go.
    ClosePane,
    /// Run one of the user's programs on a message. Only ever on the
    /// user's own key press, and never in Quill's own app. Answered by
    /// [`Action::HookDone`].
    RunHook(Box<HookRun>),
}

/// What the app shell mirrors: enough to enable menu items, badge the Dock
/// and title the window, and nothing about mail itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostState {
    pub context: Context,
    pub enabled: Vec<CommandId>,
    /// The commands that switch something on and off, and have it on.
    pub checked: Vec<CommandId>,
    pub unread: u32,
    pub title: String,
    /// The colour the screen is drawn on and the colour of its text, as
    /// red, green and blue, where the theme fixes them: for a terminal the
    /// host shows beside this one, and the cells nothing paints.
    pub background: Option<(u8, u8, u8)>,
    pub foreground: Option<(u8, u8, u8)>,
    /// Whether the settings say light, dark, or as the system is: the
    /// host's own windows go by it.
    pub appearance: quill_core::config::Appearance,
    /// The font the host's terminals are to have, and its size in points,
    /// as the settings say: nothing a terminal Quill runs in can be told.
    pub font: Option<String>,
    pub font_size: Option<Number>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    List,
    Reader,
}

impl Pane {
    /// Where `tab` goes: between the list and the message, the two panes
    /// of everyday use, and from the accounts to the list.
    fn next(self) -> Self {
        match self {
            Self::Sidebar | Self::Reader => Self::List,
            Self::List => Self::Reader,
        }
    }

    /// Where `shift-tab` goes: to the accounts from either of the other
    /// two, and from the accounts to the message.
    fn prev(self) -> Self {
        match self {
            Self::Sidebar => Self::Reader,
            Self::List | Self::Reader => Self::Sidebar,
        }
    }
}

/// Which of the list and the reader shows when there is room for only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainPane {
    List,
    Reader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

/// A message for the status bar that stays until dismissed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Info,
            text: text.into(),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEntry {
    Unified {
        unread: u32,
    },
    /// Drafts or the outbox: mail that lives only on this Mac.
    Local {
        mailbox: Mailbox,
        name: &'static str,
        count: u32,
    },
    /// An account: its inbox, and the row its folders open and close
    /// under.
    Account {
        index: usize,
        id: LocalAccountId,
        name: String,
        /// What is unread in its inbox: its share of the unified count.
        unread: u32,
    },
    Folder {
        id: LocalFolderId,
        /// The position of the folder's account, for its colour.
        account: usize,
        name: String,
        role: FolderRole,
        unread: u32,
        /// Made on this Mac: no server has it. It is listed under the
        /// heading that says so.
        local: bool,
    },
    /// The heading, within an account, over its folders that are only on
    /// this Mac. The cursor skips it.
    OnThisMac,
    /// The heading over the saved searches. The cursor skips it.
    Searches,
    /// A saved search: a mailbox of whatever it finds.
    Search {
        name: String,
        query: String,
    },
}

impl SidebarEntry {
    pub fn mailbox(&self) -> Option<Mailbox> {
        match self {
            Self::Unified { .. } => Some(Mailbox::Unified),
            Self::Local { mailbox, .. } => Some(*mailbox),
            Self::Account { id, .. } => Some(Mailbox::Account(*id)),
            Self::Searches | Self::Search { .. } | Self::OnThisMac => None,
            Self::Folder { id, .. } => Some(Mailbox::Folder(*id)),
        }
    }

    /// Somewhere the cursor can rest: a mailbox or a saved search, not a
    /// heading.
    pub fn is_place(&self) -> bool {
        self.mailbox().is_some() || matches!(self, Self::Search { .. })
    }
}

#[derive(Debug, Default)]
pub struct Sidebar {
    /// Every row there is, the folders of a closed account among them:
    /// they are not shown, and are still folders to move mail to or to go
    /// to by name.
    pub entries: Vec<SidebarEntry>,
    /// The row the cursor is on, as a place in `entries`. It is always one
    /// that is shown.
    pub selected: usize,
    /// How many of the rows shown are scrolled off the top.
    pub offset: usize,
    /// The accounts whose folders are folded away.
    pub closed: BTreeSet<LocalAccountId>,
}

impl Sidebar {
    /// The account a row belongs to: an account's own row, or one of its
    /// folders.
    pub fn account_of(&self, index: usize) -> Option<LocalAccountId> {
        match self.entries.get(index)? {
            SidebarEntry::Account { id, .. } => Some(*id),
            // An account's folders follow its row.
            SidebarEntry::Folder { .. } => {
                self.entries[..index]
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        SidebarEntry::Account { id, .. } => Some(*id),
                        _ => None,
                    })
            }
            _ => None,
        }
    }

    /// Where an account's own row is.
    pub fn account_row(&self, account: LocalAccountId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| matches!(entry, SidebarEntry::Account { id, .. } if *id == account))
    }

    pub fn is_open(&self, account: LocalAccountId) -> bool {
        !self.closed.contains(&account)
    }

    /// The rows on show, as places in `entries`, top to bottom: all of
    /// them but the folders of a closed account.
    pub fn shown(&self) -> Vec<usize> {
        let mut open = true;
        let mut shown = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            match entry {
                SidebarEntry::Account { id, .. } => open = self.is_open(*id),
                SidebarEntry::Folder { .. } | SidebarEntry::OnThisMac if !open => continue,
                _ => {}
            }
            shown.push(index);
        }
        shown
    }
}

#[derive(Debug, Default)]
pub struct MessageList {
    pub rows: Vec<MessageRow>,
    pub selected: usize,
    pub offset: usize,
    /// The rows are those of the current mailbox, not a mailbox just left.
    pub loaded: bool,
    /// For each row, whether it carries on the conversation of the row
    /// above. Empty when conversations are not grouped.
    pub follows: Vec<bool>,
    /// Messages selected to be acted on together.
    pub marked: BTreeSet<LocalMessageId>,
    /// Where a range being selected began. Every row from there to the
    /// cursor counts as selected, and follows the cursor until the range
    /// is ended.
    pub range: Option<LocalMessageId>,
}

impl MessageList {
    /// The first and last row of the range being selected.
    pub fn range_rows(&self) -> Option<(usize, usize)> {
        let anchor = self.range?;
        let anchor = self.rows.iter().position(|row| row.id == anchor)?;
        Some((anchor.min(self.selected), anchor.max(self.selected)))
    }

    pub fn has_marks(&self) -> bool {
        !self.marked.is_empty() || self.range.is_some()
    }

    /// The selected messages, in the order listed.
    pub fn marked_ids(&self) -> Vec<LocalMessageId> {
        let range = self.range_rows();
        self.rows
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                self.marked.contains(&row.id)
                    || range.is_some_and(|(first, last)| (first..=last).contains(index))
            })
            .map(|(_, row)| row.id)
            .collect()
    }

    /// Ends the range, keeping what it covered selected.
    fn settle_range(&mut self) {
        if let Some((first, last)) = self.range_rows() {
            self.marked
                .extend(self.rows[first..=last].iter().map(|row| row.id));
        }
        self.range = None;
    }

    fn clear_marks(&mut self) {
        self.marked.clear();
        self.range = None;
    }

    /// Forgets selected messages that are no longer listed.
    fn keep_listed_marks(&mut self) {
        if !self.has_marks() {
            return;
        }
        let listed: BTreeSet<LocalMessageId> = self.rows.iter().map(|row| row.id).collect();
        self.marked.retain(|id| listed.contains(id));
        if self.range.is_some_and(|anchor| !listed.contains(&anchor)) {
            self.range = None;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Body {
    #[default]
    None,
    /// Being read from the store.
    Loading,
    /// Not in the store yet: being fetched from the server.
    Downloading,
    Loaded(LoadedBody),
    /// Not in the store and not to be had, with the reason to show.
    Unavailable(String),
}

/// Which version of a message's body the reader shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    Html,
    Plain,
}

/// The folders a message can be moved to, open to choose one from. Typing
/// narrows the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPicker {
    /// The message to move. Its account's folders are the ones on offer.
    pub message: LocalMessageId,
    /// The other messages to move with it.
    pub also: Vec<LocalMessageId>,
    /// The messages are to be copied there, and stay where they are.
    pub copy: bool,
    pub filter: LineInput,
    pub selected: usize,
    pub offset: usize,
}

impl FolderPicker {
    pub fn for_message(message: LocalMessageId) -> Self {
        Self {
            message,
            also: Vec::new(),
            copy: false,
            filter: LineInput::default(),
            selected: 0,
            offset: 0,
        }
    }
}

/// The templates, open to choose one to put into the message being
/// written. Typing narrows the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePicker {
    pub filter: LineInput,
    pub selected: usize,
    pub offset: usize,
    /// How many templates there are before any narrowing, which is how
    /// tall the box is.
    pub total: usize,
}

/// The user's hooks, open to choose one to give a message to. Typing
/// narrows the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPicker {
    /// The message the chosen program is given: the one the user was on
    /// when the list was opened.
    pub message: LocalMessageId,
    pub filter: LineInput,
    pub selected: usize,
    pub offset: usize,
    /// How many hooks there are before any narrowing, which is how tall
    /// the box is.
    pub total: usize,
    /// The same box with something else to choose from in it: the rules,
    /// or what rules did. `None`: the hooks, and `message` is theirs.
    pub list: Option<PickList>,
}

/// Where an editor that has a message's text goes on the terminal's
/// screen, for a host that lays it over the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EditorSpot {
    /// The cells it has. `None`: it is not on show.
    pub place: Option<ratatui::layout::Rect>,
    /// How many of its rows, from the top, something drawn here lies
    /// over: the people on offer under an address being typed.
    pub covered: u16,
    /// It has the keys.
    pub keys: bool,
}

/// A message handed to Vim: what is needed to take it back.
#[derive(Debug)]
struct VimEdit {
    /// The message as it was written out. Reading the file back starts
    /// from this.
    handed: DraftContent,
    /// The draft's own folder, once it is made.
    dir: Option<PathBuf>,
    /// It was given the text alone: the fields never left.
    body_only: bool,
}

/// What to do with a message that has come back from Vim, once the files
/// it newly names have been attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AfterVim {
    verdict: Option<Verdict>,
    /// A file it named could not be attached. The message stays open,
    /// with the reason on show: it is not sent or put away without it.
    attach_failed: bool,
}

/// Other spellings of a word in the message being written, open to choose
/// one from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionPicker {
    /// The word as written.
    pub word: String,
    /// Where it is in the body, in bytes.
    range: std::ops::Range<usize>,
    /// The body as it was when the word was asked about. A choice is only
    /// put into that same text.
    revision: u64,
    pub words: Vec<String>,
    pub selected: usize,
    pub offset: usize,
}

/// The most rows the reader spends saying why the rest of a message is
/// not here.
const READER_NOTE_ROWS: usize = 3;
/// The most spellings offered for one word.
const MOST_SUGGESTIONS: usize = 12;
/// The longest text sent for checking, in bytes. A longer body is not
/// checked.
pub const MOST_SPELL_CHECKED: usize = 100_000;
/// How long an answer about spelling is waited for before a newer text
/// may be sent regardless, in seconds.
const SPELL_PATIENCE: i64 = 15;

/// Where spell checking stands. Ids only ever go up, across every message
/// written in a run, so an answer can never be taken for another's.
#[derive(Debug, Default)]
struct Spell {
    last_id: u64,
    /// The check sent and not yet answered: its id, the revision of the
    /// body that was sent (`None` once that message has been put away),
    /// and when it went.
    flight: Option<(u64, Option<u64>, i64)>,
    /// The revision of the body last sent for checking.
    sent: Option<u64>,
    /// A word whose other spellings have been asked for: the request's
    /// id, where the word is, and the revision of the body it is in.
    asking: Option<(u64, std::ops::Range<usize>, u64)>,
}

/// Where the downloaded copy of the message on show stands, when only its
/// text was at hand as it opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Rest {
    /// Nothing is missing, or nothing has been found missing yet.
    #[default]
    Here,
    /// Being fetched from the server again.
    Fetching,
    /// Not to be had, and why. The text stays on show.
    Failed(String),
}

/// The question asked before anything is done to unsubscribe from a list:
/// whose list it is, and exactly what saying yes does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribePrompt {
    /// The account the list's mail came to. A message asking to leave is
    /// written from it.
    pub account: LocalAccountId,
    /// Who the list's mail is from, as the reader shows it.
    pub sender: String,
    pub how: UnsubscribeHow,
}

/// What the question about unsubscribing says first.
pub const MAILING_LIST: &str = "This message is from a mailing list.";

/// What the question about unsubscribing says of the way on offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribeSays<'a> {
    /// What saying yes does, exactly.
    pub does: String,
    /// And what it does not.
    pub does_not: &'static str,
    /// What the address shown is called.
    pub label: &'static str,
    /// The address itself: the one the request, the message or the
    /// browser goes to.
    pub shown: &'a str,
    /// What saying yes is called.
    pub yes: &'static str,
}

impl UnsubscribePrompt {
    /// The words of the question, wherever it is asked.
    pub fn says(&self) -> UnsubscribeSays<'_> {
        match &self.how {
            UnsubscribeHow::OneClick { url, host } => UnsubscribeSays {
                does: format!("Quill will tell {host} to unsubscribe you."),
                does_not: "One request goes to the address below, which the list gave for this. It says \"unsubscribe\" and nothing else: no cookies, and no page is opened.",
                label: "Address:",
                shown: url,
                yes: "unsubscribe",
            },
            UnsubscribeHow::Mail { url: _, address } => UnsubscribeSays {
                does: format!("A message will be written to {address}."),
                does_not: "It is not sent for you. Look it over, then send it yourself to unsubscribe, or close it to do nothing.",
                label: "To:",
                shown: address,
                yes: "write the message",
            },
            UnsubscribeHow::Page { url, host } => UnsubscribeSays {
                does: format!("Your browser will open a page at {host}."),
                does_not: "This list has no way for Quill to unsubscribe you itself: its page says what to do next. Quill sends nothing.",
                label: "Address:",
                shown: url,
                yes: "open the page",
            },
        }
    }
}

/// The way of unsubscribing that is on offer. Every address in it is the
/// list's own, from the message's `List-Unsubscribe` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsubscribeHow {
    /// Quill tells the list's server itself, with one request that says
    /// only "unsubscribe" (RFC 8058).
    OneClick { url: String, host: String },
    /// A message to the list is started. The user sends it.
    Mail { url: String, address: String },
    /// The list's page is opened in the browser. Quill sends nothing.
    Page { url: String, host: String },
}

/// A folder on offer in the folder picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderChoice {
    pub id: LocalFolderId,
    pub name: String,
    /// The folder is only on this Mac: said beside its name.
    pub local: bool,
}

/// What is said after the name of a folder that is only on this Mac,
/// wherever a folder is named in a line of its own.
pub const ON_THIS_MAC_SUFFIX: &str = " (on this Mac)";

/// The picture the reader draws above a message's text: the first of its
/// files that is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// What the terminal knows the picture by.
    pub id: u32,
    /// Which part of the message it is.
    pub part: u32,
    pub name: String,
    /// Its size in pixels once it is ready to draw. `None`: still being
    /// read.
    pub size: Option<(u32, u32)>,
}

/// The number the terminal keeps a message's picture under. The same
/// message and part always give the same number, so a picture already sent
/// is not sent again. Zero is never used: the protocol keeps it for "none".
fn picture_id(message: LocalMessageId, part: u32) -> u32 {
    let mixed = (message.0 as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(u64::from(part).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    // The top bits are the best mixed.
    ((mixed >> 33) as u32) | 1
}

/// Sound or moving pictures: something to play, not to look at.
fn is_playable(content_type: &str) -> bool {
    content_type.starts_with("audio/") || content_type.starts_with("video/")
}

/// Whether a file of this kind can be drawn in the reader: a picture
/// Quill decodes itself, or, with a host that makes thumbnails, one it
/// can be given a picture of.
fn is_picturable(content_type: &str, thumbnails: bool) -> bool {
    quill_media::decode::can_decode(content_type)
        || (thumbnails
            && (content_type.starts_with("image/")
                || content_type.starts_with("video/")
                || content_type == "application/pdf"))
}

/// A search in progress: what has been typed, and whether typing is still
/// going on. While there is one, the list shows what it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub input: LineInput,
    /// The keys go to the box. Otherwise they go to the panes, and the
    /// results are a list like any other.
    pub typing: bool,
    /// How many were found, once an answer for what is typed has come.
    pub found: Option<usize>,
    /// There are more than the list shows.
    pub more: bool,
    /// Not a search at all: a rule being tried, and the list is what it
    /// would change.
    pub rule: Option<RuleTrial>,
}

/// A box asking for a time, and what the time is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhenPrompt {
    pub input: LineInput,
    pub what: WhenFor,
    /// Why what was typed would not do.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenFor {
    /// When these messages come back.
    Snooze(Vec<LocalMessageId>),
    /// When the message being written is sent.
    SendLater,
}

impl WhenPrompt {
    /// What the question is called: what the time is for.
    pub fn title(&self) -> String {
        match &self.what {
            WhenFor::Snooze(ids) if ids.len() > 1 => {
                format!("Snooze {} messages until", ids.len())
            }
            WhenFor::Snooze(_) => "Snooze until".to_owned(),
            WhenFor::SendLater => "Send at".to_owned(),
        }
    }

    /// What is said under the examples: what else can be typed, or what
    /// becomes of the message until then.
    pub fn aside(&self) -> &'static str {
        match self.what {
            WhenFor::Snooze(_) => "\"now\" brings a snoozed message straight back.",
            WhenFor::SendLater => {
                "It waits in the Outbox and goes at that time if Quill is running, or when it next is."
            }
        }
    }
}

/// The list of a message's attachments, open to choose one from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttachmentPicker {
    pub selected: usize,
    pub offset: usize,
}

/// The list of a message's links, open to choose one from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkPicker {
    pub selected: usize,
    pub offset: usize,
    /// The digits typed so far of a link's number.
    pub typed: String,
}

#[derive(Debug, Default)]
pub struct Reader {
    pub row: Option<MessageRow>,
    pub body: Body,
    /// The version the user switched this message to, if they did.
    pub chosen: Option<Showing>,
    /// Where the links in the body as shown lead, in number order.
    pub links: Vec<String>,
    /// The body laid out for `wrap_width`: wrapped plain text, or the
    /// message's HTML as styled text.
    pub lines: Vec<Line>,
    pub wrap_width: Option<usize>,
    pub scroll: usize,
    /// The picture drawn above the text, when the message has one.
    pub picture: Option<Picture>,
    /// The user asked for this message's remote content.
    pub remote: bool,
    /// The card for the invitation the message carries, when it carries
    /// one: drawn between the headers and the text.
    pub card: Vec<CardRow>,
    /// Whether the rest of the message, beyond its text, is here.
    pub rest: Rest,
    /// What is said about that above the text, laid out for the pane: a
    /// line while it is being fetched, and up to three when it could not
    /// be, so that the reason can be read.
    pub note: Vec<String>,
    /// What of the message is selected, to copy.
    pub selection: Option<Selection>,
}

impl Reader {
    /// The files attached to the message on show.
    pub fn files(&self) -> &[AttachedFile] {
        match &self.body {
            Body::Loaded(body) => &body.files,
            _ => &[],
        }
    }

    /// The version of the body on show, once there is a body.
    pub fn showing(&self) -> Option<Showing> {
        let Body::Loaded(body) = &self.body else {
            return None;
        };
        Some(match (&body.html, self.chosen) {
            (None, _) => Showing::Plain,
            // With no plain part there is only the HTML to show.
            (Some(_), _) if !body.has_plain => Showing::Html,
            (Some(_), Some(chosen)) => chosen,
            (Some(_), None) if body.html_first => Showing::Html,
            (Some(_), None) => Showing::Plain,
        })
    }

    /// The message came as both HTML and plain text.
    pub fn has_two_versions(&self) -> bool {
        matches!(&self.body, Body::Loaded(body) if body.html.is_some() && body.has_plain)
    }

    /// What to say about the rest of the message while only its text is
    /// here.
    pub fn rest_note(&self) -> Option<String> {
        match &self.rest {
            Rest::Here => None,
            Rest::Fetching => Some("Fetching the rest of this message…".to_owned()),
            Rest::Failed(why) => Some(format!(
                "The rest of this message could not be fetched: {why}"
            )),
        }
    }
}

/// What the web reading pane shows, or has been asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// A message's page: its HTML, made safe.
    Page(LocalMessageId),
    /// A message's text, while its downloaded copy is not at hand.
    Text(LocalMessageId),
    /// Words of Quill's own: why there is no message in the pane.
    Note(String),
}

/// How the web reading pane follows the list.
///
/// The pane shows the message under the list's cursor, once the cursor
/// has rested there for a moment: moving through the list quickly asks for
/// no page on the way, and a page that arrives for a message the cursor
/// has left is not shown.
#[derive(Debug, Default)]
struct Follow {
    /// The message under the list's cursor when last looked at.
    target: Option<LocalMessageId>,
    /// The cursor has rested there long enough for it to be shown.
    settled: bool,
    /// How many times the cursor has moved on to another message: tells
    /// the run loop a new wait from the one it is already timing.
    moves: u64,
    /// The user did something in this update that takes them to the
    /// message under the cursor: moved to it, clicked it, opened it.
    went: bool,
    /// The user opened the message (`enter`): shown now, not in a moment.
    now: bool,
    /// The user went to the target themselves, so that once it is shown
    /// it has been opened, and is read. Not so for the message the cursor
    /// happens to be on when a mailbox opens or a search finds something.
    by_user: bool,
    /// What the pane was last asked to show.
    asked: Option<Shown>,
    /// Whether the page last asked for was to be a dark one.
    dark: bool,
    /// The target's page could not be made, and why: the reason is what
    /// the pane shows for it, and the page is not asked for again until
    /// the cursor has been elsewhere or the message is read afresh.
    failed: Option<String>,
}

/// What the web reading pane says when the list has nothing under its
/// cursor. The same words the reader has for it.
const NO_MESSAGE: &str = "No message selected";
/// And while a message that was never downloaded is fetched.
const DOWNLOADING: &str = "Downloading this message…";
/// How wide a message is laid out to find its links while no reader is
/// drawn to lay it out for.
const UNDRAWN_WIDTH: usize = 80;
/// The longest a line above the page in the web reading pane is let be.
/// The pane cuts it to its own width; this keeps a message with a
/// thousand recipients from sending them all.
const READING_HEADER_COLUMNS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    /// The context help was opened from, and so describes.
    pub context: Context,
    pub scroll: usize,
    /// It lists every key Quill has, not only the ones for `context`.
    pub all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Next,
    Prev,
    Top,
    Bottom,
    PageDown,
    PageUp,
}

pub struct State {
    pub size: (u16, u16),
    pub focus: Pane,
    pub main_pane: MainPane,
    pub zoomed: bool,
    pub accounts: Vec<AccountRow>,
    pub sidebar: Sidebar,
    pub mailbox: Mailbox,
    pub list: MessageList,
    pub reader: Reader,
    pub help: Option<Help>,
    pub links: Option<LinkPicker>,
    pub attachments: Option<AttachmentPicker>,
    pub search: Option<Search>,
    pub palette: Option<Palette>,
    pub folders: Option<FolderPicker>,
    /// The templates to choose from, over the message being written.
    pub templates: Option<TemplatePicker>,
    /// Other spellings of a word to choose from, over the message being
    /// written.
    pub suggestions: Option<SuggestionPicker>,
    /// The hooks to choose from, over the message one is for.
    pub hooks: Option<HookPicker>,
    /// The hook that is running, by name. One runs at a time.
    hook_running: Option<String>,
    /// The message Vim has, while it has it.
    vim: Option<VimEdit>,
    /// A message has just opened that is to go to Vim at once.
    vim_wanted: bool,
    /// What Vim said to do with the message it gave back.
    after_vim: Option<AfterVim>,
    spell: Spell,
    /// The question to answer before unsubscribing from a list.
    pub unsubscribe: Option<UnsubscribePrompt>,
    /// The question to answer before an account is removed.
    pub remove: Option<RemovePrompt>,
    /// The question of which letter stands for an account in lists.
    pub letter_prompt: Option<LetterPrompt>,
    /// The service was asked for a letter from that question, and has
    /// not said yet: its answer is announced.
    letter_asked: bool,
    /// The service has been asked what removing an account would lose,
    /// and has not said yet: the question follows its answer.
    remove_asked: bool,
    /// The service has been asked how an account is set up, to change
    /// it, and has not said yet: the form follows its answer.
    edit_asked: bool,
    /// The message being written, in the reader's place.
    pub compose: Option<Compose>,
    /// A message the store worker has prepared to write, opened at the end
    /// of the update that brought it.
    compose_pending: Option<Box<Prefill>>,
    /// A reply or forward waiting for its message to be downloaded.
    respond_pending: Option<(LocalMessageId, DataRequest)>,
    /// The user has switched grouping by conversation the other way from
    /// what the config says, for this run.
    ungrouped: bool,
    /// A message to go to once the list that has it is loaded, and
    /// whether to start a reply to it: what a notification asked for.
    go_to: Option<(LocalMessageId, bool)>,
    /// The messages a notification is up for.
    announced: BTreeSet<LocalMessageId>,
    /// What was just done, in a few words, shown until the next key, and
    /// whether the undo command would take it back.
    pub flash: Option<(String, bool)>,
    /// The add-an-account flow, while it is open.
    pub setup: Option<Setup>,
    /// The host shows that form and the questions (a sign-in, a
    /// certificate, a time, unsubscribing), in a sheet of its own
    /// ([`Env::sheets`]), so none of them is drawn over the panes.
    pub form_hosted: bool,
    /// Web reading is on: mail is read in the host's web view pane, which
    /// follows the list, and no reader is drawn here.
    pub web_reading: bool,
    /// The user has switched web reading the other way from what the
    /// config says, in this run.
    web_flipped: bool,
    /// Mail in the web pane is shown dark while Quill is dark: the config
    /// and the user's switching in this run come out at on.
    pub dark_pages: bool,
    /// The user has switched that the other way from what the config
    /// says, in this run.
    dark_flipped: bool,
    /// A page made now for the web pane is a dark one: Quill is dark, and
    /// mail is shown dark while it is. Worked out as each update starts,
    /// and again in the update that hears Quill has gone dark or light.
    pages_dark: bool,
    /// The accounts pane is hidden: it has no columns beside the other
    /// panes, and is drawn over them while it has the keys. Worked out
    /// from the config and [`State::sidebar_flipped`] at every update.
    pub sidebar_hidden: bool,
    /// The user has switched the accounts pane the other way from the
    /// one Quill started with.
    sidebar_flipped: bool,
    /// How many columns the accounts pane has beside the others, where
    /// the user has said: in the config, or by dragging its edge in this
    /// run. `None`: by the window's width. Worked out from the config and
    /// [`State::sidebar_dragged`] at every update.
    pub sidebar_width: Option<u16>,
    /// Where the user has left the accounts pane's edge in this run, in
    /// place of what the config says: so many columns, or back to going
    /// by the window's width.
    sidebar_dragged: Option<Option<u16>>,
    /// The message's share of the room right of the accounts, where the
    /// user has dragged an edge in this run: it stands in place of the
    /// config's.
    pub reader_share: Option<f32>,
    /// An edge between two panes the user has hold of with the mouse.
    pub resizing: Option<Resizing>,
    /// The edge last pressed and let go of without moving it, and when: a
    /// second press on it straight away puts it back where Quill has it.
    divider_pressed: Option<(layout::Divider, i64)>,
    /// The main button is down in the reader: a click or a selection, not
    /// yet known which.
    held: Option<Held>,
    /// The last press in the reader, for a double click to be told by.
    clicked: Option<Clicked>,
    /// What an editor's keys do is said above it. Worked out from the
    /// config and [`State::tips_asked`] at every update.
    pub editor_tips: bool,
    /// The user has asked for that to be shown or hidden, whatever the
    /// config says.
    tips_asked: Option<bool>,
    /// It has been on show above an editor: once that message is done
    /// with it has been seen, and is not shown again unasked.
    tips_seen: bool,
    /// How many columns the host's window has room for, once it has said:
    /// the terminal's own while it has the window to itself, and more
    /// than that while a pane of the host's stands beside it.
    pub window_columns: Option<u16>,
    follow: Follow,
    /// The mail around the list's cursor that was last asked for.
    near_asked: Vec<LocalMessageId>,
    /// A sign-in to an account's provider, waiting on the user.
    pub sign_in: Option<SignIn>,
    /// The host's settings window, while it is open.
    pub settings: Option<SettingsWindow>,
    /// A time being asked for.
    pub when: Option<WhenPrompt>,
    /// The box asking what a new folder is to be called, and where it is
    /// to be.
    pub new_folder: Option<FolderPrompt>,
    /// The form for a new rule, started from a message.
    pub new_rule: Option<RulePrompt>,
    /// The rules as they were last read. The engine runs them as mail
    /// arrives; here they are listed, tried and run by hand.
    pub rules: std::sync::Arc<quill_sync::RuleSet>,
    /// What is wrong with the rules as written, as last said.
    rules_problem: Option<String>,
    /// The question, in a host's sheet, about the keys of a command.
    pub keys: Option<KeysPrompt>,
    /// The question, in a host's sheet, about a theme: its colours and
    /// its name, or whether it is to be deleted.
    pub theme_prompt: Option<ThemePrompt>,
    /// When the next snoozed message is due back.
    next_wake: Option<i64>,
    /// Certificates of existing accounts waiting for a decision. The first
    /// is the one on screen.
    pub certificates: VecDeque<CertificatePrompt>,
    /// The flow was offered once because there were no accounts.
    setup_offered: bool,
    /// The store has said which accounts exist.
    accounts_known: bool,
    /// Keys pressed so far of a sequence such as `g g`.
    pub pending: Vec<KeyChord>,
    pub notices: VecDeque<Notice>,
    pub now: i64,
    /// The accounts with a sync pass in progress.
    pub syncing: BTreeSet<LocalAccountId>,
    pub should_quit: bool,
    /// The last changes made from outside the window, newest first, as
    /// the settings window shows them.
    pub agent_recent: Vec<quill_store::AgentEntry>,
    /// How many notifications about agents have been posted, to give
    /// each one about a message sent, or a connection, a name of its own.
    agent_notes: u64,
    /// The screen no longer matches the state and needs a redraw.
    pub dirty: bool,
}

impl State {
    pub fn new(size: (u16, u16), now: i64) -> Self {
        Self {
            size,
            focus: Pane::List,
            main_pane: MainPane::List,
            zoomed: false,
            accounts: Vec::new(),
            sidebar: Sidebar::default(),
            mailbox: Mailbox::Unified,
            list: MessageList::default(),
            reader: Reader::default(),
            help: None,
            links: None,
            attachments: None,
            search: None,
            palette: None,
            folders: None,
            templates: None,
            suggestions: None,
            hooks: None,
            hook_running: None,
            vim: None,
            vim_wanted: false,
            after_vim: None,
            spell: Spell::default(),
            unsubscribe: None,
            remove: None,
            letter_prompt: None,
            letter_asked: false,
            remove_asked: false,
            edit_asked: false,
            compose: None,
            compose_pending: None,
            respond_pending: None,
            ungrouped: false,
            go_to: None,
            announced: BTreeSet::new(),
            flash: None,
            setup: None,
            form_hosted: false,
            web_reading: false,
            web_flipped: false,
            dark_pages: true,
            dark_flipped: false,
            pages_dark: false,
            sidebar_hidden: false,
            sidebar_flipped: false,
            sidebar_width: None,
            sidebar_dragged: None,
            reader_share: None,
            resizing: None,
            divider_pressed: None,
            held: None,
            clicked: None,
            editor_tips: false,
            tips_asked: None,
            tips_seen: false,
            window_columns: None,
            follow: Follow::default(),
            near_asked: Vec::new(),
            sign_in: None,
            settings: None,
            when: None,
            new_folder: None,
            new_rule: None,
            rules: std::sync::Arc::default(),
            rules_problem: None,
            keys: None,
            theme_prompt: None,
            next_wake: None,
            certificates: VecDeque::new(),
            setup_offered: false,
            accounts_known: false,
            pending: Vec::new(),
            notices: VecDeque::new(),
            now,
            syncing: BTreeSet::new(),
            should_quit: false,
            agent_recent: Vec::new(),
            agent_notes: 0,
            dirty: true,
        }
    }

    /// What to ask the store for when the UI starts.
    pub fn startup_requests(&self) -> Vec<DataRequest> {
        vec![
            DataRequest::LoadSidebar,
            DataRequest::LoadMessages(self.mailbox),
        ]
    }

    pub fn context(&self) -> Context {
        if self.help.is_some() {
            return Context::Help;
        }
        // A certificate is asked about before anything else goes on: mail
        // from that server waits on the answer.
        if !self.certificates.is_empty() {
            return Context::Certificate;
        }
        if self.sign_in.is_some() {
            return Context::SignIn;
        }
        if self.when.is_some() {
            return Context::When;
        }
        if self.new_folder.is_some() {
            return Context::NewFolder;
        }
        if self.letter_prompt.is_some() {
            return Context::AccountLetter;
        }
        if self.new_rule.is_some() {
            return Context::NewRule;
        }
        if self.keys.is_some() {
            return Context::RecordKeys;
        }
        if self.theme_prompt.is_some() {
            return Context::EditTheme;
        }
        if self.unsubscribe.is_some() {
            return Context::Unsubscribe;
        }
        if self.remove.is_some() {
            return Context::RemoveAccount;
        }
        if self.setup.is_some() {
            return Context::Setup;
        }
        if self.palette.is_some() {
            return Context::Palette;
        }
        if self.links.is_some() {
            return Context::Links;
        }
        if self.attachments.is_some() {
            return Context::Attachments;
        }
        if self.folders.is_some() {
            return Context::Folders;
        }
        if self.templates.is_some() {
            return Context::Templates;
        }
        if self.suggestions.is_some() {
            return Context::Suggestions;
        }
        if self.hooks.is_some() {
            return Context::Hooks;
        }
        if self.compose.is_some() {
            return Context::Compose;
        }
        if self.search.as_ref().is_some_and(|search| search.typing) {
            return Context::Search;
        }
        match self.focus {
            Pane::Sidebar => Context::Sidebar,
            Pane::List => Context::List,
            Pane::Reader => Context::Reader,
        }
    }

    /// Applies one action and returns the store requests it calls for.
    pub fn update(&mut self, env: &Env, action: Action) -> Effects {
        let mut out = Effects::default();
        let mut changed = true;
        // Where a form or a question is shown is the host's to say, and
        // the layout goes by it.
        self.form_hosted = env.sheets;
        // And so is whether there is a pane to read mail in.
        self.web_reading = self.web_reading_wanted(env);
        self.dark_pages = env.dark_pages != self.dark_flipped;
        self.pages_dark = self.dark_pages && env.look.is_dark();
        self.sidebar_hidden = env.sidebar == self.sidebar_flipped;
        self.sidebar_width = self.sidebar_dragged.unwrap_or(env.sidebar_width);
        self.editor_tips = self.tips_asked.unwrap_or(env.editor_tips);
        // A press on an edge between panes is half of a double click only
        // while nothing else is done in between.
        if matches!(
            action,
            Action::Key(_) | Action::Command(_) | Action::Paste(_)
        ) {
            self.divider_pressed = None;
        }
        match action {
            Action::Key(chord) => self.on_key(env, chord, &mut out),
            // Asked for from the host's menu, the list of every key goes
            // in the host's window, where it can stay up. The one drawn
            // here, if it is up, has nothing left to do.
            Action::Command(CommandId::HelpKeys)
                if env.keys_window && self.is_enabled(env, CommandId::HelpKeys) =>
            {
                if self.help.is_some_and(|help| help.all) {
                    self.help = None;
                }
                out.host.push(HostRequest::ShowKeys);
            }
            Action::Command(id) => self.run(env, id, &mut out),
            Action::Resize(width, height) => self.size = (width, height),
            Action::Data(event) => self.on_data(env, event, &mut out),
            Action::Mouse(mouse) => {
                // What the last key did has been seen.
                self.flash = None;
                self.on_mouse(env, mouse, &mut out);
            }
            Action::PictureLoaded { id, size } => {
                if let Some(picture) = &mut self.reader.picture
                    && picture.id == id
                {
                    match size {
                        Some(size) => picture.size = Some(size),
                        // Not a picture after all. The file is still in
                        // the list.
                        None => self.reader.picture = None,
                    }
                }
            }
            Action::TypingPaused => {
                // Nothing on screen changes by asking.
                changed = false;
                self.check_spelling(env, &mut out);
            }
            Action::Spelling { id, words } => self.on_spelling(id, &words),
            Action::Suggestions { id, words } => self.on_suggestions(id, words),
            Action::EditorExited { ran, status } => {
                self.on_editor_exited(env, ran, status, &mut out);
            }
            Action::FormUsed(used) => self.on_form(env, &used, &mut out),
            Action::SelectionSettled => {
                // Nothing drawn here changes: the page is the host's.
                changed = false;
                self.follow.settled = true;
            }
            Action::WindowColumns(columns) => {
                // Told again what it was told before, the host is asking
                // where its pane goes: it is answered, and nothing drawn
                // here changes.
                changed = self.window_columns != Some(columns);
                self.window_columns = Some(columns);
                out.place_again = !changed;
            }
            Action::ReadingDragged(columns) => self.reading_dragged(env, columns, &mut out),
            Action::HookDone { name, outcome } => {
                self.hook_running = None;
                let said = outcome.said(&name);
                if outcome.succeeded() {
                    self.flash = Some((said, false));
                } else {
                    // A failure is not to be missed for the next key.
                    self.notices.push_back(Notice::error(said));
                }
            }
            Action::ClockTick(now) => {
                // Dates are drawn relative to today, so only a new day shows.
                changed = text::local_day(now, &env.tz) != text::local_day(self.now, &env.tz);
                self.now = now;
                // A snoozed message whose time has come is brought back.
                if self.next_wake.take_if(|at| *at <= now).is_some() {
                    out.data.push(DataRequest::Wake);
                }
                // What is being written is saved as it goes, so a crash
                // costs a few seconds of typing at most.
                if let Some(compose) = &self.compose
                    && compose.is_modified()
                    && !compose.is_blank()
                    && !compose.sending
                {
                    out.data.push(DataRequest::SaveDraft {
                        draft: compose.draft,
                        account: compose.account,
                        content: Box::new(compose.content()),
                        announce: false,
                    });
                }
            }
            Action::Notify(notice) => self.notices.push_back(notice),
            Action::AgentNotice(notice) => self.on_agent_notice(env, &notice, &mut out),
            Action::AgentConnected(name) => self.on_agent_connected(env, &name, &mut out),
            Action::Sync(event) => self.on_sync(env, event, &mut out),
            Action::Account(event) => self.on_account(env, event, &mut out),
            Action::Settings(act) => self.on_settings(env, act, &mut out),
            // The run loop puts it where the next frame finds it: `env`
            // is not this function's to change.
            Action::SystemAppearance { dark } => {
                if env.look.system_dark != Some(dark) {
                    out.system_dark = Some(dark);
                    // The page on show is made again for the look Quill
                    // is about to have.
                    self.pages_dark = self.dark_pages && env.look.appearance.is_dark(Some(dark));
                }
            }
            Action::ThemeImported(name) => self.on_theme_imported(env, name, &mut out),
            Action::Paste(text) => match self.context() {
                Context::Setup => {
                    if let Some(setup) = &mut self.setup {
                        setup.paste(&text);
                    }
                }
                Context::Folders => {
                    if let Some(picker) = &mut self.folders {
                        picker.filter.insert(&text);
                        picker.selected = 0;
                    }
                }
                Context::NewRule => {
                    if let Some(prompt) = &mut self.new_rule
                        && !prompt.busy
                        && let Some(input) = prompt.typing()
                    {
                        // One line: a pasted line break is a space.
                        input.insert(&text.replace(['\r', '\n'], " "));
                        prompt.problem = None;
                    }
                }
                Context::Templates => {
                    if let Some(picker) = &mut self.templates {
                        picker.filter.insert(&text);
                        picker.selected = 0;
                        picker.offset = 0;
                    }
                }
                Context::Hooks => {
                    if let Some(picker) = &mut self.hooks {
                        picker.filter.insert(&text);
                        picker.selected = 0;
                        picker.offset = 0;
                    }
                }
                Context::Palette => {
                    if let Some(palette) = &mut self.palette {
                        palette.input.insert(&text.replace(['\r', '\n'], " "));
                        palette.selected = 0;
                        palette.offset = 0;
                    }
                }
                Context::Search => {
                    if let Some(search) = &mut self.search {
                        // One line: a pasted line break is a space.
                        search.input.insert(&text.replace(['\r', '\n'], " "));
                    }
                    self.run_search(env, &mut out);
                }
                Context::Compose => {
                    if let Some(compose) = &mut self.compose
                        && let Did::Lookup(typed) = compose.paste(&text)
                    {
                        out.data.push(DataRequest::Contacts { typed });
                    }
                }
                _ => {}
            },
            Action::Mailto(url) => self.on_mailto(env, &url),
            Action::Notification { id, action } => {
                self.on_notification(env, &id, action.as_deref(), &mut out);
            }
            Action::FileSaved { name, saved } => {
                self.flash = Some((
                    if saved {
                        text::sanitize_line(&format!("Saved {name}."))
                    } else {
                        "Not saved.".to_owned()
                    },
                    false,
                ));
            }
            Action::Files { paths, temporary } => {
                // Files are for a message: the one being written, or a new
                // one started for them.
                if self.compose.is_none() && self.context() != Context::Setup {
                    self.start_composing(env);
                }
                if let Some(compose) = &mut self.compose {
                    compose.attaching = None;
                }
                let handed = Some(if temporary {
                    Handed::Temporary
                } else {
                    Handed::Kept
                });
                // Asked for even with no message to put them on, so that
                // copies made for Quill are cleared away.
                out.data.extend(
                    paths
                        .into_iter()
                        .map(|path| DataRequest::Attach { path, handed }),
                );
            }
        }
        if let Some(prefill) = self.compose_pending.take() {
            if self
                .compose
                .as_ref()
                .is_some_and(|compose| compose.away.is_some())
            {
                // Vim has a message, and it is not put aside for another.
                self.flash = Some((
                    format!(
                        "A message is being written in {}. Finish it first.",
                        env.external.name()
                    ),
                    false,
                ));
            } else {
                self.open_compose(env, *prefill);
            }
        }
        // With `editor = "vim"` a message goes to Vim as soon as it is
        // open with what it starts from.
        if std::mem::take(&mut self.vim_wanted) {
            self.start_vim(env, &mut out);
        }
        // What the editor's keys do has been on show for a whole message:
        // it has been seen, and is not shown again unless it is asked for.
        if self.tips_seen && self.compose.is_none() {
            self.tips_seen = false;
            self.tips_asked = Some(false);
            self.editor_tips = false;
            out.accounts
                .push(AccountRequest::Remember(Preference::EditorTips(false)));
        }
        self.follow_selection(env, &mut out);
        self.ask_for_mail_nearby(&mut out);
        self.follow_theme_draft(env, &mut out);
        self.settle_notifications(&mut out);
        // With no accounts there is nothing to read: offer to add one,
        // once, instead of an empty window.
        if env.accounts && self.accounts_known && self.accounts.is_empty() && !self.setup_offered {
            self.setup_offered = true;
            self.setup = Some(Setup::default());
        }
        self.relayout(env);
        self.dirty |= changed;
        out
    }

    fn on_key(&mut self, env: &Env, chord: KeyChord, out: &mut Effects) {
        // What the last key did has been seen.
        self.flash = None;
        // Where there is typing to do, a key is typing first and a command
        // only if the field has no use for it.
        match self.context() {
            Context::Compose => {
                let accounts: Vec<LocalAccountId> =
                    self.accounts.iter().map(|account| account.id).collect();
                if let Some(compose) = &mut self.compose {
                    match compose.key(chord, &accounts) {
                        Did::Nothing => {}
                        Did::Handled => return,
                        Did::Copy(text) => {
                            out.host.push(HostRequest::Copy(text));
                            return;
                        }
                        Did::Lookup(typed) => {
                            out.data.push(DataRequest::Contacts { typed });
                            return;
                        }
                        Did::Attach(path) => {
                            out.data.push(DataRequest::Attach { path, handed: None });
                            return;
                        }
                    }
                }
            }
            Context::When => {
                if let Some(when) = &mut self.when
                    && when.input.key(chord)
                {
                    when.problem = None;
                    return;
                }
            }
            Context::NewFolder => {
                if let Some(prompt) = &mut self.new_folder
                    && prompt.key(chord)
                {
                    return;
                }
            }
            Context::AccountLetter => {
                if let Some(prompt) = &mut self.letter_prompt
                    && prompt.key(chord)
                {
                    return;
                }
            }
            Context::NewRule => {
                if let Some(prompt) = &mut self.new_rule
                    && prompt.key(chord)
                {
                    return;
                }
            }
            Context::Folders => {
                if let Some(picker) = &mut self.folders
                    && picker.filter.key(chord)
                {
                    // A narrower list starts again from its top.
                    picker.selected = 0;
                    picker.offset = 0;
                    return;
                }
            }
            Context::Templates => {
                if let Some(picker) = &mut self.templates
                    && picker.filter.key(chord)
                {
                    // A narrower list starts again from its top.
                    picker.selected = 0;
                    picker.offset = 0;
                    return;
                }
            }
            Context::Hooks => {
                if let Some(picker) = &mut self.hooks
                    && picker.filter.key(chord)
                {
                    // A narrower list starts again from its top.
                    picker.selected = 0;
                    picker.offset = 0;
                    return;
                }
            }
            Context::Palette => {
                if let Some(palette) = &mut self.palette
                    && palette.input.key(chord)
                {
                    // A narrower list starts again from its top.
                    palette.selected = 0;
                    palette.offset = 0;
                    return;
                }
            }
            Context::Search => {
                let before = self.search_text();
                if let Some(search) = &mut self.search
                    && search.input.key(chord)
                {
                    if self.search_text() != before {
                        self.run_search(env, out);
                    }
                    return;
                }
            }
            Context::Setup => {
                if let Some(requests) = self.setup.as_mut().and_then(|setup| setup.key(chord)) {
                    out.accounts.extend(requests);
                    return;
                }
            }
            // The question is in a sheet of the host's, and is answered
            // there: a letter typed here must not accept a certificate
            // that is not on this screen.
            Context::Certificate if self.form_hosted && matches!(chord.key, Key::Char(_)) => {
                return;
            }
            Context::Certificate if self.answer_certificate(chord, out) => return,
            // Waiting on the browser: only giving up is on offer, and a
            // letter must not run a command behind the dialog.
            Context::SignIn if matches!(chord.key, Key::Char(_)) => return,
            // A question with two answers, enter and esc. A letter is
            // neither, and must not run a command behind it.
            Context::Unsubscribe if matches!(chord.key, Key::Char(_)) => return,
            // In a sheet of the host's it is answered there, and a letter
            // typed here must not answer what is not on this screen. Drawn
            // here, one letter says yes and no other does anything.
            Context::RemoveAccount if self.form_hosted && matches!(chord.key, Key::Char(_)) => {
                return;
            }
            Context::RemoveAccount if self.answer_remove_key(chord, out) => return,
            _ => {}
        }
        self.pending.push(chord);
        match env
            .keymap
            .resolve(&env.registry, &self.pending, self.context())
        {
            Resolution::Command(id) => {
                self.pending.clear();
                self.run(env, id, out);
            }
            Resolution::Pending => {}
            Resolution::Unbound => {
                let broke_a_sequence = self.pending.len() > 1;
                self.pending.clear();
                if !broke_a_sequence && self.type_link_number(chord) {
                    return;
                }
                if broke_a_sequence {
                    // The key that broke it may mean something by itself.
                    self.on_key(env, chord, out);
                }
            }
        }
    }

    fn run(&mut self, env: &Env, id: CommandId, out: &mut Effects) {
        let context = self.context();
        if !self.is_enabled(env, id) {
            return;
        }
        // While a message is being written, tab moves between its fields.
        if context == Context::Compose
            && let Some(compose) = &mut self.compose
        {
            match id {
                // In an editor that has all of it, the message has no
                // field here to move to.
                CommandId::FocusNext | CommandId::FocusPrev if !compose.fields_in_reach() => {
                    return;
                }
                CommandId::FocusNext => {
                    compose.move_field(true);
                    return;
                }
                CommandId::FocusPrev => {
                    compose.move_field(false);
                    return;
                }
                _ => {}
            }
        }
        // In the form, moving is between its fields and opening is going on
        // to the next step.
        if context == Context::Setup
            && let Some(setup) = &mut self.setup
        {
            match id {
                CommandId::NavOpen => {
                    out.accounts.extend(setup.advance());
                    return;
                }
                CommandId::FocusNext | CommandId::NavNext => {
                    setup.move_field(true);
                    return;
                }
                CommandId::FocusPrev | CommandId::NavPrev => {
                    setup.move_field(false);
                    return;
                }
                _ => {}
            }
        }
        match id {
            CommandId::NavNext => self.navigate(env, Motion::Next, out),
            CommandId::NavPrev => self.navigate(env, Motion::Prev, out),
            CommandId::NavTop => self.navigate(env, Motion::Top, out),
            CommandId::NavBottom => self.navigate(env, Motion::Bottom, out),
            CommandId::NavPageDown => self.navigate(env, Motion::PageDown, out),
            CommandId::NavPageUp => self.navigate(env, Motion::PageUp, out),
            CommandId::NavOpen if context == Context::Palette => {
                let chosen = self
                    .palette
                    .as_ref()
                    .and_then(|palette| self.palette_items(env).into_iter().nth(palette.selected));
                self.palette = None;
                match chosen.map(|item| item.what) {
                    // Run where the palette was opened, as its key would.
                    Some(What::Command(id)) => self.run(env, id, out),
                    Some(What::Search(query)) => {
                        self.open_search(env, &query, out);
                        self.set_focus(Pane::List);
                    }
                    Some(What::Mailbox(mailbox)) => {
                        self.show_mailbox(mailbox, out);
                        // A folder of a closed account is opened to.
                        self.select_mailbox_row(mailbox, out);
                        self.set_focus(Pane::List);
                    }
                    None => {}
                }
            }
            CommandId::NavOpen if context == Context::Links => self.open_selected_link(out),
            CommandId::NavOpen if context == Context::Search => {
                // To the results, if there are any to go to.
                if let Some(search) = &mut self.search
                    && !self.list.rows.is_empty()
                {
                    search.typing = false;
                }
            }
            CommandId::NavOpen if context == Context::Attachments => {
                self.use_attachment(FileUse::View, out);
            }
            CommandId::NavOpen if context == Context::When => self.answer_when(env, out),
            CommandId::NavOpen if context == Context::Unsubscribe => {
                self.answer_unsubscribe(env, out);
            }
            CommandId::NavOpen if context == Context::Templates => self.insert_template(env),
            CommandId::NavOpen if context == Context::Hooks => self.run_hook(env, out),
            CommandId::NavOpen if context == Context::Suggestions => self.use_suggestion(),
            CommandId::NavOpen if context == Context::Folders => {
                if let Some(picker) = self.folders.take()
                    && let Some(choice) = self.folder_choices(&picker).get(picker.selected)
                {
                    if picker.copy {
                        let mut ids = vec![picker.message];
                        ids.extend(picker.also);
                        out.data.push(DataRequest::CopyTo {
                            ids,
                            folder: choice.id,
                        });
                    } else if picker.also.is_empty() {
                        out.data.push(DataRequest::MoveTo {
                            id: picker.message,
                            folder: choice.id,
                        });
                    } else {
                        let mut ids = vec![picker.message];
                        ids.extend(picker.also);
                        out.data.push(DataRequest::Many {
                            ids,
                            what: Bulk::MoveTo(choice.id),
                        });
                    }
                    self.list.clear_marks();
                }
            }
            CommandId::MessageArchive => match self.targets().as_slice() {
                [] => {}
                [id] => out.data.push(DataRequest::Archive(*id)),
                ids => out.data.push(DataRequest::Many {
                    ids: ids.to_vec(),
                    what: Bulk::Archive,
                }),
            },
            CommandId::MessageDelete => match self.targets().as_slice() {
                [] => {}
                [id] => out.data.push(DataRequest::Delete(*id)),
                ids => out.data.push(DataRequest::Many {
                    ids: ids.to_vec(),
                    what: Bulk::Delete,
                }),
            },
            CommandId::MessageMove | CommandId::MessageCopy => {
                // Drafts are in no folder of any server and stay put.
                let mut ids = self
                    .targets()
                    .into_iter()
                    .filter(|id| DraftId::from_message_id(*id).is_none());
                if let Some(first) = ids.next() {
                    let mut picker = FolderPicker::for_message(first);
                    picker.also = ids.collect();
                    picker.copy = id == CommandId::MessageCopy;
                    self.folders = Some(picker);
                }
            }
            CommandId::MessagePutBack => self.put_back(out),
            CommandId::FolderCreate => self.ask_new_folder(env),
            CommandId::FolderRemove => {
                if let Some(folder) = self.chosen_local_folder() {
                    out.data.push(DataRequest::RemoveLocalFolder(folder));
                }
            }
            CommandId::NavOpen if context == Context::NewFolder => {
                self.answer_new_folder(env, out);
            }
            CommandId::NavOpen if context == Context::AccountLetter => self.answer_letter(out),
            CommandId::NavOpen if context == Context::NewRule => self.answer_new_rule(out),
            CommandId::RulesToggle => self.toggle_rules(out),
            CommandId::RulesRun => self.pick_rule(),
            CommandId::RulesRunAll => self.try_rules(env, None, out),
            CommandId::RulesApply => self.apply_rule(out),
            CommandId::RulesNew => self.ask_new_rule(),
            CommandId::RulesCreate => self.ask_blank_rule(),
            CommandId::RulesEdit => out.accounts.push(AccountRequest::Rules(RulesAsk::Open)),
            CommandId::RulesLog => out.data.push(DataRequest::RulesLog),
            CommandId::SelectToggle => {
                if self.list.range.is_some() {
                    // The range ends here, with everything in it selected.
                    self.list.settle_range();
                } else if let Some(row) = self.list.rows.get(self.list.selected) {
                    let id = row.id;
                    if !self.list.marked.remove(&id) {
                        self.list.marked.insert(id);
                    }
                    // On to the next, so a run can be picked out with one key.
                    if self.list.selected + 1 < self.list.rows.len() {
                        self.navigate(env, Motion::Next, out);
                    }
                }
            }
            CommandId::SelectRange => {
                if self.list.range.is_some() {
                    self.list.settle_range();
                } else {
                    self.list.range = self.list.rows.get(self.list.selected).map(|row| row.id);
                }
            }
            CommandId::SelectAll => {
                self.list.range = None;
                self.list.marked = self.list.rows.iter().map(|row| row.id).collect();
            }
            CommandId::EditUndo => out.data.push(DataRequest::Undo),
            CommandId::MessageCompose => self.start_composing(env),
            CommandId::MessageReply | CommandId::MessageReplyAll | CommandId::MessageForward => {
                if let Some(row) = self.target_row() {
                    out.data.push(DataRequest::Respond {
                        id: row.id,
                        how: match id {
                            CommandId::MessageForward => Response::Forward,
                            _ => Response::Reply {
                                all: id == CommandId::MessageReplyAll,
                            },
                        },
                        date_line: text::long_date(row.headers.date, &env.tz),
                        position: if env.compose.reply_above {
                            ReplyPosition::Above
                        } else {
                            ReplyPosition::Below
                        },
                    });
                }
            }
            // In the Outbox, with no message being written: the one the
            // list is on goes now, as it is.
            CommandId::ComposeSend if self.compose.is_none() => {
                if let Some(draft) = self.outbox_selection() {
                    out.data.push(DataRequest::SendNow(draft));
                }
            }
            // While Vim has the message, what is here is what it was:
            // Vim is asked to send what it has, and its going does the rest.
            CommandId::ComposeSend => match self.vim_dir() {
                Some(dir) => {
                    out.data.push(DataRequest::EditTell {
                        dir,
                        verdict: Verdict::Send,
                    });
                    self.flash = Some(("Asked Vim to send it.".to_owned(), false));
                }
                None => self.send(env, None, out),
            },
            // From an editor running in the text's place, the other one
            // is Quill's own: the editor is asked to save and go, and what
            // it wrote comes back here.
            CommandId::ComposeVim
                if self.compose.as_ref().is_some_and(Compose::editor_in_place) =>
            {
                out.host.push(HostRequest::ClosePane);
                self.flash = Some((
                    format!(
                        "Asked {} to save and hand the message back.",
                        env.external.name()
                    ),
                    false,
                ));
            }
            CommandId::ComposeVim => {
                // Asked for by hand, the editor is where the keys are
                // wanted: with the fields still here, the cursor goes to
                // the text it is about to have.
                if env.editor_pane
                    && let Some(compose) = &mut self.compose
                {
                    compose.go_to_body();
                }
                self.start_vim(env, out);
            }
            CommandId::ComposeFields | CommandId::ComposeBody => {
                let up = id == CommandId::ComposeFields;
                let accounts: Vec<LocalAccountId> =
                    self.accounts.iter().map(|account| account.id).collect();
                if let Some(compose) = &mut self.compose {
                    if compose.away.is_some() {
                        // An editor has the text: between it and the
                        // fields that are still here.
                        if up {
                            compose.go_to_fields();
                        } else {
                            compose.go_to_body();
                        }
                    } else {
                        // In Quill's own editor these keys are the ends of
                        // the text, as they are in any text on a Mac. Run
                        // from a menu, the command is that key.
                        let key = if up { Key::Up } else { Key::Down };
                        let mods = Mods {
                            cmd: true,
                            ..Mods::default()
                        };
                        compose.key(KeyChord { key, mods }, &accounts);
                    }
                }
            }
            CommandId::ComposeTips => {
                let shown = !self.editor_tips;
                self.tips_asked = Some(shown);
                self.editor_tips = shown;
                // Asked for, it is there for this message; and put away,
                // it is not shown unasked again.
                self.tips_seen = shown;
                if !shown {
                    out.accounts
                        .push(AccountRequest::Remember(Preference::EditorTips(false)));
                }
            }
            CommandId::MessageHook => {
                if let Some(message) = self.hook_message() {
                    self.hooks = Some(HookPicker {
                        message,
                        filter: LineInput::default(),
                        selected: 0,
                        offset: 0,
                        total: env.hooks.len(),
                        list: None,
                    });
                }
            }
            CommandId::ComposeSendLater => {
                self.when = Some(WhenPrompt {
                    input: LineInput::default(),
                    what: WhenFor::SendLater,
                    problem: None,
                });
            }
            CommandId::MessageSnooze => {
                let ids: Vec<LocalMessageId> = self
                    .targets()
                    .into_iter()
                    .filter(|id| DraftId::from_message_id(*id).is_none())
                    .collect();
                if !ids.is_empty() {
                    self.when = Some(WhenPrompt {
                        input: LineInput::default(),
                        what: WhenFor::Snooze(ids),
                        problem: None,
                    });
                }
            }
            CommandId::ComposeSave => {
                if let Some(dir) = self.vim_dir() {
                    out.data.push(DataRequest::EditTell {
                        dir,
                        verdict: Verdict::Postpone,
                    });
                    self.flash = Some(("Asked Vim to save it as a draft.".to_owned(), false));
                } else if let Some(compose) = &self.compose {
                    out.data.push(DataRequest::SaveDraft {
                        draft: compose.draft,
                        account: compose.account,
                        content: Box::new(compose.content()),
                        announce: true,
                    });
                }
            }
            CommandId::ComposeBcc => {
                if let Some(compose) = &mut self.compose {
                    compose.show_bcc();
                }
            }
            CommandId::ComposeTemplate => {
                self.templates = Some(TemplatePicker {
                    filter: LineInput::default(),
                    selected: 0,
                    offset: 0,
                    total: env.templates.len(),
                });
            }
            CommandId::MessageUnsubscribe => {
                self.unsubscribe = self.unsubscribe_offer(env, context);
            }
            CommandId::InviteAccept | CommandId::InviteDecline | CommandId::InviteTentative => {
                let answer = match id {
                    CommandId::InviteAccept => quill_mime::invite::Answer::Accepted,
                    CommandId::InviteDecline => quill_mime::invite::Answer::Declined,
                    _ => quill_mime::invite::Answer::Tentative,
                };
                self.answer_invite(env, answer, context, out);
            }
            CommandId::ComposeSpellSuggest => self.ask_for_suggestions(out),
            CommandId::ComposeSpellNext => {
                if let Some(compose) = &mut self.compose
                    && compose.body.next_misspelling()
                {
                    compose.field = Field::Body;
                    compose.completion = None;
                }
            }
            // From the app's Edit menu, which takes these keys before the
            // editor can. Typed in a terminal they reach the editor itself.
            // With no message being written they are the reader's: what
            // was selected in it with the mouse.
            CommandId::EditCopy if self.compose.is_none() => {
                self.copy_selection(env, out);
            }
            CommandId::EditSelectAll if self.compose.is_none() => self.select_all_of_message(env),
            CommandId::MessageCopyText => self.copy_message_text(env, out),
            CommandId::MessageSave => self.ask_save_as(env),
            CommandId::EditCopy => {
                if let Some(text) = self
                    .compose
                    .as_ref()
                    .and_then(|compose| compose.body.copy())
                {
                    out.host.push(HostRequest::Copy(text));
                }
            }
            CommandId::EditCut => {
                if let Some(compose) = &mut self.compose
                    && let Some(text) = compose.body.cut()
                {
                    compose.doubts.clear();
                    out.host.push(HostRequest::Copy(text));
                }
            }
            CommandId::EditSelectAll => {
                if let Some(compose) = &mut self.compose {
                    compose.body.select_all();
                }
            }
            CommandId::ComposeAttach => {
                if env.file_panel {
                    // The host's own panel: the files come back as
                    // `Action::Files`.
                    out.host.push(HostRequest::PickFiles);
                } else if let Some(compose) = &mut self.compose {
                    compose.ask_for_attachment();
                }
            }
            CommandId::AccountAdd => self.setup = Some(Setup::default()),
            // Now, instead of at its next turn: the account chosen in the
            // accounts pane, or every account when what is chosen there is
            // nobody's alone. What comes of it is told the usual way: new
            // mail appears, and an account that cannot be reached says so.
            CommandId::MailboxSync => match self.chosen_account() {
                Some(account) => {
                    out.sync.push(SyncRequest::SyncOne(account.id));
                    self.flash = Some((
                        format!(
                            "Checking {} for new mail.",
                            text::sanitize_line(&account.name)
                        ),
                        false,
                    ));
                }
                None => {
                    out.sync.push(SyncRequest::SyncNow);
                    self.flash = Some(("Checking for new mail.".to_owned(), false));
                }
            },
            CommandId::SidebarOpen => {
                if let Some(account) = self.sidebar.account_of(self.sidebar.selected) {
                    self.set_account_open(account, true, out);
                }
            }
            CommandId::SidebarClose => {
                if let Some(account) = self.sidebar.account_of(self.sidebar.selected) {
                    self.close_account(env, account, out);
                }
            }
            CommandId::AccountRemove => {
                if let Some(name) = self.chosen_account().map(|account| account.name.clone()) {
                    self.ask_remove(env, &name, out);
                }
            }
            CommandId::AccountEdit => {
                if let Some(name) = self.chosen_account().map(|account| account.name.clone()) {
                    self.ask_edit(env, &name, out);
                }
            }
            CommandId::AccountLetter => {
                if let Some((name, own)) = self
                    .chosen_account()
                    .map(|account| (account.name.clone(), account.letter.clone()))
                {
                    self.ask_letter(env, &name, own);
                }
            }
            CommandId::AccountSignIn => {
                out.accounts.push(AccountRequest::SignIn {
                    account: self.current_account().map(|account| account.name.clone()),
                });
                self.sign_in = Some(SignIn {
                    account: String::new(),
                    stage: SignInStage::Starting,
                    fresh: false,
                });
            }
            CommandId::NavOpen => self.open(out),
            CommandId::NavBack => self.back(env, out),
            // Where Quill has a window that can be closed with Quill left
            // running, closing is that, and the host's to do. Anywhere
            // else it is quitting.
            CommandId::WindowClose if env.window => out.host.push(HostRequest::CloseWindow),
            CommandId::WindowClose | CommandId::AppQuit => self.should_quit = true,
            CommandId::AgentsStop => self.stop_agents(env, out),
            // The service does it, and says what it did.
            CommandId::CommandLineInstall => out.accounts.push(AccountRequest::LinkCommand),
            CommandId::FocusSidebar => self.set_focus(Pane::Sidebar),
            CommandId::FocusList => self.set_focus(Pane::List),
            CommandId::FocusReader => self.set_focus(Pane::Reader),
            // With the message in the host's pane there is no reader
            // here to go to: the list has the keys for both, and only the
            // accounts are anywhere else.
            CommandId::FocusNext | CommandId::FocusPrev if self.web_reading => {
                self.set_focus(match (id, self.focus) {
                    (CommandId::FocusPrev, Pane::List | Pane::Reader) => Pane::Sidebar,
                    _ => Pane::List,
                });
            }
            CommandId::FocusNext => self.set_focus(self.focus.next()),
            CommandId::FocusPrev => self.set_focus(self.focus.prev()),
            CommandId::ViewSidebar => self.toggle_sidebar(env, out),
            CommandId::ViewZoom => self.zoomed = !self.zoomed,
            CommandId::ViewWebReading => self.toggle_web_reading(env, out),
            CommandId::ViewDarkPages => self.toggle_dark_pages(env, out),
            CommandId::ViewThreads => {
                self.ungrouped = !self.ungrouped;
                self.flash = Some((
                    if self.ungrouped == env.threads {
                        "Messages are listed one by one, newest first."
                    } else {
                        "Conversations are listed together."
                    }
                    .to_owned(),
                    false,
                ));
                self.reload(env, out);
            }
            CommandId::MessageToggleUnread => self.toggle_unread(out),
            CommandId::MessageToggleStar => self.toggle_star(out),
            CommandId::MessageToggleHtml => {
                if let Some(showing) = self.reader.showing() {
                    self.reader.chosen = Some(match showing {
                        Showing::Html => Showing::Plain,
                        Showing::Plain => Showing::Html,
                    });
                    // Another version is another text: start at its top.
                    self.reader.scroll = 0;
                    self.reader.wrap_width = None;
                }
            }
            CommandId::MessageOpenLink => self.links = Some(LinkPicker::default()),
            CommandId::PaletteOpen => {
                // What applies here, decided before the palette takes over.
                let commands = env
                    .registry
                    .specs()
                    .iter()
                    .map(|spec| spec.id)
                    .filter(|id| *id != CommandId::PaletteOpen && self.is_enabled(env, *id))
                    .collect();
                self.palette = Some(Palette {
                    input: LineInput::default(),
                    selected: 0,
                    offset: 0,
                    commands,
                });
            }
            CommandId::SearchStart => {
                // Again, to change what is being searched for.
                match &mut self.search {
                    // A rule on show is not a search to change: it is
                    // left for one.
                    Some(search) if search.rule.is_some() => {
                        *search = Search {
                            typing: true,
                            ..Search::default()
                        };
                        self.list = MessageList::default();
                        self.list.loaded = true;
                    }
                    Some(search) => search.typing = true,
                    None => {
                        self.search = Some(Search {
                            typing: true,
                            ..Search::default()
                        });
                        self.list = MessageList::default();
                        self.list.loaded = true;
                    }
                }
                // The results are in the list: bring it into view.
                self.set_focus(Pane::List);
            }
            CommandId::MessageAttachments => {
                self.attachments = Some(AttachmentPicker::default());
            }
            CommandId::MessageOpenInBrowser => self.show_page(env, PageUse::Browser, out),
            CommandId::MessageLoadRemote => {
                self.reader.remote = true;
                self.show_page(env, self.page_again(), out);
            }
            CommandId::SenderAlwaysRemote => {
                let sender = self
                    .reader
                    .row
                    .as_ref()
                    .and_then(|row| row.headers.from.first())
                    .map(|from| from.email.clone());
                if let Some(address) = sender {
                    self.flash = Some((
                        format!(
                            "Mail from {} loads remote pictures from now on.",
                            text::sanitize_line(&address)
                        ),
                        false,
                    ));
                    out.data.push(DataRequest::AllowRemote { address });
                    self.reader.remote = true;
                    self.show_page(env, self.page_again(), out);
                }
            }
            CommandId::AttachmentOpen => self.use_attachment(FileUse::Open, out),
            CommandId::AttachmentSave => {
                if env.file_panel {
                    // The host asks where; the file is made ready for it.
                    self.use_attachment(FileUse::Save, out);
                } else if let Some((message, file)) = self.chosen_attachment() {
                    out.data.push(DataRequest::SaveAttachment {
                        message,
                        part: file.part,
                    });
                }
            }
            CommandId::HelpToggle | CommandId::HelpKeys => {
                // Either key puts away the list it brought up, and the
                // other's list takes its place.
                let all = id == CommandId::HelpKeys;
                self.help = match self.help {
                    Some(help) if help.all == all => None,
                    Some(help) => Some(Help {
                        context: help.context,
                        scroll: 0,
                        all,
                    }),
                    None => Some(Help {
                        context,
                        scroll: 0,
                        all,
                    }),
                }
            }
        }
    }

    /// Whether running the command now would do anything. Decides what a key
    /// does and which menu items and toolbar buttons the app shell enables.
    pub fn is_enabled(&self, env: &Env, id: CommandId) -> bool {
        let context = self.context();
        if !env
            .registry
            .get(id)
            .is_some_and(|spec| spec.applies_in(context))
        {
            return false;
        }
        // The form or the question is in a sheet of the host's, and is
        // worked there. The commands that work it in a terminal would act
        // on what the core holds, which is not what the sheet's fields
        // hold until a button sends it: going on from here would be going
        // on blind, and saying yes to a question that is not on this
        // screen.
        if self.answered_in_sheet(env, id) {
            return false;
        }
        match id {
            CommandId::NavOpen if context == Context::Palette => true,
            CommandId::NavOpen if context == Context::Links => !self.reader.links.is_empty(),
            CommandId::NavOpen if context == Context::Attachments => {
                self.chosen_attachment().is_some()
            }
            CommandId::NavOpen if context == Context::Setup => true,
            CommandId::NavOpen if context == Context::Search => !self.list.rows.is_empty(),
            CommandId::NavOpen if context == Context::Folders => self
                .folders
                .as_ref()
                .is_some_and(|picker| !self.folder_choices(picker).is_empty()),
            CommandId::AgentsStop => env.agent.commands || env.agent.mcp,
            CommandId::MessageDelete => self.target_row().is_some(),
            CommandId::SelectToggle | CommandId::SelectRange | CommandId::SelectAll => {
                !self.list.rows.is_empty()
            }
            CommandId::MessageCompose => !self.accounts.is_empty(),
            CommandId::MessageReply | CommandId::MessageReplyAll | CommandId::MessageForward => {
                self.target_row()
                    .is_some_and(|row| DraftId::from_message_id(row.id).is_none())
            }
            // A message in Vim is sent and saved by asking Vim; while it
            // is on its way there or back there is nobody to ask. An
            // editor without Quill's plugin cannot be asked at all: the
            // message is sent once it is back.
            // In the Outbox, Send is for the message the list is on.
            CommandId::ComposeSend if self.compose.is_none() => self.outbox_selection().is_some(),
            CommandId::ComposeSend | CommandId::ComposeSave => {
                self.compose.as_ref().is_some_and(|compose| {
                    !compose.sending
                        && match compose.away {
                            None => true,
                            Some(Away::InVim) => env.external.is_vim(),
                            Some(_) => false,
                        }
                })
            }
            // Everything else about writing waits for it to come back.
            CommandId::ComposeSendLater => self
                .compose
                .as_ref()
                .is_some_and(|compose| !compose.sending && compose.away.is_none()),
            // To the other editor: to the one outside from Quill's own,
            // and back from one that is running in the text's place.
            CommandId::ComposeVim => self.compose.as_ref().is_some_and(|compose| {
                !compose.sending && (compose.away.is_none() || compose.editor_in_place())
            }),
            // The fields are still here while an editor has the text
            // alone, and so is what is done to them.
            CommandId::ComposeAttach | CommandId::ComposeBcc => {
                self.compose.as_ref().is_none_or(Compose::fields_in_reach)
            }
            CommandId::ComposeFields | CommandId::ComposeBody => self
                .compose
                .as_ref()
                .is_some_and(|compose| !compose.sending && compose.fields_in_reach()),
            CommandId::ComposeTips => self.compose.as_ref().is_some_and(Compose::editor_in_place),
            CommandId::MessageHook => {
                env.hooks_enabled && !env.hooks.is_empty() && self.hook_message().is_some()
            }
            CommandId::NavOpen if context == Context::Hooks => self
                .hooks
                .as_ref()
                .is_some_and(|picker| !self.hook_choices(env, picker).is_empty()),
            CommandId::MessageSnooze => self
                .targets()
                .iter()
                .any(|id| DraftId::from_message_id(*id).is_none()),
            CommandId::NavOpen if context == Context::When => true,
            CommandId::NavOpen if context == Context::NewFolder => true,
            CommandId::NavOpen if context == Context::AccountLetter => true,
            CommandId::NavOpen if context == Context::NewRule => true,
            CommandId::RulesRun => self.can_run_a_rule(),
            CommandId::RulesRunAll => self.can_run_all_rules(),
            CommandId::RulesApply => self.can_apply_rule(),
            CommandId::RulesNew => self.can_start_rule(),
            CommandId::RulesCreate => self.can_create_rule(),
            CommandId::MessageCopy => self
                .target_row()
                .is_some_and(|row| DraftId::from_message_id(row.id).is_none()),
            CommandId::MessagePutBack => self.can_put_back(),
            CommandId::FolderCreate => self.can_make_folder(env),
            CommandId::FolderRemove => self.chosen_local_folder().is_some(),
            CommandId::NavOpen if context == Context::Unsubscribe => true,
            CommandId::NavOpen if context == Context::Templates => self
                .templates
                .as_ref()
                .is_some_and(|picker| !self.template_choices(env, picker).is_empty()),
            CommandId::NavOpen if context == Context::Suggestions => true,
            CommandId::InviteAccept | CommandId::InviteDecline | CommandId::InviteTentative => {
                self.invite_asked(context).is_some()
            }
            CommandId::ComposeSpellSuggest => {
                self.spelling_on(env)
                    && self.compose.as_ref().is_some_and(|compose| {
                        compose.field == Field::Body
                            && compose.attaching.is_none()
                            && !compose.sending
                            && compose.away.is_none()
                            && compose.body.misspelling_at_cursor().is_some()
                    })
            }
            CommandId::ComposeSpellNext => {
                self.spelling_on(env)
                    && self.compose.as_ref().is_some_and(|compose| {
                        compose.attaching.is_none()
                            && !compose.sending
                            && compose.away.is_none()
                            && compose.body.has_misspellings()
                    })
            }
            // The question is all there is until it is answered: nothing
            // behind it moves.
            _ if context == Context::Unsubscribe => {
                matches!(id, CommandId::NavBack | CommandId::HelpToggle)
            }
            _ if context == Context::RemoveAccount => {
                matches!(id, CommandId::NavBack | CommandId::HelpToggle)
            }
            CommandId::ComposeTemplate => {
                !env.templates.is_empty()
                    && self.compose.as_ref().is_some_and(|compose| {
                        !compose.sending && compose.attaching.is_none() && compose.away.is_none()
                    })
            }
            CommandId::MessageUnsubscribe => self.unsubscribe_offer(env, context).is_some(),
            // With no message being written, the reader's selection.
            CommandId::EditCopy if self.compose.is_none() => self.selected_text(env).is_some(),
            CommandId::EditSelectAll if self.compose.is_none() => {
                context == Context::Reader && self.reader_text_on_show(env)
            }
            CommandId::MessageCopyText | CommandId::MessageSave => {
                self.message_to_save(context).is_some()
            }
            // Only where the body's own selection is the one meant.
            CommandId::EditCopy | CommandId::EditCut => {
                self.compose.as_ref().is_some_and(|compose| {
                    compose.field == Field::Body
                        && compose.attaching.is_none()
                        && !compose.sending
                        && compose.away.is_none()
                        && compose.body.selection().is_some()
                })
            }
            CommandId::EditSelectAll => self.compose.as_ref().is_some_and(|compose| {
                compose.field == Field::Body
                    && compose.attaching.is_none()
                    && !compose.sending
                    && compose.away.is_none()
            }),
            // A draft is in no folder of any server: it can be thrown away,
            // not filed or flagged.
            CommandId::MessageArchive | CommandId::MessageMove => self
                .targets()
                .iter()
                .any(|id| DraftId::from_message_id(*id).is_none()),
            CommandId::AccountAdd => env.accounts,
            CommandId::AccountSignIn => env.accounts && !self.accounts.is_empty(),
            // The account the accounts pane is on, and no other.
            CommandId::AccountRemove => {
                self.can_remove_account(env) && self.chosen_account().is_some()
            }
            CommandId::AccountEdit => self.can_edit_account(env) && self.chosen_account().is_some(),
            CommandId::AccountLetter => self.can_set_letter(env) && self.chosen_account().is_some(),
            // Only where there is a server to ask: not the demo mailbox.
            CommandId::MailboxSync => env.accounts && !self.accounts.is_empty(),
            // Only on an account that is closed, and on an open one or
            // one of its folders.
            CommandId::SidebarOpen => self
                .sidebar
                .account_of(self.sidebar.selected)
                .is_some_and(|account| !self.sidebar.is_open(account)),
            CommandId::SidebarClose => self
                .sidebar
                .account_of(self.sidebar.selected)
                .is_some_and(|account| self.sidebar.is_open(account)),
            CommandId::NavOpen => {
                self.focus == Pane::Sidebar || self.list.rows.get(self.list.selected).is_some()
            }
            // The versions are the text reader's to switch between.
            CommandId::MessageToggleHtml => !self.web_reading && self.reader.has_two_versions(),
            CommandId::ViewWebReading | CommandId::ViewDarkPages => env.web_pane,
            // There is no reader here to go to: the message is in the
            // host's pane, and the keys that work on it work from the list.
            CommandId::FocusReader => !self.web_reading,
            // Nor from the list with `tab`, which goes nowhere else.
            CommandId::FocusNext => !(self.web_reading && context == Context::List),
            CommandId::MessageOpenLink => !self.reader.links.is_empty(),
            // The files listed are those of the message in the reader, so
            // from the list it must be the one under the cursor.
            CommandId::MessageAttachments => {
                !self.reader.files().is_empty()
                    && (context == Context::Reader
                        || self.target_row().map(|row| row.id)
                            == self.reader.row.as_ref().map(|row| row.id))
            }
            CommandId::AttachmentOpen | CommandId::AttachmentSave => {
                self.chosen_attachment().is_some()
            }
            CommandId::MessageOpenInBrowser => self.page_message(context).is_some(),
            CommandId::MessageLoadRemote | CommandId::SenderAlwaysRemote => {
                !env.remote_never && self.page_message(context).is_some()
            }
            CommandId::NavBack => {
                self.help.is_some()
                    || self.palette.is_some()
                    || self.search.is_some()
                    || self.links.is_some()
                    || self.attachments.is_some()
                    || self.folders.is_some()
                    || self.templates.is_some()
                    || self.suggestions.is_some()
                    || self.hooks.is_some()
                    || self.unsubscribe.is_some()
                    || self.remove.is_some()
                    || self.compose.is_some()
                    || self.setup.is_some()
                    || self.sign_in.is_some()
                    || self.when.is_some()
                    || self.new_folder.is_some()
                    || self.letter_prompt.is_some()
                    || self.new_rule.is_some()
                    || self.keys.is_some()
                    || self.theme_prompt.is_some()
                    || !self.certificates.is_empty()
                    || !self.notices.is_empty()
                    || self.focus == Pane::Reader
                    || self.list.has_marks()
                    || (self.focus == Pane::Sidebar
                        && layout::sidebar_floats(self, self.size.0, env.reader_width))
            }
            CommandId::MessageToggleUnread | CommandId::MessageToggleStar => self
                .target_row()
                .is_some_and(|row| DraftId::from_message_id(row.id).is_none()),
            _ => true,
        }
    }

    /// The state the app shell mirrors.
    pub fn host_state(&self, env: &Env) -> HostState {
        let unread = self
            .sidebar
            .entries
            .iter()
            .find_map(|entry| match entry {
                SidebarEntry::Unified { unread } => Some(*unread),
                _ => None,
            })
            .unwrap_or(0);
        let mailbox = self.mailbox_title();
        HostState {
            context: self.context(),
            enabled: env
                .registry
                .specs()
                .iter()
                .map(|spec| spec.id)
                .filter(|id| self.is_enabled(env, *id))
                .collect(),
            checked: env
                .registry
                .specs()
                .iter()
                .map(|spec| spec.id)
                .filter(|id| self.is_on(*id))
                .collect(),
            unread,
            title: if mailbox.is_empty() {
                "Quill".to_owned()
            } else {
                mailbox
            },
            background: env.styles.background_rgb(),
            foreground: env.styles.text_rgb(),
            appearance: env.look.appearance,
            font: env.font.clone(),
            font_size: env.font_size,
        }
    }

    /// Whether a command that switches something on and off has it on.
    /// The app shell checks its menu item.
    pub fn is_on(&self, id: CommandId) -> bool {
        match id {
            CommandId::ViewWebReading => self.web_reading,
            CommandId::ViewDarkPages => self.dark_pages,
            CommandId::ViewSidebar => !self.sidebar_hidden,
            CommandId::RulesToggle => self.rules_on(),
            _ => false,
        }
    }

    fn set_focus(&mut self, pane: Pane) {
        // With the message in the host's pane there is no reader here to
        // have the keys: only a message being written takes its place.
        if pane == Pane::Reader && self.web_reading && self.compose.is_none() {
            return;
        }
        self.focus = pane;
        match pane {
            Pane::Sidebar => {}
            Pane::List => self.main_pane = MainPane::List,
            Pane::Reader => self.main_pane = MainPane::Reader,
        }
    }

    fn navigate(&mut self, env: &Env, motion: Motion, out: &mut Effects) {
        let areas = layout::compute(self, self.size, env.reader_width);
        if let Some(help) = &mut self.help {
            let page = areas.help.map_or(1, layout::inner_height).max(1);
            help.scroll = scroll(help.scroll, motion, page);
            return;
        }
        // A question or a form is all there is until it is dealt with:
        // whatever lies behind it stays where it is.
        if matches!(
            self.context(),
            Context::Certificate
                | Context::SignIn
                | Context::When
                | Context::NewFolder
                | Context::AccountLetter
                | Context::NewRule
                | Context::RecordKeys
                | Context::EditTheme
                | Context::Unsubscribe
                | Context::RemoveAccount
                | Context::Setup
        ) {
            return;
        }
        if let Some(picker) = &mut self.links {
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            picker.selected = step(picker.selected, self.reader.links.len(), motion, page);
            picker.typed.clear();
            return;
        }
        if let Some(palette) = &self.palette {
            let count = self.palette_items(env).len();
            let page = areas
                .links
                .map_or(1, layout::inner_height)
                .saturating_sub(1)
                .max(1);
            let selected = step(palette.selected, count, motion, page);
            if let Some(palette) = &mut self.palette {
                palette.selected = selected;
            }
            return;
        }
        if let Some(picker) = &mut self.attachments {
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            picker.selected = step(picker.selected, self.reader.files().len(), motion, page);
            return;
        }
        if let Some(picker) = &self.folders {
            let count = self.folder_choices(picker).len();
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            let selected = step(picker.selected, count, motion, page);
            if let Some(picker) = &mut self.folders {
                picker.selected = selected;
            }
            return;
        }
        if let Some(picker) = &self.templates {
            let count = self.template_choices(env, picker).len();
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            let selected = step(picker.selected, count, motion, page);
            if let Some(picker) = &mut self.templates {
                picker.selected = selected;
            }
            return;
        }
        if let Some(picker) = &mut self.suggestions {
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            picker.selected = step(picker.selected, picker.words.len(), motion, page);
            return;
        }
        if let Some(picker) = &self.hooks {
            let count = self.hook_choices(env, picker).len();
            let page = areas.links.map_or(1, layout::inner_height).max(1);
            let selected = step(picker.selected, count, motion, page);
            if let Some(picker) = &mut self.hooks {
                picker.selected = selected;
            }
            return;
        }
        match self.focus {
            Pane::Sidebar => {
                let selectable: Vec<usize> = self
                    .sidebar
                    .shown()
                    .into_iter()
                    .filter(|index| {
                        self.sidebar
                            .entries
                            .get(*index)
                            .is_some_and(SidebarEntry::is_place)
                    })
                    .collect();
                let position = selectable
                    .iter()
                    .position(|index| *index == self.sidebar.selected)
                    .unwrap_or(0);
                let page = areas.sidebar.map_or(1, layout::inner_height).max(1);
                let Some(&target) = selectable.get(step(position, selectable.len(), motion, page))
                else {
                    return;
                };
                if target != self.sidebar.selected {
                    self.go_to_entry(env, target, out);
                }
            }
            // With the message in the host's pane, a page down or up is
            // a page of the message, as it is in the reader: the list is
            // moved through a message at a time, or to its ends.
            Pane::List
                if self.web_reading && matches!(motion, Motion::PageDown | Motion::PageUp) =>
            {
                out.host.push(HostRequest::ScrollReading {
                    down: motion == Motion::PageDown,
                });
            }
            Pane::List => {
                let page = areas
                    .list
                    .map_or(1, layout::inner_height)
                    .saturating_sub(1)
                    .max(1);
                let target = step(self.list.selected, self.list.rows.len(), motion, page);
                if target != self.list.selected {
                    self.list.selected = target;
                    self.follow.went = true;
                    self.sync_reader(out);
                }
            }
            // The message being written is in the reader's place, and
            // moves by its own keys: there is nothing here to scroll.
            Pane::Reader if self.compose.is_some() => {}
            Pane::Reader => {
                let header = self.reader_header_rows();
                let page = areas
                    .reader
                    .map_or(1, |rect| {
                        layout::reader_body_height(rect, header, self.reader_image_rows(env, rect))
                    })
                    .saturating_sub(1)
                    .max(1);
                // Clamped to the end of the body by `relayout`.
                self.reader.scroll = scroll(self.reader.scroll, motion, page);
            }
        }
    }

    fn show_mailbox(&mut self, mailbox: Mailbox, out: &mut Effects) {
        // Choosing a mailbox is leaving the search, even for the mailbox
        // the search was started from.
        let searching = self.search.take().is_some();
        if self.mailbox == mailbox && !searching {
            return;
        }
        self.mailbox = mailbox;
        self.list = MessageList::default();
        out.data.push(DataRequest::LoadMessages(mailbox));
    }

    fn open(&mut self, out: &mut Effects) {
        match self.focus {
            Pane::Sidebar => {
                // Choosing a mailbox is leaving the search, even for the
                // mailbox the search was started from. A saved search is
                // itself what was chosen: its mail is what to go to.
                let on_saved_search = matches!(
                    self.sidebar.entries.get(self.sidebar.selected),
                    Some(SidebarEntry::Search { .. })
                );
                if !on_saved_search && self.search.take().is_some() {
                    self.list = MessageList::default();
                    out.data.push(DataRequest::LoadMessages(self.mailbox));
                }
                self.set_focus(Pane::List);
            }
            Pane::List => {
                let Some(row) = self.list.rows.get(self.list.selected) else {
                    return;
                };
                // A draft opens to be written, not read.
                if let Some(draft) = DraftId::from_message_id(row.id) {
                    out.data.push(DataRequest::OpenDraft(draft));
                    return;
                }
                let (id, previous) = (row.id, row.flags);
                if !previous.seen {
                    self.set_seen_locally(id, true);
                    out.data.push(DataRequest::SetSeen { id, seen: true });
                }
                self.sync_reader(out);
                // In the host's pane it is shown now, not in a moment, and
                // the keys stay with the list.
                self.follow.went = true;
                self.follow.now = true;
                self.set_focus(Pane::Reader);
            }
            Pane::Reader => {}
        }
    }

    fn back(&mut self, env: &Env, out: &mut Effects) {
        if self.help.is_some() {
            self.help = None;
        } else if self.palette.is_some() {
            self.palette = None;
        } else if !self.certificates.is_empty() {
            // Not answering is rejecting: nothing is sent to that server.
            self.certificates.pop_front();
        } else if self.when.take().is_some() {
            // Nothing was snoozed or sent.
        } else if self.new_folder.take().is_some() {
            // No folder was made.
        } else if self.letter_prompt.take().is_some() {
            // The account's letter is as it was.
        } else if self.new_rule.take().is_some() {
            // No rule was written.
        } else if self.keys.take().is_some() {
            // No key was changed.
        } else if self.theme_prompt.take().is_some() {
            // No theme was changed.
        } else if self.unsubscribe.take().is_some() {
            // Nothing was asked of the list, written or opened.
        } else if self.remove.is_some() {
            // The account stays, unless it is already being removed.
            self.cancel_remove();
        } else if self.sign_in.is_some() {
            self.give_up_sign_in(out);
        } else if let Some(setup) = &mut self.setup {
            if setup.back() {
                self.setup = None;
            }
        } else if self.links.is_some() {
            self.links = None;
        } else if self.attachments.is_some() {
            self.attachments = None;
        } else if self.folders.is_some() {
            self.folders = None;
        } else if self.templates.is_some() {
            self.templates = None;
        } else if self.suggestions.is_some() {
            self.suggestions = None;
        } else if self.hooks.is_some() {
            self.hooks = None;
        } else if let Some(away) = self.compose.as_ref().and_then(|compose| compose.away) {
            // Vim has the message. Closing here would close it on what it
            // was before: Vim is asked to save and go instead, and the
            // message comes back to be sent, saved or closed.
            if away == Away::InVim {
                out.host.push(HostRequest::ClosePane);
                self.flash = Some(("Asked the editor to save and close.".to_owned(), false));
            }
        } else if self.compose.is_some() {
            self.put_away(false, out);
        } else if self.notices.pop_front().is_some() {
            // One notice dismissed per press.
        } else if self.list.has_marks() && self.focus != Pane::Reader {
            self.list.clear_marks();
        } else if self.search.is_some() && self.focus != Pane::Reader {
            // Out of the search, back to the mailbox it was started from.
            self.search = None;
            self.list = MessageList::default();
            out.data.push(DataRequest::LoadMessages(self.mailbox));
            self.select_mailbox_row(self.mailbox, out);
        } else {
            match self.focus {
                Pane::Reader => self.set_focus(Pane::List),
                Pane::Sidebar if layout::sidebar_floats(self, self.size.0, env.reader_width) => {
                    // Close the overlay and return to whatever was under it.
                    self.focus = match self.main_pane {
                        MainPane::List => Pane::List,
                        MainPane::Reader => Pane::Reader,
                    };
                }
                Pane::Sidebar | Pane::List => {}
            }
        }
    }

    /// Closes the message being written, keeping what was written. A
    /// reply opened and closed untouched leaves nothing behind, and a
    /// draft emptied out is a draft thrown away. `asked`: saving is what
    /// the user asked for, so it is said to be saved even when the draft
    /// already had everything.
    fn put_away(&mut self, asked: bool, out: &mut Effects) {
        let Some(compose) = self.compose.take() else {
            return;
        };
        if compose.sending {
            // It is on its way to the outbox; there is nothing to keep.
        } else if !compose.is_blank() {
            if compose.is_modified() || asked {
                out.data.push(DataRequest::SaveDraft {
                    draft: compose.draft,
                    account: compose.account,
                    content: Box::new(compose.content()),
                    announce: true,
                });
            }
        } else if let Some(draft) = compose.draft {
            out.data.push(DataRequest::Delete(draft.as_message_id()));
        }
        self.set_focus(Pane::List);
    }

    /// What to ask of the store as Quill closes: a message being written
    /// is saved as a draft.
    pub fn parting_requests(&self) -> Vec<DataRequest> {
        match &self.compose {
            Some(compose) if compose.is_modified() && !compose.is_blank() && !compose.sending => {
                vec![DataRequest::SaveDraft {
                    draft: compose.draft,
                    account: compose.account,
                    content: Box::new(compose.content()),
                    announce: false,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// Everything the open palette offers for what has been typed, best
    /// match first: the commands that applied where it was opened, then
    /// the mailboxes to go to.
    pub fn palette_items(&self, env: &Env) -> Vec<Item> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        let mut items: Vec<Item> = palette
            .commands
            .iter()
            .filter_map(|id| env.registry.get(*id))
            // Moving about is done with the keys, not looked up.
            .filter(|spec| spec.category != crate::command::Category::Navigation)
            .map(|spec| Item {
                what: What::Command(spec.id),
                title: spec.title.to_owned(),
                keys: env
                    .hint_key(spec.id)
                    .map_or_else(String::new, ToString::to_string),
            })
            .collect();
        items.extend(self.sidebar.entries.iter().filter_map(|entry| {
            let title = match entry {
                SidebarEntry::Unified { .. } => "Go to Inbox (unified)".to_owned(),
                SidebarEntry::Local { name, .. } => format!("Go to {name}"),
                SidebarEntry::Folder {
                    account,
                    name,
                    local,
                    ..
                } => format!(
                    "Go to {} / {name}{}",
                    self.accounts
                        .get(*account)
                        .map_or("", |account| account.name.as_str()),
                    if *local { ON_THIS_MAC_SUFFIX } else { "" }
                ),
                SidebarEntry::Search { name, query } => {
                    return Some(Item {
                        what: What::Search(query.clone()),
                        title: format!("Search: {name}"),
                        keys: String::new(),
                    });
                }
                SidebarEntry::Account { .. } | SidebarEntry::Searches | SidebarEntry::OnThisMac => {
                    return None;
                }
            };
            Some(Item {
                what: What::Mailbox(entry.mailbox()?),
                title,
                keys: String::new(),
            })
        }));
        palette::matching(palette.input.text(), items)
    }

    /// The message in the reader and the file the attachment list's cursor
    /// is on, when the list is open.
    fn chosen_attachment(&self) -> Option<(LocalMessageId, &AttachedFile)> {
        let picker = self.attachments?;
        let message = self.reader.row.as_ref()?.id;
        let file = self.reader.files().get(picker.selected)?;
        Some((message, file))
    }

    /// Asks for the chosen attachment to be written where it can be used.
    fn use_attachment(&mut self, then: FileUse, out: &mut Effects) {
        if let Some((message, file)) = self.chosen_attachment() {
            out.data.push(DataRequest::Extract {
                message,
                part: file.part,
                then,
            });
        }
    }

    /// Opens an empty message, from the account whose mail is in view.
    fn start_composing(&mut self, env: &Env) {
        let account = self
            .target_row()
            .map(|row| row.account)
            .filter(|_| self.mailbox != Mailbox::Unified)
            // An account chosen by its own row, with no mail to go by.
            .or(match self.mailbox {
                Mailbox::Account(id) => Some(id),
                _ => None,
            })
            .or_else(|| self.accounts.first().map(|account| account.id));
        let Some(account) = account else {
            return;
        };
        self.open_compose(
            env,
            Prefill {
                draft: None,
                account,
                content: DraftContent::default(),
                cursor: 0,
                focus: Field::To,
                title: "New message",
                note: None,
            },
        );
    }

    /// Puts a message to write in the reader's place.
    fn open_compose(&mut self, env: &Env, prefill: Prefill) {
        let fresh = prefill.draft.is_none();
        let account = prefill.account;
        // A new message ends with the account's signature. A draft taken
        // up again already has whatever it was given.
        let signature = self
            .accounts
            .iter()
            .find(|row| row.id == account)
            .and_then(|row| env.identities.get(&row.name))
            .and_then(|identity| identity.signature.as_deref())
            .filter(|signature| !signature.trim().is_empty())
            .filter(|_| fresh);
        let mut prefill = prefill;
        let mut placed = false;
        if let Some(signature) = signature
            && !prefill.content.body.trim().is_empty()
            && prefill.content.body.is_char_boundary(prefill.cursor)
        {
            let signature = signature.trim_end();
            let (before, after) = prefill.content.body.split_at(prefill.cursor);
            if before.trim().is_empty() {
                // Written above a quoted message, the signature goes
                // above it too, under what is about to be written.
                prefill.content.body = format!(
                    "{before}\n\n-- \n{signature}\n\n{}",
                    after.trim_start_matches('\n')
                );
                placed = true;
            } else if after.trim().is_empty() {
                // Written below it, the signature ends the message, with
                // a line left for the reply.
                let quoted = before.trim_end();
                prefill.cursor = quoted.len() + 2;
                prefill.content.body = format!("{quoted}\n\n\n\n-- \n{signature}\n");
                placed = true;
            }
        }
        let mut compose = Compose::new(prefill);
        if !placed && let Some(signature) = signature {
            compose.body.insert_signature(signature);
        }
        compose.body.mark_saved();
        self.compose = Some(compose);
        // Whatever Vim had belonged to a message that is no longer here:
        // its folder is cleared away when Vim goes.
        self.after_vim = None;
        self.vim_wanted = env.compose.vim;
        self.help = None;
        self.links = None;
        self.folders = None;
        self.hooks = None;
        // What was known of the last message's spelling says nothing of
        // this one's. A check still out is waited for, and then dropped.
        self.suggestions = None;
        self.spell.sent = None;
        self.spell.asking = None;
        if let Some((_, revision, _)) = &mut self.spell.flight {
            *revision = None;
        }
        self.set_focus(Pane::Reader);
    }

    /// The folder of the draft Vim is running on, while it is.
    fn vim_dir(&self) -> Option<PathBuf> {
        let compose = self.compose.as_ref()?;
        (compose.away == Some(Away::InVim))
            .then(|| self.vim.as_ref()?.dir.clone())
            .flatten()
    }

    /// Hands the message being written to Vim: asks for it to be written
    /// out as a draft file. Vim is started when that is done.
    fn start_vim(&mut self, env: &Env, out: &mut Effects) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        if compose.sending || compose.away.is_some() {
            return;
        }
        compose.completion = None;
        compose.attaching = None;
        compose.doubts.clear();
        compose.error = None;
        let content = compose.content();
        // Saved first, so that whatever becomes of Vim or of the terminal
        // it takes over, what has been written so far is a draft.
        if compose.is_modified() && !compose.is_blank() {
            out.data.push(DataRequest::SaveDraft {
                draft: compose.draft,
                account: compose.account,
                content: Box::new(content.clone()),
                announce: false,
            });
        }
        compose.away = Some(Away::Leaving);
        // Where the host has a place for an editor, it runs in the text's
        // place and is given the text alone. In a terminal it has the
        // whole screen, and the whole message.
        compose.embedded = env.editor_pane;
        let body_only = compose.embedded;
        out.data.push(DataRequest::EditOut {
            account: compose.account,
            content: Box::new(content.clone()),
            // A Vim is given a copy of the plugin beside the draft. No
            // other editor has a use for it.
            plugin: env.external.is_vim(),
            share: if body_only {
                vim::Share::Body
            } else {
                vim::Share::Whole
            },
        });
        self.vim = Some(VimEdit {
            handed: content,
            dir: None,
            body_only,
        });
        self.templates = None;
        self.suggestions = None;
        self.when = None;
    }

    /// The draft file is written: have Vim run on it.
    fn on_edit_ready(
        &mut self,
        env: &Env,
        dir: PathBuf,
        path: PathBuf,
        body_line: u32,
        out: &mut Effects,
    ) {
        match (&mut self.compose, &mut self.vim) {
            (Some(compose), Some(edit)) if compose.away == Some(Away::Leaving) => {
                compose.away = Some(Away::InVim);
                edit.dir = Some(dir.clone());
                self.tips_seen |= compose.embedded && self.editor_tips;
                let subject = compose.subject.text().trim();
                let title = if subject.is_empty() {
                    compose.title.to_owned()
                } else {
                    text::sanitize_line(subject)
                };
                out.host.push(HostRequest::EditDraft {
                    path,
                    dir,
                    title,
                    editor: env.external,
                    body_line,
                });
            }
            // The message it was for has gone.
            _ => {
                self.vim = None;
                out.data.push(DataRequest::EditDrop { dir });
            }
        }
    }

    /// Vim has gone, or never started.
    fn on_editor_exited(&mut self, env: &Env, ran: bool, status: i32, out: &mut Effects) {
        let Some(edit) = self.vim.take() else {
            return;
        };
        let compose = self
            .compose
            .as_mut()
            .filter(|compose| compose.away == Some(Away::InVim));
        let (Some(compose), Some(dir)) = (compose, edit.dir.clone()) else {
            // Nothing here is waiting for it: only its folder is left.
            if let Some(dir) = edit.dir {
                out.data.push(DataRequest::EditDrop { dir });
            }
            return;
        };
        if !ran {
            compose.away = None;
            // Said in the message's own pane, where the next key clears
            // it: the message is still there to write.
            let editor = env.external;
            compose.error = Some(match &env.editor_program {
                Some(named) => format!(
                    "{} could not be started, so the message stays here. Quill looked \
                     for {}, which `editor_path` or QUILL_VIM names.",
                    editor.name(),
                    text::sanitize_line(&named.to_string_lossy())
                ),
                None => format!(
                    "{} could not be started, so the message stays here. Quill runs \
                     the `{}` on your PATH; `editor_path` in the settings file names \
                     another.",
                    editor.name(),
                    editor.program()
                ),
            });
            out.data.push(DataRequest::EditDrop { dir });
        } else if status != 0 {
            // `:cq`, or a crash: never mind. The message is as it was.
            compose.away = None;
            self.flash = Some((
                format!(
                    "{} quit without handing the message back: it is as it was.",
                    env.external.name()
                ),
                false,
            ));
            out.data.push(DataRequest::EditDrop { dir });
        } else {
            compose.away = Some(Away::Returning);
            out.data.push(DataRequest::EditBack {
                dir,
                before: Box::new(edit.handed.clone()),
            });
            self.vim = Some(edit);
        }
    }

    /// What was written in Vim has been read: the message takes it, and
    /// then what Vim said is done with it.
    fn on_edit_back(
        &mut self,
        file: quill_store::draft_file::DraftFile,
        verdict: Option<Verdict>,
        out: &mut Effects,
    ) {
        let Some(edit) = self.vim.take() else {
            return;
        };
        let Some(compose) = self
            .compose
            .as_mut()
            .filter(|compose| compose.away == Some(Away::Returning))
        else {
            return;
        };
        if edit.body_only {
            compose.take_body_back(&file.content.body);
        } else {
            compose.take_back(file.content, edit.handed.attachments.len());
        }
        if verdict == Some(Verdict::Discard) {
            // Thrown away, files and all: nothing more is attached to it.
            compose.away = None;
            self.discard_compose(out);
            return;
        }
        // Files newly named are attached as if their paths had been typed
        // here, with the same limits. What Vim said waits for them: the
        // store answers `Settle` only once it has dealt with them all.
        out.data.extend(
            file.attach
                .into_iter()
                .map(|path| DataRequest::Attach { path, handed: None }),
        );
        self.after_vim = Some(AfterVim {
            verdict,
            attach_failed: false,
        });
        out.data.push(DataRequest::Settle);
    }

    /// The message is back from Vim with its files attached: send it,
    /// put it away, or leave it open, as Vim said.
    fn after_vim(&mut self, env: &Env, out: &mut Effects) {
        let Some(after) = self.after_vim.take() else {
            return;
        };
        let Some(compose) = self
            .compose
            .as_mut()
            .filter(|compose| compose.away == Some(Away::Returning))
        else {
            return;
        };
        compose.away = None;
        if after.attach_failed {
            // Why is on show. The message stays, for the user to decide.
            return;
        }
        match after.verdict {
            Some(Verdict::Send) => self.send(env, None, out),
            Some(Verdict::Postpone) => self.put_away(true, out),
            Some(Verdict::Discard) => self.discard_compose(out),
            // `:wq` with nothing said: the message is here, to send, to
            // save or to close.
            None => self.flash = Some((format!("Back from {}.", env.external.name()), false)),
        }
    }

    /// Throws the message being written away, and its draft with it.
    fn discard_compose(&mut self, out: &mut Effects) {
        let Some(compose) = self.compose.take() else {
            return;
        };
        match compose.draft {
            // The store says so once it is gone.
            Some(draft) => out.data.push(DataRequest::Delete(draft.as_message_id())),
            None => self.flash = Some(("Thrown away.".to_owned(), false)),
        }
        self.set_focus(Pane::List);
    }

    /// The message a hook would be given: the one the user is on, when it
    /// is mail and not something still being written.
    fn hook_message(&self) -> Option<LocalMessageId> {
        self.target_row()
            .map(|row| row.id)
            .filter(|id| DraftId::from_message_id(*id).is_none())
    }

    /// The hooks the picker offers, as places in `env.hooks`: those whose
    /// names what has been typed narrows to.
    pub fn hook_choices(&self, env: &Env, picker: &HookPicker) -> Vec<usize> {
        // The same box with another list in it: its lines, narrowed the
        // same way.
        if let Some(list) = &picker.list {
            return list
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| template::narrows(picker.filter.text(), &line.name))
                .map(|(index, _)| index)
                .collect();
        }
        env.hooks
            .iter()
            .enumerate()
            .filter(|(_, hook)| template::narrows(picker.filter.text(), &hook.name))
            .map(|(index, _)| index)
            .collect()
    }

    /// Gives the message to the chosen hook: asks the store for the
    /// message in the form the hook wants. The program is started when
    /// that arrives.
    fn run_hook(&mut self, env: &Env, out: &mut Effects) {
        let Some(picker) = self.hooks.take() else {
            return;
        };
        if let Some(list) = &picker.list {
            if let Some(index) = self.hook_choices(env, &picker).get(picker.selected) {
                self.picked(env, list, *index, out);
            }
            return;
        }
        let chosen = self
            .hook_choices(env, &picker)
            .get(picker.selected)
            .and_then(|index| env.hooks.get(*index));
        let Some(hook) = chosen.filter(|_| env.hooks_enabled) else {
            return;
        };
        if let Some(running) = &self.hook_running {
            self.flash = Some((format!("{running} is still running."), false));
            return;
        }
        self.hook_running = Some(hook.name.clone());
        self.flash = Some((format!("Running {}…", hook.name), false));
        out.data.push(DataRequest::ReadForHook {
            message: picker.message,
            hook: Box::new(hook.clone()),
        });
    }

    /// Whether spelling is checked at all: the user wants it and the host
    /// has a checker.
    fn spelling_on(&self, env: &Env) -> bool {
        env.spell_check && env.compose.spell_check
    }

    /// Whether the body of the message being written wants its spelling
    /// checked: it has changed since it was last sent for checking, and no
    /// check is out. The run loop asks, and sends [`Action::TypingPaused`]
    /// once the keys have been quiet for a moment.
    pub fn spell_check_due(&self, env: &Env) -> bool {
        self.spelling_on(env)
            && self
                .compose
                .as_ref()
                .is_some_and(|compose| Some(compose.body.revision()) != self.spell.sent)
            // One check at a time: a newer text waits for the answer to
            // the older. An answer that never comes is not waited for for
            // ever.
            && self
                .spell
                .flight
                .as_ref()
                .is_none_or(|(_, _, sent_at)| self.now - sent_at >= SPELL_PATIENCE)
    }

    /// Sends the body of the message being written to have its spelling
    /// checked, if it is due. The body and nothing else.
    fn check_spelling(&mut self, env: &Env, out: &mut Effects) {
        if !self.spell_check_due(env) {
            return;
        }
        let Some(compose) = &mut self.compose else {
            return;
        };
        let revision = compose.body.revision();
        let text = compose.body.text();
        self.spell.sent = Some(revision);
        if text.len() > MOST_SPELL_CHECKED || text.trim().is_empty() {
            // Not sent: too long to check, or nothing to. Marks from when
            // it was otherwise would say it had been.
            compose.body.set_misspellings(Vec::new());
            return;
        }
        self.spell.last_id += 1;
        self.spell.flight = Some((self.spell.last_id, Some(revision), self.now));
        out.host.push(HostRequest::SpellCheck {
            id: self.spell.last_id,
            text,
        });
    }

    /// Takes the checker's answer: the misspelt words of the text that was
    /// sent. An answer for a text that has changed since is dropped, since
    /// its offsets no longer say where anything is; the text as it is now
    /// is checked next.
    fn on_spelling(&mut self, id: u64, words: &[(usize, usize)]) {
        let Some((_, revision, _)) = self.spell.flight.take_if(|(sent, _, _)| *sent == id) else {
            return;
        };
        let (Some(compose), Some(revision)) = (&mut self.compose, revision) else {
            return;
        };
        if compose.body.revision() != revision {
            return;
        }
        // The offsets are the host's word for it. One that does not fall
        // between characters of the text that was sent is not used.
        let text = compose.body.text();
        let ranges = words
            .iter()
            .filter(|(start, end)| {
                start < end
                    && *end <= text.len()
                    && text.is_char_boundary(*start)
                    && text.is_char_boundary(*end)
            })
            .map(|(start, end)| *start..*end)
            .collect();
        compose.body.set_misspellings(ranges);
    }

    /// Asks the host what the misspelt word at the cursor might have been
    /// meant to be.
    fn ask_for_suggestions(&mut self, out: &mut Effects) {
        let Some(compose) = &self.compose else {
            return;
        };
        let Some(range) = compose.body.misspelling_at_cursor() else {
            return;
        };
        let Some(word) = compose.body.text().get(range.clone()).map(str::to_owned) else {
            return;
        };
        self.spell.last_id += 1;
        self.spell.asking = Some((self.spell.last_id, range, compose.body.revision()));
        out.host.push(HostRequest::SpellSuggest {
            id: self.spell.last_id,
            word,
        });
    }

    /// Takes the host's suggestions and offers them, if the word they are
    /// for is still as it was and the message is still what is in front
    /// of the user.
    fn on_suggestions(&mut self, id: u64, words: Vec<String>) {
        let Some((_, range, revision)) = self.spell.asking.take_if(|(asked, ..)| *asked == id)
        else {
            return;
        };
        if self.context() != Context::Compose {
            return;
        }
        let Some(compose) = &self.compose else {
            return;
        };
        if compose.body.revision() != revision {
            return;
        }
        let Some(word) = compose.body.text().get(range.clone()).map(str::to_owned) else {
            return;
        };
        let mut offered: Vec<String> = Vec::new();
        for suggestion in words {
            // One word or a few, on one line, and not what is there now.
            let suggestion = text::sanitize_line(&suggestion).trim().to_owned();
            if !suggestion.is_empty()
                && suggestion.len() <= 200
                && suggestion != word
                && !offered.contains(&suggestion)
                && offered.len() < MOST_SUGGESTIONS
            {
                offered.push(suggestion);
            }
        }
        if offered.is_empty() {
            self.flash = Some((
                format!("No suggestions for \"{}\".", text::sanitize_line(&word)),
                false,
            ));
            return;
        }
        self.suggestions = Some(SuggestionPicker {
            word,
            range,
            revision,
            words: offered,
            selected: 0,
            offset: 0,
        });
    }

    /// Puts the chosen spelling in place of the word, as one step to undo.
    fn use_suggestion(&mut self) {
        let Some(picker) = self.suggestions.take() else {
            return;
        };
        let (Some(word), Some(compose)) = (picker.words.get(picker.selected), &mut self.compose)
        else {
            return;
        };
        if compose.body.revision() == picker.revision
            && compose.body.replace_range(picker.range, word)
        {
            compose.field = Field::Body;
            compose.doubts.clear();
        }
    }

    /// The message in the reader, when it is the one the user is on and
    /// carries an invitation that asks them for an answer.
    fn invite_asked(&self, context: Context) -> Option<LocalMessageId> {
        let id = self.page_message(context)?;
        let row = self.reader.row.as_ref()?;
        let Body::Loaded(body) = &self.reader.body else {
            return None;
        };
        let account = self.accounts.iter().find(|a| a.id == row.account)?;
        body.invite
            .as_ref()?
            .invite
            .asks(&account.address)
            .then_some(id)
    }

    /// Asks for an answer to the invitation in the reader to be built and
    /// put in the outbox.
    fn answer_invite(
        &mut self,
        env: &Env,
        answer: quill_mime::invite::Answer,
        context: Context,
        out: &mut Effects,
    ) {
        let Some(message) = self.invite_asked(context) else {
            return;
        };
        let from_name = self
            .reader
            .row
            .as_ref()
            .and_then(|row| self.accounts.iter().find(|a| a.id == row.account))
            .and_then(|account| env.identities.get(&account.name))
            .and_then(|identity| identity.full_name.clone());
        out.data
            .push(DataRequest::AnswerInvite(Box::new(InviteAnswer {
                message,
                answer,
                from_name,
                delay_seconds: i64::from(env.compose.undo_send_seconds),
                utc_offset: jiff::Timestamp::from_second(self.now)
                    .map_or(0, |now| env.tz.to_offset(now).seconds()),
            })));
    }

    /// Hands the message over to be checked, built and put in the outbox.
    /// Takes the time typed into the box and does what it was asked for.
    fn answer_when(&mut self, env: &Env, out: &mut Effects) {
        let Some(when) = &mut self.when else {
            return;
        };
        let typed = when.input.text().trim().to_lowercase();
        // A snoozed message can be had back at once.
        if typed == "now"
            && let WhenFor::Snooze(ids) = &when.what
        {
            out.data.push(DataRequest::Snooze {
                ids: ids.clone(),
                until: None,
            });
            self.when = None;
            self.list.clear_marks();
            return;
        }
        let Some(at) = crate::when::parse(&typed, self.now, &env.tz) else {
            when.problem = Some(if typed.is_empty() {
                "Type a time.".to_owned()
            } else {
                "That is not a time to come that Quill can read.".to_owned()
            });
            return;
        };
        let Some(when) = self.when.take() else {
            return;
        };
        match when.what {
            WhenFor::Snooze(ids) => {
                out.data.push(DataRequest::Snooze {
                    ids,
                    until: Some(at),
                });
                self.list.clear_marks();
            }
            WhenFor::SendLater => self.send(env, Some(at), out),
        }
    }

    /// Gives up on the sign-in that is waiting on the user.
    fn give_up_sign_in(&mut self, out: &mut Effects) {
        let Some(sign_in) = self.sign_in.take() else {
            return;
        };
        // Nothing is kept of a sign-in given up on.
        out.accounts.push(AccountRequest::CancelSignIn);
        // The account the form added a moment ago is kept, though, and
        // has not been heard saying what it still needs.
        if sign_in.fresh {
            self.notices.push_back(Notice::info(format!(
                "Added {}, with nobody signed in to it yet. \"Sign in to an account\" in the palette or the File menu does that.",
                sign_in.account
            )));
        }
    }

    /// Sends the message being written: after the undo delay, or `at` a
    /// time the user chose.
    fn send(&mut self, env: &Env, at: Option<i64>, out: &mut Effects) {
        let from_name = self.compose.as_ref().and_then(|compose| {
            self.accounts
                .iter()
                .find(|row| row.id == compose.account)
                .and_then(|row| env.identities.get(&row.name))
                .and_then(|identity| identity.full_name.clone())
        });
        let Some(compose) = &mut self.compose else {
            return;
        };
        if compose.sending {
            return;
        }
        compose.sending = true;
        compose.error = None;
        out.data.push(DataRequest::Send(Box::new(SendRequest {
            draft: compose.draft,
            account: compose.account,
            content: compose.content(),
            from_name,
            format: if env.compose.markdown {
                BodyFormat::Markdown
            } else {
                BodyFormat::Plain
            },
            delay_seconds: match at {
                Some(at) => at.saturating_sub(self.now).max(0),
                None => i64::from(env.compose.undo_send_seconds),
            },
            utc_offset: jiff::Timestamp::from_second(self.now)
                .map_or(0, |now| env.tz.to_offset(now).seconds()),
            // Asked a second time, with the doubts on screen: send it.
            confirmed: !compose.doubts.is_empty(),
        })));
    }

    /// The folders the picker offers: those of the message's account other
    /// than the one it is in, narrowed by what has been typed. Letters
    /// typed need only appear in a name in order, so "arc" and "ahv" both
    /// find Archive.
    pub fn folder_choices(&self, picker: &FolderPicker) -> Vec<FolderChoice> {
        let Some(row) = self
            .list
            .rows
            .iter()
            .chain(self.reader.row.as_ref())
            .find(|row| row.id == picker.message)
        else {
            return Vec::new();
        };
        let typed = picker.filter.text().to_lowercase();
        self.sidebar
            .entries
            .iter()
            .filter_map(|entry| match entry {
                SidebarEntry::Folder {
                    id,
                    account,
                    name,
                    local,
                    ..
                } if *id != row.folder
                    && self.accounts.get(*account).map(|account| account.id)
                        == Some(row.account) =>
                {
                    Some(FolderChoice {
                        id: *id,
                        name: name.clone(),
                        local: *local,
                    })
                }
                _ => None,
            })
            .filter(|choice| template::narrows(&typed, &choice.name))
            .collect()
    }

    /// The templates the picker offers, as places in `env.templates`:
    /// those whose names what has been typed narrows to.
    pub fn template_choices(&self, env: &Env, picker: &TemplatePicker) -> Vec<usize> {
        env.templates
            .iter()
            .enumerate()
            .filter(|(_, template)| template::narrows(picker.filter.text(), &template.name))
            .map(|(index, _)| index)
            .collect()
    }

    /// Puts the chosen template into the message being written, with what
    /// is known about the message filled in.
    fn insert_template(&mut self, env: &Env) {
        let Some(picker) = self.templates.take() else {
            return;
        };
        let chosen = self
            .template_choices(env, &picker)
            .get(picker.selected)
            .and_then(|index| env.templates.get(*index));
        let (Some(template), Some(compose)) = (chosen, &mut self.compose) else {
            return;
        };
        let from_name = self
            .accounts
            .iter()
            .find(|row| row.id == compose.account)
            .and_then(|row| env.identities.get(&row.name))
            .and_then(|identity| identity.full_name.as_deref());
        let values = template::Values::of(
            compose.to.text(),
            compose.subject.text(),
            from_name,
            &text::day(self.now, &env.tz),
        );
        compose.insert_template(template, values);
    }

    /// What unsubscribing from the list of the message in the reader would
    /// be, when that message is the one the user is on, is here in full,
    /// and its list gives a way Quill will use. One click needs the
    /// service that makes the request, so without one (the demo mailbox)
    /// it is a message or the list's page.
    fn unsubscribe_offer(&self, env: &Env, context: Context) -> Option<UnsubscribePrompt> {
        self.page_message(context)?;
        let row = self.reader.row.as_ref()?;
        let Body::Loaded(body) = &self.reader.body else {
            return None;
        };
        let offered = body.unsubscribe.as_ref()?;
        let host = |url: &str| quill_mime::unsubscribe::web_host(url).map(text::sanitize_line);
        let how = match offered.way(env.accounts)? {
            quill_mime::unsubscribe::Way::OneClick(url) => UnsubscribeHow::OneClick {
                url: url.to_owned(),
                host: host(url)?,
            },
            quill_mime::unsubscribe::Way::Mail(url) => UnsubscribeHow::Mail {
                url: url.to_owned(),
                address: quill_core::links::parse_mailto(url)?
                    .to
                    .first()
                    .map(|address| text::sanitize_line(address))?,
            },
            quill_mime::unsubscribe::Way::Page(url) => UnsubscribeHow::Page {
                url: Some(url)
                    .filter(|url| quill_mime::links::is_openable(url))?
                    .to_owned(),
                host: host(url)?,
            },
        };
        Some(UnsubscribePrompt {
            account: row.account,
            sender: row.headers.from.first().map_or_else(
                || "a sender it does not name".to_owned(),
                ToString::to_string,
            ),
            how,
        })
    }

    /// Whether the message in the reader can be unsubscribed from, for the
    /// reader to say so.
    pub fn reader_can_unsubscribe(&self, env: &Env) -> bool {
        self.unsubscribe_offer(env, Context::Reader).is_some()
    }

    /// The user said yes: does the one thing the question described.
    fn answer_unsubscribe(&mut self, env: &Env, out: &mut Effects) {
        let Some(prompt) = self.unsubscribe.take() else {
            return;
        };
        match prompt.how {
            UnsubscribeHow::OneClick { url, host } => {
                out.accounts.push(AccountRequest::Unsubscribe { url });
                self.flash = Some((format!("Asking {host} to unsubscribe you…"), false));
            }
            UnsubscribeHow::Mail { url, .. } => {
                let Some(mut mailto) = quill_core::links::parse_mailto(&url) else {
                    return;
                };
                // The request goes to the address the question named and
                // to nobody else, whoever else the header names.
                mailto.to.truncate(1);
                mailto.cc.clear();
                mailto.bcc.clear();
                if mailto.subject.trim().is_empty() {
                    // What lists expect when they do not say.
                    mailto.subject = "unsubscribe".to_owned();
                }
                self.write_to(env, mailto, Some(prompt.account));
                if self.compose.is_some() {
                    self.flash = Some((
                        "Nothing has been sent. Send this message to unsubscribe.".to_owned(),
                        false,
                    ));
                }
            }
            UnsubscribeHow::Page { url, host } => {
                if quill_mime::links::is_openable(&url) {
                    out.host.push(HostRequest::OpenUrl(url));
                    self.flash = Some((format!("Opened {host} in your browser."), false));
                }
            }
        }
    }

    /// Takes a key as the answer to the certificate on screen: `o` trusts
    /// it once, `p` pins it, `r` rejects it. Returns whether the key was an
    /// answer (or typing that must not fall through to a command).
    fn answer_certificate(&mut self, chord: KeyChord, out: &mut Effects) -> bool {
        let Some(prompt) = self.certificates.front_mut() else {
            return false;
        };
        let Key::Char(letter) = chord.key else {
            return false;
        };
        if prompt.busy || chord.mods != Mods::default() {
            return true;
        }
        let pin = match letter {
            'r' => {
                self.certificates.pop_front();
                return true;
            }
            'o' => false,
            'p' => true,
            _ => return true,
        };
        // A certificate with no fingerprint cannot be held to anything.
        if let Some(fingerprint) = prompt.problem.fingerprint.clone() {
            prompt.busy = true;
            out.accounts.push(AccountRequest::Trust {
                account: prompt.account.clone(),
                fingerprint,
                pin,
                sending: prompt.sending,
            });
        }
        true
    }

    fn on_account(&mut self, env: &Env, event: AccountEvent, out: &mut Effects) {
        // An account has just been written into the config: the settings
        // window lists the accounts the config has, so it is read again.
        if self.settings.is_some() && matches!(event, AccountEvent::Saved(Ok(_))) {
            out.accounts.push(AccountRequest::Settings(None));
        }
        if let AccountEvent::Trusted {
            account,
            sending,
            result,
        } = event
        {
            self.certificates
                .retain(|prompt| prompt.account != account || prompt.sending != sending);
            if let Err(reason) = result {
                self.notices.push_back(Notice::error(format!(
                    "{account}: the certificate could not be accepted: {}",
                    text::sanitize_line(&reason)
                )));
            }
            return;
        }
        match event {
            AccountEvent::SignInBrowser { account, url } => {
                // An answer for a sign-in already given up on is dropped.
                if let Some(sign_in) = &mut self.sign_in
                    && quill_mime::links::is_openable(&url)
                {
                    out.host.push(HostRequest::OpenUrl(url));
                    sign_in.account = text::sanitize_line(&account);
                    sign_in.stage = SignInStage::Browser;
                }
                return;
            }
            AccountEvent::SignInCode {
                account,
                address,
                code,
                minutes,
            } => {
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.account = text::sanitize_line(&account);
                    sign_in.stage = SignInStage::Code {
                        address: text::sanitize_line(&address),
                        code: text::sanitize_line(&code),
                        minutes,
                    };
                }
                return;
            }
            AccountEvent::SignedIn { account, result } => {
                if self.sign_in.take().is_none() {
                    return;
                }
                let name = text::sanitize_line(&account);
                match result {
                    Ok(()) => {
                        self.notices.push_back(Notice::info(format!(
                            "Signed in to {name}. Its mail is on the way."
                        )));
                        out.sync.push(SyncRequest::SyncNow);
                    }
                    // With no account named, the reason says it all.
                    Err(reason) if name.is_empty() => self
                        .notices
                        .push_back(Notice::error(text::sanitize_line(&reason))),
                    Err(reason) => self.notices.push_back(Notice::error(format!(
                        "{name} is not signed in: {}",
                        text::sanitize_line(&reason)
                    ))),
                }
                return;
            }
            AccountEvent::CommandLinked(result) => {
                self.notices.push_back(match result {
                    Ok(said) => Notice::info(text::sanitize_line(&said)),
                    Err(reason) => Notice::error(format!(
                        "The command line tool was not installed: {}.",
                        text::sanitize_line(reason.trim_end_matches('.'))
                    )),
                });
                return;
            }
            AccountEvent::Rules(now) => {
                self.on_rules(env, now, out);
                return;
            }
            AccountEvent::RuleAdded(result) => {
                self.on_rule_added(env, result, out);
                return;
            }
            AccountEvent::RulesFile(result) => {
                self.on_rules_file(env, result, out);
                return;
            }
            AccountEvent::Remembered(result) => {
                // The choice holds for this run. That it will not be how
                // Quill starts next time is worth a line, not a notice to
                // dismiss.
                if let Err(reason) = result {
                    self.flash = Some((
                        format!(
                            "That holds until Quill is closed: {}",
                            text::sanitize_line(&reason)
                        ),
                        false,
                    ));
                }
                // The settings window shows what the config says, and the
                // config has just been written to.
                if self.settings.is_some() {
                    out.accounts.push(AccountRequest::Settings(None));
                }
                return;
            }
            AccountEvent::Settings { read, changed } => {
                self.on_settings_read(env, read, changed, out);
                return;
            }
            AccountEvent::KeysChanged { read, result } => {
                self.on_settings_read(env, read, None, out);
                // Why the keys are as they were, said in the window.
                if let Err(why) = result {
                    self.settings_answered(Some(text::sanitize_line(&why)));
                }
                return;
            }
            AccountEvent::ThemesChanged { read, result } => {
                self.on_settings_read(env, read, None, out);
                // Why the themes are as they were, said in the window.
                if let Err(why) = result {
                    self.settings_answered(Some(text::sanitize_line(&why)));
                }
                return;
            }
            AccountEvent::RemoveAsked { account, found } => {
                self.on_remove_asked(account, found);
                return;
            }
            AccountEvent::Removed { account, result } => {
                self.on_removed(account, result, out);
                return;
            }
            AccountEvent::EditAsked { account, found } => {
                self.on_edit_asked(account, found);
                return;
            }
            AccountEvent::LetterSet {
                account,
                letter,
                result,
            } => {
                self.on_letter_set(&account, letter, result, out);
                return;
            }
            AccountEvent::Unsubscribed { host, result } => {
                let host = text::sanitize_line(&host);
                self.notices.push_back(match result {
                    Ok(()) => Notice::info(format!(
                        "Unsubscribed: {host} took the request. Mail already on its way may still arrive."
                    )),
                    Err(reason) => Notice::error(format!(
                        "Not unsubscribed. {}",
                        text::sanitize_line(&reason)
                    )),
                });
                return;
            }
            _ => {}
        }
        let Some(setup) = &mut self.setup else {
            // The form was put away while an account was being saved, and
            // it was saved all the same. It is not added unseen.
            if let AccountEvent::Saved(Ok(name)) = event {
                self.notices.push_back(Notice::info(format!(
                    "Added {}: it was already being saved when the form was closed.",
                    text::sanitize_line(&name)
                )));
                out.data.push(DataRequest::LoadSidebar);
                out.data.push(DataRequest::LoadMessages(self.mailbox));
            }
            return;
        };
        let (requests, outcome) = setup.on_event(event);
        out.accounts.extend(requests);
        match outcome {
            Outcome::Open => {}
            Outcome::Changed(name) => {
                self.setup = None;
                self.on_account_changed(&name, out);
            }
            Outcome::Added(name) => {
                self.setup = None;
                self.notices.push_back(Notice::info(format!(
                    "Added {}. Its mail is on the way.",
                    text::sanitize_line(&name)
                )));
                // The new account is in the store: show it.
                out.data.push(DataRequest::LoadSidebar);
                out.data.push(DataRequest::LoadMessages(self.mailbox));
            }
            // Saved, with nobody signed in to it yet. The form has asked
            // for that already, so its place is taken by the sign-in, which
            // says how it ends.
            Outcome::SigningIn(name) => {
                self.setup = None;
                self.sign_in = Some(SignIn {
                    account: text::sanitize_line(&name),
                    stage: SignInStage::Starting,
                    fresh: true,
                });
                out.data.push(DataRequest::LoadSidebar);
                out.data.push(DataRequest::LoadMessages(self.mailbox));
            }
        }
    }

    /// Takes a digit typed while choosing a link as part of its number, and
    /// moves to that link. Returns whether the key was taken.
    fn type_link_number(&mut self, chord: KeyChord) -> bool {
        let count = self.reader.links.len();
        let (Some(picker), Key::Char(digit @ '0'..='9')) = (&mut self.links, chord.key) else {
            return false;
        };
        if chord.mods != Mods::default() {
            return false;
        }
        picker.typed.push(digit);
        let number = |typed: &str| {
            typed
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=count).contains(n))
        };
        // "1" then "2" is link 12 when there is one, and link 2 otherwise.
        let chosen = number(&picker.typed).or_else(|| {
            picker.typed = digit.to_string();
            number(&picker.typed)
        });
        match chosen {
            Some(number) => picker.selected = number - 1,
            None => picker.typed.clear(),
        }
        true
    }

    /// Hands the chosen link to the host to open, if it is one Quill opens.
    fn open_selected_link(&mut self, out: &mut Effects) {
        let Some(picker) = self.links.take() else {
            return;
        };
        let Some(url) = self.reader.links.get(picker.selected) else {
            return;
        };
        if quill_mime::links::is_openable(url) {
            out.host.push(HostRequest::OpenUrl(url.clone()));
        } else {
            self.notices.push_back(Notice::error(
                "Quill only opens web (http, https) and mail (mailto) links. This is not one.",
            ));
        }
    }

    /// The message a message command acts on: the one being read when the
    /// reader has focus, otherwise the one under the list cursor.
    fn target_row(&self) -> Option<&MessageRow> {
        match self.focus {
            Pane::Reader => self.reader.row.as_ref(),
            Pane::Sidebar | Pane::List => self.list.rows.get(self.list.selected),
        }
    }

    /// The account the user is looking at, where that can be told: the
    /// one whose message is under the cursor, or whose folder is open.
    fn current_account(&self) -> Option<&AccountRow> {
        if let Some(row) = self.target_row() {
            let id = row.account;
            return self.accounts.iter().find(|account| account.id == id);
        }
        let folder = match self.mailbox {
            Mailbox::Folder(folder) => folder,
            Mailbox::Account(id) => {
                return self.accounts.iter().find(|account| account.id == id);
            }
            _ => return None,
        };
        self.sidebar.entries.iter().find_map(|entry| match entry {
            SidebarEntry::Folder { id, account, .. } if *id == folder => {
                self.accounts.get(*account)
            }
            _ => None,
        })
    }

    /// The account chosen in the accounts pane: the one the cursor there
    /// is on, or whose folder it is on, wherever the keys are. `None` on
    /// a row that is every account's: the unified inbox, the mail kept
    /// only here, a saved search.
    /// The message the list is on, while the list is the Outbox's: what
    /// Send is for when no message is being written.
    fn outbox_selection(&self) -> Option<DraftId> {
        if self.mailbox != Mailbox::Outbox || self.search.is_some() {
            return None;
        }
        let row = self.list.rows.get(self.list.selected)?;
        DraftId::from_message_id(row.id)
    }

    fn chosen_account(&self) -> Option<&AccountRow> {
        let id = self.sidebar.account_of(self.sidebar.selected)?;
        self.accounts.iter().find(|account| account.id == id)
    }

    /// Opens or closes an account in the accounts pane, and has the store
    /// remember it.
    fn set_account_open(&mut self, account: LocalAccountId, open: bool, out: &mut Effects) {
        let changed = if open {
            self.sidebar.closed.remove(&account)
        } else {
            self.sidebar.closed.insert(account)
        };
        if changed {
            out.data.push(DataRequest::SetAccountOpen { account, open });
        }
    }

    /// Folds an account's folders away. The cursor does not go with them:
    /// from one of them it moves to the account's own row, and the list
    /// shows what that row stands for.
    fn close_account(&mut self, env: &Env, account: LocalAccountId, out: &mut Effects) {
        let on_a_folder = matches!(
            self.sidebar.entries.get(self.sidebar.selected),
            Some(SidebarEntry::Folder { .. })
        ) && self.sidebar.account_of(self.sidebar.selected) == Some(account);
        if on_a_folder && let Some(row) = self.sidebar.account_row(account) {
            self.go_to_entry(env, row, out);
        }
        self.set_account_open(account, false, out);
    }

    /// Opens an account that is closed and closes one that is open: what
    /// a click on its marker does.
    pub(super) fn toggle_account(&mut self, env: &Env, account: LocalAccountId, out: &mut Effects) {
        if self.sidebar.is_open(account) {
            self.close_account(env, account, out);
        } else {
            self.set_account_open(account, true, out);
        }
    }

    /// Puts the accounts pane's cursor on a row. The cursor is never on a
    /// row that is not shown: going to a folder of a closed account opens
    /// the account.
    fn select_row(&mut self, index: usize, out: &mut Effects) {
        self.sidebar.selected = index;
        if matches!(
            self.sidebar.entries.get(index),
            Some(SidebarEntry::Folder { .. })
        ) && let Some(account) = self.sidebar.account_of(index)
        {
            self.set_account_open(account, true, out);
        }
    }

    /// Puts the accounts pane's cursor on the row of a mailbox, if it has
    /// one.
    fn select_mailbox_row(&mut self, mailbox: Mailbox, out: &mut Effects) -> bool {
        let row = self
            .sidebar
            .entries
            .iter()
            .position(|entry| entry.mailbox() == Some(mailbox));
        if let Some(row) = row {
            self.select_row(row, out);
        }
        row.is_some()
    }

    /// Starts a message to the people a `mailto:` address names.
    fn on_mailto(&mut self, env: &Env, url: &str) {
        let Some(mailto) = quill_core::links::parse_mailto(url) else {
            return;
        };
        self.write_to(env, mailto, None);
    }

    /// Starts a message from what a `mailto:` address asked for, from
    /// `account` when it is given and still here, otherwise from the first.
    fn write_to(
        &mut self,
        env: &Env,
        mailto: quill_core::links::Mailto,
        account: Option<LocalAccountId>,
    ) {
        if self.compose.is_some() {
            self.notices.push_back(Notice::info(
                "A message is already being written. Send or close it, then follow the link again.",
            ));
            return;
        }
        let account = account
            .filter(|wanted| self.accounts.iter().any(|account| account.id == *wanted))
            .or_else(|| self.accounts.first().map(|account| account.id));
        let Some(account) = account else {
            self.notices.push_back(Notice::info(
                "Add an account first; then a mail link can start a message.",
            ));
            return;
        };
        let clean = |list: &[String]| -> String {
            list.iter()
                .map(|address| text::sanitize_line(address))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let content = DraftContent {
            to: clean(&mailto.to),
            cc: clean(&mailto.cc),
            bcc: clean(&mailto.bcc),
            subject: text::sanitize_line(&mailto.subject),
            body: text::sanitize_body(&mailto.body),
            ..DraftContent::default()
        };
        // Start where there is still something to fill in.
        let focus = if content.to.is_empty() {
            Field::To
        } else if content.subject.is_empty() {
            Field::Subject
        } else {
            Field::Body
        };
        // A link's words are a start: the cursor goes after them.
        let cursor = content.body.len();
        self.setup = None;
        self.palette = None;
        self.open_compose(
            env,
            Prefill {
                draft: None,
                account,
                content,
                cursor,
                focus,
                title: "New message",
                note: None,
            },
        );
    }

    /// Does what a notification was clicked for: goes to its message, or
    /// archives it, marks it read or starts a reply.
    /// Something was changed or sent from outside the window. It is said
    /// in the bottom bar already; here the user is told as a notification,
    /// as far as they asked to be, and the settings window's list of what
    /// was done last is asked for again.
    fn on_agent_notice(&mut self, env: &Env, notice: &crate::agent::Notice, out: &mut Effects) {
        use quill_core::config::AgentNotify;
        if self.settings.is_some() {
            out.data.push(DataRequest::AgentLog);
        }
        let wanted = match env.agent.notify {
            AgentNotify::Changes => true,
            AgentNotify::Sends => notice.send,
            AgentNotify::None => false,
        };
        if !wanted {
            return;
        }
        // Changes are gathered: each one takes the place of the one
        // before, so that forty moves are one notification. A message
        // sent is always said by itself.
        let id = if notice.send {
            self.agent_notes += 1;
            format!("{AGENT_NOTIFICATION}send.{}", self.agent_notes)
        } else {
            format!("{AGENT_NOTIFICATION}changes")
        };
        out.host.push(HostRequest::Notify {
            id,
            title: if notice.send {
                "An agent sent mail".to_owned()
            } else {
                "An agent changed your mail".to_owned()
            },
            subtitle: None,
            body: notice.text.clone(),
            actions: vec![("stop", "Stop Agents", false)],
        });
    }

    /// An agent's program connected over MCP. Always said, whatever the
    /// settings: it is how the user learns that something has come in.
    fn on_agent_connected(&mut self, env: &Env, name: &str, out: &mut Effects) {
        let said = if env.agent.mcp {
            format!("{name} connected to Quill over MCP.")
        } else {
            format!("{name} tried to connect over MCP, which is switched off in Settings, Agents.")
        };
        self.agent_notes += 1;
        out.host.push(HostRequest::Notify {
            id: format!("{AGENT_NOTIFICATION}connect.{}", self.agent_notes),
            title: "An agent connected".to_owned(),
            subtitle: None,
            body: said.clone(),
            actions: if env.agent.mcp {
                vec![("stop", "Stop Agents", false)]
            } else {
                Vec::new()
            },
        });
        self.notices.push_back(Notice::info(said));
    }

    /// Stops everything that works the mail from outside the window: both
    /// ways in are switched off, here at once and in the settings file,
    /// whoever is connected is hung up on, and mail they sent that has
    /// not gone is taken back to Drafts. What was done before is not
    /// undone: the log has it, and `quill undo` takes a change back.
    fn stop_agents(&mut self, env: &Env, out: &mut Effects) {
        if !(env.agent.commands || env.agent.mcp) {
            self.notices
                .push_back(Notice::info("Agents are stopped already."));
            return;
        }
        out.agents_stopped = true;
        out.accounts
            .push(AccountRequest::Remember(Preference::AgentsOff));
        out.data.push(DataRequest::AgentsStop);
        // The settings window shows the switches as they are now.
        if self.settings.is_some() {
            out.accounts.push(AccountRequest::Settings(None));
        }
        self.notices.push_back(Notice::info(
            "Agents are stopped: shell commands and MCP are switched off. Switch them on again \
             in Settings, Agents.",
        ));
    }

    fn on_notification(&mut self, env: &Env, id: &str, action: Option<&str>, out: &mut Effects) {
        let Some(message) = id
            .strip_prefix(MESSAGE_NOTIFICATION)
            .and_then(|number| number.parse().ok())
            .map(LocalMessageId)
        else {
            // One about agents: its button stops them, and a click on it
            // is a wish to see what they did.
            if id.starts_with(AGENT_NOTIFICATION) {
                match action {
                    Some("stop") => self.run(env, CommandId::AgentsStop, out),
                    _ => self.notices.push_back(Notice::info(
                        "What agents did is in Settings, Agents, and all of it in `quill log`.",
                    )),
                }
                return;
            }
            // The one about several messages: the inbox is where they are.
            if id == SEVERAL_NOTIFICATION && action.is_none() && self.compose.is_none() {
                self.show_mailbox(Mailbox::Unified, out);
                self.select_mailbox_row(Mailbox::Unified, out);
                self.set_focus(Pane::List);
            }
            return;
        };
        match action {
            Some("archive") => out.data.push(DataRequest::Archive(message)),
            Some("read") => out.data.push(DataRequest::SetSeen {
                id: message,
                seen: true,
            }),
            Some("reply") | None => {
                self.go_to = Some((message, action.is_some()));
                // The unified inbox has every account's new mail.
                if self.mailbox != Mailbox::Unified || self.search.is_some() {
                    self.show_mailbox(Mailbox::Unified, out);
                    self.select_mailbox_row(Mailbox::Unified, out);
                } else {
                    self.go_to_pending(env, out);
                }
            }
            Some(_) => {}
        }
    }

    /// Goes to the message a notification asked for, if the list has it.
    fn go_to_pending(&mut self, env: &Env, out: &mut Effects) {
        let Some((message, reply)) = self.go_to.take() else {
            return;
        };
        let Some(index) = self.list.rows.iter().position(|row| row.id == message) else {
            return;
        };
        // What is being written is not put aside for it.
        if self.compose.is_some() {
            self.list.selected = index;
            return;
        }
        self.help = None;
        self.palette = None;
        self.links = None;
        self.attachments = None;
        self.folders = None;
        self.list.selected = index;
        self.set_focus(Pane::List);
        self.open(out);
        if reply {
            self.run(env, CommandId::MessageReply, out);
        }
    }

    /// Tells the user about mail that has just arrived: each message by
    /// itself when there are a few, one line for them all when many.
    fn announce(&mut self, env: &Env, mut rows: Vec<MessageRow>, out: &mut Effects) {
        rows.iter_mut().for_each(sanitize_row);
        rows.retain(|row| !row.flags.seen);
        if !env.notifications || rows.is_empty() {
            return;
        }
        let who = |row: &MessageRow| {
            row.headers.from.first().map_or_else(
                || "Someone".to_owned(),
                |from| from.display_name().to_owned(),
            )
        };
        if rows.len() > MOST_ANNOUNCED_SINGLY {
            let mut people: Vec<String> = Vec::new();
            for row in &rows {
                let name = who(row);
                if !people.contains(&name) {
                    people.push(name);
                }
            }
            let named = people
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let body = if people.len() > 3 {
                format!("From {named} and others")
            } else {
                format!("From {named}")
            };
            out.host.push(HostRequest::Notify {
                id: SEVERAL_NOTIFICATION.to_owned(),
                title: format!("{} new messages", rows.len()),
                subtitle: None,
                body,
                actions: Vec::new(),
            });
            return;
        }
        for row in rows {
            // Which account, when there is more than one to tell apart.
            let subtitle = (self.accounts.len() > 1)
                .then(|| {
                    self.accounts
                        .iter()
                        .find(|account| account.id == row.account)
                })
                .flatten()
                .map(|account| account.name.clone());
            self.announced.insert(row.id);
            out.host.push(HostRequest::Notify {
                id: format!("{MESSAGE_NOTIFICATION}{}", row.id.0),
                title: who(&row),
                subtitle,
                body: if row.headers.subject.is_empty() {
                    "(no subject)".to_owned()
                } else {
                    row.headers.subject.clone()
                },
                actions: vec![
                    ("archive", "Archive", false),
                    ("read", "Mark as Read", false),
                    // A reply is written in the window.
                    ("reply", "Reply", true),
                ],
            });
        }
    }

    /// Takes down the notifications of messages the user has just dealt
    /// with: read, archived, deleted or moved.
    fn settle_notifications(&mut self, out: &mut Effects) {
        if self.announced.is_empty() {
            return;
        }
        let mut dealt_with = Vec::new();
        for request in &out.data {
            match request {
                DataRequest::SetSeen { id, seen: true }
                | DataRequest::Archive(id)
                | DataRequest::Delete(id)
                | DataRequest::MoveTo { id, .. } => dealt_with.push(*id),
                DataRequest::Many { ids, what } if !matches!(what, Bulk::Seen(false)) => {
                    dealt_with.extend(ids.iter().copied());
                }
                _ => {}
            }
        }
        let ids: Vec<String> = dealt_with
            .into_iter()
            .filter(|id| self.announced.remove(id))
            .map(|id| format!("{MESSAGE_NOTIFICATION}{}", id.0))
            .collect();
        if !ids.is_empty() {
            out.host.push(HostRequest::ClearNotifications(ids));
        }
    }

    /// Goes to a row of the sidebar: shows its mailbox, or runs its search.
    fn go_to_entry(&mut self, env: &Env, index: usize, out: &mut Effects) {
        let Some(entry) = self.sidebar.entries.get(index).cloned() else {
            return;
        };
        match entry {
            SidebarEntry::Search { query, .. } => {
                self.sidebar.selected = index;
                self.open_search(env, &query, out);
            }
            entry => {
                if let Some(mailbox) = entry.mailbox() {
                    self.sidebar.selected = index;
                    self.show_mailbox(mailbox, out);
                }
            }
        }
    }

    /// Shows what a search finds in the list, as if it had been typed and
    /// entered: a saved search, opened.
    fn open_search(&mut self, env: &Env, query: &str, out: &mut Effects) {
        self.search = Some(Search {
            input: LineInput::new(query),
            typing: false,
            found: None,
            more: false,
            rule: None,
        });
        self.list = MessageList::default();
        if let Some(request) = self.search_request(env) {
            out.data.push(request);
        }
    }

    /// The message whose page can be shown: the one in the reader, once it
    /// is here in full, when it is the one the user is on.
    fn page_message(&self, context: Context) -> Option<LocalMessageId> {
        let row = self.reader.row.as_ref()?;
        let on_it =
            context == Context::Reader || self.target_row().map(|row| row.id) == Some(row.id);
        (on_it && matches!(self.reader.body, Body::Loaded(_))).then_some(row.id)
    }

    /// Asks for the message's page, to show in the web reading pane or the
    /// browser. Whether it may load remote content is decided here, from
    /// what the user has said, and written into the page.
    fn show_page(&mut self, env: &Env, then: PageUse, out: &mut Effects) {
        let Some(message) = self.page_message(self.context()) else {
            return;
        };
        out.data.push(DataRequest::WebPage {
            message,
            remote: self.remote_content(env),
            // A page for the browser is light or dark as the browser is.
            dark: then == PageUse::Reading && self.pages_dark,
            then,
        });
        if then == PageUse::Reading {
            self.follow.dark = self.pages_dark;
        }
    }

    /// Whether the page of the message in the reader may load remote
    /// content, from what the user has said.
    fn remote_content(&self, env: &Env) -> RemoteContent {
        if env.remote_never {
            RemoteContent::Never
        } else if self.reader.remote {
            RemoteContent::Allowed
        } else {
            RemoteContent::IfSenderAllowed
        }
    }

    /// Where a message's page is shown once it may load its remote
    /// content: in the web reading pane while that is where mail is read,
    /// and otherwise in the browser. The reader drawn here fetches
    /// nothing, so it has nothing more to show.
    fn page_again(&self) -> PageUse {
        if self.web_reading {
            PageUse::Reading
        } else {
            PageUse::Browser
        }
    }

    /// Whether web reading is on: the host has a pane for it, and the
    /// config and the user's switching in this run come out at on.
    fn web_reading_wanted(&self, env: &Env) -> bool {
        env.web_pane && (env.web_reading != self.web_flipped)
    }

    /// Switches between reading mail in the host's web view pane and in
    /// the reader drawn here, and has the choice remembered.
    fn toggle_web_reading(&mut self, env: &Env, out: &mut Effects) {
        self.web_flipped = !self.web_flipped;
        self.web_reading = self.web_reading_wanted(env);
        out.accounts
            .push(AccountRequest::Remember(Preference::WebReading(
                self.web_reading,
            )));
        self.web_reading_switched(env, out);
    }

    /// Switches between showing mail in the web pane dark while Quill is
    /// dark and showing it in the colours it was sent with, and has the
    /// choice remembered. The page on show is made again by
    /// [`State::follow_selection`], which sees that it is not as asked.
    fn toggle_dark_pages(&mut self, env: &Env, out: &mut Effects) {
        self.dark_flipped = !self.dark_flipped;
        self.dark_pages = env.dark_pages != self.dark_flipped;
        self.pages_dark = self.dark_pages && env.look.is_dark();
        out.accounts
            .push(AccountRequest::Remember(Preference::DarkPages(
                self.dark_pages,
            )));
        let key = env
            .hint_key(CommandId::ViewDarkPages)
            .map_or_else(String::new, |key| format!(" {key} switches back."));
        let said = match (self.dark_pages, env.look.is_dark()) {
            (true, true) => "Messages are shown dark.",
            (true, false) => "Messages are shown dark while Quill is dark.",
            (false, _) => "Messages are shown in their own colours.",
        };
        self.flash = Some((format!("{said}{key}"), false));
    }

    /// Hides the accounts pane or puts it back, and has the choice
    /// remembered. Hidden, it still comes up over the other panes when
    /// the keys are taken to it, and goes when they leave.
    fn toggle_sidebar(&mut self, env: &Env, out: &mut Effects) {
        self.sidebar_flipped = !self.sidebar_flipped;
        self.sidebar_hidden = env.sidebar == self.sidebar_flipped;
        out.accounts
            .push(AccountRequest::Remember(Preference::Sidebar(
                !self.sidebar_hidden,
            )));
        if self.sidebar_hidden {
            // Hiding it is not a way to end up in it.
            if self.focus == Pane::Sidebar {
                self.focus = match self.main_pane {
                    MainPane::List => Pane::List,
                    MainPane::Reader => Pane::Reader,
                };
            }
            let peek = env
                .hint_key(CommandId::FocusPrev)
                .map_or_else(String::new, |key| format!(" {key} brings it up;"));
            let back = env
                .hint_key(CommandId::ViewSidebar)
                .map_or_else(String::new, |key| format!(" {key} puts it back."));
            self.flash = Some((format!("The accounts pane is hidden.{peek}{back}"), false));
        }
    }

    /// Web reading has just been switched, by its command or in the
    /// host's settings window: what follows from it either way.
    fn web_reading_switched(&mut self, env: &Env, out: &mut Effects) {
        let key = env
            .hint_key(CommandId::ViewWebReading)
            .map_or_else(String::new, |key| format!(" {key} switches back."));
        if self.web_reading {
            // Asked for, so shown at once; and not thereby read.
            self.follow = Follow {
                target: self.list.rows.get(self.list.selected).map(|row| row.id),
                settled: true,
                ..Follow::default()
            };
            // There is no reader here now to have the keys.
            if self.compose.is_none() {
                if self.focus == Pane::Reader {
                    self.focus = Pane::List;
                }
                self.main_pane = MainPane::List;
            }
            self.flash = Some((format!("Messages are shown as web pages.{key}"), false));
        } else {
            self.follow = Follow::default();
            out.host.push(HostRequest::CloseReading);
            // Laid out again, for the reader that is drawn here now.
            self.reader.wrap_width = None;
            self.flash = Some((format!("Messages are shown as text.{key}"), false));
        }
    }

    /// Where the editor that has the text of the message being written
    /// goes, for a host that lays it over the terminal, and whether it
    /// has the keys. It is on show only while nothing of Quill's is
    /// drawn over the message's pane: a list to choose from, the help or
    /// a question takes its place, and the keys with it.
    pub fn editor_spot(&self, env: &Env) -> EditorSpot {
        let Some(compose) = self
            .compose
            .as_ref()
            .filter(|compose| compose.editor_in_place())
        else {
            return EditorSpot::default();
        };
        if self.context() != Context::Compose {
            return EditorSpot::default();
        }
        let areas = layout::compute(self, self.size, env.reader_width);
        let Some(pane) = areas.reader else {
            return EditorSpot::default();
        };
        let Some(place) = view::compose::editor_rect(pane, compose, env, self.editor_tips) else {
            return EditorSpot::default();
        };
        EditorSpot {
            place: Some(place),
            covered: view::compose::rows_over(pane, compose, place),
            keys: compose.field == Field::Body && compose.attaching.is_none() && !compose.sending,
        }
    }

    /// How many columns the terminal keeps beside the host's reading
    /// pane, for the host to put the pane by: the accounts' and the
    /// list's, so that the pane has the reader's place exactly. `None`
    /// while web reading is off or the host has not said how wide its
    /// window is.
    ///
    /// By the widths in force, as the panes are drawn by: an edge dragged
    /// in this run counts, and so do the accounts' own columns.
    pub fn reading_place(&self, env: &Env) -> Option<u16> {
        let window = self.window_columns.filter(|_| self.web_reading)?;
        Some(layout::beside_reading(
            window,
            layout::Split::of(self, env.reader_width),
            !self.sidebar_hidden,
        ))
    }

    /// Whether the terminal is about to be given a different width by the
    /// host: web reading is on and the terminal still has the whole
    /// window, or is off and the terminal is still short of it. The run
    /// loop waits a moment for the new width before it draws, so that
    /// nothing is drawn at the old one in an arrangement meant for the
    /// new.
    pub fn resize_expected(&self) -> bool {
        self.window_columns
            .is_some_and(|window| self.web_reading == (self.size.0 >= window))
    }

    /// The count of the cursor's moves, while the web reading pane is
    /// waiting for the cursor to rest: the run loop sends
    /// [`Action::SelectionSettled`] once this has stayed the same for a
    /// moment. `None` when nothing is waiting.
    pub fn settle_due(&self) -> Option<u64> {
        (self.web_reading && !self.follow.settled).then_some(self.follow.moves)
    }

    /// What the web reading pane was last asked to show.
    pub fn reading_asked(&self) -> Option<&Shown> {
        self.follow.asked.as_ref()
    }

    /// What the web reading pane should show for the message under the
    /// list's cursor, or `None` while that is not known yet: the list or
    /// the message is still being read from the store.
    fn reading_wanted(&self) -> Option<Shown> {
        let Some(target) = self.follow.target else {
            return self.list.loaded.then(|| Shown::Note(NO_MESSAGE.to_owned()));
        };
        self.reader.row.as_ref().filter(|row| row.id == target)?;
        if let Some(why) = &self.follow.failed {
            return Some(Shown::Note(why.clone()));
        }
        match &self.reader.body {
            Body::None | Body::Loading => None,
            Body::Downloading => Some(Shown::Note(DOWNLOADING.to_owned())),
            Body::Unavailable(why) => Some(Shown::Note(why.clone())),
            // A page is made from the message as it arrived. A draft never
            // arrived, and mail whose copy was let go is shown as its text
            // until the copy is back.
            Body::Loaded(body)
                if body.copy_missing || DraftId::from_message_id(target).is_some() =>
            {
                Some(Shown::Text(target))
            }
            Body::Loaded(_) => Some(Shown::Page(target)),
        }
    }

    /// Asks for the mail around the list's cursor to be downloaded, when
    /// what is around it has changed: the message under it, then those
    /// after it and a few before, nearest first. Whatever list is on show
    /// and however old its mail, the message opened next is then here
    /// already. Run at the end of every update, like the reading pane's
    /// following; the engine waits for the cursor to rest, and knows
    /// which of them it has.
    fn ask_for_mail_nearby(&mut self, out: &mut Effects) {
        if !self.list.loaded {
            return;
        }
        let rows = &self.list.rows;
        let at = self.list.selected;
        let mut near: Vec<LocalMessageId> = rows.get(at).map(|row| row.id).into_iter().collect();
        for step in 1..=NEAR_AFTER {
            near.extend(rows.get(at + step).map(|row| row.id));
            if step <= NEAR_BEFORE {
                let before = at.checked_sub(step).and_then(|at| rows.get(at));
                near.extend(before.map(|row| row.id));
            }
        }
        if near.is_empty() || near == self.near_asked {
            return;
        }
        self.near_asked.clone_from(&near);
        out.near = Some(near);
    }

    /// Keeps the web reading pane on the message under the list's cursor.
    /// Run at the end of every update: whatever moved the cursor, the
    /// pane follows it, a moment after it comes to rest.
    fn follow_selection(&mut self, env: &Env, out: &mut Effects) {
        let went = std::mem::take(&mut self.follow.went);
        let now = std::mem::take(&mut self.follow.now);
        if !self.web_reading {
            return;
        }
        // However a message being written was put away, the keys are not
        // left with a reader that is not drawn.
        if self.focus == Pane::Reader && self.compose.is_none() {
            self.focus = Pane::List;
            self.main_pane = MainPane::List;
        }
        let target = self.list.rows.get(self.list.selected).map(|row| row.id);
        if target != self.follow.target {
            self.follow.target = target;
            self.follow.moves += 1;
            self.follow.settled = false;
            self.follow.by_user = went;
            self.follow.failed = None;
        } else if went {
            self.follow.by_user = true;
        }
        if now {
            self.follow.settled = true;
        }
        if !self.follow.settled {
            return;
        }
        let Some(wanted) = self.reading_wanted() else {
            return;
        };
        // Only a message's page is made differently for a dark window:
        // Quill's own words are light or dark by themselves.
        let dark = self.pages_dark && matches!(wanted, Shown::Page(_));
        if self.follow.asked.as_ref() == Some(&wanted)
            && (self.follow.dark == dark || !matches!(wanted, Shown::Page(_)))
        {
            return;
        }
        self.follow.dark = dark;
        match &wanted {
            Shown::Page(message) => out.data.push(DataRequest::WebPage {
                message: *message,
                remote: self.remote_content(env),
                dark,
                then: PageUse::Reading,
            }),
            Shown::Text(message) => {
                if let (Some(row), Body::Loaded(body)) = (&self.reader.row, &self.reader.body) {
                    out.data.push(DataRequest::TextPage {
                        message: *message,
                        title: row.headers.subject.clone(),
                        text: body.text.clone(),
                    });
                }
            }
            Shown::Note(text) => out.data.push(DataRequest::NotePage { text: text.clone() }),
        }
        self.follow.asked = Some(wanted);
    }

    /// What the web reading pane says above the message in the reader:
    /// who it is from and to, when it was sent, and the files it carries.
    /// Plain lines, already cleaned as everything from a message is.
    fn reading_header(&self, env: &Env) -> Vec<String> {
        let Some(row) = &self.reader.row else {
            return Vec::new();
        };
        let join = |addresses: &[quill_core::types::Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = Vec::new();
        for (label, addresses) in [
            ("From", &row.headers.from),
            ("To", &row.headers.to),
            ("Cc", &row.headers.cc),
        ] {
            if !addresses.is_empty() {
                lines.push(format!("{label}: {}", join(addresses)));
            }
        }
        lines.push(text::long_date(row.headers.date, &env.tz));
        let files = self.reader.files();
        if !files.is_empty() {
            let key = env
                .hint_key(CommandId::MessageAttachments)
                .map_or_else(String::new, |key| format!("  ({key} lists them)"));
            let named = files
                .iter()
                .map(|file| format!("{} ({})", file.name, text::file_size(file.size)))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Attached: {named}{key}"));
        }
        lines
            .iter()
            .map(|line| text::sanitize_line(line))
            .map(|line| text::truncate(&line, READING_HEADER_COLUMNS))
            .filter(|line| !line.is_empty())
            .collect()
    }

    /// The buttons beside the words that say remote pictures were left
    /// out of a page: load them for this message, or always for its
    /// sender. None where the user has said never to load them.
    fn remote_actions(env: &Env) -> Vec<(&'static str, &'static str)> {
        if env.remote_never {
            return Vec::new();
        }
        [
            (CommandId::MessageLoadRemote, "Load Pictures"),
            (CommandId::SenderAlwaysRemote, "Always from This Sender"),
        ]
        .into_iter()
        .filter_map(|(id, title)| Some((env.registry.get(id)?.name, title)))
        .collect()
    }

    /// Puts the page of the message under the cursor in the web reading
    /// pane. A message the user went to is read once it is shown.
    fn show_in_reading_pane(
        &mut self,
        env: &Env,
        message: LocalMessageId,
        page: crate::files::WebPage,
        notice: Option<PageNotice>,
        out: &mut Effects,
    ) {
        out.host.push(HostRequest::Reading {
            path: page.path,
            title: text::sanitize_line(&page.title),
            remote: page.remote,
            header: self.reading_header(env),
            notice,
        });
        // Opened, as far as being read goes: the same rule as `enter` in
        // the list with the reader drawn here.
        let unread = self
            .list
            .rows
            .get(self.list.selected)
            .is_some_and(|row| row.id == message && !row.flags.seen);
        // While several are selected, moving among them is choosing, not
        // reading.
        if std::mem::take(&mut self.follow.by_user)
            && unread
            && !self.list.has_marks()
            && DraftId::from_message_id(message).is_none()
        {
            self.set_seen_locally(message, true);
            out.data.push(DataRequest::SetSeen {
                id: message,
                seen: true,
            });
        }
    }

    /// The messages a filing or marking command acts on: those selected
    /// in the list, when there are any and the list is where the user is;
    /// otherwise the one [`State::target_row`] names.
    fn targets(&self) -> Vec<LocalMessageId> {
        if self.focus != Pane::Reader && self.list.has_marks() {
            self.list.marked_ids()
        } else {
            self.target_row().map(|row| row.id).into_iter().collect()
        }
    }

    /// Marks every selected message the same way: all read if any is
    /// unread, otherwise all unread; and likewise for stars. Returns
    /// whether there were several to mark.
    fn mark_selected(&mut self, star: bool, out: &mut Effects) -> bool {
        let ids = self.targets();
        if ids.len() < 2 {
            return false;
        }
        let wanted: BTreeSet<LocalMessageId> = ids.iter().copied().collect();
        let is_on = |row: &MessageRow| {
            if star {
                row.flags.flagged
            } else {
                row.flags.seen
            }
        };
        let on = !self
            .list
            .rows
            .iter()
            .filter(|row| wanted.contains(&row.id))
            .all(is_on);
        for row in self
            .list
            .rows
            .iter_mut()
            .chain(self.reader.row.as_mut())
            .filter(|row| wanted.contains(&row.id))
        {
            if star {
                row.flags.flagged = on;
            } else {
                row.flags.seen = on;
            }
        }
        out.data.push(DataRequest::Many {
            ids,
            what: if star {
                Bulk::Flagged(on)
            } else {
                Bulk::Seen(on)
            },
        });
        true
    }

    fn toggle_unread(&mut self, out: &mut Effects) {
        if self.mark_selected(false, out) {
            return;
        }
        let Some(row) = self.target_row() else {
            return;
        };
        let (id, previous) = (row.id, row.flags);
        let seen = !previous.seen;
        self.set_seen_locally(id, seen);
        out.data.push(DataRequest::SetSeen { id, seen });
    }

    fn toggle_star(&mut self, out: &mut Effects) {
        if self.mark_selected(true, out) {
            return;
        }
        let Some(row) = self.target_row() else {
            return;
        };
        let (id, previous) = (row.id, row.flags);
        let flagged = !previous.flagged;
        for row in self.rows_with_id(id) {
            row.flags.flagged = flagged;
        }
        out.data.push(DataRequest::SetFlagged { id, flagged });
    }

    /// Shows the change at once; the store's answer follows and agrees.
    fn set_seen_locally(&mut self, id: LocalMessageId, seen: bool) {
        for row in self.rows_with_id(id) {
            row.flags.seen = seen;
        }
    }

    fn rows_with_id(&mut self, id: LocalMessageId) -> impl Iterator<Item = &mut MessageRow> {
        self.list
            .rows
            .iter_mut()
            .chain(self.reader.row.as_mut())
            .filter(move |row| row.id == id)
    }

    /// Points the reader at the message under the list cursor.
    fn sync_reader(&mut self, out: &mut Effects) {
        match self.list.rows.get(self.list.selected) {
            Some(row)
                if self
                    .reader
                    .row
                    .as_ref()
                    .is_some_and(|open| open.id == row.id) =>
            {
                // Same message: pick up any flag changes, keep the scroll.
                self.reader.row = Some(row.clone());
            }
            Some(row) => {
                out.data.push(DataRequest::LoadBody(row.id));
                self.reader = Reader {
                    row: Some(row.clone()),
                    body: Body::Loading,
                    ..Reader::default()
                };
                // The links on offer were the last message's.
                self.links = None;
            }
            None => {
                self.reader = Reader::default();
                self.links = None;
            }
        }
    }

    fn on_sync(&mut self, env: &Env, event: SyncEvent, out: &mut Effects) {
        let reading = |state: &Self, id| state.reader.row.as_ref().is_some_and(|row| row.id == id);
        match event {
            SyncEvent::Changed => self.reload(env, out),
            SyncEvent::Syncing { account, active } => {
                if active {
                    self.syncing.insert(account);
                } else {
                    self.syncing.remove(&account);
                }
            }
            SyncEvent::Certificate {
                account,
                problem,
                sending,
            } => {
                // One question per server: a newer report replaces an
                // unanswered older one.
                self.certificates
                    .retain(|prompt| prompt.account != account || prompt.sending != sending);
                self.certificates.push_back(CertificatePrompt {
                    account,
                    problem,
                    sending,
                    busy: false,
                });
            }
            SyncEvent::Sent { .. } => self.flash = Some(("Sent.".to_owned(), false)),
            SyncEvent::Ruled { said } => {
                self.flash = Some((format!("{}.", text::sanitize_line(&said)), false));
            }
            SyncEvent::RulesWaiting { account, count } => self.on_rules_waiting(&account, count),
            SyncEvent::FolderMade { account, name } => {
                self.notices.push_back(Notice::info(format!(
                    "Made the folder {} on {}'s server.",
                    text::sanitize_line(&name),
                    text::sanitize_line(&account)
                )));
            }
            SyncEvent::Arrived { messages, .. } => {
                if env.notifications {
                    out.data.push(DataRequest::Announce(messages));
                }
            }
            SyncEvent::BodyReady(id) => {
                if reading(self, id) {
                    out.data.push(DataRequest::LoadBody(id));
                }
                if let Some((_, request)) =
                    self.respond_pending.take_if(|(waiting, _)| *waiting == id)
                {
                    out.data.push(request);
                }
            }
            SyncEvent::BodyFailed { id, reason } => {
                if self
                    .respond_pending
                    .take_if(|(waiting, _)| *waiting == id)
                    .is_some()
                {
                    self.flash = Some((
                        text::sanitize_line(&format!(
                            "That message could not be downloaded: {reason}"
                        )),
                        false,
                    ));
                }
                if reading(self, id) {
                    if matches!(self.reader.body, Body::Loaded(_)) {
                        // Its text is here and stays on show. Only the
                        // rest could not be had.
                        if self.reader.rest == Rest::Fetching {
                            self.reader.rest = Rest::Failed(text::sanitize_line(&reason));
                        }
                    } else {
                        self.reader.body = Body::Unavailable(text::sanitize_line(&reason));
                        self.reader.wrap_width = None;
                    }
                }
            }
            SyncEvent::Problem { account, message } => {
                // An account the form added a moment ago says at once that
                // nobody has signed in to it. They are doing just that, and
                // the sign-in will say how it went.
                if self
                    .sign_in
                    .as_ref()
                    .is_some_and(|sign_in| sign_in.fresh && sign_in.account == account)
                {
                    return;
                }
                self.notices
                    .push_back(Notice::error(text::sanitize_line(&format!(
                        "{account}: {message}"
                    ))));
            }
        }
    }

    /// Asks the store again for everything on screen that it holds.
    fn reload(&self, env: &Env, out: &mut Effects) {
        out.data.push(DataRequest::LoadSidebar);
        match self.search_request(env) {
            // While searching the list is what was found, and what was
            // found may have changed too.
            Some(request) if self.search.is_some() => out.data.push(request),
            _ if self.search.is_some() => {}
            _ => out.data.push(DataRequest::LoadMessages(self.mailbox)),
        }
    }

    /// What is typed in the search box, if there is one.
    fn search_text(&self) -> Option<String> {
        self.search
            .as_ref()
            .map(|search| search.input.text().to_owned())
    }

    /// The request that finds what is typed, or `None` when nothing is.
    fn search_request(&self, env: &Env) -> Option<DataRequest> {
        let text = self.search_text()?;
        // A rule being tried is in the list as a search is, and is asked
        // of the rules, not of the search.
        if let Some(trial) = self.rule_trial() {
            return Some(DataRequest::RulesTry {
                text,
                set: std::sync::Arc::clone(&self.rules),
                which: trial.which.clone(),
            });
        }
        let tz = env.tz.clone();
        let query = quill_core::search::parse(&text, |year, month, day| {
            let date = jiff::civil::Date::new(
                i16::try_from(year).ok()?,
                i8::try_from(month).ok()?,
                i8::try_from(day).ok()?,
            )
            .ok()?;
            Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_second())
        });
        (!query.is_empty()).then(|| DataRequest::Search {
            text,
            query: Box::new(query),
        })
    }

    /// Puts what a search found, or what a rule would change, in the
    /// list.
    fn show_found(&mut self, mut rows: Vec<MessageRow>, more: bool, out: &mut Effects) {
        rows.iter_mut().for_each(sanitize_row);
        // The message under the cursor stays under it if it is
        // still found; otherwise the newest is.
        let keep = self.list.rows.get(self.list.selected).map(|row| row.id);
        self.list.selected = keep
            .and_then(|id| rows.iter().position(|row| row.id == id))
            .unwrap_or(0);
        self.list.offset = 0;
        if let Some(search) = &mut self.search {
            search.found = Some(rows.len());
            search.more = more;
        }
        // What a search finds is a list of messages, not of
        // conversations.
        self.list.follows.clear();
        self.list.rows = rows;
        self.list.loaded = true;
        self.list.keep_listed_marks();
        self.sync_reader(out);
    }

    /// Asks for what is typed to be found. With nothing typed there is
    /// nothing found, and nothing to ask.
    fn run_search(&mut self, env: &Env, out: &mut Effects) {
        match self.search_request(env) {
            Some(request) => out.data.push(request),
            None => {
                self.list = MessageList::default();
                self.list.loaded = true;
                if let Some(search) = &mut self.search {
                    search.found = None;
                    search.more = false;
                }
                self.sync_reader(out);
            }
        }
    }

    fn on_data(&mut self, env: &Env, event: DataEvent, out: &mut Effects) {
        match event {
            // The answer to a request from outside the window is the run
            // loop's to give: it never comes this far.
            DataEvent::AgentDone { .. } => {}
            DataEvent::AgentLog(entries) => {
                if self.agent_recent != entries {
                    self.agent_recent = entries;
                    // The settings window shows them, if it is open.
                    self.settings_answered(None);
                }
            }
            DataEvent::SidebarLoaded {
                accounts,
                folders,
                drafts,
                outbox,
                snoozed: (snoozed, next_wake),
                closed,
            } => {
                self.next_wake = next_wake;
                // How the accounts were left is read once, as Quill
                // starts. From then on they are opened and closed here and
                // the store is told: an answer that was on its way while
                // one was closed must not open it again.
                if !self.accounts_known {
                    self.sidebar.closed = closed.into_iter().collect();
                }
                self.sidebar
                    .closed
                    .retain(|id| accounts.iter().any(|account| account.id == *id));
                self.sidebar.entries =
                    sidebar_entries(&accounts, &folders, [drafts, outbox, snoozed], self.mailbox);
                if !self.sidebar.entries.is_empty() && !env.searches.is_empty() {
                    self.sidebar.entries.push(SidebarEntry::Searches);
                    self.sidebar
                        .entries
                        .extend(
                            env.searches
                                .iter()
                                .map(|(name, query)| SidebarEntry::Search {
                                    name: text::sanitize_line(name),
                                    query: query.clone(),
                                }),
                        );
                }
                self.accounts = accounts;
                self.accounts_known = true;
                // A saved search that is on show keeps the cursor.
                let searching = self.search_text();
                let on_search = self.sidebar.entries.iter().position(|entry| {
                    matches!(entry, SidebarEntry::Search { query, .. }
                        if Some(query) == searching.as_ref())
                });
                match on_search.or_else(|| {
                    self.sidebar
                        .entries
                        .iter()
                        .position(|entry| entry.mailbox() == Some(self.mailbox))
                }) {
                    Some(index) => self.select_row(index, out),
                    None => {
                        // The folder being shown is gone.
                        self.sidebar.selected = 0;
                        self.show_mailbox(Mailbox::Unified, out);
                    }
                }
            }
            DataEvent::SearchLoaded { text, rows, more } => {
                // An answer for something typed over since is dropped,
                // and so is one that comes while a rule has the list.
                if self.search_text().as_deref() != Some(text.as_str())
                    || self.rule_trial().is_some()
                {
                    return;
                }
                self.show_found(rows, more, out);
            }
            DataEvent::RulesTried {
                text,
                rows,
                more,
                says,
                count,
            } => {
                if self.search_text().as_deref() != Some(text.as_str()) {
                    return;
                }
                match self.search.as_mut().and_then(|search| search.rule.as_mut()) {
                    Some(trial) => {
                        trial.says = Some(text::sanitize_line(&says));
                        trial.count = count;
                    }
                    // The rule was left before the answer came.
                    None => return,
                }
                self.show_found(rows, more, out);
            }
            DataEvent::RulesRan {
                said,
                problems,
                undoable,
            } => self.on_rules_ran(&said, problems, undoable),
            DataEvent::RulesLog(entries) => self.on_rules_log(env, entries),
            DataEvent::MessagesLoaded { mailbox, mut rows } => {
                if mailbox != self.mailbox || self.search.is_some() {
                    // An answer for a mailbox the user has already left,
                    // or one that arrives while a search has the list.
                    return;
                }
                rows.iter_mut().for_each(sanitize_row);
                let (rows, follows) = if env.threads != self.ungrouped {
                    by_conversation(rows)
                } else {
                    (rows, Vec::new())
                };
                self.list.follows = follows;
                let keep = self
                    .list
                    .loaded
                    .then(|| self.list.rows.get(self.list.selected).map(|row| row.id))
                    .flatten();
                let kept = keep.and_then(|id| rows.iter().position(|row| row.id == id));
                // The message under the cursor has gone, filed or deleted,
                // and its neighbour is what the user is on now: gone to,
                // as far as the web reading pane goes.
                self.follow.went |= keep.is_some() && kept.is_none();
                self.list.selected = kept.unwrap_or(if self.list.loaded {
                    self.list.selected.min(rows.len().saturating_sub(1))
                } else {
                    0
                });
                self.list.rows = rows;
                self.list.loaded = true;
                self.list.keep_listed_marks();
                self.sync_reader(out);
                // Where a notification asked to go, now that the list is
                // here to go in.
                self.go_to_pending(env, out);
            }
            DataEvent::BodyLoaded { id, body } => {
                if self.reader.row.as_ref().is_some_and(|row| row.id == id) {
                    // Read afresh: a page that could not be made of it
                    // before may be made now.
                    self.follow.failed = None;
                    self.reader.body = match body {
                        Some(body) => {
                            self.reader.rest = match (body.copy_missing, &self.reader.rest) {
                                (false, _) => Rest::Here,
                                // The text is here and the downloaded copy
                                // was let go. The text is shown while the
                                // copy is fetched again: once for each
                                // time the message is opened, never more.
                                (true, Rest::Here) => {
                                    out.sync.push(SyncRequest::FetchBody(id));
                                    Rest::Fetching
                                }
                                // Fetched, and still no copy to read.
                                (true, Rest::Fetching) => Rest::Failed(
                                    "it was downloaded and could not be kept on this Mac."
                                        .to_owned(),
                                ),
                                (true, Rest::Failed(why)) => Rest::Failed(why.clone()),
                            };
                            Body::Loaded(LoadedBody {
                                text: text::sanitize_body(body.text.trim_end()),
                                ..body
                            })
                        }
                        None => {
                            // Not downloaded yet. Ask for it; the reader
                            // hears when it lands or why it cannot.
                            out.sync.push(SyncRequest::FetchBody(id));
                            self.reader.rest = Rest::Fetching;
                            Body::Downloading
                        }
                    };
                    self.reader.wrap_width = None;
                    self.reader.card = self.invite_card(env);
                    // The first picture among the files is drawn above
                    // the text, where the terminal can draw one.
                    self.reader.picture = None;
                    let first = self
                        .reader
                        .files()
                        .iter()
                        .find(|file| is_picturable(&file.content_type, env.thumbnails))
                        .filter(|_| env.images)
                        .map(|file| (file.part, file.name.clone()));
                    if let Some((part, name)) = first {
                        self.reader.picture = Some(Picture {
                            id: picture_id(id, part),
                            part,
                            name,
                            size: None,
                        });
                        out.data.push(DataRequest::Extract {
                            message: id,
                            part,
                            then: FileUse::Picture,
                        });
                    }
                }
            }
            DataEvent::StoreChanged => self.reload(env, out),
            // The change is written down: the engine can take it from there.
            DataEvent::Queued => out.sync.push(SyncRequest::Queue),
            DataEvent::Flash { text, undoable } => self.flash = Some((text, undoable)),
            DataEvent::Contacts { typed, people } => {
                if let Some(compose) = &mut self.compose {
                    compose.offer(typed, people);
                }
            }
            DataEvent::DraftSaved { draft } => {
                if let Some(compose) = &mut self.compose {
                    compose.mark_saved(draft);
                }
            }
            DataEvent::SendQueued => {
                self.compose = None;
                self.set_focus(Pane::List);
            }
            DataEvent::SendDoubts(doubts) => {
                if let Some(compose) = &mut self.compose {
                    compose.sending = false;
                    compose.doubts = doubts;
                }
            }
            DataEvent::SendRefused(reason) => {
                if let Some(compose) = &mut self.compose {
                    compose.sending = false;
                    compose.error = Some(text::sanitize_line(&reason));
                }
            }
            DataEvent::Attached(attachment) => {
                if let Some(compose) = &mut self.compose {
                    compose.attach(attachment);
                }
            }
            DataEvent::AttachFailed(reason) => {
                if let Some(compose) = &mut self.compose {
                    compose.error = Some(text::sanitize_line(&reason));
                }
                if let Some(after) = &mut self.after_vim {
                    after.attach_failed = true;
                }
            }
            DataEvent::EditReady {
                dir,
                path,
                body_line,
            } => self.on_edit_ready(env, dir, path, body_line, out),
            DataEvent::EditFailed(reason) => {
                self.vim = None;
                if let Some(compose) = &mut self.compose
                    && compose.away.take().is_some()
                {
                    compose.error = Some(text::sanitize_line(&reason));
                }
            }
            DataEvent::EditBack { file, verdict } => self.on_edit_back(*file, verdict, out),
            DataEvent::EditUnread(reason) => {
                // Nothing came back: the message is as it was before Vim.
                self.vim = None;
                if let Some(compose) = &mut self.compose
                    && compose.away.take().is_some()
                {
                    compose.error = Some(text::sanitize_line(&reason));
                }
            }
            DataEvent::Settled => self.after_vim(env, out),
            DataEvent::HookReady(run) => {
                // Asked for by a key a moment ago, and still not something
                // Quill's own app ever does.
                if env.hooks_enabled {
                    out.host.push(HostRequest::RunHook(run));
                } else {
                    self.hook_running = None;
                }
            }
            DataEvent::HookUnread(reason) => {
                self.hook_running = None;
                self.flash = Some((text::sanitize_line(&reason), false));
            }
            DataEvent::ComposeReady(prefill) => self.compose_pending = Some(prefill),
            DataEvent::Extracted { files, index, then } => {
                let Some(chosen) = files.get(index).cloned() else {
                    return;
                };
                match then {
                    // Sound and video are played; everything else is
                    // looked at, with the other files a step away.
                    FileUse::View if is_playable(&chosen.content_type) => {
                        out.host.push(HostRequest::Play {
                            path: chosen.path,
                            title: chosen.name,
                        });
                    }
                    FileUse::View => {
                        let shown: Vec<&ExtractedFile> = files
                            .iter()
                            .filter(|file| !is_playable(&file.content_type))
                            .collect();
                        let index = shown
                            .iter()
                            .position(|file| file.path == chosen.path)
                            .unwrap_or(0);
                        out.host.push(HostRequest::Preview {
                            files: shown
                                .into_iter()
                                .map(|file| (file.path.clone(), file.name.clone()))
                                .collect(),
                            index,
                        });
                    }
                    FileUse::Picture => {
                        // Still the picture of the message on show?
                        if let Some(picture) = &self.reader.picture
                            && picture.name == chosen.name
                        {
                            out.host.push(HostRequest::LoadPicture {
                                id: picture.id,
                                path: chosen.path,
                                content_type: chosen.content_type,
                            });
                        }
                    }
                    FileUse::Open => out.host.push(HostRequest::OpenFile(chosen.path)),
                    FileUse::Save => out.host.push(HostRequest::SaveFile {
                        path: chosen.path,
                        name: chosen.name,
                    }),
                }
            }
            DataEvent::RespondNeedsBody(request) => {
                if let DataRequest::Respond { id, .. } = *request {
                    self.respond_pending = Some((id, *request));
                    out.sync.push(SyncRequest::FetchBody(id));
                    self.flash = Some(("Downloading the message…".to_owned(), false));
                }
            }
            DataEvent::Arrived(rows) => self.announce(env, rows, out),
            DataEvent::Snoozed { count, until } => {
                let back = text::long_date(until, &env.tz);
                self.flash = Some((
                    if count == 1 {
                        format!("Snoozed. It comes back {back}.")
                    } else {
                        format!("Snoozed {count} messages. They come back {back}.")
                    },
                    false,
                ));
            }
            DataEvent::PageReady {
                message,
                page,
                then,
            } => {
                // A page for a message the user has left is not shown.
                if self.reader.row.as_ref().map(|row| row.id) != Some(message) {
                    return;
                }
                // The reading pane's page is only shown while it is the
                // one being waited for.
                if then == PageUse::Reading
                    && (!self.web_reading
                        || self.follow.target != Some(message)
                        || !matches!(
                            self.follow.asked,
                            Some(Shown::Page(asked) | Shown::Text(asked)) if asked == message
                        ))
                {
                    return;
                }
                // Remote pictures were left out: say so, and how to have
                // them. The reading pane says it in a bar above the page,
                // with buttons; a page for the browser has only the
                // status line to say it in.
                let left_out = (page.blocked > 0 && !page.remote).then(|| {
                    if page.blocked == 1 {
                        "1 remote picture was left out".to_owned()
                    } else {
                        format!("{} remote pictures were left out", page.blocked)
                    }
                });
                match then {
                    PageUse::Browser => {
                        if let Some(pictures) = left_out {
                            let how = match (
                                env.remote_never,
                                env.hint_key(CommandId::MessageLoadRemote),
                                env.hint_key(CommandId::SenderAlwaysRemote),
                            ) {
                                (false, Some(once), Some(always)) => {
                                    format!(". {once} loads them, {always} always for this sender")
                                }
                                _ => String::new(),
                            };
                            self.flash = Some((format!("{pictures}{how}."), false));
                        }
                        out.host.push(HostRequest::OpenFile(page.path));
                    }
                    PageUse::Reading => {
                        let notice = left_out.map(|pictures| PageNotice {
                            text: format!("{pictures}."),
                            actions: Self::remote_actions(env),
                        });
                        self.show_in_reading_pane(env, message, page, notice, out);
                    }
                }
            }
            DataEvent::PageFailed { message, problem } => {
                // Still the message the pane is waiting for: the reason
                // is shown in its place, not the message before it. A
                // message shown as its text stays as it is: there was
                // never a page to be had of it.
                if self.web_reading && self.follow.asked == Some(Shown::Page(message)) {
                    let problem = text::sanitize_line(&problem);
                    self.follow.failed = Some(problem.clone());
                    self.follow.asked = Some(Shown::Note(problem.clone()));
                    out.data.push(DataRequest::NotePage { text: problem });
                }
            }
            DataEvent::NoteReady { text, page } => {
                if self.web_reading && self.follow.asked == Some(Shown::Note(text)) {
                    out.host.push(HostRequest::Reading {
                        path: page.path,
                        title: String::new(),
                        remote: false,
                        header: Vec::new(),
                        notice: None,
                    });
                }
            }
            DataEvent::Error(message) => self.notices.push_back(Notice::error(message)),
        }
    }

    /// Re-derives everything that depends on pane sizes: scroll offsets that
    /// keep the selection visible, the reader's wrapped lines, and clamps.
    fn relayout(&mut self, env: &Env) {
        let areas = layout::compute(self, self.size, env.reader_width);
        if let Some(rect) = areas.sidebar {
            // Counted among the rows on show: a closed account's folders
            // take no room.
            let shown = self.sidebar.shown();
            let selected = shown
                .iter()
                .position(|index| *index == self.sidebar.selected)
                .unwrap_or(0);
            keep_visible(
                &mut self.sidebar.offset,
                selected,
                layout::inner_height(rect),
                shown.len(),
            );
        }
        if let Some(rect) = areas.list {
            keep_visible(
                &mut self.list.offset,
                self.list.selected,
                layout::inner_height(rect),
                self.list.rows.len(),
            );
        }
        if let Some(rect) = areas.reader {
            let width = layout::reader_text_width(rect);
            self.reader.note = match self.reader_note() {
                Some(note) => {
                    let mut lines = text::wrap(&note, width);
                    if lines.len() > READER_NOTE_ROWS {
                        lines.truncate(READER_NOTE_ROWS);
                        if let Some(last) = lines.last_mut() {
                            *last = text::truncate(&format!("{last}…"), width);
                        }
                    }
                    lines
                }
                None => Vec::new(),
            };
            if self.reader.wrap_width != Some(width) {
                let (lines, links) = match (&self.reader.body, self.reader.showing()) {
                    (Body::Loaded(body), Some(showing)) => body_lines(body, showing, width),
                    _ => (Vec::new(), Vec::new()),
                };
                self.reader.lines = lines;
                self.reader.links = links;
                self.reader.wrap_width = Some(width);
                // A selection names cells of the lines as they were.
                self.reader.selection = None;
                self.held = None;
            }
            let viewport = layout::reader_body_height(
                rect,
                self.reader_header_rows(),
                self.reader_image_rows(env, rect),
            );
            let last = self.reader.lines.len().saturating_sub(viewport);
            self.reader.scroll = self.reader.scroll.min(last);
        } else if self.web_reading && self.reader.wrap_width != Some(UNDRAWN_WIDTH) {
            // No reader is drawn, and the message's links are still to be
            // had from the list: it is laid out all the same, to find them.
            let (lines, links) = match (&self.reader.body, self.reader.showing()) {
                (Body::Loaded(body), Some(showing)) => body_lines(body, showing, UNDRAWN_WIDTH),
                _ => (Vec::new(), Vec::new()),
            };
            self.reader.lines = lines;
            self.reader.links = links;
            self.reader.wrap_width = Some(UNDRAWN_WIDTH);
        }
        if let (Some(compose), Some(rect)) = (&mut self.compose, areas.reader) {
            let body = layout::compose_body(rect, compose);
            compose.body_view = compose
                .body
                .view(usize::from(body.width), usize::from(body.height));
        }
        if self.reader.links.is_empty() {
            self.links = None;
        }
        if let Some(palette) = &self.palette {
            let count = self.palette_items(env).len();
            let selected = palette.selected.min(count.saturating_sub(1));
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(palette) = &mut self.palette {
                palette.selected = selected;
                if let Some(rect) = rect {
                    keep_visible(
                        &mut palette.offset,
                        selected,
                        // One row of the box is the line being typed.
                        layout::inner_height(rect).saturating_sub(1),
                        count,
                    );
                }
            }
        }
        let files = self.reader.files().len();
        if files == 0 {
            self.attachments = None;
        }
        if let Some(picker) = &mut self.attachments {
            picker.selected = picker.selected.min(files - 1);
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(rect) = rect
                && let Some(picker) = &mut self.attachments
            {
                keep_visible(
                    &mut picker.offset,
                    picker.selected,
                    layout::inner_height(rect),
                    files,
                );
            }
        }
        if let Some(picker) = &self.folders {
            let count = self.folder_choices(picker).len();
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(picker) = &mut self.folders {
                picker.selected = picker.selected.min(count.saturating_sub(1));
                if let Some(rect) = rect {
                    keep_visible(
                        &mut picker.offset,
                        picker.selected,
                        // One row of the box is the line being typed.
                        layout::inner_height(rect).saturating_sub(1),
                        count,
                    );
                }
            }
        }
        // The templates are for a message being written: with none, there
        // is nothing to put one into.
        if self.compose.is_none() {
            self.templates = None;
        }
        if let Some(picker) = &self.templates {
            let count = self.template_choices(env, picker).len();
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(picker) = &mut self.templates {
                picker.selected = picker.selected.min(count.saturating_sub(1));
                if let Some(rect) = rect {
                    keep_visible(
                        &mut picker.offset,
                        picker.selected,
                        // One row of the box is the line being typed.
                        layout::inner_height(rect).saturating_sub(1),
                        count,
                    );
                }
            }
        }
        if let Some(picker) = &self.hooks {
            let count = self.hook_choices(env, picker).len();
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(picker) = &mut self.hooks {
                picker.selected = picker.selected.min(count.saturating_sub(1));
                if let Some(rect) = rect {
                    keep_visible(
                        &mut picker.offset,
                        picker.selected,
                        // One row of the box is the line being typed.
                        layout::inner_height(rect).saturating_sub(1),
                        count,
                    );
                }
            }
        }
        // Spellings are for a word of the message being written, as it
        // was when they were asked for.
        if self.suggestions.as_ref().is_some_and(|picker| {
            self.compose
                .as_ref()
                .is_none_or(|compose| compose.body.revision() != picker.revision)
        }) {
            self.suggestions = None;
        }
        if self.suggestions.is_some() {
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let (Some(picker), Some(rect)) = (&mut self.suggestions, rect) {
                picker.selected = picker.selected.min(picker.words.len().saturating_sub(1));
                keep_visible(
                    &mut picker.offset,
                    picker.selected,
                    layout::inner_height(rect),
                    picker.words.len(),
                );
            }
        }
        if let Some(picker) = &mut self.links {
            picker.selected = picker.selected.min(self.reader.links.len() - 1);
            // Laid out again: the picker's own size follows the links.
            let rect = layout::compute(self, self.size, env.reader_width).links;
            if let Some(rect) = rect
                && let Some(picker) = &mut self.links
            {
                keep_visible(
                    &mut picker.offset,
                    picker.selected,
                    layout::inner_height(rect),
                    self.reader.links.len(),
                );
            }
        }
        if let (Some(help), Some(rect)) = (&mut self.help, areas.help) {
            let lines = view::help::shown(env, help).len();
            help.scroll = help
                .scroll
                .min(lines.saturating_sub(layout::inner_height(rect)));
        }
    }

    /// The cells of the reader given over to its picture: columns and
    /// rows, both zero when there is none to draw or no room for it.
    pub fn reader_picture_cells(&self, env: &Env, reader: ratatui::layout::Rect) -> (u16, u16) {
        let size = self
            .reader
            .picture
            .as_ref()
            .and_then(|picture| picture.size);
        match size {
            Some(size) if env.images => {
                layout::reader_picture_cells(reader, self.reader_header_rows(), size)
            }
            _ => (0, 0),
        }
    }

    /// The rows of the reader given over to its picture.
    pub fn reader_image_rows(&self, env: &Env, reader: ratatui::layout::Rect) -> usize {
        usize::from(self.reader_picture_cells(env, reader).1)
    }

    /// How many rows the reader's header fields take for the message on
    /// show: From, To, Date, Subject, and Cc and the files when it has
    /// them.
    pub fn reader_field_rows(&self) -> usize {
        layout::reader_header_height(self.reader_has_cc(), !self.reader.files().is_empty())
    }

    /// How many rows there are above the reader's picture and text: the
    /// header fields, and under them, each after a blank row, the line
    /// about the rest of the message and the invitation's card when there
    /// is one.
    pub fn reader_header_rows(&self) -> usize {
        let under = |rows: usize| if rows == 0 { 0 } else { rows + 1 };
        self.reader_field_rows() + under(self.reader.note.len()) + under(self.reader.card.len())
    }

    /// What the reader says, above a message's text, about the rest of it
    /// not being here.
    fn reader_note(&self) -> Option<String> {
        self.reader
            .rest_note()
            .filter(|_| matches!(self.reader.body, Body::Loaded(_)))
    }

    /// The card for the invitation the message on show carries.
    fn invite_card(&self, env: &Env) -> Vec<CardRow> {
        let (Some(row), Body::Loaded(body)) = (&self.reader.row, &self.reader.body) else {
            return Vec::new();
        };
        let Some(invitation) = &body.invite else {
            return Vec::new();
        };
        let mine = self
            .accounts
            .iter()
            .find(|account| account.id == row.account)
            .map(|account| account.address.as_str());
        let key = |id| env.hint_key(id).map(ToString::to_string);
        let keys = match (
            key(CommandId::InviteAccept),
            key(CommandId::InviteDecline),
            key(CommandId::InviteTentative),
        ) {
            (Some(accept), Some(decline), Some(maybe)) => Some([accept, decline, maybe]),
            _ => None,
        };
        crate::invite::card(invitation, &env.tz, mine, keys.as_ref())
    }

    pub fn reader_has_cc(&self) -> bool {
        self.reader
            .row
            .as_ref()
            .is_some_and(|row| !row.headers.cc.is_empty())
    }

    /// The position of an account, which picks its colour and its marker.
    pub fn account_index(&self, id: LocalAccountId) -> Option<usize> {
        self.accounts.iter().position(|account| account.id == id)
    }

    fn current_folder(&self) -> Option<(&str, &str, FolderRole, u32)> {
        let Mailbox::Folder(current) = self.mailbox else {
            return None;
        };
        self.sidebar.entries.iter().find_map(|entry| match entry {
            SidebarEntry::Folder {
                id,
                account,
                name,
                role,
                unread,
                ..
            } if *id == current => Some((
                self.accounts.get(*account).map_or("", |a| a.name.as_str()),
                name.as_str(),
                *role,
                *unread,
            )),
            _ => None,
        })
    }

    /// The name and unread count of the account whose own row is what
    /// the list shows.
    fn shown_account(&self) -> Option<(&str, u32)> {
        let Mailbox::Account(current) = self.mailbox else {
            return None;
        };
        self.sidebar.entries.iter().find_map(|entry| match entry {
            SidebarEntry::Account {
                id, name, unread, ..
            } if *id == current => Some((name.as_str(), *unread)),
            _ => None,
        })
    }

    /// The list pane's title: "Inbox (unified)", "Inbox (Work)" for an
    /// account's own row, or "Work / Sent".
    pub fn mailbox_title(&self) -> String {
        match (self.mailbox, self.current_folder()) {
            (Mailbox::Unified, _) => "Inbox (unified)".to_owned(),
            (Mailbox::Account(_), _) => self
                .shown_account()
                .map_or_else(String::new, |(name, _)| format!("Inbox ({name})")),
            (Mailbox::Folder(_), Some((account, folder, _, _))) => format!(
                "{account} / {folder}{}",
                if self.current_folder_is_local() {
                    ON_THIS_MAC_SUFFIX
                } else {
                    ""
                }
            ),
            (Mailbox::Folder(_), None) => String::new(),
            (Mailbox::Drafts, _) => "Drafts".to_owned(),
            (Mailbox::Outbox, _) => "Outbox".to_owned(),
            (Mailbox::Snoozed, _) => "Snoozed".to_owned(),
        }
    }

    pub fn mailbox_unread(&self) -> u32 {
        match self.mailbox {
            Mailbox::Unified => self
                .sidebar
                .entries
                .iter()
                .find_map(|entry| match entry {
                    SidebarEntry::Unified { unread } => Some(*unread),
                    _ => None,
                })
                .unwrap_or(0),
            Mailbox::Account(_) => self.shown_account().map_or(0, |(_, unread)| unread),
            Mailbox::Folder(_) => self.current_folder().map_or(0, |(_, _, _, unread)| unread),
            Mailbox::Drafts | Mailbox::Outbox | Mailbox::Snoozed => 0,
        }
    }

    /// In Sent and Drafts the interesting party is the recipient.
    pub fn shows_recipients(&self) -> bool {
        matches!(self.mailbox, Mailbox::Drafts | Mailbox::Outbox)
            || self.current_folder().is_some_and(|(_, _, role, _)| {
                matches!(role, FolderRole::Sent | FolderRole::Drafts)
            })
    }
}

fn sidebar_entries(
    accounts: &[AccountRow],
    folders: &[FolderRow],
    [drafts, outbox, snoozed]: [u32; 3],
    showing: Mailbox,
) -> Vec<SidebarEntry> {
    if accounts.is_empty() {
        return Vec::new();
    }
    let unified_unread = folders
        .iter()
        .filter(|folder| folder.role == FolderRole::Inbox)
        .map(|folder| folder.unread)
        .sum();
    let mut entries = vec![SidebarEntry::Unified {
        unread: unified_unread,
    }];
    // Mail that lives only here: shown while there is any, and while it is
    // what the list shows, so the cursor is never left on a missing row.
    for (mailbox, name, count) in [
        (Mailbox::Drafts, "Drafts", drafts),
        (Mailbox::Outbox, "Outbox", outbox),
        (Mailbox::Snoozed, "Snoozed", snoozed),
    ] {
        if count > 0 || showing == mailbox {
            entries.push(SidebarEntry::Local {
                mailbox,
                name,
                count,
            });
        }
    }
    for (index, account) in accounts.iter().enumerate() {
        entries.push(SidebarEntry::Account {
            index,
            id: account.id,
            name: text::sanitize_line(&account.name),
            // Its inbox's, as the unified count is every inbox's.
            unread: folders
                .iter()
                .filter(|folder| folder.account == account.id && folder.role == FolderRole::Inbox)
                .map(|folder| folder.unread)
                .sum(),
        });
        // The server's folders, and then, under a heading that says so,
        // the ones that are only on this Mac.
        for local in [false, true] {
            let mut of_kind = folders
                .iter()
                .filter(|folder| folder.account == account.id && folder.local == local)
                .peekable();
            if local && of_kind.peek().is_some() {
                entries.push(SidebarEntry::OnThisMac);
            }
            entries.extend(of_kind.map(|folder| SidebarEntry::Folder {
                id: folder.id,
                account: index,
                name: text::sanitize_line(&folder.name),
                role: folder.role,
                unread: folder.unread,
                local,
            }));
        }
    }
    entries
}

/// Header text comes from the network. Clean it once, on the way in.
fn sanitize_row(row: &mut MessageRow) {
    let headers = &mut row.headers;
    headers.subject = text::sanitize_line(&headers.subject);
    for address in headers
        .from
        .iter_mut()
        .chain(&mut headers.to)
        .chain(&mut headers.cc)
    {
        address.email = text::sanitize_line(&address.email);
        if let Some(name) = &mut address.name {
            *name = text::sanitize_line(name);
        }
    }
}

fn step(current: usize, len: usize, motion: Motion, page: usize) -> usize {
    let last = len.saturating_sub(1);
    match motion {
        Motion::Next => (current + 1).min(last),
        Motion::Prev => current.saturating_sub(1),
        Motion::Top => 0,
        Motion::Bottom => last,
        Motion::PageDown => (current + page).min(last),
        Motion::PageUp => current.saturating_sub(page),
    }
}

fn scroll(current: usize, motion: Motion, page: usize) -> usize {
    match motion {
        Motion::Next => current.saturating_add(1),
        Motion::Prev => current.saturating_sub(1),
        Motion::Top => 0,
        Motion::Bottom => usize::MAX,
        Motion::PageDown => current.saturating_add(page),
        Motion::PageUp => current.saturating_sub(page),
    }
}

/// Lists the messages of each conversation together. Conversations stay in
/// the order of their newest message, newest first, so new mail is still
/// at the top; within one, messages run in the order they were written.
/// Returns, beside the rows, which of them carry on the conversation of
/// the row above.
fn by_conversation(rows: Vec<MessageRow>) -> (Vec<MessageRow>, Vec<bool>) {
    // The rows come newest first: a conversation's place is where its
    // newest message was.
    let mut order: Vec<i64> = Vec::new();
    let mut groups: HashMap<i64, Vec<MessageRow>> = HashMap::new();
    for row in rows {
        let group = groups.entry(row.thread).or_default();
        if group.is_empty() {
            order.push(row.thread);
        }
        group.push(row);
    }
    let mut arranged = Vec::new();
    let mut follows = Vec::new();
    for thread in order {
        let Some(mut group) = groups.remove(&thread) else {
            continue;
        };
        // Oldest first within the conversation.
        group.reverse();
        for (index, row) in group.into_iter().enumerate() {
            follows.push(index > 0);
            arranged.push(row);
        }
    }
    (arranged, follows)
}

/// A body laid out `width` columns wide, and where its links lead: its
/// HTML as styled text when that is what is showing and it renders,
/// otherwise its plain text, wrapped.
fn body_lines(body: &LoadedBody, showing: Showing, width: usize) -> (Vec<Line>, Vec<String>) {
    let rendered = match showing {
        Showing::Html => body
            .html
            .as_deref()
            .and_then(|html| quill_mime::html::render(html, width)),
        Showing::Plain => None,
    }
    // Plain text is laid out too: quotes marked, links found, a signature
    // set apart.
    .unwrap_or_else(|| quill_mime::plain::render(&body.text, width));
    match Some(rendered) {
        Some(mut rendered) => {
            // What a stranger's HTML says is as untrusted as anything else.
            for line in &mut rendered.lines {
                for span in &mut line.spans {
                    span.text = text::sanitize_line(&span.text);
                }
                line.spans.retain(|span| !span.text.is_empty());
            }
            for link in &mut rendered.links {
                *link = text::sanitize_line(link);
            }
            (rendered.lines, rendered.links)
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// Moves `offset` the least distance that puts `selected` inside a window of
/// `height` rows, without scrolling past the end.
fn keep_visible(offset: &mut usize, selected: usize, height: usize, len: usize) {
    if height == 0 {
        return;
    }
    if selected < *offset {
        *offset = selected;
    } else if selected >= *offset + height {
        *offset = selected + 1 - height;
    }
    *offset = (*offset).min(len.saturating_sub(height));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_stays_inside_the_list() {
        assert_eq!(step(0, 0, Motion::Next, 5), 0);
        assert_eq!(step(0, 3, Motion::Prev, 5), 0);
        assert_eq!(step(2, 3, Motion::Next, 5), 2);
        assert_eq!(step(0, 30, Motion::PageDown, 5), 5);
        assert_eq!(step(28, 30, Motion::PageDown, 5), 29);
        assert_eq!(step(3, 30, Motion::PageUp, 5), 0);
        assert_eq!(step(7, 30, Motion::Top, 5), 0);
        assert_eq!(step(7, 30, Motion::Bottom, 5), 29);
    }

    #[test]
    fn keep_visible_scrolls_the_least_it_can() {
        let mut offset = 0;
        keep_visible(&mut offset, 4, 5, 20);
        assert_eq!(offset, 0);
        keep_visible(&mut offset, 5, 5, 20);
        assert_eq!(offset, 1);
        keep_visible(&mut offset, 19, 5, 20);
        assert_eq!(offset, 15);
        keep_visible(&mut offset, 3, 5, 20);
        assert_eq!(offset, 3);
        // A list that shrank or a pane that grew must not leave a gap below.
        let mut offset = 15;
        keep_visible(&mut offset, 2, 10, 12);
        assert_eq!(offset, 2);
        let mut offset = 7;
        keep_visible(&mut offset, 8, 0, 20);
        assert_eq!(offset, 7);
    }
}
