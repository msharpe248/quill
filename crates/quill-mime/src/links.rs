//! Links: finding them in plain text. Which ones Quill will open is
//! `quill_core::links::is_openable`, offered here too.

pub use quill_core::links::is_openable;

/// The web addresses written out in plain text, each once, in the order
/// they appear.
pub fn find(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let is_boundary =
        |c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '|' | '\\');
    for token in text.split(is_boundary) {
        let lower = token.to_ascii_lowercase();
        let Some(start) = lower.find("https://").or_else(|| lower.find("http://")) else {
            continue;
        };
        let mut url = &token[start..];
        // Punctuation that ends the sentence, not the address, and a closing
        // bracket that has no opening one inside the address.
        while let Some(last) = url.chars().next_back() {
            let unbalanced = |close: char, open: char| {
                last == close && url.matches(close).count() > url.matches(open).count()
            };
            let trailing = matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_')
                || unbalanced(')', '(')
                || unbalanced(']', '[')
                || unbalanced('}', '{');
            if !trailing {
                break;
            }
            url = &url[..url.len() - last.len_utf8()];
        }
        if is_openable(url) && !found.iter().any(|known| known == url) {
            found.push(url.to_owned());
        }
    }
    found
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
}
