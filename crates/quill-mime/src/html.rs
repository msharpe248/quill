//! HTML mail as styled text for a terminal.
//!
//! The aim is a message someone can read, not a page: headings, lists,
//! emphasis, quotes and simple tables survive, links are numbered and listed
//! at the end, and layout, colour, scripts and remote content are left
//! behind. Nothing is fetched and nothing is run.
//!
//! The text that comes back is safe to draw: no control characters and no
//! escape sequences, however the message spelt them. A link whose text
//! names one place while it leads to another gets a line under it saying
//! where it really goes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use html2text::render::{TaggedLine, TextDecorator};

use crate::disguise::{self, Disguise};
use crate::{clean, guard, plain};

/// One line of a rendered message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

/// A run of text drawn one way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    /// The link this text belongs to, as a place in [`Rendered::links`].
    pub link: Option<usize>,
}

/// A message body laid out for a width.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub lines: Vec<Line>,
    /// Where the numbered links lead: link `[1]` is `links[0]`.
    pub links: Vec<String>,
}

/// How a span is drawn. What each flag looks like is the theme's business.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Code and preformatted text.
    pub code: bool,
    /// The text of a link.
    pub link: bool,
    /// Asides: link numbers, the list of links, image descriptions, and a
    /// plain-text signature.
    pub dim: bool,
    /// How deeply the line is quoted: 0 for the sender's own words, 1 for
    /// what they are answering, and so on. Every span of a line has the
    /// same depth, the `>` markers included.
    pub quote: u8,
}

impl Line {
    /// A line of plain text.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            return Self::default();
        }
        Self {
            spans: vec![Span {
                text,
                style: Style::default(),
                link: None,
            }],
        }
    }

    /// The line without its styling.
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// Whether the line shows nothing: no characters, or only spaces and
    /// the invisible padding marketing mail is full of.
    pub fn is_blank(&self) -> bool {
        self.spans
            .iter()
            .all(|span| span.text.chars().all(is_invisible))
    }

    fn push(&mut self, text: &str, style: Style, link: Option<usize>) {
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style && last.link == link => last.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_owned(),
                style,
                link,
            }),
        }
    }

    /// Removes and returns the spaces the line ends with.
    fn take_trailing_space(&mut self) -> String {
        let Some(last) = self.spans.last_mut() else {
            return String::new();
        };
        let kept = last.text.trim_end_matches(' ').len();
        let gap = last.text.split_off(kept);
        if last.text.is_empty() {
            self.spans.pop();
        }
        gap
    }

    fn trim_end(&mut self) {
        while let Some(last) = self.spans.last_mut() {
            let kept = last.text.trim_end_matches(is_invisible).len();
            last.text.truncate(kept);
            if !last.text.is_empty() {
                break;
            }
            self.spans.pop();
        }
    }
}

/// HTML larger than this is not rendered: the reader falls back to the
/// message's plain text rather than stall on a pathological document.
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

/// The narrowest the text is laid out. A narrower pane clips it.
pub(crate) const MIN_WIDTH: usize = 20;

/// Renders `html` as lines no wider than `width` columns where the content
/// allows. Returns `None` when there is nothing to show or the document
/// cannot be rendered, so the caller can fall back to plain text: one that
/// is too large, or nested so deeply that laying it out would take longer
/// than anyone would wait.
pub fn render(html: &str, width: usize) -> Option<Rendered> {
    if html.len() > MAX_HTML_BYTES {
        return None;
    }
    let width = width.max(MIN_WIDTH);
    let links = Rc::new(RefCell::new(Links::default()));
    let decorator = Decorator {
        links: Rc::clone(&links),
    };
    let mut config = html2text::config::with_decorator(decorator)
        // For `style="display:none"`, which is all of the message's CSS
        // that is left by the time the renderer looks: see `guard`.
        .use_doc_css()
        // A table that cannot fit is clipped by the pane, not refused.
        .allow_width_overflow()
        // Struck-out text is marked by its style. Left to itself the
        // renderer also puts a combining stroke after every character,
        // which no search would ever match.
        .unicode_strikeout(false);
    if is_laid_out_with_tables(html) {
        // Tables used for layout read best as one block after another.
        config = config.raw_mode(true);
    }
    // Escape sequences go before the parser can take them apart.
    let html = clean::markup(html);
    // Parsed against the clock, then stripped of what the renderer cannot
    // be trusted with and measured before it is laid out: see `guard`.
    let dom = config.parse_html(guard::Source::new(&html)).ok()?;
    if !guard::prepare(&dom) {
        return None;
    }
    let tree = config.dom_to_render_tree(&dom).ok()?;
    let tagged = config.render_to_lines(tree, width).ok()?;
    let urls = std::mem::take(&mut links.borrow_mut().urls);
    let rendered = assemble(&tagged, &urls, width);
    (!rendered.lines.is_empty()).then_some(rendered)
}

/// `html` as plain text, for search and for quoting in a reply.
pub fn to_text(html: &str) -> Option<String> {
    let rendered = render(html, 78)?;
    let mut text = String::new();
    for line in rendered.lines {
        text.push_str(&line.text());
        text.push('\n');
    }
    Some(text)
}

