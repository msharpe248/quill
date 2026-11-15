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

use pulldown_cmark::{Alignment, Event, LinkType, Options, Parser, Tag, TagEnd};
use quill_core::links::is_openable;

/// Quoted text: a bar down the left, as mail programs draw it. No colours
/// beyond the bar, so it reads the same on a dark background.
const QUOTE_STYLE: &str = "margin:0 0 1em 0;padding:0 0 0 1ex;border-left:2px solid #cccccc";
/// Code keeps its line breaks but may wrap, so one long line does not make
/// the whole message scroll sideways.
const CODE_STYLE: &str = "white-space:pre-wrap;font-family:monospace";
const TABLE_STYLE: &str = "border-collapse:collapse";
const CELL_STYLE: &str = "border:1px solid #cccccc;padding:2px 6px";

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
    // Tables are the one extension: they cannot be mistaken for anything
    // else. Strikethrough is left off because "~5 or ~10 minutes" is
    // ordinary prose.
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        writer.event(event);
    }
    writer.out.push_str("</body>\n</html>\n");
    writer.out
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
