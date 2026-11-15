//! One key to leave a mailing list: asked about first, then done the way
//! the list's own header says, and never by fetching anything.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_tui::command::{CommandId, Context};
use quill_tui::compose::Field;
use quill_tui::data::DataEvent;
use quill_tui::setup::{AccountEvent, AccountRequest};
use quill_tui::state::{Action, HostRequest, NoticeKind, UnsubscribeHow};

const SUBJECT: &str = "New this November";
const PAGE: &str = "https://lists.harbourbooks.example.com/u/r0b1n";
const MAIL: &str = "mailto:leave@harbourbooks.example.com";
const ONE_CLICK: &str = "List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n";

/// A message from a list, with these lines among its headers.
fn from_a_list(headers: &str) -> String {
    format!(
        "From: Harbour Books <news@harbourbooks.example.com>\r\n\
         To: Robin Vale <robin@example.com>\r\n\
         Subject: {SUBJECT}\r\n\
         {headers}\
         \r\n\
         Three new arrivals this month.\r\n"
    )
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// Puts a message of our own at the top of the Work inbox and opens it.
/// `service`: there is an accounts service to make requests with, as there
/// is for every mailbox but the demo.
fn reading(raw: &str, service: bool) -> Harness {
    let mut harness = if service {
        Harness::with_accounts(200, 60)
    } else {
        Harness::demo(200, 60)
    };
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("list-1".to_owned());
    let parsed = quill_mime::parse(raw.as_bytes(), NOW).unwrap();
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![RemoteMessage {
                    id: remote.clone(),
                    headers: Headers {
                        date: NOW + 60,
                        ..parsed.headers.clone()
                    },
                    flags: Flags::default(),
                    size: raw.len() as u64,
                    has_attachments: false,
                    thread: Vec::new(),
                }],
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor("c".to_owned()),
            },
        )
        .unwrap();
    let id = harness
        .store
        .local_message_id(inbox, &remote)
        .unwrap()
        .unwrap();
    harness
        .store
        .put_message(id, &parsed.body, raw.as_bytes())
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.keys("2 g g enter");
    assert_eq!(harness.selected_subject(), parsed.headers.subject);
    harness
}

/// Whether anything at all was asked of the world outside: the accounts
/// service, the browser, or the outbox.
fn nothing_left_quill(harness: &Harness) -> bool {
    harness.account_requests.is_empty()
        && !harness
            .host_requests
            .iter()
            .any(|request| matches!(request, HostRequest::OpenUrl(_)))
        && harness.store.outbox().unwrap().is_empty()
}

#[test]
fn the_reader_says_when_a_message_can_be_unsubscribed_from() {
    let harness = reading(
        &from_a_list(&format!("List-Unsubscribe: <{PAGE}>\r\n")),
        true,
    );
    assert!(
        harness.row_with("[3] Reader").contains("X unsubscribes"),
        "{}",
        harness.row_with("[3] Reader")
    );
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageUnsubscribe)
    );

    // An ordinary message says nothing of the kind, and the key does
    // nothing there.
    let mut harness = Harness::with_accounts(200, 60);
    harness.keys("enter");
    assert!(!harness.screen().contains("unsubscribes"));
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageUnsubscribe)
    );
    harness.keys("X");
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(harness.state.unsubscribe.is_none());
}

#[test]
fn one_click_is_asked_about_first_and_then_goes_through_the_service() {
    let raw = from_a_list(&format!(
        "List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}"
    ));
    let mut harness = reading(&raw, true);
    harness.keys("X");
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    let screen = harness.screen();
    assert!(screen.contains("Unsubscribe"), "{screen}");
    // Who it is from, and exactly what yes will do.
    assert!(
        screen.contains("From:      Harbour Books <news@harbourbooks.example.com>"),
        "{screen}"
    );
    assert!(
        screen.contains("Quill will tell lists.harbourbooks.example.com to unsubscribe you."),
        "{screen}"
    );
    assert!(screen.contains(PAGE), "{screen}");
    assert!(screen.contains("enter unsubscribe"), "{screen}");
    assert!(screen.contains("esc cancel"), "{screen}");
    // Asking is all that has happened.
    assert!(nothing_left_quill(&harness));

    // A letter is not an answer, and runs nothing behind the question: the
    // message is not deleted, archived or starred, and the list not moved.
    let before = harness.state.list.rows.len();
    harness.keys("d e s j y space");
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    assert_eq!(harness.state.list.rows.len(), before);
    assert_eq!(harness.selected_subject(), SUBJECT);
    // Nor do the keys that move about.
    harness.keys("down pagedown");
    assert_eq!(harness.selected_subject(), SUBJECT);
    assert!(nothing_left_quill(&harness));

    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Reader);
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Unsubscribe {
            url: PAGE.to_owned()
        }]
    );
    // The UI itself opened nothing and wrote nothing.
    assert!(
        harness
            .host_requests
            .iter()
            .all(|request| !matches!(request, HostRequest::OpenUrl(_)))
    );
    assert!(harness.state.compose.is_none());
    assert!(
        status(&harness).contains("Asking lists.harbourbooks.example.com to unsubscribe you…"),
        "{}",
        status(&harness)
    );

    // The service's answer is said, and stays until it is dismissed.
    harness.act(Action::Account(AccountEvent::Unsubscribed {
        host: "lists.harbourbooks.example.com".to_owned(),
        result: Ok(()),
    }));
    let notice = harness.state.notices.back().unwrap();
    assert_eq!(notice.kind, NoticeKind::Info);
    assert!(
        notice
            .text
            .starts_with("Unsubscribed: lists.harbourbooks.example.com took the request."),
        "{}",
        notice.text
    );
    assert!(status(&harness).contains("Unsubscribed"));
}