/// Marketing mail builds its layout out of nested tables. A message with one
/// table inside another, or a table marked as presentation, has no data
/// table worth drawing as a grid.
fn is_laid_out_with_tables(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    if lower.contains("role=\"presentation\"") || lower.contains("role='presentation'") {
        return true;
    }
    let mut depth = 0usize;
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("table") {
        let before = &rest[..at];
        if before.ends_with("</") {
            depth = depth.saturating_sub(1);
        } else if before.ends_with('<') {
            depth += 1;
            if depth > 1 {
                return true;
            }
        }
        rest = &rest[at + "table".len()..];
    }
    false
}

/// What html2text is asked to remember about a piece of text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Mark {
    #[default]
    None,
    /// A link, by its place in `Links::urls`.
    Link(usize),
    Bold,
    Italic,
    Strike,
    Code,
    Image,
}

#[derive(Debug, Default)]
struct Links {
    /// Every distinct target, in the order first met.
    urls: Vec<String>,
    /// Where each target is in `urls`.
    known: HashMap<String, usize>,
    /// The links currently open, innermost last. `None` is a link that goes
    /// nowhere worth numbering.
    open: Vec<Option<usize>>,
}

/// Marks the number html2text is asked to lay out after a link. Zero width
/// and never typed, so it cannot be mistaken for the message's own text.
const MARKER: char = '\u{2060}';

#[derive(Debug, Clone)]
struct Decorator {
    links: Rc<RefCell<Links>>,
}

impl TextDecorator for Decorator {
    type Annotation = Mark;

    fn decorate_link_start(&mut self, url: &str) -> (String, Mark) {
        // The address is shown in the list of links, so it is cleaned like
        // any other text from the message.
        let url = clean::header(url);
        let url = url.trim();
        let mut links = self.links.borrow_mut();
        if !is_worth_numbering(url) {
            links.open.push(None);
            return (String::new(), Mark::None);
        }
        let id = match links.known.get(url) {
            Some(id) => *id,
            None => {
                let id = links.urls.len();
                links.urls.push(url.to_owned());
                links.known.insert(url.to_owned(), id);
                id
            }
        };
        links.open.push(Some(id));
        (String::new(), Mark::Link(id))
    }

    fn decorate_link_end(&mut self) -> String {
        // The number is laid out with the text so that wrapping leaves room
        // for it. `assemble` gives it its final value, or removes it.
        match self.links.borrow_mut().open.pop().flatten() {
            Some(id) => format!("{MARKER}[{}]", id + 1),
            None => String::new(),
        }
    }

    fn decorate_em_start(&self) -> (String, Mark) {
        (String::new(), Mark::Italic)
    }

    fn decorate_em_end(&self) -> String {
        String::new()
    }

    fn decorate_strong_start(&self) -> (String, Mark) {
        (String::new(), Mark::Bold)
    }

    fn decorate_strong_end(&self) -> String {
        String::new()
    }

    fn decorate_strikeout_start(&self) -> (String, Mark) {
        (String::new(), Mark::Strike)
    }

    fn decorate_strikeout_end(&self) -> String {
        String::new()
    }

    fn decorate_code_start(&self) -> (String, Mark) {
        (String::new(), Mark::Code)
    }

    fn decorate_code_end(&self) -> String {
        String::new()
    }

    fn decorate_preformat_first(&self) -> Mark {
        Mark::Code
    }

    fn decorate_preformat_cont(&self) -> Mark {
        Mark::Code
    }

    fn decorate_image(&mut self, _src: &str, title: &str) -> (String, Mark) {
        // Only an image that says what it is gets a mention.
        (format!("[{}]", title.trim()), Mark::Image)
    }

    // html2text measures these prefixes in bytes, so they stay ASCII.
    fn header_prefix(&self, level: usize) -> String {
        "#".repeat(level) + " "
    }

    fn quote_prefix(&self) -> String {
        "> ".to_owned()
    }

    fn unordered_item_prefix(&self) -> String {
        "* ".to_owned()
    }

    fn ordered_item_prefix(&self, i: i64) -> String {
        format!("{i}. ")
    }

    fn make_subblock_decorator(&self) -> Self {
        self.clone()
    }

    fn finalise(&mut self, _urls: Vec<String>) -> Vec<TaggedLine<Mark>> {
        Vec::new()
    }
}

/// A link to somewhere a reader could go. Anchors within the page and
/// script links are not, however the word "javascript" is spaced out.
fn is_worth_numbering(url: &str) -> bool {
    let lower: String = url
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    !(lower.is_empty()
        || lower.starts_with('#')
        || lower.starts_with("javascript:")
        || lower.starts_with("vbscript:"))
}

/// Characters that take up a place in the text and show nothing: spaces,
/// and the zero-width padding hidden previews are made of.
fn is_invisible(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{00AD}' | '\u{034F}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}'
        )
}

