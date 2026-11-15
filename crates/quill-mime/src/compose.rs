//! Writing mail: from what the user wrote to a complete RFC 5322 message.
//!
//! The message built here is what a [`quill_core::Sender`] transmits and
//! what is saved as the sent copy. It is written so that any server will
//! take it: nothing but ASCII on the wire, every line ending in CRLF and
//! none longer than the limit. Headers are encoded per RFC 2047 where they
//! need it, bodies as quoted-printable or base64.
//!
//! One thing cannot be encoded away: an address that itself has non-ASCII
//! characters. It is written as it is, and the message then needs a server
//! that offers SMTPUTF8, which the SMTP client checks before sending.
//!
//! Every field is treated as text someone typed or pasted, line breaks and
//! all. None of it can start a header of its own, and building never
//! fails or panics, whatever it is given.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use mail_builder::MessageBuilder;
use mail_builder::headers::HeaderType;
use mail_builder::headers::address::Address as HeaderAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::raw::Raw;
use mail_builder::headers::text::Text;
use mail_builder::mime::{BodyPart, MimePart};
use quill_core::Envelope;
use quill_core::types::Address;

use crate::markdown;

/// How the body was written, which decides what is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BodyFormat {
    /// Markdown: sent as `multipart/alternative`, the text as written plus
    /// HTML made from it.
    #[default]
    Markdown,
    /// Plain text, sent as that alone.
    Plain,
}

/// A message to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Kept in the built message, so the sent copy shows it. The sender
    /// takes the header out before transmitting.
    pub bcc: Vec<Address>,
    pub subject: String,
    /// The body as the user wrote it.
    pub body: String,
    pub format: BodyFormat,
    /// The `Message-ID` of the message this one answers, with its angle
    /// brackets.
    pub in_reply_to: Option<String>,
    /// The conversation so far, oldest first.
    pub references: Vec<String>,
    pub attachments: Vec<OutgoingAttachment>,
    /// When it was written, in seconds since the Unix epoch.
    pub date: i64,
    /// From [`new_message_id`], made once when the draft is created so a
    /// message sent twice by a retry is still one message.
    pub message_id: String,
}

/// A file to send along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub name: String,
    /// The media type, such as `application/pdf`. A whole message being
    /// forwarded is `message/rfc822`.
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Something to ask the user about before sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    /// There is no subject.
    EmptySubject,
    /// The body speaks of an attachment and nothing is attached.
    MissingAttachment,
}

/// The complete message, ready to hand to a [`quill_core::Sender`] and to
/// save as the sent copy.
///
/// `utc_offset_seconds` is the zone the `Date` header is written in. The
/// offset tells every recipient roughly where the user is; pass 0 to write
/// the date in UTC instead.
pub fn build(message: &Outgoing, utc_offset_seconds: i32) -> Vec<u8> {
    let plain = body_text(&message.body);
    let html = match message.format {
        BodyFormat::Markdown => Some(markdown::to_html(&plain)),
        BodyFormat::Plain => None,
    };
    let message_id =
        identifier(&message.message_id).unwrap_or_else(|| fallback_message_id(message, &plain));
    let boundary = boundary_token(message, &message_id, &plain, html.as_deref());

    let to = header_addresses(&message.to);
    let cc = header_addresses(&message.cc);
    let bcc = header_addresses(&message.bcc);

    let mut builder = MessageBuilder::new().from(
        header_address(&message.from)
            .unwrap_or_else(|| HeaderAddress::new_address(None::<String>, String::new())),
    );
    if to.is_empty() && cc.is_empty() {
        // What is transmitted has no Bcc, and a message with no recipient
        // header at all is what spam looks like. This is the customary way
        // to say that the recipients were not listed.
        builder = builder.header("To", Raw::new("undisclosed-recipients:;"));
    }
    if !to.is_empty() {
        builder = builder.to(HeaderAddress::new_list(to));
    }
    if !cc.is_empty() {
        builder = builder.cc(HeaderAddress::new_list(cc));
    }
    if !bcc.is_empty() {
        builder = builder.bcc(HeaderAddress::new_list(bcc));
    }
    builder = builder
        .header("Subject", subject_header(&message.subject))
        .header(
            "Date",
            Raw::new(date_header(message.date, utc_offset_seconds)),
        )
        .message_id(MessageId::new(message_id));
    if let Some(id) = message.in_reply_to.as_deref().and_then(identifier) {
        builder = builder.in_reply_to(MessageId::new(id));
    }
    let references: Vec<String> = message
        .references
        .iter()
        .filter_map(|id| identifier(id))
        .collect();
    if !references.is_empty() {
        builder = builder.references(MessageId::new_list(references.into_iter()));
    }
    // No User-Agent or X-Mailer: the name of the program is nobody's
    // business but the user's, and a version would say more still.

    let text_part = |kind: &'static str, text: &str| {
        MimePart::new(
            ContentType::new(kind).attribute("charset", "utf-8"),
            BodyPart::Text(text.replace('\n', "\r\n").into()),
        )
    };
    let mut body = text_part("text/plain", &plain);
    if let Some(html) = &html {
        body = MimePart::new(
            ContentType::new("multipart/alternative")
                .attribute("boundary", format!("=_alt_{boundary}")),
            vec![body, text_part("text/html", html)],
        );
    }
    if !message.attachments.is_empty() {
        let mut parts = Vec::with_capacity(message.attachments.len() + 1);
        parts.push(body);
        parts.extend(message.attachments.iter().map(attachment_part));
        body = MimePart::new(
            ContentType::new("multipart/mixed").attribute("boundary", format!("=_mix_{boundary}")),
            parts,
        );
    }

    let mut raw = Vec::new();
    builder.body(body).serialize(&mut raw);
    raw
}

/// Who it goes to on the wire: To, Cc and Bcc together, each once.
///
/// The addresses are cleaned the same way [`build`] cleans them for the
/// headers, so the two always agree.
pub fn envelope(message: &Outgoing) -> Envelope {
    let mut to: Vec<String> = Vec::new();
    for address in message.to.iter().chain(&message.cc).chain(&message.bcc) {
        if let Some(email) = mailbox(&address.email)
            && !to.iter().any(|known| same_mailbox(known, &email))
        {
            to.push(email);
        }
    }
    Envelope {
        from: mailbox(&message.from.email).unwrap_or_default(),
        to,
    }
}

/// A new `Message-ID` for mail from `address`, with its angle brackets.
///
/// It is the random bytes and the address's domain, nothing else: not the
/// Mac's name, the time, or a counter, any of which would tell a recipient
/// something about the machine. `random` must come from the system's
/// random source; it is all that makes the identifier unique.
pub fn new_message_id(address: &str, random: &[u8; 16]) -> String {
    let mut id = String::with_capacity(64);
    id.push('<');
    for byte in random {
        id.push_str(&format!("{byte:02x}"));
    }
    id.push('@');
    id.push_str(&identifier_domain(address));
    id.push('>');
    id
}

/// What to ask the user about before sending `message`.
pub fn warnings(message: &Outgoing) -> Vec<Warning> {
    let mut found = Vec::new();
    if message.subject.trim().is_empty() {
        found.push(Warning::EmptySubject);
    }
    if message.attachments.is_empty() && mentions_attachment(&message.body) {
        found.push(Warning::MissingAttachment);
    }
    found
}

/// Whether the user's own words speak of an attachment. Quoted lines are
/// someone else's words and do not count.
fn mentions_attachment(body: &str) -> bool {
    const WORDS: [&str; 5] = [
        "attached",
        "attachment",
        "attachments",
        "attaching",
        "enclosed",
    ];
    body.lines()
        .filter(|line| !line.trim_start().starts_with('>'))
        .flat_map(|line| line.split(|c: char| !c.is_alphabetic()))
        .any(|word| WORDS.iter().any(|known| word.eq_ignore_ascii_case(known)))
}

// Text and addresses.

/// The body as it is sent: `\n` line endings, a final newline, and no
/// control characters but tab. Nobody types an escape or a NUL into a
/// message on purpose, and neither may travel in a 7-bit body.
fn body_text(body: &str) -> String {
    let mut text: String = body
        .replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Text for a header: one line. Line breaks and other control characters
/// become a space, which is what stops a pasted subject or name from
/// starting a header of its own.
fn header_text(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
            gap = true;
            continue;
        }
        if gap && !line.is_empty() && !line.ends_with(' ') && c != ' ' {
            line.push(' ');
        }
        gap = false;
        line.push(c);
    }
    line.trim().to_owned()
}

/// The longest address there can be: 64 for the mailbox, 255 for the
/// domain, and the sign between them.
const MAX_ADDRESS: usize = 320;

/// An address fit to stand between angle brackets and in the envelope, or
/// `None` when nothing usable is left. Spaces, control characters and the
/// characters that separate addresses are taken out: with them one address
/// could pass for two, or for a header.
fn mailbox(email: &str) -> Option<String> {
    let email: String = email
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '<' | '>' | ',' | ';'))
        .collect();
    (!email.is_empty() && email.len() <= MAX_ADDRESS).then_some(email)
}

/// Whether two envelope addresses are one mailbox. The domain's case never
/// matters. The mailbox name's might, to its own server, so there it is
/// compared exactly: a second copy is harmless, a missing one is not.
fn same_mailbox(a: &str, b: &str) -> bool {
    match (a.rsplit_once('@'), b.rsplit_once('@')) {
        (Some((a_name, a_domain)), Some((b_name, b_domain))) => {
            a_name == b_name && a_domain.eq_ignore_ascii_case(b_domain)
        }
        _ => a == b,
    }
}

