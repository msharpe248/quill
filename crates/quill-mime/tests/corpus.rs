//! Every message in `corpus/` through everything this crate does with a
//! message.
//!
//! The corpus is small messages, each written to be awkward in one way: a
//! charset, a broken structure, a hostile body. Three are a matter of size
//! (a header line of ten kilobytes, a body line of two megabytes, HTML five
//! thousand elements deep) and one is not a message at all.
//!
//! Three things are asked of each: that reading it never panics and comes
//! back in bounded time with bounded output; that no text Quill would draw
//! carries a control character or an escape sequence; and that the HTML
//! Quill would hand a web view can run nothing and load nothing it was not
//! allowed to. Then each is cut short and corrupted a few hundred ways, to
//! see that the first of those holds for input nobody wrote on purpose.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use quill_mime::html::{self, Line, Rendered};
use quill_mime::sanitize::{self, Options, Sanitized};
use quill_mime::{Parsed, attachment_bytes, links, parse, part_by_content_id, plain};

/// 2026-11-14T09:30:00Z, standing in for when each message arrived.
const RECEIVED: i64 = 1_794_648_600;

const WIDTHS: [usize; 4] = [1, 20, 80, 200];

/// The longest one message may take through everything, in a build without
/// optimisation on a machine doing other things. The slowest in the corpus,
/// the five thousand nested elements, takes about a fifth of this; it is
/// parsed seven times over, and the parser's work grows with the square of
/// the depth.
const TIME_LIMIT: Duration = Duration::from_secs(20);

