//! Reading mail: one pipeline from a raw RFC 5322 message to Quill's types,
//! whichever backend the bytes came from.
//!
//! Nothing here trusts its input. A message is whatever a stranger chose to
//! send: parsing never panics, and every piece of text it returns has had
//! its control characters and escape sequences taken out ([`clean`]), so
//! that drawing it cannot do anything to the terminal. [`html`] and
//! [`plain`] lay a body out for the reader, and [`sanitize`] makes the HTML
//! safe for a web view.
//!
//! Writing mail is here too: [`compose`] turns what the user wrote into a
//! complete message, [`reply`] starts a reply or a forward from a message
//! that was read, and [`markdown`] makes the HTML alternative.

pub mod clean;
pub mod compose;
mod dark;
mod disguise;
mod guard;
pub mod html;
pub mod invite;
pub mod links;
pub mod markdown;
pub mod plain;
pub mod reply;
pub mod sanitize;
pub mod unsubscribe;
mod width;

pub use compose::{
    BodyFormat, Outgoing, OutgoingAttachment, Warning, build, envelope, new_message_id, warnings,
};
pub use reply::{Draft, ReplyPosition, forward, reply};
pub use unsubscribe::{Unsubscribe, list_id};

use mail_parser::{MessageParser, PartKind};
use quill_core::types::{Address, Headers};

/// A message, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub headers: Headers,
    /// Where the sender asked for replies to go (`Reply-To`). Empty when
    /// they did not say, which means back to `headers.from`.
    pub reply_to: Vec<Address>,
    /// The message this one answers (`In-Reply-To`), with its angle
    /// brackets. The first one, should a message name several.
    pub in_reply_to: Option<String>,
    /// The conversation so far (`References`), oldest first, each with its
    /// angle brackets.
    pub references: Vec<String>,
    /// The body as plain text: the plain part when the message has one,
    /// otherwise the text of its HTML. This is what search and quoting use.
    pub body: String,
    /// The HTML body, when the message has one that renders to something.
    /// Lay it out with [`html::render`].
    pub html: Option<String>,
    /// The reader shows the HTML first. False when there is no HTML, or
    /// when the plain part is clearly the fuller of the two.
    pub html_first: bool,
    /// The message has a plain-text part of its own, so `body` is the
    /// sender's text and not a rendering of the HTML.
    pub has_plain: bool,
    pub attachments: Vec<Attachment>,
    /// How to leave the list that sent the message, when it is from a list
    /// that says (`List-Unsubscribe`).
    pub unsubscribe: Option<Unsubscribe>,
}

/// The most earlier messages a message is taken to follow. A long
/// conversation's `References` is cut to its newest: those are the ones
/// that tie it to the mail already here.
const MAX_THREAD: usize = 50;

impl Parsed {
    /// The messages this one follows in its conversation: its `References`,
    /// oldest first, then the one it answers, each named once. Empty for a
    /// message that starts a conversation.
    pub fn thread(&self) -> Vec<String> {
        let mut thread: Vec<String> = Vec::new();
        for id in self.references.iter().chain(&self.in_reply_to) {
            if !thread.contains(id) {
                thread.push(id.clone());
            }
        }
        let extra = thread.len().saturating_sub(MAX_THREAD);
        thread.drain(..extra);
        thread
    }
}

/// A file attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Which part of the message it is. The same raw message always gives
    /// the same number; pass it to [`attachment_bytes`].
    pub part: u32,
    pub name: Option<String>,
    /// The media type, such as `application/pdf`.
    pub content_type: String,
    /// The size once decoded, in bytes.
    pub size: usize,
    /// Its `Content-ID`, without the angle brackets: what a `cid:` address
    /// in the HTML refers to it by.
    pub content_id: Option<String>,
    /// The sender marked it to be shown in the body, not listed as a file.
    pub inline: bool,
}

impl Attachment {
    /// The name, media type and size: what a list of files shows.
    pub fn summary(&self) -> (Option<&str>, &str, usize) {
        (self.name.as_deref(), self.content_type.as_str(), self.size)
    }
}

/// The decoded bytes of one part of a raw message: an attachment, by the
/// number [`Attachment::part`] gave. `None` when the message has no such
/// part.
pub fn attachment_bytes(raw: &[u8], part: u32) -> Option<Vec<u8>> {
    let message = MessageParser::default().parse(raw)?;
    let part = message.part(part)?;
    Some(part.decoded().into_owned())
}

