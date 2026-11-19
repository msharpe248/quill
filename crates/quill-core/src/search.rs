//! What the user types to search, read into what to look for.
//!
//! The syntax is the one mail programs share: words, `"exact phrases"`, and
//! `key:value` filters.
//!
//! ```text
//! from:ana has:attachment after:2026-10-01 is:unread in:work/inbox "exact phrase"
//! -from:newsletter older:30d larger:5m account:work
//! ```
//!
//! A `-` before a word or a filter means not: `-from:ana` is mail that is
//! not from Ana, `-is:starred` mail that is not starred.
//!
//! Anything that is not a filter Quill knows is a word to find, so a typo in
//! a filter's name finds nothing rather than everything, and a colon in
//! ordinary text (a time, a URL) is harmless. Parsing never fails: a search
//! box is typed into a letter at a time, and every prefix of a query must
//! mean something.

/// A search, parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Words and phrases to find anywhere: subject, people or body.
    pub text: Vec<Term>,
    /// Words to find in the sender.
    pub from: Vec<Term>,
    /// Words to find in the recipients, To or Cc.
    pub to: Vec<Term>,
    /// Words to find in the subject.
    pub subject: Vec<Term>,
    /// Words to find in the message's text, and nowhere else (`body:`).
    /// Found only once the message has been downloaded.
    pub body: Vec<Term>,
    /// The mailing list the message came through: part of its `List-Id`
    /// (`list:builds.example.com`). Known once the message has been
    /// downloaded.
    pub list: Vec<String>,
    /// Only mail with files attached.
    pub has_attachment: bool,
    /// `Some(true)`: unread only. `Some(false)`: read only.
    pub unread: Option<bool>,
    /// `Some(true)`: starred only. `Some(false)`: unstarred only.
    pub starred: Option<bool>,
    /// Only mail that has, or has not, been answered.
    pub answered: Option<bool>,
    /// Only mail dated on or after the start of this day (seconds since the
    /// Unix epoch, UTC, as the day begins in the user's time zone).
    pub after: Option<i64>,
    /// Only mail dated before the start of this day.
    pub before: Option<i64>,
    /// Only mail in this account or folder, as typed: `work`, `inbox`, or
    /// `work/inbox`. Compared without regard to case.
    pub within: Option<Place>,
    /// Only mail of the account of this name. Unlike `within`, never a
    /// folder.
    pub account: Option<String>,
    /// Only mail dated more than this many seconds ago (`older:30d`), as
    /// counted when the search is run.
    pub older: Option<i64>,
    /// Only mail dated within this many seconds (`newer:2w`).
    pub newer: Option<i64>,
    /// Only mail bigger than this many bytes (`larger:5m`).
    pub larger: Option<i64>,
    /// Only mail smaller than this many bytes.
    pub smaller: Option<i64>,
    /// Only the conversation this message is in, by the message's id in
    /// the store (`thread:4127`).
    pub thread_of: Option<i64>,
    /// Only the message with this Message-ID, written without its angle
    /// brackets (`msgid:<abc@example.com>`).
    pub message_id: Option<String>,
    /// What the mail must not be: what was typed with a `-` before it.
    pub not: Negated,
}

/// What a search leaves out: each as the field of the same name in
/// [`Query`], turned round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Negated {
    pub text: Vec<Term>,
    pub from: Vec<Term>,
    pub to: Vec<Term>,
    pub subject: Vec<Term>,
    pub body: Vec<Term>,
    /// Not through any of these lists.
    pub list: Vec<String>,
    /// Only mail with no files attached.
    pub has_attachment: bool,
    /// Not in any of these places.
    pub within: Vec<Place>,
    /// Not of any of these accounts.
    pub accounts: Vec<String>,
}

/// A word or a phrase to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub text: String,
    /// Written in quotes: the words must appear together, in order.
    pub phrase: bool,
    /// The last thing typed, and not finished with a space or a closing
    /// quote: it matches words that merely start with it, so results show
    /// while a word is still being typed.
    pub prefix: bool,
}

/// Where to look: an account, a folder, or a folder of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub account: Option<String>,
    pub folder: Option<String>,
    /// What was typed had no slash, so it may name either.
    pub either: Option<String>,
}

