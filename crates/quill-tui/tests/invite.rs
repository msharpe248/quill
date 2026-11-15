//! Calendar invitations: the card the reader shows, and answering one.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Address, Flags, FolderRole, Headers, LocalMessageId, RemoteId, RemoteMessage, SyncCursor,
    SyncDelta,
};
use quill_mime::invite::{Answer, Method};
use quill_store::{DraftState, Mailbox};
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataEvent;
use quill_tui::outgoing::{INVITE_REPLY_NAME, INVITE_REPLY_TYPE};
use quill_tui::state::{Action, Body, HostRequest, Identity, Mouse, MouseKind};

const ANSWERS: [CommandId; 3] = [
    CommandId::InviteAccept,
    CommandId::InviteDecline,
    CommandId::InviteTentative,
];

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// A message that carries `calendar` beside a line of plain text.
fn message(subject: &str, method: &str, calendar: &str) -> String {
    format!(
        "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
         To: Robin Vale <robin@example.com>\r\n\
         Subject: {subject}\r\n\
         Message-ID: <invite-1@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/alternative; boundary=\"b\"\r\n\
         \r\n\
         --b\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         You are invited to the offsite.\r\n\
         --b\r\n\
         Content-Type: text/calendar; method={method}; charset=utf-8\r\n\
         \r\n\
         {calendar}\r\n\
         --b--\r\n"
    )
}

