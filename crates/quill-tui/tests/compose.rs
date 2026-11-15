//! Writing, replying, forwarding, drafts and sending, from the keys to the
//! outbox.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_store::{DraftState, Outgoing};
use quill_sync::{SyncEvent, SyncRequest};
use quill_tui::command::Context;
use quill_tui::compose::Field;
use quill_tui::data::{DataEvent, DataRequest};
use quill_tui::outgoing::Response;
use quill_tui::state::{Action, HostRequest, Identity};

/// Wide enough that nothing in the compose pane is cut short.
fn wide() -> Harness {
    Harness::demo(160, 30)
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// Takes the one message waiting in the outbox as the engine would, and
/// returns it with its finished text.
fn outgoing(harness: &mut Harness) -> (Outgoing, String) {
    let outbox = harness.store.outbox().unwrap();
    assert_eq!(outbox.len(), 1, "{outbox:?}");
    assert_eq!(outbox[0].state, DraftState::Outbox);
    // Long after any undo-send delay.
    let claimed = harness
        .store
        .claim_send(outbox[0].account, i64::MAX / 2)
        .unwrap()
        .unwrap();
    let raw = harness.store.blob(&claimed.raw_hash).unwrap().unwrap();
    (claimed, String::from_utf8(raw).unwrap())
}

/// Writes a short message to Priya and leaves the cursor in the body.
fn write_to_priya(harness: &mut Harness) {
    harness.keys("c");
    harness.type_text("pri");
    harness.keys("enter tab tab");
    harness.type_text("Lunch");
    harness.keys("tab");
    harness.type_text("Are you free on *Thursday*?");
}

#[test]
fn c_opens_an_empty_message_from_the_first_account() {
    let mut harness = wide();
    harness.keys("c");
    assert_eq!(harness.state.context(), Context::Compose);
    let screen = harness.screen();
    assert!(screen.contains("[3] New message"), "{screen}");
    assert!(
        harness
            .row_with("From:")
            .contains("robin@example.com  (Work)")
    );
    // The list stays in view beside it, and the cursor waits in To.
    assert!(screen.contains("[2] Inbox"), "{screen}");
    let (_, row) = harness.cursor().unwrap();
    let to_row = screen
        .lines()
        .position(|line| line.contains("To:"))
        .unwrap();
    assert_eq!(usize::from(row), to_row);
    assert!(status(&harness).contains("send"), "{}", status(&harness));
}

#[test]
fn a_message_from_a_folder_goes_from_that_folders_account() {
    let mut harness = wide();
    // The Personal account's inbox.
    harness.keys("1 G k k k k enter c");
    assert!(
        harness.row_with("From:").contains("(Personal)"),
        "{}",
        harness.screen()
    );
}

#[test]
fn letters_are_typed_not_run_as_commands() {
    let mut harness = wide();
    harness.keys("c");
    // Every one of these is a command in the list.
    harness.type_text("jkdeq?");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("To:").contains("jkdeq?"));
    assert_eq!(harness.store.drafts().unwrap().len(), 0);
}

#[test]
fn someone_written_to_before_is_offered_and_taken() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("pri");
    assert!(
        harness
            .screen()
            .contains("> Priya Nair <priya.nair@example.com>"),
        "{}",
        harness.screen()
    );
    harness.keys("enter");
    assert!(
        harness
            .row_with("To:")
            .contains("Priya Nair <priya.nair@example.com>, ")
    );
    // Taking a person keeps the cursor in the field for the next one.
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::To);
    harness.type_text("ana");
    harness.keys("tab");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().content().to,
        "Priya Nair <priya.nair@example.com>, Ana Ruiz <ana.ruiz@example.com>, "
    );
}

#[test]
fn escape_declines_the_people_on_offer_before_it_closes_anything() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("pri");
    harness.keys("escape");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        !harness.screen().contains("> Priya"),
        "{}",
        harness.screen()
    );
    assert!(harness.row_with("To:").contains("pri"));
}