/// The part a `cid:` address refers to: its media type and decoded bytes.
/// `content_id` is the address without the `cid:`, with or without angle
/// brackets.
pub fn part_by_content_id(raw: &[u8], content_id: &str) -> Option<(String, Vec<u8>)> {
    // Ids are compared as `parse` reports them, so one that had control
    // characters taken out of it still finds its part.
    let wanted = content_id_of(content_id)?;
    let message = MessageParser::default().parse(raw)?;
    let part = message
        .parts()
        .find(|part| part.content_id().and_then(content_id_of).as_ref() == Some(&wanted))?;
    Some((media_type(&part), part.decoded().into_owned()))
}

/// A `Content-ID` as Quill reports it: cleaned, and without its angle
/// brackets. `None` for one with nothing in it.
fn content_id_of(id: &str) -> Option<String> {
    let id = clean::header(id);
    let id = id.trim().trim_matches(|c| c == '<' || c == '>').trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// A part's media type in lower case, such as `image/png`. A part that
/// does not say is a file of unknown kind.
fn media_type(part: &mail_parser::MessagePart<'_>) -> String {
    let kind = part.content_type().map_or_else(
        || "application/octet-stream".to_owned(),
        |kind| match kind.subtype() {
            Some(subtype) => format!("{}/{subtype}", kind.ctype()).to_ascii_lowercase(),
            None => kind.ctype().to_ascii_lowercase(),
        },
    );
    match clean::header(&kind) {
        std::borrow::Cow::Borrowed(_) => kind,
        std::borrow::Cow::Owned(cleaned) => cleaned,
    }
}

/// Whether `part` is a multipart that was not taken apart: it calls itself
/// one, and the parser hands it over as a single piece.
fn is_unsplit_multipart(part: &mail_parser::MessagePart<'_>) -> bool {
    !part.is_multipart()
        && part
            .content_type()
            .is_some_and(|kind| kind.ctype().eq_ignore_ascii_case("multipart"))
}

/// A header's text as Quill shows it: cleaned, and without the space
/// around it. `None` when nothing is left.
fn header_text(text: &str) -> Option<String> {
    let text = clean::header(text);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
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
                    Some(Address {
                        name: mailbox.name().and_then(header_text),
                        email: header_text(mailbox.address()?)?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    };

    let headers = Headers {
        message_id: message
            .message_id()
            .and_then(header_text)
            .map(|id| format!("<{id}>")),
        subject: message.subject().and_then(header_text).unwrap_or_default(),
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

    let reply_to = addresses(message.reply_to());
    let identifiers = |list: Option<mail_parser::TextList<'_>>| -> Vec<String> {
        list.map(|list| {
            list.iter()
                .filter_map(header_text)
                .map(|id| format!("<{id}>"))
                .collect()
        })
        .unwrap_or_default()
    };
    let in_reply_to = identifiers(message.in_reply_to()).into_iter().next();
    let references = identifiers(message.references());

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
            // Cleaned here, once, for everything downstream: the body
            // that is searched and quoted, and the HTML that is laid out
            // and sanitized.
            joined.push_str(&match html {
                true => clean::markup(&text),
                false => clean::body(&text),
            });
        }
        joined
    };
    let mut plain = join(&mut message.text_body(), false);
    let markup = join(&mut message.html_body(), true);
    // A multipart whose parts could not be told apart (no boundary was
    // given, or it is nested deeper than the parser goes) comes back as one
    // lump. It is shown as the text it is; as a file it would be a file
    // called nothing that no program opens.
    for part in message.attachments().filter(is_unsplit_multipart) {
        let lump = part.decoded();
        let text = String::from_utf8_lossy(&lump);
        if !text.trim().is_empty() {
            if !plain.is_empty() && !plain.ends_with('\n') {
                plain.push('\n');
            }
            plain.push_str(&clean::body(&text));
        }
    }
    let Chosen {
        body,
        html,
        html_first,
        has_plain,
    } = choose_body(plain, markup);

    let attachments = message
        .attachments()
        .filter(|part| !is_unsplit_multipart(part))
        .map(|part| Attachment {
            part: part.id(),
            // A file name is where a direction override does its work:
            // `header_text` takes those out with the control characters.
            name: part.attachment_name().and_then(header_text),
            content_type: match part.content_type() {
                // Inside a digest, a part that does not say what it is is
                // a message.
                None if part
                    .parent()
                    .is_some_and(|parent| parent.is_content_type("multipart", "digest")) =>
                {
                    "message/rfc822".to_owned()
                }
                _ => media_type(&part),
            },
            size: part.decoded_len(),
            content_id: part.content_id().and_then(content_id_of),
            inline: part.is_inline(),
        })
        .collect();

    Some(Parsed {
        headers,
        reply_to,
        in_reply_to,
        references,
        body,
        html,
        html_first,
        has_plain,
        attachments,
        unsubscribe: unsubscribe::read(&message),
    })
}

