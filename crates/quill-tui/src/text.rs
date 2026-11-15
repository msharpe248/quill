//! Text helpers: sanitising untrusted mail text, width-aware fitting and
//! wrapping, and date formatting.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Makes one line of untrusted text safe to draw: escape sequences and control
/// characters are removed, and line breaks and tabs become spaces.
pub fn sanitize_line(input: &str) -> String {
    strip(input, false)
}

/// Makes a body of untrusted text safe to draw. Line feeds survive, tabs
/// become four spaces, and every other control character and escape sequence
/// is removed.
pub fn sanitize_body(input: &str) -> String {
    strip(input, true)
}

/// Mail is untrusted terminal input. This drops C0 and C1 controls and whole
/// escape sequences (CSI, OSC and the other string sequences), so nothing a
/// sender writes can move the cursor, recolour the screen or set the title.
fn strip(input: &str, multiline: bool) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' if multiline => out.push('\n'),
            '\n' => out.push(' '),
            '\t' if multiline => out.push_str("    "),
            '\t' => out.push(' '),
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                // OSC, DCS, SOS, PM, APC: strings that run to a terminator.
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    chars.next();
                    skip_string(&mut chars);
                }
                // A two-character escape such as ESC c (reset).
                Some(next) if !next.is_control() => {
                    chars.next();
                }
                _ => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Consumes a CSI sequence's parameters up to and including its final byte.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            break;
        }
    }
}

/// Consumes a string sequence up to BEL, ST, or the end of the line. Stopping
/// at a line end keeps an unterminated sequence from eating the whole body.
fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.peek().copied() {
        match c {
            '\n' => break,
            '\u{07}' | '\u{9c}' => {
                chars.next();
                break;
            }
            '\u{1b}' => {
                chars.next();
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                break;
            }
            _ => {
                chars.next();
            }
        }
    }
}

/// The number of terminal cells `text` occupies.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Cuts `text` to at most `max` cells, ending in an ellipsis when cut. Never
/// splits a grapheme cluster.
pub fn truncate(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let w = width(grapheme);
        if used + w > max - 1 {
            break;
        }
        out.push_str(grapheme);
        used += w;
    }
    out.push('…');
    out
}

/// The longest start of `text` that fits in `max` cells, with no ellipsis:
/// what is left of text drawn into a space too small for it. Never splits a
/// grapheme cluster.
pub fn clip(text: &str, max: usize) -> &str {
    let mut used = 0;
    for (start, grapheme) in text.grapheme_indices(true) {
        let w = width(grapheme);
        if used + w > max {
            return &text[..start];
        }
        used += w;
    }
    text
}

/// Truncates then pads with spaces on the right to exactly `cells` cells.
pub fn fit(text: &str, cells: usize) -> String {
    let mut out = truncate(text, cells);
    let used = width(&out);
    out.extend(std::iter::repeat_n(' ', cells.saturating_sub(used)));
    out
}

/// Truncates then pads with spaces on the left to exactly `cells` cells.
pub fn fit_right(text: &str, cells: usize) -> String {
    let cut = truncate(text, cells);
    let pad = cells.saturating_sub(width(&cut));
    let mut out = String::with_capacity(cut.len() + pad);
    out.extend(std::iter::repeat_n(' ', pad));
    out.push_str(&cut);
    out
}

/// Soft-wraps `text` to `max` cells per line. Breaks at spaces, and inside a
/// word only when the word alone is wider than a line. Blank lines survive.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        // Something, possibly an empty word, has been placed on this line.
        let mut started = false;
        // This line continues a paragraph that has already wrapped.
        let mut wrapped = false;
        // Splitting on single spaces keeps runs of spaces, so indentation and
        // column-aligned text survive when they fit.
        for word in paragraph.trim_end_matches(' ').split(' ') {
            if word.is_empty() && wrapped && !started {
                continue;
            }
            let word_width = width(word);
            let gap = usize::from(started);
            if used + gap + word_width <= max {
                if started {
                    line.push(' ');
                }
                line.push_str(word);
                used += gap + word_width;
                started = true;
                continue;
            }
            if started {
                lines.push(std::mem::take(&mut line));
                used = 0;
                started = false;
                wrapped = true;
            }
            if word.is_empty() {
                continue;
            }
            for grapheme in word.graphemes(true) {
                let w = width(grapheme);
                if used + w > max && used > 0 {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                    wrapped = true;
                }
                line.push_str(grapheme);
                used += w;
            }
            started = true;
        }
        lines.push(line);
    }
    lines
}