/// Turns html2text's lines into ours: text cleaned of anything a terminal
/// would act on, styles from the marks, final link numbers, headings in
/// bold, quote depths, runs of blank lines closed up, warnings under links
/// that are not what they say, and the list of links at the end.
fn assemble(tagged: &[TaggedLine<Vec<Mark>>], urls: &[String], width: usize) -> Rendered {
    let mut numbering = Numbering::new(urls.len());
    let mut shown = Shown::default();

    let mut lines: Vec<Line> = Vec::new();
    let mut after_heading = false;
    for tagged_line in tagged {
        let mut line = Line::default();
        // The links found out on this line.
        let mut disguised: Vec<(usize, Disguise)> = Vec::new();
        for piece in tagged_line.tagged_strings() {
            let mut style = Style::default();
            let mut link = None;
            for mark in &piece.tag {
                match mark {
                    Mark::None => {}
                    Mark::Link(id) => {
                        style.link = true;
                        link = Some(*id);
                    }
                    Mark::Bold => style.bold = true,
                    Mark::Italic => style.italic = true,
                    Mark::Strike => style.strike = true,
                    Mark::Code => style.code = true,
                    Mark::Image => style.dim = true,
                }
            }

            // An entity can spell any character, so the text is cleaned
            // here, after the parser has decoded it. A span is part of one
            // line: what is left of a tab or a line break is a space.
            let text = clean::body(&piece.s);
            let text = match text.contains(['\n', '\t']) {
                true => text.replace(['\n', '\t'], " "),
                false => text.into_owned(),
            };
            let mut rest = text.as_str();
            while let Some(at) = rest.find(MARKER) {
                let before = &rest[..at];
                let target = numbering.note(link, before);
                shown.text(link, before);
                line.push(before, style, target);
                rest = &rest[at + MARKER.len_utf8()..];

                // "[12]" follows the marker unless wrapping tore it apart.
                let Some(id) = rest
                    .strip_prefix('[')
                    .and_then(|after| after.split_once(']'))
                    .and_then(|(digits, _)| digits.parse::<usize>().ok())
                    .and_then(|number| number.checked_sub(1))
                    .filter(|id| *id < urls.len())
                else {
                    continue;
                };
                rest = rest.split_once(']').map_or("", |(_, after)| after);
                disguised.extend(shown.end(id, urls));
                // A link with nothing to read, such as one around an image
                // with no description, is not worth a number.
                let Some(number) = numbering.close(id) else {
                    continue;
                };
                let aside = Style {
                    dim: true,
                    ..Style::default()
                };
                // The number sits against the link's last word, whatever
                // space the link's text ended with.
                let gap = line.take_trailing_space();
                line.push(&format!("[{number}]"), aside, Some(number - 1));
                line.push(&gap, style, None);
            }
            let target = numbering.note(link, rest);
            shown.text(link, rest);
            line.push(rest, style, target);
        }
        line.trim_end();
        let heading = embolden_heading(&mut line);
        mark_quote(&mut line);
        // One blank line between blocks is enough, and none to start with.
        let blank = line.is_blank();
        if blank && lines.last().is_none_or(Line::is_blank) {
            continue;
        }
        // A heading stands clear of what it heads.
        if after_heading && !blank && !heading {
            lines.push(Line::default());
        }
        after_heading = heading;
        lines.push(if blank { Line::default() } else { line });
        for (id, disguise) in disguised {
            warn(&mut lines, &numbering, id, &disguise, width);
        }
    }
    // A link whose end was torn apart by wrapping is still judged.
    if let Some((id, disguise)) = shown.finish(urls) {
        warn(&mut lines, &numbering, id, &disguise, width);
    }
    while lines.last().is_some_and(Line::is_blank) {
        lines.pop();
    }

    if !lines.is_empty() && !numbering.numbered.is_empty() {
        let aside = Style {
            dim: true,
            ..Style::default()
        };
        lines.push(Line::default());
        for (index, id) in numbering.numbered.iter().enumerate() {
            let mut line = Line::default();
            line.push(
                &format!("[{}] {}", index + 1, urls[*id]),
                aside,
                Some(index),
            );
            lines.push(line);
        }
    }
    Rendered {
        lines,
        links: numbering
            .numbered
            .iter()
            .map(|id| urls[*id].clone())
            .collect(),
    }
}

/// Longer than any name a link could be showing.
const MAX_NAME_BYTES: usize = 1024;

/// What the links have shown so far, so that each can be compared with
/// where it leads once its text is complete.
#[derive(Default)]
struct Shown {
    /// The link being read: its place in the list of targets and its text
    /// so far.
    open: Option<(usize, String)>,
    /// Whether that link has been judged since it last showed any text.
    judged: bool,
    /// Whether other text has been shown since it was judged.
    apart: bool,
}

impl Shown {
    /// Notes `text` shown by `link`, or by no link.
    fn text(&mut self, link: Option<usize>, text: &str) {
        if text.chars().all(is_invisible) {
            return;
        }
        let Some(id) = link else {
            self.apart = true;
            return;
        };
        match &mut self.open {
            // More of the same link, or the same target again with nothing
            // between: one name as far as a reader can see, so it is judged
            // as one. Splitting a name over two links hides nothing.
            Some((open, shown)) if *open == id && !(self.judged && self.apart) => {
                if self.judged {
                    // Everything up to the last space has been judged, and
                    // a name has no spaces in it. Only the word that may
                    // still be going on is kept, and that only while it
                    // could be a name: a page of a thousand links to one
                    // place must not be read a thousand times over.
                    let word = shown
                        .char_indices()
                        .rev()
                        .find(|(_, c)| c.is_whitespace())
                        .map_or(0, |(at, c)| at + c.len_utf8());
                    shown.drain(..word);
                    if shown.len() > MAX_NAME_BYTES {
                        shown.clear();
                    }
                }
                shown.push_str(text);
            }
            _ => self.open = Some((id, text.to_owned())),
        }
        self.judged = false;
        self.apart = false;
    }

