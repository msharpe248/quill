//! The bottom bar: the keys that apply right now, or a notice to dismiss.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::command::{self, CommandId};
use crate::keymap::KeySeq;
use crate::state::{Env, NoticeKind, State};
use crate::text;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    if area.height == 0 {
        return;
    }
    let styles = &env.styles;
    let width = usize::from(area.width);

    if let Some(notice) = state.notices.front() {
        let dismiss = env
            .keymap
            .keys_for(CommandId::NavBack)
            .first()
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
    let hints: Vec<(String, &str, usize, bool)> = command::hints(state.context())
        .iter()
        .filter_map(|hint| {
            let keys: Vec<String> = hint
                .commands
                .iter()
                .filter_map(|id| env.keymap.keys_for(*id).first())
                .map(ToString::to_string)
                .collect();
            if keys.is_empty() {
                return None;
            }
            let keys = keys.join("/");
            let piece = text::width(&keys) + 1 + text::width(hint.label) + 2;
            let is_help = hint.commands.contains(&CommandId::HelpToggle);
            Some((keys, hint.label, piece, is_help))
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
    }
}
