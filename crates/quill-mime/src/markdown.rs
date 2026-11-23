//! Markdown as HTML for the mail Quill sends.
//!
//! The reader of this HTML is somebody else's mail program, so it is kept
//! plain: headings, lists, emphasis, links, quotes, code and simple tables,
//! with the few inline styles that make quotes and code readable there. It
//! carries no scripts, no stylesheets and nothing a mail program would
//! fetch.
//!
//! The writer is composing mail, not a web page. HTML they type is shown as
//! they typed it, never passed through, and a line break they typed is a
//! line break in the result.
//!
//! Only what the writer wrote is Markdown. A message they are answering or
//! passing on is somebody else's words, which were never written as
//! Markdown for this message and are not read as it: a line that begins
//! with `>`, and everything after the line a forward puts above the
//! original ([`FORWARDED`]). Those go out as the text they are, a quote
//! behind its bar, so that an asterisk or an underscore in them is an
//! asterisk or an underscore to whoever reads the reply.

use pulldown_cmark::{Alignment, Event, LinkType, Options, Parser, Tag, TagEnd};
use quill_core::links::is_openable;

use crate::reply::FORWARDED;

/// Quoted text: a bar down the left, as mail programs draw it. No colours
/// beyond the bar, so it reads the same on a dark background.
const QUOTE_STYLE: &str = "margin:0 0 1em 0;padding:0 0 0 1ex;border-left:2px solid #cccccc";
/// Code keeps its line breaks but may wrap, so one long line does not make
/// the whole message scroll sideways.
const CODE_STYLE: &str = "white-space:pre-wrap;font-family:monospace";
const TABLE_STYLE: &str = "border-collapse:collapse";
const CELL_STYLE: &str = "border:1px solid #cccccc;padding:2px 6px";

/// How deep quotes are drawn one inside another. A quote deeper than this
/// is drawn at this depth: nobody reads a bar for each of a hundred
/// answers, and no message gets to ask for a thousand.
const DEEPEST_QUOTE: usize = 12;

/// The HTML for a message body written as Markdown: a whole document, with
/// `\n` line endings.
pub fn to_html(markdown: &str) -> String {
    let mut writer = Writer {
        out: String::with_capacity(markdown.len() * 3 / 2 + 128),
        ..Writer::default()
    };
    writer
        .out
        .push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n</head>\n<body>\n");
    for piece in pieces(markdown) {
        match piece {
            Piece::Written(text) => {
                // Tables are the one extension: they cannot be mistaken
                // for anything else. Strikethrough is left off because
                // "~5 or ~10 minutes" is ordinary prose.
                for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
                    writer.event(event);
                }
            }
            Piece::Quoted(lines) => quoted(&mut writer.out, &lines),
            Piece::Passed(lines) => as_text(&mut writer.out, &lines),
        }
    }
    writer.out.push_str("</body>\n</html>\n");
    writer.out
}

/// A run of lines of a message body, by whose words they are.
enum Piece<'a> {
    /// The writer's own: Markdown.
    Written(&'a str),
    /// Lines that begin with `>`: a message being answered, as text.
    Quoted(Vec<&'a str>),
    /// Lines after a forward's own line: a message passed on, as text.
    Passed(Vec<&'a str>),
}

/// `body` as runs of lines, each the writer's own, quoted, or passed on.
///
/// A line that begins with `>` inside a code block the writer fenced is
/// code, as they meant it.
fn pieces(body: &str) -> Vec<Piece<'_>> {
    let mut pieces: Vec<Piece<'_>> = Vec::new();
    // Where the run of the writer's own lines now being gathered began.
    let mut written: Option<usize> = None;
    let mut fence: Option<char> = None;
    let mut passed = false;
    let mut at = 0;
    for line in body.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let words = line.trim_end_matches(['\n', '\r']);
        let quote = fence.is_none() && is_quote(words);
        if !passed && !quote {
            if fence.is_none() && words.trim_end() == FORWARDED {
                passed = true;
            } else {
                if let Some(mark) = fence_mark(words) {
                    fence = match fence {
                        None => Some(mark),
                        Some(open) if open == mark => None,
                        open => open,
                    };
                }
                written.get_or_insert(start);
                continue;
            }
        }
        if let Some(from) = written.take() {
            pieces.push(Piece::Written(&body[from..start]));
        }
        match (quote, pieces.last_mut()) {
            (true, Some(Piece::Quoted(lines))) | (false, Some(Piece::Passed(lines))) => {
                lines.push(words);
            }
            (true, _) => pieces.push(Piece::Quoted(vec![words])),
            (false, _) => pieces.push(Piece::Passed(vec![words])),
        }
    }
    if let Some(from) = written {
        pieces.push(Piece::Written(&body[from..]));
    }
    pieces
}

