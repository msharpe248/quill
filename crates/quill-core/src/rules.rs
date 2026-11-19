//! Rules as they are written: in `rules.toml`, and the older way, as
//! `[[rule]]` in `config.toml`.
//!
//! A rule says which mail it is about, in the search box's words, and what
//! to do with it. This is the file's side only: what the words mean is
//! [`crate::search`]'s to say, and what is done is the sync engine's.

use serde::Deserialize;

/// How far back a run over mail already here goes, unless the rule or the
/// settings say: a week.
pub const LOOK_BACK: &str = "1w";

/// The most messages a run over mail already here changes without asking.
pub const CAP: usize = 50;

/// One rule, as written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    /// What the rule is called: in the list of rules, the log and notices.
    /// `rules.toml` needs one; a rule in `config.toml` may have none.
    #[serde(default)]
    pub name: Option<String>,
    /// Which mail it is for, in the search box's words: `from:buildbot`.
    /// A list is any one of them.
    #[serde(rename = "match")]
    pub matching: Matching,
    /// Only mail of this account. Every account's, without it.
    #[serde(default)]
    pub account: Option<String>,
    /// File it in this folder of its account: a folder's name, or what a
    /// folder is for (`archive`, `trash`, `junk`).
    #[serde(default)]
    pub move_to: Option<String>,
    /// Move it to the Trash, from which it can be put back.
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub mark_read: bool,
    #[serde(default)]
    pub star: bool,
    /// Whether to announce it. Left out: announced if it stays in the
    /// inbox, not if the rule files it elsewhere.
    #[serde(default)]
    pub notify: Option<bool>,
    /// `false` switches this one rule off.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// How far back a run over mail already here goes: `1d`, `1w`, `30d`.
    #[serde(default)]
    pub look_back: Option<String>,
}

fn yes() -> bool {
    true
}

/// What a rule matches: one search, or any of several.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Matching {
    One(String),
    Any(Vec<String>),
}

impl Matching {
    /// The searches, any one of which is a match.
    pub fn each(&self) -> &[String] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Any(any) => any,
        }
    }
}

impl RuleSpec {
    /// Whether the rule makes sense as written, and if not, why not.
    /// `named` is what to call it in saying so.
    pub fn check(&self, named: &str) -> Result<(), String> {
        let searches = self.matching.each();
        if searches.is_empty() || searches.iter().any(|search| search.trim().is_empty()) {
            return Err(format!(
                "{named} has nothing to match. Give it `match = \"from:someone\"`"
            ));
        }
        if self.move_to.is_none()
            && !self.delete
            && !self.mark_read
            && !self.star
            && self.notify.is_none()
        {
            return Err(format!(
                "{named} does nothing. Give it one of move_to, delete, mark_read, star or notify"
            ));
        }
        if self.delete && self.move_to.is_some() {
            return Err(format!(
                "{named} has both delete and move_to. A message goes to one place"
            ));
        }
        if self
            .move_to
            .as_deref()
            .is_some_and(|folder| folder.trim().is_empty())
        {
            return Err(format!("{named} has an empty move_to"));
        }
        if let Some(look_back) = &self.look_back
            && look_back_seconds(look_back).is_none()
        {
            return Err(format!(
                "{named} has a look_back Quill cannot read: {look_back:?}. Write it as \"1d\", \
                 \"1w\" or \"30d\""
            ));
        }
        Ok(())
    }
}

/// A look-back as written (`1d`, `2w`, `30d`, `12h`), in seconds.
pub fn look_back_seconds(text: &str) -> Option<i64> {
    crate::search::age(&text.trim().to_lowercase())
}

/// The file as a whole.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(rename = "rule", default)]
    rules: Vec<RuleSpec>,
}

