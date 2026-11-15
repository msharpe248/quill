//! Text made safe to draw in a terminal.
//!
//! Mail is untrusted terminal input. A subject, a sender's name or a body
//! can carry escape sequences that retitle the window, write to the
//! clipboard, clear the screen or dress a link up as another one, and a file
//! name can carry direction overrides that make `fdp.exe` read as `exe.pdf`.
//! Everything this crate returns as text has been through one of the two
//! functions here first.

use std::borrow::Cow;

/// How long a string sequence (a window title, a clipboard write) may be,
/// in bytes, and still be removed whole. Of one that runs longer, or never
/// ends, only what starts it goes, and the rest is left as visible text.
const MAX_STRING_SEQUENCE: usize = 4096;

/// Which text is being cleaned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Header,
    Body,
}

/// A header's value, a name, an address or a file name: one line with no
/// control characters at all, and none of the characters that reorder the
/// text around them. A tab or a line break becomes a space.
pub fn header(text: &str) -> Cow<'_, str> {
    strip(text, Kind::Header)
}

/// A body: line feeds and tabs are kept, a carriage return ends a line, and
/// every other control character and escape sequence is removed.
pub fn body(text: &str) -> Cow<'_, str> {
    strip(text, Kind::Body)
}

/// HTML source: as [`body`], and a control character spelt as a numeric
/// reference (`&#27;`) is removed with the sequence it starts, exactly as
/// if it had been written raw. Without this an escape sequence would reach
/// the parser in pieces and come out the other side as visible litter.
pub fn markup(html: &str) -> Cow<'_, str> {
    let Some(first) = html.find("&#") else {
        return body(html);
    };
    let mut raw = String::new();
    let mut copied = 0;
    let mut at = first;
    while let Some(found) = html[at..].find("&#") {
        let start = at + found;
        at = start + 2;
        if let Some((control, end)) = control_reference(&html[at..]) {
            raw.push_str(&html[copied..start]);
            // An escape goes in raw, to be removed with its sequence. The
            // eight-bit controls are only dropped: a browser would not
            // have taken what follows one for a sequence.
            if control.is_ascii() {
                raw.push(control);
            }
            copied = at + end;
            at = copied;
        }
    }
    if copied == 0 {
        return body(html);
    }
    raw.push_str(&html[copied..]);
    Cow::Owned(body(&raw).into_owned())
}

/// The control character a numeric reference stands for, given what
/// follows its `&#`, and how many bytes the rest of the reference takes.
/// `None` for a reference to anything else. Most of the C1 range is not
/// here: in HTML those numbers mean the punctuation of Windows-1252. The
/// five that Windows-1252 leaves empty stay control characters.
fn control_reference(after: &str) -> Option<(char, usize)> {
    let bytes = after.as_bytes();
    let (radix, from) = match bytes.first() {
        Some(b'x' | b'X') => (16, 1),
        _ => (10, 0),
    };
    let digits = bytes[from..]
        .iter()
        .take_while(|byte| char::from(**byte).is_digit(radix))
        .count();
    // Leading zeros aside, no control character needs more than three.
    let number = after[from..from + digits].trim_start_matches('0');
    if digits == 0 || number.len() > 3 {
        return None;
    }
    let code = u32::from_str_radix(number, radix).ok()?;
    let control = char::from_u32(code).filter(|c| is_control(*c) && *c != '\t' && *c != '\n')?;
    let unmapped = matches!(code, 0x81 | 0x8d | 0x8f | 0x90 | 0x9d);
    if control == '\0' || !(control.is_ascii() || unmapped) {
        return None;
    }
    let end = from + digits;
    Some((control, end + usize::from(bytes.get(end) == Some(&b';'))))
}

/// Whether `c` is a control character: C0, DEL or C1.
pub(crate) fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// The characters that override or isolate the direction of the text around
/// them. They are how `invoice<override>fdp.exe` is made to read
/// `invoiceexe.pdf`.
pub(crate) fn is_reordering(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// `text` with every control character dropped, one at a time. Unlike
/// [`body`] it never removes a character that is not one, which is what
/// serialized HTML needs: taking a whole sequence out of markup could take a
/// quote or a bracket with it.
pub(crate) fn drop_controls(text: &str) -> Cow<'_, str> {
    let unwanted = |c: char| is_control(c) && c != '\n' && c != '\t';
    if !text.contains(unwanted) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.chars().filter(|c| !unwanted(*c)).collect())
}