/// Whether a line is a quoted one: `>` first, after no more than the three
/// spaces Markdown allows before it.
fn is_quote(line: &str) -> bool {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    indent <= 3 && line[indent..].starts_with('>')
}

/// The character a line opens or closes a fenced code block with, if it
/// is a fence.
fn fence_mark(line: &str) -> Option<char> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    let rest = &line[indent..];
    if indent > 3 {
        None
    } else if rest.starts_with("```") {
        Some('`')
    } else if rest.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// How deep a quoted line is, and its words: `>> older` is 2 and "older".
fn quote_line(line: &str) -> (usize, &str) {
    let mut rest = line.trim_start_matches(' ');
    let mut depth = 0;
    loop {
        if let Some(after) = rest.strip_prefix('>') {
            depth += 1;
            rest = after;
        } else if let Some(after) = rest
            .strip_prefix(' ')
            .filter(|after| after.starts_with('>'))
        {
            // "> > older", as some programs write it.
            rest = after;
        } else {
            break;
        }
    }
    (depth, rest.strip_prefix(' ').unwrap_or(rest).trim_end())
}

/// Quoted lines as text behind their bars, one bar for each level.
fn quoted(out: &mut String, lines: &[&str]) {
    // Marks with nothing quoted after any of them are not a quote: they
    // are the characters the writer typed.
    if lines.iter().all(|line| quote_line(line).1.is_empty()) {
        as_text(out, lines);
        return;
    }
    let mut depth = 0;
    let mut paragraph = Paragraph::default();
    for line in lines {
        let (wanted, words) = quote_line(line);
        let wanted = wanted.min(DEEPEST_QUOTE);
        if wanted != depth {
            paragraph.close(out);
        }
        while depth < wanted {
            out.push_str("<blockquote style=\"");
            out.push_str(QUOTE_STYLE);
            out.push_str("\">\n");
            depth += 1;
        }
        while depth > wanted {
            out.push_str("</blockquote>\n");
            depth -= 1;
        }
        paragraph.line(out, words);
    }
    paragraph.close(out);
    for _ in 0..depth {
        out.push_str("</blockquote>\n");
    }
}

/// Lines as the text they are: a paragraph for each run of them, and a
/// line break where there was one.
fn as_text(out: &mut String, lines: &[&str]) {
    let mut paragraph = Paragraph::default();
    for line in lines {
        paragraph.line(out, line.trim_end());
    }
    paragraph.close(out);
}

/// A paragraph of text that is not Markdown, a line at a time.
#[derive(Default)]
struct Paragraph {
    open: bool,
}

impl Paragraph {
    /// One more line of it. An empty line ends it.
    fn line(&mut self, out: &mut String, words: &str) {
        if words.is_empty() {
            self.close(out);
            return;
        }
        out.push_str(if self.open { "<br>\n" } else { "<p>" });
        self.open = true;
        escape_into(out, words);
    }

    fn close(&mut self, out: &mut String) {
        if self.open {
            out.push_str("</p>\n");
            self.open = false;
        }
    }
}