#[test]
fn a_written_message_waits_in_the_outbox() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-enter");

    assert_eq!(harness.state.context(), Context::List);
    assert!(
        status(&harness).contains("Sending in 10 seconds. U undoes it."),
        "{}",
        status(&harness)
    );
    assert!(harness.row_with("Outbox").contains('1'));
    assert!(harness.sync_requests.contains(&SyncRequest::Queue));

    let (claimed, raw) = outgoing(&mut harness);
    assert_eq!(claimed.from, "robin@example.com");
    assert_eq!(claimed.to, ["priya.nair@example.com"]);
    assert!(raw.contains("From: <robin@example.com>\r\n"), "{raw}");
    assert!(
        raw.contains("To: \"Priya Nair\" <priya.nair@example.com>\r\n"),
        "{raw}"
    );
    assert!(raw.contains("Subject: Lunch\r\n"), "{raw}");
    assert!(raw.contains("Message-ID: <"), "{raw}");
    // Markdown goes as text with HTML beside it.
    assert!(raw.contains("multipart/alternative"), "{raw}");
    assert!(raw.contains("Are you free on *Thursday*?"), "{raw}");
    assert!(raw.contains("<em>Thursday</em>"), "{raw}");
}

#[test]
fn plain_text_only_sends_one_part() {
    let mut harness = wide();
    harness.env.compose.markdown = false;
    write_to_priya(&mut harness);
    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    assert!(!raw.contains("multipart/"), "{raw}");
    assert!(!raw.contains("<em>"), "{raw}");
    assert!(raw.contains("Are you free on *Thursday*?"), "{raw}");
}

#[test]
fn the_name_and_signature_come_from_the_account() {
    let mut harness = wide();
    harness.env.identities.insert(
        "Work".to_owned(),
        Identity {
            full_name: Some("Robin Vale".to_owned()),
            signature: Some("Robin Vale\nExample Ltd".to_owned()),
        },
    );
    harness.keys("c");
    assert!(
        harness
            .row_with("From:")
            .contains("Robin Vale <robin@example.com>  (Work)")
    );
    let screen = harness.screen();
    assert!(screen.contains("-- "), "{screen}");
    assert!(screen.contains("Example Ltd"), "{screen}");

    // A signature alone is not a message worth keeping.
    harness.keys("escape");
    assert_eq!(harness.store.drafts().unwrap().len(), 0);

    write_to_priya(&mut harness);
    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    assert!(
        raw.contains("From: \"Robin Vale\" <robin@example.com>\r\n"),
        "{raw}"
    );
    // The signature's marker ends in a space, which is sent so that it
    // survives: `-- ` as quoted-printable.
    assert!(
        raw.contains("Are you free on *Thursday*?\r\n\r\n--=20\r\nRobin Vale\r\nExample Ltd"),
        "{raw}"
    );
}

#[test]
fn a_missing_subject_is_questioned_once() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("priya.nair@example.com");
    // The first tab takes the person on offer; the rest pass Cc and Subject.
    harness.keys("tab tab tab tab");
    harness.type_text("Hello");
    harness.keys("cmd-enter");

    assert_eq!(harness.state.context(), Context::Compose);
    let screen = harness.screen();
    assert!(screen.contains("There is no subject."), "{screen}");
    assert!(
        screen.contains("cmd-enter again sends it as it is."),
        "{screen}"
    );
    assert!(harness.store.outbox().unwrap().is_empty());

    harness.keys("cmd-enter");
    assert_eq!(harness.state.context(), Context::List);
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Hello"), "{raw}");
}

#[test]
fn a_doubt_goes_away_when_the_message_changes() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("priya.nair@example.com");
    // The first tab takes the person on offer; the rest pass Cc and Subject.
    harness.keys("tab tab tab tab");
    harness.type_text("Hello");
    harness.keys("cmd-enter");
    assert!(harness.screen().contains("There is no subject."));
    // Having changed it, the next send is checked afresh.
    harness.type_text(" there");
    assert!(!harness.screen().contains("There is no subject."));
    harness.keys("cmd-enter");
    assert!(harness.screen().contains("There is no subject."));
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn a_promised_attachment_that_is_not_there_is_questioned() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.type_text(" The agenda is attached.");
    harness.keys("cmd-enter");
    let screen = harness.screen();
    assert!(screen.contains("attach"), "{screen}");
    assert!(screen.contains("again sends it as it is."), "{screen}");
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn what_is_not_an_address_keeps_the_message_open() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("priya");
    harness.keys("escape tab tab");
    harness.type_text("Lunch");
    harness.keys("cmd-enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        harness
            .screen()
            .contains("To: \"priya\" is not an email address."),
        "{}",
        harness.screen()
    );
    assert!(harness.store.outbox().unwrap().is_empty());
    // The complaint goes with the next key, and the message can be put
    // right and sent.
    harness.keys("shift-tab shift-tab");
    harness.type_text(".nair@example.com");
    assert!(!harness.screen().contains("is not an email address"));
    harness.keys("escape cmd-enter");
    let (claimed, _) = outgoing(&mut harness);
    assert_eq!(claimed.to, ["priya.nair@example.com"]);
}