fn strip(text: &str, kind: Kind) -> Cow<'_, str> {
    let unwanted = |c: char| match kind {
        Kind::Header => is_control(c) || is_reordering(c),
        Kind::Body => is_control(c) && c != '\n' && c != '\t',
    };
    let Some(first) = text.find(unwanted) else {
        return Cow::Borrowed(text);
    };

    let mut clean = String::with_capacity(text.len());
    clean.push_str(&text[..first]);
    let mut strings = Strings { text, known: None };
    let mut rest = &text[first..];
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        rest = match c {
            '\u{1b}' => after_escape(after, &mut strings),
            // The C1 forms of the same introducers.
            '\u{9b}' => after_control_sequence(after),
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => strings.after(after),
            '\r' if kind == Kind::Body => {
                // A carriage return alone would send the cursor back to
                // overwrite the line. It ends the line instead.
                if !after.starts_with('\n') {
                    clean.push('\n');
                }
                after
            }
            '\t' | '\n' | '\r' if kind == Kind::Header => {
                clean.push(' ');
                after
            }
            c if unwanted(c) => after,
            c => {
                clean.push(c);
                after
            }
        };
    }
    Cow::Owned(clean)
}

/// What follows an escape character, past the sequence it introduces.
fn after_escape<'a>(after: &'a str, strings: &mut Strings<'a>) -> &'a str {
    match after.chars().next() {
        Some('[') => after_control_sequence(&after[1..]),
        // Operating system command, device control, start of string,
        // privacy message, application command: each carries a string.
        Some(']' | 'P' | 'X' | '^' | '_') => strings.after(&after[1..]),
        _ => {
            // Any other sequence is some intermediates and one final
            // character, such as `ESC c` to reset or `ESC ( B`.
            let intermediates = after
                .bytes()
                .take_while(|byte| (0x20..=0x2f).contains(byte))
                .count();
            match after.as_bytes().get(intermediates) {
                Some(byte) if (0x30..=0x7e).contains(byte) => &after[intermediates + 1..],
                // Not a sequence a terminal would finish. Only the escape
                // goes, and whatever follows is judged as text.
                _ => after,
            }
        }
    }
}

/// What follows `CSI`, past its parameters and its final character.
fn after_control_sequence(after: &str) -> &str {
    let bytes = after.as_bytes();
    let parameters = bytes
        .iter()
        .take_while(|byte| (0x30..=0x3f).contains(*byte))
        .count();
    let intermediates = bytes[parameters..]
        .iter()
        .take_while(|byte| (0x20..=0x2f).contains(*byte))
        .count();
    let end = parameters + intermediates;
    match bytes.get(end) {
        Some(byte) if (0x40..=0x7e).contains(byte) => &after[end + 1..],
        _ => &after[end..],
    }
}

/// Where string sequences end. Text that starts a thousand of them and
/// ends none must not be searched a thousand times, so the last answer is
/// kept: where the search began, and the terminator it found, if any.
struct Strings<'a> {
    text: &'a str,
    known: Option<(usize, Option<(usize, usize)>)>,
}