#[derive(Default)]
struct Writer {
    out: String,
    /// One entry for each link or image that is open: whether it became an
    /// `<a>`, and how long the output was when it opened.
    links: Vec<OpenLink>,
    /// The raw HTML block being collected, to be shown as text.
    raw_block: Option<String>,
    table: Vec<Alignment>,
    in_table_head: bool,
    cell: usize,
}

struct OpenLink {
    anchored: bool,
    /// What to show when the link turns out to have no text of its own.
    fallback: String,
    text_starts: usize,
}

impl Writer {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => escape_into(&mut self.out, &text),
            Event::Code(code) => {
                self.out.push_str("<code>");
                escape_into(&mut self.out, &code);
                self.out.push_str("</code>");
            }
            // HTML the writer typed. Shown, not obeyed.
            Event::Html(html) => match &mut self.raw_block {
                Some(block) => block.push_str(&html),
                None => escape_into(&mut self.out, &html),
            },
            Event::InlineHtml(html) => escape_into(&mut self.out, &html),
            Event::SoftBreak | Event::HardBreak => self.out.push_str("<br>\n"),
            Event::Rule => self.out.push_str("<hr>\n"),
            // Extensions that are not switched on. Should one arrive, its
            // text is still the writer's text.
            Event::InlineMath(text) | Event::DisplayMath(text) | Event::FootnoteReference(text) => {
                escape_into(&mut self.out, &text);
            }
            Event::TaskListMarker(done) => {
                self.out.push_str(if done { "[x] " } else { "[ ] " });
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.out.push_str("<p>"),
            Tag::Heading { level, .. } => {
                self.out.push('<');
                self.out.push_str(&level.to_string());
                self.out.push('>');
            }
            Tag::BlockQuote(_) => {
                self.out.push_str("<blockquote style=\"");
                self.out.push_str(QUOTE_STYLE);
                self.out.push_str("\">\n");
            }
            Tag::CodeBlock(_) => {
                self.out.push_str("<pre style=\"");
                self.out.push_str(CODE_STYLE);
                self.out.push_str("\"><code>");
            }
            Tag::HtmlBlock => self.raw_block = Some(String::new()),
            Tag::List(None) => self.out.push_str("<ul>\n"),
            Tag::List(Some(1)) => self.out.push_str("<ol>\n"),
            Tag::List(Some(first)) => {
                self.out.push_str("<ol start=\"");
                self.out.push_str(&first.to_string());
                self.out.push_str("\">\n");
            }
            Tag::Item => self.out.push_str("<li>"),
            Tag::Emphasis => self.out.push_str("<em>"),
            Tag::Strong => self.out.push_str("<strong>"),
            Tag::Strikethrough => self.out.push_str("<del>"),
            // A picture would have to be fetched by whoever reads the mail,
            // which tells its host that they did. It goes out as a link to
            // the picture instead, under its description.
            Tag::Link {
                link_type,
                dest_url,
                ..
            }
            | Tag::Image {
                link_type,
                dest_url,
                ..
            } => {
                // "<robin@example.com>" arrives as the bare address.
                let target = if link_type == LinkType::Email {
                    format!("mailto:{dest_url}")
                } else {
                    dest_url.into_string()
                };
                // Only links Quill itself would open: nothing that runs,
                // and nothing that hides where it leads. A picture inside a
                // link stays part of that link.
                let anchored = is_openable(&target) && !self.links.iter().any(|link| link.anchored);
                if anchored {
                    self.out.push_str("<a href=\"");
                    escape_into(&mut self.out, &target);
                    self.out.push_str("\">");
                }
                self.links.push(OpenLink {
                    anchored,
                    fallback: target,
                    text_starts: self.out.len(),
                });
            }
            Tag::Table(alignments) => {
                self.table = alignments;
                self.out.push_str("<table style=\"");
                self.out.push_str(TABLE_STYLE);
                self.out.push_str("\">\n");
            }
            Tag::TableHead => {
                self.in_table_head = true;
                self.cell = 0;
                self.out.push_str("<tr>");
            }
            Tag::TableRow => {
                self.cell = 0;
                self.out.push_str("<tr>");
            }
            Tag::TableCell => {
                self.out
                    .push_str(if self.in_table_head { "<th" } else { "<td" });
                self.out.push_str(" style=\"");
                self.out.push_str(CELL_STYLE);
                match self.table.get(self.cell) {
                    Some(Alignment::Left) => self.out.push_str(";text-align:left"),
                    Some(Alignment::Center) => self.out.push_str(";text-align:center"),
                    Some(Alignment::Right) => self.out.push_str(";text-align:right"),
                    Some(Alignment::None) | None => {}
                }
                self.out.push_str("\">");
            }
            // Extensions that are not switched on: their text still comes
            // through as text events.
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.out.push_str("</p>\n"),
            TagEnd::Heading(level) => {
                self.out.push_str("</");
                self.out.push_str(&level.to_string());
                self.out.push_str(">\n");
            }
            TagEnd::BlockQuote(_) => self.out.push_str("</blockquote>\n"),
            TagEnd::CodeBlock => self.out.push_str("</code></pre>\n"),
            TagEnd::HtmlBlock => {
                if let Some(block) = self.raw_block.take() {
                    self.out.push_str("<p>");
                    let mut lines = block.trim_end().lines();
                    if let Some(first) = lines.next() {
                        escape_into(&mut self.out, first);
                    }
                    for line in lines {
                        self.out.push_str("<br>\n");
                        escape_into(&mut self.out, line);
                    }
                    self.out.push_str("</p>\n");
                }
            }
            TagEnd::List(true) => self.out.push_str("</ol>\n"),
            TagEnd::List(false) => self.out.push_str("</ul>\n"),
            TagEnd::Item => self.out.push_str("</li>\n"),
            TagEnd::Emphasis => self.out.push_str("</em>"),
            TagEnd::Strong => self.out.push_str("</strong>"),
            TagEnd::Strikethrough => self.out.push_str("</del>"),
            TagEnd::Link | TagEnd::Image => {
                if let Some(link) = self.links.pop() {
                    // "![](https://example.com/chart.png)" has no words to
                    // carry the link, so the address is its text.
                    if self.out.len() == link.text_starts {
                        escape_into(&mut self.out, &link.fallback);
                    }
                    if link.anchored {
                        self.out.push_str("</a>");
                    }
                }
            }
            TagEnd::Table => {
                self.table.clear();
                self.out.push_str("</table>\n");
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
                self.out.push_str("</tr>\n");
            }
            TagEnd::TableRow => self.out.push_str("</tr>\n"),
            TagEnd::TableCell => {
                self.cell += 1;
                self.out
                    .push_str(if self.in_table_head { "</th>" } else { "</td>" });
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
        }
    }
}

