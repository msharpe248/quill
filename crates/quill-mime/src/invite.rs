//! Calendar invitations: the `text/calendar` part some mail carries, read
//! well enough to say what is being asked, and an answer to send back.
//!
//! This reads the one event an invitation is about. It is not a calendar:
//! recurrence rules are noticed, not worked out, and time zones are named,
//! not resolved. The caller knows the user's zone and how to show a time.
//!
//! An invitation is as untrusted as the mail around it. Everything
//! returned is clean ([`crate::clean`]) and bounded, and nothing in it is
//! ever fetched or run.

use mail_parser::MessageParser;

use crate::clean;

/// The largest calendar part that is read. An invitation is a few
/// kilobytes; one that is megabytes is something else.
const MAX_CALENDAR: usize = 256 * 1024;
/// The most people listed.
const MAX_ATTENDEES: usize = 200;
/// The longest any one piece of text may be.
const MAX_TEXT: usize = 2_000;

/// What the sender wants done with the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// An invitation, or a change to one: it asks for an answer.
    Request,
    /// The event is off.
    Cancel,
    /// Someone's answer to an invitation of the user's.
    Reply,
    /// For information: nothing is asked.
    Publish,
}

/// A moment or a day, as the invitation wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When {
    pub year: i16,
    pub month: i8,
    pub day: i8,
    /// Hour, minute and second. `None` for an all-day event.
    pub time: Option<(i8, i8, i8)>,
    pub zone: Zone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    Utc,
    /// A zone by name, as written (`TZID`): an IANA name from most senders,
    /// a Windows one from some.
    Named(String),
    /// No zone given: the time is whatever the clock says where the reader
    /// is.
    Floating,
}

/// Someone on the invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Their address, lower case, without `mailto:`.
    pub address: String,
    pub name: Option<String>,
    /// What they have answered, if the invitation says.
    pub status: Option<Answer>,
}

/// An answer to an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Accepted,
    Declined,
    Tentative,
}

impl Answer {
    fn partstat(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Declined => "DECLINED",
            Self::Tentative => "TENTATIVE",
        }
    }

    /// The word that starts the subject of an answer.
    pub fn word(self) -> &'static str {
        match self {
            Self::Accepted => "Accepted",
            Self::Declined => "Declined",
            Self::Tentative => "Tentative",
        }
    }
}

/// One event, as an invitation describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub method: Method,
    /// The event's id, which an answer must repeat exactly.
    pub uid: String,
    /// Which revision of the event this is.
    pub sequence: u32,
    pub summary: String,
    pub start: Option<When>,
    pub end: Option<When>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub organizer: Option<Person>,
    pub attendees: Vec<Person>,
    /// The event repeats. When and how often is not worked out.
    pub repeats: bool,
    /// The start exactly as written, parameters and all, to repeat in an
    /// answer.
    raw_start: Option<String>,
    /// For one occurrence of a repeating event: which one, as written.
    raw_recurrence_id: Option<String>,
}

/// The invitation a raw message carries, if it carries one: its first
/// calendar part that describes an event.
pub fn find(raw: &[u8]) -> Option<Invite> {
    let message = MessageParser::default().parse(raw)?;
    message.parts().find_map(|part| {
        let kind = part.content_type()?;
        let subtype = kind.subtype().unwrap_or_default();
        let is_calendar = (kind.ctype().eq_ignore_ascii_case("text")
            && subtype.eq_ignore_ascii_case("calendar"))
            || (kind.ctype().eq_ignore_ascii_case("application")
                && subtype.eq_ignore_ascii_case("ics"));
        if !is_calendar || part.decoded_len() > MAX_CALENDAR {
            return None;
        }
        parse(&String::from_utf8_lossy(&part.decoded()))
    })
}

