//! A test harness that runs the real pipeline without a terminal or threads:
//! the demo mailbox in an in-memory store, `update` for every key, the store
//! worker's own `handle` for every request, and `view` into a test backend.

#![allow(dead_code)]

use std::collections::VecDeque;

use jiff::tz::TimeZone;
use quill_core::config::KeymapPreset;
use quill_store::Store;
use quill_tui::command::Registry;
use quill_tui::data::{self, DataRequest};
use quill_tui::files::Files;
use quill_tui::keymap::Keymap;
use quill_tui::state::{Action, Env, HostRequest, State};
use quill_tui::theme::{ColorMode, Styles, Theme};
use quill_tui::{demo, view};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

/// Saturday 2026-11-14T09:30:00Z.
pub const NOW: i64 = 1_794_648_600;

pub struct Harness {
    pub state: State,
    pub env: Env,
    pub store: Store,
    /// What the UI has asked of the host so far.
    pub host_requests: Vec<HostRequest>,
    /// What the UI has asked of the sync engine so far.
    pub sync_requests: Vec<quill_sync::SyncRequest>,
    /// What the UI has asked of the accounts service so far.
    pub account_requests: Vec<quill_tui::setup::AccountRequest>,
    /// Folders the store worker may write files in: one for viewing, one
    /// standing in for Downloads. Both go when the harness does.
    pub files: Files,
    _scratch: tempfile::TempDir,
}

impl Harness {
    /// The demo mailbox with the default keys and the dark theme.
    pub fn demo(width: u16, height: u16) -> Self {
        Self::build(
            width,
            height,
            Keymap::preset(KeymapPreset::Vim),
            ColorMode::TrueColor,
        )
    }

    /// The demo mailbox in a terminal that draws pictures, and with a host
    /// that makes thumbnails if `thumbnails`. Set from the start, because
    /// the first message is read as the mailbox opens.
    pub fn with_pictures(width: u16, height: u16, thumbnails: bool) -> Self {
        let mut store = Store::open_in_memory().unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(demo::seed(&mut store, NOW))
            .unwrap();
        Self::over_with(
            store,
            width,
            height,
            Keymap::preset(KeymapPreset::Vim),
            ColorMode::TrueColor,
            false,
            |env| {
                env.images = true;
                env.thumbnails = thumbnails;
            },
        )
    }

    /// The demo mailbox, with a service to add accounts through.
    pub fn with_accounts(width: u16, height: u16) -> Self {
        let mut harness = Self::demo(width, height);
        harness.env.accounts = true;
        harness
    }

    /// No accounts at all, and a service to add one through: a first run.
    pub fn first_run(width: u16, height: u16) -> Self {
        Self::over(
            Store::open_in_memory().unwrap(),
            width,
            height,
            Keymap::preset(KeymapPreset::Vim),
            ColorMode::TrueColor,
            true,
        )
    }

    pub fn build(width: u16, height: u16, keymap: Keymap, mode: ColorMode) -> Self {
        let mut store = Store::open_in_memory().unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(demo::seed(&mut store, NOW))
            .unwrap();
        Self::over(store, width, height, keymap, mode, false)
    }

    fn over(
        store: Store,
        width: u16,
        height: u16,
        keymap: Keymap,
        mode: ColorMode,
        accounts: bool,
    ) -> Self {
        Self::over_with(store, width, height, keymap, mode, accounts, |_| {})
    }