#[test]
fn a_message_to_nobody_is_not_sent() {
    let mut harness = wide();
    harness.keys("c tab tab");
    harness.type_text("Lunch");
    harness.keys("cmd-enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        harness.screen().contains("nobody to send"),
        "{}",
        harness.screen()
    );
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn undo_takes_a_message_back_out_of_the_outbox() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-enter U");
    assert!(
        status(&harness).contains("Not sent. The message is a draft again."),
        "{}",
        status(&harness)
    );
    assert!(harness.store.outbox().unwrap().is_empty());
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.subject, "Lunch");
    assert!(harness.row_with("Drafts").contains('1'));
}

#[test]
fn closing_keeps_a_draft_that_enter_takes_up_again() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("escape");
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        status(&harness).contains("Saved to Drafts."),
        "{}",
        status(&harness)
    );
    assert_eq!(harness.store.drafts().unwrap().len(), 1);

    // The drafts of every account, under the unified inbox.
    harness.keys("1 g g j enter");
    assert!(
        harness.screen().contains("[2] Drafts"),
        "{}",
        harness.screen()
    );
    assert!(harness.row_with("Lunch").contains("Priya Nair"));
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("Subject:").contains("Lunch"));
    assert!(harness.screen().contains("Are you free on *Thursday*?"));

    // Closing it unchanged saves nothing new; changing it saves over it.
    harness.keys("escape");
    assert!(!status(&harness).contains("Saved"), "{}", status(&harness));
    harness.keys("enter");
    harness.type_text(" Or Friday?");
    harness.keys("escape");
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].content.body.contains("Or Friday?"), "{drafts:?}");
}

#[test]
fn saving_keeps_the_message_open() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-s");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        status(&harness).contains("Saved to Drafts."),
        "{}",
        status(&harness)
    );
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
    // Sending it afterwards leaves no draft behind.
    harness.keys("cmd-enter");
    assert!(harness.store.drafts().unwrap().is_empty());
    assert_eq!(harness.store.outbox().unwrap().len(), 1);
}

#[test]
fn what_is_being_written_is_saved_as_it_goes() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    assert!(harness.store.drafts().unwrap().is_empty());
    harness.act(Action::ClockTick(NOW + 5));
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.subject, "Lunch");
    // Quietly, and without leaving the message.
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(!status(&harness).contains("Saved"), "{}", status(&harness));
    // Nothing new, nothing written.
    let before = drafts[0].updated;
    harness.act(Action::ClockTick(NOW + 10));
    assert_eq!(harness.store.drafts().unwrap()[0].updated, before);
}

#[test]
fn a_draft_emptied_out_is_thrown_away() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("x");
    harness.keys("cmd-s");
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
    harness.keys("backspace escape");
    assert!(harness.store.drafts().unwrap().is_empty());
}

#[test]
fn d_throws_a_draft_away_from_the_list() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("escape 1 g g j enter");
    assert!(harness.screen().contains("Lunch"));
    harness.keys("d");
    assert!(harness.store.drafts().unwrap().is_empty());
    assert!(
        status(&harness).contains("Thrown away."),
        "{}",
        status(&harness)
    );
}

#[test]
fn mail_in_the_outbox_can_be_taken_up_again() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-enter");
    // Drafts has gone from the sidebar with its last draft; Outbox is there.
    harness.keys("1 g g j enter");
    assert!(
        harness.screen().contains("[2] Outbox"),
        "{}",
        harness.screen()
    );
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("Subject:").contains("Lunch"));
    assert!(harness.store.outbox().unwrap().is_empty());
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
}

