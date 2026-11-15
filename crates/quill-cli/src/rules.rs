//! The rules in `config.toml`, as the sync engine wants them.

use quill_core::config::Config;
use quill_sync::Rule;

/// The config's rules, in the order written. What each matches is read as
/// the search box reads it, with dates as days where the user is.
pub fn from_config(config: &Config) -> Vec<Rule> {
    let tz = jiff::tz::TimeZone::system();
    config
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| Rule {
            name: rule
                .name
                .clone()
                .unwrap_or_else(|| format!("rule {}", index + 1)),
            query: quill_core::search::parse(&rule.matching, |year, month, day| {
                let date = jiff::civil::Date::new(
                    i16::try_from(year).ok()?,
                    i8::try_from(month).ok()?,
                    i8::try_from(day).ok()?,
                )
                .ok()?;
                Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_second())
            }),
            account: rule.account.clone(),
            move_to: rule.move_to.clone(),
            mark_read: rule.mark_read,
            star: rule.star,
            notify: rule.notify,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rules_come_out_in_order_with_names_for_those_that_have_none() {
        let config = Config::parse(
            r#"
[[rule]]
match = "from:buildbot"
move_to = "archive"
mark_read = true

[[rule]]
name = "Invoices"
match = "subject:invoice has:attachment"
account = "Work"
star = true
notify = false
"#,
        )
        .unwrap();
        let rules = from_config(&config);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].name, "rule 1");
        assert_eq!(rules[0].move_to.as_deref(), Some("archive"));
        assert!(rules[0].mark_read && !rules[0].star);
        assert_eq!(rules[0].notify, None);
        assert_eq!(rules[1].name, "Invoices");
        assert_eq!(rules[1].account.as_deref(), Some("Work"));
        assert!(rules[1].query.has_attachment);
        assert_eq!(rules[1].notify, Some(false));
    }

    #[test]
    fn a_rule_that_matches_or_does_nothing_is_a_config_error() {
        for text in [
            "[[rule]]\nmatch = \"  \"\nstar = true\n",
            "[[rule]]\nmatch = \"from:ana\"\n",
            "[[rule]]\nname = \"Noisy\"\nmatch = \"from:ana\"\nlabel = \"x\"\n",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
    }
}