/// A calendar with one event: `method`, then the event's own lines.
fn calendar(method: &str, event: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Cal//EN\r\nMETHOD:{method}\r\n\
         BEGIN:VEVENT\r\n{event}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

const OFFSITE: &str = "UID:offsite-2026@example.com\r\n\
    SEQUENCE:2\r\n\
    DTSTAMP:20261114T093000Z\r\n\
    DTSTART;TZID=Europe/London:20261120T090000\r\n\
    DTEND;TZID=Europe/London:20261120T160000\r\n\
    SUMMARY:Offsite\\, day one\r\n\
    LOCATION:Room 4\\; second floor\r\n\
    ORGANIZER;CN=Ana Ruiz:mailto:ana.ruiz@example.com\r\n\
    ATTENDEE;CN=Robin Vale;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:robin@example.com\r\n\
    ATTENDEE;CN=Kit Moss;PARTSTAT=ACCEPTED:mailto:kit@example.com\r\n\
    ATTENDEE;CN=Sam Lee;PARTSTAT=DECLINED:mailto:sam.lee@example.com\r\n";

/// Puts a message of our own at the top of the Work inbox, as downloaded
/// mail is kept, and opens it at `size`.
fn reading_at(raw: &str, subject: &str, size: (u16, u16)) -> Harness {
    let mut harness = Harness::demo(size.0, size.1);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("invite-1".to_owned());
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![RemoteMessage {
                    id: remote.clone(),
                    headers: Headers {
                        message_id: None,
                        subject: subject.to_owned(),
                        from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                        to: vec![Address::new("Robin Vale", "robin@example.com")],
                        cc: Vec::new(),
                        date: NOW + 60,
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
    let text = quill_mime::parse(raw.as_bytes(), NOW).unwrap().body;
    harness
        .store
        .put_message(id, &text, raw.as_bytes())
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.keys("2 g g enter");
    assert_eq!(harness.selected_subject(), subject);
    harness
}

fn reading(raw: &str, subject: &str) -> Harness {
    reading_at(raw, subject, (200, 60))
}

fn request() -> Harness {
    reading(
        &message("Offsite", "REQUEST", &calendar("REQUEST", OFFSITE)),
        "Offsite",
    )
}

fn open_message(harness: &Harness) -> LocalMessageId {
    harness.state.reader.row.as_ref().unwrap().id
}

fn enabled(harness: &Harness) -> Vec<bool> {
    ANSWERS
        .iter()
        .map(|id| harness.state.is_enabled(&harness.env, *id))
        .collect()
}

/// The one message waiting in the outbox: its row, who it goes to on the
/// wire, and its finished text.
fn outgoing(harness: &mut Harness) -> (quill_store::DraftRow, Vec<String>, String) {
    let outbox = harness.store.outbox().unwrap();
    assert_eq!(outbox.len(), 1, "{outbox:?}");
    assert_eq!(outbox[0].state, DraftState::Outbox);
    let row = outbox[0].clone();
    // Long after any undo-send delay.
    let claimed = harness
        .store
        .claim_send(row.account, i64::MAX / 2)
        .unwrap()
        .unwrap();
    let raw = harness.store.blob(&claimed.raw_hash).unwrap().unwrap();
    (row, claimed.to, String::from_utf8(raw).unwrap())
}

#[test]
fn an_invitation_is_a_card_between_the_headers_and_the_text() {
    let harness = request();
    let screen = harness.screen();
    let row = |needle: &str| harness.row_with(needle);
    assert!(row("── Invitation ─").contains("─────"), "{screen}");
    assert!(row("What:").contains("Offsite, day one"), "{screen}");
    // The harness reads in UTC: nine in London in November is nine.
    assert!(
        row("When:").contains("Fri 20 Nov 2026 09:00 to 16:00"),
        "{screen}"
    );
    assert!(row("Where:").contains("Room 4; second floor"), "{screen}");
    assert!(
        row("Host:").contains("Ana Ruiz <ana.ruiz@example.com>"),
        "{screen}"
    );
    assert!(
        row("Invited:").contains("3 people: 1 accepted, 1 declined, 1 not answered"),
        "{screen}"
    );
    assert!(
        screen.contains("Asks for an answer: y accepts, n declines, M maybe."),
        "{screen}"
    );
    // The card is above the message's own words, under its headers.
    let line_of = |needle: &str| {
        screen
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} is not on screen:\n{screen}"))
    };
    assert!(line_of("Subject:") < line_of("── Invitation"));
    assert!(line_of("Asks for an answer") < line_of("You are invited to the offsite."));
    // And the keys that answer are offered where the keys are.
    assert!(
        status(&harness).contains("y accept") && status(&harness).contains("n decline"),
        "{}",
        status(&harness)
    );
    assert_eq!(enabled(&harness), [true, true, true]);
}

#[test]
fn times_are_shown_in_the_users_own_zone() {
    let mut harness = request();
    harness.env.tz = jiff::tz::TimeZone::get("America/New_York").unwrap();
    // Read again, as it is when the message is opened.
    harness.keys("esc j k enter");
    assert!(
        harness
            .row_with("When:")
            .contains("Fri 20 Nov 2026 04:00 to 11:00"),
        "{}",
        harness.screen()
    );
}

#[test]
fn the_card_takes_its_rows_from_the_text_and_the_text_still_scrolls() {
    let long = format!(
        "See https://example.com/agenda first.\r\n{}",
        "A line of the agenda.\r\n".repeat(59)
    );
    let raw = message("Offsite", "REQUEST", &calendar("REQUEST", OFFSITE))
        .replace("You are invited to the offsite.\r\n", &long);
    let mut harness = reading_at(&raw, "Offsite", (80, 24));
    let screen = harness.screen();
    // Everything of the card is on screen, whole, and text under it.
    assert!(
        screen.contains("Asks for an answer: y accepts, n declines, M maybe."),
        "{screen}"
    );
    assert!(screen.contains("A line of the agenda."), "{screen}");
    // 21 rows inside the pane: five of headers (the calendar is a file
    // too), a blank, seven of card, a blank, and seven left for the text.
    assert!(screen.contains("1-7 of 60"), "{screen}");

    // A click is counted from where the text is drawn, under the card:
    // the address in its first line opens.
    let (row, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("https://example.com/agenda"))
        .unwrap();
    let column = line.chars().take_while(|c| *c != 'h').count() + 4;
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::Press,
        column: column as u16,
        row: row as u16,
    }));
    assert_eq!(
        harness.host_requests,
        [HostRequest::OpenUrl(
            "https://example.com/agenda".to_owned()
        )]
    );
    // And the line naming the files is still the one that lists them.
    let files = screen
        .lines()
        .position(|line| line.contains("Attach:"))
        .unwrap();
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::Press,
        column: column as u16,
        row: files as u16,
    }));
    assert_eq!(harness.state.context(), Context::Attachments);
    harness.keys("esc");

    harness.keys("G");
    assert!(harness.screen().contains("54-60 of 60"));
    assert!(harness.screen().contains("Asks for an answer"));
    harness.keys("g g space");
    assert!(
        harness.screen().contains("7-13 of 60"),
        "{}",
        harness.screen()
    );
    // No pane is too small to draw it in.
    for (width, height) in [(80, 8), (60, 12), (30, 10), (24, 6), (200, 9)] {
        harness.resize(width, height);
        harness.screen();
    }
}

