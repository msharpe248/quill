//! Finding mail: a parsed search run against the index and the message
//! table.
//!
//! Words go to the full-text index; states, dates and places are ordinary
//! conditions on the message. Everything the user typed reaches SQL as a
//! bound value or, for the full-text expression, as quoted text: nothing
//! typed is ever read as SQL or as full-text syntax.

use quill_core::search::{Query, Term};
use rusqlite::types::Value;

use crate::{MessageRow, Result, Store, message_row};

impl Store {
    /// The messages that match `query`, newest first, at most `limit`. An
    /// empty query matches nothing: a search box with nothing in it is not
    /// a request for every message there is.
    pub fn search(&self, query: &Query, limit: usize) -> Result<Vec<MessageRow>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut conditions = vec!["m.hidden = 0".to_owned()];
        let mut values: Vec<Value> = Vec::new();
        let mut bind = |value: Value, values: &mut Vec<Value>| -> String {
            values.push(value);
            format!("?{}", values.len())
        };

        let expression = match_expression(query);
        match expression {
            Some(expression) => {
                let place = bind(Value::Text(expression), &mut values);
                conditions.push(format!(
                    "m.id IN (SELECT rowid FROM message_search WHERE message_search MATCH {place})"
                ));
            }
            // Words were typed and none of them is a word the index could
            // hold (punctuation alone): they match nothing.
            None if has_terms(query) => return Ok(Vec::new()),
            None => {}
        }
        if query.has_attachment {
            conditions.push("m.has_attachments = 1".to_owned());
        }
        for (column, wanted) in [
            // Unread is the absence of the seen flag.
            ("m.seen", query.unread.map(|unread| !unread)),
            ("m.flagged", query.starred),
            ("m.answered", query.answered),
        ] {
            if let Some(wanted) = wanted {
                conditions.push(format!("{column} = {}", i64::from(wanted)));
            }
        }
        if let Some(after) = query.after {
            let place = bind(Value::Integer(after), &mut values);
            conditions.push(format!("m.date >= {place}"));
        }
        if let Some(before) = query.before {
            let place = bind(Value::Integer(before), &mut values);
            conditions.push(format!("m.date < {place}"));
        }
        if let Some(place) = &query.within {
            // A folder is named by its name or by what it is for: `in:sent`
            // finds the Sent folder whatever the server calls it.
            let lower = |text: &str| Value::Text(text.trim().to_lowercase());
            if let Some(either) = &place.either {
                let name = bind(lower(either), &mut values);
                conditions.push(format!(
                    "(lower(a.name) = {name} OR lower(f.name) = {name} OR f.role = {name})"
                ));
            }
            if let Some(account) = &place.account {
                let name = bind(lower(account), &mut values);
                conditions.push(format!("lower(a.name) = {name}"));
            }
            if let Some(folder) = &place.folder {
                let name = bind(lower(folder), &mut values);
                conditions.push(format!("(lower(f.name) = {name} OR f.role = {name})"));
            }
        }
        let limit = bind(
            Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)),
            &mut values,
        );
        let sql = format!(
            "SELECT m.id, m.folder_id, f.account_id, m.message_id, m.subject,
                    m.from_addrs, m.to_addrs, m.cc_addrs, m.date, m.seen, m.flagged, m.answered,
                    m.draft, m.has_attachments
               FROM messages m
               JOIN folders f ON f.id = m.folder_id
               JOIN accounts a ON a.id = f.account_id
              WHERE {}
              ORDER BY m.date DESC, m.id DESC
              LIMIT {limit}",
            conditions.join(" AND ")
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(values), message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn has_terms(query: &Query) -> bool {
    !(query.text.is_empty()
        && query.from.is_empty()
        && query.to.is_empty()
        && query.subject.is_empty())
}

/// The full-text expression for a query's words: every one must match, each
/// in the column its filter names. `None` when there is nothing to match.
fn match_expression(query: &Query) -> Option<String> {
    let mut parts = Vec::new();
    for (column, terms) in [
        (None, &query.text),
        (Some("sender"), &query.from),
        (Some("recipients"), &query.to),
        (Some("subject"), &query.subject),
    ] {
        for term in terms {
            if let Some(quoted) = quoted(term) {
                parts.push(match column {
                    Some(column) => format!("{column} : {quoted}"),
                    None => quoted,
                });
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

/// A term as a quoted full-text string, which the index reads as the words
/// in it, in order: one word for a word, several for a phrase or an
/// address. `None` for a term with no letter or digit in it, which holds no
/// word at all.
fn quoted(term: &Term) -> Option<String> {
    if !term.text.chars().any(char::is_alphanumeric) {
        return None;
    }
    // Inside quotes the only character with a meaning is the quote itself,
    // which is written twice.
    let mut out = format!("\"{}\"", term.text.replace('"', "\"\""));
    if term.prefix {
        out.push('*');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, prefix: bool) -> Term {
        Term {
            text: text.to_owned(),
            phrase: false,
            prefix,
        }
    }

    #[test]
    fn words_are_quoted_and_joined() {
        let query = Query {
            text: vec![term("road\"map", false), term("q4", true)],
            from: vec![term("ana", false)],
            subject: vec![term("plan", true)],
            ..Query::default()
        };
        assert_eq!(
            match_expression(&query).unwrap(),
            "\"road\"\"map\" AND \"q4\"* AND sender : \"ana\" AND subject : \"plan\"*"
        );
    }

    #[test]
    fn punctuation_alone_is_no_word() {
        let query = Query {
            text: vec![term("!!!", false), term("--", true)],
            ..Query::default()
        };
        assert_eq!(match_expression(&query), None);
        assert!(has_terms(&query));
    }
}
