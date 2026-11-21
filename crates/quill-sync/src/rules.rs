//! Rules: what to do with mail by itself.
//!
//! A rule is a search and some things to do with what it finds: file it,
//! delete it, mark it, announce it or keep quiet about it. Rules are asked
//! about mail in an inbox, in the order they are written, and what they do
//! goes through the same queue as what the user does, so it reaches the
//! server the same way and is as safe to repeat.
//!
//! They are asked at three times: about mail that has just arrived; about
//! mail already here that they have not been asked about, when they are
//! switched on (catching up); and about the mail of the last while when
//! the user runs them by hand. The last two stop at the rule's look-back.
//!
//! Nothing a rule does is for good. Its delete is a move to the trash,
//! with where the message was noted so that it can be put back, and every
//! change is written in the store's log with what is needed to undo it.

use std::collections::{HashMap, HashSet};

use quill_core::search::Query;
use quill_core::types::{Flags, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_store::{NewAgentEntry, OperationId, Store};
use serde_json::{Value, json};

/// The way in a rule's line of the log is written under, where a command
/// for a shell has `shell` and an agent `mcp`.
pub const LOG_WAY: &str = "rule";
/// And its action.
pub const LOG_ACTION: &str = "rule";

/// Something to do with mail that matches a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// What it is called: in the list of rules, the log and notices.
    pub name: String,
    /// What it matches: any one of these.
    pub queries: Vec<Query>,
    /// Only mail of the account with this name.
    pub account: Option<String>,
    /// A folder of the message's account, by its name or by what it is
    /// for.
    pub move_to: Option<String>,
    /// To the account's trash.
    pub delete: bool,
    pub mark_read: bool,
    pub star: bool,
    /// Whether to announce what it matches. `None`: yes if the message
    /// stays in the inbox, no if this rule files it elsewhere.
    pub notify: Option<bool>,
    /// Switched off, it is asked nothing by itself. It can still be run
    /// by hand.
    pub enabled: bool,
    /// How far back, in seconds, a run over mail already here goes.
    /// `None`: as far as the set says.
    pub look_back: Option<i64>,
}

impl Rule {
    /// Whether what it matches can be told only of a downloaded message.
    pub fn needs_body(&self) -> bool {
        self.queries.iter().any(Query::needs_body)
    }

    /// What it does, in a few words: "move to Receipts, mark read".
    pub fn does(&self) -> String {
        let mut parts = Vec::new();
        if self.delete {
            parts.push("delete".to_owned());
        }
        if let Some(folder) = &self.move_to {
            parts.push(format!("move to {folder}"));
        }
        if self.mark_read {
            parts.push("mark read".to_owned());
        }
        if self.star {
            parts.push("star".to_owned());
        }
        match self.notify {
            Some(true) => parts.push("announce".to_owned()),
            Some(false) => parts.push("keep quiet".to_owned()),
            None => {}
        }
        parts.join(", ")
    }
}

/// The rules as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSet {
    /// Whether rules run by themselves at all.
    pub on: bool,
    /// How far back, in seconds, a rule that does not say goes over mail
    /// already here.
    pub look_back: i64,
    /// In the order they are asked.
    pub rules: Vec<Rule>,
    /// Some of the rules as written could not be read, and are not
    /// here. While that is so no mail is noted as asked about: it is
    /// asked again, of all the rules, once they are put right.
    pub held: bool,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self {
            on: true,
            look_back: 7 * 24 * 60 * 60,
            rules: Vec::new(),
            held: false,
        }
    }
}

