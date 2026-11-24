//! Plain text laid out for the reader.
//!
//! The same shape [`html::render`] returns, so the reader draws both the
//! same way: lines wrapped to the pane, quoted lines marked with how deep
//! the quote is, the signature dimmed, and every address a link.
//!
//! The text is the sender's and stays as written. Nothing is reflowed
//! beyond wrapping lines too long for the pane, and an address is marked
//! where it stands, not listed again at the end.

use std::collections::HashMap;
use std::ops::Range;

use crate::html::{self, Line, Rendered, Span, Style, Wrapped};
use crate::{clean, links, width};

/// Text longer than this is cut short, with a line saying so: nobody reads
/// further in a pane, and laying it out costs memory for every row.
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// What the last line says when the text was cut short.
pub const CUT_SHORT: &str = "[The rest of this message is too long to show here.]";

/// The line that starts a signature, by convention: two dashes and a space.
const SIGNATURE: &str = "-- ";

/// Tabs move to the next multiple of this many cells.
const TAB_STOP: usize = 8;

/// Lays `text` out in rows no wider than `width` cells. A pane narrower
/// than the narrowest layout clips it, as with HTML.
pub fn render(text: &str, width: usize) -> Rendered {
    let width = width.max(html::MIN_WIDTH);
    let (text, cut_short) = match text.len() > MAX_TEXT_BYTES {
        true => (&text[..text.floor_char_boundary(MAX_TEXT_BYTES)], true),
        false => (text, false),
    };
    let text = clean::body(text);

    let mut layout = Layout {
        width,
        lines: Vec::new(),
        links: Vec::new(),
        known: HashMap::new(),
    };
    let mut signature = false;
    for line in text.split('\n') {
        let line = expand_tabs(line);
        let (quote, prefix) = quote_prefix(&line);
        // Everything from the marker on is the signature, the marker too.
        signature |= quote == 0 && line == SIGNATURE;
        let style = Style {
            dim: signature,
            quote,
            ..Style::default()
        };
        layout.line(&line[..prefix], &line[prefix..], style);
    }
    while layout.lines.last().is_some_and(Line::is_blank) {
        layout.lines.pop();
    }
    if cut_short {
        let aside = Style {
            dim: true,
            ..Style::default()
        };
        layout.lines.push(Line::default());
        layout.line("", CUT_SHORT, aside);
    }
    Rendered {
        lines: layout.lines,
        links: layout.links,
    }
}

/// How deeply a line is quoted, and how many bytes its markers take: `>`,
/// `> >` and `>>` in any mixture, with the space that follows them.
pub(crate) fn quote_prefix(line: &str) -> (u8, usize) {
    let bytes = line.as_bytes();
    let mut level = 0u8;
    let mut at = 0;
    while bytes.get(at) == Some(&b'>') {
        level = level.saturating_add(1);
        at += 1;
        if bytes.get(at) == Some(&b' ') {
            at += 1;
        }
    }
    (level, at)
}

/// `text` wrapped to `width` cells, as plain strings. For the asides the
/// renderers write themselves.
pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    wrap(text, width.max(1))
        .into_iter()
        .map(|row| text[row].to_owned())
        .collect()
}

struct Layout {
    width: usize,
    lines: Vec<Line>,
    links: Vec<String>,
    /// Where each address already is in `links`.
    known: HashMap<String, usize>,
}

impl Layout {
    /// Lays out one line of the text: its quote markers, then what it says.
    fn line(&mut self, prefix: &str, content: &str, style: Style) {
        // Markers are kept on every row a quoted line wraps to, so a
        // wrapped quote still reads as a quote. A pile of markers wider
        // than half the pane is cut down to leave room for the words.
        let prefix = &prefix[..prefix.len().min(self.width / 2)];
        let available = self.width - prefix.len();

        let places = links::locate(content);
        let targets: Vec<usize> = places
            .iter()
            .map(|place| self.link(&content[place.clone()]))
            .collect();

        // Rows and addresses both run left to right, so each row starts
        // looking where the one before left off.
        let mut first = 0;
        let rows = wrap(content, available);
        for (index, row) in rows.iter().enumerate() {
            // A row that is not the line's last was broken to fit: where
            // a space was dropped, or in the middle of a long word.
            let mut line = Line {
                wrapped: match rows.get(index + 1) {
                    None => Wrapped::No,
                    Some(next) if next.start > row.end => Wrapped::AtSpace,
                    Some(_) => Wrapped::InWord,
                },
                ..Line::default()
            };
            push(&mut line, prefix, style, None);
            let mut at = row.start;
            while places.get(first).is_some_and(|place| place.end <= at) {
                first += 1;
            }
            for (place, target) in places[first..].iter().zip(&targets[first..]) {
                if place.start >= row.end {
                    break;
                }
                let from = place.start.max(at);
                let to = place.end.min(row.end);
                push(&mut line, &content[at..from], style, None);
                let linked = Style {
                    link: true,
                    ..style
                };
                push(&mut line, &content[from..to], linked, Some(*target));
                at = to;
            }
            push(&mut line, &content[at..row.end], style, None);
            self.lines.push(line);
        }
    }