/// Reads the text of a `rules.toml`: its rules, in the order written. A
/// mistake anywhere in it is a mistake in the whole file: half a set of
/// rules is not what was written.
pub fn parse(text: &str) -> Result<Vec<RuleSpec>, String> {
    let file: File = toml::from_str(text).map_err(|err| {
        let line = err.span().map_or(1, |span| {
            text.get(..span.start)
                .map_or(1, |before| before.matches('\n').count() + 1)
        });
        format!("line {line}: {}", err.message())
    })?;
    let mut seen: Vec<String> = Vec::new();
    for (index, rule) in file.rules.iter().enumerate() {
        let Some(name) = rule
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            return Err(format!(
                "rule {} has no name. Give it `name = \"Receipts\"`",
                index + 1
            ));
        };
        if name.chars().any(char::is_control) || name.chars().count() > 80 {
            return Err(format!(
                "rule {} has a name Quill cannot show. A name is one line, of at most 80 \
                 characters",
                index + 1
            ));
        }
        let lower = name.to_lowercase();
        if seen.contains(&lower) {
            return Err(format!(
                "two rules are called \"{name}\". Each needs a name of its own"
            ));
        }
        seen.push(lower);
        rule.check(&format!("The rule \"{name}\""))?;
    }
    Ok(file.rules)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_file_of_rules_is_read_in_order() {
        let rules = parse(
            r#"
            [[rule]]
            name = "Receipts"
            match = "from:orders@example.com subject:receipt"
            move_to = "Receipts"
            look_back = "1w"

            [[rule]]
            name = "Build mail"
            match = ["from:buildbot", "list:builds.example.com"]
            move_to = "archive"
            mark_read = true

            [[rule]]
            name = "Promotions"
            match = "body:unsubscribe from:deals"
            delete = true
            enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].name.as_deref(), Some("Receipts"));
        assert_eq!(rules[0].look_back.as_deref(), Some("1w"));
        assert!(rules[0].enabled);
        assert_eq!(rules[1].matching.each().len(), 2);
        assert!(rules[2].delete && !rules[2].enabled);
    }

    #[test]
    fn an_empty_file_has_no_rules() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("# nothing yet\n").unwrap().is_empty());
    }

    #[test]
    fn a_mistake_is_said_with_the_rule_it_is_in() {
        let no_name = parse("[[rule]]\nmatch = \"from:a\"\ndelete = true\n").unwrap_err();
        assert!(no_name.contains("no name"), "{no_name}");

        let twice = parse(
            "[[rule]]\nname = \"A\"\nmatch = \"x\"\nstar = true\n\
             [[rule]]\nname = \"a\"\nmatch = \"y\"\nstar = true\n",
        )
        .unwrap_err();
        assert!(twice.contains("two rules"), "{twice}");

        let both = parse("[[rule]]\nname = \"A\"\nmatch = \"x\"\ndelete = true\nmove_to = \"B\"\n")
            .unwrap_err();
        assert!(both.contains("both delete and move_to"), "{both}");

        let nothing = parse("[[rule]]\nname = \"A\"\nmatch = \"x\"\n").unwrap_err();
        assert!(nothing.contains("does nothing"), "{nothing}");

        let empty = parse("[[rule]]\nname = \"A\"\nmatch = []\nstar = true\n").unwrap_err();
        assert!(empty.contains("nothing to match"), "{empty}");

        let back =
            parse("[[rule]]\nname = \"A\"\nmatch = \"x\"\nstar = true\nlook_back = \"a while\"\n")
                .unwrap_err();
        assert!(back.contains("look_back"), "{back}");

        let unknown =
            parse("[[rule]]\nname = \"A\"\nmatch = \"x\"\nstar = true\nforward = \"b\"\n")
                .unwrap_err();
        assert!(unknown.starts_with("line "), "{unknown}");
    }

    #[test]
    fn a_look_back_is_read_as_an_age() {
        const DAY: i64 = 24 * 60 * 60;
        assert_eq!(look_back_seconds("1d"), Some(DAY));
        assert_eq!(look_back_seconds(" 1W "), Some(7 * DAY));
        assert_eq!(look_back_seconds("30d"), Some(30 * DAY));
        assert_eq!(look_back_seconds("0d"), None);
        assert_eq!(look_back_seconds("soon"), None);
    }
}
