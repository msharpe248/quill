//! The UI state and its one way of changing: `Event -> Action -> update`.
//!
//! `update` never touches the terminal, the disk or the network. It mutates
//! the state and returns the store requests the change calls for; the run
//! loop sends those to the store worker, whose answers come back as actions.

use std::collections::{BTreeSet, HashMap, VecDeque};

use jiff::tz::TimeZone;
use quill_core::types::{FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_mime::html::Line;
use quill_mime::{BodyFormat, ReplyPosition};
use quill_store::{AccountRow, DraftContent, DraftId, FolderRow, Mailbox, MessageRow};
use quill_sync::{SyncEvent, SyncRequest};

use crate::command::{CommandId, Context, Registry};
use crate::compose::{Compose, Did, Field, Prefill};
use crate::data::{DataEvent, DataRequest, LoadedBody};
use crate::editor::Editor;
use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Keymap, Mods, Resolution};
use crate::layout::{self, LayoutMode};
use crate::outgoing::{Response, SendRequest};
use crate::setup::{AccountEvent, AccountRequest, CertificatePrompt, Outcome, Setup};
use crate::theme::Styles;
use crate::{text, view};

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
    /// How mail is written and sent, from the config.
    pub compose: ComposeSettings,
    /// Who the user is on each account, by the account's name.
    pub identities: HashMap<String, Identity>,
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
    /// Show the built-in sample file in the host's preview pane.
    PreviewSample,
    /// Open a web or mail address in the user's browser or mail program.
    /// Only ever an address `quill_mime::links::is_openable` accepts.
    OpenUrl(String),
    /// Put text on the system clipboard.
    Copy(String),
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
}