fn corpus() -> Vec<(String, Vec<u8>)> {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut messages: Vec<(String, Vec<u8>)> = fs::read_dir(&folder)
        .expect("the corpus folder")
        .map(|entry| entry.expect("a corpus entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "eml"))
        .map(|path| {
            let name = path
                .file_stem()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            (name, fs::read(&path).expect("a readable message"))
        })
        .collect();
    messages.sort();
    messages
}

fn message(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.eml"));
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn read(name: &str) -> Parsed {
    parse(&message(name), RECEIVED).unwrap_or_else(|| panic!("{name} is a message"))
}

fn text(rendered: &Rendered) -> Vec<String> {
    rendered.lines.iter().map(Line::text).collect()
}

// ---------------------------------------------------------------------------
// What is promised about every string.

fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

fn is_reordering(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A header, a name, an address, a file name: no control characters at all,
/// and nothing that reorders the text around it.
#[track_caller]
fn assert_header(what: &str, text: &str) {
    assert!(
        !text.contains(|c: char| is_control(c) || is_reordering(c)),
        "{what}: {text:?}"
    );
}

/// A body: line feeds and tabs, and no other control character.
#[track_caller]
fn assert_body(what: &str, text: &str) {
    let bad = text
        .char_indices()
        .find(|(_, c)| is_control(*c) && *c != '\n' && *c != '\t');
    if let Some((at, c)) = bad {
        let from = text.floor_char_boundary(at.saturating_sub(40));
        let to = text.ceil_char_boundary((at + 40).min(text.len()));
        panic!("{what}: {c:?} in {:?}", &text[from..to]);
    }
}

/// A laid-out body: every span on one line, and nothing a terminal acts on.
#[track_caller]
fn assert_rendered(what: &str, rendered: &Rendered) {
    for line in &rendered.lines {
        for span in &line.spans {
            assert!(!span.text.contains(is_control), "{what}: {:?}", span.text);
            if let Some(link) = span.link {
                assert!(link < rendered.links.len(), "{what}: link {link}");
            }
        }
    }
    for link in &rendered.links {
        assert_header(what, link);
    }
}

// ---------------------------------------------------------------------------
// What is promised about a sanitized document.

/// A start tag in sanitized markup.
struct Tag<'a> {
    name: &'a str,
    attributes: Vec<(&'a str, &'a str)>,
}

/// Reads a start tag in the regular form the sanitizer writes: a name, then
/// attributes each as ` name="value"`, then `>`.
fn read_tag(input: &str) -> Option<(Tag<'_>, &str)> {
    let length = input
        .bytes()
        .take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        .count();
    let (name, mut rest) = input.split_at(length);
    if name.is_empty() {
        return None;
    }
    let mut attributes = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix('>') {
            return Some((Tag { name, attributes }, after));
        }
        rest = rest.strip_prefix(' ')?;
        let length = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            .count();
        let (attribute, after) = rest.split_at(length);
        let after = after.strip_prefix("=\"")?;
        let end = after.find('"')?;
        attributes.push((attribute, &after[..end]));
        rest = &after[end + 1..];
    }
}

/// Elements that have no business in a sanitized message.
const NEVER: &[&str] = &[
    "applet", "base", "button", "embed", "form", "frame", "frameset", "iframe", "input", "link",
    "math", "meta", "object", "script", "select", "svg", "textarea", "title",
];

/// Checks a sanitized document tag by tag: nothing that runs, nothing that
/// submits, links only to where Quill would go, and, unless remote content
/// was allowed, nothing that loads from anywhere but the message itself.
#[track_caller]
fn assert_sanitized(what: &str, sanitized: &Sanitized, remote: bool, files: &[String]) {
    let document = &sanitized.html;
    assert_body(what, document);
    assert!(document.starts_with("<!doctype html>"), "{what}");
    assert_eq!(
        document
            .matches("http-equiv=\"Content-Security-Policy\"")
            .count(),
        1,
        "{what}"
    );
    assert!(!document.to_ascii_lowercase().contains("<script"), "{what}");
    if remote {
        assert_eq!(sanitized.blocked, 0, "{what}");
    }

    let (_, rest) = document.split_once("<body>\n").expect("a body");
    let (body, _) = rest.rsplit_once("\n</body>").expect("the end of the body");
    let squeeze = |text: &str| -> String {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let is_local = |css: &str| {
        let css = squeeze(css);
        !css.contains("http:") && !css.contains("https:") && !css.contains("//")
    };
    let is_tame = |css: &str| {
        let css = squeeze(css);
        !css.contains("@import")
            && !css.contains("expression(")
            && !css.contains("javascript:")
            && !css.contains('\\')
    };

    let mut rest = body;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        if rest.starts_with('/') {
            continue;
        }
        let Some((tag, after)) = read_tag(rest) else {
            let shown = &rest[..rest.ceil_char_boundary(rest.len().min(80))];
            panic!("{what}: not a tag the sanitizer writes: {shown:?}");
        };
        rest = after;
        assert!(!NEVER.contains(&tag.name), "{what}: <{}>", tag.name);
        if tag.name == "style" {
            let (css, after) = rest.split_once("</style>").expect("the end of a style");
            assert!(!css.contains('<') && is_tame(css), "{what}: {css:?}");
            assert!(remote || is_local(css), "{what}: {css:?}");
            rest = after;
        }
        for (name, value) in &tag.attributes {
            let squeezed = squeeze(value);
            assert!(!name.starts_with("on"), "{what}: {name}");
            assert!(!squeezed.contains("javascript:"), "{what}: {value:?}");
            assert!(!squeezed.contains("vbscript:"), "{what}: {value:?}");
            match *name {
                "href" => {
                    assert_eq!(tag.name, "a", "{what}");
                    let url = value.replace("&amp;", "&");
                    assert!(links::is_openable(&url), "{what}: {value:?}");
                    assert!(
                        tag.attributes.contains(&("rel", "noopener noreferrer")),
                        "{what}"
                    );
                }
                "src" | "poster" | "background" => {
                    let picture = tag.name == "img" && *name == "src";
                    let embedded =
                        value.starts_with("./") || (picture && squeezed.starts_with("data:image/"));
                    let fetched =
                        squeezed.starts_with("https://") || squeezed.starts_with("http://");
                    assert!(embedded || (remote && fetched), "{what}: {name}={value:?}");
                }
                "srcset" => assert!(remote, "{what}: {value:?}"),
                "style" => {
                    assert!(is_tame(value), "{what}: {value:?}");
                    assert!(remote || is_local(value), "{what}: {value:?}");
                }
                "action" | "formaction" | "srcdoc" | "data" | "ping" | "xlink:href" => {
                    panic!("{what}: {name}={value:?}")
                }
                _ => {}
            }
        }
    }
    for id in &sanitized.inline {
        assert_header(what, id);
        assert!(files.contains(id), "{what}: no part {id:?}");
    }
}

// ---------------------------------------------------------------------------
// One message through everything.

/// Runs `raw` through the whole crate, checking what must hold for any
/// input at all. With `whole`, also what holds for a message that has not
/// been corrupted: its files can be fetched by the numbers they were listed
/// with. Returns what was parsed and how long it all took.
#[track_caller]
fn examine(what: &str, raw: &[u8], whole: bool) -> (Option<Parsed>, Duration) {
    let started = Instant::now();
    let Some(parsed) = parse(raw, RECEIVED) else {
        return (None, started.elapsed());
    };

    assert_header(what, &parsed.headers.subject);
    if let Some(id) = &parsed.headers.message_id {
        assert_header(what, id);
    }
    if let Some(id) = &parsed.in_reply_to {
        assert_header(what, id);
    }
    for id in &parsed.references {
        assert_header(what, id);
    }
    let people = [
        &parsed.headers.from,
        &parsed.headers.to,
        &parsed.headers.cc,
        &parsed.reply_to,
    ];
    for address in people.into_iter().flatten() {
        assert_header(what, &address.email);
        assert!(!address.email.is_empty(), "{what}");
        if let Some(name) = &address.name {
            assert_header(what, name);
            assert!(!name.is_empty(), "{what}");
        }
    }
    assert_body(what, &parsed.body);
    // Decoding can make text longer, but not without limit.
    assert!(parsed.body.len() <= 8 * raw.len() + 4096, "{what}");

    let mut files: Vec<String> = Vec::new();
    for file in &parsed.attachments {
        assert_header(what, &file.content_type);
        if let Some(name) = &file.name {
            assert_header(what, name);
            assert!(!name.trim().is_empty(), "{what}");
        }
        let bytes = attachment_bytes(raw, file.part);
        if whole {
            let bytes = bytes.unwrap_or_else(|| panic!("{what}: part {} is gone", file.part));
            assert_eq!(bytes.len(), file.size, "{what}: part {}", file.part);
        }
        if let Some(id) = &file.content_id {
            assert_header(what, id);
            assert!(!id.is_empty(), "{what}");
            let found = part_by_content_id(raw, id);
            if whole {
                let (kind, _) = found.unwrap_or_else(|| panic!("{what}: no part {id:?}"));
                assert_header(what, &kind);
            }
            files.push(id.clone());
        }
    }
    assert!(attachment_bytes(raw, u32::MAX).is_none(), "{what}");

    for width in WIDTHS {
        let rendered = plain::render(&parsed.body, width);
        assert_rendered(what, &rendered);
        assert!(rendered.lines.len() <= parsed.body.len() + 2, "{what}");
        for link in &rendered.links {
            assert!(links::is_openable(link), "{what}: {link:?}");
        }
    }

    if let Some(markup) = &parsed.html {
        assert_body(what, markup);
        examine_html(what, markup, &parsed.headers.subject, &files);
    }

    let took = started.elapsed();
    assert!(took < TIME_LIMIT, "{what}: took {took:?}");
    (Some(parsed), took)
}

/// Lays `markup` out as text at every width and sanitizes it both ways,
/// checking what must hold of the results. `files` are the content ids
/// that have a part behind them.
#[track_caller]
fn examine_html(what: &str, markup: &str, title: &str, files: &[String]) {
    for width in WIDTHS {
        if let Some(rendered) = html::render(markup, width) {
            assert_rendered(what, &rendered);
            assert!(!rendered.lines.is_empty(), "{what}");
            assert!(rendered.lines.len() <= 2 * markup.len() + 64, "{what}");
        }
    }
    if let Some(text) = html::to_text(markup) {
        assert_body(what, &text);
    }

    let file_for = |id: &str| {
        files
            .iter()
            .position(|known| known == id)
            .map(|at| format!("inline {at}.bin"))
    };
    for remote in [false, true] {
        let options = Options {
            remote,
            cid_file: &file_for,
            title,
        };
        let sanitized = sanitize::sanitize(markup, &options);
        assert_sanitized(what, &sanitized, remote, files);
        // Escaping makes a document longer, but not without limit.
        assert!(
            sanitized.html.len() <= 16 * markup.len() + 128 * 1024,
            "{what}"
        );
    }
}

#[test]
fn every_message_is_read_safely_and_in_bounded_time() {
    let messages = corpus();
    assert!(messages.len() >= 45, "{} messages", messages.len());
    let mut read = 0;
    for (name, raw) in &messages {
        let (parsed, _) = examine(name, raw, true);
        read += usize::from(parsed.is_some());
    }
    // The empty file and the picture are not messages. Everything else is.
    assert_eq!(read, messages.len() - 2);
}

// ---------------------------------------------------------------------------
// Robustness: input nobody wrote on purpose.

/// A small generator of the same numbers every run.
struct Random(u64);

impl Random {
    /// Seeded from a message's name, so each has its own fixed sequence.
    fn named(name: &str) -> Self {
        let mut seed = 0xcbf2_9ce4_8422_2325u64;
        for byte in name.bytes() {
            seed = (seed ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `limit`, or zero when there is nothing below it.
    fn below(&mut self, limit: usize) -> usize {
        match u64::try_from(limit) {
            Ok(limit) if limit > 0 => usize::try_from(self.next() % limit).unwrap_or(0),
            _ => 0,
        }
    }
}

/// Bytes that mean something to a parser of mail, of HTML or of terminals.
const SPECIAL: &[u8] = b"\0\r\n\t \"'<>=;:-%&?()[]{}\\/@,.\x1b\x07\x7f\x80\x9b\xc3\xe2\xf0\xff";

/// `raw` with one small thing wrong with it.
fn corrupted(random: &mut Random, raw: &[u8]) -> Vec<u8> {
    let mut bytes = raw.to_vec();
    // Half the time in the headers, where the structure is decided.
    let reach = match random.below(2) {
        0 => bytes.len().min(600),
        _ => bytes.len(),
    };
    let at = random.below(reach);
    let special = SPECIAL[random.below(SPECIAL.len())];
    match random.below(7) {
        0 if !bytes.is_empty() => bytes[at] = u8::try_from(random.below(256)).unwrap_or(0),
        1 if !bytes.is_empty() => bytes[at] = special,
        2 => bytes.insert(at, special),
        3 if !bytes.is_empty() => {
            bytes.remove(at);
        }
        4 => {
            // A piece of the message again, somewhere else.
            let length = random.below(64).min(bytes.len() - at);
            let piece = bytes[at..at + length].to_vec();
            let to = random.below(bytes.len() + 1);
            bytes.splice(to..to, piece);
        }
        5 => {
            // A piece of the message gone.
            let length = random.below(64).min(bytes.len() - at);
            bytes.drain(at..at + length);
        }
        _ => {
            // A run of one byte, as long as a line.
            let run = vec![special; 1 + random.below(120)];
            bytes.splice(at..at, run);
        }
    }
    bytes
}

/// How many variations a message gets: fifty cuts and two hundred
/// corruptions, or as many as fit in a few seconds for the handful that
/// are slow to read even once.
fn variations(cost: Duration) -> (usize, usize) {
    let fit = |budget: Duration, most: usize, least: usize| {
        let runs = budget.as_micros() / cost.as_micros().max(1);
        usize::try_from(runs).unwrap_or(most).clamp(least, most)
    };
    (
        fit(Duration::from_millis(1500), 50, 2),
        fit(Duration::from_millis(3000), 200, 3),
    )
}

/// Cuts and corrupts every message whose place in the corpus falls to this
/// share of the work, and checks each result as it would any message.
fn survive(share: usize, shares: usize) {
    for (index, (name, raw)) in corpus().iter().enumerate() {
        if index % shares != share {
            continue;
        }
        let (_, cost) = examine(name, raw, false);
        let (cuts, corruptions) = variations(cost);
        let mut random = Random::named(name);

        // A download can stop anywhere: at even steps through the message,
        // and at places chosen at random.
        for cut in 0..cuts {
            let end = match cut % 2 {
                0 => raw.len() * cut / cuts,
                _ => random.below(raw.len()),
            };
            examine(&format!("{name} cut at {end}"), &raw[..end], false);
        }
        for round in 0..corruptions {
            let mut bytes = corrupted(&mut random, raw);
            // Every fourth has a second thing wrong with it.
            if round % 4 == 3 {
                bytes = corrupted(&mut random, &bytes);
            }
            examine(&format!("{name}, corruption {round}"), &bytes, false);
        }
    }
}

#[test]
fn cut_and_corrupted_messages_are_survived_1_of_4() {
    survive(0, 4);
}

#[test]
fn cut_and_corrupted_messages_are_survived_2_of_4() {
    survive(1, 4);
}

#[test]
fn cut_and_corrupted_messages_are_survived_3_of_4() {
    survive(2, 4);
}

#[test]
fn cut_and_corrupted_messages_are_survived_4_of_4() {
    survive(3, 4);
}

// ---------------------------------------------------------------------------
// Markup nobody wrote on purpose either.

/// Elements, including every kind that changes how what follows is parsed.
const SOUP_TAGS: &[&str] = &[
    "a",
    "area",
    "b",
    "blockquote",
    "body",
    "br",
    "button",
    "caption",
    "center",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "font",
    "form",
    "frameset",
    "h1",
    "h3",
    "head",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "li",
    "map",
    "marquee",
    "math",
    "nobr",
    "noscript",
    "object",
    "ol",
    "option",
    "p",
    "plaintext",
    "pre",
    "q",
    "rt",
    "ruby",
    "s",
    "script",
    "select",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "u",
    "ul",
    "wbr",
    "xmp",
];

/// Attributes, with the values that have made a renderer fall over.
const SOUP_ATTRIBUTES: &[&str] = &[
    "align=right",
    "alt=\"\"",
    "alt=\"picture\"",
    "background=\"https://evil.example/bg.png\"",
    "border=1",
    "class=x",
    "colspan=-1",
    "colspan=0",
    "colspan=1",
    "colspan=18446744073709551615",
    "colspan=2",
    "colspan=3",
    "colspan=7",
    "colspan=99999999999999999999",
    "dir=rtl",
    "hidden",
    "href=\"\"",
    "href=\"#x\"",
    "href=\"https://example.com/a\"",
    "href=\"https://evil.example/\"",
    "href=\"javascript:steal()\"",
    "id=y",
    "onclick=\"steal()\"",
    "role=presentation",
    "rowspan=0",
    "rowspan=100",
    "rowspan=18446744073709551615",
    "rowspan=2",
    "rowspan=3",
    "src=\"cid:x\"",
    "src=\"https://evil.example/a.png\"",
    "start=-9223372036854775808",
    "start=5",
    "start=9223372036854775807",
    "style=\"background:url(https://evil.example/a.png)\"",
    "style=\"color:rgb(999999999999,1,2)\"",
    "style=\"display:none\"",
    "style=\"display:table-cell\"",
    "style=\"margin:999999999999999999999px\"",
    "style=\"white-space:pre\"",
    "style=\"white-space:pre-wrap;color:#fff;background:#000\"",
    "style=\"x:\\\"\"",
    "type=I",
    "width=0",
    "width=100%",
];

/// Text, with the same.
const SOUP_TEXT: &[&str] = &[
    "",
    " ",
    "\n",
    "\t",
    "x",
    "word ",
    "a much longer run of ordinary text that will need wrapping somewhere along the line ",
    "averyveryveryveryveryveryveryveryveryveryverylongwordwithoutanyspacesinitatall",
    "\u{9648}\u{9759} \u{65e5}\u{672c}\u{8a9e} ",
    "\u{1f389}\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} ",
    "&nbsp;",
    "&#x81;",
    "&#27;[2J",
    "&#x202e;",
    "&lt;b&gt; ",
    "\u{1b}]0;t\u{7}",
    "\u{0}",
    "> quoted ",
    "# heading ",
    "[1] ",
    "\u{2060}[1]",
    "\u{2060}[999999999999999999999]",
    "\u{2060}[",
    "https://example.com/x ",
    "www.mybank.example ",
    "p:nth-child(99999999999n+1){color:red} ",
    "a{b:c} ",
    "@import 'https://evil.example/x.css'; ",
    "} ",
    "<!-- c -->",
    "<![CDATA[x]]>",
];

/// A document made of tags opened, closed and left open at random.
fn soup(random: &mut Random) -> String {
    let mut markup = String::new();
    let mut open: Vec<&str> = Vec::new();
    for _ in 0..5 + random.below(60) {
        match random.below(10) {
            0..=3 => {
                let tag = SOUP_TAGS[random.below(SOUP_TAGS.len())];
                markup.push('<');
                markup.push_str(tag);
                for _ in 0..random.below(3) {
                    markup.push(' ');
                    markup.push_str(SOUP_ATTRIBUTES[random.below(SOUP_ATTRIBUTES.len())]);
                }
                markup.push('>');
                open.push(tag);
            }
            // Closed in order, or closed whether it is open or not.
            4 | 5 => {
                if let Some(tag) = open.pop() {
                    markup.push_str(&format!("</{tag}>"));
                }
            }
            6 => {
                let tag = SOUP_TAGS[random.below(SOUP_TAGS.len())];
                markup.push_str(&format!("</{tag}>"));
            }
            _ => markup.push_str(SOUP_TEXT[random.below(SOUP_TEXT.len())]),
        }
    }
    markup
}

/// Makes documents from `seed` and checks each as it would a message's
/// HTML. One that makes anything panic is named, since the panic itself
/// would not say which it was.
fn survive_soup(seed: &str) {
    let files = ["x".to_owned()];
    let mut random = Random::named(seed);
    for number in 0..2_500 {
        let markup = soup(&mut random);
        let checked = std::panic::catch_unwind(|| {
            examine_html("soup", &markup, &markup, &files);
        });
        if let Err(panic) = checked {
            let said = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("something");
            panic!("document {number} of {seed:?}: {said}\n{markup:?}");
        }
    }
}

#[test]
fn random_markup_is_survived_1_of_2() {
    survive_soup("soup, first half");
}

#[test]
fn random_markup_is_survived_2_of_2() {
    survive_soup("soup, second half");
}

#[test]
fn the_generator_gives_the_same_numbers_every_run() {
    let mut random = Random::named("plain-ascii");
    let first: Vec<u64> = (0..4).map(|_| random.next()).collect();
    let mut again = Random::named("plain-ascii");
    assert_eq!(first, (0..4).map(|_| again.next()).collect::<Vec<_>>());
    assert_ne!(first[0], Random::named("base64-text").next());
    assert_eq!(random.below(0), 0);
    assert!((0..1000).all(|_| random.below(7) < 7));
    // Corrupting changes something, and copes with nothing to change.
    let raw = message("plain-ascii");
    assert!((0..50).any(|_| corrupted(&mut random, &raw) != raw));
    for _ in 0..50 {
        let _ = corrupted(&mut random, b"");
        let _ = corrupted(&mut random, b"x");
    }
}

// ---------------------------------------------------------------------------
// What particular messages should come out as.

fn names(people: &[quill_core::types::Address]) -> Vec<(&str, &str)> {
    people
        .iter()
        .map(|person| (person.name.as_deref().unwrap_or(""), person.email.as_str()))
        .collect()
}

/// Each file as the list of files shows it: name, media type, decoded size.
fn files(parsed: &Parsed) -> Vec<(&str, &str, usize)> {
    parsed
        .attachments
        .iter()
        .map(|file| {
            (
                file.name.as_deref().unwrap_or(""),
                file.content_type.as_str(),
                file.size,
            )
        })
        .collect()
}

fn no_files(_: &str) -> Option<String> {
    None
}

/// The sanitized body of a message's HTML, without the document around it.
fn sanitized(parsed: &Parsed, remote: bool) -> (String, Sanitized) {
    let options = Options {
        remote,
        cid_file: &no_files,
        title: &parsed.headers.subject,
    };
    let result = sanitize::sanitize(parsed.html.as_deref().expect("an HTML body"), &options);
    let (_, rest) = result.html.split_once("<body>\n").expect("a body");
    let (body, _) = rest.rsplit_once("\n</body>").expect("the end of the body");
    (body.to_owned(), result)
}

#[test]
fn a_plain_message_is_read_as_written() {
    let parsed = read("plain-ascii");
    assert_eq!(parsed.headers.subject, "Q4 roadmap review");
    assert_eq!(
        names(&parsed.headers.from),
        [("Ana Ruiz", "ana.ruiz@example.com")]
    );
    assert_eq!(
        names(&parsed.headers.cc),
        [
            ("Sam Lee", "sam.lee@example.com"),
            ("Nair, Priya", "priya.nair@example.com")
        ]
    );
    assert_eq!(parsed.headers.date, RECEIVED - 16 * 60);
    assert_eq!(
        parsed.headers.message_id.as_deref(),
        Some("<q4-roadmap.1@example.com>")
    );
    assert_eq!(
        parsed.body,
        "Hi Robin,\n\nHere is the draft of the Q4 roadmap. Could you look it over before\n\
         Thursday's review?\n\nThanks,\nAna\n"
    );
    assert!(parsed.html.is_none() && parsed.has_plain && parsed.attachments.is_empty());
}

#[test]
fn transfer_encodings_are_undone() {
    assert_eq!(
        read("quoted-printable-soft-breaks").body,
        "Hola Robin:\n\nEl presupuesto de otoño está casi listo. Esta línea es tan larga que \
         el programa del remitente la partió con saltos suaves, y debe llegar entera.\n\n\
         Total: 1\u{a0}250,00\u{a0}€ = lo acordado. \n\
         Un signo igual suelto al final de línea no es un salto: =\nTomás\n"
    );
    assert_eq!(
        read("base64-text").body,
        "Seven o'clock at the north car park.\nBring the badge – the gate is locked after six.\n"
    );
    // Broken encodings give what can be made of them, not nothing.
    let malformed = read("quoted-printable-malformed").body;
    assert!(
        malformed.contains("=ZZ and =G1") && malformed.contains("café"),
        "{malformed:?}"
    );
    assert!(
        read("base64-broken")
            .body
            .starts_with("Seven o'clock at the north car park.")
    );
}

#[test]
fn single_byte_and_unicode_charsets_are_decoded() {
    let latin = read("charset-iso-8859-1");
    assert_eq!(latin.headers.subject, "Réunion à Besançon");
    assert!(
        latin
            .body
            .contains("Ça vous convient ? Coût : 12 £ ou ½ journée.")
    );

    let windows = read("charset-windows-1252");
    assert_eq!(windows.headers.subject, "“Smart quotes” – and the € sign");
    assert_eq!(
        windows.body,
        "Robin said: “the budget is €1 250” — that’s final… ™ • ‰ Š œ\n"
    );

    let cyrillic = read("charset-koi8-r");
    assert_eq!(cyrillic.headers.subject, "Отчёт за квартал");
    assert!(cyrillic.body.starts_with("Здравствуйте, Робин!"));

    let wide = read("charset-utf-16");
    assert_eq!(wide.headers.subject, "Sixteen bits ☕");
    assert_eq!(
        wide.body,
        "Sent from a program that writes UTF-16.\nTomás, 陈静 and 🎉 all fit.\n"
    );
}

#[test]
fn a_charset_nobody_knows_or_a_wrong_one_loses_only_what_it_must() {
    let unknown = read("charset-unknown");
    assert!(unknown.headers.subject.ends_with("what charset is this"));
    assert!(unknown.body.contains("names no charset anyone knows."));
    assert!(
        unknown
            .body
            .contains("and the ASCII around them should survive.")
    );

    let wrong = read("charset-wrong-label");
    assert!(wrong.headers.subject.ends_with("says UTF-8, is Latin-1"));
    assert!(
        wrong
            .body
            .starts_with("Labelled UTF-8, written in Latin-1: caf\u{fffd}, r\u{fffd}union")
    );
    assert!(
        wrong
            .body
            .ends_with("a five-byte form \u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}.\n")
    );

    assert!(
        read("charset-ascii-label-utf8-bytes")
            .body
            .starts_with("Labelled seven-bit ASCII")
    );
}

/// Needs the mail parser's `full_encoding` feature, which is on in the
/// workspace manifest: without it the parser knows the names of the
/// multi-byte charsets and cannot decode them.
#[test]
fn multi_byte_charsets_are_decoded() {
    for (name, subject, text) in [
        (
            "charset-iso-2022-jp",
            "会議のお知らせ",
            "会議は金曜日の午後三時に変更になりました。",
        ),
        (
            "charset-shift-jis",
            "見積書の件",
            "見積書を添付いたします。ｶﾀｶﾅ も 表示 できますか。",
        ),
        (
            "charset-gb2312",
            "季度报告",
            "季度报告已经完成，请在星期五之前审阅。",
        ),
        (
            "charset-gbk",
            "會議紀錄与镕",
            "含有 GB2312 没有的字：镕、堃、喆。",
        ),
        ("charset-big5", "會議紀錄", "附上星期三的會議紀錄，請查收。"),
        (
            "charset-euc-kr",
            "회의 일정 변경",
            "회의가 금요일 오후 세 시로 변경되었습니다.",
        ),
    ] {
        let parsed = read(name);
        assert_eq!(parsed.headers.subject, subject, "{name}");
        assert!(parsed.body.contains(text), "{name}: {:?}", parsed.body);
    }
}

#[test]
fn encoded_words_are_decoded_however_they_were_cut() {
    let adjacent = read("encoded-words-adjacent");
    assert_eq!(
        adjacent.headers.subject,
        "Tomás sent you the ☕ café budget 🎉"
    );
    assert_eq!(
        names(&adjacent.headers.from),
        [("Tomás Ortega", "tomas.ortega@example.com")]
    );
    assert_eq!(
        names(&adjacent.headers.to),
        [
            ("Renée ☕", "renee@example.org"),
            ("陈静", "chen.jing@example.com")
        ]
    );

    // Cut in the middle of a character, and put back together.
    assert_eq!(
        read("encoded-words-split-mid-character").headers.subject,
        "日本語の件名"
    );

    // Words that are not what they claim are left as they came, and the
    // good one among them is still read.
    let malformed = read("encoded-words-malformed");
    assert!(malformed.headers.subject.ends_with("lower case works"));
    assert_eq!(malformed.headers.from[0].email, "sam.lee@example.com");
    assert_eq!(malformed.headers.to.len(), 2);
    assert_eq!(malformed.headers.cc.len(), 3);

    let long = read("encoded-words-absurdly-long");
    assert!(!long.headers.subject.is_empty());
    assert_eq!(
        long.body,
        "One encoded word several kilobytes long, on one line.\n"
    );
}

#[test]
fn file_names_are_read_in_every_form() {
    assert_eq!(
        files(&read("rfc2231-filename-continuations")),
        [
            ("季度报告 Q4 final.pdf", "application/pdf", 28),
            (
                "a name long enough that the program which sent it decided to cut it into \
                 continuations without any encoding at all.txt",
                "text/plain",
                20
            ),
            (
                "pieces out of order with a gap.bin",
                "application/octet-stream",
                5
            ),
        ]
    );
    let charset = read("rfc2231-filename-charset");
    assert_eq!(charset.headers.subject, "Presentación");
    assert_eq!(
        files(&charset)[..3],
        [
            (
                "Presentación de otoño.ppt",
                "application/vnd.ms-powerpoint",
                8
            ),
            ("☕ foto.png", "image/png", 70),
            ("fallback.txt", "text/plain", 55),
        ]
    );
    assert_eq!(charset.attachments.len(), 4);

    let raw = read("eight-bit-headers");
    assert_eq!(
        raw.headers.subject,
        "Raw UTF-8 in a header: café, 日本語, 🎉"
    );
    assert_eq!(
        names(&raw.headers.to),
        [
            ("陈静", "chen.jing@example.com"),
            ("Renée ☕", "renee@example.org")
        ]
    );
    assert_eq!(
        files(&raw),
        [
            ("résumé de Tomás.pdf", "application/pdf", 9),
            ("caf\u{fffd}.txt", "text/plain", 23)
        ]
    );

    // No name at all, an empty one, and one that is only spaces.
    assert_eq!(
        files(&read("attachment-without-name")),
        [
            ("", "application/octet-stream", 10),
            ("", "application/octet-stream", 4),
            ("", "application/x-unknown-thing", 17),
        ]
    );
}

#[test]
fn the_right_body_is_chosen_from_alternatives() {
    let parsed = read("multipart-alternative");
    assert!(
        parsed
            .body
            .starts_with("Offsite agenda\n\n* Travel\n* Rooms\n")
    );
    assert!(parsed.html.is_some() && parsed.html_first && parsed.has_plain);
    assert!(parsed.attachments.is_empty());
    // Neither the preamble nor what follows the last boundary is the body.
    assert!(!parsed.body.contains("your mail") && !parsed.body.contains("not part of"));
    let shown = html::render(parsed.html.as_deref().unwrap_or_default(), 80).unwrap_or_default();
    assert_eq!(
        text(&shown),
        [
            "# Offsite agenda",
            "",
            "* Travel",
            "* Rooms",
            "",
            "The agenda[1] and the room list[2] are up.",
            "",
            "> Can we move it a week?",
            "",
            "[1] https://example.com/agenda",
            "[2] https://example.com/rooms",
        ]
    );
    assert_eq!(shown.lines[7].spans[0].style.quote, 1);
}

#[test]
fn attachments_are_listed_and_fetched() {
    let raw = message("multipart-mixed-attachments");
    let parsed = read("multipart-mixed-attachments");
    assert_eq!(
        parsed.body,
        "Please find the invoice attached, with two photos of the delivery.\n\
         A second piece of text after the files, which is still part of the body."
    );
    assert_eq!(
        files(&parsed),
        [
            ("invoice 2026-114.pdf", "application/pdf", 28),
            ("delivery-1.png", "image/png", 70),
            ("line items.csv", "text/csv", 29),
        ]
    );
    assert_eq!(
        attachment_bytes(&raw, parsed.attachments[0].part).as_deref(),
        Some(&b"%PDF-1.4\n% not really a PDF\n"[..])
    );
    assert_eq!(
        attachment_bytes(&raw, parsed.attachments[2].part).as_deref(),
        Some("item,cost\r\nTrain,48\r\nCafé,12".as_bytes())
    );
    assert!(
        attachment_bytes(&raw, parsed.attachments[1].part)
            .is_some_and(|png| png.starts_with(b"\x89PNG\r\n"))
    );

    assert_eq!(
        files(&read("winmail-dat")),
        [("winmail.dat", "application/ms-tnef", 157)]
    );
    assert_eq!(
        files(&read("multipart-signed-detached")),
        [("smime.p7s", "application/pkcs7-signature", 162)]
    );
    assert_eq!(
        read("multipart-signed-detached").body,
        "The terms are as we discussed: thirty days, renewable.\n\nAna"
    );
}

#[test]
fn inline_pictures_are_found_by_content_id_and_pointed_at_files() {
    let raw = message("multipart-related-cid-images");
    let parsed = read("multipart-related-cid-images");
    let ids: Vec<(Option<&str>, bool)> = parsed
        .attachments
        .iter()
        .map(|file| (file.content_id.as_deref(), file.inline))
        .collect();
    assert_eq!(
        ids,
        [
            (Some("logo@example.com"), true),
            (Some("chart+q4@example.com"), true)
        ]
    );
    for id in [
        "logo@example.com",
        "<logo@example.com>",
        " chart+q4@example.com ",
    ] {
        let (kind, bytes) = part_by_content_id(&raw, id).expect(id);
        assert_eq!(kind, "image/png");
        assert!(bytes.starts_with(b"\x89PNG"));
    }
    assert!(part_by_content_id(&raw, "missing@example.com").is_none());
    assert!(part_by_content_id(&raw, "").is_none());

    let file_for = |id: &str| match id {
        "logo@example.com" => Some("logo.png".to_owned()),
        "chart+q4@example.com" => Some("chart q4.png".to_owned()),
        _ => None,
    };
    let options = Options {
        remote: false,
        cid_file: &file_for,
        title: &parsed.headers.subject,
    };
    let result = sanitize::sanitize(parsed.html.as_deref().expect("an HTML body"), &options);
    assert_eq!(result.inline, ["logo@example.com", "chart+q4@example.com"]);
    assert_eq!(result.blocked, 0);
    assert!(
        result
            .html
            .contains("<img src=\"./logo.png\" alt=\"Example Works logo\" width=\"120\">")
    );
    // Written two ways in the message, the same file both times.
    assert_eq!(result.html.matches("src=\"./chart%20q4.png\"").count(), 2);
    assert!(result.html.contains("<table background=\"./logo.png\">"));
    assert!(
        result
            .html
            .contains("background-image: url(&quot;./logo.png&quot;)")
    );
    // The one that is not in the message is gone, and no `cid:` is left.
    assert!(!result.html.contains("Missing") && !result.html.contains("cid:"));
    assert!(result.html.contains("<title>The new logo</title>"));

    // Ids that clash or are not ids: the first part with an id answers
    // for it, and none of them is taken for a path.
    let raw = message("cid-oddities");
    let odd = read("cid-oddities");
    assert_eq!(odd.attachments.len(), 6);
    assert_eq!(
        part_by_content_id(&raw, "same@example.com")
            .map(|(kind, _)| kind)
            .as_deref(),
        Some("image/png")
    );
    assert!(part_by_content_id(&raw, "../../etc/passwd").is_some());
    let as_named = |id: &str| Some(id.to_owned());
    let options = Options {
        remote: false,
        cid_file: &as_named,
        title: "",
    };
    let result = sanitize::sanitize(odd.html.as_deref().expect("an HTML body"), &options);
    assert!(result.html.contains("src=\"./..%2F..%2Fetc%2Fpasswd\""));
    assert!(!result.html.contains("src=\"../"));
}

#[test]
fn nested_messages_keep_their_shape() {
    let five = read("multipart-nested-five-deep");
    assert_eq!(five.body, "The text at the bottom of five levels.");
    assert!(
        five.html
            .as_deref()
            .is_some_and(|html| html.contains("five levels"))
    );
    assert_eq!(
        files(&five),
        [
            ("note.txt", "text/plain", 24),
            ("", "image/png", 70),
            ("contract.pdf", "application/pdf", 9),
        ]
    );
    assert_eq!(
        five.attachments[1].content_id.as_deref(),
        Some("deep@example.com")
    );

    // Deeper than the parser goes: what it would not take apart is shown
    // as text, not offered as a file with no name.
    let hundred = read("multipart-nested-hundred-deep");
    assert!(
        hundred
            .body
            .contains("The text inside a hundred multiparts.")
    );
    assert!(hundred.attachments.is_empty());

    let digest = read("multipart-digest");
    assert_eq!(
        digest.body,
        "Today's topics:\n\n   1. Launch week (Ana Ruiz)\n   2. Re: Launch week (Sam Lee)\n\
         End of Roadmap Digest, Vol 12, Issue 3"
    );
    assert_eq!(
        files(&digest),
        [("", "message/rfc822", 152), ("", "message/rfc822", 245)]
    );

    // A forwarded message is one file. The file inside it is its own.
    let raw = message("message-rfc822-attachment");
    let forwarded = read("message-rfc822-attachment");
    assert_eq!(files(&forwarded), [("Contract.eml", "message/rfc822", 535)]);
    let inner = attachment_bytes(&raw, forwarded.attachments[0].part).expect("the message");
    let inner = parse(&inner, RECEIVED).expect("a message inside");
    assert_eq!(inner.headers.subject, "Contract");
    assert_eq!(inner.body, "The inner message, with a file of its own.");
    assert_eq!(files(&inner), [("contract.pdf", "application/pdf", 9)]);
}

#[test]
fn reports_and_invitations_list_their_parts() {
    let report = read("multipart-report-dsn");
    assert!(
        report
            .body
            .starts_with("This is the mail system at host mx.example.org.")
    );
    assert!(
        report
            .body
            .ends_with("Recipient address rejected: User unknown")
    );
    assert_eq!(
        files(&report),
        [
            ("", "message/delivery-status", 378),
            ("", "text/rfc822-headers", 201)
        ]
    );

    let raw = message("calendar-invite");
    let invite = read("calendar-invite");
    assert!(
        invite
            .body
            .starts_with("You are invited to: Offsite: Q4 planning")
    );
    assert!(invite.html.is_some());
    assert_eq!(
        files(&invite),
        [
            ("", "text/calendar", 631),
            ("invite.ics", "application/ics", 633)
        ]
    );
    for file in &invite.attachments {
        let calendar = attachment_bytes(&raw, file.part).expect("the invitation");
        assert!(calendar.starts_with(b"BEGIN:VCALENDAR"));
        assert!(calendar.windows(14).any(|bytes| bytes == b"METHOD:REQUEST"));
    }
}

#[test]
fn broken_structure_still_yields_what_arrived() {
    let cut = read("missing-final-boundary");
    assert_eq!(cut.body, "The part that arrived.");
    assert_eq!(files(&cut), [("half.pdf", "application/pdf", 24)]);

    let frontier = read("boundary-in-content");
    assert!(
        frontier
            .body
            .starts_with("A boundary only counts at the start of a line.")
    );
    assert!(
        frontier
            .body
            .contains("so is this: x--frontier--\n --frontier (after a space)\n")
    );
    assert!(
        frontier
            .body
            .ends_with("The text carries on after all of them.")
    );
    assert_eq!(frontier.attachments.len(), 1);

    // No boundary was named, so nothing can be taken apart. It is shown.
    let unnamed = read("no-boundary-parameter");
    assert!(
        unnamed
            .body
            .contains("The header never said what separates the parts.")
    );
    assert!(unnamed.attachments.is_empty());
    assert!(read("boundary-oddities").body.contains("After the second."));

    let empty = read("empty-parts");
    assert_eq!(empty.body, "The only part with anything in it.");
    assert_eq!(
        files(&empty),
        [
            ("empty.bin", "application/octet-stream", 0),
            ("", "image/png", 0)
        ]
    );
    assert!(
        empty
            .attachments
            .iter()
            .all(|file| file.content_id.is_none())
    );

    let headers = read("headers-only");
    assert_eq!(headers.headers.subject, "Nothing follows");
    assert!(headers.body.is_empty() && headers.html.is_none() && headers.attachments.is_empty());
    let promised = read("headers-only-multipart");
    assert_eq!(promised.headers.subject, "Promised parts, sent none");
    assert!(promised.body.is_empty() && promised.attachments.is_empty());

    assert!(parse(&message("zero-length"), RECEIVED).is_none());
    assert!(parse(&message("not-a-message"), RECEIVED).is_none());
}

#[test]
fn missing_and_repeated_headers_are_settled_one_way() {
    let missing = read("missing-date-from-subject");
    assert_eq!(missing.headers.subject, "");
    assert!(missing.headers.from.is_empty());
    assert_eq!(missing.headers.message_id, None);
    assert_eq!(missing.headers.date, RECEIVED);
    assert_eq!(
        names(&missing.headers.to),
        [("Robin Vale", "robin@example.org")]
    );
    assert!(
        missing
            .body
            .starts_with("No date, no sender and no subject.")
    );

    // One of each, whole: never the two run together.
    let repeated = read("duplicate-conflicting-headers");
    assert!(
        ["The first subject", "The second subject"].contains(&repeated.headers.subject.as_str())
    );
    assert_eq!(repeated.headers.from.len(), 1);
    assert!(
        ["<dup.1@example.com>", "<dup.2@example.com>"]
            .contains(&repeated.headers.message_id.as_deref().unwrap_or_default())
    );
    assert_eq!(repeated.reply_to.len(), 1);

    let odd = read("address-oddities");
    assert_eq!(odd.headers.from[0].email, "ana.ruiz@example.com");
    assert_eq!(
        names(&odd.headers.to),
        [
            ("", "maya.okafor@example.com"),
            ("Jo Park", "jo.park@example.com")
        ]
    );
    assert!(odd.reply_to.is_empty());
    assert_eq!(odd.in_reply_to.as_deref(), Some("<one@example.com>"));

    let saved = read("mbox-from-line");
    assert_eq!(saved.headers.subject, "Saved from a mailbox file");
    assert_eq!(
        names(&saved.headers.from),
        [("Ana Ruiz", "ana.ruiz@example.com")]
    );
}

#[test]
fn long_lines_are_read_whole() {
    let header = read("header-line-10kb");
    assert_eq!(
        header.headers.subject,
        "After a header line of ten kilobytes"
    );
    assert_eq!(header.references.len(), 400);
    assert_eq!(header.references[399], "<ref.399@example.com>");

    let line = read("body-line-2mb");
    assert_eq!(line.body.len(), 2 * 1024 * 1024);
    let shown = plain::render(&line.body, 80);
    // Every row full to the last word that fits: forty-four characters of
    // words, so at most seventy-nine of eighty cells, and never more.
    assert!(shown.lines.len() >= line.body.len() / 80);
    assert!(shown.lines.len() <= line.body.len() / 40);
    assert!(shown.lines.iter().all(|row| row.text().len() <= 80));
}

#[test]
fn line_endings_of_every_kind_are_line_endings() {
    let endings = read("bare-cr-and-bare-lf");
    assert_eq!(endings.headers.subject, "Line endings of every kind");
    assert!(endings.body.starts_with(
        "A line ended by a line feed alone.\nA line ended by a carriage return alone.\n\
         A line ended by both.\nA line ended the wrong way round.\n"
    ));
    // A carriage return does not get to overwrite the line before it.
    assert!(endings.body.contains("Pay Ana 100\nPay Eve 900\n"));
    assert!(endings.body.ends_with("The end.\n"));

    let mixed = read("mixed-crlf-lf");
    assert_eq!(
        mixed.headers.subject,
        "Some lines end one way, some the other"
    );
    assert_eq!(
        mixed.body,
        "First line, CRLF.\nSecond line, LF.\nThird line, CRLF."
    );
    assert_eq!(files(&mixed), [("unix.txt", "text/plain", 14)]);

    let nul = read("nul-bytes");
    assert_eq!(nul.headers.subject, "NUL bytes in the subject");
    assert_eq!(
        names(&nul.headers.from),
        [("Sam Lee", "sam.lee@example.com")]
    );
    assert_eq!(nul.body, "Text with  NUL  bytes in it, and a run: end.");
    assert_eq!(files(&nul), [("nul.bin", "application/octet-stream", 4)]);
    // The file's own bytes are not Quill's to clean.
    let raw = message("nul-bytes");
    assert_eq!(
        attachment_bytes(&raw, nul.attachments[0].part).as_deref(),
        Some(&b"\x00\x01\x02\x03"[..])
    );
}

#[test]
fn escape_sequences_in_headers_are_gone_and_the_words_are_not() {
    let subject = read("escape-sequences-subject");
    assert_eq!(subject.headers.subject, "Invoice overdue");
    assert_eq!(
        subject.headers.message_id.as_deref(),
        Some("<esc.subject@example.com>")
    );
    assert_eq!(subject.in_reply_to.as_deref(), Some("<before@example.com>"));
    assert_eq!(
        subject.references,
        ["<one@example.com>", "<two@example.com>"]
    );

    let sender = read("escape-sequences-sender-name");
    assert_eq!(
        names(&sender.headers.from),
        [("Your Bank", "sender@evil.example")]
    );
    assert_eq!(
        names(&sender.headers.to),
        [
            ("Robin Vale", "robin@example.org"),
            ("Sam Lee", "sam.lee@example.com")
        ]
    );
    assert_eq!(
        names(&sender.headers.cc),
        [
            // Written backwards under an override; shown as it is stored.
            ("elaV niboR", "rtl@example.org"),
            ("isolated \u{200f}Name", "iso@example.org"),
            ("", "esc@example.org"),
        ]
    );
    assert_eq!(sender.headers.subject, "Encoded subject txt.exe");
}

#[test]
fn file_names_cannot_disguise_what_they_are() {
    let raw = message("escape-sequences-file-name");
    let parsed = read("escape-sequences-file-name");
    assert_eq!(
        files(&parsed),
        [
            // Drawn with its override this read "invoiceexe.pdf".
            ("invoicefdp.exe", "application/octet-stream", 12),
            ("report.pdf", "application/pdf", 9),
            ("photosgpj.zip", "application/zip", 22),
            ("notestxt.scr tab line", "text/plain", 52),
        ]
    );
    // The id is reported clean and still finds its part.
    assert_eq!(
        parsed.attachments[1].content_id.as_deref(),
        Some("file@example.com")
    );
    assert_eq!(
        part_by_content_id(&raw, "file@example.com")
            .map(|(kind, _)| kind)
            .as_deref(),
        Some("application/pdf")
    );
    assert!(
        attachment_bytes(&raw, parsed.attachments[0].part)
            .is_some_and(|exe| exe.starts_with(b"MZ"))
    );
}

#[test]
fn escape_sequences_in_bodies_are_gone_and_the_words_are_not() {
    let parsed = read("escape-sequences-body");
    assert_eq!(
        parsed.body.lines().take(5).collect::<Vec<_>>(),
        [
            "Dear Robin,",
            "Nothing was copied to your clipboard.",
            "https://www.mybank.example/ is a real link.",
            "Pay Ana 100",
            "Pay Eve 900",
        ]
    );
    assert!(
        parsed
            .body
            .contains("Backspaces: secretpublic. Bell DEL FF VT SO.")
    );

    let shown = plain::render(&parsed.body, 80);
    let rows = text(&shown);
    assert_eq!(rows[0], "Dear Robin,");
    assert_eq!(
        shown.links,
        ["https://www.mybank.example/", "https://example.com/"]
    );
    let quoted = shown
        .lines
        .iter()
        .find(|line| line.text().starts_with("> quoted line"))
        .expect("the quoted line");
    assert!(quoted.spans.iter().all(|span| span.style.quote == 1));
    let last = shown.lines.last().expect("a signature");
    assert_eq!(last.text(), "Blinking signature");
    assert!(last.spans[0].style.dim);

    let markup = parsed.html.as_deref().expect("an HTML body");
    let shown = html::render(markup, 80).expect("something to show");
    let rows = text(&shown);
    assert_eq!(
        rows[..7],
        [
            "Dear Robin,",
            "",
            "Nothing was copied to your clipboard.",
            "",
            "A link[1]",
            "",
            "Pay Ana 100 Pay Eve 900",
        ]
    );
    assert!(rows.contains(&"[A chart]".to_owned()));
    assert_eq!(shown.links, ["https://example.com/"]);
    assert!(
        rows.iter()
            .all(|row| !row.contains("owned") && !row.contains("cm0g"))
    );

    let (body, _) = sanitized(&parsed, false);
    assert!(body.contains("<p>Dear Robin,</p>"));
    assert!(body.contains(
        "<a href=\"https://example.com/\" title=\"tip\" rel=\"noopener noreferrer\">A link</a>"
    ));
    assert!(!body.contains("owned") && !body.contains("cm0g"));
}

#[test]
fn html_only_mail_is_readable_as_text() {
    let parsed = read("html-only");
    assert!(parsed.html.is_some() && parsed.html_first && !parsed.has_plain);
    assert_eq!(
        parsed.body,
        "# Your hold is ready\n\nHello Robin,\n\n\
         The book you asked for, The Sea, the Sea, is waiting at the Riverside branch.\n\
         We keep it for seven days.\n* Pick up by: 21 November\n* Desk hours: 9 to 6\n\n\
         You can manage your holds[1] or write to us[2].\n\n\
         Riverside Library · 1 River Street\n\n\
         [1] https://example.org/holds/1042\n[2] mailto:holds@example.org\n"
    );
    let (body, result) = sanitized(&parsed, false);
    assert_eq!(result.blocked, 0);
    assert!(body.contains("<h1>Your hold is ready</h1>"));
    assert!(body.contains(
        "<style>body { font-family: sans-serif } .small { color: #666; font-size: 12px }</style>"
    ));
    // The message's own title and charset are not carried over; ours are.
    assert_eq!(result.html.matches("<title>").count(), 1);
    assert!(result.html.contains("<title>Your hold is ready</title>"));
    assert_eq!(result.html.matches("charset").count(), 1);
}

#[test]
fn large_and_deep_html_is_laid_out() {
    let table = read("html-giant-table");
    let shown = html::render(table.html.as_deref().expect("an HTML body"), 80).expect("a table");
    assert_eq!(shown.links.len(), 600);
    assert_eq!(shown.links[599], "https://example.com/items/600");
    let rows = text(&shown);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("600│Item 600 & parts")),
        "{:?}",
        &rows[..6]
    );
    let (body, _) = sanitized(&table, false);
    assert_eq!(body.matches("<tr>").count(), 601);

    let deep = read("html-5000-nested-divs");
    assert_eq!(deep.body, "The text at the bottom.\n");
    let (body, _) = sanitized(&deep, false);
    assert_eq!(body.matches("<div>").count(), 5000);
    assert!(body.contains("The text at the bottom."));

    let nested = read("html-nested-tables-and-styles");
    let rows =
        text(&html::render(nested.html.as_deref().expect("an HTML body"), 80).expect("text"));
    assert_eq!(rows[0], "Forty tables deep.");
    assert_eq!(rows[1], "Emphasis, a hundred and twenty deep.");
    assert!(
        rows.iter()
            .any(|row| row.ends_with("> Quoted thirty times."))
    );
    assert!(
        rows.iter()
            .any(|row| row.ends_with("* A list thirty deep."))
    );
    // The style sheet has an escape in it, which nothing safe can be made
    // of, so the page goes without it.
    let (body, _) = sanitized(&nested, false);
    assert!(!body.contains("<style"));
}

#[test]
fn hostile_html_is_text_in_the_reader_and_harmless_in_the_page() {
    let parsed = read("html-hostile-script-style");
    let rows =
        text(&html::render(parsed.html.as_deref().expect("an HTML body"), 80).expect("text"));
    assert_eq!(rows[0], "Your account needs attention.");
    assert!(rows.contains(&"! [3] goes to evil.example, not www.mybank.example".to_owned()));
    assert!(
        rows.iter()
            .all(|row| !row.contains("steal") && !row.contains("document."))
    );

    for remote in [false, true] {
        let (body, result) = sanitized(&parsed, remote);
        assert!(body.contains("Your account needs attention."));
        for gone in [
            "steal",
            "<script",
            "<form",
            "<input",
            "<button",
            "<iframe",
            "<object",
            "<embed",
            "<svg",
            "<math",
            "<base",
            "<link",
            "<meta",
            "refresh",
            "@import",
            "expression",
            "javascript",
            "file:",
            "data:text",
            "onload",
            "onclick",
            "onerror",
            "onmouseover",
            "autoplay",
            "u\\72l",
        ] {
            assert!(!body.contains(gone), "{gone} with remote {remote}");
        }
        // The link that lies about where it goes still goes there, and
        // only there; the others lost their targets.
        assert_eq!(body.matches("href=").count(), 2);
        assert!(body.contains("<a href=\"https://evil.example/login\" rel=\"noopener noreferrer\">https://www.mybank.example/login</a>"));
        if remote {
            assert_eq!(result.blocked, 0);
            assert!(body.contains("https://evil.example/pixel.gif?id=robin"));
        } else {
            // What allowing remote content would bring back: the second
            // style sheet's picture, the paragraph's, the pixel, the logo,
            // the video and the overlay. The first style sheet is gone
            // either way: it has an escape in it.
            assert_eq!(result.blocked, 6, "{body}");
            assert_eq!(body.matches("evil.example").count(), 2, "{body}");
            assert!(body.contains("<span class=\"quill-blocked\">Logo</span>"));
        }
    }
}

#[test]
fn a_newsletter_loads_nothing_until_it_is_allowed_to() {
    let parsed = read("newsletter-tracking-pixel");
    assert_eq!(parsed.headers.subject, "Black Friday starts early 🎉");
    assert!(parsed.html_first && parsed.has_plain);
    let rows =
        text(&html::render(parsed.html.as_deref().expect("an HTML body"), 80).expect("text"));
    assert!(rows.contains(&"# Black Friday starts early.".to_owned()));
    // The hidden preview line and the pixels are not text.
    assert!(
        rows.iter()
            .all(|row| !row.contains("40% off") && !row.contains("open.gif"))
    );

    let (body, result) = sanitized(&parsed, false);
    // The logo, two products, two pixels, a table's background and a
    // cell's.
    assert_eq!(result.blocked, 7, "{body}");
    assert!(!body.contains("cdn.example.com") && !body.contains("track.example"));
    for alt in ["Example Works", "Desk lamp", "Notebook"] {
        assert!(
            body.contains(&format!("<span class=\"quill-blocked\">{alt}</span>")),
            "{alt}"
        );
    }
    // Links are still links. A pixel had no description and leaves nothing.
    assert_eq!(body.matches("rel=\"noopener noreferrer\"").count(), 4);
    assert_eq!(body.matches("<img").count(), 0);
    assert!(body.contains("@media only screen and (max-width: 600px)"));

    let (body, result) = sanitized(&parsed, true);
    assert_eq!(result.blocked, 0);
    assert_eq!(body.matches("<img").count(), 5);
    assert!(body.contains("https://track.example.com/open.gif?m=news-1&amp;u=robin"));
    assert!(result.html.contains("img-src data: 'self' https: http:;"));
}

#[test]
fn links_that_lie_are_called_out() {
    let parsed = read("phishing-disguised-link");
    assert_eq!(names(&parsed.reply_to), [("", "accounts@evil.example")]);
    let shown = html::render(parsed.html.as_deref().expect("an HTML body"), 80).expect("text");
    let warnings: Vec<String> = text(&shown)
        .into_iter()
        .filter(|row| row.starts_with("! ["))
        .collect();
    assert_eq!(
        warnings,
        [
            "! [1] goes to evil.example, not www.mybank.example",
            "! [2] goes to www.mybank.example.evil.example, not www.mybank.example",
            "! [3] goes to accounts@evil.example, not support@mybank.example",
            "! [4] goes to xn--mybnk-6ve.example, not myb\u{430}nk.example",
        ]
    );
    assert_eq!(shown.links.len(), 6);
    // The same in the plain text made of it, for search and for a reply.
    assert!(
        parsed
            .body
            .contains("! [1] goes to evil.example, not www.mybank.example")
    );
}

#[test]
fn quotes_signatures_and_addresses_in_plain_text() {
    let parsed = read("plain-quotes-signature-links");
    assert_eq!(
        parsed.in_reply_to.as_deref(),
        Some("<q4-roadmap.2@example.com>")
    );
    assert_eq!(
        parsed.references,
        ["<q4-roadmap.1@example.com>", "<q4-roadmap.2@example.com>"]
    );
    let shown = plain::render(&parsed.body, 60);
    assert_eq!(
        shown.links,
        [
            "https://example.org/notes/q4-roadmap?rev=3#comments",
            "mailto:roadmap@example.org",
            "https://example.com/plans/q4",
            "https://example.org/people/robin",
        ]
    );
    let depth_of = |start: &str| {
        let line = shown
            .lines
            .iter()
            .find(|line| line.text().starts_with(start))
            .unwrap_or_else(|| panic!("a line starting {start:?}"));
        (line.spans[0].style.quote, line.spans[0].style.dim)
    };
    assert_eq!(depth_of("Looks good."), (0, false));
    assert_eq!(depth_of("> Thanks for the quick read."), (1, false));
    // A long quoted line keeps its marker on every row it wraps to.
    assert_eq!(depth_of("> 47 and kept the freeze"), (1, false));
    assert_eq!(depth_of("> working days for the checks."), (1, false));
    assert_eq!(depth_of("> > Can we move the launch"), (2, false));
    assert_eq!(depth_of("> > same day as the audit."), (2, false));
    assert_eq!(depth_of("> > > Original plan:"), (3, false));
    assert_eq!(depth_of("> > > week 46)."), (3, false));
    assert_eq!(depth_of(">> Also: who owns"), (2, false));
    assert_eq!(depth_of("Four days is tight"), (0, false));
    assert_eq!(depth_of("        Tabbed line     with columns"), (0, false));
    assert_eq!(depth_of("-- "), (0, true));
    assert_eq!(depth_of("Robin Vale"), (0, true));
    assert_eq!(depth_of("> not a quote, part of"), (1, true));
    for line in &shown.lines {
        assert!(line.text().chars().count() <= 60, "{:?}", line.text());
    }
}
