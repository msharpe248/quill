//! Templates: text kept ready, to put into a message being written.
//!
//! A template is words with `{{placeholders}}` in them. Quill fills the
//! ones it knows from the message as it stands (who it is to, who it is
//! from, today's date) and leaves the rest as written for the user to fill.
//! Filling is substitution and nothing more: a template cannot run
//! anything, include a file or fetch anything, and what is put in for one
//! placeholder is never read again for more.
//!
//! Nothing here reads a file. Whoever starts the UI reads the templates and
//! hands their text in.

use std::ops::Range;

use crate::compose::parse_recipients;
use crate::{editor, text};

/// The longest a template's name is shown.
const MAX_NAME: usize = 80;

/// The longest name a placeholder may have.
const MAX_PLACEHOLDER: usize = 40;

/// A piece of text to put into a message, and perhaps a subject for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// What the picker calls it: its file's name without the ending.
    pub name: String,
    pub subject: Option<String>,
    pub body: String,
}

impl Template {
    /// Reads a template from its name and its file's text. A first line
    /// `Subject: ...` with a blank line under it (or nothing under it) is
    /// the subject and not part of the text.
    ///
    /// The text is cleaned as the editor cleans what is typed into it, so
    /// that it goes in exactly as it is here: no control characters, tabs
    /// as spaces, one kind of line break.
    pub fn parse(name: &str, text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let cleaned = editor::clean(text);
        let text: &str = &cleaned;
        let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
        let subject = first
            .get(..8)
            .filter(|label| label.eq_ignore_ascii_case("subject:"))
            .map(|_| first[8..].trim())
            // A header ends at a blank line. Without one this is just a
            // first line that happens to start with the word.
            .filter(|_| rest.is_empty() || rest.starts_with('\n'))
            .filter(|subject| !subject.is_empty());
        let body = match subject {
            Some(_) => rest.strip_prefix('\n').unwrap_or(rest),
            None => text,
        };
        Self {
            name: text::truncate(text::sanitize_line(name).trim(), MAX_NAME),
            subject: subject.map(str::to_owned),
            // A file ends with a line break; a snippet put in the middle of
            // a line should not bring one.
            body: body.trim_end_matches('\n').to_owned(),
        }
    }
}

/// What Quill knows to fill placeholders with. `None` is not known: the
/// placeholder is left for the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values {
    /// `{{to}}`: the first recipient's address.
    pub to: Option<String>,
    /// `{{to_name}}`: their name, or the part of their address before the
    /// `@` when no name is given.
    pub to_name: Option<String>,
    /// `{{first_name}}`: the first word of their name. Not guessed from
    /// an address.
    pub first_name: Option<String>,
    /// `{{from_name}}`: the user's own name on the account.
    pub from_name: Option<String>,
    /// `{{date}}`: today.
    pub date: Option<String>,
    /// `{{subject}}`: the message's subject.
    pub subject: Option<String>,
}