impl SidebarEntry {
    pub fn mailbox(&self) -> Option<Mailbox> {
        match self {
            Self::Unified { .. } => Some(Mailbox::Unified),
            Self::Local { mailbox, .. } => Some(*mailbox),
            Self::Account { .. } => None,
            Self::Folder { id, .. } => Some(Mailbox::Folder(*id)),
        }
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
    /// The message to move.
    pub message: LocalMessageId,
    pub filter: LineInput,
    pub selected: usize,
    pub offset: usize,
}

impl FolderPicker {
    pub fn for_message(message: LocalMessageId) -> Self {
        Self {
            message,
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
}

impl Reader {
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

#[derive(Debug, Clone, Copy)]
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
    pub folders: Option<FolderPicker>,
    /// The message being written, in the reader's place.
    pub compose: Option<Compose>,
    /// A message the store worker has prepared to write, opened at the end
    /// of the update that brought it.
    compose_pending: Option<Box<Prefill>>,
    /// A reply or forward waiting for its message to be downloaded.
    respond_pending: Option<(LocalMessageId, DataRequest)>,
    /// What was just done, in a few words, shown until the next key, and
    /// whether the undo command would take it back.
    pub flash: Option<(String, bool)>,
    /// The add-an-account flow, while it is open.
    pub setup: Option<Setup>,
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
    /// Show the built-in sample image in the reader. A stand-in until
    /// messages carry images of their own (M4).
    pub sample_image: bool,
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
            folders: None,
            compose: None,
            compose_pending: None,
            respond_pending: None,
            flash: None,
            setup: None,
            certificates: VecDeque::new(),
            setup_offered: false,
            accounts_known: false,
            pending: Vec::new(),
            notices: VecDeque::new(),
            now,
            syncing: BTreeSet::new(),
            sample_image: false,
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
        if self.setup.is_some() {
            return Context::Setup;
        }
        if self.links.is_some() {
            return Context::Links;
        }
        if self.folders.is_some() {
            return Context::Folders;
        }
        if self.compose.is_some() {
            return Context::Compose;
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
            Action::Data(event) => self.on_data(event, &mut out),
            Action::ClockTick(now) => {
                // Dates are drawn relative to today, so only a new day shows.
                changed = text::local_day(now, &env.tz) != text::local_day(self.now, &env.tz);
                self.now = now;
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
            Action::Sync(event) => self.on_sync(event, &mut out),
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
                Context::Compose => {
                    if let Some(compose) = &mut self.compose {
                        compose.paste(&text);
                    }
                }
                _ => {}
            },
        }
        if let Some(prefill) = self.compose_pending.take() {
            self.open_compose(env, *prefill);
        }
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
                            out.data.push(DataRequest::Attach { path });
                            return;
                        }
                    }
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
            Context::Setup => {
                if let Some(requests) = self.setup.as_mut().and_then(|setup| setup.key(chord)) {
                    out.accounts.extend(requests);
                    return;
                }
            }
            Context::Certificate if self.answer_certificate(chord, out) => return,
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
            CommandId::NavOpen if context == Context::Links => self.open_selected_link(out),
            CommandId::NavOpen if context == Context::Folders => {
                if let Some(picker) = self.folders.take()
                    && let Some(choice) = self.folder_choices(&picker).get(picker.selected)
                {
                    out.data.push(DataRequest::MoveTo {
                        id: picker.message,
                        folder: choice.id,
                    });
                }
            }
            CommandId::MessageArchive => {
                if let Some(row) = self.target_row() {
                    out.data.push(DataRequest::Archive(row.id));
                }
            }
            CommandId::MessageDelete => {
                if let Some(row) = self.target_row() {
                    out.data.push(DataRequest::Delete(row.id));
                }
            }
            CommandId::MessageMove => {
                if let Some(row) = self.target_row() {
                    self.folders = Some(FolderPicker::for_message(row.id));
                }
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
            CommandId::ComposeSend => self.send(env, out),
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
            CommandId::ComposeAttach => {
                if let Some(compose) = &mut self.compose {
                    compose.ask_for_attachment();
                }
            }
            CommandId::AccountAdd => self.setup = Some(Setup::default()),
            CommandId::NavOpen => self.open(out),
            CommandId::NavBack => self.back(out),
            CommandId::AppQuit => self.should_quit = true,
            CommandId::FocusSidebar => self.set_focus(Pane::Sidebar),
            CommandId::FocusList => self.set_focus(Pane::List),
            CommandId::FocusReader => self.set_focus(Pane::Reader),
            CommandId::FocusNext => self.set_focus(self.focus.next()),
            CommandId::FocusPrev => self.set_focus(self.focus.prev()),
            CommandId::ViewZoom => self.zoomed = !self.zoomed,
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
            CommandId::DebugPreviewSample => out.host.push(HostRequest::PreviewSample),
            CommandId::DebugImageSample => self.sample_image = !self.sample_image,
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
            CommandId::NavOpen if context == Context::Links => !self.reader.links.is_empty(),
            CommandId::NavOpen if context == Context::Setup => true,
            CommandId::NavOpen if context == Context::Folders => self
                .folders
                .as_ref()
                .is_some_and(|picker| !self.folder_choices(picker).is_empty()),
            CommandId::MessageDelete => self.target_row().is_some(),
            CommandId::MessageCompose => !self.accounts.is_empty(),
            CommandId::MessageReply | CommandId::MessageReplyAll | CommandId::MessageForward => {
                self.target_row()
                    .is_some_and(|row| DraftId::from_message_id(row.id).is_none())
            }
            CommandId::ComposeSend => self
                .compose
                .as_ref()
                .is_some_and(|compose| !compose.sending),
            // A draft is in no folder of any server: it can be thrown away,
            // not filed or flagged.
            CommandId::MessageArchive | CommandId::MessageMove => self
                .target_row()
                .is_some_and(|row| DraftId::from_message_id(row.id).is_none()),
            CommandId::AccountAdd => env.accounts,
            CommandId::NavOpen => {
                self.focus == Pane::Sidebar || self.list.rows.get(self.list.selected).is_some()
            }
            CommandId::MessageToggleHtml => self.reader.has_two_versions(),
            CommandId::MessageOpenLink => !self.reader.links.is_empty(),
            CommandId::NavBack => {
                self.help.is_some()
                    || self.links.is_some()
                    || self.folders.is_some()
                    || self.compose.is_some()
                    || self.setup.is_some()
                    || !self.certificates.is_empty()
                    || !self.notices.is_empty()
                    || self.focus == Pane::Reader
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
                    .filter(|(_, entry)| entry.mailbox().is_some())
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
                    self.sidebar.selected = target;
                    if let Some(mailbox) = self.sidebar.entries[target].mailbox() {
                        self.show_mailbox(mailbox, out);
                    }
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
                let has_cc = self.reader_has_cc();
                let page = areas
                    .reader
                    .map_or(1, |rect| {
                        layout::reader_body_height(rect, has_cc, self.reader_image_rows(env, rect))
                    })
                    .saturating_sub(1)
                    .max(1);
                // Clamped to the end of the body by `relayout`.
                self.reader.scroll = scroll(self.reader.scroll, motion, page);
            }
        }
    }