    /// The link `id` has ended. Returns it if it is not what it says.
    fn end(&mut self, id: usize, urls: &[String]) -> Option<(usize, Disguise)> {
        let (open, shown) = self.open.as_ref()?;
        if *open != id || self.judged {
            return None;
        }
        self.judged = true;
        self.apart = false;
        let found = disguise::find(shown, urls.get(id)?)?;
        Some((id, found))
    }

    /// The document has ended, perhaps with a link still open.
    fn finish(&mut self, urls: &[String]) -> Option<(usize, Disguise)> {
        let (id, _) = self.open.as_ref()?;
        let id = *id;
        self.end(id, urls)
    }
}

/// Adds the line that says where a disguised link really goes. It is text,
/// so it survives a terminal with no colour, and bold, so it is not taken
/// for part of the message.
fn warn(
    lines: &mut Vec<Line>,
    numbering: &Numbering,
    id: usize,
    disguise: &Disguise,
    width: usize,
) {
    let Some(number) = numbering.numbers.get(id).copied().flatten() else {
        return;
    };
    let loud = Style {
        bold: true,
        ..Style::default()
    };
    // Rows after the first are set in, so a long warning reads as one.
    let rows = plain::wrap_text(&disguise.warning(number), width.saturating_sub(2));
    for (index, row) in rows.iter().enumerate() {
        let mut line = Line::default();
        let indent = if index == 0 { "" } else { "  " };
        line.push(&format!("{indent}{row}"), loud, Some(number - 1));
        lines.push(line);
    }
}

/// Gives every span of a quoted line its depth. A line of code that
/// happens to start with `>` is a prompt, not a quote.
fn mark_quote(line: &mut Line) {
    let Some(first) = line.spans.first() else {
        return;
    };
    if first.style.code {
        return;
    }
    let (depth, _) = plain::quote_prefix(&first.text);
    if depth > 0 {
        for span in &mut line.spans {
            span.style.quote = depth;
        }
    }
}

/// Which links get a number. Only a link with something to read counts,
/// and links are numbered in the order their text first appears.
struct Numbering {
    /// Whether each link has shown any text since its last number.
    has_text: Vec<bool>,
    /// The number each link ends up with, from 1.
    numbers: Vec<Option<usize>>,
    /// The numbered links, in order.
    numbered: Vec<usize>,
}

impl Numbering {
    fn new(links: usize) -> Self {
        Self {
            has_text: vec![false; links],
            numbers: vec![None; links],
            numbered: Vec::new(),
        }
    }

    /// Notes that link `id` showed `text`. Returns the link's place in the
    /// final list, once it has one.
    fn note(&mut self, id: Option<usize>, text: &str) -> Option<usize> {
        let id = id.filter(|id| *id < self.numbers.len())?;
        if !text.chars().all(is_invisible) {
            self.has_text[id] = true;
            if self.numbers[id].is_none() {
                self.numbered.push(id);
                self.numbers[id] = Some(self.numbered.len());
            }
        }
        self.numbers[id].map(|number| number - 1)
    }

    /// Ends one use of link `id`. Returns its number if that use had
    /// anything to read.
    fn close(&mut self, id: usize) -> Option<usize> {
        if std::mem::take(self.has_text.get_mut(id)?) {
            self.numbers[id]
        } else {
            None
        }
    }
}

