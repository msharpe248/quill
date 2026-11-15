//! HTML mail as styled text for a terminal.
//!
//! The aim is a message someone can read, not a page: headings, lists,
//! emphasis, quotes and simple tables survive, links are numbered and listed
//! at the end, and layout, colour, scripts and remote content are left
//! behind. Nothing is fetched and nothing is run.
//!
//! The text that comes back is still untrusted. The UI strips control
//! characters from every span before drawing it.

use std::cell::RefCell;
use std::rc::Rc;

use html2text::render::{TaggedLine, TextDecorator};

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
    /// Asides: link numbers, the list of links, image descriptions.
    pub dim: bool,
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

    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_owned(),
                style,
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
const MIN_WIDTH: usize = 20;

/// Renders `html` as lines no wider than `width` columns where the content
/// allows. Returns `None` when there is nothing to show or the document
/// cannot be rendered, so the caller can fall back to plain text.
pub fn render(html: &str, width: usize) -> Option<Vec<Line>> {
    if html.len() > MAX_HTML_BYTES {
        return None;
    }
    let links = Rc::new(RefCell::new(Links::default()));
    let decorator = Decorator {
        links: Rc::clone(&links),
    };
    let mut config = html2text::config::with_decorator(decorator)
        .use_doc_css()
        // A table that cannot fit is clipped by the pane, not refused.
        .allow_width_overflow();
    if is_laid_out_with_tables(html) {
        // Tables used for layout read best as one block after another.
        config = config.raw_mode(true);
    }
    let tagged = config
        .lines_from_read(html.as_bytes(), width.max(MIN_WIDTH))
        .ok()?;
    let urls = std::mem::take(&mut links.borrow_mut().urls);
    let lines = assemble(&tagged, &urls);
    (!lines.is_empty()).then_some(lines)
}

/// `html` as plain text, for search and for quoting in a reply.
pub fn to_text(html: &str) -> Option<String> {
    let lines = render(html, 78)?;
    let mut text = String::new();
    for line in lines {
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
        let url = url.trim();
        let mut links = self.links.borrow_mut();
        if !is_worth_numbering(url) {
            links.open.push(None);
            return (String::new(), Mark::None);
        }
        let id = match links.urls.iter().position(|known| known == url) {
            Some(id) => id,
            None => {
                links.urls.push(url.to_owned());
                links.urls.len() - 1
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
/// script links are not.
fn is_worth_numbering(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    !(url.is_empty() || lower.starts_with('#') || lower.starts_with("javascript:"))
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

/// Turns html2text's lines into ours: styles from the marks, final link
/// numbers, headings in bold, runs of blank lines closed up, and the list
/// of links at the end.
fn assemble(tagged: &[TaggedLine<Vec<Mark>>], urls: &[String]) -> Vec<Line> {
    // Whether each link has shown any text since its last number, and the
    // number each link ends up with: only links that are referred to count.
    let mut has_text = vec![false; urls.len()];
    let mut numbers: Vec<Option<usize>> = vec![None; urls.len()];
    let mut numbered: Vec<usize> = Vec::new();

    let mut lines: Vec<Line> = Vec::new();
    let mut after_heading = false;
    for tagged_line in tagged {
        let mut line = Line::default();
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

            let mut rest = piece.s.as_str();
            while let Some(at) = rest.find(MARKER) {
                let before = &rest[..at];
                if let Some(id) = link
                    && !before.chars().all(is_invisible)
                    && let Some(seen) = has_text.get_mut(id)
                {
                    *seen = true;
                }
                line.push(before, style);
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
                // A link with nothing to read, such as one around an image
                // with no description, is not worth a number.
                if !std::mem::take(&mut has_text[id]) {
                    continue;
                }
                let number = *numbers[id].get_or_insert_with(|| {
                    numbered.push(id);
                    numbered.len()
                });
                let aside = Style {
                    dim: true,
                    ..Style::default()
                };
                // The number sits against the link's last word, whatever
                // space the link's text ended with.
                let gap = line.take_trailing_space();
                line.push(&format!("[{number}]"), aside);
                line.push(&gap, style);
            }
            if let Some(id) = link
                && !rest.chars().all(is_invisible)
                && let Some(seen) = has_text.get_mut(id)
            {
                *seen = true;
            }
            line.push(rest, style);
        }
        line.trim_end();
        let heading = embolden_heading(&mut line);
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
    }
    while lines.last().is_some_and(Line::is_blank) {
        lines.pop();
    }

    if !lines.is_empty() && !numbered.is_empty() {
        let aside = Style {
            dim: true,
            ..Style::default()
        };
        lines.push(Line::default());
        for (index, id) in numbered.iter().enumerate() {
            let mut line = Line::default();
            line.push(&format!("[{}] {}", index + 1, urls[*id]), aside);
            lines.push(line);
        }
    }
    lines
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
        let lines =
            render("<p>The <b>agenda</b> is <em>nearly</em> done.</p>", 60).unwrap_or_default();
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
    fn headings_lists_and_quotes_keep_their_shape() {
        let html = "<h1>Offsite</h1><ul><li>Travel</li><li>Rooms</li></ul>\
                    <ol><li>First</li><li>Second</li></ol>\
                    <blockquote>Can we move it a week?</blockquote>";
        let lines = render(html, 60).unwrap_or_default();
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
        let lines = render(html, 100).unwrap_or_default();
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
        .unwrap_or_default();
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
    fn plain_text_of_html() {
        assert_eq!(
            to_text("<p>Hello <b>Robin</b>.</p><p>Bye.</p>").as_deref(),
            Some("Hello Robin.\n\nBye.\n")
        );
    }
}
