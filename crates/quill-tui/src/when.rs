//! Reading a time as a person types it: "2h", "tonight", "tomorrow 9",
//! "fri 14:30", "2026-12-01". For snoozing mail and for sending later.

use jiff::civil::{Date, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

/// When a day begins for these purposes: "tomorrow" means tomorrow morning.
const MORNING: i8 = 8;
/// And when its evening does: "tonight".
const EVENING: i8 = 18;

/// Examples to show beside the box.
pub const EXAMPLES: &str = "30m  2h  3d  tonight  tomorrow  fri 14:30  2026-12-01 9:00";

/// The moment `text` names, in seconds since the Unix epoch, as someone in
/// `tz` at `now` would mean it. `None` when it names none, or one that is
/// not in the future.
pub fn parse(text: &str, now: i64, tz: &TimeZone) -> Option<i64> {
    let text = text.trim().to_lowercase();
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let second = words.next();
    if words.next().is_some() {
        return None;
    }
    let today = Timestamp::from_second(now).ok()?.to_zoned(tz.clone());
    let at = |date: Date, time: Time| -> Option<i64> {
        Some(
            date.to_datetime(time)
                .to_zoned(tz.clone())
                .ok()?
                .timestamp()
                .as_second(),
        )
    };
    let clock = |hour: i8| Time::new(hour, 0, 0, 0).ok();

    let moment = if let Some(seconds) = second.is_none().then(|| span(first)).flatten() {
        now.checked_add(seconds)?
    } else if first == "tonight" && second.is_none() {
        let evening = at(today.date(), clock(EVENING)?)?;
        if evening > now {
            evening
        } else {
            at(today.date().tomorrow().ok()?, clock(EVENING)?)?
        }
    } else if first == "tomorrow" {
        let time = match second {
            Some(time) => time_of_day(time)?,
            None => clock(MORNING)?,
        };
        at(today.date().tomorrow().ok()?, time)?
    } else if let Some(weekday) = weekday(first) {
        let time = match second {
            Some(time) => time_of_day(time)?,
            None => clock(MORNING)?,
        };
        // The next such day, never today: "fri" said on a Friday is a
        // week off.
        let mut date = today.date().tomorrow().ok()?;
        while date.weekday() != weekday {
            date = date.tomorrow().ok()?;
        }
        at(date, time)?
    } else if let Ok(date) = first.parse::<Date>() {
        let time = match second {
            Some(time) => time_of_day(time)?,
            None => clock(MORNING)?,
        };
        at(date, time)?
    } else if second.is_none() {
        // A time alone: today if it is still to come, otherwise tomorrow.
        let time = time_of_day(first)?;
        let same_day = at(today.date(), time)?;
        if same_day > now {
            same_day
        } else {
            at(today.date().checked_add(1.day()).ok()?, time)?
        }
    } else {
        return None;
    };
    (moment > now).then_some(moment)
}

/// "30m", "2h", "3d", "1w": that long, in seconds.
fn span(word: &str) -> Option<i64> {
    let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &word[digits.len()..];
    let count: i64 = digits.parse().ok().filter(|count| *count > 0)?;
    let seconds = match unit {
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        "w" | "wk" | "week" | "weeks" => 7 * 24 * 60 * 60,
        _ => return None,
    };
    count.checked_mul(seconds)
}

/// "9", "9:30", "14:05", "9am", "3pm", "3:15pm".
fn time_of_day(word: &str) -> Option<Time> {
    let (clock, half) = if let Some(clock) = word.strip_suffix("am") {
        (clock, Some(false))
    } else if let Some(clock) = word.strip_suffix("pm") {
        (clock, Some(true))
    } else {
        (word, None)
    };
    let (hour, minute) = match clock.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => (hour, minute.parse::<i8>().ok()?),
        Some(_) => return None,
        None => (clock, 0),
    };
    let mut hour: i8 = hour.parse().ok()?;
    match half {
        Some(afternoon) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = match (hour, afternoon) {
                (12, false) => 0,
                (12, true) => 12,
                (hour, true) => hour + 12,
                (hour, false) => hour,
            };
        }
        None if !(0..=23).contains(&hour) => return None,
        None => {}
    }
    Time::new(hour, minute, 0, 0).ok()
}

