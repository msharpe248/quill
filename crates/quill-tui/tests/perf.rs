//! The spec's performance budgets, measured on a 100,000-message inbox.
//! Ignored by default because it takes a while to fill the mailbox; run:
//!
//! ```sh
//! cargo test --release -p quill-tui --test perf -- --ignored --nocapture
//! ```

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::{Harness, NOW};
use quill_core::types::{
    Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_store::Mailbox;
use quill_tui::data::DataEvent;
use quill_tui::state::Action;

const MESSAGES: usize = 100_000;

/// The demo mailbox with 100,000 more messages in the Work inbox, a fifth
/// of them replies in conversations of five, every one with a body.
fn big() -> Harness {
    let mut harness = Harness::demo(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let people = ["Ana Ruiz", "Kit Moss", "Sam Lee", "Jo Park", "Priya Nair"];
    for batch in 0..MESSAGES / 1000 {
        let added = (0..1000)
            .map(|n| {
                let n = batch * 1000 + n;
                // Every fifth message starts a conversation the next four
                // carry on.
                let root = n - n % 5;
                RemoteMessage {
                    id: RemoteId(format!("big-{n}")),
                    headers: Headers {
                        message_id: Some(format!("<big-{n}@example.com>")),
                        subject: format!("Report {root} on the quarterly parsing figures"),
                        from: vec![Address::new(people[n % 5], "someone@example.com")],
                        to: vec![Address::new("Robin Vale", "robin@example.com")],
                        cc: Vec::new(),
                        date: NOW - (n as i64) * 60,
                    },
                    flags: Flags {
                        seen: n % 3 != 0,
                        ..Flags::default()
                    },
                    size: 2048,
                    has_attachments: n % 7 == 0,
                    thread: if n % 5 == 0 {
                        Vec::new()
                    } else {
                        vec![format!("<big-{root}@example.com>")]
                    },
                }
            })
            .collect();
        harness
            .store
            .apply_delta(
                inbox,
                &SyncDelta {
                    reset: false,
                    added,
                    changed: Vec::new(),
                    removed: Vec::new(),
                    has_more: false,
                    next_cursor: SyncCursor("c".to_owned()),
                },
            )
            .unwrap();
    }
    // Bodies for the search index: every message says something, and one
    // in a thousand says something rare.
    for (index, row) in harness
        .store
        .messages(Mailbox::Folder(inbox))
        .unwrap()
        .iter()
        .enumerate()
    {
        let rare = if index % 1000 == 0 {
            "zeppelin"
        } else {
            "ordinary"
        };
        harness
            .store
            .put_body(
                row.id,
                &format!("The figures for week {index} are attached. Nothing {rare} to report."),
            )
            .unwrap();
    }
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness
}

fn timed<T>(what: &str, work: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = work();
    let took = start.elapsed();
    println!("{what}: {took:?}");
    (out, took)
}

#[test]
#[ignore = "fills a 100,000-message mailbox; run it in release when the numbers are wanted"]
fn a_hundred_thousand_messages_within_the_budgets() {
    let (mut harness, _) = timed("filling the mailbox", big);
    assert!(harness.state.list.rows.len() > MESSAGES);

    // A frame is 16 ms. Moving the cursor and drawing must fit, wherever
    // in the list it is.
    harness.keys("2");
    let mut worst = Duration::ZERO;
    for keys in ["j", "j", "space", "space", "G", "k", "g g", "j"] {
        let start = Instant::now();
        harness.keys(keys);
        let _ = harness.screen();
        worst = worst.max(start.elapsed());
    }
    println!("slowest key and frame: {worst:?}");
    assert!(worst < Duration::from_millis(16), "{worst:?}");

    // Drawing alone, a hundred frames.
    let (_, frames) = timed("100 frames", || {
        for _ in 0..100 {
            let _ = harness.screen();
        }
    });
    assert!(frames / 100 < Duration::from_millis(16));

    // Search: 50 ms, for a rare word, a common word and a filter.
    for text in ["zeppelin", "figures", "from:kit is:unread quarterly"] {
        let query = quill_core::search::parse(text, |_, _, _| None);
        let (found, took) = timed(&format!("search {text:?}"), || {
            harness.store.search(&query, 501).unwrap()
        });
        assert!(!found.is_empty(), "{text}");
        assert!(took < Duration::from_millis(50), "{text}: {took:?}");
    }

    // What the store worker does when anything changes: read the mailbox
    // again. Not a frame, but the list is stale until it is done.
    let (rows, _) = timed("reading the mailbox", || {
        harness.store.messages(Mailbox::Unified).unwrap()
    });
    let (_, took) = timed("taking it in (grouping by conversation)", || {
        harness.act(Action::Data(DataEvent::MessagesLoaded {
            mailbox: Mailbox::Unified,
            rows,
        }));
    });
    println!(
        "the list after a change: stale for the two above together ({took:?} of it on the UI task)"
    );
}
