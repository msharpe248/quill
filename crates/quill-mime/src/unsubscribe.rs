//! What a message offers for leaving the list that sent it.
//!
//! A list says how in `List-Unsubscribe` (RFC 2369): addresses in angle
//! brackets, separated by commas, each a web page or a mail address.
//! `List-Unsubscribe-Post: List-Unsubscribe=One-Click` (RFC 8058) adds that
//! the web address takes a bare POST, with no page to click through.
//!
//! These headers are a stranger's, like the rest of the message. Only
//! `https` and `mailto` addresses are taken, each must be short and plain
//! printable ASCII, and a web address must name its server outright: no
//! user name in front of the host, no number in place of a name. Anything
//! else is dropped, not repaired: an address that had to be changed to be
//! safe is no longer the address the list gave.

use quill_core::links::parse_mailto;

/// The most of a header that is looked at. A real one is a line or two.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// The longest address taken. A longer one is dropped whole.
pub const MAX_ADDRESS_BYTES: usize = 2048;

/// The most addresses of each kind that are kept.
pub const MAX_ADDRESSES: usize = 4;

/// The ways a message gives to unsubscribe from its list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unsubscribe {
    /// The `https` addresses, in the order the list gave them.
    pub https: Vec<String>,
    /// The `mailto:` addresses, in the order the list gave them.
    pub mailto: Vec<String>,
    /// The list takes a one-click POST (RFC 8058) at its `https` address.
    pub one_click: bool,
}

/// One way to unsubscribe, and the address it uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way<'a> {
    /// POST `List-Unsubscribe=One-Click` to this `https` address.
    OneClick(&'a str),
    /// Write to this `mailto:` address.
    Mail(&'a str),
    /// Open this `https` page, where the list says what to do next.
    Page(&'a str),
}

impl Unsubscribe {
    /// The way to use: one click if the list takes it and `can_post` (there
    /// is something to send the request with), otherwise a message to the
    /// list, otherwise its page.
    pub fn way(&self, can_post: bool) -> Option<Way<'_>> {
        let web = self.https.first().map(String::as_str);
        match (web, self.mailto.first()) {
            (Some(url), _) if self.one_click && can_post => Some(Way::OneClick(url)),
            (_, Some(address)) => Some(Way::Mail(address)),
            (Some(url), None) => Some(Way::Page(url)),
            (None, None) => None,
        }
    }
}

/// The server an `https` address names, when it names one plainly: a
/// domain name with a dot in it, in letters, digits and hyphens. `None`
/// for any other scheme, and for an address with a user name before the
/// host, a backslash, a bare number or a bracketed address for a host, or
/// a port that is not a number. Those are how an address is made to read
/// as one server and lead to another, or to a machine on the user's own
/// network.
pub fn web_host(url: &str) -> Option<&str> {
    let rest = url
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])?;
    if url.contains('\\') {
        return None;
    }
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let plain = !port.is_empty()
                && port.len() <= 5
                && port.bytes().all(|byte| byte.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|port| port != 0);
            if !plain {
                return None;
            }
            host
        }
        None => authority,
    };
    let labels: Vec<&str> = host.split('.').collect();
    let named = host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        // A name ends in letters. All digits is an IP address.
        && labels
            .last()
            .is_some_and(|last| last.bytes().any(|byte| byte.is_ascii_alphabetic()));
    named.then_some(host)
}

/// Reads the two headers' values, as written after their colons. `None`
/// when the message offers nothing Quill will use.
pub fn from_headers(list_unsubscribe: &str, post: Option<&str>) -> Option<Unsubscribe> {
    let mut found = Unsubscribe::default();
    for address in addresses(list_unsubscribe) {
        if web_host(&address).is_some() {
            if found.https.len() < MAX_ADDRESSES && !found.https.contains(&address) {
                found.https.push(address);
            }
        } else if is_mail(&address)
            && found.mailto.len() < MAX_ADDRESSES
            && !found.mailto.contains(&address)
        {
            found.mailto.push(address);
        }
    }
    // The POST is to a web address: with none, there is no one click.
    found.one_click = !found.https.is_empty()
        && post.is_some_and(|value| {
            let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
            value.eq_ignore_ascii_case("List-Unsubscribe=One-Click")
        });
    (!found.https.is_empty() || !found.mailto.is_empty()).then_some(found)
}

