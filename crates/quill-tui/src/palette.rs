//! The command palette: type a few letters of what you want and run it.
//!
//! It lists the commands that apply where it was opened, each with its key,
//! so it is also how a key is learned; and every mailbox, to go to.

use quill_store::Mailbox;

use crate::command::CommandId;
use crate::input::LineInput;

/// The palette, open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub input: LineInput,
    pub selected: usize,
    pub offset: usize,
    /// The commands that applied where it was opened, in the order the
    /// registry lists them. Decided then: once open, the palette itself is
    /// where the keys go.
    pub commands: Vec<CommandId>,
}

/// One thing the palette offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub what: What,
    /// What it is called in the list.
    pub title: String,
    /// A command's key, or nothing.
    pub keys: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    Command(CommandId),
    Mailbox(Mailbox),
    /// A saved search, by what it searches for.
    Search(String),
}

/// How well `name` answers what was `typed`, or `None` when it does not:
/// the letters typed must all appear in the name, in order, whatever their
/// case. Higher is better. Letters that start a word or follow the letter
/// before them count for more, so "ra" finds "Reply to all" before
/// "Archive", and a shorter name wins a tie.
pub fn score(typed: &str, name: &str) -> Option<i32> {
    let wanted: Vec<char> = typed
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if wanted.is_empty() {
        return Some(0);
    }
    let name: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    // The best way of laying the letters over the name, not just the first:
    // `best[j]` is the most that the letters from the current one on can
    // score with the current one placed at `name[j]`. Worked from the last
    // letter back. Names are a few dozen characters, so this is cheap.
    let place = |at: usize| -> i32 {
        let starts_word = at == 0 || !name[at - 1].is_alphanumeric();
        match (starts_word, at) {
            // The very first letter of the name is the best place to start.
            (true, 0) => 12,
            (true, _) => 8,
            (false, _) => 1,
        }
    };
    let mut best: Vec<Option<i32>> = vec![None; name.len()];
    for (index, letter) in wanted.iter().enumerate().rev() {
        let last = index + 1 == wanted.len();
        let mut next: Vec<Option<i32>> = vec![None; name.len()];
        for at in 0..name.len() {
            if name[at] != *letter {
                continue;
            }
            next[at] = if last {
                Some(place(at))
            } else {
                // The rest go somewhere to the right; right next door
                // continues a run. Three letters running count for more
                // than three word starts ("star" is "Star or unstar" before
                // it is "Select a range"); two in the middle of a word
                // count for less than one word start and a letter after it.
                (at + 1..name.len())
                    .filter_map(|after| {
                        let run = if after == at + 1 { 6 } else { 0 };
                        best[after].map(|rest| rest + run)
                    })
                    .max()
                    .map(|rest| place(at) + rest)
            };
        }
        best = next;
    }
    let total = best.into_iter().flatten().max()?;
    // Among names that match as well, the shorter is the closer.
    Some(total * 100 - i32::try_from(name.len()).unwrap_or(99).min(99))
}

/// `items` that answer what was typed, best first; with nothing typed, all
/// of them in the order given.
pub fn matching(typed: &str, items: Vec<Item>) -> Vec<Item> {
    let mut scored: Vec<(i32, usize, Item)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| Some((score(typed, &item.title)?, index, item)))
        .collect();
    // Best first; the registry's order among equals.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str) -> Item {
        Item {
            what: What::Mailbox(Mailbox::Unified),
            title: title.to_owned(),
            keys: String::new(),
        }
    }

    fn titles(typed: &str, names: &[&str]) -> Vec<String> {
        matching(typed, names.iter().map(|name| item(name)).collect())
            .into_iter()
            .map(|item| item.title)
            .collect()
    }

    #[test]
    fn letters_need_only_appear_in_order() {
        assert!(score("arc", "Archive").is_some());
        assert!(score("ahv", "Archive").is_some());
        assert!(score("ARC", "archive").is_some());
        assert!(score("a r c", "Archive").is_some());
        assert!(score("cra", "Archive").is_none());
        assert!(score("archivex", "Archive").is_none());
        // Nothing typed matches everything, equally.
        assert_eq!(score("", "Archive"), Some(0));
        assert_eq!(score("  ", "Reply"), Some(0));
    }

    #[test]
    fn word_starts_and_runs_rank_first() {
        // Both letters start words; one does; neither does.
        assert_eq!(
            titles("ra", &["Narrate", "Reply to all", "Mark read or unread"]),
            ["Reply to all", "Mark read or unread", "Narrate"]
        );
        // A run beats the same letters apart.
        assert_eq!(
            titles("del", &["Add a label", "Delete"]),
            ["Delete", "Add a label"]
        );
        // Among equals, the shorter name; then the order given.
        assert_eq!(
            titles("re", &["Reply to all", "Reply", "Refresh"]),
            ["Reply", "Refresh", "Reply to all"]
        );
    }

    #[test]
    fn the_best_placing_of_the_letters_counts_not_the_first() {
        // The first "r" is in "Mark", but the one that starts "read" is
        // the better fit; and "un" is found at the start of "unread".
        assert!(score("r", "Mark read") > score("r", "Mark bead"));
        assert_eq!(
            score("ru", "Mark read or unread"),
            score("ru", "Bark read or unread")
        );
        assert!(score("un", "Run unread") > score("un", "Run inroad nu"));
    }

    #[test]
    fn with_nothing_typed_everything_is_offered_in_order() {
        assert_eq!(
            titles("", &["Zoom", "Archive", "Move"]),
            ["Zoom", "Archive", "Move"]
        );
        assert!(titles("xyz", &["Zoom", "Archive"]).is_empty());
    }

    #[test]
    fn accents_and_other_scripts_do_not_break_it() {
        assert!(score("zo", "Zoë / Inbox").is_some());
        assert!(score("设", "设计评审").is_some());
        assert!(score("🎉", "Party 🎉").is_some());
    }
}