/// Reads an iCalendar text for the event it is about. `None` when it has
/// none, or none with an id.
pub fn parse(text: &str) -> Option<Invite> {
    if text.len() > MAX_CALENDAR {
        return None;
    }
    let lines = unfold(text);
    let mut method = Method::Publish;
    let mut in_event = false;
    let mut nested = 0usize;
    let mut invite = Invite {
        method,
        uid: String::new(),
        sequence: 0,
        summary: String::new(),
        start: None,
        end: None,
        location: None,
        description: None,
        organizer: None,
        attendees: Vec::new(),
        repeats: false,
        raw_start: None,
        raw_recurrence_id: None,
    };
    let mut found = false;
    for line in &lines {
        let Some(property) = Property::read(line) else {
            continue;
        };
        match (property.name.as_str(), in_event) {
            ("METHOD", false) => {
                method = match property.value.trim().to_ascii_uppercase().as_str() {
                    "REQUEST" => Method::Request,
                    "CANCEL" => Method::Cancel,
                    "REPLY" => Method::Reply,
                    _ => Method::Publish,
                };
            }
            ("BEGIN", false) if property.value.eq_ignore_ascii_case("VEVENT") && !found => {
                in_event = true;
            }
            // An alarm inside the event has properties of its own that
            // are not the event's.
            ("BEGIN", true) => nested += 1,
            ("END", true) if nested > 0 => nested -= 1,
            ("END", true) if property.value.eq_ignore_ascii_case("VEVENT") => {
                in_event = false;
                found = true;
            }
            (_, true) if nested > 0 => {}
            ("UID", true) => invite.uid = property.value.trim().to_owned(),
            ("SEQUENCE", true) => invite.sequence = property.value.trim().parse().unwrap_or(0),
            ("SUMMARY", true) => invite.summary = line_value(&property.value),
            ("LOCATION", true) => invite.location = Some(line_value(&property.value)),
            ("DESCRIPTION", true) => invite.description = Some(text_value(&property.value)),
            ("DTSTART", true) => {
                invite.start = when(&property);
                invite.raw_start = Some(line.clone());
            }
            ("DTEND", true) => invite.end = when(&property),
            ("RECURRENCE-ID", true) => invite.raw_recurrence_id = Some(line.clone()),
            ("RRULE" | "RDATE", true) => invite.repeats = true,
            ("ORGANIZER", true) => invite.organizer = person(&property),
            ("ATTENDEE", true) if invite.attendees.len() < MAX_ATTENDEES => {
                invite.attendees.extend(person(&property));
            }
            _ => {}
        }
    }
    invite.method = method;
    // An id made of anything but plain printable text cannot be repeated
    // safely in an answer.
    let usable = !invite.uid.is_empty()
        && invite.uid.len() <= 512
        && invite.uid.chars().all(|c| c.is_ascii_graphic() || c == ' ');
    (found && usable).then_some(invite)
}

/// A long line is continued on the next, which starts with a space or a
/// tab. Puts them back together.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ if line.is_empty() => {}
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// `NAME;PARAM=value;PARAM="quoted:value":the value`.
struct Property {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Property {
    fn read(line: &str) -> Option<Self> {
        // The value starts at the first colon that is not inside quotes.
        let mut quoted = false;
        let colon = line.char_indices().find_map(|(at, c)| match c {
            '"' => {
                quoted = !quoted;
                None
            }
            ':' if !quoted => Some(at),
            _ => None,
        })?;
        let (head, value) = (&line[..colon], &line[colon + 1..]);
        let mut pieces = split_unquoted(head, ';').into_iter();
        let name = pieces.next()?.trim().to_ascii_uppercase();
        let params = pieces
            .filter_map(|piece| {
                let (name, value) = piece.split_once('=')?;
                Some((
                    name.trim().to_ascii_uppercase(),
                    value.trim().trim_matches('"').to_owned(),
                ))
            })
            .collect();
        Some(Self {
            name,
            params,
            value: value.to_owned(),
        })
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(param, _)| param == name)
            .map(|(_, value)| value.as_str())
    }
}

fn split_unquoted(text: &str, separator: char) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == separator && !quoted {
            pieces.push(&text[start..at]);
            start = at + c.len_utf8();
        }
    }
    pieces.push(&text[start..]);
    pieces
}

/// A text value with its escapes undone, made clean and cut to size.
fn text_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    let mut clean = clean::body(out.trim()).into_owned();
    if clean.len() > MAX_TEXT {
        clean.truncate(clean.floor_char_boundary(MAX_TEXT));
    }
    clean
}

/// A text value that is shown on one line: clean of anything that could
/// reorder the line as well.
fn line_value(value: &str) -> String {
    clean::header(&text_value(value)).trim().to_owned()
}