/// A compact date for a list column: the time today, the weekday within the
/// last week, the day and month this year, otherwise the month and year.
/// A file's size in the unit that keeps the number short: `937 bytes`,
/// `214 KB`, `3.2 MB`.
pub fn file_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else if bytes == 1 {
        "1 byte".to_owned()
    } else {
        format!("{bytes} bytes")
    }
}

pub fn short_date(ts: i64, now: i64, tz: &TimeZone) -> String {
    let (Ok(then), Ok(today)) = (Timestamp::from_second(ts), Timestamp::from_second(now)) else {
        return String::new();
    };
    let then = then.to_zoned(tz.clone());
    let today = today.to_zoned(tz.clone());
    let days_ago = (today.date() - then.date()).get_days();
    let pattern = if days_ago == 0 {
        "%H:%M"
    } else if (1..7).contains(&days_ago) {
        "%a"
    } else if then.year() == today.year() {
        "%b %d"
    } else {
        "%b %Y"
    };
    then.strftime(pattern).to_string()
}

/// A full date for the reader's header.
pub fn long_date(ts: i64, tz: &TimeZone) -> String {
    match Timestamp::from_second(ts) {
        Ok(then) => then
            .to_zoned(tz.clone())
            .strftime("%a %d %b %Y %H:%M")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// The local calendar day of a timestamp, as days since the Unix epoch. Used
/// to notice midnight passing.
pub fn local_day(ts: i64, tz: &TimeZone) -> i64 {
    let Ok(then) = Timestamp::from_second(ts) else {
        return 0;
    };
    let date = then.to_zoned(tz.clone()).date();
    i64::from((date - jiff::civil::date(1970, 1, 1)).get_days())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_and_cursor_sequences_are_removed_whole() {
        assert_eq!(sanitize_line("\u{1b}[31mred\u{1b}[0m alert"), "red alert");
        assert_eq!(sanitize_line("a\u{1b}[2J\u{1b}[Hb"), "ab");
        assert_eq!(sanitize_line("a\u{9b}31mb"), "ab");
    }

    #[test]
    fn title_and_clipboard_sequences_are_removed_whole() {
        assert_eq!(sanitize_line("\u{1b}]0;pwned\u{07}Subject"), "Subject");
        assert_eq!(sanitize_line("\u{1b}]52;c;aGk=\u{1b}\\Subject"), "Subject");
        assert_eq!(sanitize_line("\u{1b}Pq#0;1\u{1b}\\ok"), "ok");
        assert_eq!(sanitize_line("\u{9d}0;pwned\u{9c}ok"), "ok");
    }

    #[test]
    fn an_unterminated_string_sequence_stops_at_the_line_end() {
        assert_eq!(sanitize_body("one\u{1b}]0;never closed\ntwo"), "one\ntwo");
    }

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(sanitize_line("a\u{07}b\u{08}c\u{7f}d\u{85}e\r"), "abcde");
        assert_eq!(sanitize_line("lone\u{1b}"), "lone");
        assert_eq!(sanitize_line("reset\u{1b}c!"), "reset!");
    }

    #[test]
    fn line_breaks_and_tabs_depend_on_the_mode() {
        assert_eq!(sanitize_line("a\nb\tc"), "a b c");
        assert_eq!(sanitize_body("a\r\nb\tc"), "a\nb    c");
    }

    #[test]
    fn ordinary_unicode_passes_through() {
        let text = "Tomás 陈静 🎉 — “quoted”";
        assert_eq!(sanitize_line(text), text);
    }

    #[test]
    fn truncate_counts_cells_not_bytes() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello!", 5), "hell…");
        // Each CJK character is two cells wide.
        assert_eq!(truncate("陈静陈静", 5), "陈静…");
        assert_eq!(truncate("陈静陈静", 4), "陈…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
    }

    #[test]
    fn truncate_keeps_grapheme_clusters_whole() {
        // "e" + combining acute is one cluster, one cell.
        let text = "e\u{301}e\u{301}e\u{301}";
        assert_eq!(truncate(text, 2), "e\u{301}…");
    }

    #[test]
    fn fit_pads_to_the_exact_width() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(width(&fit("陈静陈", 4)), 4);
        assert_eq!(fit_right("ab", 4), "  ab");
        assert_eq!(fit_right("abcdef", 4), "abc…");
    }

    #[test]
    fn wrap_breaks_at_spaces() {
        assert_eq!(wrap("the quick brown fox", 10), ["the quick", "brown fox"]);
        assert_eq!(wrap("exact fit", 9), ["exact fit"]);
    }

    #[test]
    fn wrap_keeps_blank_lines_and_paragraphs() {
        assert_eq!(wrap("one\n\ntwo", 10), ["one", "", "two"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn wrap_splits_a_word_wider_than_the_line() {
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(
            wrap("see abcdefghij now", 4),
            ["see", "abcd", "efgh", "ij", "now"]
        );
        for line in wrap("陈静陈静陈静", 5) {
            assert!(width(&line) <= 5, "{line}");
        }
    }

    #[test]
    fn wrap_keeps_indentation_and_aligned_columns_that_fit() {
        assert_eq!(wrap("    indented", 20), ["    indented"]);
        assert_eq!(wrap("fmt      ok", 20), ["fmt      ok"]);
        assert_eq!(wrap("trailing   ", 20), ["trailing"]);
    }

    #[test]
    fn wrap_does_not_invent_blank_lines_from_spaces_at_a_break() {
        assert_eq!(wrap("abcd  efgh", 4), ["abcd", "efgh"]);
        assert_eq!(wrap("abcd ", 4), ["abcd"]);
    }

    #[test]
    fn wrap_never_exceeds_the_width() {
        let text =
            "Legal accepted everything except clause 7.2. Their counter-proposal is attached.";
        for max in 1..40 {
            for line in wrap(text, max) {
                assert!(width(&line) <= max, "{max}: {line:?}");
            }
        }
    }

    const NOW: i64 = 1_794_648_600; // Saturday 2026-11-14T09:30:00Z

    #[test]
    fn short_dates_get_coarser_with_age() {
        let utc = TimeZone::UTC;
        assert_eq!(short_date(NOW - 16 * 60, NOW, &utc), "09:14");
        assert_eq!(short_date(NOW - 26 * 3600, NOW, &utc), "Fri");
        assert_eq!(short_date(NOW - 6 * 86400, NOW, &utc), "Sun");
        assert_eq!(short_date(NOW - 7 * 86400, NOW, &utc), "Nov 07");
        assert_eq!(short_date(NOW - 400 * 86400, NOW, &utc), "Oct 2025");
    }

    #[test]
    fn short_dates_use_the_local_calendar_day() {
        // 23:50 UTC on Friday is already Saturday in UTC+1, so it is "today".
        let ts = NOW - 9 * 3600 - 40 * 60;
        assert_eq!(short_date(ts, NOW, &TimeZone::UTC), "Fri");
        let plus_one = TimeZone::fixed(jiff::tz::offset(1));
        assert_eq!(short_date(ts, NOW, &plus_one), "00:50");
    }

    #[test]
    fn long_date_is_complete() {
        assert_eq!(
            long_date(NOW - 16 * 60, &TimeZone::UTC),
            "Sat 14 Nov 2026 09:14"
        );
    }

    #[test]
    fn local_day_changes_at_local_midnight() {
        let utc = TimeZone::UTC;
        let midnight = NOW - 9 * 3600 - 30 * 60;
        assert_eq!(local_day(midnight, &utc), local_day(NOW, &utc));
        assert_eq!(local_day(midnight - 1, &utc) + 1, local_day(NOW, &utc));
    }
}
