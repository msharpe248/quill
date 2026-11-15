//! The card the reader shows for a calendar invitation: what, when, where,
//! who is asking, and what is being asked.
//!
//! `quill_mime::invite` reads the invitation; this says it in the user's
//! own time zone. Everything in an invitation is a stranger's text, so
//! every piece is made safe to draw here, whatever was done to it before.
//!
//! It is done in two steps, because the UI task never touches disk and
//! looking a time zone up by name does. [`Invitation::find`] runs on the
//! store worker and places the event's times; [`card`] runs in `update`
//! and only does arithmetic.

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use quill_mime::invite::{Answer, Invite, Method, Person, When, Zone};

use crate::text;

/// One row of the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardRow {
    /// The rule across the top, naming what kind of thing this is.
    Title(&'static str),
    /// A labelled line, as the headers above it are.
    Field { label: &'static str, text: String },
    /// What is asked of the user, or what has happened.
    Note(String),
}

/// The keys that answer an invitation, as they are to be named: accept,
/// decline, maybe.
pub type AnswerKeys = [String; 3];

/// An invitation as the store worker hands it over: what the message
/// says, and where its times fall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    pub invite: Invite,
    pub start: Option<Moment>,
    pub end: Option<Moment>,
}

impl Invitation {
    /// The invitation a raw message carries, if it carries one. Looks up
    /// the zones its times are given in, which reads the system's zone
    /// files: for the store worker, not the UI task.
    pub fn find(raw: &[u8]) -> Option<Self> {
        quill_mime::invite::find(raw).map(Self::of)
    }

    pub fn of(invite: Invite) -> Self {
        Self {
            start: invite.start.as_ref().map(moment),
            end: invite.end.as_ref().map(moment),
            invite,
        }
    }
}

/// The card for an invitation, with times in `tz`. `mine` is the address
/// the message came to, which decides whether an answer is asked of the
/// user; `keys` are what to press to give one, when they are bound.
pub fn card(
    invitation: &Invitation,
    tz: &TimeZone,
    mine: Option<&str>,
    keys: Option<&AnswerKeys>,
) -> Vec<CardRow> {
    let invite = &invitation.invite;
    let field = |label, text: String| CardRow::Field { label, text };
    let mut rows = vec![CardRow::Title(match invite.method {
        Method::Request => "Invitation",
        Method::Cancel => "Cancelled event",
        Method::Reply => "Answer to an invitation",
        Method::Publish => "Event",
    })];
    let summary = text::sanitize_line(&invite.summary);
    rows.push(field(
        "What:",
        if summary.trim().is_empty() {
            "(no title)".to_owned()
        } else {
            summary
        },
    ));
    if let Some(start) = &invitation.start {
        let mut when = when_line(start, invitation.end.as_ref(), tz);
        if invite.repeats {
            when.push_str(", repeats");
        }
        rows.push(field("When:", when));
    } else if invite.repeats {
        rows.push(field("When:", "repeats".to_owned()));
    }
    if let Some(place) = invite
        .location
        .as_deref()
        .map(text::sanitize_line)
        .filter(|place| !place.trim().is_empty())
    {
        rows.push(field("Where:", place));
    }
    if let Some(organizer) = &invite.organizer {
        rows.push(field("Host:", person(organizer)));
    }
    if !invite.attendees.is_empty() {
        rows.push(field("Invited:", invited(&invite.attendees)));
    }
    rows.push(CardRow::Note(match invite.method {
        Method::Request if mine.is_some_and(|mine| invite.asks(mine)) => match keys {
            // Short enough to be read whole in a pane 60 columns wide.
            Some([accept, decline, maybe]) => {
                format!("Asks for an answer: {accept} accepts, {decline} declines, {maybe} maybe.")
            }
            None => "Asks for an answer.".to_owned(),
        },
        Method::Request if invite.organizer.is_some() => "This is your own invitation.".to_owned(),
        Method::Request => "It names nobody to answer.".to_owned(),
        Method::Cancel => "This event was cancelled.".to_owned(),
        Method::Reply => answered(invite),
        Method::Publish => "For your information: no answer is asked.".to_owned(),
    }));
    rows
}

/// A person as the headers write one: `Ana Ruiz <ana.ruiz@example.com>`.
fn person(person: &Person) -> String {
    let address = text::sanitize_line(&person.address);
    match person
        .name
        .as_deref()
        .map(text::sanitize_line)
        .filter(|name| !name.trim().is_empty())
    {
        Some(name) => format!("{} <{address}>", name.trim()),
        None => address,
    }
}

/// What a person is called in a sentence: their name, or their address.
fn called(person: &Person) -> String {
    person
        .name
        .as_deref()
        .map(text::sanitize_line)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| text::sanitize_line(&person.address))
}

