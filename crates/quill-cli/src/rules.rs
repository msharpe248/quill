//! The rules, read from their files for the sync engine and the UI.
//!
//! Rules are written in `rules.toml`, beside `config.toml`, and the older
//! way as `[[rule]]` in `config.toml` itself. Whether they run at all, and
//! how far back they look, is `[rules]` in the config. This reads both,
//! keeps the engine and the UI told of what they say, and writes the one
//! thing Quill writes there: a rule the user made in the UI, added at the
//! end.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use quill_core::config::Config;
use quill_core::rules::RuleSpec;
use quill_sync::{Rule, RuleSet, SyncHandle};
use quill_tui::setup::{AccountEvent, NewRule, RulesAsk, RulesNow};
use tokio::sync::mpsc::UnboundedSender;

use crate::config_edit;

/// What a `rules.toml` that is not there yet is made as: nothing but how
/// to write one.
const TEMPLATE: &str = r#"# Quill's rules: what to do with mail by itself.
#
# A rule says which mail it is about, in the search box's words, and what
# to do with it. Rules are asked in the order written, about mail as it
# arrives in an inbox. Quill reads this file again whenever it changes.
#
# [[rule]]
# name = "Receipts"
# match = "from:orders@example.com subject:receipt"
# move_to = "Receipts"            # a folder's name, or archive, trash, junk
#
# [[rule]]
# name = "Build mail"
# match = ["from:buildbot", "list:builds.example.com"]   # either
# mark_read = true
# move_to = "archive"
#
# [[rule]]
# name = "Promotions"
# match = "body:unsubscribe from:deals"
# delete = true                   # to the Trash; Put Back returns it
# look_back = "1d"                # how far back a run over mail already here goes
# enabled = false                 # switched off, until you have tried it
#
# Rules > Run a Rule shows what a rule would match before it changes anything.
"#;

/// The rules as read, and what is wrong with them as written, if
/// anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Loaded {
    pub set: RuleSet,
    pub problem: Option<String>,
}

/// Reads the rules: the config's, then the file's, in the order written.
/// A mistake in `rules.toml` leaves out every rule in it, and is said:
/// half a set of rules is not what the user wrote.
pub fn load(config: &Config, rules_file: &Path) -> Loaded {
    let mut rules: Vec<Rule> = config
        .rules
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let name = spec
                .name
                .clone()
                .unwrap_or_else(|| format!("rule {}", index + 1));
            rule(spec, name)
        })
        .collect();
    let mut problem = None;
    match read(rules_file) {
        Ok(specs) => {
            let taken: Vec<String> = rules.iter().map(|rule| rule.name.to_lowercase()).collect();
            let clash = specs.iter().find_map(|spec| {
                let name = spec.name.as_deref().unwrap_or_default().trim();
                taken
                    .contains(&name.to_lowercase())
                    .then(|| name.to_owned())
            });
            match clash {
                Some(name) => {
                    problem = Some(format!(
                        "rules.toml: a rule is called \"{name}\", and so is one in config.toml. \
                         Each needs a name of its own. None of the rules in rules.toml run \
                         until it is put right."
                    ));
                }
                None => rules.extend(specs.iter().map(|spec| {
                    rule(
                        spec,
                        spec.name.as_deref().unwrap_or_default().trim().to_owned(),
                    )
                })),
            }
        }
        Err(mistake) => {
            problem = Some(format!(
                "rules.toml: {mistake}. None of the rules in it run until it is put right."
            ));
        }
    }
    Loaded {
        set: RuleSet {
            on: config.ruling.enabled,
            look_back: config.ruling.look_back_seconds(),
            rules,
            held: problem.is_some(),
        },
        problem,
    }
}

/// The rules written in the file. One that is not there has none.
fn read(rules_file: &Path) -> Result<Vec<RuleSpec>, String> {
    match std::fs::read_to_string(rules_file) {
        Ok(text) => quill_core::rules::parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("the file cannot be read: {err}")),
    }
}

