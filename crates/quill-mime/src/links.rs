//! Links: finding them in plain text. Which ones Quill will open is
//! `quill_core::links::is_openable`, offered here too.

use std::ops::Range;

pub use quill_core::links::is_openable;

/// No address is longer than this. A longer run of characters after
/// `https://` is padding, not somewhere to go.
const MAX_ADDRESS_BYTES: usize = 8192;

/// The web addresses written out in plain text, each once, in the order
/// they appear.
pub fn find(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for place in locate_schemes(text, &["https://", "http://"]) {
        let url = &text[place];
        if !found.iter().any(|known| known == url) {
            found.push(url.to_owned());
        }
    }
    found
}

/// Where the addresses written out in `text` are: every web address and
/// every `mailto:` address that [`is_openable`] accepts, in order. Each
/// range covers the address exactly as written.
pub fn locate(text: &str) -> Vec<Range<usize>> {
    locate_schemes(text, &["https://", "http://", "mailto:"])
}

fn locate_schemes(text: &str, schemes: &[&str]) -> Vec<Range<usize>> {
    let is_boundary =
        |c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '|' | '\\');
    let mut places = Vec::new();
    let mut start = 0;
    let mut token = |range: Range<usize>| {
        if let Some(found) = address_in(&text[range.clone()], schemes) {
            places.push(range.start + found.start..range.start + found.end);
        }
    };
    for (at, c) in text.char_indices() {
        if is_boundary(c) {
            token(start..at);
            start = at + c.len_utf8();
        }
    }
    token(start..text.len());
    places
}

/// The address inside one run of characters with no space in it, if it
/// holds one.
fn address_in(token: &str, schemes: &[&str]) -> Option<Range<usize>> {
    // Nothing shorter than a scheme and a character holds an address.
    if token.len() < "http://".len() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    let start = schemes
        .iter()
        .filter_map(|scheme| lower.find(scheme))
        .min()?;
    let url = &token[start..];
    if url.len() > MAX_ADDRESS_BYTES {
        return None;
    }

    // Punctuation that ends the sentence, not the address, and a closing
    // bracket that has no opening one inside the address. The brackets are
    // counted once and kept up to date, so a long tail of them costs no
    // more than its length.
    let count = |wanted: char| url.chars().filter(|c| *c == wanted).count();
    let mut surplus = [
        (')', count(')').saturating_sub(count('('))),
        (']', count(']').saturating_sub(count('['))),
        ('}', count('}').saturating_sub(count('{'))),
    ];
    let mut end = url.len();
    while let Some(last) = url[..end].chars().next_back() {
        let unbalanced = surplus
            .iter_mut()
            .find(|(close, left)| *close == last && *left > 0);
        let trailing = match unbalanced {
            Some((_, left)) => {
                *left -= 1;
                true
            }
            None => matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_'),
        };
        if !trailing {
            break;
        }
        end -= last.len_utf8();
    }
    let url = &url[..end];
    let is_address = if lower[start..].starts_with("mailto:") {
        // "mailto:" in running text is only an address with someone in it.
        url.contains('@')
    } else {
        true
    };
    (is_address && is_openable(url)).then_some(start..start + end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_found_in_plain_text() {
        let text = "The agenda is at https://example.com/agenda. Rooms \
                    (see https://example.com/rooms_(east)) are booked; the map is \
                    <http://example.com/map>, and again: https://example.com/agenda\n\
                    Not links: example.com, ftp://example.com/x, mailto:robin@example.com";
        assert_eq!(
            find(text),
            [
                "https://example.com/agenda",
                "https://example.com/rooms_(east)",
                "http://example.com/map",
            ]
        );
        assert!(find("no links here").is_empty());
    }

    fn located(text: &str) -> Vec<&str> {
        locate(text).into_iter().map(|place| &text[place]).collect()
    }

    #[test]
    fn addresses_are_located_where_they_are_written() {
        let text = "Write to mailto:robin@example.com, or see \
                    (https://example.com/a_(b)). Again: https://example.com/a_(b)!";
        assert_eq!(
            located(text),
            [
                "mailto:robin@example.com",
                "https://example.com/a_(b)",
                "https://example.com/a_(b)",
            ]
        );
        assert_eq!(
            located("HTTPS://EXAMPLE.COM/Agenda"),
            ["HTTPS://EXAMPLE.COM/Agenda"]
        );
        assert_eq!(located("x=https://example.com/"), ["https://example.com/"]);
    }

    #[test]
    fn what_cannot_be_opened_is_not_located() {
        for text in [
            "",
            "https://",
            "http://.",
            "mailto:",
            "mailto:nobody",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/x",
            "example.com",
            // An address is plain ASCII once written out; anything else
            // could hide where it leads.
            "https://ex\u{430}mple.com/",
        ] {
            assert!(located(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_long_tail_of_brackets_is_quick() {
        // Each bracket used to cost a count of all the others.
        let text = format!("https://example.com/{}", ")".repeat(8000));
        let started = std::time::Instant::now();
        assert_eq!(located(&text), ["https://example.com/"]);
        assert_eq!(find(&text), ["https://example.com/"]);
        assert!(started.elapsed().as_secs() < 2);
        // And past any length an address has, there is no address.
        let endless = format!("https://example.com/{}", "a".repeat(2_000_000));
        assert!(located(&endless).is_empty());
    }
}
