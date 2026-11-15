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
use crate::compose::{Compose, Did, Field, Prefill};
use crate::data::{Bulk, DataEvent, DataRequest, FileUse, LoadedBody, PageUse};
use crate::editor::Editor;
use crate::files::RemoteContent;
use crate::files::{AttachedFile, ExtractedFile};
use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Keymap, Mods, Resolution};
use crate::layout::{self, LayoutMode};
use crate::outgoing::{Handed, Response, SendRequest};
use crate::palette::{self, Item, Palette, What};

mod mouse;
use crate::setup::{
    AccountEvent, AccountRequest, CertificatePrompt, Outcome, Setup, SignIn, SignInStage,
};
use crate::theme::Styles;
use crate::{text, view};
pub use mouse::{Mouse, MouseKind};

/// What a notification about one message is called: this and the message's
/// id in the store.
const MESSAGE_NOTIFICATION: &str = "message:";
/// What the notification about several messages at once is called.
const SEVERAL_NOTIFICATION: &str = "arrived";
/// More new messages than this at once are announced together.
const MOST_ANNOUNCED_SINGLY: usize = 3;

/// Everything fixed for the life of the run: what `update` and the views
/// consult but never change.
pub struct Env {
    pub registry: Registry,
    pub keymap: Keymap,
    pub styles: Styles,
    pub tz: TimeZone,
    /// The reader's share of the space right of the sidebar, from the config.
    pub reader_width: f32,
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
}

impl Env {
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
}

impl Default for ComposeSettings {
    fn default() -> Self {
        Self {
            markdown: true,
            reply_above: true,
            undo_send_seconds: 10,
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
}

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
            && self.host.is_empty()
            && self.sync.is_empty()
            && self.accounts.is_empty()
    }
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
    /// Show a message's sanitized page beside the terminal. `remote`: it
    /// may load remote content.
    Web {
        path: PathBuf,
        title: String,
        remote: bool,
    },
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
}

/// What the app shell mirrors: enough to enable menu items, badge the Dock
/// and title the window, and nothing about mail itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostState {
    pub context: Context,
    pub enabled: Vec<CommandId>,
    pub unread: u32,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    List,
    Reader,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::List,
            Self::List => Self::Reader,
            Self::Reader => Self::Sidebar,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Sidebar => Self::Reader,
            Self::List => Self::Sidebar,
            Self::Reader => Self::List,
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
    /// A heading. The cursor skips it.
    Account {
        index: usize,
        name: String,
    },
    Folder {
        id: LocalFolderId,
        /// The position of the folder's account, for its colour.
        account: usize,
        name: String,
        role: FolderRole,
        unread: u32,
    },
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
            Self::Account { .. } | Self::Searches | Self::Search { .. } => None,
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
    pub entries: Vec<SidebarEntry>,
    pub selected: usize,
    pub offset: usize,
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
    pub filter: LineInput,
    pub selected: usize,
    pub offset: usize,
}

impl FolderPicker {
    pub fn for_message(message: LocalMessageId) -> Self {
        Self {
            message,
            also: Vec::new(),
            filter: LineInput::default(),
            selected: 0,
            offset: 0,
        }
    }
}