#[test]
fn a_request_that_fails_says_why() {
    let raw = from_a_list(&format!("List-Unsubscribe: <{PAGE}>\r\n{ONE_CLICK}"));
    let mut harness = reading(&raw, true);
    harness.keys("X enter");
    harness.act(Action::Account(AccountEvent::Unsubscribed {
        host: "lists.harbourbooks.example.com".to_owned(),
        // What a server says is a stranger's words too.
        result: Err(
            "lists.harbourbooks.example.com refused the request (410).\u{1b}[2J".to_owned(),
        ),
    }));
    let notice = harness.state.notices.back().unwrap();
    assert_eq!(notice.kind, NoticeKind::Error);
    assert_eq!(
        notice.text,
        "Not unsubscribed. lists.harbourbooks.example.com refused the request (410)."
    );
}

#[test]
fn esc_asks_nothing_of_anyone() {
    let raw = from_a_list(&format!(
        "List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}"
    ));
    let mut harness = reading(&raw, true);
    harness.keys("X esc");
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(harness.state.unsubscribe.is_none());
    assert!(nothing_left_quill(&harness));
    assert!(harness.state.compose.is_none());
    // And the message is still the one being read.
    assert_eq!(harness.selected_subject(), SUBJECT);
}

#[test]
fn a_mail_address_starts_a_message_and_sends_nothing() {
    let raw = from_a_list(
        "List-Unsubscribe: <mailto:leave@harbourbooks.example.com,other@example.net?subject=unsubscribe%20robin&to=third@example.net&cc=boss@example.com&bcc=spy@example.com&body=Please%20remove%20me.>\r\n",
    );
    let mut harness = reading(&raw, true);
    harness.keys("X");
    let screen = harness.screen();
    assert!(
        screen.contains("A message will be written to leave@harbourbooks.example.com."),
        "{screen}"
    );
    assert!(screen.contains("It is not sent for you."), "{screen}");
    assert!(screen.contains("enter write the message"), "{screen}");
    assert!(nothing_left_quill(&harness));

    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Compose);
    let compose = harness.state.compose.as_ref().unwrap();
    let content = compose.content();
    assert_eq!(content.to, "leave@harbourbooks.example.com");
    assert_eq!(content.subject, "unsubscribe robin");
    assert!(content.body.starts_with("Please remove me."));
    // The request goes to the one address the question named, whoever
    // else the header named.
    assert_eq!((content.cc.as_str(), content.bcc.as_str()), ("", ""));
    // Written from the account the list wrote to.
    let reading = harness.state.reader.row.as_ref().unwrap().account;
    assert_eq!(compose.account, reading);
    // Not sent, and not about to be: sending is the user's key to press.
    assert!(!compose.sending);
    assert!(nothing_left_quill(&harness));
    assert!(
        status(&harness).contains("Nothing has been sent. Send this message to unsubscribe."),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_mail_address_with_no_subject_gets_the_usual_one() {
    let mut harness = reading(
        &from_a_list(&format!("List-Unsubscribe: <{MAIL}>\r\n")),
        true,
    );
    harness.keys("X enter");
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.to.text(), "leave@harbourbooks.example.com");
    assert_eq!(compose.subject.text(), "unsubscribe");
    // Nothing is left to fill in but the user's own second thoughts.
    assert_eq!(compose.field, Field::Body);
}

#[test]
fn a_page_is_opened_in_the_browser_and_never_fetched() {
    // An https address and no word that it takes a POST: Quill posts
    // nothing to it.
    let mut harness = reading(
        &from_a_list(&format!("List-Unsubscribe: <{PAGE}>\r\n")),
        true,
    );
    harness.keys("X");
    let screen = harness.screen();
    assert!(
        screen.contains("Your browser will open a page at lists.harbourbooks.example.com."),
        "{screen}"
    );
    assert!(screen.contains("Quill sends nothing."), "{screen}");
    assert!(screen.contains("enter open the page"), "{screen}");
    assert!(nothing_left_quill(&harness));

    harness.keys("enter");
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::OpenUrl(PAGE.to_owned()))
    );
    assert!(harness.account_requests.is_empty());
    assert!(
        status(&harness).contains("Opened lists.harbourbooks.example.com in your browser."),
        "{}",
        status(&harness)
    );
}

