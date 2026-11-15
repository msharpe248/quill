//! What a server says it can do, read from its reply to `EHLO`.

use crate::reply::Reply;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub starttls: bool,
    /// The sign-in mechanisms offered, in upper case.
    pub auth: Vec<String>,
    /// The largest message taken, in bytes. `None` when the server names no
    /// fixed limit.
    pub size: Option<u64>,
    pub eight_bit_mime: bool,
    pub smtputf8: bool,
}

impl Capabilities {
    /// Reads the extensions out of an `EHLO` reply. Its first line is the
    /// server's name and a greeting; each line after it is one extension.
    pub fn from_ehlo(reply: &Reply) -> Self {
        let mut capabilities = Self::default();
        for line in reply.lines.iter().skip(1) {
            let line = line.to_ascii_uppercase();
            let mut words = line.split_ascii_whitespace();
            let Some(keyword) = words.next() else {
                continue;
            };
            match keyword {
                "STARTTLS" => capabilities.starttls = true,
                "8BITMIME" => capabilities.eight_bit_mime = true,
                "SMTPUTF8" => capabilities.smtputf8 = true,
                // "SIZE" alone, or "SIZE 0", means no fixed limit.
                "SIZE" => {
                    capabilities.size = words
                        .next()
                        .and_then(|limit| limit.parse().ok())
                        .filter(|limit| *limit > 0);
                }
                "AUTH" => capabilities.add_mechanisms(words),
                // Some old servers write "AUTH=LOGIN PLAIN", usually beside
                // the standard line.
                _ => {
                    if let Some(first) = keyword.strip_prefix("AUTH=") {
                        capabilities.add_mechanisms(std::iter::once(first).chain(words));
                    }
                }
            }
        }
        capabilities
    }

    fn add_mechanisms<'a>(&mut self, names: impl Iterator<Item = &'a str>) {
        for name in names {
            if !name.is_empty() && !self.auth.iter().any(|known| known == name) {
                self.auth.push(name.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ehlo(lines: &[&str]) -> Capabilities {
        Capabilities::from_ehlo(&Reply {
            code: 250,
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        })
    }

    #[test]
    fn extensions_are_read_from_the_lines_after_the_first() {
        let capabilities = ehlo(&[
            "mail.example.com Hello",
            "PIPELINING",
            "SIZE 52428800",
            "STARTTLS",
            "AUTH PLAIN LOGIN SCRAM-SHA-256",
            "8BITMIME",
            "SMTPUTF8",
            "ENHANCEDSTATUSCODES",
        ]);
        assert_eq!(
            capabilities,
            Capabilities {
                starttls: true,
                auth: vec![
                    "PLAIN".to_owned(),
                    "LOGIN".to_owned(),
                    "SCRAM-SHA-256".to_owned()
                ],
                size: Some(52_428_800),
                eight_bit_mime: true,
                smtputf8: true,
            }
        );
    }

    #[test]
    fn the_greeting_line_is_not_an_extension() {
        // A server whose name happens to be a keyword offers nothing by it.
        assert_eq!(ehlo(&["STARTTLS"]), Capabilities::default());
        assert_eq!(ehlo(&[]), Capabilities::default());
    }

    #[test]
    fn keywords_are_matched_in_any_case() {
        let capabilities = ehlo(&["host", "starttls", "Auth plain", "size 10", "8bitmime"]);
        assert!(capabilities.starttls && capabilities.eight_bit_mime);
        assert_eq!(capabilities.auth, ["PLAIN"]);
        assert_eq!(capabilities.size, Some(10));
    }

    #[test]
    fn a_size_without_a_number_or_of_zero_is_no_limit() {
        assert_eq!(ehlo(&["host", "SIZE"]).size, None);
        assert_eq!(ehlo(&["host", "SIZE 0"]).size, None);
        assert_eq!(ehlo(&["host", "SIZE lots"]).size, None);
    }

    #[test]
    fn the_old_auth_equals_form_is_read_and_merged() {
        let capabilities = ehlo(&["host", "AUTH PLAIN LOGIN", "AUTH=LOGIN XOAUTH2"]);
        assert_eq!(capabilities.auth, ["PLAIN", "LOGIN", "XOAUTH2"]);
    }
}
