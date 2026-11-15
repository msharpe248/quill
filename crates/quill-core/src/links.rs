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

/// What a `mailto:` address asks to be written: who to, and perhaps a
/// subject and some words to start with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailto {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// The most text a `mailto:` address may put in a message's body. A link is
/// a way to start a message, not to write one.
const MAILTO_BODY: usize = 16 * 1024;

/// Reads a `mailto:` address (RFC 6068). `None` for anything else.
///
/// Only recipients, a subject and a body are taken from it. A link cannot
/// attach a file, set a sender or add any other header, whatever it asks
/// for: those would be a stranger writing the user's mail for them. Control
/// characters are dropped, so what comes back is safe to put in a form.
pub fn parse_mailto(url: &str) -> Option<Mailto> {
    let rest = url
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .map(|_| &url[7..])?;
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut mailto = Mailto {
        to: addresses(&percent_decode(to)),
        ..Mailto::default()
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match percent_decode(name).to_ascii_lowercase().as_str() {
            "to" => mailto.to.extend(addresses(&value)),
            "cc" => mailto.cc.extend(addresses(&value)),
            "bcc" => mailto.bcc.extend(addresses(&value)),
            // One line: a subject cannot smuggle in a header of its own.
            "subject" => mailto.subject = one_line(&value),
            "body" => {
                let mut body: String = value
                    .replace("\r\n", "\n")
                    .chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .collect();
                if body.len() > MAILTO_BODY {
                    body.truncate(body.floor_char_boundary(MAILTO_BODY));
                }
                mailto.body = body;
            }
            _ => {}
        }
    }
    Some(mailto)
}

/// `%41` and the like, as UTF-8. A `+` is a plus: in a `mailto:` address
/// it does not mean a space. What is not valid is kept as written.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |index: usize| bytes.get(index).and_then(|b| (*b as char).to_digit(16));
        match (bytes[at], hex(at + 1), hex(at + 2)) {
            (b'%', Some(high), Some(low)) => {
                // Two hex digits always fit in a byte.
                out.push(u8::try_from(high * 16 + low).unwrap_or(b'?'));
                at += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The addresses in a comma-separated list, each on one line.
fn addresses(list: &str) -> Vec<String> {
    list.split(',')
        .map(one_line)
        .filter(|address| !address.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mail_address_says_who_to_and_what_to_start_with() {
        assert_eq!(
            parse_mailto("mailto:ana.ruiz@example.com"),
            Some(Mailto {
                to: vec!["ana.ruiz@example.com".to_owned()],
                ..Mailto::default()
            })
        );
        let full = parse_mailto(
            "MAILTO:ana@example.com,%22Kit%20Moss%22%20%3Ckit@example.com%3E?subject=Q4%20roadmap&cc=sam@example.com&bcc=lee@example.com&to=jo@example.com&body=Hello%2C%0D%0A%0D%0AOne%20%2B%20one.",
        )
        .unwrap();
        assert_eq!(
            full.to,
            [
                "ana@example.com",
                "\"Kit Moss\" <kit@example.com>",
                "jo@example.com"
            ]
        );
        assert_eq!(full.cc, ["sam@example.com"]);
        assert_eq!(full.bcc, ["lee@example.com"]);
        assert_eq!(full.subject, "Q4 roadmap");
        assert_eq!(full.body, "Hello,\n\nOne + one.");
        // A plus is a plus, and an address may have none at all.
        assert_eq!(
            parse_mailto("mailto:a+b@example.com").unwrap().to,
            ["a+b@example.com"]
        );
        assert_eq!(
            parse_mailto("mailto:?subject=Hi").unwrap(),
            Mailto {
                subject: "Hi".to_owned(),
                ..Mailto::default()
            }
        );
    }

    #[test]
    fn a_mail_address_cannot_write_the_users_mail_for_them() {
        let sly = parse_mailto(
            "mailto:ana@example.com?attach=/etc/passwd&from=boss@example.com&subject=Hi%0D%0ABcc%3A%20spy@example.com&body=a%1b%5B2Jb&x-header=1&cc=one@example.com%0Atwo@example.com",
        )
        .unwrap();
        assert_eq!(sly.to, ["ana@example.com"]);
        // No header rides in on the subject or an address, no escape
        // sequence in the body, and nothing else is taken at all.
        assert_eq!(sly.subject, "Hi  Bcc: spy@example.com");
        assert_eq!(sly.body, "a[2Jb");
        assert_eq!(sly.cc, ["one@example.com two@example.com"]);
        assert!(sly.bcc.is_empty());

        let long = format!("mailto:a@example.com?body={}", "x".repeat(MAILTO_BODY * 2));
        assert_eq!(parse_mailto(&long).unwrap().body.len(), MAILTO_BODY);
        for other in ["https://example.com/", "mail:a@example.com", "", "mailt"] {
            assert_eq!(parse_mailto(other), None, "{other}");
        }
    }

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
