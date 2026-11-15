//! Telling the user about new mail, what a notification's buttons do, and
//! starting a message from a `mailto:` address.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::types::{FolderRole, LocalMessageId};
use quill_store::Mailbox;
use quill_sync::SyncEvent;
use quill_tui::command::Context;
use quill_tui::compose::Field;
use quill_tui::editor::Editor as _;
use quill_tui::state::{Action, HostRequest};

/// Unread messages of the unified inbox: their ids, senders and subjects.
fn unread(harness: &Harness) -> Vec<(LocalMessageId, String, String)> {
    harness
        .state
        .list
        .rows
        .iter()
        .filter(|row| !row.flags.seen)
        .map(|row| {
            (
                row.id,
                row.headers.from[0].display_name().to_owned(),
                row.headers.subject.clone(),
            )
        })
        .collect()
}

fn arrive(harness: &mut Harness, ids: &[LocalMessageId]) {
    let account = harness.state.accounts[0].id;
    harness.act(Action::Sync(SyncEvent::Arrived {
        account,
        messages: ids.to_vec(),
    }));
}

fn notified(harness: &Harness) -> Vec<&HostRequest> {
    harness
        .host_requests
        .iter()
        .filter(|request| matches!(request, HostRequest::Notify { .. }))
        .collect()
}

#[test]
fn new_mail_is_announced_message_by_message_with_things_to_do_about_it() {
    let mut harness = Harness::demo(200, 60);
    let new = unread(&harness);
    // The first message is read as the mailbox opens; take two others.
    let (first, second) = (new[1].clone(), new[2].clone());
    arrive(&mut harness, &[first.0, second.0]);
    let said = notified(&harness);
    assert_eq!(said.len(), 2);
    let HostRequest::Notify {
        id,
        title,
        subtitle,
        body,
        actions,
    } = said[0]
    else {
        unreachable!()
    };
    assert_eq!(id, &format!("message:{}", first.0.0));
    assert_eq!((title, body), (&first.1, &first.2));
    // The demo has two accounts: which one is said.
    assert!(subtitle.is_some());
    assert_eq!(
        actions,
        &[
            ("archive", "Archive", false),
            ("read", "Mark as Read", false),
            ("reply", "Reply", true)
        ]
    );
}

#[test]
fn many_at_once_are_one_announcement_and_read_mail_is_none() {
    let mut harness = Harness::demo(200, 60);
    let new: Vec<LocalMessageId> = unread(&harness).iter().map(|(id, _, _)| *id).collect();
    assert!(new.len() >= 5);
    arrive(&mut harness, &new[1..6]);
    let said = notified(&harness);
    assert_eq!(said.len(), 1);
    let HostRequest::Notify {
        id, title, actions, ..
    } = said[0]
    else {
        unreachable!()
    };
    assert_eq!((id.as_str(), title.as_str()), ("arrived", "5 new messages"));
    assert!(actions.is_empty());

    // Mail that is already read by the time it is looked up says nothing.
    let mut harness = Harness::demo(200, 60);
    let read: Vec<LocalMessageId> = harness
        .state
        .list
        .rows
        .iter()
        .filter(|row| row.flags.seen)
        .map(|row| row.id)
        .collect();
    arrive(&mut harness, &read);
    assert!(notified(&harness).is_empty());
    // Nor does anything, with notifications turned off.
    harness.env.notifications = false;
    let new: Vec<LocalMessageId> = unread(&harness).iter().map(|(id, _, _)| *id).collect();
    arrive(&mut harness, &new[..1]);
    assert!(notified(&harness).is_empty());
}

#[test]
fn reading_or_filing_a_message_takes_its_notification_down() {
    let mut harness = Harness::demo(200, 60);
    let new = unread(&harness);
    let (one, two) = (new[1].0, new[2].0);
    arrive(&mut harness, &[one, two]);
    // Go to the first of them and open it: it is read.
    let index = harness
        .state
        .list
        .rows
        .iter()
        .position(|row| row.id == one)
        .unwrap();
    harness.keys("g g");
    for _ in 0..index {
        harness.keys("j");
    }
    harness.keys("enter");
    assert!(
        harness
            .host_requests
            .contains(&HostRequest::ClearNotifications(vec![format!(
                "message:{}",
                one.0
            )]))
    );
    // The other is archived from its notification, and its goes too.
    harness.act(Action::Notification {
        id: format!("message:{}", two.0),
        action: Some("archive".to_owned()),
    });
    assert!(
        harness
            .host_requests
            .contains(&HostRequest::ClearNotifications(vec![format!(
                "message:{}",
                two.0
            )]))
    );
    assert!(!harness.state.list.rows.iter().any(|row| row.id == two));
    let archived: Vec<LocalMessageId> = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .filter(|folder| folder.role == FolderRole::Archive)
        .flat_map(|folder| harness.store.messages(Mailbox::Folder(folder.id)).unwrap())
        .map(|row| row.id)
        .collect();
    assert!(archived.contains(&two));
}

