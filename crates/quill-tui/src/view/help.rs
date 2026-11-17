//! The help overlay: every command that applies where help was opened, with
//! its keys, grouped by kind; or every key Quill has, wherever it works.
//! Built from the registry and the live keymap, so it cannot drift from
//! what the keys actually do.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::command::{Category, CommandId, CommandSpec, Context, Registry};
use crate::keymap::Keymap;
use crate::state::{Env, Help, State};
use crate::text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    Heading(&'static str),
    Entry {
        /// The command's keys in a line, or that it has none.
        keys: String,
        title: &'static str,
        id: CommandId,
    },
    Blank,
}

/// What the list of every key is called, on the overlay and in print.
pub const ALL_KEYS_TITLE: &str = "Keyboard shortcuts";

/// The places the list of every key is set out by, after the panes: what
/// is particular to each, under its own heading.
const ELSEWHERE: [(&str, Context); 2] = [
    ("Writing a message", Context::Compose),
    ("In the list of attachments", Context::Attachments),
];

/// The panes mail is read in.
const READING: [Context; 3] = [Context::Sidebar, Context::List, Context::Reader];

fn entry(keymap: &Keymap, spec: &CommandSpec) -> HelpLine {
    let keys: Vec<String> = keymap
        .keys_for(spec.id)
        .iter()
        .map(ToString::to_string)
        .collect();
    HelpLine::Entry {
        keys: if keys.is_empty() {
            "(unbound)".to_owned()
        } else {
            keys.join(", ")
        },
        title: spec.title,
        id: spec.id,
    }
}

/// Web reading needs the host's web view pane. Where there is none, as in
/// a terminal, its key does nothing and is not listed.
fn listed(spec: &CommandSpec, web_pane: bool) -> bool {
    spec.id != CommandId::ViewWebReading || web_pane
}

/// The commands that apply in any of `contexts`, under a heading for each
/// kind that has any.
fn by_category(
    registry: &Registry,
    keymap: &Keymap,
    web_pane: bool,
    contexts: &[Context],
) -> Vec<HelpLine> {
    let mut lines = Vec::new();
    for category in Category::ALL {
        let entries: Vec<HelpLine> = registry
            .specs()
            .iter()
            .filter(|spec| spec.category == category)
            .filter(|spec| contexts.iter().any(|context| spec.applies_in(*context)))
            .filter(|spec| listed(spec, web_pane))
            .map(|spec| entry(keymap, spec))
            .collect();
        if entries.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(HelpLine::Blank);
        }
        lines.push(HelpLine::Heading(category.title()));
        lines.extend(entries);
    }
    lines
}

/// The overlay's content for `context`.
pub fn lines(env: &Env, context: Context) -> Vec<HelpLine> {
    by_category(&env.registry, &env.keymap, env.web_pane, &[context])
}

/// Every key Quill has: the ones for reading mail by kind, as help has
/// them, then what is particular to writing a message and to the list of
/// a message's attachments. A command is listed once, where it is first
/// of use.
pub fn all_lines(registry: &Registry, keymap: &Keymap, web_pane: bool) -> Vec<HelpLine> {
    let mut lines = by_category(registry, keymap, web_pane, &READING);
    let mut seen: Vec<CommandId> = registry
        .specs()
        .iter()
        .filter(|spec| READING.iter().any(|context| spec.applies_in(*context)))
        .map(|spec| spec.id)
        .collect();
    for (heading, context) in ELSEWHERE {
        let particular: Vec<&CommandSpec> = registry
            .specs()
            .iter()
            .filter(|spec| spec.applies_in(context) && !seen.contains(&spec.id))
            .filter(|spec| listed(spec, web_pane))
            .collect();
        if particular.is_empty() {
            continue;
        }
        lines.push(HelpLine::Blank);
        lines.push(HelpLine::Heading(heading));
        for spec in particular {
            seen.push(spec.id);
            lines.push(entry(keymap, spec));
        }
    }
    lines
}

/// The list of every key as plain text, for printing: a heading on a line
/// of its own, and under it each command's keys and what it does.
pub fn all_text(registry: &Registry, keymap: &Keymap, web_pane: bool) -> String {
    let lines = all_lines(registry, keymap, web_pane);
    let keys_width = lines
        .iter()
        .filter_map(|line| match line {
            HelpLine::Entry { keys, .. } => Some(text::width(keys)),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for line in &lines {
        match line {
            HelpLine::Blank => {}
            HelpLine::Heading(title) => out.push_str(title),
            HelpLine::Entry { keys, title, .. } => {
                out.push_str(&format!("  {}  {title}", text::fit(keys, keys_width)));
            }
        }
        out.push('\n');
    }
    out
}

/// What the overlay shows: the help for where it was opened, or every key.
pub fn shown(env: &Env, help: &Help) -> Vec<HelpLine> {
    if help.all {
        all_lines(&env.registry, &env.keymap, env.web_pane)
    } else {
        lines(env, help.context)
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(help) = state.help else {
        return;
    };
    let styles = &env.styles;
    let lines = shown(env, &help);
    let title = if help.all {
        format!(" {ALL_KEYS_TITLE} ")
    } else {
        format!(" Help: {} ", help.context.title())
    };
    let mut block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(title, styles.border(true)));
    let viewport = usize::from(area.height.saturating_sub(2));
    if lines.len() > viewport && viewport > 0 {
        let first = help.scroll + 1;
        let last = (help.scroll + viewport).min(lines.len());
        block = block.title_bottom(
            Line::styled(format!(" {first}-{last} of {} ", lines.len()), styles.dim())
                .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = usize::from(inner.width);
    let keys_width = lines
        .iter()
        .filter_map(|line| match line {
            HelpLine::Entry { keys, .. } => Some(text::width(keys)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(width / 2);
    let visible = lines
        .iter()
        .skip(help.scroll)
        .take(usize::from(inner.height));
    for (row, line) in visible.enumerate() {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let line = match line {
            HelpLine::Blank => continue,
            HelpLine::Heading(title) => Line::styled(*title, styles.heading()),
            HelpLine::Entry { keys, title, .. } => Line::from(vec![
                Span::raw("  "),
                Span::styled(text::fit(keys, keys_width), styles.key()),
                Span::raw("  "),
                Span::styled(
                    text::truncate(title, width.saturating_sub(keys_width + 4)),
                    styles.text(),
                ),
            ]),
        };
        frame.render_widget(line, rect);
    }
}