#[test]
fn accepting_puts_an_answer_to_the_organizer_in_the_outbox() {
    let mut harness = request();
    harness.env.identities.insert(
        "Work".to_owned(),
        Identity {
            full_name: Some("Robin Vale".to_owned()),
            signature: Some("Robin".to_owned()),
        },
    );
    let invitation = open_message(&harness);
    harness.keys("y");
    assert!(
        status(&harness).contains("Accepted. The answer goes in 10 seconds. U undoes it."),
        "{}",
        status(&harness)
    );
    // Nothing was opened to be written, and the invitation is still read.
    assert_eq!(harness.state.context(), Context::Reader);
    assert_eq!(open_message(&harness), invitation);
    assert!(harness.row_with("Outbox").contains('1'));

    let (row, to, text) = outgoing(&mut harness);
    assert_eq!(to, ["ana.ruiz@example.com"]);
    assert_eq!(row.content.to, "Ana Ruiz <ana.ruiz@example.com>");
    assert_eq!(row.content.subject, "Accepted: Offsite, day one");
    assert_eq!(row.content.body, "Robin Vale has accepted.\n");
    assert_eq!(
        row.content.in_reply_to.as_deref(),
        Some("<invite-1@example.com>")
    );
    assert_eq!(row.content.replied_to, Some(invitation.0));
    // The calendar's own form of the answer is attached, and says what it
    // is.
    assert_eq!(row.content.attachments.len(), 1);
    let attached = &row.content.attachments[0];
    assert_eq!(attached.name, INVITE_REPLY_NAME);
    assert_eq!(attached.name, "invite.ics");
    assert_eq!(attached.content_type, INVITE_REPLY_TYPE);
    assert_eq!(
        attached.content_type,
        "text/calendar; method=REPLY; charset=utf-8"
    );
    let ics = String::from_utf8(harness.store.blob(&attached.blob_hash).unwrap().unwrap()).unwrap();
    assert!(ics.contains("\r\nMETHOD:REPLY\r\n"), "{ics}");
    assert!(
        ics.contains("\r\nUID:offsite-2026@example.com\r\n"),
        "{ics}"
    );
    assert!(ics.contains("\r\nSEQUENCE:2\r\n"), "{ics}");
    assert!(
        ics.contains(
            "\r\nATTENDEE;PARTSTAT=ACCEPTED;CN=\"Robin Vale\":mailto:robin@example.com\r\n"
        ),
        "{ics}"
    );
    // Stamped now, in UTC.
    let stamp = ics
        .lines()
        .find_map(|line| line.strip_prefix("DTSTAMP:"))
        .unwrap();
    assert_eq!(stamp.len(), 16, "{stamp}");
    assert!(
        stamp.ends_with('Z')
            && stamp.as_bytes()[8] == b'T'
            && stamp
                .bytes()
                .enumerate()
                .all(|(at, byte)| at == 8 || at == 15 || byte.is_ascii_digit()),
        "{stamp}"
    );

    // The message as it will go: to the organizer, from the account, with
    // the calendar a part of it that reads back as the same answer.
    assert!(
        text.contains("From: \"Robin Vale\" <robin@example.com>\r\n")
            || text.contains("From: Robin Vale <robin@example.com>\r\n"),
        "{text}"
    );
    assert!(
        text.contains("Subject: Accepted: Offsite, day one\r\n"),
        "{text}"
    );
    assert!(
        text.contains("In-Reply-To: <invite-1@example.com>\r\n"),
        "{text}"
    );
    assert!(text.contains("Robin Vale has accepted."), "{text}");
    // No signature, no quoted invitation: one line and the answer.
    assert!(!text.contains("You are invited"), "{text}");
    let sent = quill_mime::invite::find(text.as_bytes()).expect("a calendar part");
    assert_eq!(sent.method, Method::Reply);
    assert_eq!(sent.uid, "offsite-2026@example.com");
    assert_eq!(sent.sequence, 2);
    assert_eq!(sent.attendees.len(), 1);
    assert_eq!(sent.attendees[0].address, "robin@example.com");
    assert_eq!(sent.attendees[0].status, Some(Answer::Accepted));
    let parsed = quill_mime::parse(text.as_bytes(), NOW).unwrap();
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].content_type, "text/calendar");
    assert_eq!(parsed.attachments[0].name.as_deref(), Some("invite.ics"));
}

