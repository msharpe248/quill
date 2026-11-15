//! Rules: what to do with mail as it arrives.
//!
//! A rule is a search and some things to do with what it finds: file it,
//! mark it, announce it or keep quiet about it. Rules are asked about mail
//! that has just arrived in an inbox, in the order they are written, and
//! what they do goes through the same queue as what the user does, so it
//! reaches the server the same way and is as safe to repeat.
//!
//! A rule sees what is known of a message when it arrives: who it is from
//! and to, its subject, its marks and whether it has attachments. The
//! words of its body are not known yet.

use std::collections::HashSet;

use quill_core::search::Query;
use quill_core::types::{Flags, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId};
use quill_store::Store;

/// Something to do with arriving mail that matches a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// What to call it when there is something to say about it.
    pub name: String,
    pub query: Query,
    /// Only mail of the account with this name.
    pub account: Option<String>,
    /// A folder of the message's account, by its name or by what it is
    /// for.
    pub move_to: Option<String>,
    pub mark_read: bool,
    pub star: bool,
    /// Whether to announce what it matches. `None`: yes if the message
    /// stays in the inbox, no if this rule files it elsewhere.
    pub notify: Option<bool>,
}

/// What the rules did with a batch of arrivals.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Something was queued for the server.
    pub queued: bool,
    /// Arrivals not to announce.
    pub quiet: HashSet<LocalMessageId>,
    /// Rules that could not do what they say, and why.
    pub problems: Vec<String>,
}

/// Asks every rule about `arrived`, mail new to an inbox of `account`, and
/// queues what they say to do. The first rule that files a message is the
/// one that files it; marks from every matching rule are made.
pub fn apply(
    store: &mut Store,
    rules: &[Rule],
    account: LocalAccountId,
    account_name: &str,
    arrived: &[LocalMessageId],
    now: i64,
) -> quill_store::Result<Outcome> {
    let mut outcome = Outcome::default();
    if rules.is_empty() || arrived.is_empty() {
        return Ok(outcome);
    }
    let mut filed: HashSet<LocalMessageId> = HashSet::new();
    for rule in rules {
        if rule
            .account
            .as_deref()
            .is_some_and(|name| !name.eq_ignore_ascii_case(account_name))
        {
            continue;
        }
        let matched = store.search_among(&rule.query, arrived)?;
        if matched.is_empty() {
            continue;
        }
        let target = match &rule.move_to {
            Some(wanted) => match folder(store, account, wanted)? {
                Some(target) => Some(target),
                None => {
                    outcome.problems.push(format!(
                        "The rule \"{}\" files mail in \"{wanted}\", and {account_name} has no such folder.",
                        rule.name
                    ));
                    None
                }
            },
            None => None,
        };
        for row in matched {
            if rule.mark_read || rule.star {
                // The marks as they stand now: an earlier rule may have
                // made one already.
                let flags = store
                    .message_location(row.id)?
                    .map_or(row.flags, |location| location.flags);
                let wanted = Flags {
                    seen: flags.seen || rule.mark_read,
                    flagged: flags.flagged || rule.star,
                    ..flags
                };
                outcome.queued |= store.queue_flags(row.id, wanted, now)?;
            }
            let moved = match target {
                // No waiting to be undone: nobody is watching.
                Some(target) if !filed.contains(&row.id) && target != row.folder => {
                    store.queue_move(row.id, target, now, now)?.is_some()
                }
                _ => false,
            };
            if moved {
                filed.insert(row.id);
                outcome.queued = true;
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
    }
    Ok(outcome)
}

/// The folder of `account` called `wanted`, or that is for what `wanted`
/// names ("archive", "trash").
fn folder(
    store: &Store,
    account: LocalAccountId,
    wanted: &str,
) -> quill_store::Result<Option<LocalFolderId>> {
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
    Ok(folders
        .iter()
        .find(|folder| folder.name.to_lowercase() == wanted)
        .or_else(|| {
            folders
                .iter()
                .find(|folder| role.is_some_and(|role| folder.role == role))
        })
        .map(|folder| folder.id))
}