/// The way on offer for a message with these headers.
fn offered(headers: &str, service: bool) -> Option<UnsubscribeHow> {
    let mut harness = reading(&from_a_list(headers), service);
    harness.keys("X");
    harness.state.unsubscribe.map(|prompt| prompt.how)
}

#[test]
fn one_click_is_preferred_then_a_message_then_the_page() {
    let both = format!("List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n");
    assert!(matches!(
        offered(&format!("{both}{ONE_CLICK}"), true),
        Some(UnsubscribeHow::OneClick { url, host })
            if url == PAGE && host == "lists.harbourbooks.example.com"
    ));
    assert!(matches!(
        offered(&both, true),
        Some(UnsubscribeHow::Mail { url, address })
            if url == MAIL && address == "leave@harbourbooks.example.com"
    ));
    assert!(matches!(
        offered(&format!("List-Unsubscribe: <{PAGE}>\r\n"), true),
        Some(UnsubscribeHow::Page { url, .. }) if url == PAGE
    ));
    // The demo mailbox has no service to make the request with: one click
    // is not offered where it could not be done.
    assert!(matches!(
        offered(&format!("{both}{ONE_CLICK}"), false),
        Some(UnsubscribeHow::Mail { .. })
    ));
    assert!(matches!(
        offered(&format!("List-Unsubscribe: <{PAGE}>\r\n{ONE_CLICK}"), false),
        Some(UnsubscribeHow::Page { .. })
    ));
}

#[test]
fn a_header_quill_will_not_use_offers_nothing() {
    for headers in [
        // Plain http, and schemes that are not for this at all.
        "List-Unsubscribe: <http://lists.harbourbooks.example.com/u/r0b1n>\r\n",
        "List-Unsubscribe: <javascript:alert(1)>, <file:///etc/passwd>\r\n",
        // An address dressed up as the list's that leads elsewhere.
        "List-Unsubscribe: <https://lists.harbourbooks.example.com@evil.example.net/u>\r\n",
        // The user's own machine.
        "List-Unsubscribe: <https://127.0.0.1:8443/admin>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        // The POST header alone names nowhere to post to.
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        "List-Unsubscribe: \r\n",
    ] {
        let mut harness = reading(&from_a_list(headers), true);
        assert!(
            !harness
                .state
                .is_enabled(&harness.env, CommandId::MessageUnsubscribe),
            "{headers}"
        );
        assert!(!harness.screen().contains("unsubscribes"), "{headers}");
        harness.keys("X enter");
        assert!(harness.state.unsubscribe.is_none());
        assert!(nothing_left_quill(&harness), "{headers}");
    }
}

#[test]
fn what_the_question_shows_is_made_safe_to_draw() {
    // The sender's name tries to retitle the window and clear the screen.
    let raw = format!(
        "From: \"Harbour\u{1b}]0;owned\u{7} Books\u{1b}[2J\" <news@harbourbooks.example.com>\r\n\
         Subject: {SUBJECT}\r\n\
         List-Unsubscribe: <{PAGE}>\r\n\
         {ONE_CLICK}\
         \r\n\
         Hello.\r\n"
    );
    let mut harness = reading(&raw, true);
    harness.keys("X");
    let prompt = harness.state.unsubscribe.as_ref().unwrap();
    assert!(
        !prompt.sender.contains(|c: char| c.is_control()),
        "{:?}",
        prompt.sender
    );
    let screen = harness.screen();
    assert!(!screen.contains('\u{1b}') && !screen.contains('\u{7}'));
    assert!(screen.contains("news@harbourbooks.example.com"), "{screen}");
}

#[test]
fn from_the_list_it_is_the_message_under_the_cursor_or_nothing() {
    let raw = from_a_list(&format!("List-Unsubscribe: <{PAGE}>\r\n{ONE_CLICK}"));
    let mut harness = reading(&raw, true);
    // Back in the list, on the same message: the key works from there.
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    harness.keys("X");
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    harness.keys("esc");
    // On another message, which is from no list, it does nothing.
    harness.keys("j X");
    assert_eq!(harness.state.context(), Context::List);
    assert!(nothing_left_quill(&harness));
}

#[test]
fn the_mutt_keys_have_it_on_the_same_key() {
    use quill_core::config::KeymapPreset;
    use quill_tui::keymap::Keymap;
    let raw = from_a_list(&format!("List-Unsubscribe: <{PAGE}>\r\n{ONE_CLICK}"));
    let mut harness = reading(&raw, true);
    harness.env.keymap = Keymap::preset(KeymapPreset::Mutt);
    harness.keys("X");
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    // Mutt's `i` leaves a message. Here it is a letter, and not an answer.
    harness.keys("i");
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::Reader);
}

#[test]
fn snapshot_of_the_unsubscribe_question() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let raw = from_a_list(&format!(
            "List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}"
        ));
        let mut harness = reading(&raw, true);
        harness.resize(width, height);
        harness.keys("X");
        insta::assert_snapshot!(
            format!("unsubscribe_one_click_{width}x{height}"),
            harness.screen()
        );
    }
}