#[test]
fn a_reply_answers_the_sender_in_the_thread() {
    let mut harness = wide();
    harness.keys("r");
    assert_eq!(harness.state.context(), Context::Compose);
    let screen = harness.screen();
    assert!(screen.contains("[3] Reply"), "{screen}");
    assert!(
        harness
            .row_with("To:")
            .contains("Ana Ruiz <ana.ruiz@example.com>")
    );
    assert!(harness.row_with("Subject:").contains("Re: Q4 roadmap"));
    assert!(
        screen.contains("On Sat 14 Nov 2026 09:14, Ana Ruiz wrote:"),
        "{screen}"
    );
    assert!(screen.contains("> Hi Robin,"), "{screen}");
    // The cursor is in the body, above the quoted message.
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Body);
    harness.type_text("Thursday works.");
    harness.keys("cmd-enter");

    let (claimed, raw) = outgoing(&mut harness);
    assert_eq!(claimed.to, ["ana.ruiz@example.com"]);
    assert!(claimed.replied_to.is_some());
    assert!(raw.contains("Subject: Re: Q4 roadmap\r\n"), "{raw}");
    assert!(raw.contains("In-Reply-To: <"), "{raw}");
    assert!(raw.contains("References: <"), "{raw}");
    let written = raw.find("Thursday works.").unwrap();
    let quoted = raw.find("> Hi Robin,").unwrap();
    assert!(written < quoted, "{raw}");
}

#[test]
fn a_reply_can_go_below_the_quoted_message() {
    let mut harness = wide();
    harness.env.compose.reply_above = false;
    harness.keys("r");
    harness.type_text("Thursday works.");
    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    let written = raw.find("Thursday works.").unwrap();
    let quoted = raw.find("> Hi Robin,").unwrap();
    assert!(quoted < written, "{raw}");
}

#[test]
fn a_reply_opened_and_closed_leaves_nothing_behind() {
    let mut harness = wide();
    harness.keys("r escape");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.store.drafts().unwrap().is_empty());
    harness.keys("R");
    assert!(
        harness.screen().contains("[3] Reply to all"),
        "{}",
        harness.screen()
    );
    harness.keys("escape");
    assert!(harness.store.drafts().unwrap().is_empty());
}

#[test]
fn replying_works_from_the_open_message_too() {
    let mut harness = wide();
    harness.keys("j enter r");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("Subject:").contains("Re: [quill] PR"));
}

#[test]
fn a_forward_carries_the_message_and_asks_who_for() {
    let mut harness = wide();
    harness.keys("f");
    let screen = harness.screen();
    assert!(screen.contains("[3] Forward"), "{screen}");
    assert!(harness.row_with("Subject:").contains("Fwd: Q4 roadmap"));
    assert!(screen.contains("Forwarded message"), "{screen}");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::To);
    harness.type_text("priya.nair@example.com");
    harness.keys("cmd-enter");
    let (claimed, raw) = outgoing(&mut harness);
    assert_eq!(claimed.to, ["priya.nair@example.com"]);
    // A forward starts a conversation of its own and answers nobody.
    assert!(claimed.replied_to.is_none());
    assert!(!raw.contains("In-Reply-To:"), "{raw}");
    assert!(raw.contains("Here is the draft roadmap for Q4."), "{raw}");
}

/// What the store answers when asked to start a reply to a message it has
/// only the headers of.
fn not_downloaded(harness: &Harness) -> (quill_core::types::LocalMessageId, Action) {
    let id = harness.state.list.rows[0].id;
    let request = DataRequest::Respond {
        id,
        how: Response::Reply { all: false },
        date_line: "Sat 14 Nov 2026 09:14".to_owned(),
        position: quill_mime::ReplyPosition::Above,
    };
    (
        id,
        Action::Data(DataEvent::RespondNeedsBody(Box::new(request))),
    )
}

#[test]
fn a_message_not_yet_downloaded_is_fetched_before_it_is_answered() {
    let mut harness = wide();
    let (id, answer) = not_downloaded(&harness);
    harness.act(answer);
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(harness.sync_requests, [SyncRequest::FetchBody(id)]);
    assert!(
        status(&harness).contains("Downloading the message"),
        "{}",
        status(&harness)
    );
    // News of some other message opens nothing.
    let other = harness.state.list.rows[1].id;
    harness.act(Action::Sync(SyncEvent::BodyReady(other)));
    assert_eq!(harness.state.context(), Context::List);
    // It arrives, and the reply opens by itself.
    harness.act(Action::Sync(SyncEvent::BodyReady(id)));
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("Subject:").contains("Re: Q4 roadmap"));
}