/// A rule as the engine asks it: what it matches read as the search box
/// reads it, with dates as days where the user is.
fn rule(spec: &RuleSpec, name: String) -> Rule {
    let tz = jiff::tz::TimeZone::system();
    let queries = spec
        .matching
        .each()
        .iter()
        .map(|search| {
            quill_core::search::parse(search, |year, month, day| {
                let date = jiff::civil::Date::new(
                    i16::try_from(year).ok()?,
                    i8::try_from(month).ok()?,
                    i8::try_from(day).ok()?,
                )
                .ok()?;
                Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_second())
            })
        })
        .collect();
    Rule {
        name,
        queries,
        account: spec.account.clone(),
        move_to: spec.move_to.clone(),
        delete: spec.delete,
        mark_read: spec.mark_read,
        star: spec.star,
        notify: spec.notify,
        enabled: spec.enabled,
        look_back: spec
            .look_back
            .as_deref()
            .and_then(quill_core::rules::look_back_seconds),
    }
}

/// `text`, a `rules.toml`, with `new` added at its end. What is there is
/// kept as it is, comments and all. A file Quill cannot read is not added
/// to, and nothing is handed back that Quill could not read again.
pub fn add(text: &str, new: &NewRule) -> Result<String, String> {
    let before = quill_core::rules::parse(text)
        .map_err(|err| format!("rules.toml has a mistake to put right first: {err}"))?;
    let name = new.name.trim();
    if name.is_empty() {
        return Err("The rule needs a name.".to_owned());
    }
    if before.iter().any(|rule| {
        rule.name
            .as_deref()
            .is_some_and(|known| known.trim().eq_ignore_ascii_case(name))
    }) {
        return Err(format!(
            "There is a rule called \"{name}\" already. Give this one another name."
        ));
    }
    // A string as TOML writes one: whatever was typed is a value, never
    // a line of the file.
    let quoted = |text: &str| toml_edit::value(text).to_string();
    let mut lines = vec![
        "[[rule]]".to_owned(),
        format!("name = {}", quoted(name).trim()),
        format!("match = {}", quoted(new.matching.trim()).trim()),
    ];
    if let Some(account) = new
        .account
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        lines.push(format!("account = {}", quoted(account.trim()).trim()));
    }
    if new.delete {
        lines.push("delete = true".to_owned());
    } else if let Some(folder) = new
        .move_to
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        lines.push(format!("move_to = {}", quoted(folder.trim()).trim()));
    }
    if new.mark_read {
        lines.push("mark_read = true".to_owned());
    }
    let mut after = text.to_owned();
    if !after.is_empty() && !after.ends_with('\n') {
        after.push('\n');
    }
    if !after.trim().is_empty() {
        after.push('\n');
    }
    after.push_str(&lines.join("\n"));
    after.push('\n');
    quill_core::rules::parse(&after).map_err(|err| format!("The rule cannot be written: {err}"))?;
    Ok(after)
}

/// How long a look-back is, in the words it would be written in: `7d`.
fn age(seconds: i64) -> String {
    const HOUR: i64 = 60 * 60;
    const DAY: i64 = 24 * HOUR;
    if seconds % DAY == 0 {
        format!("{}d", seconds / DAY)
    } else {
        format!("{}h", (seconds / HOUR).max(1))
    }
}