fn header_address(address: &Address) -> Option<HeaderAddress<'static>> {
    let email = mailbox(&address.email)?;
    let name = address
        .name
        .as_deref()
        .map(header_text)
        .filter(|name| !name.is_empty());
    Some(HeaderAddress::new_address(name, email))
}

fn header_addresses(addresses: &[Address]) -> Vec<HeaderAddress<'static>> {
    addresses.iter().filter_map(header_address).collect()
}

// Headers.

/// The longest word a subject may have and still be folded. With words
/// this short, folding at spaces keeps every line within 78.
const MAX_PLAIN_WORD: usize = 60;

/// The longest subject that shares a 78-character line with "Subject: ".
const ONE_LINE_SUBJECT: usize = 69;

fn subject_header(subject: &str) -> HeaderType<'static> {
    let subject = header_text(subject);
    if !subject.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
        // Outside ASCII: encoded words, Q or B, whichever is shorter.
        return Text::new(subject).into();
    }
    // Plain ASCII is written as it is, folded at its spaces, and stays
    // readable in the raw message. Three things rule that out. "=?" could
    // be read back as an encoded word and decoded into something the user
    // never wrote. A very long word cannot be folded. And where a line is
    // folded at two spaces, some readers give back one.
    let looks_encoded = subject.contains("=?");
    let fits = subject.len() <= ONE_LINE_SUBJECT;
    let folds =
        !subject.contains("  ") && subject.split(' ').all(|word| word.len() <= MAX_PLAIN_WORD);
    if !looks_encoded && (fits || folds) {
        Raw::new(subject).into()
    } else {
        Raw::new(encoded_words(&subject)).into()
    }
}

/// `text` as RFC 2047 encoded words with spaces between, each within the
/// 75 characters an encoded word may have and none splitting a character.
fn encoded_words(text: &str) -> String {
    // 42 bytes become 56 characters of base64, plus 12 around them: short
    // enough to share a 78-character line with "Subject: ".
    const BYTES_PER_WORD: usize = 42;
    let mut words = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(BYTES_PER_WORD);
        while end > 0 && !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            // Cannot happen: a character is at most four bytes.
            break;
        }
        let (chunk, tail) = rest.split_at(end);
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str("=?utf-8?B?");
        words.push_str(&BASE64.encode(chunk));
        words.push_str("?=");
        rest = tail;
    }
    words
}

/// The longest `Message-ID` worth repeating. Real ones are a fraction of
/// this; a longer one would not fit on a header line.
const MAX_IDENTIFIER: usize = 250;

/// A `Message-ID` without its angle brackets, or `None` when it is not one
/// that can be written: it must be printable ASCII with no spaces. A
/// damaged identifier is dropped whole, since a repaired one would name a
/// message that does not exist.
fn identifier(id: &str) -> Option<String> {
    let id = id.trim();
    let id = id.strip_prefix('<').unwrap_or(id);
    let id = id.strip_suffix('>').unwrap_or(id);
    let valid = !id.is_empty()
        && id.len() <= MAX_IDENTIFIER
        && id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'<' | b'>'));
    valid.then(|| id.to_owned())
}

/// The domain a `Message-ID` for mail from `address` ends in: the
/// address's own, or a reserved name when it has none that can be written.
fn identifier_domain(address: &str) -> String {
    let domain = address
        .rsplit_once('@')
        .map_or("", |(_, domain)| domain)
        .trim()
        .trim_end_matches('>')
        .to_ascii_lowercase();
    let valid = !domain.is_empty()
        && domain.len() <= 253
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
    if valid {
        domain
    } else {
        "mail.invalid".to_owned()
    }
}

/// A `Message-ID` for a message that came without a usable one. It is made
/// from the message itself, so building the same message twice gives the
/// same identifier, and it still says nothing about the machine.
fn fallback_message_id(message: &Outgoing, plain: &str) -> String {
    let mut parts: Vec<&[u8]> = vec![
        message.from.email.as_bytes(),
        message.subject.as_bytes(),
        plain.as_bytes(),
    ];
    let recipients = message.to.iter().chain(&message.cc).chain(&message.bcc);
    parts.extend(recipients.map(|address| address.email.as_bytes()));
    format!(
        "{:016x}.{:x}@{}",
        fnv1a(&parts),
        message.date.unsigned_abs(),
        identifier_domain(&message.from.email)
    )
}

/// The last moment of the year 9998. A later date would not fit the four
/// digits a year has in a header once a zone offset is added.
const MAX_DATE: i64 = 253_370_764_799;

/// A `Date` header value such as `Sat, 14 Nov 2026 10:30:00 +0100`.
fn date_header(timestamp: i64, utc_offset_seconds: i32) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    // Zones are whole minutes and less than a day from UTC. Anything else
    // is a mistake, and UTC is the honest answer to it.
    let offset_minutes = if utc_offset_seconds.unsigned_abs() < 86_400 {
        i64::from(utc_offset_seconds / 60)
    } else {
        0
    };
    let local = timestamp.clamp(0, MAX_DATE) + offset_minutes * 60;
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);

    // Days since 1970 to a calendar date (Howard Hinnant's civil_from_days).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    // 1 January 1970 was a Thursday.
    let weekday = usize::try_from((days + 4).rem_euclid(7)).unwrap_or_default();
    let month_name = usize::try_from(month - 1)
        .ok()
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("Jan");
    format!(
        "{}, {day} {month_name} {year:04} {:02}:{:02}:{:02} {}{:02}{:02}",
        DAYS.get(weekday).copied().unwrap_or("Sun"),
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60,
        if offset_minutes < 0 { '-' } else { '+' },
        offset_minutes.abs() / 60,
        offset_minutes.abs() % 60,
    )
}

// Parts.

/// The part that carries one attachment.
fn attachment_part(attachment: &OutgoingAttachment) -> MimePart<'_> {
    let mut kind = media_type(&attachment.content_type);
    let mut name = header_text(&attachment.name);

    if kind == "message/rfc822" {
        if name.is_empty() {
            name = "message.eml".to_owned();
        }
        // A message inside a message may not be base64 or quoted-printable
        // (RFC 2046 section 5.2.1). One that is already 7-bit goes in as it
        // is. One that is not cannot be made so without rewriting someone
        // else's message, so it travels as a file instead: the same bytes,
        // base64, under a name a mail program will open.
        if let Some(lines) = seven_bit_message(&attachment.data) {
            return MimePart::new(
                with_name(ContentType::new("message/rfc822"), &name),
                BodyPart::Binary(lines.into()),
            )
            .attachment(name)
            .transfer_encoding("7bit");
        }
        kind = "application/octet-stream".to_owned();
        if !name.to_ascii_lowercase().ends_with(".eml") {
            name.push_str(".eml");
        }
    }
    if name.is_empty() {
        name = "attachment".to_owned();
    }

    let mut content_type = ContentType::new(kind);
    // Text with no charset is read as ASCII. When the bytes are UTF-8,
    // which is what nearly every text file is now, say so.
    if content_type.is_text() && std::str::from_utf8(&attachment.data).is_ok() {
        content_type = content_type.attribute("charset", "utf-8");
    }
    // A calendar part says what it is for: without `method=REPLY` an
    // answer to an invitation is just a calendar file.
    if let Some(method) = calendar_method(&attachment.content_type) {
        content_type = content_type.attribute("method", method);
    }
    // Always base64, text files too: a file must arrive byte for byte,
    // whatever line endings or control characters it holds.
    MimePart::new(
        with_name(content_type, &name),
        BodyPart::Binary(base64_lines(&attachment.data).into()),
    )
    .attachment(name)
    .transfer_encoding("base64")
}

/// The `method` of a `text/calendar` type, when it is one of the methods
/// there are. It is the one parameter of an attachment's type that is
/// carried over; every other is dropped.
fn calendar_method(content_type: &str) -> Option<&'static str> {
    let mut pieces = content_type.split(';');
    if !pieces
        .next()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/calendar"))
    {
        return None;
    }
    pieces.find_map(|piece| {
        let (name, value) = piece.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("method") {
            return None;
        }
        [
            "REQUEST", "REPLY", "CANCEL", "PUBLISH", "COUNTER", "REFRESH",
        ]
        .into_iter()
        .find(|method| value.trim().trim_matches('"').eq_ignore_ascii_case(method))
    })
}

/// Base64 in lines of 76 characters, with CRLF between them.
fn base64_lines(data: &[u8]) -> Vec<u8> {
    let encoded = BASE64.encode(data);
    let mut lines = Vec::with_capacity(encoded.len() + encoded.len() / 38);
    for (index, line) in encoded.as_bytes().chunks(76).enumerate() {
        if index > 0 {
            lines.extend_from_slice(b"\r\n");
        }
        lines.extend_from_slice(line);
    }
    lines
}

/// Adds the file name to a part's `Content-Type` for mail programs that
/// look there and not at `Content-Disposition`, written the way they
/// expect: as it is when ASCII, as one RFC 2047 encoded word when not.
/// (RFC 2047 does not allow that in a parameter; every mail program does
/// it anyway and none reads the standard form here.) A name too long for
/// one encoded word is left to `Content-Disposition`, where RFC 2231
/// carries any name.
fn with_name<'x>(content_type: ContentType<'x>, name: &str) -> ContentType<'x> {
    if name.is_ascii() {
        if name.contains("=?") {
            return content_type;
        }
        return content_type.attribute("name", name.to_owned());
    }
    let word = encoded_words(name);
    if word.contains(' ') {
        content_type
    } else {
        content_type.attribute("name", word)
    }
}