/// `20261120T090000Z`, `20261120T090000` with or without a `TZID`, or
/// `20261120` for a whole day.
fn when(property: &Property) -> Option<When> {
    let value = property.value.trim();
    let (date, time) = match value.split_once(['T', 't']) {
        Some((date, time)) => (date, Some(time)),
        None => (value, None),
    };
    if date.len() != 8 || !date.is_ascii() {
        return None;
    }
    let year: i16 = date[..4].parse().ok()?;
    let month: i8 = date[4..6].parse().ok()?;
    let day: i8 = date[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (time, utc) = match time {
        Some(time) => {
            let (digits, utc) = match time.strip_suffix(['Z', 'z']) {
                Some(digits) => (digits, true),
                None => (time, false),
            };
            if digits.len() != 6 || !digits.is_ascii() {
                return None;
            }
            let hour: i8 = digits[..2].parse().ok()?;
            let minute: i8 = digits[2..4].parse().ok()?;
            let second: i8 = digits[4..6].parse().ok()?;
            if !(0..=23).contains(&hour)
                || !(0..=59).contains(&minute)
                || !(0..=60).contains(&second)
            {
                return None;
            }
            (Some((hour, minute, second.min(59))), utc)
        }
        None => (None, false),
    };
    let zone = if utc {
        Zone::Utc
    } else {
        match property.param("TZID") {
            Some(name) if time.is_some() => {
                let name = clean::header(name).trim().to_owned();
                if name.is_empty() || name.len() > 100 {
                    Zone::Floating
                } else {
                    Zone::Named(name)
                }
            }
            _ => Zone::Floating,
        }
    };
    Some(When {
        year,
        month,
        day,
        time,
        zone,
    })
}

/// An organizer or attendee: `mailto:` and an address, with a name and an
/// answer among the parameters.
fn person(property: &Property) -> Option<Person> {
    let value = property.value.trim();
    let address = value
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .map(|_| &value[7..])?;
    let address = clean::header(address).trim().to_lowercase();
    // What an address is made of, and nothing that could carry a second
    // one or a header along with it.
    let plain = !address.is_empty()
        && address.len() <= 254
        && address.contains('@')
        && address
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, ',' | ';' | '<' | '>' | '"' | '\\'));
    if !plain {
        return None;
    }
    let name = property
        .param("CN")
        .map(|name| clean::header(name).trim().to_owned())
        .filter(|name| !name.is_empty() && name.len() <= 200);
    let status = match property
        .param("PARTSTAT")
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("ACCEPTED") => Some(Answer::Accepted),
        Some("DECLINED") => Some(Answer::Declined),
        Some("TENTATIVE") => Some(Answer::Tentative),
        _ => None,
    };
    Some(Person {
        address,
        name,
        status,
    })
}

impl Invite {
    /// Whether the invitation asks `address` for an answer: it is a
    /// request, it has someone to answer to, and it is not the user's own
    /// event.
    pub fn asks(&self, address: &str) -> bool {
        self.method == Method::Request
            && self
                .organizer
                .as_ref()
                .is_some_and(|organizer| !organizer.address.eq_ignore_ascii_case(address))
    }

    /// The calendar text that tells the organizer `answer`, from the
    /// attendee at `address`. `stamp` is now, as `YYYYMMDDTHHMMSSZ`.
    ///
    /// It repeats the event's id, revision and start exactly, which is how
    /// the organizer's calendar knows which event is meant, and says
    /// nothing else about the event.
    pub fn reply(&self, answer: Answer, address: &str, name: Option<&str>, stamp: &str) -> String {
        let mut lines = vec![
            "BEGIN:VCALENDAR".to_owned(),
            "VERSION:2.0".to_owned(),
            "PRODID:-//Quill//Quill Mail//EN".to_owned(),
            "METHOD:REPLY".to_owned(),
            "BEGIN:VEVENT".to_owned(),
            format!("UID:{}", self.uid),
            format!("SEQUENCE:{}", self.sequence),
            format!("DTSTAMP:{stamp}"),
        ];
        // As written, so that a zone Quill does not know is not lost.
        lines.extend(self.raw_start.iter().map(|line| one_line(line)));
        lines.extend(self.raw_recurrence_id.iter().map(|line| one_line(line)));
        if let Some(organizer) = &self.organizer {
            lines.push(format!("ORGANIZER:mailto:{}", organizer.address));
        }
        let name = name
            .map(|name| name.replace(['"', ';', ':', ',', '\r', '\n'], " "))
            .filter(|name| !name.trim().is_empty())
            .map_or_else(String::new, |name| format!(";CN=\"{}\"", name.trim()));
        lines.push(format!(
            "ATTENDEE;PARTSTAT={}{name}:mailto:{}",
            answer.partstat(),
            one_line(address)
        ));
        if !self.summary.is_empty() {
            lines.push(format!("SUMMARY:{}", escape(&self.summary)));
        }
        lines.push("END:VEVENT".to_owned());
        lines.push("END:VCALENDAR".to_owned());
        let mut text = String::new();
        for line in lines {
            fold_into(&mut text, &line);
        }
        text
    }
}