/// `quill rules list`: the rules in the order they are asked, what each
/// does, and which are switched off.
pub fn list(out: &mut dyn std::io::Write, loaded: &Loaded, json: bool) -> std::io::Result<()> {
    let set = &loaded.set;
    if json {
        let rules: Vec<serde_json::Value> = set
            .rules
            .iter()
            .map(|rule| {
                serde_json::json!({
                    "name": rule.name,
                    "enabled": rule.enabled,
                    "does": rule.does(),
                    "account": rule.account,
                    "look_back": age(rule.look_back.unwrap_or(set.look_back)),
                    "needs_body": rule.needs_body(),
                })
            })
            .collect();
        let value = serde_json::json!({
            "on": set.on,
            "look_back": age(set.look_back),
            "problem": loaded.problem,
            "rules": rules,
        });
        return writeln!(out, "{value}");
    }
    if let Some(problem) = &loaded.problem {
        writeln!(out, "{problem}\n")?;
    }
    if set.rules.is_empty() {
        return writeln!(
            out,
            "There are no rules. They are written in rules.toml, beside config.toml: Rules > \
             Edit Rules in Quill opens it."
        );
    }
    writeln!(
        out,
        "Rules are {}. A rule looks back {} over mail already here, unless it says.\n",
        if set.on { "on" } else { "off" },
        age(set.look_back)
    )?;
    let widest = set
        .rules
        .iter()
        .map(|rule| quill_tui::text::width(&quill_tui::text::sanitize_line(&rule.name)))
        .max()
        .unwrap_or(0);
    for (index, rule) in set.rules.iter().enumerate() {
        let name = quill_tui::text::sanitize_line(&rule.name);
        let pad = " ".repeat(widest.saturating_sub(quill_tui::text::width(&name)));
        let mut notes = Vec::new();
        if let Some(account) = &rule.account {
            notes.push(format!("{} only", quill_tui::text::sanitize_line(account)));
        }
        if let Some(back) = rule.look_back {
            notes.push(format!("looks back {}", age(back)));
        }
        if !rule.enabled {
            notes.push("off".to_owned());
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join(", "))
        };
        writeln!(
            out,
            "{:>3}  {name}{pad}  {}{notes}",
            index + 1,
            quill_tui::text::sanitize_line(&rule.does())
        )?;
    }
    Ok(())
}

/// `quill rules test NAME`: what a rule would change in the inboxes, as
/// far back as it looks. Nothing is changed, and nothing is written but
/// what opening the mailbox writes.
pub fn test(
    out: &mut dyn std::io::Write,
    store: &mut quill_store::Store,
    loaded: &Loaded,
    name: &str,
    json: bool,
) -> anyhow::Result<()> {
    let Some(rule) = loaded.set.named(name) else {
        let known: Vec<&str> = loaded
            .set
            .rules
            .iter()
            .map(|rule| rule.name.as_str())
            .collect();
        anyhow::bail!(
            "there is no rule called \"{}\". {}",
            quill_tui::text::sanitize_line(name),
            if known.is_empty() {
                "There are no rules: `quill rules list` says where they are written.".to_owned()
            } else {
                format!(
                    "The rules are: {}.",
                    quill_tui::text::sanitize_line(&known.join(", "))
                )
            }
        );
    };
    let now = jiff::Timestamp::now().as_second();
    let hand = quill_sync::rules::by_hand(store, &loaded.set, Some(&rule.name), true, now, now)?;
    let rows = store.message_rows(&hand.changed)?;
    let tz = jiff::tz::TimeZone::system();
    if json {
        let messages: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id.0,
                    "date": row.headers.date,
                    "from": row.headers.from.first().map(|from| from.email.clone()),
                    "subject": row.headers.subject,
                })
            })
            .collect();
        let value = serde_json::json!({
            "rule": rule.name,
            "would": hand.would(),
            "count": hand.changed.len(),
            "messages": messages,
            "problems": hand.problems,
        });
        writeln!(out, "{value}")?;
        return Ok(());
    }
    writeln!(
        out,
        "{} {}.",
        quill_tui::text::sanitize_line(&rule.name),
        hand.would()
    )?;
    for problem in &hand.problems {
        writeln!(out, "{}", quill_tui::text::sanitize_line(problem))?;
    }
    if !rows.is_empty() {
        writeln!(out)?;
    }
    for row in &rows {
        let from = row
            .headers
            .from
            .first()
            .map(|from| from.email.clone())
            .unwrap_or_default();
        writeln!(
            out,
            "{:>8}  {:>6}  {}  {}",
            row.id.0,
            quill_tui::text::short_date(row.headers.date, now, &tz).trim(),
            quill_tui::text::sanitize_line(&from),
            quill_tui::text::sanitize_line(&row.headers.subject)
        )?;
    }
    Ok(())
}