#[test]
fn declining_and_maybe_say_so_and_an_account_without_a_name_signs_with_its_address() {
    for (key, word, said, partstat, answer) in [
        (
            "n",
            "Declined",
            "robin@example.com has declined.",
            "DECLINED",
            Answer::Declined,
        ),
        (
            "M",
            "Tentative",
            "robin@example.com has tentatively accepted.",
            "TENTATIVE",
            Answer::Tentative,
        ),
    ] {
        let mut harness = request();
        harness.keys(key);
        assert!(
            status(&harness).contains(&format!("{word}. The answer goes in 10 seconds.")),
            "{}",
            status(&harness)
        );
        let (row, to, text) = outgoing(&mut harness);
        assert_eq!(to, ["ana.ruiz@example.com"]);
        assert_eq!(row.content.subject, format!("{word}: Offsite, day one"));
        assert_eq!(row.content.body, format!("{said}\n"));
        let sent = quill_mime::invite::find(text.as_bytes()).unwrap();
        assert_eq!(sent.attendees[0].status, Some(answer));
        let ics = String::from_utf8(
            harness
                .store
                .blob(&row.content.attachments[0].blob_hash)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            ics.contains(&format!(
                "\r\nATTENDEE;PARTSTAT={partstat}:mailto:robin@example.com\r\n"
            )),
            "{ics}"
        );
    }
}

#[test]
fn an_answer_can_be_given_from_the_list_and_taken_back() {
    let mut harness = request();
    // Back in the list, on the invitation: its keys still answer it.
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(enabled(&harness), [true, true, true]);
    harness.keys("y");
    assert_eq!(harness.store.outbox().unwrap().len(), 1);
    // Undo, within the delay, takes it back: nothing goes.
    harness.keys("U");
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(
        status(&harness).contains("Not sent."),
        "{}",
        status(&harness)
    );

    // On another message the keys are nobody's.
    harness.keys("j");
    assert_eq!(enabled(&harness), [false, false, false]);
    harness.keys("y n M");
    assert!(harness.store.outbox().unwrap().is_empty());
    // With no delay, it goes at once and says so.
    harness.keys("k");
    harness.env.compose.undo_send_seconds = 0;
    harness.keys("n");
    assert!(
        status(&harness).contains("Declined. The answer is on its way."),
        "{}",
        status(&harness)
    );
    assert!(!status(&harness).contains("undoes it"));
}

