//! The bottom bar: the keys that apply right now, or a notice to dismiss.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::command::{self, CommandId};
use crate::keymap::KeySeq;
use crate::state::{Env, NoticeKind, State};
use crate::text;

const SYNCING: &str = "syncing… ";

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    if area.height == 0 {
        return;
    }
    let styles = &env.styles;
    let width = usize::from(area.width);

    if let Some(notice) = state.notices.front() {
        let dismiss = env
            .hint_key(CommandId::NavBack)
            .map_or_else(String::new, |key| format!("  [{key} to dismiss]"));
        let more = match state.notices.len() - 1 {
            0 => String::new(),
            more => format!(" (+{more} more)"),
        };
        let tail = format!("{more}{dismiss}");
        // The message gives way before the instructions for dismissing it.
        let room = width.saturating_sub(1 + text::width(&tail));
        let style = match notice.kind {
            NoticeKind::Info => styles.accent(),
            NoticeKind::Error => styles.error(),
        };
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled(text::truncate(&notice.text, room), style),
            Span::styled(tail, styles.dim()),
        ]);
        frame.render_widget(line, area);
        return;
    }

    // Keys typed so far of a sequence, on the right.
    let pending = if state.pending.is_empty() {
        String::new()
    } else {
        format!("{} … ", KeySeq(state.pending.clone()))
    };
    let budget = width.saturating_sub(text::width(&pending));

    // Each hint as (keys, label, width). A command the user's keys.toml
    // unbound has nothing to advertise and is left out.
    // A message that is in Vim has its own short list: the rest of what
    // writing offers waits for it to come back.
    let in_vim = state.context() == command::Context::Compose
        && state
            .compose
            .as_ref()
            .is_some_and(|compose| compose.away.is_some());
    let offered = if in_vim {
        command::hints_in_vim()
    } else if state.form_in_sheet() {
        // The form or the question is in a sheet of the host's, with
        // buttons of its own: the keys that work it here are not what is
        // pressed there.
        &[]
    } else {
        command::hints(state.context())
    };
    let hints: Vec<(String, &str, usize, bool)> = offered
        .iter()
        // A hint for a command that mostly has nothing to act on is kept
        // back until it has.
        .filter(|hint| {
            !hint.only_enabled || hint.commands.iter().any(|id| state.is_enabled(env, *id))
        })
        .filter_map(|hint| {
            let keys: Vec<String> = hint
                .commands
                .iter()
                .filter_map(|id| env.hint_key(*id))
                .map(ToString::to_string)
                .collect();
            if keys.is_empty() {
                return None;
            }
            let keys = keys.join("/");
            // The editor a message can be handed to, and the way back
            // from it, are called by that editor's name.
            let label = if hint.commands == [CommandId::ComposeVim] {
                env.external.name()
            } else if in_vim && hint.commands == [CommandId::NavBack] {
                env.external.close_hint()
            } else {
                hint.label
            };
            let piece = text::width(&keys) + 1 + text::width(label) + 2;
            let is_help = hint.commands.contains(&CommandId::HelpToggle);
            Some((keys, label, piece, is_help))
        })
        .collect();
    // The way to the full list of keys keeps its place however narrow the
    // bar: its room is set aside first, and the rest fill what remains.
    let reserved: usize = hints
        .iter()
        .filter(|(_, _, _, is_help)| *is_help)
        .map(|(_, _, piece, _)| *piece)
        .sum();
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1 + reserved;
    let mut full = false;
    for (keys, label, piece, is_help) in hints {
        if !is_help {
            full |= used + piece > budget;
            if full {
                continue;
            }
            used += piece;
        } else if used > budget {
            continue;
        }
        spans.push(Span::styled(keys, styles.key()));
        spans.push(Span::styled(format!(" {label}  "), styles.dim()));
    }
    frame.render_widget(Line::from(spans), area);
    if !pending.is_empty() {
        frame.render_widget(Line::styled(pending, styles.key()).right_aligned(), area);
    } else if let Some((flash, undoable)) = &state.flash {
        // What the last key did, and how to take it back.
        let undo = env
            .hint_key(CommandId::EditUndo)
            .filter(|_| *undoable)
            .map_or_else(String::new, |key| format!(" {key} undoes it."));
        let said = format!("{flash}{undo} ");
        if used + text::width(&said) < width {
            frame.render_widget(Line::styled(said, styles.accent()).right_aligned(), area);
        } else {
            // Too narrow for the hints as well: what just happened matters
            // more than what could be pressed next.
            let said = text::truncate(&format!(" {said}"), width);
            frame.render_widget(ratatui::widgets::Clear, area);
            frame.buffer_mut().set_style(area, styles.base());
            frame.render_widget(Line::styled(said, styles.accent()), area);
        }
    } else if !state.syncing.is_empty() && used + SYNCING.len() < width {
        // Work in the background shows here and blocks nothing.
        frame.render_widget(Line::styled(SYNCING, styles.dim()).right_aligned(), area);
    }
}
