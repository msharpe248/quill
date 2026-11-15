//! The run loop: terminal events, store answers and host events in; frames,
//! store requests and host messages out.
//!
//! This task owns the terminal and never waits on disk or the network. The
//! store lives on its own thread (see [`crate::data`]), the host has its own
//! (see `quill-host`); everything here is `select`, `update`, draw.

use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use quill_host::bridge::{self, CommandInfo, StateUpdate};
use quill_host::{Attached, Host, HostEvent};
use quill_media::kitty;
use quill_store::Store;
use quill_sync::{SyncEvent, SyncHandle, SyncRequest};
use ratatui::DefaultTerminal;
use ratatui::backend::IntoCrossterm;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::event::{Event, EventStream};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Attribute, ContentStyle, Print, SetAttribute, SetStyle};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::command::{Context, Menu};
use crate::data::{self, DataEvent, DataHandle};
use crate::keymap::KeyChord;
use crate::setup::{AccountEvent, AccountRequest};
use crate::state::{Action, Effects, Env, HostRequest, HostState, Notice, State};
use crate::view::{ImagePlacement, LinkPlacement};
use crate::{hyperlink, sample, terminal, view};

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
    env: Env,
    mut notices: Vec<Notice>,
    attached: Attached,
    sync: SyncLink,
    accounts: AccountsLink,
) -> io::Result<()> {
    let (events_tx, events) = unbounded_channel();
    let data = step(
        "cannot start the store worker",
        data::spawn(store, events_tx),
    )?;
    let Attached {
        host,
        events: host_events,
        problem,
    } = attached;
    notices.extend(problem.map(Notice::error));

    let mut screen = step("cannot take over the terminal", terminal::enter())?;
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
        preview_dir: None,
        last_host_state: None,
        images: Images::default(),
    };
    let mut inbound = Inbound {
        data: events,
        host: host_events,
        sync: sync_events,
        accounts: account_events,
        injected,
    };
    let result = event_loop(&mut screen, &env, notices, &mut io, &mut inbound).await;
    io.images.forget();
    terminal::leave();
    if let Some(dir) = io.preview_dir {
        // Files written for viewing do not outlive the run.
        let _ = std::fs::remove_dir_all(dir);
    }
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
    preview_dir: Option<PathBuf>,
    last_host_state: Option<HostState>,
    images: Images,
}

/// What the terminal has been asked to draw over the frame.
#[derive(Default)]
struct Images {
    /// The sample image's pixels are with the terminal.
    transmitted: bool,
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
        for image in &wanted {
            if !self.transmitted {
                // The pixels go once; placing the image again is a few bytes.
                queue!(
                    out,
                    Print(kitty::transmit(image.id, &quill_media::sample::image()))
                )?;
                self.transmitted = true;
            }
            queue!(
                out,
                MoveTo(image.area.x, image.area.y),
                Print(kitty::place(image.id, image.area.width, image.area.height))
            )?;
        }
        out.flush()?;
        self.placed = wanted;
        self.size = size;
        Ok(())
    }

    /// Frees what the terminal is holding for us.
    fn forget(&mut self) {
        if self.transmitted {
            let mut out = stdout();
            let _ = queue!(out, Print(kitty::forget(view::SAMPLE_IMAGE_ID)));
            let _ = out.flush();
            self.transmitted = false;
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
                HostRequest::PreviewSample => self.preview_sample(),
                HostRequest::OpenUrl(url) => self.host.open_url(&url),
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
                (None, SyncRequest::SyncNow | SyncRequest::SetFlags { .. }) => {}
            }
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
                Context::Reader => bridge::Context::Reader,
                Context::Help => bridge::Context::Help,
                Context::Links => bridge::Context::Links,
                Context::Setup => bridge::Context::Setup,
                Context::Certificate => bridge::Context::Certificate,
            },
            enabled: current
                .enabled
                .iter()
                .filter_map(|id| env.registry.get(*id))
                .map(|spec| spec.name.to_owned())
                .collect(),
            unread: current.unread,
            title: current.title.clone(),
        });
        self.last_host_state = Some(current);
    }

    fn preview_sample(&mut self) {
        let dir = self
            .preview_dir
            .get_or_insert_with(|| {
                std::env::temp_dir().join(format!("quill-preview-{}", std::process::id()))
            })
            .clone();
        let host = Arc::clone(self.host);
        let injected = self.injected.clone();
        // Writing a file is not for the UI task.
        tokio::task::spawn_blocking(move || {
            let path = dir.join(sample::FILE_NAME);
            match sample::write(&dir, &path) {
                Ok(()) => host.preview(&path, sample::FILE_NAME),
                Err(err) => {
                    let _ = injected.send(Action::Notify(Notice::error(format!(
                        "Cannot write the sample file: {err}"
                    ))));
                }
            }
        });
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
                Menu::Mailbox => bridge::Menu::Mailbox,
                Menu::View => bridge::Menu::View,
                Menu::Message => bridge::Menu::Message,
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
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    env: &Env,
    notices: Vec<Notice>,
    io: &mut Io<'_>,
    inbound: &mut Inbound,
) -> io::Result<()> {
    let size = step("cannot read the terminal size", terminal.size())?;
    let mut state = State::new((size.width, size.height), now());
    for notice in notices {
        state.update(env, Action::Notify(notice));
    }
    io.host.commands(command_infos(env));
    for request in state.startup_requests() {
        io.data.send(request);
    }

    let mut input = EventStream::new();
    let mut clock = tokio::time::interval(Duration::from_secs(30));
    loop {
        // Redraw only on change.
        if state.dirty {
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

        let action = tokio::select! {
            event = input.next() => match event {
                Some(Ok(event)) => translate(event),
                Some(Err(err)) => return step("cannot read input", Err(err)),
                None => return Ok(()),
            },
            Some(event) = inbound.data.recv() => Some(Action::Data(event)),
            Some(event) = inbound.host.recv() => match event {
                HostEvent::RunCommand(name) => Some(match env.registry.by_name(&name) {
                    Some(spec) => Action::Command(spec.id),
                    None => Action::Notify(Notice::error(format!(
                        "The app asked for an unknown command \"{name}\"."
                    ))),
                }),
                // Either way there is nothing left to draw for.
                HostEvent::WillQuit | HostEvent::Disconnected => return Ok(()),
                HostEvent::Problem(text) => Some(Action::Notify(Notice::error(text))),
            },
            Some(event) = inbound.sync.recv() => Some(Action::Sync(event)),
            Some(event) = inbound.accounts.recv() => Some(Action::Account(event)),
            Some(action) = inbound.injected.recv() => Some(action),
            _ = clock.tick() => Some(Action::ClockTick(now())),
        };
        if let Some(action) = action {
            let effects = state.update(env, action);
            io.perform(effects);
        }
        if state.should_quit {
            return Ok(());
        }
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
            tz: TimeZone::UTC,
            reader_width: 0.5,
            images: false,
            accounts: false,
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
        // Quit is the shell's to offer.
        assert_eq!(find("app.quit").menu, None);
        assert_eq!(find("help.toggle").menu, Some(bridge::Menu::Help));
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
}