/// Text made safe to sit in HTML, between tags or inside a quoted attribute.
fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is between `<body>` and `</body>`.
    fn body(markdown: &str) -> String {
        let html = to_html(markdown);
        let start = html.find("<body>\n").expect("a body") + "<body>\n".len();
        let end = html.rfind("</body>").expect("a closed body");
        html[start..end].to_owned()
    }

    #[test]
    fn it_is_a_whole_document_that_says_it_is_utf8() {
        let html = to_html("Hi Ana");
        assert!(html.starts_with("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n"));
        assert!(html.ends_with("<p>Hi Ana</p>\n</body>\n</html>\n"));
    }

    #[test]
    fn paragraphs_keep_the_line_breaks_that_were_typed() {
        assert_eq!(
            body("Hi Ana,\nHere is the draft.\n\nThanks,\nRobin\n"),
            "<p>Hi Ana,<br>\nHere is the draft.</p>\n<p>Thanks,<br>\nRobin</p>\n"
        );
    }

    #[test]
    fn headings_emphasis_and_rules() {
        assert_eq!(
            body("# Plan\n\n## Next *week*\n\nIt is **done** and `tested`.\n\n---\n"),
            "<h1>Plan</h1>\n<h2>Next <em>week</em></h2>\n\
             <p>It is <strong>done</strong> and <code>tested</code>.</p>\n<hr>\n"
        );
    }

    #[test]
    fn lists() {
        assert_eq!(
            body("- tea\n- coffee\n\n3. three\n4. four\n"),
            "<ul>\n<li>tea</li>\n<li>coffee</li>\n</ul>\n\
             <ol start=\"3\">\n<li>three</li>\n<li>four</li>\n</ol>\n"
        );
        assert!(body("1. one\n2. two\n").starts_with("<ol>\n<li>one</li>"));
    }

    #[test]
    fn links_lead_only_where_quill_would_go() {
        assert_eq!(
            body("See [the agenda](https://example.com/agenda?a=1&b=2) or <robin@example.com>."),
            "<p>See <a href=\"https://example.com/agenda?a=1&amp;b=2\">the agenda</a> or \
             <a href=\"mailto:robin@example.com\">robin@example.com</a>.</p>\n"
        );
        // The words stay; the link does not.
        for target in [
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "vbscript:x",
            "/relative",
        ] {
            let html = body(&format!("Click [here]({target}) now"));
            assert!(!html.contains("<a "), "{html}");
            assert!(!html.contains("href"), "{html}");
            assert!(html.contains("here"), "{html}");
        }
    }

    #[test]
    fn a_link_cannot_break_out_of_its_attribute() {
        let html = body("[x](https://example.com/\"onmouseover=\"alert(1))");
        assert!(!html.contains("\"onmouseover"), "{html}");
        let html = body("[x](<https://example.com/a b\" onclick=\"y>)");
        assert!(!html.contains("onclick=\""), "{html}");
    }

    #[test]
    fn pictures_become_links_so_nothing_is_fetched() {
        assert_eq!(
            body("![The chart](https://example.com/chart.png)"),
            "<p><a href=\"https://example.com/chart.png\">The chart</a></p>\n"
        );
        assert_eq!(
            body("![](https://example.com/chart.png)"),
            "<p><a href=\"https://example.com/chart.png\">https://example.com/chart.png</a></p>\n"
        );
        for markdown in [
            "![The chart](https://example.com/chart.png)",
            "![x](data:image/png;base64,AAAA)",
            "<img src=\"https://example.com/pixel.gif\">",
        ] {
            let html = to_html(markdown);
            assert!(!html.contains("<img"), "{html}");
            assert!(!html.contains(" src=\""), "{html}");
        }
    }

    #[test]
    fn quotes_nest_and_carry_their_bar() {
        let html = body("On Saturday, Ana wrote:\n> first\n> second\n>> older\n");
        assert_eq!(html.matches("<blockquote style=\"").count(), 2);
        assert_eq!(html.matches("</blockquote>").count(), 2);
        assert!(html.contains("border-left"), "{html}");
        assert!(html.contains("first<br>\nsecond"), "{html}");
        assert!(html.starts_with("<p>On Saturday, Ana wrote:</p>\n<blockquote"));
    }

    #[test]
    fn a_message_being_answered_is_text_and_only_the_answer_is_markdown() {
        let html = body(
            "Yes, **Thursday** works.\n\n\
             On Saturday, Ana wrote:\n\
             > Is *Thursday* ok for the snake_case_name review?\n\
             > # not a heading\n\
             >     not code\n\
             > 1. not a list\n\
             > See <https://example.com/a> and <b>this</b>.\n\
             >\n\
             > A second paragraph.\n",
        );
        // The answer is Markdown.
        assert!(html.starts_with("<p>Yes, <strong>Thursday</strong> works.</p>\n"));
        // What is answered is not: its marks are the characters they were.
        let quote = &html[html.find("<blockquote").unwrap()..];
        assert!(quote.contains("<p>Is *Thursday* ok for the snake_case_name review?<br>\n"));
        assert!(quote.contains("# not a heading<br>\n"), "{quote}");
        assert!(quote.contains("    not code<br>\n"), "{quote}");
        assert!(quote.contains("1. not a list<br>\n"), "{quote}");
        assert!(
            quote.contains("See &lt;https://example.com/a&gt; and &lt;b&gt;this&lt;/b&gt;.</p>\n"),
            "{quote}"
        );
        assert!(quote.contains("<p>A second paragraph.</p>\n"), "{quote}");
        for tag in ["<em>", "<h1>", "<pre", "<ol", "<li>", "<a ", "<code>"] {
            assert!(!quote.contains(tag), "{tag} in {quote}");
        }
        assert_eq!(html.matches("<blockquote").count(), 1);
    }

    #[test]
    fn an_answer_written_under_a_quote_is_not_part_of_it() {
        let html = body("On Saturday, Ana wrote:\n> Is it *done*?\nIt is *done*.\n");
        assert!(
            html.ends_with("<p>Is it *done*?</p>\n</blockquote>\n<p>It is <em>done</em>.</p>\n"),
            "{html}"
        );
    }

    #[test]
    fn quotes_written_other_ways_are_as_deep_as_they_say() {
        let html = body("> one\n> > two\n>>> three\n> one again\n   > indented\n");
        assert_eq!(html.matches("<blockquote style=\"").count(), 3);
        assert_eq!(html.matches("</blockquote>").count(), 3);
        assert!(html.contains("<p>two</p>"), "{html}");
        assert!(html.contains("<p>three</p>"), "{html}");
        assert!(html.contains("<p>one again<br>\nindented</p>"), "{html}");
        // A mark with nothing after it quotes nothing.
        assert_eq!(body("a\n>\n"), "<p>a</p>\n<p>&gt;</p>\n");
        // A thousand levels are drawn as a dozen.
        let deep = body(&format!("{} x\n", ">".repeat(1_000)));
        assert_eq!(deep.matches("<blockquote").count(), DEEPEST_QUOTE);
        assert_eq!(deep.matches("</blockquote>").count(), DEEPEST_QUOTE);
    }

    #[test]
    fn a_message_passed_on_is_text_from_the_line_that_says_so() {
        let html = body(&format!(
            "FYI, see the **second** point.\n\n\
             {FORWARDED}\n\
             From: Ana Ruiz <ana.ruiz@example.com>\n\
             Subject: The *plan*\n\n\
             # Not a heading\n\
             - not a list\n\
             > She quoted *this*.\n\
             And a _last_ line.\n"
        ));
        assert!(html.starts_with("<p>FYI, see the <strong>second</strong> point.</p>\n"));
        let passed = &html[html.find("<p>--------").unwrap()..];
        assert!(passed.starts_with(&format!(
            "<p>{FORWARDED}<br>\nFrom: Ana Ruiz &lt;ana.ruiz@example.com&gt;<br>\n\
             Subject: The *plan*</p>\n<p># Not a heading<br>\n- not a list</p>\n"
        )));
        // A quote inside it keeps its bar, and is text too.
        assert!(passed.contains("<p>She quoted *this*.</p>\n</blockquote>\n"));
        assert!(passed.ends_with("<p>And a _last_ line.</p>\n"), "{passed}");
        for tag in ["<em>", "<h1>", "<ul>", "<a ", "<strong>"] {
            assert!(!passed.contains(tag), "{tag} in {passed}");
        }
    }

    #[test]
    fn a_quote_mark_inside_fenced_code_is_code() {
        let html = body("```\n> not a quote\n```\n> a quote\n");
        assert!(html.contains("<code>&gt; not a quote\n</code>"), "{html}");
        assert_eq!(html.matches("<blockquote").count(), 1);
        // The line that says a message is passed on is code there too.
        let html = body(&format!("~~~\n{FORWARDED}\n~~~\n**after**\n"));
        assert!(html.ends_with("<p><strong>after</strong></p>\n"), "{html}");
    }

    #[test]
    fn code_blocks_are_kept_as_typed_and_escaped() {
        assert_eq!(
            body("```rust\nif a < b && c > d {\n    run(\"x\");\n}\n```\n"),
            format!(
                "<pre style=\"{CODE_STYLE}\"><code>if a &lt; b &amp;&amp; c &gt; d {{\n    \
                 run(&quot;x&quot;);\n}}\n</code></pre>\n"
            )
        );
    }

    #[test]
    fn html_the_writer_typed_is_shown_not_obeyed() {
        for markdown in [
            "<script>alert(1)</script>",
            "Hello <script>alert(1)</script> there",
            "<div onclick=\"x()\">\n*still text*\n</div>",
            "<style>p { display: none }</style>",
            "<iframe src=\"https://example.com/\"></iframe>",
            "a <b>bold</b> claim",
            "<!-- a comment -->",
            "<a href=\"javascript:alert(1)\">x</a>",
            "<p\n>",
            "<img src=x onerror=alert(1)>",
            "&lt;script&gt; and <ScRiPt >x</sCrIpT >",
        ] {
            let html = body(markdown);
            // The only tags in the result are the ones this module writes.
            let mut rest = html.as_str();
            while let Some(at) = rest.find('<') {
                rest = &rest[at + 1..];
                let name: String = rest
                    .trim_start_matches('/')
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                assert!(
                    matches!(name.as_str(), "p" | "br" | "em" | "strong"),
                    "{markdown:?} gave a live <{name}> in {html}"
                );
            }
        }
        assert_eq!(
            body("<script>alert(1)</script>"),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>\n"
        );
        assert_eq!(
            body("a <b>bold</b> claim"),
            "<p>a &lt;b&gt;bold&lt;/b&gt; claim</p>\n"
        );
        assert_eq!(
            body("<div>\none\ntwo\n</div>"),
            "<p>&lt;div&gt;<br>\none<br>\ntwo<br>\n&lt;/div&gt;</p>\n"
        );
    }

    #[test]
    fn tables() {
        let html = body("| Item | Cost |\n|:-----|-----:|\n| Tea  | 3    |\n");
        assert!(html.starts_with(&format!("<table style=\"{TABLE_STYLE}\">\n<tr><th style=")));
        assert!(html.contains(";text-align:left\">Item</th>"), "{html}");
        assert!(html.contains(";text-align:right\">3</td>"), "{html}");
        assert!(html.ends_with("</tr>\n</table>\n"));
    }

    #[test]
    fn ordinary_prose_is_not_struck_through() {
        assert_eq!(
            body("About ~5 or ~10 minutes."),
            "<p>About ~5 or ~10 minutes.</p>\n"
        );
    }

    #[test]
    fn nothing_makes_it_panic() {
        let deep_quote = ">".repeat(5_000) + " x";
        let deep_list = (0..500)
            .map(|depth| format!("{}- x\n", "  ".repeat(depth)))
            .collect::<String>();
        let seeds = [
            "",
            "\n\n\n",
            "\0\u{1b}[2J",
            "[",
            "](",
            "![",
            "<",
            "<<<<>>>>",
            "```",
            "| a |\n|---|",
            "[a]: <",
            "* * *\n- - -\n___",
            "\u{feff}# x",
            &"[".repeat(5_000),
            &"*a ".repeat(5_000),
            &"<a ".repeat(5_000),
            &deep_quote,
            &deep_list,
        ];
        for seed in seeds {
            let _ = to_html(seed);
            for (end, _) in seed.char_indices().take(64) {
                let _ = to_html(&seed[..end]);
            }
        }
    }
}
