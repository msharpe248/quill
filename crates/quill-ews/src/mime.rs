//! The little of a message's headers the way EWS sends makes it necessary
//! to read.
//!
//! EWS is handed a message and no envelope: Exchange sends it to the
//! addresses in its `To`, `Cc` and `Bcc` headers. Before a message is
//! handed over, the recipients it is meant for are checked against those
//! headers, so that nobody is left out without anyone being told.
//!
//! `quill-graph` reads the same headers for the same reason, in the same
//! way. Neither crate can lean on `quill-mime`, and what is wanted is less
//! than a parser: whether an address stands in three headers.

/// The first of `recipients` that the message's `To`, `Cc` and `Bcc`
/// headers do not name, if there is one.
pub(crate) fn unnamed<'a>(raw: &[u8], recipients: &'a [String]) -> Option<&'a str> {
    let named = addressed_to(raw).to_lowercase();
    recipients
        .iter()
        .map(String::as_str)
        .find(|recipient| !names(&named, &recipient.trim().to_lowercase()))
}

/// The text of every `To`, `Cc` and `Bcc` header, folded lines joined.
fn addressed_to(raw: &[u8]) -> String {
    let mut text = String::new();
    // Inside one of the three: its continuation lines belong to it.
    let mut wanted = false;
    for line in raw.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match line.first() {
            // The empty line that ends the headers.
            None => break,
            Some(b' ' | b'\t') => {
                if wanted {
                    text.push_str(&String::from_utf8_lossy(line));
                }
            }
            Some(_) => {
                let (name, value) = match line.iter().position(|byte| *byte == b':') {
                    Some(colon) => (&line[..colon], &line[colon + 1..]),
                    None => (line, &[][..]),
                };
                // The old syntax allows spaces between the name and the colon.
                let name = name.trim_ascii_end();
                wanted = [&b"To"[..], b"Cc", b"Bcc"]
                    .iter()
                    .any(|header| name.eq_ignore_ascii_case(header));
                if wanted {
                    // Keeps the last address of one header and the first of
                    // the next apart.
                    text.push(',');
                    text.push_str(&String::from_utf8_lossy(value));
                }
            }
        }
    }
    text
}

/// Whether `address` stands in `headers` as an address of its own, and not
/// as the tail of a longer one: `ann@example.com` is not named by a header
/// that says `joann@example.com`.
fn names(headers: &str, address: &str) -> bool {
    if address.is_empty() {
        return false;
    }
    headers.match_indices(address).any(|(at, found)| {
        let before = headers[..at].chars().next_back();
        let after = headers[at + found.len()..].chars().next();
        !before.is_some_and(in_address) && !after.is_some_and(in_address)
    })
}

/// Whether `c` can be part of an unquoted address.
fn in_address(c: char) -> bool {
    c.is_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.@".contains(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(addresses: &[&str]) -> Vec<String> {
        addresses.iter().map(|a| (*a).to_owned()).collect()
    }

    const MESSAGE: &[u8] = b"From: Robin Okafor <robin@example.com>\r\n\
        To: Ana Ruiz <Ana.Ruiz@example.com>,\r\n\
        \t\"Haddad, Omar\" <omar.haddad@example.com>\r\n\
        Subject: Minutes\r\n\
        cc: lena.fischer@example.com\r\n\
        Bcc : Sam Park <sam.park@example.com>\r\n\
        \r\n\
        To: nobody@example.com is only a line in the body.\r\n";

    #[test]
    fn recipients_in_to_cc_and_bcc_are_named() {
        assert_eq!(
            unnamed(
                MESSAGE,
                &to(&[
                    "ana.ruiz@example.com",
                    "omar.haddad@example.com",
                    "LENA.FISCHER@example.com",
                    "sam.park@example.com",
                ])
            ),
            None
        );
    }

    #[test]
    fn a_recipient_the_headers_leave_out_is_found() {
        assert_eq!(
            unnamed(
                MESSAGE,
                &to(&["ana.ruiz@example.com", "kim.novak@example.com"])
            ),
            Some("kim.novak@example.com")
        );
        // The sender is not a recipient, and the body is not a header.
        assert_eq!(
            unnamed(MESSAGE, &to(&["robin@example.com"])),
            Some("robin@example.com")
        );
        assert_eq!(
            unnamed(MESSAGE, &to(&["nobody@example.com"])),
            Some("nobody@example.com")
        );
    }

    #[test]
    fn part_of_a_longer_address_does_not_count() {
        let message = b"To: joann@example.com, ann@example.community\n\nHello\n";
        assert_eq!(
            unnamed(message, &to(&["ann@example.com"])),
            Some("ann@example.com")
        );
        assert_eq!(unnamed(message, &to(&["joann@example.com"])), None);
    }

    #[test]
    fn a_message_with_bcc_stripped_does_not_name_its_blind_recipients() {
        let stripped = b"To: ana.ruiz@example.com\r\nSubject: Minutes\r\n\r\nHello\r\n";
        assert_eq!(
            unnamed(
                stripped,
                &to(&["ana.ruiz@example.com", "sam.park@example.com"])
            ),
            Some("sam.park@example.com")
        );
    }

    #[test]
    fn odd_messages_do_not_panic() {
        for raw in [
            &b""[..],
            b"\r\n",
            b"no colon here",
            b"To",
            b" leading continuation\r\nTo: a@example.com",
            b"To: \xff\xfe@example.com\r\n",
        ] {
            let _ = unnamed(raw, &to(&["a@example.com", ""]));
        }
        assert_eq!(
            unnamed(b" x\r\nTo: a@example.com", &to(&["a@example.com"])),
            None
        );
        assert_eq!(unnamed(b"To: a@example.com", &to(&[""])), Some(""));
        assert_eq!(unnamed(b"To: a@example.com", &[]), None);
    }
}