impl Query {
    /// Whether there is nothing to look for: an empty box, or only spaces.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether what it asks can be told only of a message that has been
    /// downloaded: words that may be in its text, or the list it came
    /// through.
    pub fn needs_body(&self) -> bool {
        !(self.text.is_empty()
            && self.body.is_empty()
            && self.list.is_empty()
            && self.not.text.is_empty()
            && self.not.body.is_empty()
            && self.not.list.is_empty())
    }
}

/// Reads a query. `day_start` turns a calendar date (year, month, day) into
/// the moment that day begins for the user, or `None` if there is no such
/// date; a date it refuses is searched for as a word.
pub fn parse(input: &str, day_start: impl Fn(i32, u32, u32) -> Option<i64>) -> Query {
    let mut query = Query::default();
    let tokens = tokens(input);
    let count = tokens.len();
    for (index, mut token) in tokens.into_iter().enumerate() {
        // A `-` before a piece means not. Dashes alone, or one inside
        // quotes, are what they look like.
        let negated = !token.quoted
            && token.text.starts_with('-')
            && token.text.chars().any(char::is_alphanumeric);
        if negated {
            token.text.remove(0);
            token.value_quoted_at = token.value_quoted_at.map(|at| at - 1);
        }
        // Only what is still being typed matches by prefix, and only
        // what is looked for: leaving out everything that starts with a
        // half-typed word would empty the list as it is typed.
        let last = index + 1 == count;
        let term = |text: &str, phrase: bool| Term {
            text: text.to_owned(),
            phrase,
            prefix: last && token.open && !negated,
        };
        let Some((key, value, quoted)) = token.filter() else {
            if !token.text.is_empty() {
                let word = term(&token.text, token.quoted);
                if negated {
                    query.not.text.push(word);
                } else {
                    query.text.push(word);
                }
            }
            continue;
        };
        // A filter with nothing after its colon yet filters nothing.
        if value.is_empty() {
            continue;
        }
        let lower = value.to_lowercase();
        // What is not a filter Quill knows, or has a value it cannot
        // read, is a word to find, as it was typed.
        let as_word = |query: &mut Query, quoted: bool| {
            let word = term(&token.text, quoted);
            if negated {
                query.not.text.push(word);
            } else {
                query.text.push(word);
            }
        };
        match key.as_str() {
            "from" if negated => query.not.from.push(term(value, quoted)),
            "from" => query.from.push(term(value, quoted)),
            "to" | "cc" if negated => query.not.to.push(term(value, quoted)),
            "to" | "cc" => query.to.push(term(value, quoted)),
            "subject" if negated => query.not.subject.push(term(value, quoted)),
            "subject" => query.subject.push(term(value, quoted)),
            "body" if negated => query.not.body.push(term(value, quoted)),
            "body" => query.body.push(term(value, quoted)),
            "list" if negated => query.not.list.push(lower.trim().to_owned()),
            "list" => query.list.push(lower.trim().to_owned()),
            "has" if is_prefix_of(&lower, "attachment") || lower == "file" || lower == "files" => {
                if negated {
                    query.not.has_attachment = true;
                } else {
                    query.has_attachment = true;
                }
            }
            "is" => match lower.as_str() {
                "unread" | "new" => query.unread = Some(!negated),
                "read" | "seen" => query.unread = Some(negated),
                "starred" | "flagged" => query.starred = Some(!negated),
                "unstarred" => query.starred = Some(negated),
                "answered" | "replied" => query.answered = Some(!negated),
                "unanswered" => query.answered = Some(negated),
                // Not a state Quill knows: nothing is in it.
                _ => as_word(&mut query, false),
            },
            // Not after a day is before it, and so on down: a date or a
            // size left out is the other side of it.
            "after" | "since" | "before" | "until" => match date(&lower, &day_start) {
                Some(day) if matches!(key.as_str(), "after" | "since") != negated => {
                    query.after = Some(day);
                }
                Some(day) => query.before = Some(day),
                None => as_word(&mut query, false),
            },
            "older" | "newer" => match age(&lower) {
                Some(seconds) if (key == "older") != negated => query.older = Some(seconds),
                Some(seconds) => query.newer = Some(seconds),
                None => as_word(&mut query, false),
            },
            "larger" | "bigger" | "smaller" => match size(&lower) {
                Some(bytes) if (key != "smaller") != negated => query.larger = Some(bytes),
                Some(bytes) => query.smaller = Some(bytes),
                None => as_word(&mut query, false),
            },
            "in" => {
                let place = match value.split_once('/') {
                    Some((account, folder)) => Place {
                        account: Some(account.trim().to_owned()).filter(|name| !name.is_empty()),
                        folder: Some(folder.trim().to_owned()).filter(|name| !name.is_empty()),
                        either: None,
                    },
                    None => Place {
                        account: None,
                        folder: None,
                        either: Some(value.to_owned()),
                    },
                };
                if negated {
                    query.not.within.push(place);
                } else {
                    query.within = Some(place);
                }
            }
            "account" if negated => query.not.accounts.push(value.trim().to_owned()),
            "account" => query.account = Some(value.trim().to_owned()),
            "thread" if !negated => match lower.parse::<i64>() {
                Ok(id) if id > 0 => query.thread_of = Some(id),
                _ => as_word(&mut query, false),
            },
            "msgid" if !negated => {
                let id = value.trim().trim_start_matches('<').trim_end_matches('>');
                if id.is_empty() {
                    as_word(&mut query, false);
                } else {
                    query.message_id = Some(id.to_owned());
                }
            }
            _ => as_word(&mut query, token.quoted),
        }
    }
    query
}