/// A clean `type/subtype`, or `application/octet-stream` for anything that
/// is not one. Parameters are dropped. Types that describe structure
/// (`multipart/*`, and `message/*` other than a whole message) are not
/// attachments a user can make, and are sent as plain files too.
fn media_type(content_type: &str) -> String {
    const FALLBACK: &str = "application/octet-stream";
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let token = |text: &str| {
        !text.is_empty()
            && text.len() <= 100
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&byte))
    };
    match essence.split_once('/') {
        Some((kind, subtype)) if token(kind) && token(subtype) => {
            let structural = kind == "multipart" || (kind == "message" && subtype != "rfc822");
            if structural {
                FALLBACK.to_owned()
            } else {
                essence
            }
        }
        _ => FALLBACK.to_owned(),
    }
}

/// The longest line a message may have, not counting its CRLF.
const MAX_LINE: usize = 998;

/// A message as 7-bit lines ending in CRLF, or `None` when it is not
/// 7-bit: a byte outside ASCII, a NUL, a carriage return on its own, or a
/// line over the limit. A bare line feed is not a fault; it is how a
/// message is kept on disk, and it is written out as CRLF.
fn seven_bit_message(data: &[u8]) -> Option<Vec<u8>> {
    let mut lines = Vec::with_capacity(data.len() + data.len() / 32);
    let mut line_length = 0;
    let mut bytes = data.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        match byte {
            0 | 0x80.. => return None,
            b'\r' => {
                if bytes.next() != Some(b'\n') {
                    return None;
                }
                lines.extend_from_slice(b"\r\n");
                line_length = 0;
            }
            b'\n' => {
                lines.extend_from_slice(b"\r\n");
                line_length = 0;
            }
            _ => {
                line_length += 1;
                if line_length > MAX_LINE {
                    return None;
                }
                lines.push(byte);
            }
        }
    }
    Some(lines)
}

/// The part of the MIME boundaries that is particular to this message.
///
/// A boundary must not appear in what it separates. The ones built from
/// this start with "=_", which neither base64 nor quoted-printable can
/// produce, so only parts sent as they are could contain one: the token is
/// checked against all of them and changed until it appears in none. It is
/// made from the message, not from a clock or a random source, so the same
/// message always builds to the same bytes.
fn boundary_token(message: &Outgoing, message_id: &str, plain: &str, html: Option<&str>) -> String {
    let mut hash = fnv1a(&[
        message_id.as_bytes(),
        &message.date.to_le_bytes(),
        message.subject.as_bytes(),
        plain.as_bytes(),
    ]);
    let mut token = format!("{hash:016x}");
    // Sixteen hex digits turning up in the message by chance will not
    // happen once, let alone repeatedly; the bound only makes the loop
    // certain to end.
    for _ in 0..64 {
        let appears = |text: &[u8]| contains(text, token.as_bytes());
        let taken = appears(plain.as_bytes())
            || html.is_some_and(|html| appears(html.as_bytes()))
            || message
                .attachments
                .iter()
                .any(|attachment| appears(&attachment.data));
        if !taken {
            break;
        }
        hash = hash.wrapping_mul(FNV_PRIME).wrapping_add(1);
        token = format!("{hash:016x}");
    }
    token
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    let mut from = 0;
    while let Some(found) = haystack
        .get(from..)
        .and_then(|tail| tail.iter().position(|&byte| byte == first))
    {
        let start = from + found + 1;
        if haystack.get(start..start + rest.len()) == Some(rest) {
            return true;
        }
        from = start;
    }
    false
}