/// Keeps the engine and the UI told of the rules: reads them when asked
/// and when their files change, and makes the changes the UI asks for.
pub struct Keeper {
    config_file: PathBuf,
    rules_file: PathBuf,
    /// The engine, where there is one: the demo mailbox has none.
    sync: Option<Arc<SyncHandle>>,
    events: UnboundedSender<AccountEvent>,
    /// What the UI was last told.
    told: Mutex<Option<Loaded>>,
}

impl Keeper {
    pub fn new(
        config_file: PathBuf,
        sync: Option<Arc<SyncHandle>>,
        events: UnboundedSender<AccountEvent>,
    ) -> Arc<Self> {
        let rules_file = match config_file.parent() {
            Some(dir) => crate::paths::rules_file_in(dir),
            None => PathBuf::from("rules.toml"),
        };
        // What the rules are as this starts is what the engine was
        // started with: nothing is said of it until it is asked for, or
        // changes.
        let told = Config::load(&config_file)
            .ok()
            .map(|config| load(&config, &rules_file));
        Arc::new(Self {
            config_file,
            rules_file,
            sync,
            events,
            told: Mutex::new(told),
        })
    }

    /// Reads the rules and tells whoever has not heard: the engine, when
    /// they differ from the ones it has, and the UI, when they differ
    /// from what it was last told or `always`.
    ///
    /// A config that cannot be read changes nothing: the rules stay as
    /// they were, and the config's own notice says what is wrong with it.
    pub fn refresh(&self, always: bool) {
        let Ok(config) = Config::load(&self.config_file) else {
            return;
        };
        let loaded = load(&config, &self.rules_file);
        if let Some(sync) = &self.sync
            && *sync.rules() != loaded.set
        {
            sync.set_rules(loaded.set.clone());
        }
        let Ok(mut told) = self.told.lock() else {
            return;
        };
        if always || told.as_ref() != Some(&loaded) {
            let _ = self.events.send(AccountEvent::Rules(RulesNow {
                set: Arc::new(loaded.set.clone()),
                problem: loaded.problem.clone(),
            }));
            *told = Some(loaded);
        }
    }

    /// Does what the UI asked about the rules. What the rules then are is
    /// told beside the answer.
    pub fn answer(&self, ask: RulesAsk) -> Option<AccountEvent> {
        match ask {
            RulesAsk::Read => {
                self.refresh(true);
                None
            }
            RulesAsk::Add(new) => {
                let result = self.add(&new);
                self.refresh(false);
                Some(AccountEvent::RuleAdded(result))
            }
            RulesAsk::Open => Some(AccountEvent::RulesFile(self.file())),
        }
    }

    fn add(&self, new: &NewRule) -> Result<String, String> {
        let before = match std::fs::read_to_string(&self.rules_file) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("rules.toml cannot be read: {err}")),
        };
        // A name a rule in the config has is taken too.
        if let Ok(config) = Config::load(&self.config_file)
            && load(&config, Path::new("")).set.named(&new.name).is_some()
        {
            return Err(format!(
                "There is a rule called \"{}\" already, in config.toml. Give this one another \
                 name.",
                new.name.trim()
            ));
        }
        let after = add(&before, new)?;
        config_edit::write(&self.rules_file, &after)
            .map_err(|err| format!("rules.toml cannot be written: {err}"))?;
        Ok(new.name.trim().to_owned())
    }

    /// Where the file is, made with nothing in it but how to write one if
    /// it was not there.
    fn file(&self) -> Result<PathBuf, String> {
        match std::fs::metadata(&self.rules_file) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                config_edit::write(&self.rules_file, TEMPLATE)
                    .map_err(|err| format!("rules.toml cannot be made: {err}"))?;
            }
            Err(err) => return Err(format!("rules.toml cannot be read: {err}")),
        }
        Ok(self.rules_file.clone())
    }

    /// Watches the two files for as long as the UI is there to tell, and
    /// reads the rules again when either changes: a rule written in an
    /// editor holds as soon as it is saved. Must be called on a Tokio
    /// runtime.
    pub fn watch(self: &Arc<Self>) {
        let keeper = Arc::downgrade(self);
        tokio::spawn(async move {
            let stamp = |path: &Path| -> Option<SystemTime> {
                std::fs::metadata(path)
                    .and_then(|file| file.modified())
                    .ok()
            };
            let mut seen: Option<(Option<SystemTime>, Option<SystemTime>)> = None;
            loop {
                let Some(keeper) = keeper.upgrade() else {
                    return;
                };
                if keeper.events.is_closed() {
                    return;
                }
                let now = (stamp(&keeper.config_file), stamp(&keeper.rules_file));
                if seen != Some(now) {
                    // The first look is only to know how they stand.
                    let changed = seen.is_some();
                    seen = Some(now);
                    if changed {
                        let work = Arc::clone(&keeper);
                        let _ = tokio::task::spawn_blocking(move || work.refresh(false)).await;
                    }
                }
                drop(keeper);
                tokio::time::sleep(WATCH_EVERY).await;
            }
        });
    }
}