    fn over_with(
        store: Store,
        width: u16,
        height: u16,
        keymap: Keymap,
        mode: ColorMode,
        accounts: bool,
        adjust: impl FnOnce(&mut Env),
    ) -> Self {
        let mut env = Env {
            registry: Registry::builtin(),
            keymap,
            styles: Styles::new(Theme::builtin("dark").unwrap(), mode),
            tz: TimeZone::UTC,
            reader_width: 0.5,
            images: false,
            accounts,
            // As in Quill's own window.
            cmd_keys: true,
            // Tests name files by path unless they say otherwise.
            mouse: true,
            file_panel: false,
            thumbnails: false,
            downloads: None,
            compose: quill_tui::state::ComposeSettings::default(),
            identities: std::collections::HashMap::new(),
            remote_never: false,
            notifications: true,
            threads: true,
        };
        adjust(&mut env);
        let scratch = tempfile::tempdir().unwrap();
        let mut harness = Self {
            state: State::new((width, height), NOW),
            env,
            store,
            host_requests: Vec::new(),
            sync_requests: Vec::new(),
            account_requests: Vec::new(),
            files: Files {
                view_dir: Some(scratch.path().join("view")),
                downloads: Some(scratch.path().join("Downloads")),
            },
            _scratch: scratch,
        };
        let requests = harness.state.startup_requests();
        harness.pump(requests);
        harness
    }

    /// Applies one action, then answers store requests until none are left.
    pub fn act(&mut self, action: Action) {
        let effects = self.state.update(&self.env, action);
        self.host_requests.extend(effects.host);
        self.sync_requests.extend(effects.sync);
        self.account_requests.extend(effects.accounts);
        self.pump(effects.data);
    }

    pub fn pump(&mut self, requests: Vec<DataRequest>) {
        let mut queue: VecDeque<DataRequest> = requests.into();
        while let Some(request) = queue.pop_front() {
            for event in data::handle_with(&mut self.store, &self.files, request) {
                let effects = self.state.update(&self.env, Action::Data(event));
                self.host_requests.extend(effects.host);
                self.sync_requests.extend(effects.sync);
                self.account_requests.extend(effects.accounts);
                queue.extend(effects.data);
            }
        }
    }

    /// Presses keys written as in the help overlay, separated by spaces:
    /// `"1 j j enter"`. A two-key sequence is just two keys: `"g g"`.
    pub fn keys(&mut self, keys: &str) {
        for key in keys.split_whitespace() {
            self.act(Action::Key(key.parse().unwrap()));
        }
    }

    /// Types text a character at a time, as key presses.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.act(Action::Key(quill_tui::keymap::KeyChord::plain(
                quill_tui::keymap::Key::Char(c),
            )));
        }
    }

    /// Where the terminal's cursor is shown, if it is.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let (width, height) = self.state.size;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut position = None;
        terminal
            .draw(|frame| {
                view::render(frame, &self.state, &self.env);
            })
            .unwrap();
        if terminal.backend().cursor_visible() {
            let at = terminal.get_cursor_position().unwrap();
            position = Some((at.x, at.y));
        }
        position
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.act(Action::Resize(width, height));
    }

    pub fn buffer(&self) -> Buffer {
        let (width, height) = self.state.size;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                view::render(frame, &self.state, &self.env);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// What the view asks the terminal to draw over the frame: pictures.
    pub fn placements(&self) -> Vec<quill_tui::view::ImagePlacement> {
        let (width, height) = self.state.size;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut images = Vec::new();
        terminal
            .draw(|frame| {
                images = view::render(frame, &self.state, &self.env).images;
            })
            .unwrap();
        images
    }

    /// The screen as text, one line per row, trailing spaces trimmed.
    pub fn screen(&self) -> String {
        let buffer = self.buffer();
        let width = usize::from(buffer.area.width);
        let mut out = String::new();
        for row in buffer.content.chunks(width.max(1)) {
            let mut line = String::new();
            // A wide character owns the cell after it, which holds a filler.
            let mut covered = 0;
            for cell in row {
                if covered == 0 {
                    line.push_str(cell.symbol());
                }
                covered = covered
                    .max(quill_tui::text::width(cell.symbol()))
                    .saturating_sub(1);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// The row of the screen that contains `needle`.
    pub fn row_with(&self, needle: &str) -> String {
        self.screen()
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("nothing on screen contains {needle:?}"))
            .to_owned()
    }

    pub fn selected_subject(&self) -> &str {
        &self.state.list.rows[self.state.list.selected]
            .headers
            .subject
    }
}