impl RuleSet {
    /// A set of these rules, switched on, with the usual look-back.
    pub fn of(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            ..Self::default()
        }
    }

    /// The rules that are asked by themselves: none while the set is off.
    pub fn active(&self) -> Vec<&Rule> {
        if !self.on {
            return Vec::new();
        }
        self.rules.iter().filter(|rule| rule.enabled).collect()
    }

    /// Whether any rule that is asked by itself needs the message
    /// downloaded.
    pub fn needs_body(&self) -> bool {
        self.active().iter().any(|rule| rule.needs_body())
    }

    /// The furthest back any active rule goes.
    pub fn longest_look_back(&self) -> i64 {
        self.active()
            .iter()
            .map(|rule| rule.look_back.unwrap_or(self.look_back))
            .max()
            .unwrap_or(self.look_back)
    }

    /// The rule of this name, whatever the case it is written in.
    pub fn named(&self, name: &str) -> Option<&Rule> {
        let wanted = name.trim().to_lowercase();
        self.rules
            .iter()
            .find(|rule| rule.name.to_lowercase() == wanted)
    }
}

/// One asking of some rules about some mail.
#[derive(Debug, Clone, Copy)]
pub struct Run<'a> {
    pub account: LocalAccountId,
    pub account_name: &'a str,
    /// The mail to ask about.
    pub among: &'a [LocalMessageId],
    /// Ask a rule only about mail dated within its look-back, this being
    /// the look-back of a rule that has none of its own. `None`: whatever
    /// the mail's date, which is how arriving mail is asked about.
    pub look_back: Option<i64>,
    /// Change nothing: say what would be changed.
    pub dry: bool,
    /// When the server may be told of a move. Now, where nobody is
    /// watching; later, where the user may still take it back.
    pub not_before: i64,
    pub now: i64,
}

/// What the rules did with some mail, or would.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Something was queued for the server.
    pub queued: bool,
    /// Arrivals not to announce.
    pub quiet: HashSet<LocalMessageId>,
    /// Rules that could not do what they say, and why.
    pub problems: Vec<String>,
    /// The messages changed, each once, in the order they were.
    pub changed: Vec<LocalMessageId>,
    /// The moves queued, for the user's undo to take back as one.
    pub operations: Vec<OperationId>,
    /// What each rule did, in the order asked. Only rules that did
    /// something.
    pub did: Vec<Did>,
}

/// What one rule did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Did {
    pub rule: String,
    /// The messages it filed or deleted.
    pub filed: Vec<LocalMessageId>,
    /// The folder it filed them in, by its name.
    pub to: Option<String>,
    /// It filed them in the trash because it deletes.
    pub deleted: bool,
    /// The messages it marked.
    pub marked: Vec<LocalMessageId>,
    /// Its line in the log, once written.
    pub entry: Option<i64>,
}

impl Did {
    /// How many messages it changed.
    pub fn count(&self) -> usize {
        let mut all: HashSet<LocalMessageId> = self.filed.iter().copied().collect();
        all.extend(self.marked.iter().copied());
        all.len()
    }

    /// What it did, as the log says it: "moved 3 messages to Receipts".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.filed.is_empty() {
            let count = messages(self.filed.len());
            parts.push(match (&self.to, self.deleted) {
                (_, true) => format!("deleted {count}"),
                (Some(folder), false) => format!("moved {count} to {folder}"),
                (None, false) => format!("moved {count}"),
            });
        }
        if !self.marked.is_empty() {
            parts.push(format!("marked {}", messages(self.marked.len())));
        }
        parts.join(", ")
    }
}

/// "1 message", "3 messages".
pub fn messages(count: usize) -> String {
    if count == 1 {
        "1 message".to_owned()
    } else {
        format!("{count} messages")
    }
}