/// An age written `30d`, `2w`, `6m` or `1y`, or in hours as `12h`, in
/// seconds. A month is thirty days and a year 365: this is for "older
/// than about", not for a calendar.
pub fn age(text: &str) -> Option<i64> {
    let (number, unit) = split_unit(text)?;
    let seconds: i64 = match unit {
        "h" => 60 * 60,
        "d" | "" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        "m" | "mo" => 30 * 24 * 60 * 60,
        "y" => 365 * 24 * 60 * 60,
        _ => return None,
    };
    number.checked_mul(seconds).filter(|age| *age > 0)
}

/// A size written `5m`, `100k`, `2g` (or `5mb`, `100kb`), or in bytes as
/// a number alone.
fn size(text: &str) -> Option<i64> {
    let (number, unit) = split_unit(text)?;
    let bytes: i64 = match unit {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return None,
    };
    number.checked_mul(bytes)
}

/// `30d` as 30 and `d`: digits, then letters, and nothing else.
fn split_unit(text: &str) -> Option<(i64, &str)> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    if !unit.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((number.parse().ok()?, unit))
}

/// `has:att` means `has:attachment` while the word is being typed.
fn is_prefix_of(typed: &str, word: &str) -> bool {
    !typed.is_empty() && word.starts_with(typed)
}

/// A date written `2026-10-01`, or `2026-10` or `2026` for the first day of
/// the month or the year.
fn date(text: &str, day_start: &impl Fn(i32, u32, u32) -> Option<i64>) -> Option<i64> {
    let mut parts = text.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = match parts.next() {
        Some(month) => month.parse().ok()?,
        None => 1,
    };
    let day: u32 = match parts.next() {
        Some(day) => day.parse().ok()?,
        None => 1,
    };
    if parts.next().is_some() || !(1..=9999).contains(&year) {
        return None;
    }
    day_start(year, month, day)
}

/// One space-separated piece of the input.
#[derive(Debug, PartialEq, Eq)]
struct Token {
    /// The piece as typed, without the quotes around a quoted one.
    text: String,
    /// The whole piece was in quotes.
    quoted: bool,
    /// Nothing ended it: the input stopped in the middle of it.
    open: bool,
    /// Where a quoted value begins, for `from:"Ana Ruiz"`: the piece up to
    /// and including the colon is the key.
    value_quoted_at: Option<usize>,
}

impl Token {
    /// The piece as a `key:value` filter: the key in lower case, the value,
    /// and whether the value was quoted. `None` for a piece with no key, or
    /// one that was quoted as a whole (a quoted `"from:ana"` is a phrase).
    fn filter(&self) -> Option<(String, &str, bool)> {
        if self.quoted {
            return None;
        }
        let (key, value) = self.text.split_once(':')?;
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let quoted = self.value_quoted_at == Some(key.len() + 1);
        Some((key.to_ascii_lowercase(), value, quoted))
    }
}

