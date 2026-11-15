//! Limits that keep hostile HTML from stalling Quill or exhausting it.
//!
//! An HTML parser does what the standard says, and the standard was written
//! for browsers that have limits of their own. Left to itself:
//!
//! - It does work for every tag in proportion to how deeply the tags before
//!   it are nested. A hundred thousand elements inside one another take
//!   minutes.
//! - It reopens unclosed `<b>`, `<i>`, `<font>` and their kind in every
//!   paragraph that follows them. Three thousand of those and three
//!   thousand paragraphs, forty kilobytes of mail, become four million
//!   elements.
//! - Its writer looks ahead through the rest of a piece of text each time
//!   it escapes a character. A megabyte of `&nbsp;` takes minutes to write
//!   back out.
//!
//! None of that can be seen without doing the parse: what a tag does
//! depends on every tag before it. So the parse itself is what is limited,
//! by the clock, and a document that runs out of time is not shown as HTML
//! at all. The reader has the plain text to fall back on. Time is also
//! what bounds memory: a parser stopped after so many milliseconds has
//! built no more than so many elements.
//!
//! A clock makes the answer depend on the machine, so it is kept away from
//! every document that can be shown to be small whatever it does: few
//! enough tags that nesting cannot matter, and few enough of the reopening
//! kind that they cannot multiply. Such a document always gives the same
//! answer. For the rest the allowance is many times what an honest
//! document of that size needs.

use std::io;
use std::time::{Duration, Instant};

/// A document that opens no more tags than this cannot nest deeply enough
/// to be slow. Its cost is at worst the square of this.
const UNTIMED_TAGS: usize = 6_000;

/// A document cannot grow to more elements than its tags, times one more
/// than its tags of the reopening kind. Under this many, it is parsed
/// without a clock.
const UNTIMED_ELEMENTS: usize = 300_000;

/// How much slower a build without optimisation is at parsing, generously.
const SLOWNESS: u32 = if cfg!(debug_assertions) { 20 } else { 1 };

/// The time any timed document is allowed, however small.
const TIME_FLOOR: Duration = Duration::from_millis(50);

/// The further time allowed for each byte: sixty milliseconds a megabyte,
/// where an honest document takes ten or twenty.
const TIME_PER_BYTE: Duration = Duration::from_nanos(60);

/// How much is handed to the parser between looks at the clock.
const CHUNK: usize = 1024;

/// How many pieces the writer may hand over between looks at the clock.
const WRITES_PER_LOOK: u32 = 64;

/// The deepest a document may nest and still be drawn as text.
const MAX_DEPTH: usize = 6_000;

/// The most a document may weigh and still be drawn as text: the depths of
/// all its leaves, added up. Every piece of text carries a mark for each
/// element around it, so this is what laying it out costs.
const MAX_WEIGHT: usize = 3_000_000;

/// The elements the parser reopens wherever they were left unclosed.
const REOPENED: &[&[u8]] = &[
    b"a", b"b", b"big", b"code", b"em", b"font", b"i", b"nobr", b"s", b"small", b"strike",
    b"strong", b"tt", b"u",
];

/// How long a document of `length` bytes may take to parse, or to write.
pub(crate) fn allowance(length: usize) -> Duration {
    let bytes = u32::try_from(length).unwrap_or(u32::MAX);
    (TIME_FLOOR + TIME_PER_BYTE * bytes) * SLOWNESS
}

/// Whether `html` is small enough, whatever it does, to need no clock.
fn is_small(html: &str) -> bool {
    let bytes = html.as_bytes();
    let mut tags = 0usize;
    let mut reopened = 0usize;
    // Every tag the parser will open starts with `<` and a letter, so
    // counting those counts too many, never too few.
    for (at, pair) in bytes.windows(2).enumerate() {
        if pair[0] != b'<' || !pair[1].is_ascii_alphabetic() {
            continue;
        }
        tags += 1;
        if tags > UNTIMED_TAGS {
            return false;
        }
        let name = &bytes[at + 1..];
        let length = name
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .count();
        if REOPENED
            .iter()
            .any(|known| name[..length].eq_ignore_ascii_case(known))
        {
            reopened += 1;
        }
    }
    tags * (reopened + 1) <= UNTIMED_ELEMENTS
}

/// A document, read in pieces for the parser, with a deadline unless it is
/// small. Past the deadline, reading fails and the parse is abandoned.
pub(crate) struct Source<'a> {
    rest: &'a [u8],
    deadline: Option<Instant>,
}

impl<'a> Source<'a> {
    pub(crate) fn new(html: &'a str) -> Self {
        Self {
            rest: html.as_bytes(),
            deadline: (!is_small(html)).then(|| Instant::now() + allowance(html.len())),
        }
    }
}

impl io::Read for Source<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let count = self.rest.len().min(buffer.len()).min(CHUNK);
        let (now, later) = self.rest.split_at(count);
        buffer[..count].copy_from_slice(now);
        self.rest = later;
        Ok(count)
    }
}

