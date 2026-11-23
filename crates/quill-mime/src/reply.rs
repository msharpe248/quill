//! Starting a reply or a forward from a message that was read.
//!
//! What comes back is a [`Draft`]: who it goes to, the subject, the quoted
//! text and the threading headers. The user edits it, and
//! [`crate::compose::build`] makes the message.
//!
//! The original is a stranger's message. Its text is quoted with control
//! characters removed, so nothing in it can reach the editor or the next
//! reader as anything but text.

use quill_core::types::Address;

use crate::Parsed;

/// The line a forward puts above the message it passes on. What follows
/// it in a message being sent is somebody else's words, and is never read
/// as Markdown ([`crate::markdown::to_html`]).
pub const FORWARDED: &str = "-------- Forwarded message --------";

/// Where the user starts typing in a reply (`compose.reply_position`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyPosition {
    /// Above the quoted message.
    Above,
    /// Below it.
    Below,
}

/// The start of a message: what a reply or a forward fills in before the
/// user writes anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub body: String,
    /// The `Message-ID` of the message being answered, with its angle
    /// brackets.
    pub in_reply_to: Option<String>,
    /// The conversation so far, oldest first, ending with the message this
    /// draft continues.
    pub references: Vec<String>,
    /// Where the cursor starts: a byte offset into `body`, always on a
    /// character boundary.
    pub cursor: usize,
}

/// A reply to `original`.
///
/// It goes to the `Reply-To` address when the sender gave one, otherwise to
/// the sender. `all` also addresses everyone else the original went to,
/// keeping them in To or Cc as they were. `mine` are the user's own
/// addresses: they are never added by `all`, and a reply to a message the
/// user sent goes to the people it was sent to, not back to the user.
///
/// `date_line` is when the original was sent, already worded for the
/// attribution line ("On {date_line}, {name} wrote:").
pub fn reply(
    original: &Parsed,
    all: bool,
    mine: &[String],
    position: ReplyPosition,
    date_line: &str,
) -> Draft {
    let headers = &original.headers;
    let is_mine = |address: &Address| mine.iter().any(|own| same_address(own, &address.email));
    let from_me = headers.from.iter().any(is_mine);

    let mut to: Vec<Address> = Vec::new();
    let mut cc: Vec<Address> = Vec::new();
    if from_me {
        // Following up on something the user sent: it goes where that went.
        add_new(&mut to, &[], headers.to.iter().filter(|a| !is_mine(a)));
        if all {
            add_new(&mut cc, &to, headers.cc.iter().filter(|a| !is_mine(a)));
        }
    }
    if to.is_empty() {
        // The ordinary case, and also a note the user sent only to
        // themselves.
        let sender = if original.reply_to.is_empty() {
            &headers.from
        } else {
            &original.reply_to
        };
        add_new(&mut to, &[], sender.iter());
        if all && !from_me {
            let others = headers.to.iter().filter(|a| !is_mine(a));
            let mut more = Vec::new();
            add_new(&mut more, &to, others);
            to.append(&mut more);
            add_new(&mut cc, &to, headers.cc.iter().filter(|a| !is_mine(a)));
        }
    }

    let sender = headers
        .from
        .first()
        .map(|address| one_line(address.display_name()))
        .filter(|name| !name.is_empty());
    let date_line = one_line(date_line);
    let attribution = match (date_line.is_empty(), sender) {
        (false, Some(name)) => format!("On {date_line}, {name} wrote:"),
        (true, Some(name)) => format!("{name} wrote:"),
        (false, None) => format!("On {date_line}:"),
        (true, None) => "Earlier message:".to_owned(),
    };
    let quoted = quote(&original.body);

    let (body, cursor) = match position {
        ReplyPosition::Above => (format!("\n\n{attribution}\n{quoted}"), 0),
        ReplyPosition::Below => {
            let body = format!("{attribution}\n{quoted}\n");
            let cursor = body.len();
            (body, cursor)
        }
    };

    Draft {
        to,
        cc,
        subject: reply_subject(&headers.subject),
        body,
        in_reply_to: headers.message_id.clone(),
        references: thread(original),
        cursor,
    }
}