fn one_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | ';' | ',' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Writes a line, continued on further lines if it is longer than the 75
/// bytes a calendar line may be.
fn fold_into(out: &mut String, line: &str) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const REQUEST: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Cal//EN\r\nMETHOD:REQUEST\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/London\r\nBEGIN:STANDARD\r\nDTSTART:19701025T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:offsite-2026@example.com\r\nSEQUENCE:2\r\nDTSTAMP:20261114T093000Z\r\n\
DTSTART;TZID=Europe/London:20261120T090000\r\nDTEND;TZID=Europe/London:20261120T160000\r\n\
SUMMARY:Offsite\\, day one\r\nLOCATION:Room 4\\; second floor\r\n\
DESCRIPTION:Bring the agenda.\\nLunch is provided and the room is ours until\r\n  four.\r\n\
ORGANIZER;CN=\"Ruiz, Ana\":mailto:Ana.Ruiz@example.com\r\n\
ATTENDEE;CN=Robin Vale;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:robin@example.com\r\n\
ATTENDEE;CN=Kit Moss;PARTSTAT=ACCEPTED:MAILTO:kit@example.com\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nSUMMARY:Not the event's\r\nEND:VALARM\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn an_invitation_says_what_when_where_and_who() {
        let invite = parse(REQUEST).unwrap();
        assert_eq!(invite.method, Method::Request);
        assert_eq!(invite.uid, "offsite-2026@example.com");
        assert_eq!(invite.sequence, 2);
        assert_eq!(invite.summary, "Offsite, day one");
        assert_eq!(invite.location.as_deref(), Some("Room 4; second floor"));
        // Folded lines are joined and the alarm's words are not the event's.
        assert_eq!(
            invite.description.as_deref(),
            Some("Bring the agenda.\nLunch is provided and the room is ours until four.")
        );
        assert_eq!(
            invite.start,
            Some(When {
                year: 2026,
                month: 11,
                day: 20,
                time: Some((9, 0, 0)),
                zone: Zone::Named("Europe/London".to_owned()),
            })
        );
        assert_eq!(invite.end.as_ref().unwrap().time, Some((16, 0, 0)));
        let organizer = invite.organizer.as_ref().unwrap();
        assert_eq!(organizer.address, "ana.ruiz@example.com");
        assert_eq!(organizer.name.as_deref(), Some("Ruiz, Ana"));
        assert_eq!(invite.attendees.len(), 2);
        assert_eq!(invite.attendees[0].status, None);
        assert_eq!(invite.attendees[1].status, Some(Answer::Accepted));
        assert!(!invite.repeats);
        assert!(invite.asks("robin@example.com"));
        // The organizer is not asked to answer their own invitation.
        assert!(!invite.asks("Ana.Ruiz@example.com"));
    }

    #[test]
    fn times_come_as_utc_floating_or_whole_days() {
        let event = |lines: &str| {
            parse(&format!(
                "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\n{lines}\nEND:VEVENT\nEND:VCALENDAR\n"
            ))
            .unwrap()
        };
        assert_eq!(
            event("DTSTART:20261120T140000Z").start.unwrap().zone,
            Zone::Utc
        );
        assert_eq!(
            event("DTSTART:20261120T140000").start.unwrap().zone,
            Zone::Floating
        );
        let day = event("DTSTART;VALUE=DATE:20261120\nRRULE:FREQ=YEARLY");
        assert_eq!(day.start.as_ref().unwrap().time, None);
        assert!(day.repeats);
        // A date that is not one is no date, and the event is still read.
        for bad in [
            "DTSTART:2026-11-20",
            "DTSTART:20261320T000000Z",
            "DTSTART:20261120T250000",
            "DTSTART:",
        ] {
            assert_eq!(event(bad).start, None, "{bad}");
        }
        // No METHOD is for information: nothing is asked.
        assert_eq!(event("SUMMARY:x").method, Method::Publish);
    }

    #[test]
    fn what_is_not_an_event_is_nothing() {
        for text in [
            "",
            "BEGIN:VCALENDAR\nEND:VCALENDAR\n",
            // No id: an answer could not say what it answers.
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:x\nEND:VEVENT\nEND:VCALENDAR\n",
            // Never finished.
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\n",
            "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:1\nEND:VTODO\nEND:VCALENDAR\n",
            // An id that could carry a line of its own into an answer.
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\u{1b}[2J\nEND:VEVENT\nEND:VCALENDAR\n",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
        assert_eq!(parse(&"X".repeat(MAX_CALENDAR + 1)), None);
    }

    #[test]
    fn hostile_text_is_made_clean_and_people_must_be_addresses() {
        let invite = parse(
            "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:1\n\
SUMMARY:Party\u{1b}]0;owned\u{7}\u{202e}\n\
ORGANIZER;CN=Boss\u{1b}[31m:mailto:boss@example.com\n\
ATTENDEE:mailto:a@example.com,b@example.com\n\
ATTENDEE:https://example.com/\n\
ATTENDEE:mailto:not-an-address\n\
ATTENDEE:mailto:fine@example.com\n\
END:VEVENT\nEND:VCALENDAR\n",
        )
        .unwrap();
        assert!(!invite.summary.chars().any(char::is_control));
        assert!(!invite.summary.contains('\u{202e}'));
        assert_eq!(
            invite.organizer.as_ref().unwrap().name.as_deref(),
            Some("Boss")
        );
        let addresses: Vec<&str> = invite
            .attendees
            .iter()
            .map(|person| person.address.as_str())
            .collect();
        assert_eq!(addresses, ["fine@example.com"]);
    }

    #[test]
    fn an_answer_repeats_the_event_and_says_only_the_answer() {
        let invite = parse(REQUEST).unwrap();
        let reply = invite.reply(
            Answer::Accepted,
            "robin@example.com",
            Some("Robin \"R\" Vale"),
            "20261114T100000Z",
        );
        assert!(reply.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(reply.contains("\r\nMETHOD:REPLY\r\n"));
        assert!(reply.contains("\r\nUID:offsite-2026@example.com\r\n"));
        assert!(reply.contains("\r\nSEQUENCE:2\r\n"));
        // The start exactly as it came, zone and all.
        assert!(reply.contains("\r\nDTSTART;TZID=Europe/London:20261120T090000\r\n"));
        assert!(reply.contains("\r\nORGANIZER:mailto:ana.ruiz@example.com\r\n"));
        assert!(
            reply.contains(
                "\r\nATTENDEE;PARTSTAT=ACCEPTED;CN=\"Robin  R  Vale\":mailto:robin@example.com\r\n"
            ),
            "{reply}"
        );
        assert!(reply.contains("\r\nSUMMARY:Offsite\\, day one\r\n"));
        assert!(reply.ends_with("END:VEVENT\r\nEND:VCALENDAR\r\n"));
        // Nothing of the other attendees, the place or the description.
        assert!(!reply.contains("kit@example.com") && !reply.contains("Room 4"));
        // It reads back as an answer to the same event.
        let read = parse(&reply).unwrap();
        assert_eq!(read.method, Method::Reply);
        assert_eq!(read.uid, invite.uid);
        assert_eq!(read.attendees[0].status, Some(Answer::Accepted));
        assert!(
            invite
                .reply(Answer::Declined, "robin@example.com", None, "x")
                .contains("PARTSTAT=DECLINED:mailto:")
        );
    }

    #[test]
    fn long_lines_are_folded_and_come_back_whole() {
        let mut invite = parse(REQUEST).unwrap();
        invite.summary = "A very long name for a meeting, ".repeat(6);
        let reply = invite.reply(
            Answer::Tentative,
            "robin@example.com",
            None,
            "20261114T100000Z",
        );
        assert!(reply.lines().all(|line| line.len() <= 76), "{reply}");
        assert_eq!(parse(&reply).unwrap().summary, invite.summary.trim());
    }

    #[test]
    fn the_invitation_is_found_in_a_message() {
        let raw = format!(
            "From: Ana Ruiz <ana.ruiz@example.com>\r\nTo: robin@example.com\r\nSubject: Offsite\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nYou are invited.\r\n\
--b\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\r\n{REQUEST}\r\n--b--\r\n"
        );
        let invite = find(raw.as_bytes()).unwrap();
        assert_eq!(invite.summary, "Offsite, day one");
        assert_eq!(find(b"Subject: Plain\r\n\r\nNothing here.\r\n"), None);
    }
}