#[test]
fn a_cancellation_says_so_and_asks_nothing() {
    let event = OFFSITE.replace("SEQUENCE:2", "SEQUENCE:3\r\nSTATUS:CANCELLED");
    let mut harness = reading(
        &message("Cancelled: Offsite", "CANCEL", &calendar("CANCEL", &event)),
        "Cancelled: Offsite",
    );
    let screen = harness.screen();
    assert!(screen.contains("── Cancelled event ─"), "{screen}");
    assert!(screen.contains("This event was cancelled."), "{screen}");
    assert!(harness.row_with("What:").contains("Offsite, day one"));
    assert!(!screen.contains("Asks for an answer"), "{screen}");
    assert_eq!(enabled(&harness), [false, false, false]);
    harness.keys("y n M");
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(!status(&harness).contains("accept"), "{}", status(&harness));
}

#[test]
fn someones_answer_to_an_invitation_says_who_and_what() {
    let event = "UID:planning@example.com\r\n\
        DTSTART:20261120T140000Z\r\n\
        SUMMARY:Planning\r\n\
        ORGANIZER:mailto:robin@example.com\r\n\
        ATTENDEE;CN=Kit Moss;PARTSTAT=ACCEPTED:mailto:kit@example.com\r\n";
    let harness = reading(
        &message("Accepted: Planning", "REPLY", &calendar("REPLY", event)),
        "Accepted: Planning",
    );
    let screen = harness.screen();
    assert!(screen.contains("── Answer to an invitation ─"), "{screen}");
    assert!(screen.contains("Kit Moss accepted."), "{screen}");
    assert!(
        harness.row_with("When:").contains("Fri 20 Nov 2026 14:00"),
        "{screen}"
    );
    assert_eq!(enabled(&harness), [false, false, false]);

    for (partstat, said) in [
        ("DECLINED", "kit@example.com declined."),
        ("TENTATIVE", "kit@example.com answered maybe."),
    ] {
        let event = format!(
            "UID:planning@example.com\r\nSUMMARY:Planning\r\n\
             ATTENDEE;PARTSTAT={partstat}:mailto:kit@example.com\r\n"
        );
        let harness = reading(
            &message("Planning", "REPLY", &calendar("REPLY", &event)),
            "Planning",
        );
        assert!(harness.screen().contains(said), "{}", harness.screen());
    }
}

#[test]
fn an_event_sent_for_information_asks_nothing() {
    let event = "UID:holiday@example.com\r\n\
        DTSTART;VALUE=DATE:20261225\r\n\
        DTEND;VALUE=DATE:20261226\r\n\
        RRULE:FREQ=YEARLY\r\n\
        SUMMARY:Office closed\r\n";
    let harness = reading(
        &message("Office closed", "PUBLISH", &calendar("PUBLISH", event)),
        "Office closed",
    );
    let screen = harness.screen();
    assert!(screen.contains("── Event ─"), "{screen}");
    // A whole day is a date, and one that comes round again says so.
    assert!(
        harness
            .row_with("When:")
            .contains("Fri 25 Dec 2026, all day, repeats"),
        "{screen}"
    );
    assert!(
        screen.contains("For your information: no answer is asked."),
        "{screen}"
    );
    // Nobody is named, so no rows are spent on them.
    assert!(!screen.contains("Host:") && !screen.contains("Invited:"));
    assert!(!screen.contains("Where:"));
    assert_eq!(enabled(&harness), [false, false, false]);
}

#[test]
fn an_all_day_event_over_several_days_names_the_first_and_the_last() {
    let event = "UID:conference@example.com\r\n\
        DTSTART;VALUE=DATE:20261120\r\n\
        DTEND;VALUE=DATE:20261123\r\n\
        SUMMARY:Conference\r\n\
        ORGANIZER:mailto:ana.ruiz@example.com\r\n\
        ATTENDEE:mailto:robin@example.com\r\n";
    let harness = reading(
        &message("Conference", "REQUEST", &calendar("REQUEST", event)),
        "Conference",
    );
    assert!(
        harness
            .row_with("When:")
            .contains("Fri 20 Nov 2026 to Sun 22 Nov 2026, all day"),
        "{}",
        harness.screen()
    );
    assert!(
        harness
            .row_with("Invited:")
            .contains("1 person: 1 not answered")
    );
    assert_eq!(enabled(&harness), [true, true, true]);
}