/// How often the files are looked at for a change.
const WATCH_EVERY: Duration = Duration::from_secs(2);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_configs_rules_come_first_with_names_for_those_that_have_none() {
        let config = Config::parse(
            r#"
            [rules]
            look_back = "2d"

            [[rule]]
            match = "from:buildbot"
            move_to = "archive"
            mark_read = true

            [[rule]]
            name = "Invoices"
            match = "subject:invoice has:attachment"
            account = "Work"
            notify = false
            "#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rules.toml");
        std::fs::write(
            &file,
            "[[rule]]\nname = \"Promotions\"\nmatch = [\"from:deals\", \"body:unsubscribe\"]\n\
             delete = true\nenabled = false\nlook_back = \"1d\"\n",
        )
        .unwrap();
        let loaded = load(&config, &file);
        assert_eq!(loaded.problem, None);
        let set = loaded.set;
        assert!(set.on);
        assert_eq!(set.look_back, 2 * 24 * 60 * 60);
        assert_eq!(set.rules.len(), 3);
        assert_eq!(set.rules[0].name, "rule 1");
        assert_eq!(set.rules[0].move_to.as_deref(), Some("archive"));
        assert!(set.rules[0].mark_read && !set.rules[0].star);
        assert_eq!(set.rules[1].name, "Invoices");
        assert_eq!(set.rules[1].account.as_deref(), Some("Work"));
        assert!(set.rules[1].queries[0].has_attachment);
        assert_eq!(set.rules[1].notify, Some(false));
        assert_eq!(set.rules[2].name, "Promotions");
        assert_eq!(set.rules[2].queries.len(), 2);
        assert!(set.rules[2].delete && !set.rules[2].enabled);
        assert_eq!(set.rules[2].look_back, Some(24 * 60 * 60));
        assert!(set.rules[2].needs_body());
        // Off, nothing is asked by itself.
        assert_eq!(set.active().len(), 2);
    }

    #[test]
    fn a_mistake_in_the_file_leaves_its_rules_out_and_is_said() {
        let config =
            Config::parse("[[rule]]\nname = \"Kept\"\nmatch = \"x\"\nstar = true\n").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rules.toml");
        std::fs::write(
            &file,
            "[[rule]]\nname = \"Good\"\nmatch = \"y\"\nstar = true\n\
             [[rule]]\nname = \"Bad\"\nmatch = \"z\"\n",
        )
        .unwrap();
        let loaded = load(&config, &file);
        let problem = loaded.problem.unwrap();
        assert!(problem.contains("\"Bad\" does nothing"), "{problem}");
        // Not half of them: none of the file's.
        assert_eq!(loaded.set.rules.len(), 1);
        assert_eq!(loaded.set.rules[0].name, "Kept");

        // A name the config's rule has is a mistake too.
        std::fs::write(
            &file,
            "[[rule]]\nname = \"kept\"\nmatch = \"y\"\nstar = true\n",
        )
        .unwrap();
        let loaded = load(&config, &file);
        assert!(loaded.problem.unwrap().contains("so is one in config.toml"));
        assert_eq!(loaded.set.rules.len(), 1);

        // No file at all is no rules, and nothing wrong.
        let loaded = load(&config, &dir.path().join("none.toml"));
        assert_eq!(loaded.problem, None);
        assert_eq!(loaded.set.rules.len(), 1);
    }

    #[test]
    fn a_rule_is_added_at_the_end_with_what_was_there_kept() {
        let before = "# Mine.\n[[rule]]\nname = \"First\"   # kept\nmatch = \"x\"\nstar = true\n";
        let new = NewRule {
            name: "From \"Ana\"".to_owned(),
            matching: "from:ana@example.com subject:\"road map\"".to_owned(),
            account: None,
            move_to: Some("Plans".to_owned()),
            delete: false,
            mark_read: true,
        };
        let after = add(before, &new).unwrap();
        assert!(after.starts_with(before), "{after}");
        let rules = quill_core::rules::parse(&after).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].name.as_deref(), Some("From \"Ana\""));
        assert_eq!(
            rules[1].matching.each(),
            ["from:ana@example.com subject:\"road map\"".to_owned()]
        );
        assert_eq!(rules[1].move_to.as_deref(), Some("Plans"));
        assert!(rules[1].mark_read && !rules[1].delete);

        // Into a file with nothing in it.
        let alone = add(
            "",
            &NewRule {
                delete: true,
                ..new.clone()
            },
        )
        .unwrap();
        assert!(alone.starts_with("[[rule]]\n"), "{alone}");
        assert!(quill_core::rules::parse(&alone).unwrap()[0].delete);

        // A name already there, a rule with nothing to it, and a file
        // with a mistake in it are all refused, and say why.
        let twice = add(&after, &new).unwrap_err();
        assert!(twice.contains("already"), "{twice}");
        let nothing = add(
            "",
            &NewRule {
                move_to: None,
                mark_read: false,
                ..new.clone()
            },
        )
        .unwrap_err();
        assert!(nothing.contains("does nothing"), "{nothing}");
        let broken = add("[[rule]]\nname = 3\n", &new).unwrap_err();
        assert!(broken.contains("mistake to put right first"), "{broken}");
    }

    #[test]
    fn what_was_typed_is_a_value_and_never_a_line_of_the_file() {
        let new = NewRule {
            name: "x\"\nenabled = false\n[[rule]]\nname = \"y".to_owned(),
            matching: "from:a".to_owned(),
            mark_read: true,
            ..NewRule::default()
        };
        // A name over two lines is not a name Quill shows: refused, and
        // not written as two rules.
        assert!(add("", &new).is_err());
        let new = NewRule {
            name: "Quotes".to_owned(),
            matching: "subject:\"a\\b\" # not a comment".to_owned(),
            mark_read: true,
            ..NewRule::default()
        };
        let after = add("", &new).unwrap();
        let rules = quill_core::rules::parse(&after).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].matching.each(),
            std::slice::from_ref(&new.matching)
        );
    }
    /// A keeper over a scratch config folder, with no engine, and the
    /// UI's end of it.
    fn keeper(
        dir: &tempfile::TempDir,
    ) -> (
        Arc<Keeper>,
        tokio::sync::mpsc::UnboundedReceiver<AccountEvent>,
    ) {
        let (events, heard) = tokio::sync::mpsc::unbounded_channel();
        (
            Keeper::new(dir.path().join("config.toml"), None, events),
            heard,
        )
    }

    fn rules_told(
        heard: &mut tokio::sync::mpsc::UnboundedReceiver<AccountEvent>,
    ) -> Option<RulesNow> {
        match heard.try_recv() {
            Ok(AccountEvent::Rules(now)) => Some(now),
            Ok(other) => panic!("{other:?}"),
            Err(_) => None,
        }
    }

    #[test]
    fn the_ui_is_told_the_rules_when_it_asks_and_when_they_change_and_not_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let (keeper, mut heard) = keeper(&dir);
        // Nothing is said until it is asked for.
        keeper.refresh(false);
        assert_eq!(rules_told(&mut heard), None);
        assert_eq!(keeper.answer(RulesAsk::Read), None);
        let first = rules_told(&mut heard).unwrap();
        assert!(first.set.on && first.set.rules.is_empty() && first.problem.is_none());

        // The file is written: read again, the UI hears of it once.
        std::fs::write(
            dir.path().join("rules.toml"),
            "[[rule]]\nname = \"A\"\nmatch = \"x\"\nstar = true\n",
        )
        .unwrap();
        keeper.refresh(false);
        assert_eq!(rules_told(&mut heard).unwrap().set.rules.len(), 1);
        keeper.refresh(false);
        assert_eq!(rules_told(&mut heard), None);

        // Switched off in the settings: the set says so.
        std::fs::write(dir.path().join("config.toml"), "[rules]\nenabled = false\n").unwrap();
        keeper.refresh(false);
        let off = rules_told(&mut heard).unwrap();
        assert!(!off.set.on && off.set.active().is_empty());

        // A config that cannot be read changes nothing.
        std::fs::write(dir.path().join("config.toml"), "[rules\n").unwrap();
        keeper.refresh(true);
        assert_eq!(rules_told(&mut heard), None);
    }

    #[test]
    fn a_rule_from_the_ui_is_written_into_the_file_and_the_rules_are_told_again() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[[rule]]\nname = \"Old\"\nmatch = \"x\"\nstar = true\n",
        )
        .unwrap();
        let (keeper, mut heard) = keeper(&dir);
        let new = NewRule {
            name: "Receipts".to_owned(),
            matching: "from:orders@example.com".to_owned(),
            account: Some("Work".to_owned()),
            move_to: Some("Receipts".to_owned()),
            delete: false,
            mark_read: false,
        };
        assert_eq!(
            keeper.answer(RulesAsk::Add(new.clone())),
            Some(AccountEvent::RuleAdded(Ok("Receipts".to_owned())))
        );
        let told = rules_told(&mut heard).unwrap();
        assert_eq!(told.set.rules.len(), 2);
        assert_eq!(told.set.rules[1].name, "Receipts");
        assert_eq!(told.set.rules[1].account.as_deref(), Some("Work"));
        let written = std::fs::read_to_string(dir.path().join("rules.toml")).unwrap();
        assert!(written.contains("move_to = \"Receipts\""), "{written}");

        // The same name again, and a name a rule in the config has, are
        // refused, and the file is as it was.
        for name in ["receipts", "Old"] {
            let again = keeper.answer(RulesAsk::Add(NewRule {
                name: name.to_owned(),
                ..new.clone()
            }));
            match again {
                Some(AccountEvent::RuleAdded(Err(why))) => {
                    assert!(why.contains("already"), "{why}");
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("rules.toml")).unwrap(),
            written
        );
        assert_eq!(rules_told(&mut heard), None);
    }

    #[test]
    fn the_file_is_made_with_how_to_write_a_rule_when_it_is_asked_for_and_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let (keeper, _heard) = keeper(&dir);
        let file = dir.path().join("rules.toml");
        assert_eq!(
            keeper.answer(RulesAsk::Open),
            Some(AccountEvent::RulesFile(Ok(file.clone())))
        );
        let made = std::fs::read_to_string(&file).unwrap();
        assert!(made.contains("# [[rule]]"), "{made}");
        // What it is made with is no rule at all, and reads without a
        // mistake.
        assert!(quill_core::rules::parse(&made).unwrap().is_empty());
        // And every example in it is a rule that would be read, with
        // the comment marks taken off.
        let uncommented: String = made
            .lines()
            .filter(|line| line.starts_with("# ") && (line.contains(" = ") || line.contains("[[")))
            .map(|line| format!("{}\n", &line[2..]))
            .collect();
        assert_eq!(quill_core::rules::parse(&uncommented).unwrap().len(), 3);

        // One that is there is left exactly as it is.
        std::fs::write(&file, "# mine\n").unwrap();
        keeper.answer(RulesAsk::Open);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# mine\n");
    }
}