#[test]
fn a_message_that_cannot_be_downloaded_says_so() {
    let mut harness = wide();
    let (id, answer) = not_downloaded(&harness);
    harness.act(answer);
    harness.act(Action::Sync(SyncEvent::BodyFailed {
        id,
        reason: "offline".to_owned(),
    }));
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        status(&harness).contains("could not be downloaded: offline"),
        "{}",
        status(&harness)
    );
    // And a later arrival opens nothing by surprise.
    harness.act(Action::Sync(SyncEvent::BodyReady(id)));
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn the_from_field_chooses_the_account() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    // Back from the body to the top.
    harness.keys("shift-tab shift-tab shift-tab shift-tab");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::From);
    harness.keys("right");
    assert!(
        harness.row_with("From:").contains("(Personal)"),
        "{}",
        harness.screen()
    );
    harness.keys("cmd-enter");
    let (claimed, raw) = outgoing(&mut harness);
    assert_ne!(claimed.from, "robin@example.com");
    assert!(
        raw.contains(&format!("From: <{}>\r\n", claimed.from)),
        "{raw}"
    );
}

#[test]
fn bcc_is_out_of_the_way_until_asked_for() {
    let mut harness = wide();
    harness.keys("c");
    assert!(!harness.screen().contains("Bcc:"));
    harness.keys("tab");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Cc);
    harness.keys("tab");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().field,
        Field::Subject
    );
    harness.keys("ctrl-o");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Bcc);
    assert!(harness.screen().contains("Bcc:"));
    harness.type_text("sam.lee@example.com");
    // With something in it, it stays in view and in the tab order.
    harness.keys("shift-tab shift-tab");
    harness.type_text("priya.nair@example.com");
    harness.keys("escape tab tab");
    assert_eq!(harness.state.compose.as_ref().unwrap().field, Field::Bcc);
    harness.keys("tab");
    harness.type_text("Quietly");
    harness.keys("tab");
    harness.type_text("Hello.");
    harness.keys("cmd-enter");

    let (claimed, raw) = outgoing(&mut harness);
    // A blind copy is delivered. The copy kept here says who got one;
    // the sender takes that line out of what goes on the wire.
    assert_eq!(
        claimed.to,
        ["priya.nair@example.com", "sam.lee@example.com"]
    );
    assert!(raw.contains("Bcc: <sam.lee@example.com>\r\n"), "{raw}");
    assert!(raw.contains("To: <priya.nair@example.com>\r\n"), "{raw}");
}

#[test]
fn a_file_is_attached_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agenda.txt");
    std::fs::write(&path, "1. Lunch\n2. Coffee\n").unwrap();

    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-A");
    assert!(harness.screen().contains("Attach the file at:"));
    // A path is pasted more often than typed.
    harness.act(Action::Paste(path.to_str().unwrap().to_owned()));
    harness.keys("enter");
    assert!(
        harness
            .row_with("Attach:")
            .contains("agenda.txt (19 bytes)"),
        "{}",
        harness.screen()
    );
    assert!(!harness.screen().contains("Attach the file at:"));

    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("multipart/mixed"), "{raw}");
    assert!(raw.contains("agenda.txt"), "{raw}");
    assert!(raw.contains("text/plain"), "{raw}");
}

#[test]
fn a_file_that_is_not_there_says_so_and_changes_nothing() {
    let mut harness = Harness::demo(100, 30);
    write_to_priya(&mut harness);
    harness.keys("ctrl-t");
    harness.type_text("/nonexistent/quill-test/agenda.pdf");
    harness.keys("enter");
    let screen = harness.screen();
    // Too long for one row of a narrow pane: it wraps instead of being cut.
    assert!(
        screen.contains("/nonexistent/quill-test/agenda.pdf"),
        "{screen}"
    );
    assert!(screen.contains("cannot be"), "{screen}");
    assert!(!screen.contains("Attach:"), "{screen}");
    assert_eq!(harness.state.context(), Context::Compose);
    // Escape leaves the question of a file, not the message.
    harness.keys("ctrl-t escape");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(!harness.screen().contains("Attach the file at:"));
}

#[test]
fn copying_in_the_body_reaches_the_clipboard() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-a cmd-c");
    assert_eq!(
        harness.host_requests,
        [HostRequest::Copy("Are you free on *Thursday*?".to_owned())]
    );
}

