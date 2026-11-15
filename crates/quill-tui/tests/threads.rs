//! Conversations in the message list.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_tui::data::DataEvent;
use quill_tui::state::Action;

/// The demo mailbox with a conversation of three at the top of the Work
/// inbox, a message from someone else arriving in the middle of it.
fn with_a_conversation() -> Harness {
    let mut harness = Harness::demo(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let message =
        |name: &str, who: &str, subject: &str, minutes: i64, follows: &[&str]| RemoteMessage {
            id: RemoteId(name.to_owned()),
            headers: Headers {
                message_id: Some(format!("<{name}@example.com>")),
                subject: subject.to_owned(),
                from: vec![Address::new(who, "someone@example.com")],
                to: Vec::new(),
                cc: Vec::new(),
                date: NOW + minutes * 60,
            },
            flags: Flags::default(),
            size: 1,
            has_attachments: false,
            thread: follows
                .iter()
                .map(|name| format!("<{name}@example.com>"))
                .collect(),
        };
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![
                    message("plan-1", "Kit Moss", "Plan", 1, &[]),
                    message("plan-2", "Sam Lee", "Re: Plan", 2, &["plan-1"]),
                    message("other", "Jo Park", "Unrelated", 3, &[]),
                    message("plan-3", "Kit Moss", "Re: Plan", 4, &["plan-1", "plan-2"]),
                ],
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.keys("2 g g");
    harness
}

fn top(harness: &Harness, count: usize) -> Vec<(String, String)> {
    harness
        .state
        .list
        .rows
        .iter()
        .take(count)
        .map(|row| {
            (
                row.headers.from[0].display_name().to_owned(),
                row.headers.subject.clone(),
            )
        })
        .collect()
}

fn pair(who: &str, subject: &str) -> (String, String) {
    (who.to_owned(), subject.to_owned())
}

#[test]
fn a_conversation_is_listed_together_where_its_newest_message_would_be() {
    let harness = with_a_conversation();
    // The conversation's newest message is the newest of all, so the
    // conversation is first, in the order it was written; the message
    // that arrived in the middle of it comes after.
    assert_eq!(
        top(&harness, 4),
        [
            pair("Kit Moss", "Plan"),
            pair("Sam Lee", "Re: Plan"),
            pair("Kit Moss", "Re: Plan"),
            pair("Jo Park", "Unrelated"),
        ]
    );
    // The replies hang from the first, and nothing else does.
    let screen = harness.screen();
    assert_eq!(screen.matches("└ Re: Plan").count(), 2, "{screen}");
    assert!(!screen.contains("└ Plan") && !screen.contains("└ Unrelated"));
    assert!(!screen.contains("└ Q4 roadmap"));
}

#[test]
fn grouping_can_be_switched_off_and_the_cursor_stays_on_its_message() {
    let mut harness = with_a_conversation();
    harness.keys("j");
    assert_eq!(harness.selected_subject(), "Re: Plan");
    let on = harness.state.list.rows[harness.state.list.selected].id;
    harness.keys("T");
    // One by one, newest first, and nothing hangs from anything.
    assert_eq!(
        top(&harness, 4),
        [
            pair("Kit Moss", "Re: Plan"),
            pair("Jo Park", "Unrelated"),
            pair("Sam Lee", "Re: Plan"),
            pair("Kit Moss", "Plan"),
        ]
    );
    assert!(!harness.screen().contains("└ "));
    assert_eq!(harness.state.list.rows[harness.state.list.selected].id, on);
    assert!(
        harness
            .screen()
            .contains("Messages are listed one by one, newest first.")
    );
    harness.keys("T");
    assert_eq!(top(&harness, 1), [pair("Kit Moss", "Plan")]);
    assert_eq!(harness.state.list.rows[harness.state.list.selected].id, on);
}

#[test]
fn with_grouping_off_in_the_config_the_key_turns_it_on() {
    let mut harness = with_a_conversation();
    harness.env.threads = false;
    harness.act(Action::Data(DataEvent::StoreChanged));
    assert_eq!(top(&harness, 1), [pair("Kit Moss", "Re: Plan")]);
    harness.keys("T");
    assert_eq!(top(&harness, 1), [pair("Kit Moss", "Plan")]);
    assert!(
        harness
            .screen()
            .contains("Conversations are listed together.")
    );
}

#[test]
fn what_a_search_finds_is_listed_message_by_message() {
    let mut harness = with_a_conversation();
    harness.keys("/");
    harness.type_text("plan");
    harness.keys("enter");
    assert!(harness.state.list.rows.len() >= 3);
    assert!(!harness.screen().contains("└ "));
}

#[test]
fn acting_on_a_message_in_a_conversation_acts_on_that_message() {
    let mut harness = with_a_conversation();
    harness.keys("j e");
    // The middle of the conversation went; the rest is still together.
    assert_eq!(
        top(&harness, 2),
        [pair("Kit Moss", "Plan"), pair("Kit Moss", "Re: Plan")]
    );
    assert_eq!(harness.screen().matches("└ Re: Plan").count(), 1);
}