impl<'a> Strings<'a> {
    /// What follows the start of a string sequence, past its terminator.
    /// `after` is the rest of the text from where the string begins. A
    /// string that does not end within reach is not a sequence, and
    /// nothing more is removed.
    fn after(&mut self, after: &'a str) -> &'a str {
        let from = self.text.len() - after.len();
        let found = match self.known {
            // Still good: nothing ends between there and here.
            Some((searched, found))
                if searched <= from && found.is_none_or(|(start, _)| start >= from) =>
            {
                found
            }
            _ => {
                let found = terminator(self.text, from);
                self.known = Some((from, found));
                found
            }
        };
        match found {
            Some((start, end)) if start - from <= MAX_STRING_SEQUENCE => &self.text[end..],
            _ => after,
        }
    }
}

/// The first thing at or after `from` that ends a string sequence: BEL, or
/// the string terminator in either of its forms. Where it starts and ends.
fn terminator(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    (from..bytes.len()).find_map(|at| match bytes[at] {
        0x07 => Some((at, at + 1)),
        0x1b if bytes.get(at + 1) == Some(&b'\\') => Some((at, at + 2)),
        // U+009C as UTF-8 writes it.
        0xc2 if bytes.get(at + 1) == Some(&0x9c) => Some((at, at + 2)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promise the rest of the crate makes about a header.
    fn is_clean_header(text: &str) -> bool {
        !text.contains(|c: char| is_control(c) || is_reordering(c))
    }

    fn is_clean_body(text: &str) -> bool {
        !text.contains(|c: char| is_control(c) && c != '\n' && c != '\t')
    }

    #[test]
    fn ordinary_text_is_not_copied() {
        for text in ["", "Q4 roadmap", "Tomás ✎ 陈静", "line\n\tindented\n"] {
            assert!(matches!(body(text), Cow::Borrowed(_)), "{text:?}");
        }
        assert!(matches!(header("Tomás ✎ 陈静"), Cow::Borrowed(_)));
    }

    #[test]
    fn escape_sequences_are_removed_whole() {
        // A window title, a clipboard write, a hyperlink, clear screen,
        // colour, cursor movement, and a terminal reset.
        for (dirty, clean) in [
            ("\u{1b}]0;pwned\u{7}Free money", "Free money"),
            ("\u{1b}]2;pwned\u{1b}\\Free money", "Free money"),
            ("Copy \u{1b}]52;c;cm0gLXJmIH4=\u{7}this", "Copy this"),
            (
                "\u{1b}]8;;https://evil.example/\u{1b}\\Your bank\u{1b}]8;;\u{1b}\\",
                "Your bank",
            ),
            ("Click \u{1b}[2J\u{1b}[Hhere", "Click here"),
            ("\u{1b}[1;31mURGENT\u{1b}[0m", "URGENT"),
            ("up\u{1b}[2A\u{1b}[?25l", "up"),
            ("reset\u{1b}c now", "reset now"),
            ("charset\u{1b}(0 lines", "charset lines"),
            ("\u{1b}P1$r0m\u{1b}\\after", "after"),
        ] {
            assert_eq!(body(dirty), clean, "{dirty:?}");
            assert_eq!(header(dirty), clean, "{dirty:?}");
        }
    }

    #[test]
    fn the_eight_bit_forms_are_removed_too() {
        for (dirty, clean) in [
            ("\u{9b}2JCleared", "Cleared"),
            ("\u{9d}0;title\u{9c}Titled", "Titled"),
            ("\u{9d}0;title\u{7}Titled", "Titled"),
            ("\u{90}payload\u{9c}Device", "Device"),
            ("a\u{85}b\u{80}c\u{9f}", "abc"),
        ] {
            assert_eq!(body(dirty), clean, "{dirty:?}");
            assert_eq!(header(dirty), clean, "{dirty:?}");
        }
    }

    #[test]
    fn a_sequence_that_never_ends_does_not_take_the_text_with_it() {
        let dirty = format!("\u{1b}]0;{}", "the rest of a long message ".repeat(400));
        let cleaned = body(&dirty);
        assert!(cleaned.contains("the rest of a long message"));
        assert!(is_clean_body(&cleaned));
        // An escape with nothing after it, and one before something that
        // is not a sequence.
        assert_eq!(body("end\u{1b}"), "end");
        assert_eq!(body("\u{1b}\u{e9}t\u{e9}"), "\u{e9}t\u{e9}");
        assert_eq!(body("\u{1b}[12;"), "");
        assert_eq!(body("\u{1b}[\u{1b}[2Jx"), "x");
    }

    #[test]
    fn a_thousand_sequences_that_never_end_cost_no_more_than_one() {
        // Each start used to look four thousand characters ahead.
        let starts = "\u{1b}]".repeat(500_000);
        let started = std::time::Instant::now();
        assert_eq!(body(&starts), "");
        let mixed = "\u{1b}]0;x".repeat(200_000) + "\u{7}tail";
        assert!(body(&mixed).ends_with("tail"));
        assert!(header(&"\u{9d}\u{1b}P\u{1b}_".repeat(100_000)).is_empty());
        assert!(started.elapsed().as_secs() < 5);
        // One ended far away is not a sequence; one ended nearby is.
        let far = format!("\u{1b}]0;{}\u{7}end", "x".repeat(MAX_STRING_SEQUENCE));
        assert_eq!(
            body(&far).len(),
            "0;".len() + MAX_STRING_SEQUENCE + "end".len()
        );
        let near = format!("a\u{1b}]0;{}\u{7}b\u{1b}]0;t\u{1b}\\c", "x".repeat(100));
        assert_eq!(body(&near), "abc");
    }

    #[test]
    fn a_body_keeps_its_lines_and_tabs_and_nothing_else() {
        assert_eq!(body("one\r\ntwo\rthree\n"), "one\ntwo\nthree\n");
        assert_eq!(body("a\tb\u{0}c\u{8}d\u{7f}e\u{b}f\u{c}g"), "a\tbcdefg");
        // Overwriting a line with a carriage return shows both.
        assert_eq!(body("Pay Ana 100\rPay Eve 900"), "Pay Ana 100\nPay Eve 900");
    }

    #[test]
    fn a_header_is_one_line_with_no_controls() {
        assert_eq!(header("Re:\tthe\r\n plan\n"), "Re: the   plan ");
        assert_eq!(header("bell\u{7}\u{0}\u{7f}"), "bell");
    }

    #[test]
    fn direction_overrides_are_taken_out_of_headers() {
        // Drawn with the override, this reads "invoiceexe.pdf".
        assert_eq!(header("invoice\u{202e}fdp.exe"), "invoicefdp.exe");
        assert_eq!(
            header("\u{202a}a\u{202b}b\u{202c}c\u{202d}d\u{2066}e\u{2067}f\u{2068}g\u{2069}"),
            "abcdefg"
        );
        // Marks that do not override are part of writing right to left.
        assert_eq!(
            header("\u{200f}\u{5e9}\u{5dc}\u{5d5}\u{5dd}"),
            "\u{200f}\u{5e9}\u{5dc}\u{5d5}\u{5dd}"
        );
        // A body keeps them: they cannot disguise anything there that the
        // reader does not show for what it is.
        assert_eq!(body("a\u{202e}b"), "a\u{202e}b");
    }

    #[test]
    fn controls_spelt_as_references_are_removed_with_their_sequences() {
        for (dirty, clean) in [
            ("<p>&#27;]0;pwned&#7;Free money</p>", "<p>Free money</p>"),
            ("<p>&#x1b;[2JCleared&#X1B;[0m</p>", "<p>Cleared</p>"),
            ("<p>&#0027;[31mred&#x00001B;[0m</p>", "<p>red</p>"),
            ("<p>&#27[1mno semicolon</p>", "<p>no semicolon</p>"),
            ("\u{1b}[1mraw&#27;]52;c;cm0=\u{7}and spelt", "rawand spelt"),
            ("a&#8;b&#127;c&#x7f;d&#1;e", "abcde"),
            ("one&#13;two", "one\ntwo"),
            ("a&#x81;b&#141;c&#x8F;d", "abcd"),
            ("a&#x9d;0;title&#7;b&#x90;x\u{9c}c", "a0;titlebxc"),
            ("one&#13;&#10;two", "one\n&#10;two"),
        ] {
            assert_eq!(markup(dirty), clean, "{dirty:?}");
        }
    }

    #[test]
    fn other_references_are_left_for_the_parser() {
        for html in [
            "&#160;&#8203;&#x202e;&#128512;",
            "&#9;&#10;&#x9;&#xA;",
            // Not controls in HTML: the parser reads these as Windows-1252
            // punctuation and as the replacement character.
            "&#x9b;&#133;&#0;&#128;&#x9f;",
            "&amp;&#;&#x;&#xzz;&# 27;",
            "&#99999999999999999999;&#x1b0;&#270;",
            "&#",
            "tail &#2",
        ] {
            if html == "tail &#2" {
                assert_eq!(markup(html), "tail ");
            } else {
                assert!(matches!(markup(html), Cow::Borrowed(_)), "{html:?}");
            }
        }
    }

    #[test]
    fn dropping_controls_one_at_a_time_keeps_everything_else() {
        assert_eq!(
            drop_controls("<a title=\"\u{1b}]0;\">x\u{7}</a>\n"),
            "<a title=\"]0;\">x</a>\n"
        );
        assert!(matches!(drop_controls("plain\n\ttext"), Cow::Borrowed(_)));
    }

    #[test]
    fn every_string_comes_out_clean() {
        // Each control character, alone and inside a sequence, between
        // ordinary characters and at either end.
        let mut seeds: Vec<String> = Vec::new();
        for code in (0u32..=0x9f).chain(0x2028..=0x202f).chain(0x2066..=0x2069) {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            seeds.push(format!("{c}"));
            seeds.push(format!("a{c}b"));
            seeds.push(format!("\u{1b}{c}tail"));
            seeds.push(format!("\u{1b}[{c}tail"));
            seeds.push(format!("\u{1b}]{c}tail\u{7}"));
            seeds.push(format!("\u{9b}{c}"));
            seeds.push(format!("{c}\u{1b}"));
        }
        for seed in seeds {
            assert!(is_clean_header(&header(&seed)), "{seed:?}");
            assert!(is_clean_body(&body(&seed)), "{seed:?}");
            assert!(is_clean_body(&drop_controls(&seed)), "{seed:?}");
        }
    }
}