impl Values {
    /// What is known about the message being written: `to` is its To field
    /// as typed.
    pub fn of(to: &str, subject: &str, from_name: Option<&str>, date: &str) -> Self {
        let known = |text: &str| {
            let text = text::sanitize_line(text);
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        let recipient = first_recipient(to);
        let name = recipient
            .as_ref()
            .and_then(|(name, _)| name.as_deref())
            .and_then(known);
        let address = recipient.as_ref().and_then(|(_, address)| known(address));
        Self {
            first_name: name.as_deref().and_then(|name| {
                // "Haddad, Omar" is a family name first; its first word is
                // still the best there is, without the comma.
                let word = name.split_whitespace().next()?;
                known(word.trim_end_matches([',', ';']))
            }),
            to_name: name.or_else(|| {
                address
                    .as_deref()
                    .and_then(|address| address.split('@').next())
                    .and_then(known)
            }),
            to: address,
            from_name: from_name.and_then(known),
            date: known(date),
            subject: known(subject),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        match name.to_ascii_lowercase().as_str() {
            "to" => self.to.as_deref(),
            "to_name" => self.to_name.as_deref(),
            "first_name" => self.first_name.as_deref(),
            "from_name" => self.from_name.as_deref(),
            "date" => self.date.as_deref(),
            "subject" => self.subject.as_deref(),
            _ => None,
        }
    }
}

/// The first person in a field of addresses, as a name (if one is given)
/// and an address. The rest of the field may still be half typed.
fn first_recipient(field: &str) -> Option<(Option<String>, String)> {
    let mut quoted = false;
    let end = field
        .char_indices()
        .find(|(_, c)| {
            if *c == '"' {
                quoted = !quoted;
            }
            !quoted && matches!(c, ',' | ';')
        })
        .map_or(field.len(), |(at, _)| at);
    let person = parse_recipients(&field[..end]).ok()?.into_iter().next()?;
    Some((person.name, person.email))
}

/// A template's text with what is known put in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filled {
    pub text: String,
    /// Where in `text` the first placeholder left unfilled is.
    pub gap: Option<Range<usize>>,
}

/// Puts `values` in for the placeholders of `text`. A placeholder is two
/// opening braces, a name of letters, digits and underscores (spaces
/// around it are allowed), and two closing braces. One whose name is not
/// known, or has nothing to fill it, stays exactly as written; so does
/// any other pair of braces.
pub fn fill(text: &str, values: &Values) -> Filled {
    let mut out = String::with_capacity(text.len());
    let mut gap = None;
    let mut rest = text;
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match placeholder(rest) {
            Some((name, len)) => {
                match values.get(name) {
                    Some(value) => out.push_str(value),
                    None => {
                        gap.get_or_insert(out.len()..out.len() + len);
                        out.push_str(&rest[..len]);
                    }
                }
                rest = &rest[len..];
            }
            None => {
                // Not a placeholder. The second brace may still start one.
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Filled { text: out, gap }
}

/// The placeholder `text` starts with: its name, and how many bytes the
/// whole of it takes, braces included.
fn placeholder(text: &str) -> Option<(&str, usize)> {
    let inside = text.strip_prefix("{{")?;
    let padded = inside.trim_start_matches([' ', '\t']);
    let name_len = padded
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    if name_len == 0 || name_len > MAX_PLACEHOLDER {
        return None;
    }
    let (name, after) = padded.split_at(name_len);
    let close = after.trim_start_matches([' ', '\t']).strip_prefix("}}")?;
    Some((name, text.len() - close.len()))
}

/// Whether the letters typed appear in `name` in order, whatever their
/// case: how a picker's list is narrowed, so "tnk" finds "Thanks".
pub fn narrows(typed: &str, name: &str) -> bool {
    let mut name = name.chars().flat_map(char::to_lowercase);
    typed
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .all(|wanted| name.any(|c| c == wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values {
        Values::of(
            "Ana Ruiz <ana.ruiz@example.com>, sam.lee@example.com",
            "Q4 roadmap",
            Some("Robin Vale"),
            "14 November 2026",
        )
    }

    #[test]
    fn known_placeholders_are_filled() {
        let filled = fill(
            "Hi {{first_name}},\n\nSent to {{to_name}} <{{to}}> on {{date}} about {{subject}}.\n\n{{from_name}}",
            &values(),
        );
        assert_eq!(
            filled.text,
            "Hi Ana,\n\nSent to Ana Ruiz <ana.ruiz@example.com> on 14 November 2026 about Q4 roadmap.\n\nRobin Vale"
        );
        assert_eq!(filled.gap, None);
        // Spaces inside the braces and capitals in the name are the same
        // placeholder.
        assert_eq!(
            fill("{{ To }} {{\tFIRST_NAME\t}}", &values()).text,
            "ana.ruiz@example.com Ana"
        );
    }

    #[test]
    fn unknown_placeholders_stay_and_the_first_is_found() {
        let filled = fill(
            "Hi {{first_name}},\n\nYour order {{order_number}} ships {{ when }}.",
            &values(),
        );
        assert_eq!(
            filled.text,
            "Hi Ana,\n\nYour order {{order_number}} ships {{ when }}."
        );
        let gap = filled.gap.unwrap();
        assert_eq!(&filled.text[gap], "{{order_number}}");
    }

    #[test]
    fn a_known_placeholder_with_nothing_to_fill_it_is_left_for_the_user() {
        // Nobody to write to yet.
        let nobody = Values::of("", "", None, "14 November 2026");
        let filled = fill("Dear {{to_name}},\n\n{{date}}", &nobody);
        assert_eq!(filled.text, "Dear {{to_name}},\n\n14 November 2026");
        assert_eq!(&filled.text[filled.gap.unwrap()], "{{to_name}}");
    }

    #[test]
    fn braces_that_are_not_a_placeholder_are_left_alone() {
        for text in [
            "{{}}",
            "{{ }}",
            "{{first name}}",
            "{{first-name}}",
            "{{first_name}",
            "{{first_name",
            "{first_name}",
            "{{\nfirst_name\n}}",
            "{{first_name}\n}",
            "fn main() {{ println!(\"{}\", 1); }}",
            "{{#if x}}yes{{/if}}",
            "{{> other_file}}",
            "{{{",
            "}}{{",
            "{{é}}",
            "{{",
        ] {
            let filled = fill(text, &values());
            assert_eq!(filled.text, text);
            assert_eq!(filled.gap, None, "{text}");
        }
        // A name longer than any placeholder has is not one.
        let long = format!("{{{{{}}}}}", "a".repeat(MAX_PLACEHOLDER + 1));
        assert_eq!(fill(&long, &values()).gap, None);
        // A third brace is a brace, and then a placeholder.
        assert_eq!(fill("{{{first_name}}}", &values()).text, "{Ana}");
        // Many braces cost nothing much and break nothing.
        let many = "{".repeat(100_000);
        assert_eq!(fill(&many, &values()).text, many);
    }

    #[test]
    fn what_is_filled_in_is_not_read_again() {
        // A subject that looks like a placeholder is put in as it is.
        let sly = Values::of(
            "ana.ruiz@example.com",
            "{{to}} {{nonsense}}",
            Some("{{date}}"),
            "today",
        );
        let filled = fill("{{subject}} / {{from_name}} / {{later}}", &sly);
        assert_eq!(filled.text, "{{to}} {{nonsense}} / {{date}} / {{later}}");
        // The gap is the template's own, not one a value brought with it.
        assert_eq!(&filled.text[filled.gap.unwrap()], "{{later}}");
    }

    #[test]
    fn the_recipient_is_the_first_person_in_the_field() {
        let of = |to: &str| Values::of(to, "", None, "");
        let named = of("Ana Ruiz <ana.ruiz@example.com>");
        assert_eq!(named.to.as_deref(), Some("ana.ruiz@example.com"));
        assert_eq!(named.to_name.as_deref(), Some("Ana Ruiz"));
        assert_eq!(named.first_name.as_deref(), Some("Ana"));

        // An address alone has a name of sorts, and no first name to
        // guess at.
        let bare = of("sam.lee@example.com, Ana Ruiz <ana.ruiz@example.com>");
        assert_eq!(bare.to.as_deref(), Some("sam.lee@example.com"));
        assert_eq!(bare.to_name.as_deref(), Some("sam.lee"));
        assert_eq!(bare.first_name, None);

        // A comma inside a quoted name does not end the first person.
        let quoted = of("\"Haddad, Omar\" <omar.haddad@example.com>, x");
        assert_eq!(quoted.to.as_deref(), Some("omar.haddad@example.com"));
        assert_eq!(quoted.to_name.as_deref(), Some("Haddad, Omar"));
        assert_eq!(quoted.first_name.as_deref(), Some("Haddad"));

        // A second address still being typed does not hide the first.
        assert_eq!(
            of("ana.ruiz@example.com, sa").to.as_deref(),
            Some("ana.ruiz@example.com")
        );
        // Nobody, or nobody yet.
        for to in ["", "  ", "ana", "Ana Ruiz <ana", ","] {
            assert_eq!(of(to), Values::default(), "{to:?}");
        }
    }

    #[test]
    fn a_template_file_may_start_with_its_subject() {
        let template = Template::parse(
            "Meeting follow-up",
            "Subject: Following up on {{subject}}\n\nHi {{first_name}},\n\nThanks for today.\n",
        );
        assert_eq!(template.name, "Meeting follow-up");
        assert_eq!(
            template.subject.as_deref(),
            Some("Following up on {{subject}}")
        );
        assert_eq!(template.body, "Hi {{first_name}},\n\nThanks for today.");

        // No subject line: all of it is the text.
        let snippet = Template::parse("thanks", "Thanks,\nRobin\n");
        assert_eq!(snippet.subject, None);
        assert_eq!(snippet.body, "Thanks,\nRobin");

        // A first line that only starts with the word is text.
        let prose = Template::parse("prose", "Subject: matter\nof the meeting\n");
        assert_eq!(prose.subject, None);
        assert_eq!(prose.body, "Subject: matter\nof the meeting");
        let empty = Template::parse("empty", "Subject:\n\nText");
        assert_eq!(empty.subject, None);

        // Windows line ends, a byte order mark, and a subject alone.
        let windows = Template::parse("w", "\u{feff}SUBJECT: Hello\r\n\r\nOne\r\nTwo\r\n");
        assert_eq!(windows.subject.as_deref(), Some("Hello"));
        assert_eq!(windows.body, "One\nTwo");
        let only = Template::parse("only", "Subject: Just this");
        assert_eq!(only.subject.as_deref(), Some("Just this"));
        assert_eq!(only.body, "");
    }

    #[test]
    fn a_template_is_cleaned_as_typing_is() {
        let template = Template::parse(
            "evil\u{1b}]0;title\u{7}name\n",
            "Subject: a\u{1b}[31mb\n\nOne\ttwo\u{0}\u{1b}[2J\u{7}three",
        );
        assert_eq!(template.name, "evilname");
        assert!(!template.subject.as_deref().unwrap().contains('\u{1b}'));
        assert!(
            !template
                .body
                .contains(|c: char| c.is_control() && c != '\n')
        );
        // What the editor would do to it has been done: it goes in as is.
        assert_eq!(editor::clean(&template.body), template.body.as_str());
        let long = Template::parse(&"n".repeat(500), "x");
        assert!(text::width(&long.name) <= MAX_NAME);
    }

    #[test]
    fn typing_narrows_by_letters_in_order() {
        assert!(narrows("", "Thanks"));
        assert!(narrows("tnk", "Thanks"));
        assert!(narrows("T K", "thanks"));
        assert!(!narrows("kt", "Thanks"));
        assert!(!narrows("thanksx", "Thanks"));
    }
}