/// Asks `rules` about the mail `run` names, and does what they say. The
/// first rule that files or deletes a message is the one that does; marks
/// from every matching rule are made. Each rule that changed something
/// has a line in the log, with what is needed to take it back.
pub fn apply(store: &mut Store, rules: &[&Rule], run: &Run<'_>) -> quill_store::Result<Outcome> {
    let mut outcome = Outcome::default();
    if rules.is_empty() || run.among.is_empty() {
        return Ok(outcome);
    }
    let mut filed: HashSet<LocalMessageId> = HashSet::new();
    let mut changed: HashSet<LocalMessageId> = HashSet::new();
    // Marks a dry run would have made, so that it counts each once.
    let mut pretended: HashMap<LocalMessageId, Flags> = HashMap::new();
    for rule in rules {
        if rule
            .account
            .as_deref()
            .is_some_and(|name| !name.eq_ignore_ascii_case(run.account_name))
        {
            continue;
        }
        let since = run
            .look_back
            .map(|usual| run.now.saturating_sub(rule.look_back.unwrap_or(usual)));
        // Any one of its searches is a match; a message found by two is
        // one message.
        let mut matched = Vec::new();
        let mut seen: HashSet<LocalMessageId> = HashSet::new();
        for query in &rule.queries {
            for row in store.search_among(query, run.among)? {
                if since.is_some_and(|since| row.headers.date < since) {
                    continue;
                }
                if seen.insert(row.id) {
                    matched.push(row);
                }
            }
        }
        if matched.is_empty() {
            continue;
        }
        let wanted = if rule.delete {
            Some("trash")
        } else {
            rule.move_to.as_deref()
        };
        let target = match wanted {
            Some(wanted) => match folder(store, run.account, wanted, rule.delete)? {
                Some(target) => Some(target),
                None if rule.delete => {
                    outcome.problems.push(format!(
                        "The rule \"{}\" deletes mail, and {} has no trash folder to move it \
                         to. The mail was left alone.",
                        rule.name, run.account_name
                    ));
                    None
                }
                None => {
                    outcome.problems.push(format!(
                        "The rule \"{}\" files mail in \"{wanted}\", and {} has no such folder.",
                        rule.name, run.account_name
                    ));
                    None
                }
            },
            None => None,
        };
        let mut did = Did {
            rule: rule.name.clone(),
            filed: Vec::new(),
            to: target.as_ref().map(|(_, name)| name.clone()),
            deleted: rule.delete,
            marked: Vec::new(),
            entry: None,
        };
        let mut moved_before: Vec<Value> = Vec::new();
        let mut marked_before: Vec<Value> = Vec::new();
        for row in matched {
            if rule.mark_read || rule.star {
                // The marks as they stand now: an earlier rule may have
                // made one already.
                let flags = match pretended.get(&row.id) {
                    Some(flags) => *flags,
                    None => store
                        .message_location(row.id)?
                        .map_or(row.flags, |location| location.flags),
                };
                let wanted = Flags {
                    seen: flags.seen || rule.mark_read,
                    flagged: flags.flagged || rule.star,
                    ..flags
                };
                if wanted != flags {
                    let made = if run.dry {
                        pretended.insert(row.id, wanted);
                        true
                    } else {
                        store.queue_flags(row.id, wanted, run.now)?
                    };
                    if made {
                        outcome.queued |= !run.dry;
                        did.marked.push(row.id);
                        marked_before.push(json!({ "id": row.id.0, "flags": flags_text(flags) }));
                    }
                }
            }
            let moved = match &target {
                Some((target, _)) if !filed.contains(&row.id) && *target != row.folder => {
                    if run.dry {
                        true
                    } else {
                        if rule.delete {
                            store.note_trashed(row.id, row.folder, run.now)?;
                        }
                        match store.queue_move(row.id, *target, run.now, run.not_before)? {
                            Some(operation) => {
                                outcome.operations.push(operation);
                                true
                            }
                            None => {
                                if rule.delete {
                                    store.forget_trashed(row.id)?;
                                }
                                false
                            }
                        }
                    }
                }
                _ => false,
            };
            if moved {
                filed.insert(row.id);
                outcome.queued |= !run.dry;
                did.filed.push(row.id);
                moved_before.push(json!({
                    "id": row.id.0,
                    "from": row.folder.0,
                    "trashed": rule.delete,
                }));
            }
            // Mail filed out of the inbox is not news unless the rule
            // says it is; mail left there is, unless it says it is not.
            match rule.notify {
                Some(false) => {
                    outcome.quiet.insert(row.id);
                }
                Some(true) => {
                    outcome.quiet.remove(&row.id);
                }
                None if moved => {
                    outcome.quiet.insert(row.id);
                }
                None => {}
            }
        }
        if did.filed.is_empty() && did.marked.is_empty() {
            continue;
        }
        for id in did.filed.iter().chain(&did.marked) {
            if changed.insert(*id) {
                outcome.changed.push(*id);
            }
        }
        if !run.dry {
            let undo = json!({ "op": "rule", "moved": moved_before, "marked": marked_before });
            let summary = did.summary();
            did.entry = Some(store.log_agent(&NewAgentEntry {
                at: run.now,
                way: LOG_WAY,
                agent: &did.rule,
                action: LOG_ACTION,
                change: true,
                summary: &summary,
                count: u32::try_from(did.count()).unwrap_or(u32::MAX),
                undo: Some(&undo.to_string()),
                due: None,
            })?);
        }
        outcome.did.push(did);
    }
    Ok(outcome)
}