/// A forward of `original`, with its text inline under a block that says
/// who sent it, when, about what and to whom. The cursor starts above it.
///
/// `date_line` is when the original was sent, already worded for the
/// reader. To forward the message as a file instead, attach its raw bytes
/// as `message/rfc822`.
pub fn forward(original: &Parsed, date_line: &str) -> Draft {
    let headers = &original.headers;
    let list = |addresses: &[Address]| {
        addresses
            .iter()
            .map(|address| one_line(&address.to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut body = format!("\n\n{FORWARDED}\n");
    for (name, value) in [
        ("From", list(&headers.from)),
        ("Date", one_line(date_line)),
        ("Subject", one_line(&headers.subject)),
        ("To", list(&headers.to)),
        ("Cc", list(&headers.cc)),
    ] {
        if !value.is_empty() {
            body.push_str(name);
            body.push_str(": ");
            body.push_str(&value);
            body.push('\n');
        }
    }
    body.push('\n');
    for line in clean_text(&original.body).trim_end().lines() {
        body.push_str(line.trim_end());
        body.push('\n');
    }

    Draft {
        to: Vec::new(),
        cc: Vec::new(),
        subject: forward_subject(&headers.subject),
        body,
        // A forward answers nobody, but naming the original keeps it in
        // the same conversation in clients that thread by References.
        in_reply_to: None,
        references: thread(original),
        cursor: 0,
    }
}

/// Appends the addresses in `candidates` that are in neither `list` nor
/// `taken` yet.
fn add_new<'a>(
    list: &mut Vec<Address>,
    taken: &[Address],
    candidates: impl Iterator<Item = &'a Address>,
) {
    for candidate in candidates {
        let known = |address: &Address| same_address(&address.email, &candidate.email);
        if !candidate.email.trim().is_empty() && !list.iter().any(known) && !taken.iter().any(known)
        {
            list.push(candidate.clone());
        }
    }
}

/// Whether two addresses are the same mailbox. Case is ignored: no mail
/// system in use tells "Ana@" from "ana@".
fn same_address(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// `References` for a message that continues `original`'s conversation, as
/// RFC 5322 section 3.6.4 lays it out: the original's `References` (or,
/// when it has none, its `In-Reply-To`) followed by its `Message-ID`.
fn thread(original: &Parsed) -> Vec<String> {
    let mut references = if original.references.is_empty() {
        original.in_reply_to.iter().cloned().collect()
    } else {
        original.references.clone()
    };
    if let Some(id) = &original.headers.message_id
        && references.last() != Some(id)
    {
        references.push(id.clone());
    }
    references
}

/// The original's text with "> " before each line. A line that is already
/// a quote gets ">" with no space, so depth reads as ">>", and an empty
/// line gets a bare ">" so no line ends in a space.
fn quote(text: &str) -> String {
    let text = clean_text(text);
    let mut quoted = String::with_capacity(text.len() + text.len() / 16 + 2);
    for line in text.trim_end().lines() {
        let line = line.trim_end();
        if line.is_empty() {
            quoted.push('>');
        } else if line.starts_with('>') {
            quoted.push('>');
            quoted.push_str(line);
        } else {
            quoted.push_str("> ");
            quoted.push_str(line);
        }
        quoted.push('\n');
    }
    quoted
}

/// Text with `\n` line endings and no control characters but tab.
fn clean_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' { '\n' } else { c })
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// Text from a header, fit for one line of the body: control characters and
/// line breaks become spaces.
fn one_line(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect();
    spaced.split(' ').filter(|word| !word.is_empty()).fold(
        String::with_capacity(spaced.len()),
        |mut line, word| {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            line
        },
    )
}

/// The subject for a reply: "Re: " before it, unless it already says so.
fn reply_subject(subject: &str) -> String {
    let subject = one_line(subject);
    if has_reply_prefix(&subject) {
        subject
    } else {
        format!("Re: {subject}").trim_end().to_owned()
    }
}

/// The subject for a forward: "Fwd: " before it, unless it already says so.
fn forward_subject(subject: &str) -> String {
    let subject = one_line(subject);
    if has_forward_prefix(&subject) {
        subject
    } else {
        format!("Fwd: {subject}").trim_end().to_owned()
    }
}

/// Whether a subject already starts with a reply marker.
///
/// "Re:" in any case, with a count as some programs write it ("Re[2]:",
/// "Re(3):"), and the markers German, Scandinavian and Dutch mail programs
/// use, in the capitals those programs write. Other languages' markers are
/// short words that also start ordinary subjects, so they are left alone:
/// the cost of missing one is a "Re: " too many, never a lost subject.
fn has_reply_prefix(subject: &str) -> bool {
    let Some((marker, _)) = split_marker(subject) else {
        return false;
    };
    let lower = marker.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("re") {
        let counted = |open: char, close: char| {
            rest.strip_prefix(open)
                .and_then(|rest| rest.strip_suffix(close))
                .is_some_and(|count| !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit()))
        };
        return rest.is_empty() || counted('[', ']') || counted('(', ')');
    }
    matches!(marker, "AW" | "SV" | "Antw" | "ANTW")
}

/// Whether a subject already starts with a forward marker: "Fwd:" or "Fw:"
/// in any case, or German "WG:".
fn has_forward_prefix(subject: &str) -> bool {
    split_marker(subject).is_some_and(|(marker, _)| {
        matches!(marker.to_ascii_lowercase().as_str(), "fwd" | "fw") || marker == "WG"
    })
}

/// Splits "Re: the rest" into its marker and the rest. The colon may be the
/// full-width one East Asian mail programs write.
fn split_marker(subject: &str) -> Option<(&str, &str)> {
    let (marker, rest) = subject.split_once([':', '\u{ff1a}'])?;
    // A marker is one short word. Anything longer is the subject itself.
    (!marker.is_empty() && marker.len() <= 8 && !marker.contains(' ')).then_some((marker, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const RECEIVED: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z
    const WHEN: &str = "Sat, 14 Nov 2026 at 09:14";

    fn read(raw: &str) -> Parsed {
        parse(raw.replace('\n', "\r\n").as_bytes(), RECEIVED).expect("a message")
    }

    fn mine() -> Vec<String> {
        vec![
            "robin@example.com".to_owned(),
            "robin.vale@example.com".to_owned(),
        ]
    }

    fn original() -> Parsed {
        read(
            "From: Ana Ruiz <ana.ruiz@example.com>\n\
             To: Robin Vale <robin@example.com>, Lena Fischer <lena.fischer@example.com>\n\
             Cc: \"Haddad, Omar\" <omar.haddad@example.com>, ROBIN.VALE@example.com\n\
             Subject: Q4 roadmap\n\
             Date: Sat, 14 Nov 2026 09:14:00 +0000\n\
             Message-ID: <roadmap.3@example.com>\n\
             In-Reply-To: <roadmap.2@example.com>\n\
             References: <roadmap.1@example.com> <roadmap.2@example.com>\n\
             \n\
             Hi Robin,\n\
             \n\
             Here is the draft.\n\
             > You asked for it on Friday.\n\
             >> Did I?\n\
             \n\
             Ana\n\
             \n",
        )
    }

    fn emails(addresses: &[Address]) -> Vec<&str> {
        addresses.iter().map(|a| a.email.as_str()).collect()
    }

    #[test]
    fn the_parser_reads_what_a_reply_needs() {
        let parsed = read(
            "From: Ana Ruiz <ana.ruiz@example.com>\n\
             Reply-To: Roadmap list <roadmap@example.com>, ana.ruiz@example.com\n\
             In-Reply-To: <b@example.com>\n\
             References: <a@example.com>\n\t<b@example.com>\n\
             Subject: x\n\nx\n",
        );
        assert_eq!(
            parsed.reply_to,
            [
                Address::new("Roadmap list", "roadmap@example.com"),
                Address::new("", "ana.ruiz@example.com"),
            ]
        );
        assert_eq!(parsed.in_reply_to.as_deref(), Some("<b@example.com>"));
        assert_eq!(parsed.references, ["<a@example.com>", "<b@example.com>"]);

        let bare = read("Subject: x\n\nx\n");
        assert!(bare.reply_to.is_empty() && bare.references.is_empty());
        assert_eq!(bare.in_reply_to, None);
    }

    #[test]
    fn a_reply_goes_to_the_sender_alone() {
        let draft = reply(&original(), false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(draft.to, [Address::new("Ana Ruiz", "ana.ruiz@example.com")]);
        assert!(draft.cc.is_empty());
        assert_eq!(draft.subject, "Re: Q4 roadmap");
        assert_eq!(
            draft.in_reply_to.as_deref(),
            Some("<roadmap.3@example.com>")
        );
    }

    #[test]
    fn a_reply_to_all_keeps_everyone_but_the_user_once_each() {
        let draft = reply(&original(), true, &mine(), ReplyPosition::Above, WHEN);
        // Robin's two addresses are gone, one of them written in capitals.
        assert_eq!(
            emails(&draft.to),
            ["ana.ruiz@example.com", "lena.fischer@example.com"]
        );
        assert_eq!(
            draft.cc,
            [Address::new("Haddad, Omar", "omar.haddad@example.com")]
        );

        let repeats = read(
            "From: ana.ruiz@example.com\n\
             To: Ana Ruiz <ANA.RUIZ@example.com>, lena.fischer@example.com, \
             Lena <Lena.Fischer@Example.com>\n\
             Cc: lena.fischer@example.com, omar.haddad@example.com, omar.haddad@example.com, \
             ana.ruiz@example.com\n\
             Subject: x\n\nx\n",
        );
        let draft = reply(&repeats, true, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            emails(&draft.to),
            ["ana.ruiz@example.com", "lena.fischer@example.com"]
        );
        assert_eq!(emails(&draft.cc), ["omar.haddad@example.com"]);
    }

    #[test]
    fn reply_to_is_honoured() {
        let listed = read(
            "From: Ana Ruiz <ana.ruiz@example.com>\n\
             Reply-To: Roadmap list <roadmap@example.com>\n\
             To: robin@example.com, lena.fischer@example.com\n\
             Subject: x\n\nx\n",
        );
        let draft = reply(&listed, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            draft.to,
            [Address::new("Roadmap list", "roadmap@example.com")]
        );
        let draft = reply(&listed, true, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            emails(&draft.to),
            ["roadmap@example.com", "lena.fischer@example.com"]
        );
        // The attribution still names who wrote it.
        assert!(draft.body.contains("Ana Ruiz wrote:"));
    }

    #[test]
    fn a_reply_to_a_message_the_user_sent_goes_to_its_recipients() {
        let sent = read(
            "From: Robin Vale <Robin@Example.com>\n\
             Reply-To: robin.vale@example.com\n\
             To: Ana Ruiz <ana.ruiz@example.com>, robin.vale@example.com\n\
             Cc: lena.fischer@example.com\n\
             Subject: Re: Q4 roadmap\n\nThanks.\n",
        );
        let draft = reply(&sent, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(emails(&draft.to), ["ana.ruiz@example.com"]);
        assert!(draft.cc.is_empty());
        let draft = reply(&sent, true, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(emails(&draft.to), ["ana.ruiz@example.com"]);
        assert_eq!(emails(&draft.cc), ["lena.fischer@example.com"]);

        // A note to oneself has nobody else to go to.
        let note = read("From: robin@example.com\nTo: robin@example.com\nSubject: x\n\nx\n");
        let draft = reply(&note, true, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(emails(&draft.to), ["robin@example.com"]);
        assert!(draft.cc.is_empty());
    }

    #[test]
    fn re_is_added_once() {
        for (subject, expected) in [
            ("Q4 roadmap", "Re: Q4 roadmap"),
            ("Re: Q4 roadmap", "Re: Q4 roadmap"),
            ("RE: Q4 roadmap", "RE: Q4 roadmap"),
            ("re:Q4 roadmap", "re:Q4 roadmap"),
            ("Re[2]: Q4 roadmap", "Re[2]: Q4 roadmap"),
            ("RE(3): Q4 roadmap", "RE(3): Q4 roadmap"),
            ("AW: Q4 roadmap", "AW: Q4 roadmap"),
            ("SV: Q4 roadmap", "SV: Q4 roadmap"),
            ("Antw: Q4 roadmap", "Antw: Q4 roadmap"),
            ("Re\u{ff1a}Q4 roadmap", "Re\u{ff1a}Q4 roadmap"),
            ("", "Re:"),
            ("  Q4 roadmap ", "Re: Q4 roadmap"),
            // Not reply markers.
            ("Fwd: Q4 roadmap", "Re: Fwd: Q4 roadmap"),
            ("Reminder: Q4 roadmap", "Re: Reminder: Q4 roadmap"),
            ("Re Q4 roadmap", "Re: Re Q4 roadmap"),
            ("Regarding: lunch", "Re: Regarding: lunch"),
            ("Re[x]: lunch", "Re: Re[x]: lunch"),
            ("Re[]: lunch", "Re: Re[]: lunch"),
            ("Aw: that is kind", "Re: Aw: that is kind"),
            ("Sv: lunch", "Re: Sv: lunch"),
            ("R: lunch", "Re: R: lunch"),
            ("Note to self: Re: lunch", "Re: Note to self: Re: lunch"),
            (
                "Line one\r\nBcc: x@example.com",
                "Re: Line one Bcc: x@example.com",
            ),
        ] {
            assert_eq!(reply_subject(subject), expected, "{subject:?}");
        }
    }

    #[test]
    fn fwd_is_added_once() {
        for (subject, expected) in [
            ("Q4 roadmap", "Fwd: Q4 roadmap"),
            ("Fwd: Q4 roadmap", "Fwd: Q4 roadmap"),
            ("FWD: Q4 roadmap", "FWD: Q4 roadmap"),
            ("Fw: Q4 roadmap", "Fw: Q4 roadmap"),
            ("FW: Q4 roadmap", "FW: Q4 roadmap"),
            ("WG: Q4 roadmap", "WG: Q4 roadmap"),
            ("Re: Q4 roadmap", "Fwd: Re: Q4 roadmap"),
            ("Forward planning: Q4", "Fwd: Forward planning: Q4"),
            ("", "Fwd:"),
        ] {
            assert_eq!(forward_subject(subject), expected, "{subject:?}");
        }
    }

    #[test]
    fn references_continue_the_originals() {
        // The original has References: they are kept and its id added.
        let draft = reply(&original(), false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            draft.references,
            [
                "<roadmap.1@example.com>",
                "<roadmap.2@example.com>",
                "<roadmap.3@example.com>",
            ]
        );
        assert_eq!(
            draft.in_reply_to.as_deref(),
            Some("<roadmap.3@example.com>")
        );

        // No References but an In-Reply-To: that, then its id.
        let second = read(
            "From: ana.ruiz@example.com\n\
             Message-ID: <b@example.com>\n\
             In-Reply-To: <a@example.com>\n\
             Subject: x\n\nx\n",
        );
        let draft = reply(&second, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(draft.references, ["<a@example.com>", "<b@example.com>"]);
        assert_eq!(draft.in_reply_to.as_deref(), Some("<b@example.com>"));

        // Neither: the conversation starts with the original.
        let first = read("From: ana.ruiz@example.com\nMessage-ID: <a@example.com>\n\nx\n");
        let draft = reply(&first, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(draft.references, ["<a@example.com>"]);
        assert_eq!(draft.in_reply_to.as_deref(), Some("<a@example.com>"));

        // No Message-ID at all: nothing to answer, and nothing invented.
        let anonymous = read("From: ana.ruiz@example.com\nSubject: x\n\nx\n");
        let draft = reply(&anonymous, false, &mine(), ReplyPosition::Above, WHEN);
        assert!(draft.references.is_empty());
        assert_eq!(draft.in_reply_to, None);

        // An original that already lists itself is not listed twice.
        let looped = read(
            "From: ana.ruiz@example.com\n\
             Message-ID: <b@example.com>\n\
             References: <a@example.com> <b@example.com>\n\nx\n",
        );
        let draft = reply(&looped, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(draft.references, ["<a@example.com>", "<b@example.com>"]);
    }

    const QUOTED: &str = "> Hi Robin,\n\
                          >\n\
                          > Here is the draft.\n\
                          >> You asked for it on Friday.\n\
                          >>> Did I?\n\
                          >\n\
                          > Ana\n";

    #[test]
    fn the_original_is_quoted_one_level_deeper_under_an_attribution() {
        let draft = reply(&original(), false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            draft.body,
            format!("\n\nOn Sat, 14 Nov 2026 at 09:14, Ana Ruiz wrote:\n{QUOTED}")
        );
        // No line of the quote ends in a space.
        assert!(draft.body.lines().all(|line| !line.ends_with(' ')));
    }

    #[test]
    fn the_cursor_starts_above_or_below_the_quote() {
        let above = reply(&original(), false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(above.cursor, 0);
        assert!(above.body.starts_with("\n\nOn "));

        let below = reply(&original(), false, &mine(), ReplyPosition::Below, WHEN);
        assert_eq!(
            below.body,
            format!("On Sat, 14 Nov 2026 at 09:14, Ana Ruiz wrote:\n{QUOTED}\n")
        );
        assert_eq!(below.cursor, below.body.len());
        // A blank line separates the quote from what the user types.
        assert!(below.body.ends_with("> Ana\n\n"));
    }

    #[test]
    fn the_attribution_copes_with_what_is_missing() {
        let unnamed = read("From: ana.ruiz@example.com\nSubject: x\n\nHello\n");
        let draft = reply(&unnamed, false, &mine(), ReplyPosition::Below, WHEN);
        assert!(
            draft.body.starts_with(
                "On Sat, 14 Nov 2026 at 09:14, ana.ruiz@example.com wrote:\n> Hello\n"
            )
        );
        let draft = reply(&unnamed, false, &mine(), ReplyPosition::Below, "");
        assert!(draft.body.starts_with("ana.ruiz@example.com wrote:\n"));

        let nobody = read("Subject: x\n\nHello\n");
        let draft = reply(&nobody, true, &mine(), ReplyPosition::Below, WHEN);
        assert!(draft.to.is_empty() && draft.cc.is_empty());
        assert!(
            draft
                .body
                .starts_with("On Sat, 14 Nov 2026 at 09:14:\n> Hello\n")
        );

        let empty = read("From: ana.ruiz@example.com\nSubject: x\n\n");
        let draft = reply(&empty, false, &mine(), ReplyPosition::Above, WHEN);
        assert_eq!(
            draft.body,
            "\n\nOn Sat, 14 Nov 2026 at 09:14, ana.ruiz@example.com wrote:\n"
        );
    }

    #[test]
    fn hostile_text_in_the_original_stays_text() {
        let hostile = parse(
            b"From: \"Ana\r\n\tRuiz\x1b[2J\" <ana.ruiz@example.com>\r\n\
              Subject: Hello\x07 there\r\n\
              \r\n\
              One\x1b]0;title\x07\r\nTwo\rThree\x00\r\n\tindented\r\n",
            RECEIVED,
        )
        .expect("a message");
        for draft in [
            reply(
                &hostile,
                true,
                &mine(),
                ReplyPosition::Above,
                "today\r\nBcc: x",
            ),
            forward(&hostile, "today\r\nBcc: x"),
        ] {
            assert!(
                draft
                    .body
                    .chars()
                    .all(|c| !c.is_control() || matches!(c, '\n' | '\t')),
                "{:?}",
                draft.body
            );
            assert!(draft.subject.chars().all(|c| !c.is_control()));
            assert!(draft.body.contains("today Bcc: x"), "{:?}", draft.body);
            assert!(draft.body.contains("\tindented"), "{:?}", draft.body);
            assert!(draft.body.is_char_boundary(draft.cursor));
        }
    }

    #[test]
    fn a_forward_carries_the_original_under_a_header_block() {
        let draft = forward(&original(), WHEN);
        assert!(draft.to.is_empty() && draft.cc.is_empty());
        assert_eq!(draft.subject, "Fwd: Q4 roadmap");
        assert_eq!(draft.cursor, 0);
        assert_eq!(
            draft.body,
            "\n\n-------- Forwarded message --------\n\
             From: Ana Ruiz <ana.ruiz@example.com>\n\
             Date: Sat, 14 Nov 2026 at 09:14\n\
             Subject: Q4 roadmap\n\
             To: Robin Vale <robin@example.com>, Lena Fischer <lena.fischer@example.com>\n\
             Cc: Haddad, Omar <omar.haddad@example.com>, ROBIN.VALE@example.com\n\
             \n\
             Hi Robin,\n\
             \n\
             Here is the draft.\n\
             > You asked for it on Friday.\n\
             >> Did I?\n\
             \n\
             Ana\n"
        );
        // It answers nobody but stays in the conversation.
        assert_eq!(draft.in_reply_to, None);
        assert_eq!(
            draft.references.last().map(String::as_str),
            Some("<roadmap.3@example.com>")
        );
    }

    #[test]
    fn a_forward_leaves_out_the_lines_it_has_nothing_for() {
        let bare = read("From: ana.ruiz@example.com\n\nJust this.\n");
        let draft = forward(&bare, "");
        assert_eq!(
            draft.body,
            "\n\n-------- Forwarded message --------\n\
             From: ana.ruiz@example.com\n\
             \n\
             Just this.\n"
        );
        assert_eq!(draft.subject, "Fwd:");
        assert!(draft.references.is_empty());
    }
}
