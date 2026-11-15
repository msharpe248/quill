//! The help overlay: every command that applies where help was opened, with
//! its keys, grouped by kind. Built from the registry and the live keymap, so
//! it cannot drift from what the keys actually do.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::command::{Category, Context};
use crate::state::{Env, State};
use crate::text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    Heading(&'static str),
    Entry { keys: String, title: &'static str },
    Blank,
}

/// The overlay's content for `context`.
pub fn lines(env: &Env, context: Context) -> Vec<HelpLine> {
    let mut lines = Vec::new();
    for category in Category::ALL {
        let entries: Vec<HelpLine> = env
            .registry
            .specs()
            .iter()
            .filter(|spec| spec.category == category && spec.applies_in(context))
            .map(|spec| {
                let keys: Vec<String> = env
                    .keymap
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
                }
            })
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

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    let Some(help) = state.help else {
        return;
    };
    let styles = &env.styles;
    let lines = lines(env, help.context);
    let mut block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(
            format!(" Help: {} ", help.context.title()),
            styles.border(true),
        ));
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
            HelpLine::Entry { keys, title } => Line::from(vec![
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
