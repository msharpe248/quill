//! Which links Quill will open.

/// Whether Quill will hand `url` to the system to open. Only web pages and
/// mail addresses: a message must not be able to open a file, run a script
/// or start another program by naming some other scheme. The address must
/// also be plain printable ASCII, which is what a real one is once encoded;
/// anything else could hide where it leads or smuggle control characters
/// into the terminal.
///
/// The core checks this before asking, and the host checks it again before
/// opening.
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://", "mailto:"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme));
    rest.is_some_and(|rest| !rest.is_empty()) && url.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_pages_and_mail_addresses_are_openable() {
        assert!(is_openable("https://example.com/a?b=c&d=%20e#f"));
        assert!(is_openable("HTTP://EXAMPLE.COM"));
        assert!(is_openable("mailto:robin@example.com?subject=Hi"));
    }

    #[test]
    fn nothing_else_is() {
        for url in [
            "",
            "https://",
            "mailto:",
            "example.com",
            "/relative/path",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "x-apple-something://do",
            "tel:+15555550100",
            "ftp://example.com/file",
            "https://example.com/a b",
            "https://example.com/\u{1b}]0;x\u{7}",
            "https://ex\u{430}mple.com/",
            " https://example.com/",
        ] {
            assert!(!is_openable(url), "{url:?}");
        }
    }
}