/// Where a parsed document is written back out, with a deadline and a limit
/// on how much may be written. Past either, writing fails and what was
/// written is thrown away.
pub(crate) struct Sink {
    written: Vec<u8>,
    limit: usize,
    deadline: Instant,
    writes: u32,
}

impl Sink {
    /// For a document that was `length` bytes before it was parsed. Written
    /// out it may be several times that, since every `&` becomes `&amp;`,
    /// and no more.
    pub(crate) fn new(length: usize) -> Self {
        Self {
            written: Vec::with_capacity(length + length / 8),
            limit: 8 * length + 64 * 1024,
            deadline: Instant::now() + allowance(length),
            writes: 0,
        }
    }

    /// What was written, if it is text.
    pub(crate) fn into_string(self) -> Option<String> {
        String::from_utf8(self.written).ok()
    }
}

impl io::Write for Sink {
    fn write(&mut self, piece: &[u8]) -> io::Result<usize> {
        if self.written.len() + piece.len() > self.limit {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.writes = self.writes.wrapping_add(1);
        if self.writes.is_multiple_of(WRITES_PER_LOOK) && Instant::now() >= self.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.written.extend_from_slice(piece);
        Ok(piece.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The widest a table cell may claim to be, in columns. The renderer adds
/// these up, and a message is free to claim billions.
const MAX_COLUMN_SPAN: u64 = 1_000;

/// The furthest from zero a numbered list may start. The renderer adds the
/// number of items to it.
const MAX_LIST_START: i64 = 1_000_000_000;

/// Readies a parsed document for the text renderer, and says whether it is
/// shallow and light enough to lay out at all.
///
/// The renderer was not written for hostile input: there are documents it
/// panics on and documents it never finishes. What sets those off is taken
/// out of the document here, before it sees them:
///
/// - Style sheets. It matches every rule against every element, and its
///   selector parser panics on a number too big for it.
/// - `style` attributes, all but the one thing wanted from them: whether
///   the element is hidden, which is how mail hides what it hides. Asked
///   to keep white space as written, the renderer can lose count of a line.
/// - `rowspan`. A cell that reaches below the last row of its table makes
///   it look up a column that is not there.
/// - `colspan` and a list's `start` beyond any sensible size, which
///   overflow its sums.
pub(crate) fn prepare(dom: &html2text::RcDom) -> bool {
    let mut weight = 0usize;
    let mut waiting = vec![(dom.document.clone(), 0usize)];
    while let Some((node, depth)) = waiting.pop() {
        if depth > MAX_DEPTH {
            return false;
        }
        if let html2text::Element { attrs, .. } = &node.data {
            attrs.borrow_mut().retain_mut(|attribute| {
                match tamed(&attribute.name.local, &attribute.value) {
                    Tamed::Kept => true,
                    Tamed::Dropped => false,
                    Tamed::Replaced(value) => {
                        attribute.value = value.into();
                        true
                    }
                }
            });
        }
        let mut children = node.children.borrow_mut();
        children.retain(|child| {
            !matches!(&child.data, html2text::Element { name, .. } if &*name.local == "style")
        });
        if children.is_empty() {
            weight += depth;
            if weight > MAX_WEIGHT {
                return false;
            }
        }
        waiting.extend(children.iter().map(|child| (child.clone(), depth + 1)));
    }
    true
}

/// What becomes of an attribute before the renderer sees it.
#[derive(Debug, PartialEq, Eq)]
enum Tamed {
    Kept,
    Dropped,
    Replaced(String),
}

fn tamed(name: &str, value: &str) -> Tamed {
    let value = value.trim();
    match name {
        "rowspan" => Tamed::Dropped,
        "colspan" => match value.parse::<u64>() {
            Ok(1..=MAX_COLUMN_SPAN) => Tamed::Kept,
            Ok(0) | Err(_) => Tamed::Dropped,
            Ok(_) => Tamed::Replaced(MAX_COLUMN_SPAN.to_string()),
        },
        "start" => match value.parse::<i64>() {
            Ok(start) if (-MAX_LIST_START..=MAX_LIST_START).contains(&start) => Tamed::Kept,
            _ => Tamed::Dropped,
        },
        "style" if hides(value) => Tamed::Replaced("display:none".to_owned()),
        "style" => Tamed::Dropped,
        _ => Tamed::Kept,
    }
}

/// Whether a `style` attribute says the element is not shown. The last
/// `display` in it is the one that counts.
fn hides(style: &str) -> bool {
    style
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .rfind(|(name, _)| name.trim().eq_ignore_ascii_case("display"))
        .is_some_and(|(_, value)| {
            let value = value.to_ascii_lowercase();
            value.trim().trim_end_matches("!important").trim_end() == "none"
        })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn a_small_document_is_read_whole_and_never_timed() {
        let html = "<p>Hello <b>Robin</b>.</p>".repeat(100);
        let mut source = Source::new(&html);
        assert!(source.deadline.is_none());
        let mut read = String::new();
        source.read_to_string(&mut read).expect("no deadline");
        assert_eq!(read, html);
    }

    #[test]
    fn what_could_be_slow_or_grow_is_timed() {
        // Small whatever it does: no clock.
        for small in [
            String::new(),
            "<div>".repeat(UNTIMED_TAGS),
            "<p>x</p>".repeat(UNTIMED_TAGS),
            "<b>x</b><p>y</p>".repeat(300),
            // Text that only mentions a less-than sign is not a tag.
            "1 < 2 <3 ".repeat(UNTIMED_TAGS),
        ] {
            assert!(is_small(&small), "{}", &small[..small.len().min(40)]);
        }
        // Too many tags to rule out deep nesting, or enough of the kind
        // the parser reopens for them to multiply.
        for large in [
            "<div>".repeat(UNTIMED_TAGS + 1),
            "<b id=1><p>x".repeat(600),
            "<FONT color=red><P>x".repeat(600),
            "<a href=x><i/><Strong\n><tt\t>".repeat(300),
        ] {
            assert!(!is_small(&large), "{}", &large[..40]);
            assert!(Source::new(&large).deadline.is_some());
        }
        // Names that only start like one of them do not count.
        assert!(is_small(&"<br><img><article><body><iframe>".repeat(1_000)));
    }

    #[test]
    fn attributes_the_renderer_cannot_be_trusted_with_are_tamed() {
        for (name, value, expected) in [
            ("rowspan", "2", Tamed::Dropped),
            ("rowspan", "18446744073709551615", Tamed::Dropped),
            ("colspan", "3", Tamed::Kept),
            ("colspan", " 1000 ", Tamed::Kept),
            ("colspan", "1001", Tamed::Replaced("1000".to_owned())),
            (
                "colspan",
                "18446744073709551615",
                Tamed::Replaced("1000".to_owned()),
            ),
            ("colspan", "99999999999999999999999", Tamed::Dropped),
            ("colspan", "0", Tamed::Dropped),
            ("colspan", "-1", Tamed::Dropped),
            ("colspan", "two", Tamed::Dropped),
            ("start", "5", Tamed::Kept),
            ("start", "-3", Tamed::Kept),
            ("start", "9223372036854775807", Tamed::Dropped),
            ("start", "-9223372036854775808", Tamed::Dropped),
            ("start", "", Tamed::Dropped),
            (
                "style",
                "display:none",
                Tamed::Replaced("display:none".to_owned()),
            ),
            (
                "style",
                "color: red; DISPLAY : None !important; max-height:0",
                Tamed::Replaced("display:none".to_owned()),
            ),
            ("style", "display:none;display:block", Tamed::Dropped),
            (
                "style",
                "white-space:pre;color:rgb(999999999999,1,2)",
                Tamed::Dropped,
            ),
            ("style", "display:nonesuch", Tamed::Dropped),
            ("style", "", Tamed::Dropped),
            ("href", "https://example.com/", Tamed::Kept),
            ("class", "display:none", Tamed::Kept),
        ] {
            assert_eq!(tamed(name, value), expected, "{name}={value:?}");
        }
    }

    #[test]
    fn the_allowance_grows_with_the_document() {
        assert_eq!(allowance(0), TIME_FLOOR * SLOWNESS);
        assert_eq!(
            allowance(1_000_000),
            (TIME_FLOOR + Duration::from_millis(60)) * SLOWNESS
        );
        assert!(allowance(usize::MAX) > allowance(2 * 1024 * 1024));
    }

    #[test]
    fn past_the_deadline_reading_stops_and_the_parse_with_it() {
        let html = "<i>x</i>".repeat(UNTIMED_TAGS + 1);
        let mut source = Source::new(&html);
        // With time in hand it reads like any other, a piece at a time.
        let mut buffer = [0u8; 4096];
        assert_eq!(source.read(&mut buffer).expect("in time"), CHUNK);
        source.deadline = Some(Instant::now());
        assert_eq!(
            source.read(&mut buffer).map_err(|error| error.kind()),
            Err(io::ErrorKind::TimedOut)
        );
        let mut late = Source::new(&html);
        late.deadline = Some(Instant::now());
        assert!(html2text::config::plain().parse_html(late).is_err());
    }

    #[test]
    fn writing_stops_at_the_limit_and_at_the_deadline() {
        let mut sink = Sink::new(10);
        sink.write_all(b"<p>Hello.</p>").expect("room");
        assert_eq!(sink.into_string().as_deref(), Some("<p>Hello.</p>"));

        let mut full = Sink::new(0);
        let piece = [b'x'; 1024];
        let outcome = (0..100).try_for_each(|_| full.write_all(&piece));
        assert_eq!(
            outcome.map_err(|error| error.kind()),
            Err(io::ErrorKind::FileTooLarge)
        );
        assert!(full.written.len() <= full.limit);

        let mut late = Sink::new(1_000_000);
        late.deadline = Instant::now();
        let outcome = (0..=WRITES_PER_LOOK).try_for_each(|_| late.write_all(b"x"));
        assert_eq!(
            outcome.map_err(|error| error.kind()),
            Err(io::ErrorKind::TimedOut)
        );
    }
}
