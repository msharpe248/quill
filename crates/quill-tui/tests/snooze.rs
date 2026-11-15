//! Putting mail aside until a time, and sending mail at a time.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_store::{DraftState, Mailbox};
use quill_tui::command::Context;
use quill_tui::state::{Action, HostRequest};

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

fn subjects(harness: &Harness) -> Vec<String> {
    harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.clone())
        .collect()
}

#[test]
fn b_asks_when_and_the_message_is_put_aside_until_then() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("b");
    assert_eq!(harness.state.context(), Context::When);
    assert!(harness.screen().contains(" Snooze until "));
    assert!(harness.screen().contains("Such as: 30m  2h"));
    // Letters are typed into the box, not run as commands.
    harness.type_text("tomorrow 9");
    harness.keys("enter");
    assert_ne!(harness.state.context(), Context::When);
    assert_eq!(subjects(&harness), all[1..]);
    assert!(
        status(&harness).contains("Snoozed. It comes back Sun 15 Nov 2026 09:00"),
        "{}",
        status(&harness)
    );
    // It is in Snoozed, in the sidebar while there is anything in it, and
    // says when it is due and not when it came.
    let screen = harness.screen();
    assert!(screen.contains("Snoozed"), "{screen}");
    let snoozed = harness.store.messages(Mailbox::Snoozed).unwrap();
    assert_eq!(snoozed.len(), 1);
    assert_eq!(snoozed[0].headers.subject, all[0]);
    assert_eq!(snoozed[0].snoozed_until, Some(NOW + 23 * 3600 + 30 * 60));
}

#[test]
fn what_is_not_a_time_is_said_and_nothing_happens() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("b enter");
    assert!(harness.screen().contains("Type a time."));
    harness.type_text("whenever");
    harness.keys("enter");
    assert!(
        harness
            .screen()
            .contains("That is not a time to come that Quill can read.")
    );
    assert_eq!(harness.state.context(), Context::When);
    // Typing again clears the complaint; escape leaves everything as it was.
    harness.keys("backspace");
    assert!(!harness.screen().contains("That is not a time"));
    harness.keys("esc");
    assert_ne!(harness.state.context(), Context::When);
    assert_eq!(subjects(&harness), all);
    assert!(harness.store.messages(Mailbox::Snoozed).unwrap().is_empty());
}

#[test]
fn when_its_time_comes_the_message_is_back_on_top_and_announced() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    // The third message: when it comes back it is first, not third.
    harness.keys("j j b");
    harness.type_text("2h");
    harness.keys("enter");
    assert!(!subjects(&harness).contains(&all[2]));

    // An hour on: not yet.
    harness.act(Action::ClockTick(NOW + 3600));
    assert!(!subjects(&harness).contains(&all[2]));
    // Two hours on the store's clock is not something a test can wait
    // for: bring it back as the worker would when it is due.
    let id = harness.store.messages(Mailbox::Snoozed).unwrap()[0].id;
    let woke = harness.store.wake_snoozed(NOW + 2 * 3600, None).unwrap();
    assert_eq!(woke, [id]);
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    harness.act(Action::Data(quill_tui::data::DataEvent::Arrived(
        harness.store.message_rows(&woke).unwrap(),
    )));
    assert_eq!(subjects(&harness)[0], all[2]);
    assert!(
        harness
            .host_requests
            .iter()
            .any(|request| matches!(request, HostRequest::Notify { body, .. } if *body == all[2]))
    );
    assert!(!harness.screen().contains("Snoozed "));
}

#[test]
fn the_clock_asks_for_what_is_due_and_only_then() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("b");
    harness.type_text("30m");
    harness.keys("enter");
    let asked = |harness: &mut Harness, at: i64| {
        let effects = harness.state.update(&harness.env, Action::ClockTick(at));
        effects.data.contains(&quill_tui::data::DataRequest::Wake)
    };
    assert!(!asked(&mut harness, NOW + 60));
    assert!(asked(&mut harness, NOW + 30 * 60));
    // Asked once, not at every tick after.
    assert!(!asked(&mut harness, NOW + 31 * 60));
}

#[test]
fn several_are_snoozed_together_and_now_brings_them_back() {
    let mut harness = Harness::demo(200, 60);
    let all = subjects(&harness);
    harness.keys("x x b");
    assert!(harness.screen().contains(" Snooze 2 messages until "));
    harness.type_text("fri");
    harness.keys("enter");
    assert_eq!(subjects(&harness), all[2..]);
    assert!(status(&harness).contains("Snoozed 2 messages. They come back Fri 20 Nov"));
    assert!(!harness.state.list.has_marks());

    // In the Snoozed mailbox, both; "now" for one of them brings it back.
    harness.keys("1 j enter");
    assert_eq!(harness.state.mailbox, Mailbox::Snoozed);
    assert_eq!(subjects(&harness).len(), 2);
    harness.keys("b");
    harness.type_text("now");
    harness.keys("enter");
    assert_eq!(subjects(&harness).len(), 1);
    assert!(status(&harness).contains("Back in its folder."));
}

#[test]
fn a_message_can_be_sent_at_a_time_and_waits_in_the_outbox_until_then() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    // Past the completed address and the empty Cc, to the subject.
    harness.keys("tab tab tab");
    harness.type_text("Later");
    harness.keys("tab");
    harness.type_text("Not yet.");
    harness.keys("ctrl-l");
    assert_eq!(harness.state.context(), Context::When);
    let screen = harness.screen();
    assert!(screen.contains(" Send at "), "{screen}");
    assert!(screen.contains("It waits in the Outbox and goes at that time"), "{screen}");
    harness.type_text("tomorrow 7");
    harness.keys("enter");
    assert_ne!(
        harness.state.context(),
        Context::Compose,
        "{}",
        harness.screen()
    );
    let outbox = harness.store.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].state, DraftState::Outbox);
    // Tomorrow at seven, to within the second or two the test took.
    let due = outbox[0].send_after.unwrap();
    let real_now = jiff::Timestamp::now().as_second();
    let wanted = real_now + (21 * 3600 + 30 * 60);
    assert!((due - wanted).abs() < 5, "{due} against {wanted}");
    assert!(
        status(&harness).contains("In the Outbox. It goes in about 22 hours"),
        "{}",
        status(&harness)
    );
    // And it can still be taken back.
    harness.keys("U");
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn escaping_the_time_leaves_the_message_being_written() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    harness.keys("ctrl-l esc");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.store.outbox().unwrap().is_empty());
}