#[test]
fn a_zone_quill_does_not_know_is_shown_as_written_beside_its_own_clock() {
    let event = "UID:sync@example.com\r\n\
        DTSTART;TZID=\"W. Europe Standard Time\":20261120T093000\r\n\
        DTEND;TZID=\"W. Europe Standard Time\":20261120T100000\r\n\
        SUMMARY:Sync\r\n\
        ORGANIZER:mailto:ana.ruiz@example.com\r\n\
        ATTENDEE:mailto:robin@example.com\r\n";
    let mut harness = reading(
        &message("Sync", "REQUEST", &calendar("REQUEST", event)),
        "Sync",
    );
    assert!(
        harness
            .row_with("When:")
            .contains("Fri 20 Nov 2026 09:30 to 10:00 (W. Europe Standard Time)"),
        "{}",
        harness.screen()
    );
    // The answer repeats the start exactly as it came, zone and all, so
    // the organizer's calendar knows the event.
    harness.keys("y");
    let (row, _, _) = outgoing(&mut harness);
    let ics = String::from_utf8(
        harness
            .store
            .blob(&row.content.attachments[0].blob_hash)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(
        ics.contains("\r\nDTSTART;TZID=\"W. Europe Standard Time\":20261120T093000\r\n"),
        "{ics}"
    );
}

#[test]
fn a_hostile_invitation_draws_nothing_but_its_words() {
    let esc = "\u{1b}";
    let bel = "\u{7}";
    let event = format!(
        "UID:party@example.com\r\n\
         DTSTART;TZID={esc}[2JEurope/Nowhere{esc}]0;owned{bel}:20261120T090000\r\n\
         SUMMARY:Party{esc}]0;owned{bel}{esc}[2J\u{202e}now\r\n\
         LOCATION:The{esc}[31m roof\u{9b}5m\\nsecond line\r\n\
         DESCRIPTION:{esc}[2JBring{esc}c cake\r\n\
         ORGANIZER;CN=\"Boss{esc}[5m\u{202e}\":mailto:boss{esc}[1m@example.com\r\n\
         ATTENDEE;CN=Robin{esc}[7m;PARTSTAT=ACCEPTED{esc}[2J:mailto:robin@example.com\r\n\
         ATTENDEE;CN=\"A{esc}]8;;https://example.net{bel}\":mailto:kit@example.com\r\n"
    );
    let mut harness = reading(
        &message("Party", "REQUEST", &calendar("REQUEST", &event)),
        "Party",
    );
    let buffer = harness.buffer();
    for cell in buffer.content.iter() {
        assert!(
            !cell
                .symbol()
                .chars()
                .any(|c| c.is_control() || c == '\u{202e}'),
            "{:?} reached the screen",
            cell.symbol()
        );
    }
    let screen = harness.screen();
    assert!(!screen.contains("owned"), "{screen}");
    assert!(harness.row_with("What:").contains("Party"), "{screen}");
    assert!(harness.row_with("Where:").contains("The roof"), "{screen}");
    // The card is the size it would be for any invitation: a line break
    // in a place does not make a row.
    assert_eq!(harness.state.reader.card.len(), 7, "{screen}");
    // What the card holds is as clean as what is drawn.
    let held = format!("{:?}", harness.state.reader.card);
    assert!(
        !held.contains("\\u{1b}") && !held.contains("\\u{202e}"),
        "{held}"
    );

    // Answering it puts nothing of its dressing into the answer either.
    harness.keys("y");
    let (row, to, text) = outgoing(&mut harness);
    assert_eq!(to.len(), 1);
    assert!(!to[0].contains(['\u{1b}', '\r', '\n', ' ']), "{to:?}");
    for written in [&row.content.to, &row.content.subject, &text] {
        assert!(!written.contains('\u{1b}'), "{written:?}");
    }
    assert_eq!(text.matches("\r\nSubject:").count(), 1, "{text}");
}

#[test]
fn a_message_with_no_invitation_has_no_card_and_nothing_to_answer() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert!(harness.state.reader.card.is_empty());
    assert!(!harness.screen().contains("What:"));
    assert_eq!(enabled(&harness), [false, false, false]);
    harness.keys("y n M");
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(matches!(&harness.state.reader.body, Body::Loaded(body) if body.invite.is_none()));

    // A calendar with no event in it is no invitation.
    let empty = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n";
    let harness = reading(&message("Nothing", "REQUEST", empty), "Nothing");
    assert!(harness.state.reader.card.is_empty());
    assert_eq!(enabled(&harness), [false, false, false]);
}