struct Chosen {
    body: String,
    html: Option<String>,
    html_first: bool,
    has_plain: bool,
}

/// Decides what the reader shows, given a message's plain text and its HTML
/// (either may be empty).
///
/// HTML is shown first when the message has it: senders write the HTML and
/// let their software generate the plain part, which is often a stub ("your
/// mail program cannot show this message") or a list of raw tracking links.
/// The plain part comes first only when the HTML renders to so much less
/// that it is clearly not the whole message, such as a lone image. HTML
/// that renders to nothing is not offered at all.
fn choose_body(plain: String, markup: String) -> Chosen {
    let has_plain = !plain.trim().is_empty();
    let rendered = (!markup.trim().is_empty())
        .then(|| html::to_text(&markup))
        .flatten();
    let Some(rendered) = rendered else {
        return Chosen {
            body: plain,
            html: None,
            html_first: false,
            has_plain,
        };
    };
    if !has_plain {
        return Chosen {
            body: rendered,
            html: Some(markup),
            html_first: true,
            has_plain,
        };
    }
    let visible = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count();
    Chosen {
        html_first: visible(&rendered) * 4 >= visible(&plain),
        body: plain,
        html: Some(markup),
        has_plain,
    }
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
        assert!(parsed.html.is_some() && parsed.html_first && parsed.has_plain);
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
        // The HTML is still there to switch to; it is just not shown first.
        assert!(parsed.html.is_some() && !parsed.html_first && parsed.has_plain);
        assert!(parsed.body.starts_with("The quarterly figures"));
    }

    #[test]
    fn a_plain_message_has_no_html() {
        let parsed = read("Subject: x\n\nJust text, with <b>angle brackets</b>.\n");
        assert_eq!(parsed.html, None);
        assert!(!parsed.html_first && parsed.has_plain);
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
        // With no plain part there is nothing to switch to.
        assert!(parsed.html.is_some() && parsed.html_first && !parsed.has_plain);
        assert!(!parsed.body.contains('<'), "{:?}", parsed.body);
        assert!(!parsed.body.contains("alert"), "{:?}", parsed.body);
    }

    #[test]
    fn attachments_are_listed_with_name_type_and_decoded_size() {
        const MULTIPART_WITH_FILES: &str = "Subject: Invoice\n\
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
             --b--\n";
        let parsed = read(MULTIPART_WITH_FILES);
        assert_eq!(parsed.body.trim_end(), "Please find the invoice attached.");
        let files: Vec<_> = parsed.attachments.iter().map(Attachment::summary).collect();
        assert_eq!(
            files,
            [
                (Some("invoice 2026-114.pdf"), "application/pdf", 9),
                (Some("☕ photo.png"), "image/png", 8),
            ]
        );
        // Each can be fetched by the part number it was listed with, and
        // neither claims to belong in the body.
        let raw = MULTIPART_WITH_FILES.as_bytes();
        assert_eq!(
            attachment_bytes(raw, parsed.attachments[0].part).as_deref(),
            Some(&b"%PDF-1.4\n"[..])
        );
        assert_eq!(
            attachment_bytes(raw, parsed.attachments[1].part).map(|bytes| bytes.len()),
            Some(8)
        );
        assert!(parsed.attachments.iter().all(|file| !file.inline));
        assert_eq!(attachment_bytes(raw, 99), None);
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
    fn escape_sequences_are_taken_out_of_everything_that_is_drawn() {
        // A window title, a clipboard write, a hyperlink, clear screen, and
        // the eight-bit forms of the same, in every place text comes from.
        let raw = "From: \"\u{1b}]8;;https://evil.example/\u{7}Your \u{1b}[1mBank\u{1b}]8;;\u{7}\" <ana.ruiz@example.com>\r\n\
                   To: =?UTF-8?Q?=1B=5D0=3Bowned=07Robin_=1B=5B2JVale?= <robin\u{1b}[0m@example.com>\r\n\
                   Cc: \"Sam\u{9b}31m \u{202e}eeL\" <sam.lee@example.com>\r\n\
                   Reply-To: \"Priya\u{7}\u{8}\u{7f} Nair\" <priya.nair@example.com>\r\n\
                   Subject: \u{1b}]0;owned\u{7}\u{1b}]52;c;cm0gLXJmIH4=\u{7}Invoice\u{1b}[2J\u{1b}[H overdue\u{9d}0;x\u{9c}\r\n\
                   Message-ID: <id\u{1b}[31m@example.com>\r\n\
                   In-Reply-To: <earlier\u{1b}]0;x\u{7}@example.com>\r\n\
                   References: <one\u{1b}c@example.com> <two\u{0}@example.com>\r\n\
                   Content-Type: multipart/mixed; boundary=\"b\"\r\n\
                   \r\n\
                   --b\r\n\
                   Content-Type: text/plain; charset=utf-8\r\n\
                   \r\n\
                   \u{1b}]0;owned\u{7}Dear Robin,\u{1b}[2J\r\n\
                   \u{1b}]52;c;cm0gLXJmIH4=\u{7}Pay Ana 100\rPay Eve 900\r\n\
                   \tTabs and\u{b} \u{9b}5mblinks\u{85}.\r\n\
                   --b\r\n\
                   Content-Type: text/html; charset=utf-8\r\n\
                   \r\n\
                   <p>\u{1b}]0;owned\u{7}Dear &#27;[2JRobin,&#x1b;]52;c;cm0gLXJmIH4=&#7;</p>\r\n\
                   --b\r\n\
                   Content-Type: application/pdf; name=\"in\u{1b}[2Jvoice\u{202e}fdp.exe\"\r\n\
                   Content-Disposition: attachment; filename=\"in\u{1b}[2Jvoice\u{202e}fdp.exe\"\r\n\
                   Content-ID: <file\u{1b}]0;x\u{7}@example.com>\r\n\
                   \r\n\
                   \u{1b}[2J kept as it is\r\n\
                   --b--\r\n";
        let parsed = parse(raw.as_bytes(), RECEIVED).expect("a message");
        assert_eq!(parsed.headers.subject, "Invoice overdue");
        assert_eq!(
            parsed.headers.from,
            [Address::new("Your Bank", "ana.ruiz@example.com")]
        );
        assert_eq!(
            parsed.headers.to,
            [Address::new("Robin Vale", "robin@example.com")]
        );
        // The override that would have drawn "eeL" as "Lee" is gone too.
        assert_eq!(
            parsed.headers.cc,
            [Address::new("Sam eeL", "sam.lee@example.com")]
        );
        assert_eq!(
            parsed.reply_to,
            [Address::new("Priya Nair", "priya.nair@example.com")]
        );
        assert_eq!(
            parsed.headers.message_id.as_deref(),
            Some("<id@example.com>")
        );
        assert_eq!(parsed.in_reply_to.as_deref(), Some("<earlier@example.com>"));
        assert_eq!(
            parsed.references,
            ["<one@example.com>", "<two@example.com>"]
        );
        assert_eq!(
            parsed.body,
            "Dear Robin,\nPay Ana 100\nPay Eve 900\n\tTabs and blinks."
        );
        assert_eq!(
            parsed.html.as_deref().map(str::trim_end),
            Some("<p>Dear Robin,</p>")
        );

        let file = &parsed.attachments[0];
        assert_eq!(file.name.as_deref(), Some("invoicefdp.exe"));
        assert_eq!(file.content_id.as_deref(), Some("file@example.com"));
        // The id as reported finds the part, and the file's own bytes are
        // nobody's to clean.
        assert!(part_by_content_id(raw.as_bytes(), "file@example.com").is_some());
        assert_eq!(
            attachment_bytes(raw.as_bytes(), file.part).as_deref(),
            Some(&b"\x1b[2J kept as it is"[..])
        );
    }

    #[test]
    fn a_multipart_that_cannot_be_taken_apart_is_shown_as_text() {
        // No boundary was named, so there is no telling the parts apart.
        let parsed = read(
            "Subject: x\n\
             Content-Type: multipart/mixed\n\
             \n\
             --guess\n\
             Content-Type: text/plain\n\
             \n\
             What the sender wrote.\n\
             --guess--\n",
        );
        assert!(
            parsed.body.contains("What the sender wrote."),
            "{:?}",
            parsed.body
        );
        assert!(parsed.has_plain && parsed.attachments.is_empty());
        // One that promised parts and has nothing is not a file either.
        let empty = read("Subject: x\nContent-Type: multipart/mixed; boundary=\"b\"\n");
        assert!(empty.body.is_empty() && empty.attachments.is_empty());
    }

    #[test]
    fn a_part_of_a_digest_that_does_not_say_what_it_is_is_a_message() {
        let parsed = read(
            "Subject: Digest\n\
             Content-Type: multipart/digest; boundary=\"d\"\n\
             \n\
             --d\n\
             \n\
             From: Ana Ruiz <ana.ruiz@example.com>\n\
             Subject: First\n\
             \n\
             One.\n\
             --d\n\
             Content-Type: text/plain\n\
             Content-Disposition: attachment\n\
             \n\
             Says what it is.\n\
             --d--\n",
        );
        let kinds: Vec<&str> = parsed
            .attachments
            .iter()
            .map(|file| file.content_type.as_str())
            .collect();
        assert_eq!(kinds, ["message/rfc822", "text/plain"]);
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
