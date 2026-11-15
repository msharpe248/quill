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
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::event::{Event, EventStream};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::Print;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::command::{Context, Menu};
use crate::data::{self, DataEvent, DataHandle};
use crate::keymap::KeyChord;
use crate::state::{Action, Effects, Env, HostRequest, HostState, Notice, State};
use crate::view::ImagePlacement;
use crate::{sample, terminal, view};

/// Runs the UI over `store` until the user quits. `notices` are shown in the
/// status bar at startup, for problems found while loading configuration.
pub async fn run(
    store: Store,
    env: Env,
    mut notices: Vec<Notice>,
    attached: Attached,
) -> io::Result<()> {
    let (events_tx, mut events) = unbounded_channel();
    let data = step(
        "cannot start the store worker",
        data::spawn(store, events_tx),
    )?;
    let Attached {
        host,
        events: mut host_events,
        problem,
    } = attached;
    notices.extend(problem.map(Notice::error));

    let mut screen = step("cannot take over the terminal", terminal::enter())?;
    let (problems_tx, mut problems) = unbounded_channel();
    let mut io = Io {
        data: &data,
        host: &host,
        problems: problems_tx,
        preview_dir: None,
        last_host_state: None,
        images: Images::default(),
    };
    let result = event_loop(
        &mut screen,
        &env,
        notices,
        &mut io,
        &mut events,
        &mut host_events,
        &mut problems,
    )
    .await;
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
    /// Failures from background work, to show as notices.
    problems: UnboundedSender<String>,
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

impl Io<'_> {
    fn perform(&mut self, effects: Effects) {
        for request in effects.data {
            self.data.send(request);
        }
        for request in effects.host {
            match request {
                HostRequest::PreviewSample => self.preview_sample(),
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
        let problems = self.problems.clone();
        // Writing a file is not for the UI task.
        tokio::task::spawn_blocking(move || {
            let path = dir.join(sample::FILE_NAME);
            match sample::write(&dir, &path) {
                Ok(()) => host.preview(&path, sample::FILE_NAME),
                Err(err) => {
                    let _ = problems.send(format!("Cannot write the sample file: {err}"));
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
                Menu::View => bridge::Menu::View,
                Menu::Message => bridge::Menu::Message,
                Menu::Help => bridge::Menu::Help,
            }),
            symbol: spec.symbol.map(str::to_owned),
        })
        .collect()
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    env: &Env,
    notices: Vec<Notice>,
    io: &mut Io<'_>,
    events: &mut UnboundedReceiver<DataEvent>,
    host_events: &mut UnboundedReceiver<HostEvent>,
    problems: &mut UnboundedReceiver<String>,
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
            let mut images = Vec::new();
            let frame = step(
                "cannot draw",
                terminal.draw(|frame| images = view::render(frame, &state, env)),
            )?;
            let size = (frame.area.width, frame.area.height);
            step("cannot draw an image", io.images.sync(images, size))?;
            state.dirty = false;
        }
        io.mirror(&state, env);

        let action = tokio::select! {
            event = input.next() => match event {
                Some(Ok(event)) => translate(event),
                Some(Err(err)) => return step("cannot read input", Err(err)),
                None => return Ok(()),
            },
            Some(event) = events.recv() => Some(Action::Data(event)),
            Some(event) = host_events.recv() => match event {
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
            Some(text) = problems.recv() => Some(Action::Notify(Notice::error(text))),
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