/// What the rules did, said in a line for the user: "Rules filed 3 and
/// marked 2". `None` when they did nothing.
pub fn said(outcome: &Outcome) -> Option<String> {
    let filed: HashSet<LocalMessageId> = outcome
        .did
        .iter()
        .filter(|did| !did.deleted)
        .flat_map(|did| did.filed.iter().copied())
        .collect();
    let deleted: HashSet<LocalMessageId> = outcome
        .did
        .iter()
        .filter(|did| did.deleted)
        .flat_map(|did| did.filed.iter().copied())
        .collect();
    let marked: HashSet<LocalMessageId> = outcome
        .did
        .iter()
        .flat_map(|did| did.marked.iter().copied())
        .filter(|id| !filed.contains(id) && !deleted.contains(id))
        .collect();
    let mut parts = Vec::new();
    if !filed.is_empty() {
        parts.push(format!("filed {}", filed.len()));
    }
    if !deleted.is_empty() {
        parts.push(format!("deleted {}", deleted.len()));
    }
    if !marked.is_empty() {
        parts.push(format!("marked {}", marked.len()));
    }
    let last = parts.pop()?;
    Some(if parts.is_empty() {
        format!("Rules {last}")
    } else {
        format!("Rules {} and {last}", parts.join(", "))
    })
}

/// Four digits, as the log's undo writes flags: seen, flagged, answered,
/// draft. What `quill undo` reads back.
fn flags_text(flags: Flags) -> String {
    [flags.seen, flags.flagged, flags.answered, flags.draft]
        .iter()
        .map(|on| if *on { '1' } else { '0' })
        .collect()
}

/// The folder of `account` called `wanted`, or that is for what `wanted`
/// names ("archive", "trash"), with its name. `only_role`: by what it is
/// for and nothing else, which is how a delete finds the trash: a folder
/// the user happens to have called "trash" is not where deleted mail
/// goes.
fn folder(
    store: &Store,
    account: LocalAccountId,
    wanted: &str,
    only_role: bool,
) -> quill_store::Result<Option<(LocalFolderId, String)>> {
    let wanted = wanted.trim().to_lowercase();
    let role = match wanted.as_str() {
        "inbox" => Some(FolderRole::Inbox),
        "archive" => Some(FolderRole::Archive),
        "trash" => Some(FolderRole::Trash),
        "junk" | "spam" => Some(FolderRole::Junk),
        "sent" => Some(FolderRole::Sent),
        "drafts" => Some(FolderRole::Drafts),
        _ => None,
    };
    let folders: Vec<_> = store
        .folders()?
        .into_iter()
        .filter(|folder| folder.account == account)
        .collect();
    // A folder's own name first: someone with a folder called "Archive"
    // that is not the archive means that one.
    // By the whole name Google's own folders have on its servers too:
    // a rule written when they were shown so still means the folder.
    let by_name = folders
        .iter()
        .find(|folder| !only_role && folder.name.to_lowercase() == wanted)
        .or_else(|| {
            folders.iter().find(|folder| {
                !only_role && quill_core::providers::names_folder(&wanted, &folder.name)
            })
        });
    Ok(by_name
        .or_else(|| {
            folders
                .iter()
                .find(|folder| role.is_some_and(|role| folder.role == role))
        })
        .map(|folder| (folder.id, folder.name.clone())))
}

