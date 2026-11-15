//! Reading mail: one pipeline from a raw RFC 5322 message to Quill's types,
//! whichever backend the bytes came from.
//!
//! Nothing here trusts its input. A message is whatever a stranger chose to
//! send: parsing never panics, and the text it returns is still untrusted
//! (the UI strips control characters before drawing it).

pub mod html;

use mail_parser::{MessageParser, PartKind};
use quill_core::types::{Address, Headers};

/// A message, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub headers: Headers,
    /// The body as plain text: the plain part when the message has one,
    /// otherwise the text of its HTML. This is what search and quoting use.
    pub body: String,
    /// The HTML body, when the message has one that is worth showing in
    /// place of `body`. Render it with [`html::render`].
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// A file attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: Option<String>,
    /// The media type, such as `application/pdf`.
    pub content_type: String,
    /// The size once decoded, in bytes.
    pub size: usize,
}

/// Reads a raw message. `received` (seconds since the Unix epoch) stands in
/// for the date when the message has none it can use.
///
/// Returns `None` only for input with no headers at all, such as an empty
/// buffer. Anything that looks like a message is read as far as it goes.
pub fn parse(raw: &[u8], received: i64) -> Option<Parsed> {
    let message = MessageParser::default().parse(raw)?;

    let addresses = |list: Option<mail_parser::AddressList<'_>>| -> Vec<Address> {
        list.map(|list| {
            // Groups are flattened: "Friends: a@x, b@x;" is two addresses.
            list.mailboxes()
                .filter_map(|mailbox| {
                    let email = mailbox.address()?.trim();
                    (!email.is_empty()).then(|| Address {
                        name: mailbox
                            .name()
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned),
                        email: email.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    };

    let headers = Headers {
        message_id: message
            .message_id()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| format!("<{id}>")),
        subject: message.subject().unwrap_or_default().trim().to_owned(),
        from: addresses(message.from()),
        to: addresses(message.to()),
        cc: addresses(message.cc()),
        date: message
            .date()
            .map(|date| date.to_timestamp())
            // A date before 1970 or absurdly far ahead is a broken header.
            .filter(|timestamp| (0..=MAX_PLAUSIBLE_DATE).contains(timestamp))
            .unwrap_or(received),
    };

    // Only parts that really are plain text or HTML: the parser offers a
    // conversion of the other kind when one is missing, and Quill does its
    // own.
    let join = |parts: &mut dyn Iterator<Item = mail_parser::MessagePart<'_>>, html: bool| {
        let mut joined = String::new();
        let wanted = |part: &mail_parser::MessagePart<'_>| match part.kind() {
            PartKind::Html => html,
            PartKind::Text => !html,
            _ => false,
        };
        for part in parts.filter(wanted) {
            let Some(text) = part.text() else { continue };
            if !joined.is_empty() && !joined.ends_with('\n') {
                joined.push('\n');
            }
            joined.push_str(&text.replace("\r\n", "\n"));
        }
        joined
    };
    let plain = join(&mut message.text_body(), false);
    let markup = join(&mut message.html_body(), true);
    let (body, html) = choose_body(plain, markup);

    let attachments = message
        .attachments()
        .map(|part| Attachment {
            name: part
                .attachment_name()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            content_type: part.content_type().map_or_else(
                || "application/octet-stream".to_owned(),
                |kind| match kind.subtype() {
                    Some(subtype) => format!("{}/{subtype}", kind.ctype()).to_ascii_lowercase(),
                    None => kind.ctype().to_ascii_lowercase(),
                },
            ),
            size: part.decoded_len(),
        })
        .collect();

    Some(Parsed {
        headers,
        body,
        html,
        attachments,
    })
}

/// Decides what the reader shows, given a message's plain text and its HTML
/// (either may be empty). Returns the plain-text body and, when the HTML is
/// the better thing to show, the HTML.
///
/// HTML is shown when the message has it: senders write the HTML and let
/// their software generate the plain part, which is often a stub ("your mail
/// program cannot show this message") or a list of raw tracking links. The
/// plain part wins only when the HTML renders to nothing, or to so much less
/// that it is clearly not the whole message, such as a lone image.
fn choose_body(plain: String, markup: String) -> (String, Option<String>) {
    if markup.trim().is_empty() {
        return (plain, None);
    }
    let Some(rendered) = html::to_text(&markup) else {
        return (plain, None);
    };
    let visible = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count();
    if plain.trim().is_empty() {
        return (rendered, Some(markup));
    }
    if visible(&rendered) * 4 < visible(&plain) {
        return (plain, None);
    }
    (plain, Some(markup))
}

/// The year 2200, in seconds since the Unix epoch.
const MAX_PLAUSIBLE_DATE: i64 = 7_258_118_400;

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIVED: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

    fn read(raw: &str) -> Parsed {
        parse(raw.replace('\n', "\r\n").as_bytes(), RECEIVED).expect("a message")
    }

    #[test]
    fn a_plain_message() {
        let parsed = read(
            "From: Ana Ruiz <ana.ruiz@example.com>\n\
             To: Robin Vale <robin@example.com>, lena.fischer@example.com\n\
             Cc: \"Haddad, Omar\" <omar.haddad@example.com>\n\
             Subject: Q4 roadmap\n\
             Date: Sat, 14 Nov 2026 09:14:00 +0000\n\
             Message-ID: <abc.123@example.com>\n\
             \n\
             Hi Robin,\n\
             \n\
             Here is the draft.\n",
        );
        assert_eq!(parsed.headers.subject, "Q4 roadmap");
        assert_eq!(
            parsed.headers.from,
            [Address::new("Ana Ruiz", "ana.ruiz@example.com")]
        );
        assert_eq!(
            parsed.headers.to,
            [
                Address::new("Robin Vale", "robin@example.com"),
                Address::new("", "lena.fischer@example.com"),
            ]
        );
        assert_eq!(
            parsed.headers.cc,
            [Address::new("Haddad, Omar", "omar.haddad@example.com")]
        );
        assert_eq!(parsed.headers.date, RECEIVED - 16 * 60);
        assert_eq!(
            parsed.headers.message_id.as_deref(),
            Some("<abc.123@example.com>")
        );
        assert_eq!(parsed.body, "Hi Robin,\n\nHere is the draft.\n");
        assert!(parsed.attachments.is_empty());
    }

    #[test]
    fn the_date_is_read_in_its_own_time_zone() {
        let parsed = read("Date: Sat, 14 Nov 2026 01:14:00 -0800\nSubject: x\n\nbody\n");
        assert_eq!(parsed.headers.date, RECEIVED - 16 * 60);
    }

    #[test]
    fn a_missing_or_absurd_date_falls_back_to_when_it_was_received() {
        for header in [
            "",
            "Date: not a date\n",
            "Date: Mon, 01 Jan 1900 00:00:00 +0000\n",
            "Date: Mon, 01 Jan 9999 00:00:00 +0000\n",
        ] {
            let parsed = read(&format!("{header}Subject: x\n\nbody\n"));
            assert_eq!(parsed.headers.date, RECEIVED, "{header:?}");
        }
    }

    #[test]
    fn encoded_words_in_headers_are_decoded() {
        let parsed = read(
            "From: =?UTF-8?B?6ZmI6Z2Z?= <chen.jing@example.com>\n\
             Subject: =?utf-8?Q?Tom=C3=A1s_sent_an_invoice?= =?utf-8?B?IOKcjg==?=\n\
             \n\
             x\n",
        );
        assert_eq!(parsed.headers.from[0].name.as_deref(), Some("陈静"));
        assert_eq!(parsed.headers.subject, "Tomás sent an invoice ✎");
    }

    #[test]
    fn a_folded_subject_is_one_line() {
        let parsed = read("Subject: a subject that\n continues on the next line\n\nx\n");
        assert_eq!(
            parsed.headers.subject,
            "a subject that continues on the next line"
        );
    }

    #[test]
    fn groups_are_flattened_and_empty_addresses_dropped() {
        let parsed = read(
            "To: Friends: maya.okafor@example.com, Jo Park <jo.park@example.com>;, \
             undisclosed-recipients:;\n\
             Subject: x\n\nx\n",
        );
        assert_eq!(
            parsed.headers.to,
            [
                Address::new("", "maya.okafor@example.com"),
                Address::new("Jo Park", "jo.park@example.com"),
            ]
        );
    }

    #[test]
    fn quoted_printable_in_another_charset_becomes_utf8() {
        let parsed = read(
            "Subject: x\n\
             Content-Type: text/plain; charset=iso-8859-1\n\
             Content-Transfer-Encoding: quoted-printable\n\
             \n\
             Tom=E1s wrote a long line that the sender's software wrapped with a soft=\n \
             break.\n",
        );
        assert_eq!(
            parsed.body,
            "Tomás wrote a long line that the sender's software wrapped with a soft break.\n"
        );
    }

    #[test]
    fn a_base64_body_is_decoded() {
        // "Seven o'clock at the north car park.\n"
        let parsed = read(
            "Subject: x\n\
             Content-Type: text/plain; charset=utf-8\n\
             Content-Transfer-Encoding: base64\n\
             \n\
             U2V2ZW4gbydjbG9jayBhdCB0aGUgbm9ydGggY2FyIHBhcmsuCg==\n",
        );
        assert_eq!(parsed.body, "Seven o'clock at the north car park.\n");
    }

    #[test]
    fn a_message_with_both_keeps_the_plain_text_and_shows_the_html() {
        let parsed = read(
            "Subject: x\n\
             Content-Type: multipart/alternative; boundary=\"b\"\n\
             \n\
             --b\n\
             Content-Type: text/plain; charset=utf-8\n\
             \n\
             The plain version.\n\
             --b\n\
             Content-Type: text/html; charset=utf-8\n\
             \n\
             <p>The <b>HTML</b> version.</p>\n\
             --b--\n",
        );
        assert_eq!(parsed.body.trim_end(), "The plain version.");
        assert_eq!(
            parsed.html.as_deref().map(str::trim_end),
            Some("<p>The <b>HTML</b> version.</p>")
        );
        assert!(parsed.attachments.is_empty());
    }

    fn alternatives(plain: &str, html: &str) -> Parsed {
        read(&format!(
            "Subject: x\n\
             Content-Type: multipart/alternative; boundary=\"b\"\n\
             \n\
             --b\n\
             Content-Type: text/plain; charset=utf-8\n\
             \n\
             {plain}\n\
             --b\n\
             Content-Type: text/html; charset=utf-8\n\
             \n\
             {html}\n\
             --b--\n"
        ))
    }

    #[test]
    fn a_stub_of_a_plain_part_gives_way_to_the_html() {
        let parsed = alternatives(
            "Your mail program cannot show this message.",
            "<h1>Order shipped</h1><p>Your order 1042 left the warehouse today.</p>",
        );
        assert!(parsed.html.is_some());
        // Search still finds what the stub says, not what it hides. The
        // reader shows the HTML.
        assert_eq!(
            parsed.body.trim_end(),
            "Your mail program cannot show this message."
        );
    }

    #[test]
    fn html_that_shows_almost_nothing_gives_way_to_the_plain_text() {
        let parsed = alternatives(
            "The quarterly figures are attached. Revenue is up four percent on last year \
             and costs are flat, so the margin target holds.",
            "<img src=\"cid:scan\"><p>x</p>",
        );
        assert_eq!(parsed.html, None);
        assert!(parsed.body.starts_with("The quarterly figures"));
    }

    #[test]
    fn a_plain_message_has_no_html() {
        let parsed = read("Subject: x\n\nJust text, with <b>angle brackets</b>.\n");
        assert_eq!(parsed.html, None);
        assert_eq!(
            parsed.body.trim_end(),
            "Just text, with <b>angle brackets</b>."
        );
    }

    #[test]
    fn an_html_only_message_is_readable_as_text() {
        let parsed = read(
            "Subject: x\n\
             Content-Type: text/html; charset=utf-8\n\
             \n\
             <html><body><p>Your hold is <b>ready</b>.</p><p>We keep it seven days.</p>\
             <script>alert(1)</script></body></html>\n",
        );
        assert!(
            parsed.body.contains("Your hold is ready."),
            "{:?}",
            parsed.body
        );
        assert!(parsed.body.contains("We keep it seven days."));
        assert!(parsed.html.is_some());
        assert!(!parsed.body.contains('<'), "{:?}", parsed.body);
        assert!(!parsed.body.contains("alert"), "{:?}", parsed.body);
    }

    #[test]
    fn attachments_are_listed_with_name_type_and_decoded_size() {
        let parsed = read(
            "Subject: Invoice\n\
             Content-Type: multipart/mixed; boundary=\"b\"\n\
             \n\
             --b\n\
             Content-Type: text/plain\n\
             \n\
             Please find the invoice attached.\n\
             --b\n\
             Content-Type: application/pdf; name=\"invoice 2026-114.pdf\"\n\
             Content-Disposition: attachment; filename=\"invoice 2026-114.pdf\"\n\
             Content-Transfer-Encoding: base64\n\
             \n\
             JVBERi0xLjQK\n\
             --b\n\
             Content-Type: IMAGE/PNG\n\
             Content-Disposition: attachment; filename*=utf-8''%E2%98%95%20photo.png\n\
             Content-Transfer-Encoding: base64\n\
             \n\
             iVBORw0KGgo=\n\
             --b--\n",
        );
        assert_eq!(parsed.body.trim_end(), "Please find the invoice attached.");
        assert_eq!(
            parsed.attachments,
            [
                Attachment {
                    name: Some("invoice 2026-114.pdf".to_owned()),
                    content_type: "application/pdf".to_owned(),
                    size: 9,
                },
                Attachment {
                    name: Some("☕ photo.png".to_owned()),
                    content_type: "image/png".to_owned(),
                    size: 8,
                },
            ]
        );
    }

    #[test]
    fn a_forwarded_message_is_an_attachment_not_part_of_the_body() {
        let parsed = read(
            "Subject: Fwd: Contract\n\
             Content-Type: multipart/mixed; boundary=\"b\"\n\
             \n\
             --b\n\
             Content-Type: text/plain\n\
             \n\
             See below.\n\
             --b\n\
             Content-Type: message/rfc822\n\
             \n\
             From: Omar Haddad <omar.haddad@example.com>\n\
             Subject: Contract\n\
             \n\
             The inner message.\n\
             --b--\n",
        );
        assert_eq!(parsed.body.trim_end(), "See below.");
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].content_type, "message/rfc822");
    }

    #[test]
    fn a_multipart_that_never_closes_still_yields_its_text() {
        let parsed = read(
            "Subject: broken\n\
             Content-Type: multipart/mixed; boundary=\"b\"\n\
             \n\
             --b\n\
             Content-Type: text/plain\n\
             \n\
             The part that arrived.\n",
        );
        assert_eq!(parsed.body.trim_end(), "The part that arrived.");
    }

    #[test]
    fn missing_headers_give_empty_fields_not_errors() {
        let parsed = read("X-Mailer: something\n\nJust a body.\n");
        assert_eq!(parsed.headers.subject, "");
        assert!(parsed.headers.from.is_empty() && parsed.headers.to.is_empty());
        assert_eq!(parsed.headers.message_id, None);
        assert_eq!(parsed.body, "Just a body.\n");
    }

    #[test]
    fn bare_line_feeds_are_accepted() {
        let parsed = parse(b"Subject: unix\n\nline one\nline two\n", RECEIVED).unwrap();
        assert_eq!(parsed.headers.subject, "unix");
        assert_eq!(parsed.body, "line one\nline two\n");
    }

    #[test]
    fn hostile_bytes_come_through_as_text_for_the_ui_to_strip() {
        let parsed = parse(
            b"Subject: \x1b]0;pwned\x07Free money\r\n\r\nClick \x1b[2Jhere\r\n",
            RECEIVED,
        )
        .unwrap();
        // Parsing does not interpret them and does not lose the text around them.
        assert!(parsed.headers.subject.contains("Free money"));
        assert!(parsed.body.contains("here"));
    }

    #[test]
    fn nothing_makes_the_parser_panic() {
        let seeds: [&[u8]; 9] = [
            b"",
            b"\r\n",
            b"\0\0\0",
            b"Subject",
            b"Subject: =?utf-8?B?!!!?=\r\n\r\n",
            b"Content-Type: multipart/mixed\r\n\r\n--\r\n",
            b"Content-Type: multipart/mixed; boundary=\"\"\r\n\r\n----\r\n",
            b"Content-Type: text/plain; charset=no-such-charset\r\nContent-Transfer-Encoding: base64\r\n\r\n====\r\n",
            b"From: <<<<>>>>,,,;;;:::\r\nTo: \"unterminated\r\nDate: 99 Foo 99999\r\n\r\n",
        ];
        for seed in seeds {
            // Every prefix too: a download can stop anywhere.
            for end in 0..=seed.len() {
                let _ = parse(&seed[..end], RECEIVED);
            }
        }
        // A header block larger than any sane message's.
        let huge = format!("Subject: {}\r\n\r\nbody", "x".repeat(200_000));
        assert!(parse(huge.as_bytes(), RECEIVED).is_some());
        assert_eq!(parse(b"", RECEIVED), None);
    }
}
