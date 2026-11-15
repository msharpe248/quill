//! Terminal hyperlinks (OSC 8): the escape sequences that make a run of
//! text open an address when clicked.
//!
//! A hyperlink is a property of the cells written between the opening and
//! closing sequence. Only an address Quill opens is ever written into one;
//! such an address is printable ASCII, so it cannot end the sequence early
//! or carry another one inside it.

/// Ends the hyperlink. Text written after it is not part of one.
pub const CLOSE: &str = "\x1b]8;;\x1b\\";

/// Starts a hyperlink to `url`, or `None` when `url` is not an address
/// Quill opens.
pub fn open(url: &str) -> Option<String> {
    quill_mime::links::is_openable(url).then(|| format!("\x1b]8;;{url}\x1b\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_openable_address_becomes_a_hyperlink() {
        assert_eq!(
            open("https://example.com/a?b=1;c=2").as_deref(),
            Some("\x1b]8;;https://example.com/a?b=1;c=2\x1b\\")
        );
        assert_eq!(CLOSE, "\x1b]8;;\x1b\\");
    }

    #[test]
    fn nothing_else_is_written_into_an_escape_sequence() {
        for url in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://example.com/\x1b\\\x1b]0;owned\x07",
            "https://example.com/\x07",
            "https://example.com/a b",
        ] {
            assert_eq!(open(url), None, "{url:?}");
        }
    }
}