fn weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "mon" | "monday" => Weekday::Monday,
        "tue" | "tues" | "tuesday" => Weekday::Tuesday,
        "wed" | "wednesday" => Weekday::Wednesday,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thursday,
        "fri" | "friday" => Weekday::Friday,
        "sat" | "saturday" => Weekday::Saturday,
        "sun" | "sunday" => Weekday::Sunday,
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Saturday 2026-11-14T09:30:00Z.
    const NOW: i64 = 1_794_648_600;

    fn utc(text: &str) -> Option<String> {
        parse(text, NOW, &TimeZone::UTC).map(|at| Timestamp::from_second(at).unwrap().to_string())
    }

    #[test]
    fn so_long_from_now() {
        assert_eq!(utc("30m").unwrap(), "2026-11-14T10:00:00Z");
        assert_eq!(utc("2h").unwrap(), "2026-11-14T11:30:00Z");
        assert_eq!(utc(" 3D ").unwrap(), "2026-11-17T09:30:00Z");
        assert_eq!(utc("1w").unwrap(), "2026-11-21T09:30:00Z");
        assert_eq!(utc("2 h"), None);
    }

    #[test]
    fn days_by_name() {
        assert_eq!(utc("tonight").unwrap(), "2026-11-14T18:00:00Z");
        assert_eq!(utc("tomorrow").unwrap(), "2026-11-15T08:00:00Z");
        assert_eq!(utc("tomorrow 14:30").unwrap(), "2026-11-15T14:30:00Z");
        assert_eq!(utc("tomorrow 9pm").unwrap(), "2026-11-15T21:00:00Z");
        // It is Saturday: Monday is the 16th, and Saturday is a week off.
        assert_eq!(utc("mon").unwrap(), "2026-11-16T08:00:00Z");
        assert_eq!(utc("Friday 14").unwrap(), "2026-11-20T14:00:00Z");
        assert_eq!(utc("sat").unwrap(), "2026-11-21T08:00:00Z");
    }

    #[test]
    fn a_time_alone_is_today_if_it_is_still_to_come() {
        assert_eq!(utc("17:45").unwrap(), "2026-11-14T17:45:00Z");
        assert_eq!(utc("8").unwrap(), "2026-11-15T08:00:00Z");
        assert_eq!(utc("12am").unwrap(), "2026-11-15T00:00:00Z");
        assert_eq!(utc("12pm").unwrap(), "2026-11-14T12:00:00Z");
    }

    #[test]
    fn dates_and_what_is_not_a_time() {
        assert_eq!(utc("2026-12-01").unwrap(), "2026-12-01T08:00:00Z");
        assert_eq!(utc("2026-12-01 9:15").unwrap(), "2026-12-01T09:15:00Z");
        // Not in the future, or not a time at all.
        for text in [
            "2026-11-01",
            "",
            "soon",
            "0m",
            "25:00",
            "9:5",
            "13pm",
            "tomorrow morning early",
            "2026-13-40",
            "-2h",
        ] {
            assert_eq!(utc(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_day_is_the_users_not_utcs() {
        // 09:30 UTC is 04:30 in New York: tonight is 18:00 there, 23:00 UTC.
        let tz = TimeZone::get("America/New_York").unwrap();
        let at = parse("tonight", NOW, &tz).unwrap();
        assert_eq!(
            Timestamp::from_second(at).unwrap().to_string(),
            "2026-11-14T23:00:00Z"
        );
        let at = parse("tomorrow", NOW, &tz).unwrap();
        assert_eq!(
            Timestamp::from_second(at).unwrap().to_string(),
            "2026-11-15T13:00:00Z"
        );
    }
}