#[test]
fn the_users_own_invitation_is_not_theirs_to_answer() {
    let event = OFFSITE.replace(
        "ORGANIZER;CN=Ana Ruiz:mailto:ana.ruiz@example.com",
        "ORGANIZER;CN=Robin Vale:mailto:Robin@Example.com",
    );
    let mut harness = reading(
        &message("Offsite", "REQUEST", &calendar("REQUEST", &event)),
        "Offsite",
    );
    let screen = harness.screen();
    assert!(screen.contains("── Invitation ─"), "{screen}");
    assert!(screen.contains("This is your own invitation."), "{screen}");
    assert!(!screen.contains("Asks for an answer"), "{screen}");
    assert_eq!(enabled(&harness), [false, false, false]);
    harness.keys("y n M");
    assert!(harness.store.outbox().unwrap().is_empty());

    // One that names nobody to answer has nobody to send an answer to.
    let event = "UID:1@example.com\r\nSUMMARY:Drinks\r\n";
    let harness = reading(
        &message("Drinks", "REQUEST", &calendar("REQUEST", event)),
        "Drinks",
    );
    assert!(harness.screen().contains("It names nobody to answer."));
    assert_eq!(enabled(&harness), [false, false, false]);
}

#[test]
fn an_answer_is_built_from_the_message_and_not_from_what_was_on_screen() {
    use quill_tui::data::{self, DataRequest};
    use quill_tui::outgoing::InviteAnswer;
    let mut harness = request();
    let ask = |message| {
        DataRequest::AnswerInvite(Box::new(InviteAnswer {
            message,
            answer: Answer::Accepted,
            from_name: None,
            delay_seconds: 10,
            utc_offset: 0,
        }))
    };
    let flashed = |events: Vec<DataEvent>| match events.as_slice() {
        [DataEvent::Flash { text, undoable }] => (text.clone(), *undoable),
        other => panic!("{other:?}"),
    };
    // A message that carries none, one that is gone, and a draft.
    let plain = harness
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| row.headers.subject == "Q4 roadmap")
        .unwrap()
        .id;
    assert_eq!(
        flashed(data::handle(&mut harness.store, ask(plain))),
        ("This message carries no invitation.".to_owned(), false)
    );
    assert_eq!(
        flashed(data::handle(
            &mut harness.store,
            ask(LocalMessageId(99_999))
        ))
        .0,
        "That message is gone."
    );
    assert_eq!(
        flashed(data::handle(&mut harness.store, ask(LocalMessageId(-1)))).0,
        "That message is gone."
    );
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(harness.store.drafts().unwrap().is_empty());
}

#[test]
fn snapshot_of_an_invitation() {
    for size in [(80, 24), (200, 60)] {
        let harness = reading_at(
            &message("Offsite", "REQUEST", &calendar("REQUEST", OFFSITE)),
            "Offsite",
            size,
        );
        insta::assert_snapshot!(
            format!("invitation_{}x{}", size.0, size.1),
            harness.screen()
        );
    }
}