    fn show_mailbox(&mut self, mailbox: Mailbox, out: &mut Effects) {
        if self.mailbox == mailbox {
            return;
        }
        self.mailbox = mailbox;
        self.list = MessageList::default();
        out.data.push(DataRequest::LoadMessages(mailbox));
    }

    fn open(&mut self, out: &mut Effects) {
        match self.focus {
            Pane::Sidebar => self.set_focus(Pane::List),
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
        } else if !self.certificates.is_empty() {
            // Not answering is rejecting: nothing is sent to that server.
            self.certificates.pop_front();
        } else if let Some(setup) = &mut self.setup {
            if setup.back() {
                self.setup = None;
            }
        } else if self.links.is_some() {
            self.links = None;
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
    fn send(&mut self, env: &Env, out: &mut Effects) {
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
            delay_seconds: i64::from(env.compose.undo_send_seconds),
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

    fn toggle_unread(&mut self, out: &mut Effects) {
        let Some(row) = self.target_row() else {
            return;
        };
        let (id, previous) = (row.id, row.flags);
        let seen = !previous.seen;
        self.set_seen_locally(id, seen);
        out.data.push(DataRequest::SetSeen { id, seen });
    }

    fn toggle_star(&mut self, out: &mut Effects) {
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

    fn on_sync(&mut self, event: SyncEvent, out: &mut Effects) {
        let reading = |state: &Self, id| state.reader.row.as_ref().is_some_and(|row| row.id == id);
        match event {
            SyncEvent::Changed => self.reload(out),
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
    fn reload(&self, out: &mut Effects) {
        out.data.push(DataRequest::LoadSidebar);
        out.data.push(DataRequest::LoadMessages(self.mailbox));
    }

    fn on_data(&mut self, event: DataEvent, out: &mut Effects) {
        match event {
            DataEvent::SidebarLoaded {
                accounts,
                folders,
                drafts,
                outbox,
            } => {
                self.sidebar.entries =
                    sidebar_entries(&accounts, &folders, drafts, outbox, self.mailbox);
                self.accounts = accounts;
                self.accounts_known = true;
                match self
                    .sidebar
                    .entries
                    .iter()
                    .position(|entry| entry.mailbox() == Some(self.mailbox))
                {
                    Some(index) => self.sidebar.selected = index,
                    None => {
                        // The folder being shown is gone.
                        self.sidebar.selected = 0;
                        self.show_mailbox(Mailbox::Unified, out);
                    }
                }
            }
            DataEvent::MessagesLoaded { mailbox, mut rows } => {
                if mailbox != self.mailbox {
                    // An answer for a mailbox the user has already left.
                    return;
                }
                rows.iter_mut().for_each(sanitize_row);
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
                self.sync_reader(out);
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
                }
            }
            DataEvent::StoreChanged => self.reload(out),
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
                    compose.attachments.push(attachment);
                }
            }
            DataEvent::AttachFailed(reason) => {
                if let Some(compose) = &mut self.compose {
                    compose.error = Some(text::sanitize_line(&reason));
                }
            }
            DataEvent::ComposeReady(prefill) => self.compose_pending = Some(prefill),
            DataEvent::RespondNeedsBody(request) => {
                if let DataRequest::Respond { id, .. } = *request {
                    self.respond_pending = Some((id, *request));
                    out.sync.push(SyncRequest::FetchBody(id));
                    self.flash = Some(("Downloading the message…".to_owned(), false));
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
                self.reader_has_cc(),
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

    /// The rows of the reader given over to an inline image.
    pub fn reader_image_rows(&self, env: &Env, reader: ratatui::layout::Rect) -> usize {
        let shown = self.sample_image && self.reader.row.is_some();
        layout::reader_image_height(reader, self.reader_has_cc(), shown, env.images)
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
            Mailbox::Drafts | Mailbox::Outbox => 0,
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
    drafts: u32,
    outbox: u32,
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
    };
    match rendered {
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
        None => (
            text::wrap(&body.text, width)
                .into_iter()
                .map(Line::plain)
                .collect(),
            quill_mime::links::find(&body.text),
        ),
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
