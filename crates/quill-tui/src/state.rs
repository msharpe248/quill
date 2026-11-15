//! The UI state and its one way of changing: `Event -> Action -> update`.
//!
//! `update` never touches the terminal, the disk or the network. It mutates
//! the state and returns the store requests the change calls for; the run
//! loop sends those to the store worker, whose answers come back as actions.

use std::collections::{BTreeSet, VecDeque};

use jiff::tz::TimeZone;
use quill_core::types::{Flags, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_store::{AccountRow, FolderRow, Mailbox, MessageRow};
use quill_sync::{SyncEvent, SyncRequest};

use crate::command::{CommandId, Context, Registry};
use crate::data::{DataEvent, DataRequest};
use crate::keymap::{KeyChord, Keymap, Resolution};
use crate::layout::{self, LayoutMode};
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
}

/// What an action asks of the world outside the state: queries and writes
/// for the store worker, things only the host can do, and things only the
/// network can do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub data: Vec<DataRequest>,
    pub host: Vec<HostRequest>,
    pub sync: Vec<SyncRequest>,
}

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty() && self.host.is_empty() && self.sync.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRequest {
    /// Show the built-in sample file in the host's preview pane.
    PreviewSample,
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
    Loaded(String),
    /// Not in the store and not to be had, with the reason to show.
    Unavailable(String),
}

#[derive(Debug, Default)]
pub struct Reader {
    pub row: Option<MessageRow>,
    pub body: Body,
    /// The body wrapped to `wrap_width`.
    pub lines: Vec<String>,
    pub wrap_width: Option<usize>,
    pub scroll: usize,
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
            }
            Action::Notify(notice) => self.notices.push_back(notice),
            Action::Sync(event) => self.on_sync(event, &mut out),
        }
        self.relayout(env);
        self.dirty |= changed;
        out
    }

    fn on_key(&mut self, env: &Env, chord: KeyChord, out: &mut Effects) {
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
        match id {
            CommandId::NavNext => self.navigate(env, Motion::Next, out),
            CommandId::NavPrev => self.navigate(env, Motion::Prev, out),
            CommandId::NavTop => self.navigate(env, Motion::Top, out),
            CommandId::NavBottom => self.navigate(env, Motion::Bottom, out),
            CommandId::NavPageDown => self.navigate(env, Motion::PageDown, out),
            CommandId::NavPageUp => self.navigate(env, Motion::PageUp, out),
            CommandId::NavOpen => self.open(out),
            CommandId::NavBack => self.back(),
            CommandId::AppQuit => self.should_quit = true,
            CommandId::FocusSidebar => self.set_focus(Pane::Sidebar),
            CommandId::FocusList => self.set_focus(Pane::List),
            CommandId::FocusReader => self.set_focus(Pane::Reader),
            CommandId::FocusNext => self.set_focus(self.focus.next()),
            CommandId::FocusPrev => self.set_focus(self.focus.prev()),
            CommandId::ViewZoom => self.zoomed = !self.zoomed,
            CommandId::MessageToggleUnread => self.toggle_unread(out),
            CommandId::MessageToggleStar => self.toggle_star(out),
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
            CommandId::NavOpen => {
                self.focus == Pane::Sidebar || self.list.rows.get(self.list.selected).is_some()
            }
            CommandId::NavBack => {
                self.help.is_some()
                    || !self.notices.is_empty()
                    || self.focus == Pane::Reader
                    || (self.focus == Pane::Sidebar
                        && LayoutMode::for_width(self.size.0) == LayoutMode::Narrow)
            }
            CommandId::MessageToggleUnread | CommandId::MessageToggleStar => {
                self.target_row().is_some()
            }
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
                let (id, previous) = (row.id, row.flags);
                if !previous.seen {
                    self.set_seen_locally(id, true);
                    out.data.push(DataRequest::SetSeen { id, seen: true });
                    out.sync.push(SyncRequest::SetFlags {
                        id,
                        flags: Flags {
                            seen: true,
                            ..previous
                        },
                        previous,
                    });
                }
                self.sync_reader(out);
                self.set_focus(Pane::Reader);
            }
            Pane::Reader => {}
        }
    }

    fn back(&mut self) {
        if self.help.is_some() {
            self.help = None;
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
        out.sync.push(SyncRequest::SetFlags {
            id,
            flags: Flags { seen, ..previous },
            previous,
        });
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
        out.sync.push(SyncRequest::SetFlags {
            id,
            flags: Flags {
                flagged,
                ..previous
            },
            previous,
        });
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
            }
            None => self.reader = Reader::default(),
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
            SyncEvent::BodyReady(id) => {
                if reading(self, id) {
                    out.data.push(DataRequest::LoadBody(id));
                }
            }
            SyncEvent::BodyFailed { id, reason } => {
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
            DataEvent::SidebarLoaded { accounts, folders } => {
                self.sidebar.entries = sidebar_entries(&accounts, &folders);
                self.accounts = accounts;
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
                        Some(body) => Body::Loaded(text::sanitize_body(body.trim_end())),
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
                self.reader.lines = match &self.reader.body {
                    Body::Loaded(body) => text::wrap(body, width),
                    Body::None | Body::Loading | Body::Downloading | Body::Unavailable(_) => {
                        Vec::new()
                    }
                };
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
        }
    }

    /// In Sent and Drafts the interesting party is the recipient.
    pub fn shows_recipients(&self) -> bool {
        self.current_folder()
            .is_some_and(|(_, _, role, _)| matches!(role, FolderRole::Sent | FolderRole::Drafts))
    }
}

fn sidebar_entries(accounts: &[AccountRow], folders: &[FolderRow]) -> Vec<SidebarEntry> {
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