/// A folder on offer in the folder picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderChoice {
    pub id: LocalFolderId,
    pub name: String,
}

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
    /// Where this message's page was last shown, so that loading its
    /// remote content shows it there again.
    pub page: Option<PageUse>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    /// The context help was opened from, and so describes.
    pub context: Context,
    pub scroll: usize,
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
    /// A sign-in to an account's provider, waiting on the user.
    pub sign_in: Option<SignIn>,
    /// A time being asked for.
    pub when: Option<WhenPrompt>,
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
            compose: None,
            compose_pending: None,
            respond_pending: None,
            ungrouped: false,
            go_to: None,
            announced: BTreeSet::new(),
            flash: None,
            setup: None,
            sign_in: None,
            when: None,
            next_wake: None,
            certificates: VecDeque::new(),
            setup_offered: false,
            accounts_known: false,
            pending: Vec::new(),
            notices: VecDeque::new(),
            now,
            syncing: BTreeSet::new(),
            should_quit: false,
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
        match action {
            Action::Key(chord) => self.on_key(env, chord, &mut out),
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
            Action::Sync(event) => self.on_sync(env, event, &mut out),
            Action::Account(event) => self.on_account(event, &mut out),
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
            self.open_compose(env, *prefill);
        }
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
            Context::Certificate if self.answer_certificate(chord, out) => return,
            // Waiting on the browser: only giving up is on offer, and a
            // letter must not run a command behind the dialog.
            Context::SignIn if matches!(chord.key, Key::Char(_)) => return,
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
                        if let Some(index) = self
                            .sidebar
                            .entries
                            .iter()
                            .position(|entry| entry.mailbox() == Some(mailbox))
                        {
                            self.sidebar.selected = index;
                        }
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
            CommandId::NavOpen if context == Context::Folders => {
                if let Some(picker) = self.folders.take()
                    && let Some(choice) = self.folder_choices(&picker).get(picker.selected)
                {
                    if picker.also.is_empty() {
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
            CommandId::MessageMove => {
                // Drafts are in no folder of any server and stay put.
                let mut ids = self
                    .targets()
                    .into_iter()
                    .filter(|id| DraftId::from_message_id(*id).is_none());
                if let Some(first) = ids.next() {
                    let mut picker = FolderPicker::for_message(first);
                    picker.also = ids.collect();
                    self.folders = Some(picker);
                }
            }
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
            CommandId::ComposeSend => self.send(env, None, out),
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
                if let Some(compose) = &self.compose {
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
            // From the app's Edit menu, which takes these keys before the
            // editor can. Typed in a terminal they reach the editor itself.
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
            CommandId::AccountSignIn => {
                out.accounts.push(AccountRequest::SignIn {
                    account: self.current_account().map(|account| account.name.clone()),
                });
                self.sign_in = Some(SignIn {
                    account: String::new(),
                    stage: SignInStage::Starting,
                });
            }
            CommandId::NavOpen => self.open(out),
            CommandId::NavBack => self.back(out),
            CommandId::AppQuit => self.should_quit = true,
            CommandId::FocusSidebar => self.set_focus(Pane::Sidebar),
            CommandId::FocusList => self.set_focus(Pane::List),
            CommandId::FocusReader => self.set_focus(Pane::Reader),
            CommandId::FocusNext => self.set_focus(self.focus.next()),
            CommandId::FocusPrev => self.set_focus(self.focus.prev()),
            CommandId::ViewZoom => self.zoomed = !self.zoomed,
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
            CommandId::MessageViewHtml => self.show_page(env, PageUse::Pane, out),
            CommandId::MessageOpenInBrowser => self.show_page(env, PageUse::Browser, out),
            CommandId::MessageLoadRemote => {
                self.reader.remote = true;
                self.show_page(env, self.reader.page.unwrap_or(PageUse::Pane), out);
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
                    self.show_page(env, self.reader.page.unwrap_or(PageUse::Pane), out);
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
            CommandId::HelpToggle => {
                self.help = match self.help {
                    Some(_) => None,
                    None => Some(Help { context, scroll: 0 }),
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
            CommandId::MessageDelete => self.target_row().is_some(),
            CommandId::SelectToggle | CommandId::SelectRange | CommandId::SelectAll => {
                !self.list.rows.is_empty()
            }
            CommandId::MessageCompose => !self.accounts.is_empty(),
            CommandId::MessageReply | CommandId::MessageReplyAll | CommandId::MessageForward => {
                self.target_row()
                    .is_some_and(|row| DraftId::from_message_id(row.id).is_none())
            }
            CommandId::ComposeSend | CommandId::ComposeSendLater => self
                .compose
                .as_ref()
                .is_some_and(|compose| !compose.sending),
            CommandId::MessageSnooze => self
                .targets()
                .iter()
                .any(|id| DraftId::from_message_id(*id).is_none()),
            CommandId::NavOpen if context == Context::When => true,
            // Only where the body's own selection is the one meant.
            CommandId::EditCopy | CommandId::EditCut => {
                self.compose.as_ref().is_some_and(|compose| {
                    compose.field == Field::Body
                        && compose.attaching.is_none()
                        && !compose.sending
                        && compose.body.selection().is_some()
                })
            }
            CommandId::EditSelectAll => self.compose.as_ref().is_some_and(|compose| {
                compose.field == Field::Body && compose.attaching.is_none() && !compose.sending
            }),
            // A draft is in no folder of any server: it can be thrown away,
            // not filed or flagged.
            CommandId::MessageArchive | CommandId::MessageMove => self
                .targets()
                .iter()
                .any(|id| DraftId::from_message_id(*id).is_none()),
            CommandId::AccountAdd => env.accounts,
            CommandId::AccountSignIn => env.accounts && !self.accounts.is_empty(),
            CommandId::NavOpen => {
                self.focus == Pane::Sidebar || self.list.rows.get(self.list.selected).is_some()
            }
            CommandId::MessageToggleHtml => self.reader.has_two_versions(),
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
            CommandId::MessageViewHtml | CommandId::MessageOpenInBrowser => {
                self.page_message(context).is_some()
            }
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
                    || self.compose.is_some()
                    || self.setup.is_some()
                    || self.sign_in.is_some()
                    || self.when.is_some()
                    || !self.certificates.is_empty()
                    || !self.notices.is_empty()
                    || self.focus == Pane::Reader
                    || self.list.has_marks()
                    || (self.focus == Pane::Sidebar
                        && LayoutMode::for_width(self.size.0) == LayoutMode::Narrow)
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
            unread,
            title: if mailbox.is_empty() {
                "Quill".to_owned()
            } else {
                mailbox
            },
        }
    }

    fn set_focus(&mut self, pane: Pane) {
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
        match self.focus {
            Pane::Sidebar => {
                let selectable: Vec<usize> = self
                    .sidebar
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.is_place())
                    .map(|(index, _)| index)
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
            Pane::List => {
                let page = areas
                    .list
                    .map_or(1, layout::inner_height)
                    .saturating_sub(1)
                    .max(1);
                let target = step(self.list.selected, self.list.rows.len(), motion, page);
                if target != self.list.selected {
                    self.list.selected = target;
                    self.sync_reader(out);
                }
            }
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
                self.set_focus(Pane::Reader);
            }
            Pane::Reader => {}
        }
    }

    fn back(&mut self, out: &mut Effects) {
        if self.help.is_some() {
            self.help = None;
        } else if self.palette.is_some() {
            self.palette = None;
        } else if !self.certificates.is_empty() {
            // Not answering is rejecting: nothing is sent to that server.
            self.certificates.pop_front();
        } else if self.when.take().is_some() {
            // Nothing was snoozed or sent.
        } else if self.sign_in.take().is_some() {
            // Nothing is kept of a sign-in given up on.
            out.accounts.push(AccountRequest::CancelSignIn);
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
        } else if let Some(compose) = self.compose.take() {
            // Closing keeps what was written. A reply opened and closed
            // untouched leaves nothing behind, and a draft emptied out is
            // a draft thrown away.
            if compose.sending {
                // It is on its way to the outbox; there is nothing to keep.
            } else if !compose.is_blank() {
                if compose.is_modified() {
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
        } else if self.notices.pop_front().is_some() {
            // One notice dismissed per press.
        } else if self.list.has_marks() && self.focus != Pane::Reader {
            self.list.clear_marks();
        } else if self.search.is_some() && self.focus != Pane::Reader {
            // Out of the search, back to the mailbox it was started from.
            self.search = None;
            self.list = MessageList::default();
            out.data.push(DataRequest::LoadMessages(self.mailbox));
            if let Some(index) = self
                .sidebar
                .entries
                .iter()
                .position(|entry| entry.mailbox() == Some(self.mailbox))
            {
                self.sidebar.selected = index;
            }
        } else {
            match self.focus {
                Pane::Reader => self.set_focus(Pane::List),
                Pane::Sidebar if LayoutMode::for_width(self.size.0) == LayoutMode::Narrow => {
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
                SidebarEntry::Folder { account, name, .. } => format!(
                    "Go to {} / {name}",
                    self.accounts
                        .get(*account)
                        .map_or("", |account| account.name.as_str())
                ),
                SidebarEntry::Search { name, query } => {
                    return Some(Item {
                        what: What::Search(query.clone()),
                        title: format!("Search: {name}"),
                        keys: String::new(),
                    });
                }
                SidebarEntry::Account { .. } | SidebarEntry::Searches => return None,
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
        self.help = None;
        self.links = None;
        self.folders = None;
        self.set_focus(Pane::Reader);
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
                    id, account, name, ..
                } if *id != row.folder
                    && self.accounts.get(*account).map(|account| account.id)
                        == Some(row.account) =>
                {
                    Some(FolderChoice {
                        id: *id,
                        name: name.clone(),
                    })
                }
                _ => None,
            })
            .filter(|choice| {
                let mut name = choice.name.chars().flat_map(char::to_lowercase);
                typed
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .all(|wanted| name.any(|c| c == wanted))
            })
            .collect()
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

    fn on_account(&mut self, event: AccountEvent, out: &mut Effects) {
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
            _ => {}
        }
        let Some(setup) = &mut self.setup else {
            return;
        };
        let (requests, outcome) = setup.on_event(event);
        out.accounts.extend(requests);
        if let Outcome::Added(name) = outcome {
            self.setup = None;
            self.notices.push_back(Notice::info(format!(
                "Added {}. Its mail is on the way.",
                text::sanitize_line(&name)
            )));
            // The new account is in the store: show it.
            out.data.push(DataRequest::LoadSidebar);
            out.data.push(DataRequest::LoadMessages(self.mailbox));
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
        let Mailbox::Folder(folder) = self.mailbox else {
            return None;
        };
        self.sidebar.entries.iter().find_map(|entry| match entry {
            SidebarEntry::Folder { id, account, .. } if *id == folder => {
                self.accounts.get(*account)
            }
            _ => None,
        })
    }

    /// Starts a message to the people a `mailto:` address names.
    fn on_mailto(&mut self, env: &Env, url: &str) {
        let Some(mailto) = quill_core::links::parse_mailto(url) else {
            return;
        };
        if self.compose.is_some() {
            self.notices.push_back(Notice::info(
                "A message is already being written. Send or close it, then follow the link again.",
            ));
            return;
        }
        let Some(account) = self.accounts.first().map(|account| account.id) else {
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
    fn on_notification(&mut self, env: &Env, id: &str, action: Option<&str>, out: &mut Effects) {
        let Some(message) = id
            .strip_prefix(MESSAGE_NOTIFICATION)
            .and_then(|number| number.parse().ok())
            .map(LocalMessageId)
        else {
            // The one about several messages: the inbox is where they are.
            if id == SEVERAL_NOTIFICATION && action.is_none() && self.compose.is_none() {
                self.show_mailbox(Mailbox::Unified, out);
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

    /// Asks for the message's page, to show in the pane or the browser.
    /// Whether it may load remote content is decided here, from what the
    /// user has said, and written into the page.
    fn show_page(&mut self, env: &Env, then: PageUse, out: &mut Effects) {
        let Some(message) = self.page_message(self.context()) else {
            return;
        };
        let remote = if env.remote_never {
            RemoteContent::Never
        } else if self.reader.remote {
            RemoteContent::Allowed
        } else {
            RemoteContent::IfSenderAllowed
        };
        out.data.push(DataRequest::WebPage {
            message,
            remote,
            then,
        });
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
                    self.reader.body = Body::Unavailable(text::sanitize_line(&reason));
                    self.reader.wrap_width = None;
                }
            }
            SyncEvent::Problem { account, message } => {
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
            DataEvent::SidebarLoaded {
                accounts,
                folders,
                drafts,
                outbox,
                snoozed: (snoozed, next_wake),
            } => {
                self.next_wake = next_wake;
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
                    Some(index) => self.sidebar.selected = index,
                    None => {
                        // The folder being shown is gone.
                        self.sidebar.selected = 0;
                        self.show_mailbox(Mailbox::Unified, out);
                    }
                }
            }
            DataEvent::SearchLoaded {
                text,
                mut rows,
                more,
            } => {
                // An answer for something typed over since is dropped.
                if self.search_text().as_deref() != Some(text.as_str()) {
                    return;
                }
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
                self.list.selected = keep
                    .and_then(|id| rows.iter().position(|row| row.id == id))
                    .unwrap_or(if self.list.loaded {
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
                    self.reader.body = match body {
                        Some(body) => Body::Loaded(LoadedBody {
                            text: text::sanitize_body(body.text.trim_end()),
                            ..body
                        }),
                        None => {
                            // Not downloaded yet. Ask for it; the reader
                            // hears when it lands or why it cannot.
                            out.sync.push(SyncRequest::FetchBody(id));
                            Body::Downloading
                        }
                    };
                    self.reader.wrap_width = None;
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
                self.reader.page = Some(then);
                if page.blocked > 0 && !page.remote {
                    let pictures = if page.blocked == 1 {
                        "1 remote picture was left out".to_owned()
                    } else {
                        format!("{} remote pictures were left out", page.blocked)
                    };
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
                out.host.push(match then {
                    PageUse::Pane => HostRequest::Web {
                        path: page.path,
                        title: text::sanitize_line(&page.title),
                        remote: page.remote,
                    },
                    PageUse::Browser => HostRequest::OpenFile(page.path),
                });
            }
            DataEvent::Error(message) => self.notices.push_back(Notice::error(message)),
        }
    }

    /// Re-derives everything that depends on pane sizes: scroll offsets that
    /// keep the selection visible, the reader's wrapped lines, and clamps.
    fn relayout(&mut self, env: &Env) {
        let areas = layout::compute(self, self.size, env.reader_width);
        if let Some(rect) = areas.sidebar {
            keep_visible(
                &mut self.sidebar.offset,
                self.sidebar.selected,
                layout::inner_height(rect),
                self.sidebar.entries.len(),
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
            if self.reader.wrap_width != Some(width) {
                let (lines, links) = match (&self.reader.body, self.reader.showing()) {
                    (Body::Loaded(body), Some(showing)) => body_lines(body, showing, width),
                    _ => (Vec::new(), Vec::new()),
                };
                self.reader.lines = lines;
                self.reader.links = links;
                self.reader.wrap_width = Some(width);
            }
            let viewport = layout::reader_body_height(
                rect,
                self.reader_header_rows(),
                self.reader_image_rows(env, rect),
            );
            let last = self.reader.lines.len().saturating_sub(viewport);
            self.reader.scroll = self.reader.scroll.min(last);
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
            let lines = view::help::lines(env, help.context).len();
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

    /// How many rows the reader's headers take for the message on show.
    pub fn reader_header_rows(&self) -> usize {
        layout::reader_header_height(self.reader_has_cc(), !self.reader.files().is_empty())
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
            } if *id == current => Some((
                self.accounts.get(*account).map_or("", |a| a.name.as_str()),
                name.as_str(),
                *role,
                *unread,
            )),
            _ => None,
        })
    }

    /// The list pane's title: "Inbox (unified)" or "Work / Sent".
    pub fn mailbox_title(&self) -> String {
        match (self.mailbox, self.current_folder()) {
            (Mailbox::Unified, _) => "Inbox (unified)".to_owned(),
            (Mailbox::Folder(_), Some((account, folder, _, _))) => format!("{account} / {folder}"),
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
            name: text::sanitize_line(&account.name),
        });
        entries.extend(
            folders
                .iter()
                .filter(|folder| folder.account == account.id)
                .map(|folder| SidebarEntry::Folder {
                    id: folder.id,
                    account: index,
                    name: text::sanitize_line(&folder.name),
                    role: folder.role,
                    unread: folder.unread,
                }),
        );
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