/// A heading arrives as `# Title`. It keeps the marker, which reads well in
/// a terminal, and gains weight. Returns whether the line was a heading.
fn embolden_heading(line: &mut Line) -> bool {
    let Some(first) = line.spans.first() else {
        return false;
    };
    let hashes = first.text.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) || !first.text[hashes..].starts_with(' ') {
        return false;
    }
    for span in &mut line.spans {
        span.style.bold = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(html: &str, width: usize) -> Vec<String> {
        render(html, width)
            .unwrap_or_default()
            .lines
            .iter()
            .map(Line::text)
            .collect()
    }

    fn styled(line: &Line) -> Vec<(&str, Style)> {
        line.spans
            .iter()
            .map(|span| (span.text.as_str(), span.style))
            .collect()
    }

    const BOLD: Style = Style {
        bold: true,
        italic: false,
        strike: false,
        code: false,
        link: false,
        dim: false,
        quote: 0,
    };
    const DIM: Style = Style { dim: true, ..NONE };
    const LINK: Style = Style { link: true, ..NONE };
    const NONE: Style = Style {
        bold: false,
        italic: false,
        strike: false,
        code: false,
        link: false,
        dim: false,
        quote: 0,
    };

    #[test]
    fn paragraphs_wrap_and_are_separated() {
        assert_eq!(
            text(
                "<p>Your hold is ready at the Riverside branch.</p><p>We keep it seven days.</p>",
                24
            ),
            [
                "Your hold is ready at",
                "the Riverside branch.",
                "",
                "We keep it seven days."
            ]
        );
    }

    #[test]
    fn emphasis_becomes_style() {
        let lines = render("<p>The <b>agenda</b> is <em>nearly</em> done.</p>", 60)
            .unwrap_or_default()
            .lines;
        assert_eq!(
            styled(&lines[0]),
            [
                ("The ", NONE),
                ("agenda", BOLD),
                (" is ", NONE),
                (
                    "nearly",
                    Style {
                        italic: true,
                        ..NONE
                    }
                ),
                (" done.", NONE),
            ]
        );
    }

    #[test]
    fn struck_out_text_is_styled_and_still_the_same_text() {
        let lines = render("<p>Was <s>forty</s>, now <del>thirty</del>.</p>", 60)
            .unwrap_or_default()
            .lines;
        let struck = Style {
            strike: true,
            ..NONE
        };
        assert_eq!(
            styled(&lines[0]),
            [
                ("Was ", NONE),
                ("forty", struck),
                (", now ", NONE),
                ("thirty", struck),
                (".", NONE),
            ]
        );
    }

    #[test]
    fn script_links_are_not_numbered_however_they_are_spelt() {
        let html = "<p><a href=\"javascript:steal()\">one</a> <a href=\"jaVa&#9;sCript:steal()\">two</a> \
                    <a href=\" java\nscript:steal()\">three</a> <a href=\"vbscript:steal()\">four</a> \
                    <a href=\"#top\">five</a> <a href=\"\">six</a> <a href=\"https://example.com/\">seven</a></p>";
        let rendered = render(html, 80).unwrap_or_default();
        assert_eq!(rendered.links, ["https://example.com/"]);
        assert_eq!(text(html, 80)[0], "one two three four five six seven[1]");
    }

    #[test]
    fn headings_lists_and_quotes_keep_their_shape() {
        let html = "<h1>Offsite</h1><ul><li>Travel</li><li>Rooms</li></ul>\
                    <ol><li>First</li><li>Second</li></ol>\
                    <blockquote>Can we move it a week?</blockquote>";
        let lines = render(html, 60).unwrap_or_default().lines;
        let shown: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(
            shown,
            [
                "# Offsite",
                "",
                "* Travel",
                "* Rooms",
                "1. First",
                "2. Second",
                "",
                "> Can we move it a week?"
            ]
        );
        assert_eq!(styled(&lines[0]), [("# Offsite", BOLD)]);
    }

    #[test]
    fn links_are_numbered_and_listed_once() {
        let html = "<p>See the <a href=\"https://example.com/agenda\">agenda</a> and the \
                    <a href=\"https://example.com/rooms\">room list</a>. The \
                    <a href=\"https://example.com/agenda\">agenda</a> again.</p>";
        let lines = render(html, 100).unwrap_or_default().lines;
        assert_eq!(
            lines.iter().map(Line::text).collect::<Vec<_>>(),
            [
                "See the agenda[1] and the room list[2]. The agenda[1] again.",
                "",
                "[1] https://example.com/agenda",
                "[2] https://example.com/rooms",
            ]
        );
        assert_eq!(
            styled(&lines[0])[..4],
            [
                ("See the ", NONE),
                ("agenda", LINK),
                ("[1]", DIM),
                (" and the ", NONE)
            ]
        );
        assert_eq!(styled(&lines[2]), [("[1] https://example.com/agenda", DIM)]);
    }

    #[test]
    fn link_text_numbers_and_the_list_all_know_their_target() {
        let html = "<p>See the <a href=\"https://example.com/agenda\">agenda</a> and \
                    <a href=\"https://example.com/\"><img src=\"logo.png\"></a>\
                    <a href=\"https://example.com/rooms\">rooms</a>.</p>";
        let rendered = render(html, 100).unwrap_or_default();
        // The logo's link has nothing to read, so it is not in the list and
        // the numbers have no gap.
        assert_eq!(
            rendered.links,
            ["https://example.com/agenda", "https://example.com/rooms"]
        );
        let targets: Vec<(&str, Option<usize>)> = rendered.lines[0]
            .spans
            .iter()
            .map(|span| (span.text.as_str(), span.link))
            .collect();
        assert_eq!(
            targets,
            [
                ("See the ", None),
                ("agenda", Some(0)),
                ("[1]", Some(0)),
                (" and ", None),
                ("rooms", Some(1)),
                ("[2]", Some(1)),
                (".", None),
            ]
        );
        assert_eq!(rendered.lines[2].spans[0].link, Some(0));
        assert_eq!(rendered.lines[3].spans[0].link, Some(1));
    }

    #[test]
    fn a_link_number_sits_against_the_last_word_of_the_link() {
        let html = "<p><a href=\"https://example.com/a\">Track your order </a>\
                    <a href=\"https://example.com/b\">or change it</a> today.</p>";
        assert_eq!(
            text(html, 80)[0],
            "Track your order[1] or change it[2] today."
        );
    }

    #[test]
    fn a_link_with_nothing_to_read_gets_no_number() {
        // A logo that links home, a tracking pixel, and an anchor.
        let html = "<a href=\"https://example.com/\"><img src=\"logo.png\"></a>\
                    <a href=\"#top\">Back to top</a>\
                    <p>Read <a href=\"https://example.com/news\">the news</a>.</p>\
                    <img src=\"https://example.com/pixel.gif\" width=\"1\" height=\"1\">";
        assert_eq!(
            text(html, 60),
            [
                "Back to top",
                "",
                "Read the news[1].",
                "",
                "[1] https://example.com/news"
            ]
        );
    }

    #[test]
    fn an_image_shows_its_description() {
        let lines = render(
            "<p>Look: <img src=\"a.jpg\" alt=\"Sunset over the bay\"></p>",
            60,
        )
        .unwrap_or_default()
        .lines;
        assert_eq!(
            styled(&lines[0]),
            [("Look: ", NONE), ("[Sunset over the bay]", DIM)]
        );
    }

    #[test]
    fn scripts_styles_and_hidden_text_are_left_out() {
        let html = "<html><head><style>p { color: red }</style><title>Hi</title></head><body>\
                    <div style=\"display:none\">Preview text you were not meant to see</div>\
                    <p>Hello.</p><script>alert(1)</script></body></html>";
        assert_eq!(text(html, 60), ["Hello."]);
    }

    #[test]
    fn invisible_padding_does_not_make_lines() {
        let html = "<div>&zwnj;&nbsp;&zwnj;&nbsp;&#847;&nbsp;</div><p>One.</p>\
                    <p>&nbsp;</p><p>&nbsp;</p><p>Two.</p>";
        assert_eq!(text(html, 60), ["One.", "", "Two."]);
    }

    #[test]
    fn a_simple_table_is_drawn_as_a_grid() {
        let html = "<table><tr><th>Item</th><th>Cost</th></tr>\
                    <tr><td>Train</td><td>48</td></tr></table>";
        let shown = text(html, 40).join("\n");
        assert!(shown.contains("Item") && shown.contains('│'), "{shown}");
        let train = shown.lines().find(|line| line.contains("Train"));
        assert!(train.is_some_and(|line| line.contains("48")), "{shown}");
    }

    #[test]
    fn layout_tables_read_as_blocks() {
        let html = "<table role=\"presentation\"><tr><td><table><tr>\
                    <td>Black Friday starts early.</td><td>Shop the sale.</td>\
                    </tr></table></td></tr></table>";
        assert_eq!(
            text(html, 60),
            ["Black Friday starts early.", "Shop the sale."]
        );
    }

    #[test]
    fn nothing_to_show_is_none() {
        assert_eq!(render("", 80), None);
        assert_eq!(
            render("<html><body><script>x()</script></body></html>", 80),
            None
        );
        assert_eq!(render("<p>&nbsp;</p>", 80), None);
    }

    #[test]
    fn hostile_input_is_survived() {
        // Deep nesting, a table too wide for the pane, a width of nothing,
        // and tags that never close.
        let deep = "<div>".repeat(5_000) + "deep" + &"</div>".repeat(5_000);
        assert!(text(&deep, 40).iter().any(|line| line.contains("deep")));
        let wide = format!("<table><tr>{}</tr></table>", "<td>cell</td>".repeat(200));
        assert!(render(&wide, 10).is_some());
        assert!(render("<p>narrow</p>", 0).is_some());
        assert!(render("<b><i><a href='x'>unclosed", 40).is_some());
        assert_eq!(render(&"x".repeat(MAX_HTML_BYTES + 1), 80), None);
    }

    #[test]
    fn nesting_too_deep_to_lay_out_is_refused_not_attempted() {
        // Each of these would take minutes at full size, or the stack. None
        // is shown as HTML; the reader falls back to the plain text.
        for tag in ["div", "b", "blockquote", "sup", "span"] {
            let deep = format!("<{tag}>").repeat(7_000) + "deep";
            assert_eq!(render(&deep, 80), None, "{tag}");
        }
        // Many small pieces of text, each inside a few thousand elements.
        let heavy = "<i>".repeat(3_000) + &"<b>x</b> ".repeat(2_000);
        assert_eq!(render(&heavy, 80), None);
        // A long flat document is none of those things.
        let flat = "<p>One <b>paragraph</b> of many.</p>".repeat(20_000);
        assert_eq!(
            render(&flat, 80).map(|shown| shown.lines.len()),
            Some(39_999)
        );
    }

    #[test]
    fn a_document_that_cannot_be_parsed_in_time_is_abandoned() {
        // A hundred thousand elements, each inside the last: hours of work
        // for the parser at two megabytes, and minutes at this size.
        let deep = "<div>".repeat(100_000) + "deep";
        let started = std::time::Instant::now();
        assert_eq!(render(&deep, 80), None);
        assert!(started.elapsed() < 4 * guard::allowance(deep.len()));

        // Forty kilobytes that would become four million elements: every
        // paragraph reopens every unclosed `<b>` before it.
        let growing: String = (0..3_000).map(|n| format!("<b id={n}><p>x")).collect();
        let started = std::time::Instant::now();
        assert_eq!(render(&growing, 80), None);
        assert!(started.elapsed() < 4 * guard::allowance(growing.len()));
    }

    #[test]
    fn escape_sequences_never_reach_a_span() {
        // Written raw, and spelt with references so that only the parser
        // sees them for what they are.
        let html = "<p>\u{1b}]0;pwned\u{7}Hello &#27;[2J<b>&#x1b;]52;c;cm0gLXJmIH4=&#7;Robin</b>\
                    &#0;&#8;&#127;.</p>\
                    <p><a href=\"https://example.com/&#27;[31m&#x202e;a\">the &#27;]8;;https://evil.example/&#27;\\agenda</a></p>\
                    <pre>one&#13;two</pre>\
                    <p><img src=\"a.png\" alt=\"&#27;]0;x&#7;A chart\"></p>";
        let rendered = render(html, 60).unwrap_or_default();
        assert_eq!(
            rendered.lines.iter().map(Line::text).collect::<Vec<_>>(),
            [
                "Hello Robin\u{fffd}.",
                "",
                "the agenda[1]",
                "",
                "one",
                "two",
                "",
                "[A chart]",
                "",
                "[1] https://example.com/a",
            ]
        );
        assert_eq!(rendered.links, ["https://example.com/a"]);
        let text = to_text(html).unwrap_or_default();
        assert!(!text.contains(|c: char| clean::is_control(c) && c != '\n'));

        // Whatever the parser makes of a reference, no control character
        // comes out, in text or in a link.
        let every: String = (0u32..=0x9f)
            .map(|code| {
                format!("<a href=\"https://example.com/&#{code};\">a&#{code};b</a> &#x{code:x}; ")
            })
            .collect();
        let rendered = render(&every, 60).unwrap_or_default();
        assert!(!rendered.links.is_empty());
        for text in rendered
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| &span.text)
            .chain(&rendered.links)
        {
            assert!(!text.contains(clean::is_control), "{text:?}");
        }
    }

    #[test]
    fn quoted_blocks_know_their_depth() {
        let html = "<p>Sounds good.</p><blockquote>Can we <b>move</b> it?\
                    <blockquote>To Friday.</blockquote></blockquote>\
                    <p>&gt; typed by hand</p><pre>&gt; ls</pre>";
        let lines = render(html, 60).unwrap_or_default().lines;
        let depths: Vec<(String, u8)> = lines
            .iter()
            .filter(|line| !line.is_blank())
            .map(|line| {
                let depth = line.spans[0].style.quote;
                assert!(line.spans.iter().all(|span| span.style.quote == depth));
                (line.text(), depth)
            })
            .collect();
        assert_eq!(
            depths,
            [
                ("Sounds good.".to_owned(), 0),
                ("> Can we move it?".to_owned(), 1),
                // The gap between the two is part of the quote.
                (">".to_owned(), 1),
                ("> > To Friday.".to_owned(), 2),
                ("> typed by hand".to_owned(), 1),
                // A prompt in a block of code is not a quote.
                ("> ls".to_owned(), 0),
            ]
        );
    }

    /// The warnings a document draws, each as one string however many rows
    /// it took.
    fn warnings(html: &str, width: usize) -> Vec<String> {
        let rendered = render(html, width).unwrap_or_default();
        let mut found: Vec<String> = Vec::new();
        let mut inside = false;
        for line in &rendered.lines {
            let text = line.text();
            let loud = line.spans.len() == 1 && line.spans[0].style == BOLD;
            if loud && text.starts_with("! [") {
                found.push(text.clone());
                inside = true;
            } else if loud && inside && text.starts_with("  ") {
                if let Some(last) = found.last_mut() {
                    last.push(' ');
                    last.push_str(text.trim_start());
                }
            } else {
                inside = false;
                continue;
            }
            // It opens what the link opens, and fits the pane.
            assert!(
                line.spans[0]
                    .link
                    .is_some_and(|at| at < rendered.links.len())
            );
            assert!(text.chars().count() <= width.max(MIN_WIDTH), "{text:?}");
        }
        found
    }

    #[test]
    fn a_link_that_names_another_place_says_where_it_goes() {
        let html = "<p>Your account is locked. Sign in at \
                    <a href=\"https://evil.example/login\">www.mybank.example</a> to unlock it.</p>";
        let rendered = render(html, 80).unwrap_or_default();
        assert_eq!(
            rendered.lines.iter().map(Line::text).collect::<Vec<_>>(),
            [
                "Your account is locked. Sign in at www.mybank.example[1] to unlock it.",
                "! [1] goes to evil.example, not www.mybank.example",
                "",
                "[1] https://evil.example/login",
            ]
        );
        // Text, so it is there without colour; bold, and it opens what the
        // link opens.
        assert_eq!(
            styled(&rendered.lines[1]),
            [("! [1] goes to evil.example, not www.mybank.example", BOLD)]
        );
        assert_eq!(rendered.lines[1].spans[0].link, Some(0));
    }

    #[test]
    fn each_kind_of_disguise_is_warned_about() {
        for (text, href, warning) in [
            (
                "https://mybank.example/login",
                "http://203.0.113.9/login",
                "! [1] goes to 203.0.113.9, not mybank.example",
            ),
            (
                "<b>mybank</b>.example",
                "https://mybank.example.evil.example/",
                "! [1] goes to mybank.example.evil.example, not mybank.example",
            ),
            (
                "myb&#x430;nk.example",
                "https://mybank.example/",
                "! [1] goes to mybank.example, not myb\u{430}nk.example",
            ),
            (
                "www.my&#8203;bank&NoBreak;.example",
                "https://evil.example/",
                "! [1] goes to evil.example, not www.mybank.example",
            ),
            (
                "https://mybank.example@evil.example/",
                "https://mybank.example@evil.example/",
                "! [1] goes to evil.example",
            ),
            (
                "ana@example.com",
                "mailto:eve@example.com",
                "! [1] goes to eve@example.com, not ana@example.com",
            ),
            (
                "www.mybank.example",
                "file:///Applications/Calculator.app",
                "! [1] goes to file:///Applications/Calculator.app, not www.mybank.example",
            ),
        ] {
            let html = format!("<p>Sign in: <a href=\"{href}\">{text}</a> now.</p>");
            assert_eq!(warnings(&html, 100), [warning], "{html}");
        }
    }

    #[test]
    fn honest_links_are_not_warned_about() {
        for (text, href) in [
            ("click here", "https://evil.example/"),
            ("Sign in to MyBank", "https://evil.example/"),
            ("www.example.com", "https://www.example.com/sale?id=1"),
            ("example.com", "https://www.example.com/"),
            ("example.com", "https://mail.example.com/"),
            ("https://example.com/agenda", "https://example.com/agenda"),
            ("ana@example.com", "mailto:ana@example.com"),
            ("the report.pdf", "https://example.com/files/report.pdf"),
        ] {
            let html = format!("<p>See <a href=\"{href}\">{text}</a> today.</p>");
            assert!(warnings(&html, 100).is_empty(), "{html}");
        }
    }

    #[test]
    fn a_disguise_is_seen_however_the_link_is_laid_out() {
        // Wrapped over several rows, in a quote, in a table, twice over,
        // and split across two links to the same place.
        let wrapped = "<p>Please confirm your details by signing in to your account at \
                       <a href=\"https://evil.example/\">https://www.mybank.example/accounts/confirm/details</a></p>";
        for width in [20, 30, 50, 80] {
            assert_eq!(
                warnings(wrapped, width),
                ["! [1] goes to evil.example, not www.mybank.example"],
                "at {width}"
            );
        }
        let quoted =
            "<blockquote><a href=\"https://evil.example/\">mybank.example</a></blockquote>";
        assert_eq!(
            warnings(quoted, 60),
            ["! [1] goes to evil.example, not mybank.example"]
        );
        let table = "<table><tr><td>Bank</td><td><a href=\"https://evil.example/\">mybank.example</a></td></tr></table>";
        assert_eq!(warnings(table, 60).len(), 1);
        let twice = "<p><a href=\"https://evil.example/\">mybank.example</a> and \
                     <a href=\"https://evil.example/\">click here</a> and \
                     <a href=\"https://evil.example/\">mybank.example</a></p>";
        assert_eq!(warnings(twice, 100).len(), 2);
        let split = "<p><a href=\"https://evil.example/\">www.mybank</a>\
                     <a href=\"https://evil.example/\">.example</a></p>";
        assert!(
            warnings(split, 100)
                .iter()
                .any(|warning| warning == "! [1] goes to evil.example, not www.mybank.example"),
            "{:?}",
            warnings(split, 100)
        );
        // Split over three, with words either side.
        let pieces = "<p><a href=\"https://evil.example/\">Sign in at www.</a>\
                      <a href=\"https://evil.example/\">mybank</a>\
                      <a href=\"https://evil.example/\">.example today</a></p>";
        assert!(
            warnings(pieces, 100)
                .iter()
                .any(|warning| warning == "! [1] goes to evil.example, not www.mybank.example"),
            "{:?}",
            warnings(pieces, 100)
        );
        // A marker of the message's own inside the text changes nothing.
        let forged = "<p><a href=\"https://evil.example/\">www.mybank&#x2060;[1].example</a></p>";
        assert!(!warnings(forged, 100).is_empty());
    }

    #[test]
    fn a_page_of_links_to_one_place_is_read_once_through() {
        // Each link's text is judged with what ran into it from the link
        // before. That must not add up.
        let html = "<a href=\"https://example.com/\">example.com/</a>".repeat(20_000);
        let started = std::time::Instant::now();
        let rendered = render(&html, 80).unwrap_or_default();
        assert!(started.elapsed().as_secs() < 30);
        assert_eq!(rendered.links, ["https://example.com/"]);
    }

    #[test]
    fn plain_text_of_html() {
        assert_eq!(
            to_text("<p>Hello <b>Robin</b>.</p><p>Bye.</p>").as_deref(),
            Some("Hello Robin.\n\nBye.\n")
        );
    }
}
