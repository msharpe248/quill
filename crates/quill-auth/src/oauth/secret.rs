//! Text that must never be printed.

use std::fmt;

/// A token, a code or a client secret. It can be read on purpose with
/// [`Secret::expose`], and never by accident: printing it for debugging
/// shows that it is there and nothing more.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The secret itself, for sending to the server it belongs to.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    /// Never the secret itself: this ends up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl From<String> for Secret {
    fn from(secret: String) -> Self {
        Self(secret)
    }
}

impl From<&str> for Secret {
    fn from(secret: &str) -> Self {
        Self(secret.to_owned())
    }
}

/// A server's words made safe to show: one line, nothing a terminal would
/// act on, and not so long that it buries the screen.
pub(crate) fn words(text: &str) -> String {
    const LIMIT: usize = 300;
    let mut out = String::new();
    let mut shown = 0;
    for word in text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
            shown += 1;
        }
        for c in word.chars() {
            if shown == LIMIT {
                out.push('…');
                return out;
            }
            out.push(c);
            shown += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_does_not_print() {
        let secret = Secret::new("hunter2-refresh");
        assert_eq!(format!("{secret:?}"), "Secret(..)");
        assert_eq!(format!("{:?}", Some(&secret)), "Some(Secret(..))");
        assert_eq!(secret.expose(), "hunter2-refresh");
    }

    #[test]
    fn a_servers_words_are_made_one_safe_line() {
        assert_eq!(
            words("AADSTS70008: expired.\r\nTrace ID: 1\x1b[2J\x07 end"),
            "AADSTS70008: expired. Trace ID: 1 [2J end"
        );
        let long = words(&"x".repeat(1000));
        assert_eq!(long.chars().count(), 301);
        assert!(long.ends_with('…'));
        assert_eq!(words(""), "");
    }
}