const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a over several pieces, with a separator so that moving a byte from
/// one piece to the next changes the result. Not a secret and not meant to
/// be one: it only has to differ from message to message.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &byte in part.iter().chain(&[0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attachment, Parsed, parse};
    use mail_parser::MessageParser;

    const DATE: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z
    const ID: &str = "<0123456789abcdef0123456789abcdef@example.com>";

    fn message(body: &str) -> Outgoing {
        Outgoing {
            from: Address::new("Robin Vale", "robin@example.com"),
            to: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Q4 roadmap".to_owned(),
            body: body.to_owned(),
            format: BodyFormat::Plain,
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
            date: DATE,
            message_id: ID.to_owned(),
        }
    }

    fn attachment(name: &str, content_type: &str, data: &[u8]) -> OutgoingAttachment {
        OutgoingAttachment {
            name: name.to_owned(),
            content_type: content_type.to_owned(),
            data: data.to_vec(),
        }
    }

    /// The message as text, once it has passed what every message must:
    /// ASCII only, CRLF at the end of every line and nowhere else, no line
    /// over the limit, and base64 and quoted-printable kept to 76.
    fn wire(raw: &[u8]) -> String {
        if let Some(at) = raw.iter().position(|&byte| byte >= 0x80 || byte == 0) {
            panic!("byte {:#04x} at {at}", raw[at]);
        }
        let text = String::from_utf8(raw.to_vec()).expect("ASCII");
        assert!(text.ends_with("\r\n"), "the last line is not ended");
        for line in text.split("\r\n") {
            assert!(
                !line.contains(['\r', '\n']),
                "a bare line break in {line:?}"
            );
            assert!(line.len() <= MAX_LINE, "a line of {} octets", line.len());
        }

        let parsed = MessageParser::default().parse(raw).expect("a message");
        for part in parsed.parts() {
            let encoding = part
                .content_transfer_encoding()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if encoding == "base64" || encoding == "quoted-printable" {
                for line in part.raw_body().split(|&byte| byte == b'\n') {
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    assert!(line.len() <= 76, "an encoded line of {}", line.len());
                }
            }
        }
        text
    }

    /// Builds, checks the bytes, and reads the result back.
    fn sent(message: &Outgoing) -> (String, Parsed) {
        let raw = build(message, 0);
        let text = wire(&raw);
        let parsed = parse(&raw, 0).expect("a message");
        (text, parsed)
    }

    /// The top-level headers, unfolded, as (name, value).
    fn headers(text: &str) -> Vec<(String, String)> {
        let block = text.split("\r\n\r\n").next().unwrap_or_default();
        let unfolded = block.replace("\r\n ", " ").replace("\r\n\t", "\t");
        unfolded
            .split("\r\n")
            .map(|line| {
                let (name, value) = line.split_once(':').expect("a header line");
                (name.to_owned(), value.trim_start().to_owned())
            })
            .collect()
    }

    /// The one header with this name.
    fn header(text: &str, name: &str) -> String {
        let found: Vec<String> = headers(text)
            .into_iter()
            .filter(|(known, _)| known == name)
            .map(|(_, value)| value)
            .collect();
        assert_eq!(found.len(), 1, "{name} headers: {found:?}");
        found.into_iter().next().unwrap()
    }

    fn header_names(text: &str) -> Vec<String> {
        headers(text).into_iter().map(|(name, _)| name).collect()
    }

    /// No header line is longer than RFC 5322 recommends.
    fn headers_fit(text: &str) {
        let block = text.split("\r\n\r\n").next().unwrap_or_default();
        for line in block.split("\r\n") {
            assert!(line.len() <= 78, "a header line of {}: {line}", line.len());
        }
    }

    /// The decoded contents of each attachment, read by the parser itself
    /// rather than through `parse`, which keeps only sizes.
    fn attached(raw: &[u8]) -> Vec<Vec<u8>> {
        let parsed = MessageParser::default().parse(raw).expect("a message");
        parsed
            .attachments()
            .map(|part| part.decoded().into_owned())
            .collect()
    }

    #[test]
    fn a_plain_message_byte_for_byte() {
        let outgoing = message("Hi Ana,\n\nHere is the draft.\n");
        let raw = build(&outgoing, 0);
        assert_eq!(
            wire(&raw),
            "From: \"Robin Vale\" <robin@example.com>\r\n\
             To: \"Ana Ruiz\" <ana.ruiz@example.com>\r\n\
             Subject: Q4 roadmap\r\n\
             Date: Sat, 14 Nov 2026 09:30:00 +0000\r\n\
             Message-ID: <0123456789abcdef0123456789abcdef@example.com>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=\"utf-8\"\r\n\
             Content-Transfer-Encoding: 7bit\r\n\
             \r\n\
             Hi Ana,\r\n\
             \r\n\
             Here is the draft.\r\n"
        );

        let parsed = parse(&raw, 0).expect("a message");
        assert_eq!(parsed.headers.from, std::slice::from_ref(&outgoing.from));
        assert_eq!(parsed.headers.to, outgoing.to);
        assert!(parsed.headers.cc.is_empty());
        assert_eq!(parsed.headers.subject, "Q4 roadmap");
        assert_eq!(parsed.headers.date, DATE);
        assert_eq!(parsed.headers.message_id.as_deref(), Some(ID));
        assert_eq!(parsed.body, "Hi Ana,\n\nHere is the draft.\n");
        assert_eq!(parsed.html, None);
        assert!(parsed.has_plain && parsed.attachments.is_empty());
        assert_eq!(parsed.in_reply_to, None);
        assert!(parsed.references.is_empty() && parsed.reply_to.is_empty());
    }

    #[test]
    fn the_same_message_builds_to_the_same_bytes() {
        let mut outgoing = message("Hi *Ana*.\n");
        outgoing.format = BodyFormat::Markdown;
        outgoing
            .attachments
            .push(attachment("notes.txt", "text/plain", b"notes\n"));
        assert_eq!(build(&outgoing, 3600), build(&outgoing, 3600));
    }

    #[test]
    fn it_says_nothing_about_the_program_or_the_machine() {
        let mut outgoing = message("Hi *Ana*.\n");
        outgoing.format = BodyFormat::Markdown;
        outgoing
            .attachments
            .push(attachment("notes.txt", "text/plain", b"notes\n"));
        let (text, _) = sent(&outgoing);
        assert_eq!(
            header_names(&text),
            [
                "From",
                "To",
                "Subject",
                "Date",
                "Message-ID",
                "MIME-Version",
                "Content-Type"
            ]
        );
        let lower = text.to_ascii_lowercase();
        for word in [
            "user-agent",
            "x-mailer",
            "quill",
            "darwin",
            "macos",
            "localhost",
        ] {
            assert!(!lower.contains(word), "{word} in {text}");
        }
    }

    #[test]
    fn bcc_is_in_the_message_and_everyone_is_in_the_envelope_once() {
        let mut outgoing = message("Hello\n");
        outgoing
            .to
            .push(Address::new("", "lena.fischer@example.com"));
        outgoing.cc = vec![
            Address::new("Haddad, Omar", "omar.haddad@example.com"),
            Address::new("Ana again", "ana.ruiz@EXAMPLE.com"),
        ];
        outgoing.bcc = vec![
            Address::new("Jo Park", "jo.park@example.com"),
            Address::new("", "lena.fischer@example.com"),
            Address::new("", "  "),
        ];
        let (text, parsed) = sent(&outgoing);
        assert_eq!(
            header(&text, "Bcc"),
            "\"Jo Park\" <jo.park@example.com>, <lena.fischer@example.com>"
        );
        assert_eq!(parsed.headers.to, outgoing.to);
        assert_eq!(parsed.headers.cc, outgoing.cc);

        assert_eq!(
            envelope(&outgoing),
            Envelope {
                from: "robin@example.com".to_owned(),
                to: vec![
                    "ana.ruiz@example.com".to_owned(),
                    "lena.fischer@example.com".to_owned(),
                    "omar.haddad@example.com".to_owned(),
                    "jo.park@example.com".to_owned(),
                ],
            }
        );
    }

    #[test]
    fn a_mailbox_name_in_another_case_is_another_recipient() {
        let mut outgoing = message("Hello\n");
        outgoing.cc = vec![Address::new("", "Ana.Ruiz@example.com")];
        assert_eq!(
            envelope(&outgoing).to,
            ["ana.ruiz@example.com", "Ana.Ruiz@example.com"]
        );
    }

    #[test]
    fn a_message_to_bcc_alone_says_its_recipients_are_not_listed() {
        let mut outgoing = message("Hello\n");
        outgoing.bcc = std::mem::take(&mut outgoing.to);
        let (text, parsed) = sent(&outgoing);
        assert_eq!(header(&text, "To"), "undisclosed-recipients:;");
        assert_eq!(header(&text, "Bcc"), "\"Ana Ruiz\" <ana.ruiz@example.com>");
        assert!(parsed.headers.to.is_empty());
        assert_eq!(envelope(&outgoing).to, ["ana.ruiz@example.com"]);
    }

    #[test]
    fn text_outside_ascii_arrives_as_written() {
        let body = "Hej Åsa 👋🏽 👨‍👩‍👧‍👦\n\
                    日本語のテキストと中文文本。\n\
                    Combining: e\u{301} a\u{308}\u{304} and Zalgo t\u{336}\u{35b}e\u{334}xt\n\
                    Right to left: שלום, مرحبا\n\
                    \tA tab, a no-break\u{a0}space and a zero\u{200b}width one.\n";
        for format in [BodyFormat::Plain, BodyFormat::Markdown] {
            let mut outgoing = message(body);
            outgoing.format = format;
            let (_, parsed) = sent(&outgoing);
            assert_eq!(parsed.body, body);
        }
        // Mostly not ASCII: base64 is the shorter encoding and is chosen.
        let cjk = "日本語のテキスト。".repeat(40) + "\n";
        let (text, parsed) = sent(&message(&cjk));
        assert_eq!(parsed.body, cjk);
        assert_eq!(header(&text, "Content-Transfer-Encoding"), "base64");
        // Mostly ASCII: quoted-printable, which stays readable.
        let (text, parsed) = sent(&message("Tomás will bring the café's keys.\n"));
        assert_eq!(parsed.body, "Tomás will bring the café's keys.\n");
        assert_eq!(
            header(&text, "Content-Transfer-Encoding"),
            "quoted-printable"
        );
    }

    #[test]
    fn a_very_long_line_is_wrapped_for_the_wire_and_whole_on_arrival() {
        let long = "x".repeat(5_000);
        let body = format!("before\n{long}\nafter\n");
        for format in [BodyFormat::Plain, BodyFormat::Markdown] {
            let mut outgoing = message(&body);
            outgoing.format = format;
            let (text, parsed) = sent(&outgoing);
            assert_eq!(parsed.body, body);
            assert!(text.split("\r\n").all(|line| line.len() <= 78));
            if let Some(html) = parsed.html {
                assert!(html.contains(&long));
            }
        }
    }

    #[test]
    fn lines_a_mail_system_might_take_for_its_own_arrive_unchanged() {
        let body = ".\n\
                    .a line starting with a dot\n\
                    ..two dots\n\
                    From here on it is fine.\n\
                    From robin@example.com Sat Nov 14 09:30:00 2026\n\
                    >From the top\n\
                    -- \n\
                    Robin\n\
                    --=_alt_0000000000000000\n\
                    trailing spaces   \n\
                    =3D is not an escape and neither is =\n";
        for format in [BodyFormat::Plain, BodyFormat::Markdown] {
            let mut outgoing = message(body);
            outgoing.format = format;
            let (_, parsed) = sent(&outgoing);
            assert_eq!(parsed.body, body);
        }
    }

    #[test]
    fn the_body_gets_its_line_endings_and_loses_its_control_characters() {
        let (text, parsed) = sent(&message("one\r\ntwo\rthree\nfour\x00\x1b[2J\x07\tfive"));
        assert_eq!(parsed.body, "one\ntwo\nthree\nfour[2J\tfive\n");
        assert!(text.ends_with("four[2J\tfive\r\n"));

        let (text, parsed) = sent(&message(""));
        assert_eq!(parsed.body, "");
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn subjects_survive() {
        let long_words = "A subject that goes on for a good while longer than any line. ".repeat(5);
        let long_words = long_words.trim();
        assert!(long_words.len() > 300);
        let unbroken = "x".repeat(300);
        let cases = [
            "Q4 roadmap",
            "Lunch 🍜 on Friday? 🎉",
            "Tomás sent an invoice ✎",
            "日本語の件名はこのように長くなることがありますがそれでも正しく届く必要があります",
            long_words,
            unbroken.as_str(),
            "Två  spaces and (parentheses), \"quotes\" and a \\ backslash; a: colon",
            "=?utf-8?Q?not_an_encoded_word?=",
            "50% off =?really?= and =?utf-8?B?8J+YgA==?= too",
            "a subject with a trailing encoded-word look-alike =?",
            "Typed with two spaces after each full stop.  As some people do.  And long \
             enough to need folding.  Twice.",
            "Short  with  two  spaces",
        ];
        for subject in cases {
            let mut outgoing = message("x\n");
            outgoing.subject = subject.to_owned();
            let (text, parsed) = sent(&outgoing);
            assert_eq!(parsed.headers.subject, subject);
            if subject != unbroken {
                headers_fit(&text);
            }
        }

        // Plain ASCII is folded, not encoded: it stays readable in the raw
        // message.
        let mut outgoing = message("x\n");
        outgoing.subject = long_words.to_owned();
        let text = wire(&build(&outgoing, 0));
        assert!(!text.contains("=?"), "{text}");
        assert_eq!(header(&text, "Subject"), long_words);
    }

    #[test]
    fn display_names_come_back_as_they_were() {
        let long = "Dr. ".to_owned() + &"Wolfeschlegelstein ".repeat(12) + "Sr.";
        let names = [
            "Ana Ruiz",
            "Haddad, Omar",
            "Omar \"the boss\" Haddad",
            "back\\slash",
            "O'Brien; Pat: (accounts) [west] <not@an.address>",
            "a.b@c",
            "陈静",
            "Åsa Öberg",
            "José Álvarez-Núñez, PhD",
            "Zoë \"Z\" (work) <zoe@example.org>, and others",
            "🎉 Party Committee 🎉",
            "Two  Spaces  Between  Each  Word  Of  A  Name  Long  Enough  To  Need  Folding  Twice",
            long.as_str(),
        ];
        let mut outgoing = message("x\n");
        outgoing.to = names
            .iter()
            .enumerate()
            .map(|(index, name)| Address::new(name, &format!("person{index}@example.com")))
            .collect();
        outgoing.cc = outgoing.to.clone();
        outgoing.from = Address::new("Vale, Robin \"R\" 🖋", "robin@example.com");
        let (text, parsed) = sent(&outgoing);
        assert_eq!(parsed.headers.to, outgoing.to);
        assert_eq!(parsed.headers.cc, outgoing.cc);
        assert_eq!(parsed.headers.from, std::slice::from_ref(&outgoing.from));
        headers_fit(&text);
        assert_eq!(envelope(&outgoing).to.len(), names.len());
    }

    #[test]
    fn long_runs_of_spaces_do_not_make_long_lines() {
        let gap = " ".repeat(1_500);
        let spaced = format!("left{gap}right");
        let mut outgoing = message(&format!("{spaced}\n"));
        outgoing.subject = spaced.clone();
        outgoing.to = vec![Address::new(&spaced, "ana.ruiz@example.com")];
        outgoing.attachments = vec![attachment(&spaced, "text/plain", b"x")];
        let (_, parsed) = sent(&outgoing);
        assert_eq!(parsed.headers.subject, spaced);
        assert_eq!(parsed.headers.to, outgoing.to);
        assert_eq!(parsed.body, format!("{spaced}\n"));
        assert_eq!(parsed.attachments[0].name.as_deref(), Some(spaced.as_str()));
    }

    const MARKDOWN: &str = "# Plan\n\
        \n\
        Hi **Ana**, see [the agenda](https://example.com/agenda?day=1&room=2) and *reply*\n\
        by Friday.\n\
        \n\
        - tea\n\
        - coffee\n\
        \n\
        1. one\n\
        2. two\n\
        \n\
        ```\n\
        if a < b { run(\"x\") }\n\
        ```\n\
        \n\
        On Friday, Ana wrote:\n\
        > quoted line\n\
        > second line\n\
        \n\
        <script>alert(1)</script>\n\
        \n\
        Inline <b onclick=\"x()\">bold</b> and <img src=\"https://example.com/p.gif\"> too.\n";

    #[test]
    fn markdown_goes_out_as_text_and_html() {
        let mut outgoing = message(MARKDOWN);
        outgoing.format = BodyFormat::Markdown;
        let (text, parsed) = sent(&outgoing);

        // The text part is what was typed.
        assert_eq!(parsed.body, MARKDOWN);
        assert!(parsed.has_plain && parsed.attachments.is_empty());

        // Plain first, HTML second: a reader shows the last one it can.
        assert!(
            header(&text, "Content-Type").starts_with("multipart/alternative; boundary=\"=_alt_")
        );
        let plain_at = text
            .find("Content-Type: text/plain; charset=\"utf-8\"")
            .expect("plain");
        let html_at = text
            .find("Content-Type: text/html; charset=\"utf-8\"")
            .expect("html");
        assert!(plain_at < html_at);

        let html = parsed.html.expect("an HTML part");
        for expected in [
            "<h1>Plan</h1>",
            "Hi <strong>Ana</strong>, see \
             <a href=\"https://example.com/agenda?day=1&amp;room=2\">the agenda</a> and \
             <em>reply</em><br>\nby Friday.",
            "<ul>\n<li>tea</li>\n<li>coffee</li>\n</ul>",
            "<ol>\n<li>one</li>\n<li>two</li>\n</ol>",
            "<code>if a &lt; b { run(&quot;x&quot;) }\n</code></pre>",
            "<p>On Friday, Ana wrote:</p>\n<blockquote style=\"",
            "<p>quoted line<br>\nsecond line</p>\n</blockquote>",
            // What was typed as HTML is shown as text.
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>",
            "Inline &lt;b onclick=&quot;x()&quot;&gt;bold&lt;/b&gt; and \
             &lt;img src=&quot;https://example.com/p.gif&quot;&gt; too.",
        ] {
            assert!(html.contains(expected), "no {expected:?} in {html}");
        }
        let lower = html.to_ascii_lowercase();
        for live in [
            "<script",
            "<b ",
            "<b>",
            "<img",
            "onclick=\"",
            "<style",
            "<link",
            "src=\"",
        ] {
            assert!(!lower.contains(live), "{live:?} in {html}");
        }

        // The reader's own rendering of the HTML has the words and not the
        // markup, and nothing to fetch.
        let rendered = crate::html::to_text(&html).expect("text");
        assert!(rendered.contains("<script>alert(1)</script>"), "{rendered}");
        assert!(rendered.contains("Plan") && rendered.contains("quoted line"));
    }

    #[test]
    fn plain_text_goes_out_alone() {
        let (text, parsed) = sent(&message(MARKDOWN));
        assert_eq!(
            header(&text, "Content-Type"),
            "text/plain; charset=\"utf-8\""
        );
        assert!(!text.contains("multipart"));
        assert_eq!(parsed.body, MARKDOWN);
        assert_eq!(parsed.html, None);
    }

    #[test]
    fn attachments_arrive_byte_for_byte() {
        let every_byte: Vec<u8> = (0..=255).collect();
        let notes = b"line one\nline two  \n\nFrom here\n.\nno final newline".to_vec();
        let crlf_notes = b"line one\r\nline two\r\n".to_vec();
        let latin1 = b"caf\xe9\n".to_vec();
        let mut outgoing = message("The files are attached.\n");
        outgoing.attachments = vec![
            attachment("notes.txt", "text/plain", &notes),
            attachment("every byte.bin", "application/octet-stream", &every_byte),
            attachment("dos.txt", "text/plain", &crlf_notes),
            attachment("old.txt", "Text/Plain; charset=iso-8859-1", &latin1),
            attachment("empty.dat", "application/x-empty", b""),
            attachment("data.csv", "text/csv", "naïve,1\n".as_bytes()),
        ];
        for format in [BodyFormat::Plain, BodyFormat::Markdown] {
            outgoing.format = format;
            let raw = build(&outgoing, 0);
            let text = wire(&raw);
            let parsed = parse(&raw, 0).expect("a message");

            assert!(
                header(&text, "Content-Type").starts_with("multipart/mixed; boundary=\"=_mix_")
            );
            assert_eq!(parsed.body, "The files are attached.\n");
            assert_eq!(parsed.html.is_some(), format == BodyFormat::Markdown);
            let listed = |name: &'static str, content_type: &'static str, size: usize| {
                (Some(name), content_type, size)
            };
            let files: Vec<_> = parsed.attachments.iter().map(Attachment::summary).collect();
            assert_eq!(
                files,
                [
                    listed("notes.txt", "text/plain", notes.len()),
                    listed("every byte.bin", "application/octet-stream", 256),
                    listed("dos.txt", "text/plain", crlf_notes.len()),
                    listed("old.txt", "text/plain", latin1.len()),
                    listed("empty.dat", "application/x-empty", 0),
                    listed("data.csv", "text/csv", 9),
                ]
            );
            assert_eq!(
                attached(&raw),
                [
                    notes.clone(),
                    every_byte.clone(),
                    crlf_notes.clone(),
                    latin1.clone(),
                    Vec::new(),
                    "naïve,1\n".as_bytes().to_vec(),
                ]
            );
        }

        let text = wire(&build(&outgoing, 0));
        // UTF-8 text says so; text that is not UTF-8 claims nothing.
        assert!(text.contains("Content-Type: text/plain; charset=\"utf-8\"; name=\"notes.txt\""));
        assert!(text.contains("Content-Type: text/plain; name=\"old.txt\""));
        assert!(text.contains("Content-Disposition: attachment; filename=\"every byte.bin\""));
    }

    #[test]
    fn a_two_megabyte_attachment() {
        let data: Vec<u8> = (0..2 * 1024 * 1024_u32)
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let mut outgoing = message("See the archive.\n");
        outgoing.attachments = vec![attachment("archive.zip", "application/zip", &data)];
        let raw = build(&outgoing, 0);
        let text = wire(&raw);
        assert!(text.contains("Content-Transfer-Encoding: base64"));
        // Base64 costs a third, plus line endings, and little else.
        assert!(raw.len() < data.len() / 3 * 4 + data.len() / 19 + 2_000);

        let parsed = parse(&raw, 0).expect("a message");
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].size, data.len());
        assert!(attached(&raw)[0] == data);
    }

    /// A parameter written per RFC 2231, read without the parser: either
    /// `key*=UTF-8''...` or the sections `key*0*=UTF-8''...; key*1*=...`.
    fn extended_parameter(header: &str, key: &str) -> Option<String> {
        let section = |marker: &str| {
            let start = header.find(marker)? + marker.len();
            let rest = &header[start..];
            Some(rest.split(';').next().unwrap_or(rest).trim().to_owned())
        };
        let mut encoded = String::new();
        if let Some(whole) = section(&format!("{key}*=")) {
            encoded = whole;
        } else {
            for index in 0.. {
                match section(&format!("{key}*{index}*=")) {
                    Some(part) => encoded.push_str(&part),
                    None => break,
                }
            }
        }
        let encoded = encoded.strip_prefix("UTF-8''")?;
        let mut bytes = Vec::new();
        let mut rest = encoded.as_bytes();
        while let Some((&byte, tail)) = rest.split_first() {
            if byte == b'%' {
                let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                rest = &tail[2..];
            } else {
                bytes.push(byte);
                rest = tail;
            }
        }
        String::from_utf8(bytes).ok()
    }

    /// The unfolded header `name` of the part whose headers mention `marker`.
    fn part_header(text: &str, marker: &str, name: &str) -> String {
        let block = text
            .split("\r\n\r\n")
            .find(|block| block.contains(marker))
            .expect("a part with the marker");
        let unfolded = block.replace("\r\n ", " ").replace("\r\n\t", "\t");
        unfolded
            .split("\r\n")
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .expect("the header")
            .to_owned()
    }

    #[test]
    fn file_names_outside_ascii_survive() {
        let short = "Jahresabschluß – Übersicht.pdf";
        let long = "Réunion d'équipe : résumé des décisions du trimestre et prochaines étapes \
                    du projet (version finale, relue) 会議メモ 📎.pdf";
        let quoted = "the \"final\" report; v2\\draft.pdf";
        let mut outgoing = message("Attached.\n");
        outgoing.attachments = vec![
            attachment(short, "application/pdf", b"%PDF-1.4\n"),
            attachment(long, "application/x-long", b"%PDF-1.4\n"),
            attachment(quoted, "application/x-quoted", b"%PDF-1.4\n"),
        ];
        let (text, parsed) = sent(&outgoing);
        let names: Vec<&str> = parsed
            .attachments
            .iter()
            .filter_map(|attachment| attachment.name.as_deref())
            .collect();
        assert_eq!(names, [short, long, quoted]);

        // The same, without trusting the parser. Content-Disposition
        // carries the name per RFC 2231...
        let disposition = part_header(&text, "application/pdf", "Content-Disposition");
        assert!(
            disposition.starts_with("attachment; filename*=UTF-8''Jahresabschlu%C3%9F%20%E2%80%93"),
            "{disposition}"
        );
        assert_eq!(
            extended_parameter(&disposition, "filename").as_deref(),
            Some(short)
        );
        // ...and Content-Type as an encoded word, for programs that look
        // there.
        let content_type = part_header(&text, "application/pdf", "Content-Type");
        let word = content_type
            .strip_prefix("application/pdf; name=\"=?utf-8?B?")
            .and_then(|rest| rest.strip_suffix("?=\""))
            .expect("an encoded word");
        assert_eq!(BASE64.decode(word).expect("base64"), short.as_bytes());

        // A long name is split into sections, and is left out of
        // Content-Type, where it would not fit one encoded word.
        let disposition = part_header(&text, "application/x-long", "Content-Disposition");
        assert!(
            disposition.contains("filename*0*=UTF-8''R%C3%A9union"),
            "{disposition}"
        );
        assert_eq!(
            extended_parameter(&disposition, "filename").as_deref(),
            Some(long)
        );
        assert_eq!(
            part_header(&text, "application/x-long", "Content-Type"),
            "application/x-long"
        );

        let disposition = part_header(&text, "application/x-quoted", "Content-Disposition");
        assert_eq!(
            disposition,
            "attachment; filename=\"the \\\"final\\\" report; v2\\\\draft.pdf\""
        );
        assert!(text.split("\r\n").all(|line| line.len() <= 78));
    }

    const INNER: &str = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
        To: Robin Vale <robin@example.com>\r\n\
        Subject: Contract\r\n\
        Message-ID: <contract.1@example.com>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"inner\"\r\n\
        \r\n\
        --inner\r\n\
        Content-Type: text/plain\r\n\
        \r\n\
        The contract is attached.\r\n\
        --inner\r\n\
        Content-Type: application/pdf; name=\"contract.pdf\"\r\n\
        Content-Transfer-Encoding: base64\r\n\
        \r\n\
        JVBERi0xLjQK\r\n\
        --inner--\r\n";

    #[test]
    fn a_forwarded_message_travels_inside_as_it_is() {
        let mut outgoing = message("See the message attached.\n");
        outgoing.subject = "Fwd: Contract".to_owned();
        outgoing.format = BodyFormat::Markdown;
        outgoing.attachments = vec![attachment(
            "Contract.eml",
            "message/rfc822",
            INNER.as_bytes(),
        )];
        let raw = build(&outgoing, 0);
        let text = wire(&raw);

        // Not base64, not quoted-printable: the message itself.
        assert_eq!(
            part_header(&text, "message/rfc822", "Content-Transfer-Encoding"),
            "7bit"
        );
        assert_eq!(
            part_header(&text, "message/rfc822", "Content-Disposition"),
            "attachment; filename=\"Contract.eml\""
        );
        assert!(
            text.contains(&format!("\r\n\r\n{INNER}\r\n--=_mix_")),
            "{text}"
        );

        let parsed = parse(&raw, 0).expect("a message");
        assert_eq!(parsed.body, "See the message attached.\n");
        let files: Vec<_> = parsed.attachments.iter().map(Attachment::summary).collect();
        assert_eq!(
            files,
            [(Some("Contract.eml"), "message/rfc822", INNER.len())]
        );
        // The parser reads the message inside as a message.
        let outer = MessageParser::default().parse(&raw[..]).expect("a message");
        let inner = outer
            .attachments()
            .next()
            .and_then(|part| part.nested())
            .expect("a nested message");
        assert_eq!(inner.subject(), Some("Contract"));
        assert_eq!(inner.attachments().count(), 1);
    }

    #[test]
    fn a_forwarded_message_kept_with_bare_line_feeds_gets_its_crlf() {
        let stored = INNER.replace("\r\n", "\n");
        let mut outgoing = message("See attached.\n");
        outgoing.attachments = vec![attachment("", "message/rfc822", stored.as_bytes())];
        let raw = build(&outgoing, 0);
        let text = wire(&raw);
        assert!(text.contains(&format!("\r\n\r\n{INNER}\r\n--=_mix_")));
        assert_eq!(
            part_header(&text, "message/rfc822", "Content-Disposition"),
            "attachment; filename=\"message.eml\""
        );
    }

    #[test]
    fn a_forwarded_message_that_is_not_seven_bit_travels_as_a_file() {
        let long_line = format!("Subject: x\r\n\r\n{}\r\n", "y".repeat(1_200));
        let cases: [(&str, Vec<u8>); 4] = [
            (
                "eight bit",
                "Subject: Caf\u{e9}\r\nContent-Type: text/plain; charset=utf-8\r\n\
                 Content-Transfer-Encoding: 8bit\r\n\r\nTomás was here.\r\n"
                    .as_bytes()
                    .to_vec(),
            ),
            ("a line too long", long_line.into_bytes()),
            (
                "a bare carriage return",
                b"Subject: x\r\n\r\none\rtwo\r\n".to_vec(),
            ),
            ("a NUL", b"Subject: x\r\n\r\none\x00two\r\n".to_vec()),
        ];
        for (what, inner) in cases {
            let mut outgoing = message("See attached.\n");
            outgoing.attachments = vec![attachment("Original", "message/rfc822", &inner)];
            let raw = build(&outgoing, 0);
            let text = wire(&raw);
            assert!(!text.contains("message/rfc822"), "{what}");
            assert_eq!(
                part_header(
                    &text,
                    "application/octet-stream",
                    "Content-Transfer-Encoding"
                ),
                "base64",
                "{what}"
            );
            let parsed = parse(&raw, 0).expect("a message");
            let files: Vec<_> = parsed.attachments.iter().map(Attachment::summary).collect();
            assert_eq!(
                files,
                [(
                    Some("Original.eml"),
                    "application/octet-stream",
                    inner.len()
                )],
                "{what}"
            );
            // Not a byte of it changed.
            assert_eq!(attached(&raw), [inner], "{what}");
        }
    }

    #[test]
    fn a_boundary_never_appears_in_what_it_separates() {
        let token_of = |text: &str| {
            let at = text.find("boundary=\"=_mix_").expect("a boundary") + 16;
            text[at..at + 16].to_owned()
        };
        let mut outgoing = message("See attached.\n");
        outgoing.attachments = vec![attachment("a.eml", "message/rfc822", INNER.as_bytes())];
        let first = token_of(&wire(&build(&outgoing, 0)));

        // An attachment that happens to hold the very line that would end
        // the message early.
        let trap = format!("Subject: trap\r\n\r\n--=_mix_{first}--\r\nhidden\r\n");
        outgoing.attachments = vec![attachment("a.eml", "message/rfc822", trap.as_bytes())];
        let raw = build(&outgoing, 0);
        let text = wire(&raw);
        assert_ne!(token_of(&text), first);
        assert!(text.contains(&trap));
        let parsed = parse(&raw, 0).expect("a message");
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].size, trap.len());
    }

    #[test]
    fn odd_media_types_are_sent_as_plain_files() {
        for (given, sent_as) in [
            ("application/pdf", "application/pdf"),
            ("IMAGE/PNG", "image/png"),
            ("text/html; charset=utf-8", "text/html"),
            (
                "application/vnd.ms-excel.sheet.macroenabled.12",
                "application/vnd.ms-excel.sheet.macroenabled.12",
            ),
            ("image/svg+xml", "image/svg+xml"),
            ("", "application/octet-stream"),
            ("pdf", "application/octet-stream"),
            ("application/", "application/octet-stream"),
            ("application/pdf/extra", "application/octet-stream"),
            ("application/p df", "application/octet-stream"),
            (
                "application/pdf\r\nX-Injected: 1",
                "application/octet-stream",
            ),
            ("multipart/mixed", "application/octet-stream"),
            ("multipart/mixed; boundary=x", "application/octet-stream"),
            ("message/partial", "application/octet-stream"),
            ("message/external-body", "application/octet-stream"),
            ("applicätion/pdf", "application/octet-stream"),
        ] {
            assert_eq!(media_type(given), sent_as, "{given:?}");
        }
    }

    #[test]
    fn a_calendar_parts_method_is_the_one_parameter_carried_over() {
        assert_eq!(
            calendar_method("text/calendar; method=REPLY; charset=utf-8"),
            Some("REPLY")
        );
        assert_eq!(
            calendar_method("Text/Calendar;METHOD=\"request\""),
            Some("REQUEST")
        );
        for other in [
            "text/calendar",
            "text/calendar; method=EVIL\r\nBcc: x",
            "text/plain; method=REPLY",
            "application/pdf",
        ] {
            assert_eq!(calendar_method(other), None, "{other}");
        }
    }

    #[test]
    fn nothing_typed_into_a_field_becomes_a_header() {
        const BREAKS: [&str; 5] = ["\r\n", "\n", "\r", "\r\n ", "\u{2028}"];
        for line_break in BREAKS {
            let inject = |text: &str| {
                format!(
                    "{text}{line_break}X-Injected: yes{line_break}Bcc: evil@example.com{line_break}{line_break}<script>"
                )
            };
            let mut outgoing = message("Hello\n");
            outgoing.from = Address::new(&inject("Robin Vale"), &inject("robin@example.com"));
            outgoing.to = vec![Address::new(
                &inject("Ana Ruiz"),
                &inject("ana.ruiz@example.com"),
            )];
            outgoing.cc = vec![Address::new(
                &inject("Lena"),
                &inject("lena.fischer@example.com"),
            )];
            outgoing.bcc = vec![Address::new(&inject("Jo"), &inject("jo.park@example.com"))];
            outgoing.subject = inject("Q4 roadmap");
            outgoing.message_id = inject(ID);
            outgoing.in_reply_to = Some(inject("<a@example.com>"));
            outgoing.references = vec![inject("<a@example.com>"), "<b@example.com>".to_owned()];
            outgoing.attachments = vec![
                attachment(&inject("notes.txt"), &inject("text/plain"), b"notes\n"),
                attachment(&inject("fwd.eml"), "message/rfc822", INNER.as_bytes()),
            ];

            for format in [BodyFormat::Plain, BodyFormat::Markdown] {
                outgoing.format = format;
                let raw = build(&outgoing, 0);
                let text = wire(&raw);

                // The headers are the ones a message has, each once.
                assert_eq!(
                    header_names(&text),
                    [
                        "From",
                        "To",
                        "Cc",
                        "Bcc",
                        "Subject",
                        "Date",
                        "Message-ID",
                        "References",
                        "MIME-Version",
                        "Content-Type"
                    ],
                    "{line_break:?}"
                );
                // Outside the forwarded message, no line anywhere starts
                // with what was injected.
                let outside = text.replace(INNER, "");
                for line in outside.split("\r\n") {
                    assert!(!line.starts_with("X-Injected"), "{line_break:?}: {line}");
                    assert!(!line.starts_with("Bcc: evil"), "{line_break:?}: {line}");
                    assert!(!line.starts_with("<script>"), "{line_break:?}: {line}");
                }

                let parsed = parse(&raw, 0).expect("a message");
                assert_eq!(parsed.headers.from.len(), 1);
                assert_eq!(parsed.headers.to.len(), 1);
                assert_eq!(parsed.headers.cc.len(), 1);
                assert!(
                    parsed
                        .headers
                        .subject
                        .starts_with("Q4 roadmap X-Injected: yes")
                );
                assert!(!parsed.headers.subject.contains(['\r', '\n']));
                assert_eq!(parsed.in_reply_to, None);
                assert_eq!(parsed.references, ["<b@example.com>"]);
                assert_eq!(parsed.body, "Hello\n");
                assert_eq!(parsed.attachments.len(), 2);
                assert_eq!(
                    parsed.attachments[0].content_type,
                    "application/octet-stream"
                );
                assert_eq!(parsed.attachments[1].content_type, "message/rfc822");
                for attachment in &parsed.attachments {
                    let name = attachment.name.as_deref().expect("a name");
                    assert!(name.contains("X-Injected: yes"), "{name:?}");
                    assert!(!name.contains(['\r', '\n']), "{name:?}");
                }

                // What stands for an address in the header is what is in
                // the envelope: one address each, and nobody new.
                let wire_envelope = envelope(&outgoing);
                assert_eq!(wire_envelope.from, parsed.headers.from[0].email);
                assert_eq!(wire_envelope.to.len(), 3);
                assert_eq!(wire_envelope.to[0], parsed.headers.to[0].email);
                assert_eq!(wire_envelope.to[1], parsed.headers.cc[0].email);
                for address in wire_envelope.to.iter().chain([&wire_envelope.from]) {
                    assert!(
                        !address.contains(|c: char| c.is_control() || c.is_whitespace()),
                        "{address:?}"
                    );
                    assert_ne!(address, "evil@example.com");
                }
            }
        }
    }

    #[test]
    fn one_address_cannot_pass_for_two() {
        let mut outgoing = message("Hello\n");
        outgoing.to = vec![
            Address::new("", "ana.ruiz@example.com>, <evil@example.com"),
            Address::new("", "lena.fischer@example.com, evil@example.com"),
            Address::new("", "<omar.haddad@example.com>"),
            Address::new("Evil <evil@example.com>, Jo", "jo.park@example.com"),
            Address::new("", ""),
            Address::new("Nobody", " \r\n"),
            Address::new("", &format!("{}@example.com", "a".repeat(400))),
        ];
        let (_, parsed) = sent(&outgoing);
        assert_eq!(parsed.headers.to.len(), 4);
        assert_eq!(parsed.headers.to[2].email, "omar.haddad@example.com");
        assert_eq!(
            parsed.headers.to[3],
            Address::new("Evil <evil@example.com>, Jo", "jo.park@example.com")
        );
        let wire_envelope = envelope(&outgoing);
        assert_eq!(wire_envelope.to.len(), 4);
        for (header, address) in parsed.headers.to.iter().zip(&wire_envelope.to) {
            assert_eq!(&header.email, address);
            assert_ne!(address, "evil@example.com");
        }
    }

    #[test]
    fn threading_headers_are_written_exactly() {
        let mut outgoing = message("Agreed.\n");
        outgoing.in_reply_to = Some("<roadmap.3@example.com>".to_owned());
        outgoing.references = vec![
            "<roadmap.1@example.com>".to_owned(),
            "<roadmap.2@example.com>".to_owned(),
            "<roadmap.3@example.com>".to_owned(),
        ];
        let (text, parsed) = sent(&outgoing);
        assert!(
            text.contains("\r\nMessage-ID: <0123456789abcdef0123456789abcdef@example.com>\r\n")
        );
        assert!(text.contains("\r\nIn-Reply-To: <roadmap.3@example.com>\r\n"));
        assert!(text.contains(
            "\r\nReferences: <roadmap.1@example.com> <roadmap.2@example.com>\r\n \
             <roadmap.3@example.com>\r\n"
        ));
        assert_eq!(parsed.in_reply_to, outgoing.in_reply_to);
        assert_eq!(parsed.references, outgoing.references);

        // A long conversation folds between identifiers and loses none.
        outgoing.references = (0..200)
            .map(|index| format!("<conversation.{index}.0123456789abcdef@mail.example.com>"))
            .collect();
        let (text, parsed) = sent(&outgoing);
        headers_fit(&text);
        assert_eq!(parsed.references, outgoing.references);

        // Identifiers that could not be written are left out whole.
        outgoing.in_reply_to = Some("<has a space@example.com>".to_owned());
        outgoing.references = vec![
            "<good.1@example.com>".to_owned(),
            "<caf\u{e9}@example.com>".to_owned(),
            "<a@example.com> <b@example.com>".to_owned(),
            String::new(),
            "<>".to_owned(),
            format!("<{}@example.com>", "x".repeat(2_000)),
            "bare.but.fine@example.com".to_owned(),
        ];
        let (text, parsed) = sent(&outgoing);
        assert!(!header_names(&text).contains(&"In-Reply-To".to_owned()));
        assert_eq!(
            header(&text, "References"),
            "<good.1@example.com> <bare.but.fine@example.com>"
        );
        assert_eq!(parsed.in_reply_to, None);
    }

    #[test]
    fn a_reply_built_from_a_draft_threads_under_the_original() {
        let original = parse(
            b"From: Ana Ruiz <ana.ruiz@example.com>\r\n\
              To: robin@example.com\r\n\
              Subject: Q4 roadmap\r\n\
              Message-ID: <roadmap.2@example.com>\r\n\
              References: <roadmap.1@example.com>\r\n\
              \r\n\
              Here is the draft.\r\n",
            DATE,
        )
        .expect("a message");
        let draft = crate::reply(
            &original,
            false,
            &["robin@example.com".to_owned()],
            crate::ReplyPosition::Below,
            "Sat, 14 Nov 2026 at 09:14",
        );
        let outgoing = Outgoing {
            to: draft.to,
            cc: draft.cc,
            subject: draft.subject,
            body: draft.body + "Thanks, it reads well.\n",
            in_reply_to: draft.in_reply_to,
            references: draft.references,
            format: BodyFormat::Markdown,
            ..message("")
        };
        let (_, parsed) = sent(&outgoing);
        assert_eq!(parsed.headers.subject, "Re: Q4 roadmap");
        assert_eq!(
            parsed.headers.to,
            [Address::new("Ana Ruiz", "ana.ruiz@example.com")]
        );
        assert_eq!(
            parsed.in_reply_to.as_deref(),
            Some("<roadmap.2@example.com>")
        );
        assert_eq!(
            parsed.references,
            ["<roadmap.1@example.com>", "<roadmap.2@example.com>"]
        );
        assert_eq!(
            parsed.body,
            "On Sat, 14 Nov 2026 at 09:14, Ana Ruiz wrote:\n> Here is the draft.\n\n\
             Thanks, it reads well.\n"
        );
        let html = parsed.html.expect("html");
        assert!(
            html.contains("<p>Here is the draft.</p>\n</blockquote>"),
            "{html}"
        );
    }

    #[test]
    fn the_date_is_written_in_the_zone_given() {
        for (timestamp, offset, expected) in [
            (DATE, 0, "Sat, 14 Nov 2026 09:30:00 +0000"),
            (DATE, 3_600, "Sat, 14 Nov 2026 10:30:00 +0100"),
            (DATE, -8 * 3_600, "Sat, 14 Nov 2026 01:30:00 -0800"),
            (DATE, 5 * 3_600 + 45 * 60, "Sat, 14 Nov 2026 15:15:00 +0545"),
            (
                DATE,
                -(9 * 3_600 + 30 * 60),
                "Sat, 14 Nov 2026 00:00:00 -0930",
            ),
            (DATE, 14 * 3_600, "Sat, 14 Nov 2026 23:30:00 +1400"),
            (DATE, 15 * 3_600, "Sun, 15 Nov 2026 00:30:00 +1500"),
            (DATE, -10 * 3_600, "Fri, 13 Nov 2026 23:30:00 -1000"),
            // Seconds of offset are not a thing a header can say.
            (DATE, 3_659, "Sat, 14 Nov 2026 10:30:00 +0100"),
            (DATE, -3_659, "Sat, 14 Nov 2026 08:30:00 -0100"),
            // Not zones at all.
            (DATE, 86_400, "Sat, 14 Nov 2026 09:30:00 +0000"),
            (DATE, i32::MIN, "Sat, 14 Nov 2026 09:30:00 +0000"),
            (DATE, i32::MAX, "Sat, 14 Nov 2026 09:30:00 +0000"),
            (0, 0, "Thu, 1 Jan 1970 00:00:00 +0000"),
            (0, -3_600, "Wed, 31 Dec 1969 23:00:00 -0100"),
            (951_782_400, 0, "Tue, 29 Feb 2000 00:00:00 +0000"),
            (1_709_251_199, 0, "Thu, 29 Feb 2024 23:59:59 +0000"),
            (1_735_689_600, 0, "Wed, 1 Jan 2025 00:00:00 +0000"),
            (4_107_542_399, 0, "Sun, 28 Feb 2100 23:59:59 +0000"),
            (4_107_542_400, 0, "Mon, 1 Mar 2100 00:00:00 +0000"),
            // Out of range: clamped, never a panic or a five-digit year.
            (-1, 0, "Thu, 1 Jan 1970 00:00:00 +0000"),
            (i64::MIN, -3_600, "Wed, 31 Dec 1969 23:00:00 -0100"),
            (i64::MAX, 0, "Thu, 31 Dec 9998 23:59:59 +0000"),
            (i64::MAX, 86_399, "Fri, 1 Jan 9999 23:58:59 +2359"),
        ] {
            assert_eq!(
                date_header(timestamp, offset),
                expected,
                "{timestamp} {offset}"
            );
        }

        // Whatever the zone, it is the same moment when read back.
        for offset in [0, 3_600, -8 * 3_600, 5 * 3_600 + 45 * 60, 14 * 3_600] {
            let raw = build(&message("x\n"), offset);
            assert_eq!(parse(&raw, 0).expect("a message").headers.date, DATE);
        }
    }

    #[test]
    fn a_new_message_id_is_the_random_bytes_and_the_domain() {
        let random = [
            0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xff, 0x10, 0x20, 0x30, 0x40,
            0x50, 0x60,
        ];
        assert_eq!(
            new_message_id("robin@example.com", &random),
            "<000123456789abcdefff102030405060@example.com>"
        );
        assert_eq!(
            new_message_id("Robin@Mail.EXAMPLE.com", &random),
            "<000123456789abcdefff102030405060@mail.example.com>"
        );
        // An address with no domain that can be written gets a reserved
        // one, never the machine's.
        for address in [
            "",
            "robin",
            "robin@",
            "robin@localhost\r\nBcc: x",
            "robin@bücher.example",
            "robin@[192.0.2.1]",
            "robin@.example.com",
            "robin@example.com.",
            "robin@exa mple.com",
        ] {
            assert_eq!(
                new_message_id(address, &random),
                "<000123456789abcdefff102030405060@mail.invalid>",
                "{address:?}"
            );
        }
        // Different bytes, different identifier; and it is one that builds.
        assert_ne!(
            new_message_id("robin@example.com", &[7; 16]),
            new_message_id("robin@example.com", &[8; 16])
        );
        let mut outgoing = message("x\n");
        outgoing.message_id = new_message_id("robin@example.com", &random);
        let (_, parsed) = sent(&outgoing);
        assert_eq!(parsed.headers.message_id, Some(outgoing.message_id));
    }

    #[test]
    fn a_message_without_a_usable_id_still_gets_one() {
        for broken in [
            "",
            "   ",
            "<>",
            "<two words@example.com>",
            "<caf\u{e9}@example.com>",
        ] {
            let mut outgoing = message("x\n");
            outgoing.message_id = broken.to_owned();
            let (text, parsed) = sent(&outgoing);
            let id = parsed.headers.message_id.expect("an id");
            assert!(id.ends_with("@example.com>"), "{id}");
            assert!(id.len() > 30, "{id}");
            assert_eq!(header(&text, "Message-ID"), id);
            // The same message gets the same one, another message another.
            assert_eq!(build(&outgoing, 0), build(&outgoing, 0));
            outgoing.body = "y\n".to_owned();
            let (_, other) = sent(&outgoing);
            assert_ne!(other.headers.message_id, Some(id));
        }
    }

    #[test]
    fn an_address_outside_ascii_is_the_one_thing_written_as_it_is() {
        let mut outgoing = message("x\n");
        outgoing.to = vec![Address::new("Jürgen", "jürgen@bücher.example")];
        let raw = build(&outgoing, 0);
        let text = String::from_utf8(raw.clone()).expect("UTF-8");
        // The name is encoded; the address cannot be.
        assert!(text.contains("To: =?utf-8?"), "{text}");
        assert!(text.contains("<jürgen@bücher.example>"), "{text}");
        assert_eq!(envelope(&outgoing).to, ["jürgen@bücher.example"]);
        assert_eq!(parse(&raw, 0).expect("a message").headers.to, outgoing.to);
    }

    #[test]
    fn warnings_before_sending() {
        assert!(warnings(&message("Hi Ana,\n\nSee you Friday.\n")).is_empty());

        let mut outgoing = message("Hi\n");
        outgoing.subject = "  \t".to_owned();
        assert_eq!(warnings(&outgoing), [Warning::EmptySubject]);

        for body in [
            "The report is attached.",
            "See the ATTACHMENT for details",
            "I am attaching the invoice.\n",
            "Two attachments, as promised",
            "Please find enclosed: the contract.",
            "Summary\n\n(attached)\n> older text",
            "Not a quote > just a sign, attached",
        ] {
            let mut outgoing = message(body);
            assert_eq!(
                warnings(&outgoing),
                [Warning::MissingAttachment],
                "{body:?}"
            );
            // With something attached there is nothing to ask.
            outgoing.attachments = vec![attachment("a.txt", "text/plain", b"a")];
            assert!(warnings(&outgoing).is_empty(), "{body:?}");
            // Both at once, the subject first.
            outgoing.attachments.clear();
            outgoing.subject.clear();
            assert_eq!(
                warnings(&outgoing),
                [Warning::EmptySubject, Warning::MissingAttachment]
            );
        }

        for body in [
            "",
            "Nothing to see.",
            "> The report is attached.\n\nThanks, got it.",
            "On Friday, Ana wrote:\n> I am attaching the invoice.\n>> enclosed\n  > attachment\n",
            "I am very attached to this plan.",
            "The reattachment went well; she felt unattached and disenclosed.",
            "Attach it later.",
        ] {
            let found = warnings(&message(body));
            if body.contains("very attached") {
                // A word is a word: this one asks, and the user says send.
                assert_eq!(found, [Warning::MissingAttachment]);
            } else {
                assert!(found.is_empty(), "{body:?}");
            }
        }
    }

    #[test]
    fn nothing_makes_building_panic() {
        let nasty = [
            "",
            " ",
            "\0",
            "\r",
            "\n",
            "\r\n\r\n",
            "\u{feff}",
            "\u{202e}reversed",
            "=?",
            "=?utf-8?B?",
            "<",
            "<>",
            "@",
            "\"",
            "\\",
            "a\u{0}b\u{7f}c\u{85}d\u{9b}e",
            "--=_alt_",
            "\u{10ffff}\u{fffd}",
        ];
        let long = "é".repeat(3_000);
        for text in nasty.into_iter().chain([long.as_str()]) {
            let outgoing = Outgoing {
                from: Address::new(text, text),
                to: vec![Address::new(text, text), Address::new("", text)],
                cc: vec![Address::new(text, text)],
                bcc: vec![Address::new(text, text)],
                subject: text.to_owned(),
                body: text.to_owned(),
                format: BodyFormat::Markdown,
                in_reply_to: Some(text.to_owned()),
                references: vec![text.to_owned(), text.to_owned()],
                attachments: vec![
                    attachment(text, text, text.as_bytes()),
                    attachment(text, "message/rfc822", text.as_bytes()),
                    attachment(text, "text/plain", text.as_bytes()),
                ],
                date: i64::MIN,
                message_id: text.to_owned(),
            };
            for offset in [0, i32::MIN, i32::MAX] {
                let raw = build(&outgoing, offset);
                // Addresses aside, the promises hold even for this.
                if text.is_ascii() || mailbox(text).is_none() {
                    wire(&raw);
                }
                assert!(parse(&raw, 0).is_some());
            }
            let _ = envelope(&outgoing);
            let _ = warnings(&outgoing);
            let _ = new_message_id(text, &[0; 16]);
        }

        let empty = Outgoing {
            from: Address::new("", ""),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: String::new(),
            format: BodyFormat::Plain,
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
            date: 0,
            message_id: String::new(),
        };
        let text = wire(&build(&empty, 0));
        assert!(text.contains("To: undisclosed-recipients:;\r\n"));
        assert_eq!(
            envelope(&empty),
            Envelope {
                from: String::new(),
                to: Vec::new()
            }
        );
    }
}