/// What a parsed message's headers offer. Of a header given twice, which
/// the standard forbids, the first is the one read.
pub(crate) fn read(message: &mail_parser::Message<'_>) -> Option<Unsubscribe> {
    let headers = message.headers();
    let value = |name: &'static str| headers.all(name).next().map(|header| header.raw_value());
    let list = String::from_utf8_lossy(value("List-Unsubscribe")?);
    let post = value("List-Unsubscribe-Post").map(String::from_utf8_lossy);
    from_headers(&list, post.as_deref())
}

/// The mailing list a message came through, as its `List-Id` says: the
/// header's value on one line, cleaned, and no longer than a list's name
/// has any need to be. `None` for a message that names no list, or that
/// cannot be read at all. Only the headers are read.
pub fn list_id(raw: &[u8]) -> Option<String> {
    /// A list's name and id are short; a longer value is someone trying
    /// something.
    const LONGEST: usize = 512;
    let message = mail_parser::MessageParser::default().parse_headers(raw)?;
    let value = message
        .headers()
        .iter()
        .find(|header| header.name().as_str().eq_ignore_ascii_case("List-Id"))
        .and_then(|header| raw.get(header.offset_start() as usize..header.offset_end() as usize))?;
    let text = String::from_utf8_lossy(value);
    let cleaned = crate::clean::header(&text);
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(LONGEST).collect();
    (!cut.is_empty()).then_some(cut)
}

/// A `mailto:` address that names someone to write to.
fn is_mail(address: &str) -> bool {
    parse_mailto(address).is_some_and(|mailto| {
        mailto.to.first().is_some_and(|to| {
            to.split_once('@')
                .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'))
                && !to
                    .contains(|c: char| c.is_whitespace() || c.is_control() || "<>,;\"".contains(c))
        })
    })
}