#[test]
fn clicking_a_notification_goes_to_its_message_from_wherever_the_user_is() {
    let mut harness = Harness::demo(200, 60);
    let (id, _, subject) = unread(&harness)[2].clone();
    // Somewhere else entirely: another mailbox, with the help open.
    harness.keys("1 j j enter ?");
    assert_ne!(harness.state.mailbox, Mailbox::Unified);
    harness.act(Action::Notification {
        id: format!("message:{}", id.0),
        action: None,
    });
    assert_eq!(harness.state.mailbox, Mailbox::Unified);
    assert_eq!(harness.state.context(), Context::Reader);
    assert_eq!(harness.selected_subject(), subject);
    assert_eq!(harness.state.reader.row.as_ref().unwrap().id, id);
    // Opening it read it.
    assert!(
        harness
            .store
            .message_location(id)
            .unwrap()
            .unwrap()
            .flags
            .seen
    );

    // "Mark as Read" does that and goes nowhere.
    let mut harness = Harness::demo(200, 60);
    let (id, _, _) = unread(&harness)[2].clone();
    let before = harness.selected_subject().to_owned();
    harness.act(Action::Notification {
        id: format!("message:{}", id.0),
        action: Some("read".to_owned()),
    });
    assert!(
        harness
            .store
            .message_location(id)
            .unwrap()
            .unwrap()
            .flags
            .seen
    );
    assert_eq!(harness.selected_subject(), before);
}

#[test]
fn reply_on_a_notification_starts_a_reply_to_that_message() {
    let mut harness = Harness::demo(200, 60);
    let (id, _, subject) = unread(&harness)[1].clone();
    harness.act(Action::Notification {
        id: format!("message:{}", id.0),
        action: Some("reply".to_owned()),
    });
    assert_eq!(harness.state.context(), Context::Compose);
    let compose = harness.state.compose.as_ref().unwrap();
    assert!(
        compose.subject.text().ends_with(&subject),
        "{}",
        compose.subject.text()
    );
    assert_eq!(compose.replied_to, Some(id.0));
}

#[test]
fn a_notification_about_nothing_known_does_nothing() {
    let mut harness = Harness::demo(200, 60);
    let before = harness.screen();
    for id in [
        "message:999999",
        "message:not-a-number",
        "something-else",
        "",
    ] {
        harness.act(Action::Notification {
            id: id.to_owned(),
            action: None,
        });
        harness.act(Action::Notification {
            id: id.to_owned(),
            action: Some("archive".to_owned()),
        });
    }
    assert_eq!(harness.screen(), before);
}

#[test]
fn a_mail_link_starts_a_message_to_the_people_it_names() {
    let mut harness = Harness::demo(200, 60);
    harness.act(Action::Mailto(
        "mailto:ana.ruiz@example.com?cc=kit@example.com&subject=Q4%20roadmap&body=Hello%20Ana%2C"
            .to_owned(),
    ));
    assert_eq!(harness.state.context(), Context::Compose);
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.to.text(), "ana.ruiz@example.com");
    assert_eq!(compose.cc.text(), "kit@example.com");
    assert_eq!(compose.subject.text(), "Q4 roadmap");
    assert!(compose.body.text().starts_with("Hello Ana,"));
    // Everything is filled in, so the writing starts in the body.
    assert_eq!(compose.field, Field::Body);

    // Just an address: the subject is next.
    let mut harness = Harness::demo(200, 60);
    harness.act(Action::Mailto("mailto:ana.ruiz@example.com".to_owned()));
    assert_eq!(
        harness.state.compose.as_ref().unwrap().field,
        Field::Subject
    );
}

#[test]
fn a_mail_link_does_not_put_aside_what_is_being_written_or_run_anything() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("c");
    harness.type_text("kit@example.com");
    harness.act(Action::Mailto("mailto:ana.ruiz@example.com".to_owned()));
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.to.text(), "kit@example.com");
    assert!(
        harness
            .screen()
            .contains("A message is already being written.")
    );

    // What is not a mail address is not acted on, and a hostile one is
    // made safe.
    let mut harness = Harness::demo(200, 60);
    harness.act(Action::Mailto("https://example.com/".to_owned()));
    assert_ne!(harness.state.context(), Context::Compose);
    harness.act(Action::Mailto(
        "mailto:ana@example.com?subject=Hi%1b%5D0%3Bx%07&attach=/etc/passwd".to_owned(),
    ));
    let compose = harness.state.compose.as_ref().unwrap();
    assert!(!compose.subject.text().chars().any(char::is_control));
    assert!(compose.attachments.is_empty());
}
