//! What the user types to search, read into what to look for.
//!
//! The syntax is the one mail programs share: words, `"exact phrases"`, and
//! `key:value` filters.
//!
//! ```text
//! from:ana has:attachment after:2026-10-01 is:unread in:work/inbox "exact phrase"
//! ```
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
}

/// Reads a query. `day_start` turns a calendar date (year, month, day) into
/// the moment that day begins for the user, or `None` if there is no such
/// date; a date it refuses is searched for as a word.
pub fn parse(input: &str, day_start: impl Fn(i32, u32, u32) -> Option<i64>) -> Query {
    let mut query = Query::default();
    let tokens = tokens(input);
    let count = tokens.len();
    for (index, token) in tokens.into_iter().enumerate() {
        // Only what is still being typed matches by prefix.
        let last = index + 1 == count;
        let term = |text: &str, phrase: bool| Term {
            text: text.to_owned(),
            phrase,
            prefix: last && token.open,
        };
        let Some((key, value, quoted)) = token.filter() else {
            if !token.text.is_empty() {
                query.text.push(term(&token.text, token.quoted));
            }
            continue;
        };
        // A filter with nothing after its colon yet filters nothing.
        if value.is_empty() {
            continue;
        }
        let lower = value.to_lowercase();
        match key.as_str() {
            "from" => query.from.push(term(value, quoted)),
            "to" | "cc" => query.to.push(term(value, quoted)),
            "subject" => query.subject.push(term(value, quoted)),
            "has" if is_prefix_of(&lower, "attachment") || lower == "file" || lower == "files" => {
                query.has_attachment = true;
            }
            "is" => match lower.as_str() {
                "unread" | "new" => query.unread = Some(true),
                "read" | "seen" => query.unread = Some(false),
                "starred" | "flagged" => query.starred = Some(true),
                "unstarred" => query.starred = Some(false),
                "answered" | "replied" => query.answered = Some(true),
                "unanswered" => query.answered = Some(false),
                // Not a state Quill knows: nothing is in it.
                _ => query.text.push(term(&token.text, false)),
            },
            "after" | "since" => match date(&lower, &day_start) {
                Some(day) => query.after = Some(day),
                None => query.text.push(term(&token.text, false)),
            },
            "before" | "until" => match date(&lower, &day_start) {
                Some(day) => query.before = Some(day),
                None => query.text.push(term(&token.text, false)),
            },
            "in" => {
                query.within = Some(match value.split_once('/') {
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
                });
            }
            _ => query.text.push(term(&token.text, token.quoted)),
        }
    }
    query
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