#[test]
fn pasting_goes_where_the_cursor_is() {
    let mut harness = wide();
    harness.keys("c tab tab");
    harness.act(Action::Paste("Minutes".to_owned()));
    harness.keys("tab");
    harness.act(Action::Paste("First line\r\nSecond line".to_owned()));
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.content().subject, "Minutes");
    assert_eq!(compose.content().body, "First line\nSecond line");
}

#[test]
fn zoom_gives_the_message_the_whole_window() {
    let mut harness = Harness::demo(100, 30);
    write_to_priya(&mut harness);
    harness.keys("ctrl-z");
    let screen = harness.screen();
    assert!(!screen.contains("[2]"), "{screen}");
    assert!(
        harness
            .row_with("To:")
            .contains("Priya Nair <priya.nair@example.com>,")
    );
    harness.keys("ctrl-z");
    assert!(harness.screen().contains("[2]"));
}

#[test]
fn the_message_survives_a_resize() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.resize(70, 20);
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        harness.screen().contains("Subject: Lunch"),
        "{}",
        harness.screen()
    );
    assert!(harness.cursor().is_some());
    harness.resize(30, 8);
    assert!(harness.cursor().is_some() || !harness.screen().is_empty());
}

#[test]
fn a_signature_goes_above_the_message_being_answered() {
    let mut harness = wide();
    harness.env.identities.insert(
        "Work".to_owned(),
        Identity {
            full_name: None,
            signature: Some("Robin Vale\nExample Ltd\n".to_owned()),
        },
    );
    harness.keys("r");
    let body = harness.state.compose.as_ref().unwrap().content().body;
    assert!(
        body.starts_with("\n\n-- \nRobin Vale\nExample Ltd\n\nOn Sat 14 Nov 2026 09:14, Ana Ruiz wrote:\n> Hi Robin,"),
        "{body:?}"
    );
    // Untouched, it is still nothing to keep.
    harness.keys("escape");
    assert!(harness.store.drafts().unwrap().is_empty());

    // Written below the quoted message, it ends the reply.
    harness.env.compose.reply_above = false;
    harness.keys("r");
    harness.type_text("Yes.");
    let body = harness.state.compose.as_ref().unwrap().content().body;
    assert!(body.starts_with("On Sat 14 Nov 2026"), "{body:?}");
    assert!(
        body.ends_with("Yes.\n\n-- \nRobin Vale\nExample Ltd\n"),
        "{body:?}"
    );
}

#[test]
fn hints_name_keys_that_can_be_pressed_in_a_terminal() {
    let mut harness = wide();
    // A terminal program keeps cmd for itself.
    harness.env.cmd_keys = false;
    harness.keys("c");
    let bar = status(&harness);
    assert!(
        bar.contains("ctrl-x send  ctrl-s save draft  ctrl-t attach"),
        "{bar}"
    );
    harness.type_text("pri");
    harness.keys("enter tab tab tab");
    harness.type_text("Hello.");
    harness.keys("ctrl-x");
    assert!(
        harness.screen().contains("ctrl-x again sends it as it is."),
        "{}",
        harness.screen()
    );
    harness.keys("ctrl-x");
    assert_eq!(harness.store.outbox().unwrap().len(), 1);
}

#[test]
fn a_message_a_server_refused_says_why_when_it_is_opened() {
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-enter");
    // The engine tries it, and the server will not have it.
    let (claimed, _) = outgoing(&mut harness);
    harness
        .store
        .send_failed(claimed.id, "550 no such person here", None)
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    // It waits in the outbox, to be put right.
    harness.keys("1 g g j enter enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        harness
            .screen()
            .contains("Not sent: 550 no such person here"),
        "{}",
        harness.screen()
    );
    // Corrected and sent again, it is in the outbox once.
    harness.type_text(" Sorry, wrong address before.");
    harness.keys("cmd-enter");
    let outbox = harness.store.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].state, DraftState::Outbox);
    assert_eq!(outbox[0].error, None);
}

#[test]
fn quitting_keeps_what_was_being_written() {
    let mut harness = wide();
    // Nothing being written, nothing to save.
    assert!(harness.state.parting_requests().is_empty());
    harness.keys("r");
    assert!(harness.state.parting_requests().is_empty());
    harness.type_text("Half a thought");
    let parting = harness.state.parting_requests();
    assert_eq!(parting.len(), 1);
    harness.pump(parting);
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].content.body.starts_with("Half a thought"));
}