/// The most mail a run by hand asks about in one account.
const BY_HAND_MOST: usize = 5000;

/// What a run by hand did, or would, over every account.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Hand {
    /// The messages changed, each once, newest first within an account.
    pub changed: Vec<LocalMessageId>,
    /// What each rule did, over every account, in the order asked.
    pub did: Vec<Did>,
    pub problems: Vec<String>,
    /// The moves made, grouped: one undo takes them all back.
    pub operations: Vec<OperationId>,
    pub queued: bool,
}

impl Hand {
    /// What it would do, for the bar over a preview: "would move 14
    /// messages to Receipts", or with several rules "would change 20
    /// messages".
    pub fn would(&self) -> String {
        match self.did.as_slice() {
            [] => "matches nothing in the inbox".to_owned(),
            [only] => format!(
                "would {}",
                only.summary()
                    .replace("moved ", "move ")
                    .replace("deleted ", "delete ")
                    .replace("marked ", "mark ")
            ),
            _ => format!("would change {}", messages(self.changed.len())),
        }
    }
}

/// Runs rules by hand: one rule, by its name, or every rule that is
/// switched on, over the mail in every account's inbox within each rule's
/// look-back, whether or not it was asked about before. `dry` changes
/// nothing and says what would be. `undo_until`: when the server is told
/// of the moves; until then the user's undo takes them back as one.
///
/// A rule that is switched off can still be run by its name: that is how
/// one is tried before it is switched on.
pub fn by_hand(
    store: &mut Store,
    set: &RuleSet,
    which: Option<&str>,
    dry: bool,
    now: i64,
    undo_until: i64,
) -> quill_store::Result<Hand> {
    let rules: Vec<&Rule> = match which {
        Some(name) => set.named(name).into_iter().collect(),
        None => set.rules.iter().filter(|rule| rule.enabled).collect(),
    };
    let mut hand = Hand::default();
    if rules.is_empty() {
        return Ok(hand);
    }
    let furthest = rules
        .iter()
        .map(|rule| rule.look_back.unwrap_or(set.look_back))
        .max()
        .unwrap_or(set.look_back);
    for account in store.accounts()? {
        let among = store.inbox_mail(
            account.id,
            now.saturating_sub(furthest),
            false,
            BY_HAND_MOST,
        )?;
        if among.is_empty() {
            continue;
        }
        let outcome = apply(
            store,
            &rules,
            &Run {
                account: account.id,
                account_name: &account.name,
                among: &among,
                look_back: Some(set.look_back),
                dry,
                not_before: undo_until,
                now,
            },
        )?;
        if !dry {
            // Asked now, by the user: not asked again by themselves.
            store.mark_ruled(&among, now)?;
        }
        hand.queued |= outcome.queued;
        hand.changed.extend(outcome.changed);
        hand.operations.extend(outcome.operations);
        hand.problems.extend(outcome.problems);
        for did in outcome.did {
            // One rule over two accounts is one rule: said once.
            match hand.did.iter_mut().find(|known| {
                known.rule == did.rule && known.to == did.to && known.deleted == did.deleted
            }) {
                Some(known) => {
                    known.filed.extend(did.filed);
                    known.marked.extend(did.marked);
                }
                None => hand.did.push(did),
            }
        }
    }
    if !dry {
        store.group_operations(&hand.operations)?;
    }
    Ok(hand)
}

/// What a run by hand did, in a line for the user.
pub fn said_by_hand(hand: &Hand) -> String {
    let outcome = Outcome {
        did: hand.did.clone(),
        ..Outcome::default()
    };
    said(&outcome).unwrap_or_else(|| "Rules changed nothing".to_owned())
}