/// The addresses in a `List-Unsubscribe` value that could be used, as far
/// as their shape goes: what stands in angle brackets, and, from a sender
/// who left the brackets off, what stands between commas. Comments in
/// parentheses are skipped, and white space inside an address (a header
/// folded in the middle of one) is closed up, as RFC 2369 asks.
fn addresses(value: &str) -> Vec<String> {
    let value = &value[..value.floor_char_boundary(MAX_HEADER_BYTES.min(value.len()))];
    let mut found = Vec::new();
    // What is being read: inside brackets, or loose between commas.
    let mut bracketed: Option<String> = None;
    let mut loose = String::new();
    let mut had_brackets = false;
    let mut comment = 0usize;
    let mut escaped = false;
    let mut keep = |text: &str| {
        let plain = !text.is_empty()
            && text.len() <= MAX_ADDRESS_BYTES
            && text.bytes().all(|byte| byte.is_ascii_graphic());
        if plain {
            found.push(text.to_owned());
        }
    };
    for c in value.chars() {
        if let Some(inside) = &mut bracketed {
            match c {
                '>' => {
                    keep(inside);
                    bracketed = None;
                }
                // A second opening bracket: what came before was litter.
                '<' => inside.clear(),
                c if c.is_whitespace() => {}
                c => inside.push(c),
            }
            continue;
        }
        if comment > 0 {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '(' => comment += 1,
                ')' => comment -= 1,
                _ => {}
            }
            continue;
        }
        match c {
            '<' => {
                bracketed = Some(String::new());
                had_brackets = true;
            }
            '(' => comment = 1,
            ',' => {
                if !had_brackets {
                    keep(loose.trim());
                }
                loose.clear();
                had_brackets = false;
            }
            c => loose.push(c),
        }
    }
    // A bracket never closed holds nothing to use.
    if bracketed.is_none() && !had_brackets {
        keep(loose.trim());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clean;

    fn read(list: &str, post: Option<&str>) -> Unsubscribe {
        from_headers(list, post).expect("a way to unsubscribe")
    }

    #[test]
    fn a_web_and_a_mail_address_are_both_read() {
        let found = read(
            " <https://lists.example.com/u/abc123>, <mailto:leave@lists.example.com?subject=unsubscribe>\r\n",
            None,
        );
        assert_eq!(found.https, ["https://lists.example.com/u/abc123"]);
        assert_eq!(
            found.mailto,
            ["mailto:leave@lists.example.com?subject=unsubscribe"]
        );
        assert!(!found.one_click);
        // Without the POST header a message is written, and the page is
        // only what is left when there is no address to write to.
        assert_eq!(found.way(true), Some(Way::Mail(&found.mailto[0])));
        let page = read("<https://lists.example.com/u/abc123>", None);
        assert_eq!(page.way(true), Some(Way::Page(&page.https[0])));
    }

    #[test]
    fn the_post_header_makes_it_one_click() {
        for post in [
            "List-Unsubscribe=One-Click",
            " List-Unsubscribe=One-Click\r\n",
            "list-unsubscribe=one-click",
            "List-Unsubscribe=\r\n One-Click",
        ] {
            let found = read(
                "<mailto:leave@lists.example.com>, <https://lists.example.com/u/abc>",
                Some(post),
            );
            assert!(found.one_click, "{post:?}");
            assert_eq!(found.way(true), Some(Way::OneClick(&found.https[0])));
            // With nothing to send the request with, a message is written.
            assert_eq!(found.way(false), Some(Way::Mail(&found.mailto[0])));
        }
        for post in ["", "One-Click", "List-Unsubscribe=Two-Click", "yes"] {
            let found = read("<https://lists.example.com/u/abc>", Some(post));
            assert!(!found.one_click, "{post:?}");
        }
        // One click is a POST to a web address. With only a mail address
        // there is nothing to post to, whatever the header says.
        let found = read(
            "<mailto:leave@lists.example.com>",
            Some("List-Unsubscribe=One-Click"),
        );
        assert!(!found.one_click);
        assert_eq!(found.way(true), Some(Way::Mail(&found.mailto[0])));
    }

    #[test]
    fn folding_comments_and_missing_brackets_are_tolerated() {
        // Folded between the addresses, and in the middle of one.
        let found = read(
            "\r\n <https://lists.example.com/u/\r\n\tabc123>,\r\n (the list's robot) <mailto:leave@lists.example.com>\r\n",
            None,
        );
        assert_eq!(found.https, ["https://lists.example.com/u/abc123"]);
        assert_eq!(found.mailto, ["mailto:leave@lists.example.com"]);

        // Comments, nested and with an escaped parenthesis, hold nothing.
        let found = read(
            "(use <https://evil.example.com/> (really \\) <https://evil.example.com/2>)) <https://lists.example.com/u>",
            None,
        );
        assert_eq!(found.https, ["https://lists.example.com/u"]);

        // No brackets at all, from a sender who never read the standard.
        let found = read(
            "https://lists.example.com/u/abc , mailto:leave@lists.example.com",
            None,
        );
        assert_eq!(found.https, ["https://lists.example.com/u/abc"]);
        assert_eq!(found.mailto, ["mailto:leave@lists.example.com"]);

        // Words around a bracketed address are not an address themselves.
        let found = read(
            "Unsubscribe here: <https://lists.example.com/u>, or mailto:x@lists.example.com today",
            None,
        );
        assert_eq!(found.https, ["https://lists.example.com/u"]);
        assert!(found.mailto.is_empty());

        // The same address twice is one address.
        let found = read(
            "<https://lists.example.com/u>, <https://lists.example.com/u>",
            None,
        );
        assert_eq!(found.https.len(), 1);
    }

    #[test]
    fn only_https_and_mailto_are_taken() {
        for value in [
            "<http://lists.example.com/u/abc>",
            "<ftp://lists.example.com/u>",
            "<javascript:alert(1)>",
            "<file:///etc/passwd>",
            "<data:text/html,hello>",
            "<tel:+15555550100>",
            "<x-apple-reminder://x>",
            "<//lists.example.com/u>",
            "<lists.example.com/u>",
            "<>",
            "<",
            ">",
            "",
            ",,,",
            "<https://lists.example.com/u",
            "(<https://lists.example.com/u>",
            "<mailto:>",
            "<mailto:nobody>",
            "<mailto:?to=>",
        ] {
            assert_eq!(from_headers(value, None), None, "{value:?}");
        }
        // The one that can be used is, with the rest left out.
        let found = read(
            "<http://lists.example.com/u>, <https://lists.example.com/u>",
            Some("List-Unsubscribe=One-Click"),
        );
        assert_eq!(found.https, ["https://lists.example.com/u"]);
        assert!(found.one_click);
    }

    #[test]
    fn a_web_address_must_name_its_server_plainly() {
        for (url, host) in [
            ("https://lists.example.com/u/abc", "lists.example.com"),
            ("HTTPS://Lists.Example.COM", "Lists.Example.COM"),
            (
                "https://lists.example.com:8443/u?x=1#y",
                "lists.example.com",
            ),
            ("https://xn--bcher-kva.example/u", "xn--bcher-kva.example"),
            ("https://lists.example.com?u=a@b", "lists.example.com"),
        ] {
            assert_eq!(web_host(url), Some(host), "{url}");
        }
        for url in [
            // Made to read as one server and lead to another.
            "https://lists.example.com@evil.example.net/u",
            "https://lists.example.com:pw@evil.example.net/u",
            "https://lists.example.com\\@evil.example.net/u",
            "https://evil.example.net\\.lists.example.com/u",
            "https://lists.example.com%2f@evil.example.net/",
            "https://lists%2eexample.com/u",
            // A machine on the user's own network, or the user's own.
            "https://127.0.0.1/u",
            "https://192.168.1.1:8443/admin",
            "https://2130706433/u",
            "https://0x7f.0.0.1/u",
            "https://[::1]/u",
            "https://localhost/u",
            "https://router/u",
            // Not an address of a server at all.
            "https://",
            "https:///u",
            "https://.example.com/u",
            "https://example.com./u",
            "https://exa mple.com/u",
            "https://-bad.example.com/u",
            "https://lists.example.com:/u",
            "https://lists.example.com:0/u",
            "https://lists.example.com:99999/u",
            "https://lists.example.com:44a/u",
            "http://lists.example.com/u",
            "mailto:leave@lists.example.com",
            "",
        ] {
            assert_eq!(web_host(url), None, "{url}");
        }
        // And none of them is offered.
        assert_eq!(
            from_headers(
                "<https://lists.example.com@evil.example.net/u>, <https://127.0.0.1/u>",
                Some("List-Unsubscribe=One-Click"),
            ),
            None
        );
    }

    #[test]
    fn what_is_returned_is_clean_and_bounded() {
        // Escape sequences, control characters, direction overrides and
        // letters that only look like ASCII: the address is dropped whole,
        // never handed on with pieces taken out.
        for hostile in [
            "<https://lists.example.com/\u{1b}]0;owned\u{7}u>",
            "<https://lists.example.com/\u{1b}[2Ju>",
            "<https://lists.example.com/u\u{0}>",
            "<https://lists.example.com/\u{202e}moc.live>",
            "<https://lists.ex\u{430}mple.com/u>",
            "<mailto:leave@lists.example.com\u{9b}31m>",
            "<mailto:leave@lists.example.com?subject=\u{1b}[31mred>",
        ] {
            assert_eq!(from_headers(hostile, None), None, "{hostile:?}");
        }
        // An address longer than any real one is dropped, and one that is
        // not is kept beside it.
        let long = format!(
            "<https://lists.example.com/{}>, <https://lists.example.com/u>",
            "a".repeat(MAX_ADDRESS_BYTES)
        );
        assert_eq!(read(&long, None).https, ["https://lists.example.com/u"]);
        // Many addresses: the first few are kept.
        let many: String = (0..100)
            .map(|n| {
                format!("<https://lists.example.com/{n}>, <mailto:leave{n}@lists.example.com>, ")
            })
            .collect();
        let found = read(&many, None);
        assert_eq!(found.https.len(), MAX_ADDRESSES);
        assert_eq!(found.mailto.len(), MAX_ADDRESSES);
        assert_eq!(found.https[0], "https://lists.example.com/0");
        // A header with no end to it is read as far as the limit and no
        // further, and nothing in it panics.
        let endless = format!("{}<https://lists.example.com/u>", "(".repeat(200_000));
        assert_eq!(from_headers(&endless, None), None);
        let endless = format!("{}<https://lists.example.com/u>", "é<".repeat(100_000));
        assert_eq!(from_headers(&endless, None), None);
        let unclosed = format!("<https://lists.example.com/{}", "a".repeat(200_000));
        assert_eq!(from_headers(&unclosed, None), None);

        // Everything that does come back is what `clean` would leave alone.
        let found = read(
            "<https://lists.example.com/u/abc?x=1&y=%20>, <mailto:leave@lists.example.com?subject=unsubscribe%20me>",
            Some("List-Unsubscribe=One-Click"),
        );
        for address in found.https.iter().chain(&found.mailto) {
            assert_eq!(clean::header(address), address.as_str());
            assert!(address.len() <= MAX_ADDRESS_BYTES);
            assert!(quill_core::links::is_openable(address));
        }
    }

    #[test]
    fn the_headers_are_read_from_a_raw_message() {
        let raw = "From: Harbour Books <news@harbourbooks.example.com>\r\n\
                   Subject: November\r\n\
                   List-Unsubscribe: <mailto:leave@harbourbooks.example.com?subject=unsubscribe>,\r\n\
                   \x20<https://harbourbooks.example.com/u/\r\n\
                   \x20r0b1n>\r\n\
                   List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
                   \r\n\
                   New this month.\r\n";
        let parsed = crate::parse(raw.as_bytes(), 0).unwrap();
        assert_eq!(
            parsed.unsubscribe,
            Some(Unsubscribe {
                https: vec!["https://harbourbooks.example.com/u/r0b1n".to_owned()],
                mailto: vec![
                    "mailto:leave@harbourbooks.example.com?subject=unsubscribe".to_owned()
                ],
                one_click: true,
            })
        );
        // The body saying so is not the header saying so.
        let raw = "Subject: x\r\n\r\nList-Unsubscribe: <https://lists.example.com/u>\r\n";
        assert_eq!(crate::parse(raw.as_bytes(), 0).unwrap().unsubscribe, None);
        // Given twice, the first is read.
        let raw = "List-Unsubscribe: <https://first.example.com/u>\r\n\
                   List-Unsubscribe: <https://second.example.com/u>\r\n\r\nx\r\n";
        assert_eq!(
            crate::parse(raw.as_bytes(), 0)
                .unwrap()
                .unsubscribe
                .unwrap()
                .https,
            ["https://first.example.com/u"]
        );
        // Bytes that are not text do no harm.
        let mut raw = b"List-Unsubscribe: <https://lists.example.com/\xff\xfe>, <https://lists.example.com/u>\r\n".to_vec();
        raw.extend_from_slice(b"\r\nx\r\n");
        assert_eq!(
            crate::parse(&raw, 0).unwrap().unsubscribe.unwrap().https,
            ["https://lists.example.com/u"]
        );
    }
    #[test]
    fn the_list_a_message_came_through_is_read_from_its_headers() {
        let raw = b"From: Builds <builds@example.com>\r\nList-Id: Build results\r\n <builds.example.com>\r\nSubject: ok\r\n\r\nList-Id: not this one\r\n";
        assert_eq!(
            list_id(raw).as_deref(),
            Some("Build results <builds.example.com>")
        );
        assert_eq!(list_id(b"From: ana@example.com\r\n\r\nHello\r\n"), None);
        // An escape in it does not reach a terminal.
        let raw = b"List-Id: \x1b[31mred <l.example.com>\r\n\r\n";
        assert!(!list_id(raw).unwrap().contains('\x1b'));
    }
}