/// How many are invited, and how many of them have said what.
fn invited(people: &[Person]) -> String {
    let count = |answer: Option<Answer>| people.iter().filter(|p| p.status == answer).count();
    let said: Vec<String> = [
        (count(Some(Answer::Accepted)), "accepted"),
        (count(Some(Answer::Declined)), "declined"),
        (count(Some(Answer::Tentative)), "maybe"),
        (count(None), "not answered"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect();
    format!(
        "{} {}: {}",
        people.len(),
        if people.len() == 1 {
            "person"
        } else {
            "people"
        },
        said.join(", ")
    )
}

/// Someone's answer to an invitation of the user's, in a sentence.
fn answered(invite: &Invite) -> String {
    // An answer names the one who gives it, and nobody else.
    let Some(who) = invite.attendees.first() else {
        return "Someone answered, and the answer does not say who.".to_owned();
    };
    let name = called(who);
    match who.status {
        Some(Answer::Accepted) => format!("{name} accepted."),
        Some(Answer::Declined) => format!("{name} declined."),
        Some(Answer::Tentative) => format!("{name} answered maybe."),
        None => format!("{name} answered, without saying yes or no."),
    }
}

/// A moment of an invitation, as far as it can be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moment {
    /// A whole day.
    Day(Date),
    /// A moment that is the same everywhere, in seconds since the Unix
    /// epoch.
    At(i64),
    /// A time with no zone given: whatever the clock says where the
    /// reader is.
    Clock(DateTime),
    /// A time on the clock of a zone Quill does not know, with the zone's
    /// name as the invitation wrote it.
    Elsewhere(DateTime, String),
    /// A day that is not in the calendar.
    Unreadable,
}

/// Only what a zone's name is made of is looked up: a name is a
/// stranger's text, and is not to be read as a path.
fn known_zone(name: &str) -> Option<TimeZone> {
    let plain = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'));
    if !plain {
        return None;
    }
    TimeZone::get(name).ok()
}

/// Places a time of an invitation. A zone given by name is looked up.
fn moment(when: &When) -> Moment {
    let Ok(date) = Date::new(when.year, when.month, when.day) else {
        return Moment::Unreadable;
    };
    let Some((hour, minute, second)) = when.time else {
        return Moment::Day(date);
    };
    let clock = date.at(hour, minute, second.clamp(0, 59), 0);
    let at = |zone: TimeZone| {
        clock.to_zoned(zone).map_or(Moment::Unreadable, |at| {
            Moment::At(at.timestamp().as_second())
        })
    };
    match &when.zone {
        Zone::Utc => at(TimeZone::UTC),
        Zone::Floating => Moment::Clock(clock),
        Zone::Named(name) => match known_zone(name) {
            Some(zone) => at(zone),
            None => Moment::Elsewhere(clock, text::sanitize_line(name)),
        },
    }
}

/// A placed moment as the reader's own clock shows it, where it can.
enum Shown {
    Day(Date),
    Local(DateTime),
    Elsewhere(DateTime, String),
}

fn shown(moment: &Moment, tz: &TimeZone) -> Option<Shown> {
    Some(match moment {
        Moment::Day(day) => Shown::Day(*day),
        Moment::At(seconds) => Shown::Local(
            jiff::Timestamp::from_second(*seconds)
                .ok()?
                .to_zoned(tz.clone())
                .datetime(),
        ),
        Moment::Clock(clock) => Shown::Local(*clock),
        Moment::Elsewhere(clock, zone) => Shown::Elsewhere(*clock, text::sanitize_line(zone)),
        Moment::Unreadable => return None,
    })
}

const DAY: &str = "%a %d %b %Y";
const TIME: &str = "%H:%M";

/// When an event is: `Fri 20 Nov 2026 09:00 to 16:00`, a day or a run of
/// days for an all-day event, and the zone's name after a time that could
/// not be brought to the user's own.
fn when_line(start: &Moment, end: Option<&Moment>, tz: &TimeZone) -> String {
    let Some(from) = shown(start, tz) else {
        return "a date Quill cannot read".to_owned();
    };
    let to = end.and_then(|end| shown(end, tz));
    match (from, to) {
        (Shown::Day(first), to) => {
            // An all-day event ends on the morning after its last day.
            let last = match to {
                Some(Shown::Day(end)) => end.yesterday().ok().filter(|last| *last > first),
                _ => None,
            };
            match last {
                Some(last) => format!("{} to {}, all day", first.strftime(DAY), last.strftime(DAY)),
                None => format!("{}, all day", first.strftime(DAY)),
            }
        }
        (Shown::Local(from), Some(Shown::Local(to))) => span(from, to),
        (Shown::Elsewhere(from, zone), Some(Shown::Elsewhere(to, other))) if zone == other => {
            format!("{} ({zone})", span(from, to))
        }
        (from, to) => {
            let one = |shown: Shown| match shown {
                Shown::Day(day) => day.strftime(DAY).to_string(),
                Shown::Local(at) => format!("{} {}", at.strftime(DAY), at.strftime(TIME)),
                Shown::Elsewhere(at, zone) => {
                    format!("{} {} ({zone})", at.strftime(DAY), at.strftime(TIME))
                }
            };
            match to {
                Some(to) => format!("{} to {}", one(from), one(to)),
                None => one(from),
            }
        }
    }
}

/// Two times on one clock: the day once when both are on it.
fn span(from: DateTime, to: DateTime) -> String {
    let start = format!("{} {}", from.strftime(DAY), from.strftime(TIME));
    if to <= from {
        start
    } else if to.date() == from.date() {
        format!("{start} to {}", to.strftime(TIME))
    } else {
        format!("{start} to {} {}", to.strftime(DAY), to.strftime(TIME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i16, month: i8, day: i8, time: Option<(i8, i8, i8)>, zone: Zone) -> When {
        When {
            year,
            month,
            day,
            time,
            zone,
        }
    }

    fn london() -> TimeZone {
        TimeZone::get("Europe/London").unwrap()
    }

    /// The line for times as an invitation wrote them, read in `tz`.
    fn when_line(start: &When, end: Option<&When>, tz: &TimeZone) -> String {
        super::when_line(&moment(start), end.map(moment).as_ref(), tz)
    }

    #[test]
    fn times_are_said_in_the_readers_own_zone() {
        let new_york = TimeZone::get("America/New_York").unwrap();
        // Nine in London in November is four in the morning in New York.
        let start = at(
            2026,
            11,
            20,
            Some((9, 0, 0)),
            Zone::Named("Europe/London".to_owned()),
        );
        let end = at(
            2026,
            11,
            20,
            Some((16, 0, 0)),
            Zone::Named("Europe/London".to_owned()),
        );
        assert_eq!(
            when_line(&start, Some(&end), &new_york),
            "Fri 20 Nov 2026 04:00 to 11:00"
        );
        assert_eq!(
            when_line(&start, Some(&end), &london()),
            "Fri 20 Nov 2026 09:00 to 16:00"
        );
        // A time given in UTC, in summer, is an hour later in London.
        let utc = at(2026, 7, 1, Some((23, 30, 0)), Zone::Utc);
        assert_eq!(when_line(&utc, None, &london()), "Thu 02 Jul 2026 00:30");
        // One that runs past midnight names both days.
        let late = at(2026, 7, 2, Some((1, 0, 0)), Zone::Utc);
        assert_eq!(
            when_line(&utc, Some(&late), &TimeZone::UTC),
            "Wed 01 Jul 2026 23:30 to Thu 02 Jul 2026 01:00"
        );
        // No zone at all is the clock on the reader's own wall.
        let floating = at(2026, 11, 20, Some((9, 0, 0)), Zone::Floating);
        assert_eq!(
            when_line(&floating, None, &new_york),
            "Fri 20 Nov 2026 09:00"
        );
    }

    #[test]
    fn whole_days_are_dates_and_an_end_is_the_morning_after() {
        let day = |day| at(2026, 11, day, None, Zone::Floating);
        assert_eq!(
            when_line(&day(20), Some(&day(21)), &london()),
            "Fri 20 Nov 2026, all day"
        );
        assert_eq!(
            when_line(&day(20), None, &london()),
            "Fri 20 Nov 2026, all day"
        );
        assert_eq!(
            when_line(&day(20), Some(&day(23)), &london()),
            "Fri 20 Nov 2026 to Sun 22 Nov 2026, all day"
        );
    }

    #[test]
    fn a_zone_quill_does_not_know_is_named_beside_its_own_clock() {
        let windows = |hour| {
            at(
                2026,
                11,
                20,
                Some((hour, 0, 0)),
                Zone::Named("W. Europe Standard Time".to_owned()),
            )
        };
        assert_eq!(
            when_line(&windows(9), Some(&windows(10)), &london()),
            "Fri 20 Nov 2026 09:00 to 10:00 (W. Europe Standard Time)"
        );
        // A name that is a path, or dressed up, is not looked up at all.
        for name in [
            "../../etc/passwd",
            "/etc/localtime",
            "Europe/London\u{1b}[2J",
        ] {
            let hostile = at(2026, 11, 20, Some((9, 0, 0)), Zone::Named(name.to_owned()));
            let line = when_line(&hostile, None, &london());
            assert!(line.starts_with("Fri 20 Nov 2026 09:00 ("), "{line}");
            assert!(!line.contains('\u{1b}'), "{line:?}");
        }
        // A day that is not in the calendar is said to be unreadable.
        let never = at(2026, 2, 31, Some((9, 0, 0)), Zone::Utc);
        assert_eq!(
            when_line(&never, None, &london()),
            "a date Quill cannot read"
        );
    }
}