    /// The place of `url` in the list of links, added if it is new.
    fn link(&mut self, url: &str) -> usize {
        if let Some(index) = self.known.get(url) {
            return *index;
        }
        self.links.push(url.to_owned());
        self.known.insert(url.to_owned(), self.links.len() - 1);
        self.links.len() - 1
    }
}

fn push(line: &mut Line, text: &str, style: Style, link: Option<usize>) {
    if text.is_empty() {
        return;
    }
    line.spans.push(Span {
        text: text.to_owned(),
        style,
        link,
    });
}

/// `line` with each tab replaced by the spaces that reach the next stop.
fn expand_tabs(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\t') {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut expanded = String::with_capacity(line.len() + TAB_STOP);
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let spaces = TAB_STOP - column % TAB_STOP;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            expanded.push(c);
            column += width::of_char(c);
        }
    }
    std::borrow::Cow::Owned(expanded)
}

/// Splits `text` into rows of at most `available` cells, each a range of
/// bytes. Rows break at spaces where there are any, and the spaces a row is
/// broken at are dropped; a word longer than a row is broken between
/// characters. A line that fits is one row exactly as written, indentation
/// and all. There is always at least one row.
fn wrap(text: &str, available: usize) -> Vec<Range<usize>> {
    // A row must be able to hold one wide character.
    let available = available.max(2);
    if text.len() <= available || width::of_str(text) <= available {
        let whole = 0..text.len();
        return vec![whole];
    }

    let mut rows = Vec::new();
    // The row being filled starts here and has this many cells in it.
    let mut start = 0;
    let mut column = 0;
    // The last place the row could be broken: where the spaces after a
    // word begin and end, and the cells taken by the word after them.
    let mut gap: Option<(usize, usize)> = None;
    let mut word = 0;
    let mut has_word = false;
    let mut in_spaces = false;

    for cluster in width::clusters(text) {
        if &text[cluster.start..cluster.end] == " " {
            if has_word {
                let from = match gap {
                    Some((from, _)) if in_spaces => from,
                    _ => cluster.start,
                };
                gap = Some((from, cluster.end));
                word = 0;
                column += 1;
            } else if column + 1 >= available {
                // Indentation as wide as the pane is no indentation.
                start = cluster.end;
                column = 0;
            } else {
                column += 1;
            }
            in_spaces = true;
            continue;
        }
        in_spaces = false;

        // A cluster wider than a whole row is taken a character at a time.
        let whole = [(cluster.start, cluster.width)];
        let split: Vec<(usize, usize)>;
        let pieces: &[(usize, usize)] = if cluster.width > available {
            split = text[cluster.start..cluster.end]
                .char_indices()
                .map(|(at, c)| (cluster.start + at, width::of_char(c)))
                .collect();
            &split
        } else {
            &whole
        };
        for &(from, cells) in pieces {
            if column + cells > available {
                match gap {
                    // The word moves down whole when a row can hold it.
                    Some((row_end, next)) if word + cells <= available => {
                        rows.push(start..row_end);
                        start = next;
                        column = word;
                    }
                    _ => {
                        if has_word {
                            rows.push(start..from);
                        }
                        // Otherwise only indentation is left behind.
                        start = from;
                        column = 0;
                        word = 0;
                    }
                }
                gap = None;
            }
            column += cells;
            word += cells;
            has_word = true;
        }
    }
    let end = start + text[start..].trim_end_matches(' ').len();
    if end > start || rows.is_empty() {
        rows.push(start..end);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str, width: usize) -> Vec<String> {
        render(source, width).lines.iter().map(Line::text).collect()
    }

    fn rows(source: &str, available: usize) -> Vec<&str> {
        wrap(source, available)
            .into_iter()
            .map(|row| &source[row])
            .collect()
    }

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
    fn short_lines_are_left_as_written() {
        assert_eq!(
            text("Hi Robin,\n\n  indented   and  spaced\nBye.\n", 40),
            ["Hi Robin,", "", "  indented   and  spaced", "Bye."]
        );
        assert_eq!(render("", 40), Rendered::default());
        assert_eq!(render("\n\n\n", 40), Rendered::default());
    }

    #[test]
    fn long_lines_wrap_at_spaces() {
        assert_eq!(
            text(
                "The quarterly figures are attached and the margin target holds.",
                24
            ),
            [
                "The quarterly figures",
                "are attached and the",
                "margin target holds."
            ]
        );
        // Indentation stays on the first row only.
        assert_eq!(
            rows("    four spaces in, then words to wrap", 20),
            ["    four spaces in,", "then words to wrap"]
        );
        // The break takes every space it falls on.
        assert_eq!(
            rows("aaaa bbbb     cccc dddd", 10),
            ["aaaa bbbb", "cccc dddd"]
        );
    }

    #[test]
    fn a_word_too_long_for_a_row_is_broken() {
        assert_eq!(
            rows("see abcdefghijklmnopqrstuvwxyz now", 10),
            ["see", "abcdefghij", "klmnopqrst", "uvwxyz now"]
        );
        assert_eq!(rows("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn no_row_is_wider_than_asked() {
        let samples = [
            "The quarterly figures are attached and the margin target holds.",
            "陈静发来了一份很长很长的季度报告，请在周五之前看完并回复。",
            "emoji 🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉 party",
            "family 👨\u{200d}👩\u{200d}👧\u{200d}👦👨\u{200d}👩\u{200d}👧\u{200d}👦👨\u{200d}👩\u{200d}👧\u{200d}👦 and flags 🇪🇸🇫🇷🇯🇵🇰🇷🇪🇸🇫🇷🇯🇵🇰🇷",
            "a\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301} accents",
            "                                                  far indented words",
            "tabs\tand\tmore\ttabs\tthat\tgo\ton\tand\ton\tfor\tlong",
            "> > > quoted 陈静发来了一份很长很长的季度报告 and https://example.com/a/very/long/address/that/does/not/fit",
        ];
        for sample in samples {
            for width in [0, 1, 2, 3, 20, 21, 33, 80, 200] {
                let rendered = render(sample, width);
                assert!(!rendered.lines.is_empty(), "{sample:?} at {width}");
                for line in &rendered.lines {
                    let cells = width::of_str(&line.text());
                    assert!(
                        cells <= width.max(html::MIN_WIDTH),
                        "{cells} cells at {width}: {:?}",
                        line.text()
                    );
                }
                // Nothing but the spaces rows were broken at is lost.
                let squeeze = |text: &str| text.replace([' ', '\t', '>'], "");
                let shown: String = rendered.lines.iter().map(Line::text).collect();
                assert_eq!(squeeze(&shown), squeeze(sample), "at {width}");
            }
            for available in 0..12 {
                for row in rows(sample, available) {
                    assert!(
                        width::of_str(row) <= available.max(2),
                        "{row:?} at {available}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_cluster_wider_than_the_row_cannot_stall_it() {
        // Twenty people joined into one "character".
        let crowd = ["👨"; 20].join("\u{200d}");
        let wrapped = rows(&crowd, 6);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|row| width::of_str(row) <= 6));
    }

    #[test]
    fn quoted_lines_know_their_depth() {
        let rendered = render(
            "Sounds good.\n> Can we move it?\n> > To Friday.\n>> To Friday.\n>>>deep\n> >> mixed\n>",
            60,
        );
        let depths: Vec<(String, u8)> = rendered
            .lines
            .iter()
            .map(|line| {
                let depth = line.spans.first().map_or(0, |span| span.style.quote);
                // A line is quoted throughout or not at all.
                assert!(line.spans.iter().all(|span| span.style.quote == depth));
                (line.text(), depth)
            })
            .collect();
        assert_eq!(
            depths,
            [
                ("Sounds good.".to_owned(), 0),
                ("> Can we move it?".to_owned(), 1),
                ("> > To Friday.".to_owned(), 2),
                (">> To Friday.".to_owned(), 2),
                (">>>deep".to_owned(), 3),
                ("> >> mixed".to_owned(), 3),
                (">".to_owned(), 1),
            ]
        );
        // Not quotes: a marker that is not at the start.
        assert_eq!(quote_prefix(" > indented"), (0, 0));
        assert_eq!(quote_prefix("a > b"), (0, 0));
        assert_eq!(quote_prefix("> > x"), (2, 4));
        assert_eq!(quote_prefix(&">".repeat(300)), (255, 300));
    }

    #[test]
    fn a_wrapped_quote_keeps_its_markers() {
        assert_eq!(
            text(
                "> > The quarterly figures are attached and the margin target holds.",
                28
            ),
            [
                "> > The quarterly figures",
                "> > are attached and the",
                "> > margin target holds."
            ]
        );
        let rendered = render(">> one two three four five six seven eight nine ten", 22);
        assert!(rendered.lines.len() > 1);
        for line in &rendered.lines {
            assert!(line.text().starts_with(">> "), "{:?}", line.text());
            assert!(line.spans.iter().all(|span| span.style.quote == 2));
        }
    }

    #[test]
    fn a_quote_deeper_than_the_pane_still_shows_its_words() {
        let line = format!("{}words that matter", "> ".repeat(40));
        let rendered = render(&line, 20);
        let shown: String = rendered.lines.iter().map(Line::text).collect();
        assert!(shown.replace(['>', ' '], "").contains("wordsthatmatter"));
        for line in &rendered.lines {
            assert!(width::of_str(&line.text()) <= 20);
            assert!(line.spans.iter().all(|span| span.style.quote == 40));
        }
    }

    #[test]
    fn the_signature_is_dim() {
        let rendered = render(
            "See you there.\n-- \nAna Ruiz\nExample Works\n> not a quote of hers",
            60,
        );
        let dim: Vec<(String, bool)> = rendered
            .lines
            .iter()
            .map(|line| {
                let dim = line.spans.first().is_some_and(|span| span.style.dim);
                (line.text(), dim)
            })
            .collect();
        assert_eq!(
            dim,
            [
                ("See you there.".to_owned(), false),
                ("-- ".to_owned(), true),
                ("Ana Ruiz".to_owned(), true),
                ("Example Works".to_owned(), true),
                ("> not a quote of hers".to_owned(), true),
            ]
        );
        // Still a quote for colouring, as well as dim.
        assert_eq!(rendered.lines[4].spans[0].style.quote, 1);
    }

    #[test]
    fn only_the_exact_marker_starts_a_signature() {
        for not_a_marker in ["--", "--  ", " -- ", "-- x", "---", "> -- "] {
            let rendered = render(&format!("{not_a_marker}\nAna"), 60);
            let last = rendered.lines.last().expect("two lines");
            assert!(!last.spans[0].style.dim, "{not_a_marker:?}");
        }
        // With the line ending a server would put on it.
        let rendered = render("-- \r\nAna\r\n", 60);
        assert!(rendered.lines[1].spans[0].style.dim);
    }

    #[test]
    fn addresses_are_links_where_they_stand() {
        let rendered = render(
            "The agenda is at https://example.com/agenda. Write to \
             mailto:robin@example.com or see <http://example.com/map>.\n\
             Again: https://example.com/agenda",
            200,
        );
        assert_eq!(
            rendered.links,
            [
                "https://example.com/agenda",
                "mailto:robin@example.com",
                "http://example.com/map",
            ]
        );
        let linked: Vec<(&str, Option<usize>, bool)> = rendered
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| (span.text.as_str(), span.link, span.style.link))
            .collect();
        assert_eq!(
            linked,
            [
                ("The agenda is at ", None, false),
                ("https://example.com/agenda", Some(0), true),
                (". Write to ", None, false),
                ("mailto:robin@example.com", Some(1), true),
                (" or see <", None, false),
                ("http://example.com/map", Some(2), true),
                (">.", None, false),
                ("Again: ", None, false),
                ("https://example.com/agenda", Some(0), true),
            ]
        );
        // The text is as written: no numbers, and no list at the end.
        assert_eq!(rendered.lines.len(), 2);
    }

    #[test]
    fn an_address_broken_over_rows_is_one_link() {
        let rendered = render(
            "> see https://example.com/a/very/long/address/that/does/not/fit/on/a/row",
            30,
        );
        assert_eq!(
            rendered.links,
            ["https://example.com/a/very/long/address/that/does/not/fit/on/a/row"]
        );
        let pieces: String = rendered
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .filter(|span| span.link == Some(0))
            .map(|span| {
                assert!(span.style.link && span.style.quote == 1);
                span.text.as_str()
            })
            .collect();
        assert_eq!(pieces, rendered.links[0]);
        assert!(rendered.lines.len() >= 3);
    }

    #[test]
    fn what_will_not_open_is_not_a_link() {
        let rendered = render(
            "file:///etc/passwd javascript:alert(1) ftp://example.com/x example.com",
            200,
        );
        assert!(rendered.links.is_empty());
        assert!(
            rendered.lines[0]
                .spans
                .iter()
                .all(|span| span.link.is_none() && !span.style.link)
        );
    }

    #[test]
    fn escape_sequences_never_reach_a_span() {
        let rendered = render(
            "\u{1b}]0;pwned\u{7}Hello\u{1b}[2J \u{1b}]8;;https://evil.example/\u{1b}\\bank\u{1b}]8;;\u{1b}\\\n\
             \u{9b}31mred\u{0}\u{8}\u{7f} tab\there\rover\n\
             \u{1b}]52;c;cm0gLXJmIH4=\u{7}https://example.com/\u{1b}[0m",
            60,
        );
        assert_eq!(
            rendered.lines.iter().map(Line::text).collect::<Vec<_>>(),
            ["Hello bank", "red tab here", "over", "https://example.com/"]
        );
        assert_eq!(rendered.links, ["https://example.com/"]);
        for span in rendered.lines.iter().flat_map(|line| &line.spans) {
            assert!(!span.text.contains(clean::is_control), "{:?}", span.text);
        }
    }

    #[test]
    fn text_past_the_limit_is_cut_short_and_says_so() {
        let long = "word ".repeat(MAX_TEXT_BYTES / 5 + 10);
        let rendered = render(&long, 200);
        let last = rendered.lines.last().expect("lines");
        assert_eq!(last.text(), CUT_SHORT);
        assert_eq!(last.spans[0].style, Style { dim: true, ..NONE });
        // A character that straddles the limit is not split.
        let wide = "é".repeat(MAX_TEXT_BYTES / 2 + 10);
        assert_eq!(
            render(&wide, 80).lines.last().map(Line::text).as_deref(),
            Some(CUT_SHORT)
        );
    }

    #[test]
    fn a_line_of_two_megabytes_is_laid_out_in_bounded_rows() {
        let line = "x".repeat(2 * 1024 * 1024);
        let rendered = render(&line, 80);
        assert_eq!(rendered.lines.len(), line.len().div_ceil(80));
        let spaced = "ab ".repeat(700_000);
        assert!(render(&spaced, 1).lines.len() <= spaced.len());
    }

    #[test]
    fn a_long_line_full_of_addresses_is_laid_out_once_through() {
        // A hundred thousand addresses on one line, wrapped to as many rows.
        let line = "https://example.com/ ".repeat(100_000);
        let started = std::time::Instant::now();
        let rendered = render(&line, 20);
        assert!(started.elapsed().as_secs() < 20);
        assert_eq!(rendered.links, ["https://example.com/"]);
        assert_eq!(rendered.lines.len(), 100_000);
        assert!(
            rendered
                .lines
                .iter()
                .all(|row| row.spans.len() == 1 && row.spans[0].link == Some(0))
        );
    }

    #[test]
    fn asides_wrap_as_plain_strings() {
        assert_eq!(
            wrap_text("goes to evil.example, not www.mybank.example", 24),
            ["goes to evil.example,", "not www.mybank.example"]
        );
        assert_eq!(wrap_text("", 0), [""]);
    }

    fn whole(rendered: &Rendered) -> String {
        html::joined(
            rendered
                .lines
                .iter()
                .map(|line| (line, line.text().into(), true, true)),
        )
    }

    #[test]
    fn rows_a_line_was_wrapped_to_say_so_and_go_back_together() {
        let source = "The quarterly figures are attached and the margin target holds.\n\
                      Short line.\n\
                      > A quoted line that is long enough to be wrapped over two rows.\n\
                      see abcdefghijklmnopqrstuvwxyz now";
        let rendered = render(source, 24);
        assert!(rendered.lines.len() > 8);
        // Exactly what was written: the long word whole, the quote under
        // one mark, and the lines that were ended still ended.
        assert_eq!(whole(&rendered), source);
        let wrapped: Vec<Wrapped> = rendered.lines.iter().map(|line| line.wrapped).collect();
        assert!(wrapped.contains(&Wrapped::AtSpace));
        assert!(wrapped.contains(&Wrapped::InWord));
        assert_eq!(wrapped.last(), Some(&Wrapped::No));
    }
}