/// Splits the input at spaces, keeping what is in double quotes together. A
/// quote may open a whole piece or the value of a filter.
fn tokens(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut in_quotes = false;
    let mut quoted = false;
    let mut value_quoted_at = None;
    let mut started = false;
    let finish = |text: &mut String,
                  quoted: &mut bool,
                  value_quoted_at: &mut Option<usize>,
                  started: &mut bool,
                  open: bool,
                  out: &mut Vec<Token>| {
        if *started {
            out.push(Token {
                text: std::mem::take(text),
                quoted: *quoted,
                open,
                value_quoted_at: value_quoted_at.take(),
            });
        }
        *quoted = false;
        *started = false;
    };
    for c in input.chars() {
        match c {
            '"' if in_quotes => {
                in_quotes = false;
                // A closing quote ends the piece, whatever follows.
                finish(
                    &mut text,
                    &mut quoted,
                    &mut value_quoted_at,
                    &mut started,
                    false,
                    &mut out,
                );
            }
            '"' if !started => {
                in_quotes = true;
                quoted = true;
                started = true;
            }
            '"' if text.ends_with(':') => {
                in_quotes = true;
                value_quoted_at = Some(text.len());
            }
            c if c.is_whitespace() && !in_quotes => finish(
                &mut text,
                &mut quoted,
                &mut value_quoted_at,
                &mut started,
                false,
                &mut out,
            ),
            c => {
                text.push(c);
                started = true;
            }
        }
    }
    finish(
        &mut text,
        &mut quoted,
        &mut value_quoted_at,
        &mut started,
        true,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dash_before_a_piece_means_not() {
        let query = parse("roadmap -draft -from:ana -subject:\"weekly digest\" ", day);
        assert_eq!(query.text, [word("roadmap")]);
        assert_eq!(query.not.text, [word("draft")]);
        assert_eq!(query.not.from, [word("ana")]);
        assert_eq!(
            query.not.subject,
            [Term {
                text: "weekly digest".to_owned(),
                phrase: true,
                prefix: false,
            }]
        );
        // A state left out is the other state.
        assert_eq!(parse("-is:unread", day).unread, Some(false));
        assert_eq!(parse("-is:starred ", day).starred, Some(false));
        assert_eq!(parse("-is:unanswered ", day).answered, Some(true));
        assert!(parse("-has:attachment ", day).not.has_attachment);
        // A date left out is the other side of it.
        assert_eq!(parse("-after:2026-10-01 ", day).before, Some(20_261_001));
        assert_eq!(parse("-before:2026-10-01 ", day).after, Some(20_261_001));
        // A place left out, and an account.
        let query = parse("-in:work/sent -in:trash -account:personal ", day);
        assert_eq!(query.within, None);
        assert_eq!(query.not.within.len(), 2);
        assert_eq!(query.not.accounts, ["personal"]);
        // What is still being typed leaves nothing out by its first
        // letters.
        assert!(!parse("-dra", day).not.text[0].prefix);
        // Dashes alone, and a quoted dash, are what they look like.
        assert_eq!(parse("-- ", day).text, [word("--")]);
        assert_eq!(parse("- ", day).text, [word("-")]);
        assert_eq!(parse("\"-draft\" ", day).not, Negated::default());
        // In the middle of a word it is part of the word.
        assert_eq!(parse("e-mail ", day).text, [word("e-mail")]);
    }

    #[test]
    fn the_body_and_the_list_have_filters_of_their_own() {
        let query = parse(
            "body:unsubscribe list:Builds.Example.com -body:invoice -list:announce ",
            day,
        );
        assert_eq!(query.body.len(), 1);
        assert_eq!(query.body[0].text, "unsubscribe");
        assert_eq!(query.list, vec!["builds.example.com".to_owned()]);
        assert_eq!(query.not.body[0].text, "invoice");
        assert_eq!(query.not.list, vec!["announce".to_owned()]);
        assert!(query.text.is_empty());
        assert!(query.needs_body());
        // Who it is from and its subject are known as it arrives.
        assert!(!parse("from:ana subject:plan is:unread ", day).needs_body());
        // A bare word may be in the text.
        assert!(parse("from:ana roadmap ", day).needs_body());
    }

    #[test]
    fn ages_and_sizes_are_read_with_their_units() {
        const DAY: i64 = 24 * 60 * 60;
        assert_eq!(parse("older:30d ", day).older, Some(30 * DAY));
        assert_eq!(parse("older:2w ", day).older, Some(14 * DAY));
        assert_eq!(parse("newer:12h ", day).newer, Some(12 * 60 * 60));
        assert_eq!(parse("older:6m ", day).older, Some(180 * DAY));
        assert_eq!(parse("older:1y ", day).older, Some(365 * DAY));
        // A number alone is days.
        assert_eq!(parse("newer:7 ", day).newer, Some(7 * DAY));
        // Not older is newer.
        assert_eq!(parse("-older:30d ", day).newer, Some(30 * DAY));
        assert_eq!(parse("larger:5m ", day).larger, Some(5 * 1024 * 1024));
        assert_eq!(parse("larger:100kb ", day).larger, Some(100 * 1024));
        assert_eq!(parse("smaller:900 ", day).smaller, Some(900));
        assert_eq!(parse("-larger:1g ", day).smaller, Some(1024 * 1024 * 1024));
        // What cannot be read is a word, and nothing is filtered by it.
        for odd in [
            "older:soon ",
            "older:0d ",
            "older:3x ",
            "larger:big ",
            "older:9999999999999y ",
        ] {
            let query = parse(odd, day);
            assert_eq!((query.older, query.larger), (None, None), "{odd}");
            assert_eq!(query.text.len(), 1, "{odd}");
        }
    }

    #[test]
    fn an_account_a_conversation_and_a_message_are_named() {
        assert_eq!(parse("account:Work ", day).account.as_deref(), Some("Work"));
        assert_eq!(parse("thread:4127 ", day).thread_of, Some(4127));
        assert_eq!(parse("thread:x ", day).thread_of, None);
        assert_eq!(
            parse("msgid:<abc@example.com> ", day).message_id.as_deref(),
            Some("abc@example.com")
        );
        assert_eq!(
            parse("msgid:abc@example.com ", day).message_id.as_deref(),
            Some("abc@example.com")
        );
    }

    /// Days counted from a made-up epoch, so the tests need no calendar.
    fn day(year: i32, month: u32, day: u32) -> Option<i64> {
        ((1..=12).contains(&month) && (1..=31).contains(&day))
            .then(|| i64::from(year) * 10_000 + i64::from(month) * 100 + i64::from(day))
    }

    fn word(text: &str) -> Term {
        Term {
            text: text.to_owned(),
            phrase: false,
            prefix: false,
        }
    }

    fn typing(text: &str) -> Term {
        Term {
            prefix: true,
            ..word(text)
        }
    }

    fn phrase(text: &str) -> Term {
        Term {
            phrase: true,
            ..word(text)
        }
    }

    #[test]
    fn the_spec_example_reads_as_written() {
        let query = parse(
            "from:ana has:attachment after:2026-10-01 is:unread in:work/inbox \"exact phrase\"",
            day,
        );
        assert_eq!(
            query,
            Query {
                text: vec![phrase("exact phrase")],
                from: vec![word("ana")],
                has_attachment: true,
                unread: Some(true),
                after: Some(20_261_001),
                within: Some(Place {
                    account: Some("work".to_owned()),
                    folder: Some("inbox".to_owned()),
                    either: None,
                }),
                ..Query::default()
            }
        );
    }

    #[test]
    fn plain_words_are_looked_for_anywhere() {
        let query = parse("roadmap  Q4 ", day);
        assert_eq!(query.text, [word("roadmap"), word("Q4")]);
        assert!(!query.is_empty());
        assert!(parse("", day).is_empty());
        assert!(parse("   \t ", day).is_empty());
    }

    #[test]
    fn what_is_still_being_typed_matches_by_prefix() {
        // The last word, not yet ended by a space.
        assert_eq!(parse("road", day).text, [typing("road")]);
        assert_eq!(parse("q4 road", day).text, [word("q4"), typing("road")]);
        // A space ends it.
        assert_eq!(parse("road ", day).text, [word("road")]);
        // So does a closing quote; an open one does not.
        assert_eq!(parse("\"q4 road\"", day).text, [phrase("q4 road")]);
        assert_eq!(
            parse("\"q4 road", day).text,
            [Term {
                prefix: true,
                ..phrase("q4 road")
            }]
        );
        // In a filter too.
        assert_eq!(parse("from:an", day).from, [typing("an")]);
        assert_eq!(parse("from:an x", day).from, [word("an")]);
    }

    #[test]
    fn filters_take_quoted_values_and_any_case() {
        let query = parse("FROM:\"Ana Ruiz\" Subject:roadmap to:sam cc:priya", day);
        assert_eq!(query.from, [phrase("Ana Ruiz")]);
        assert_eq!(query.subject, [word("roadmap")]);
        assert_eq!(query.to, [word("sam"), typing("priya")]);
        assert!(query.text.is_empty());
    }

    #[test]
    fn states_and_their_opposites() {
        let query = parse("is:read is:starred is:unanswered has:files", day);
        assert_eq!(
            (
                query.unread,
                query.starred,
                query.answered,
                query.has_attachment
            ),
            (Some(false), Some(true), Some(false), true)
        );
        let query = parse("is:new is:unstarred is:replied has:att", day);
        assert_eq!(
            (
                query.unread,
                query.starred,
                query.answered,
                query.has_attachment
            ),
            (Some(true), Some(false), Some(true), true)
        );
    }

    #[test]
    fn dates_may_be_a_day_a_month_or_a_year() {
        assert_eq!(parse("after:2026-10-01", day).after, Some(20_261_001));
        assert_eq!(parse("since:2026-10", day).after, Some(20_261_001));
        assert_eq!(parse("before:2027", day).before, Some(20_270_101));
        assert_eq!(parse("until:2026-12-31", day).before, Some(20_261_231));
        // Not a date: looked for as it stands, so it finds nothing rather
        // than being quietly dropped.
        for bad in [
            "after:tuesday",
            "after:2026-13-01",
            "before:2026-1-2-3",
            "after:0",
        ] {
            let query = parse(bad, day);
            assert_eq!((query.after, query.before), (None, None), "{bad}");
            assert_eq!(query.text.len(), 1, "{bad}");
        }
    }

    #[test]
    fn a_place_is_an_account_a_folder_or_both() {
        let place = |text: &str| parse(text, day).within;
        assert_eq!(
            place("in:work"),
            Some(Place {
                account: None,
                folder: None,
                either: Some("work".to_owned())
            })
        );
        assert_eq!(
            place("in:Work/Sent"),
            Some(Place {
                account: Some("Work".to_owned()),
                folder: Some("Sent".to_owned()),
                either: None
            })
        );
        // A folder with a space in its name, quoted.
        assert_eq!(
            place("in:\"Personal/Trail runs\""),
            Some(Place {
                account: Some("Personal".to_owned()),
                folder: Some("Trail runs".to_owned()),
                either: None
            })
        );
        assert_eq!(
            place("in:/archive"),
            Some(Place {
                account: None,
                folder: Some("archive".to_owned()),
                either: None
            })
        );
    }

    #[test]
    fn what_is_not_a_filter_is_a_word() {
        // An unknown key, a time, an address with a port, a quoted filter.
        for (input, expected) in [
            ("form:ana", "form:ana"),
            ("12:30", "12:30"),
            ("https://example.com/a", "https://example.com/a"),
            (":colon", ":colon"),
            ("is:purple", "is:purple"),
            ("has:wings", "has:wings"),
        ] {
            let query = parse(&format!("{input} "), day);
            assert_eq!(query.text, [word(expected)], "{input}");
            assert_eq!(
                Query {
                    text: Vec::new(),
                    ..query
                },
                Query::default(),
                "{input}"
            );
        }
        assert_eq!(parse("\"from:ana\"", day).text, [phrase("from:ana")]);
    }

    #[test]
    fn a_filter_with_nothing_after_it_yet_filters_nothing() {
        for input in ["from:", "is:", "after:", "in:", "has:", "from:\"\""] {
            assert!(parse(input, day).is_empty(), "{input}");
        }
    }

    #[test]
    fn every_prefix_of_a_query_parses() {
        let whole = "from:\"Ana Ruiz\" has:attachment after:2026-10-01 is:unread in:work/inbox \"exact phrase\" ünïcödé 🎉";
        let mut end = 0;
        for c in whole.chars() {
            end += c.len_utf8();
            let _ = parse(&whole[..end], day);
        }
        // Stray quotes and colons in every arrangement.
        for input in [
            "\"", "\"\"", "\"\"\"", "a\"b", "a:\"", ":::", "a:b:c", "\" \" \"",
        ] {
            let _ = parse(input, day);
        }
    }
}
