//! Web reading: in Quill's own app, mail read in the web view pane, which
//! stands in the reader's place and follows the list.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::Harness;
use quill_core::types::LocalMessageId;
use quill_store::Mailbox;
use quill_tui::command::{CommandId, Context};
use quill_tui::data::{DataEvent, DataRequest, PageUse};
use quill_tui::files::RemoteContent;
use quill_tui::setup::{AccountEvent, AccountRequest, Preference};
use quill_tui::state::{Action, HostRequest, Mouse, MouseKind, PageNotice, Pane, Shown};

const SIZES: [(u16, u16); 2] = [(80, 24), (200, 60)];

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// A page put in the reading pane: its title, where it is, whether it may
/// load remote content, and the lines above it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Page {
    title: String,
    path: PathBuf,
    remote: bool,
    header: Vec<String>,
    notice: Option<PageNotice>,
}

/// Every page put in the reading pane so far, in order.
fn pages(harness: &Harness) -> Vec<Page> {
    harness
        .host_requests
        .iter()
        .filter_map(|request| match request {
            HostRequest::Reading {
                path,
                title,
                remote,
                header,
                notice,
            } => Some(Page {
                title: title.clone(),
                path: path.clone(),
                remote: *remote,
                header: header.clone(),
                notice: notice.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The page the reading pane shows.
fn showing(harness: &Harness) -> Page {
    pages(harness)
        .pop()
        .expect("the reading pane shows nothing")
}

fn html(page: &Page) -> String {
    std::fs::read_to_string(&page.path).unwrap()
}

/// The message under the list's cursor.
fn selected(harness: &Harness) -> LocalMessageId {
    harness.state.list.rows[harness.state.list.selected].id
}

/// Whether the store has the message of this subject as read.
fn is_read(harness: &Harness, subject: &str) -> bool {
    harness
        .store
        .messages(Mailbox::Unified)
        .unwrap()
        .into_iter()
        .find(|row| row.headers.subject == subject)
        .unwrap_or_else(|| panic!("no message called {subject}"))
        .flags
        .seen
}

/// The demo mailbox in the app with web reading on and the first message
/// on show.
fn reading() -> Harness {
    let mut harness = Harness::in_the_app(200, 60, true);
    harness.settle();
    harness
}

fn remembered(harness: &Harness) -> Vec<bool> {
    harness
        .account_requests
        .iter()
        .filter_map(|request| match request {
            AccountRequest::Remember(Preference::WebReading(on)) => Some(*on),
            _ => None,
        })
        .collect()
}

#[test]
fn in_a_terminal_there_is_no_web_reading_and_nothing_changes() {
    for config_says in [false, true] {
        let mut harness = Harness::demo(200, 60);
        // Even a config that asks for it: there is no pane to read in.
        harness.env.web_reading = config_says;
        harness.act(Action::Resize(200, 60));
        let before = harness.screen();
        assert!(before.contains("[3] Reader"), "{before}");
        assert!(!harness.state.web_reading);
        assert!(
            !harness
                .state
                .is_enabled(&harness.env, CommandId::ViewWebReading)
        );

        // Its key does nothing, by itself or from the app's menu.
        harness.keys("w");
        harness.act(Action::Command(CommandId::ViewWebReading));
        harness.settle();
        assert!(!harness.state.web_reading);
        assert_eq!(harness.screen(), before);
        assert!(harness.account_requests.is_empty());
        assert!(harness.host_requests.is_empty());
        assert_eq!(
            harness.state.host_state(&harness.env).checked,
            [CommandId::ViewSidebar, CommandId::RulesToggle]
        );
        assert_eq!(harness.state.settle_due(), None);

        // And it is not offered: not in the help, not in the palette, not
        // in the bottom bar.
        harness.keys("?");
        assert!(!harness.screen().contains("Web reading"));
        harness.keys("esc :");
        harness.type_text("web reading");
        assert!(!harness.screen().contains("Web reading pane"));
        harness.keys("esc");
        assert!(!status(&harness).contains("web/text"));

        // Space still pages the list, and enter still opens the reader.
        harness.keys("enter");
        assert_eq!(harness.state.context(), Context::Reader);
    }
}

#[test]
fn in_the_app_it_is_off_until_it_is_switched_on_and_the_choice_is_remembered() {
    let mut harness = Harness::in_the_app(200, 60, false);
    harness.settle();
    assert!(!harness.state.web_reading);
    assert!(harness.screen().contains("[3] Reader"));
    assert!(pages(&harness).is_empty());
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::ViewWebReading)
    );
    assert_eq!(
        harness.state.host_state(&harness.env).checked,
        [CommandId::ViewSidebar, CommandId::RulesToggle]
    );
    assert!(
        status(&harness).contains("w web/text"),
        "{}",
        status(&harness)
    );
    // The help says how.
    harness.keys("?");
    assert!(harness.screen().contains("Web reading pane"));
    harness.keys("esc");

    // On: the message under the cursor is in the pane at once, with who
    // it is from above it, and no reader is drawn here.
    harness.keys("w");
    assert!(harness.state.web_reading);
    assert_eq!(remembered(&harness), [true]);
    assert_eq!(
        harness.state.host_state(&harness.env).checked,
        [
            CommandId::ViewSidebar,
            CommandId::ViewWebReading,
            CommandId::RulesToggle
        ]
    );
    let page = showing(&harness);
    assert_eq!(page.title, "Q4 roadmap");
    assert!(!page.remote);
    assert_eq!(page.header[0], "From: Ana Ruiz <ana.ruiz@example.com>");
    assert!(page.header[1].starts_with("To: "), "{:?}", page.header);
    assert!(
        page.header.iter().any(|line| line.contains("2026")),
        "{:?}",
        page.header
    );
    assert!(
        page.header.last().unwrap().starts_with("Attached: "),
        "{:?}",
        page.header
    );
    let screen = harness.screen();
    assert!(!screen.contains("[3]"), "{screen}");
    assert!(screen.contains("[1] Accounts") && screen.contains("[2] Inbox"));
    assert!(
        status(&harness).contains("Messages are shown as web pages. w switches back."),
        "{}",
        status(&harness)
    );
    assert_eq!(harness.state.context(), Context::List);
    // Switching it on is not reading the message it happened to be on.
    assert!(!is_read(&harness, "Q4 roadmap"));

    // Off again: the pane goes, the reader is back, and that is
    // remembered too.
    harness.keys("w");
    assert!(!harness.state.web_reading);
    assert_eq!(remembered(&harness), [true, false]);
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::CloseReading)
    );
    assert_eq!(
        harness.state.host_state(&harness.env).checked,
        [CommandId::ViewSidebar, CommandId::RulesToggle]
    );
    let screen = harness.screen();
    assert!(screen.contains("[3] Reader"), "{screen}");
    // With its text laid out for it again.
    assert!(screen.contains("Here is the draft roadmap"), "{screen}");
    assert!(
        status(&harness).contains("Messages are shown as text."),
        "{}",
        status(&harness)
    );
    // Nothing follows the list any more.
    let shown = pages(&harness).len();
    harness.keys("j j");
    harness.settle();
    assert_eq!(pages(&harness).len(), shown);
    assert_eq!(harness.state.settle_due(), None);
}

#[test]
fn it_starts_on_when_the_config_says_so_and_shows_the_first_message_without_reading_it() {
    let mut harness = Harness::in_the_app(200, 60, true);
    assert!(harness.state.web_reading);
    assert!(!harness.screen().contains("[3]"));
    // Nothing is asked of the pane until the cursor has rested.
    assert!(pages(&harness).is_empty());
    assert!(harness.state.settle_due().is_some());
    harness.settle();
    assert_eq!(harness.state.settle_due(), None);
    assert_eq!(showing(&harness).title, "Q4 roadmap");
    assert_eq!(pages(&harness).len(), 1);
    // The cursor was put there by the mailbox opening, not by the user.
    assert!(!is_read(&harness, "Q4 roadmap"));
    // Nothing was written to the config: nothing was switched.
    assert!(remembered(&harness).is_empty());
    assert_eq!(
        harness.state.host_state(&harness.env).checked,
        [
            CommandId::ViewSidebar,
            CommandId::ViewWebReading,
            CommandId::RulesToggle
        ]
    );
}

#[test]
fn the_pane_follows_the_cursor_once_it_rests_and_shows_nothing_passed_on_the_way() {
    let mut harness = reading();
    let first = selected(&harness);
    assert_eq!(pages(&harness).len(), 1);

    // Three messages down, as a held key would go.
    let mut passed = Vec::new();
    for _ in 0..3 {
        let due = harness.state.settle_due();
        harness.keys("j");
        // Each move starts the wait again.
        assert!(harness.state.settle_due().is_some());
        assert_ne!(harness.state.settle_due(), due);
        passed.push(selected(&harness));
    }
    let landed = passed.pop().unwrap();
    assert_eq!(harness.selected_subject(), "Trail run Saturday");
    // Nothing was asked of the pane, and no page was made, for any of them.
    assert_eq!(pages(&harness).len(), 1);
    let view = harness.files.view_dir.clone().unwrap();
    for id in passed.iter().chain([&landed]) {
        assert!(!view.join(format!("m{}", id.0)).exists());
    }
    assert!(view.join(format!("m{}", first.0)).exists());

    // At rest: the one it rests on is shown, and only that one.
    harness.settle();
    assert_eq!(pages(&harness).len(), 2);
    assert_eq!(showing(&harness).title, "Trail run Saturday");
    assert!(view.join(format!("m{}", landed.0)).exists());
    for id in &passed {
        assert!(!view.join(format!("m{}", id.0)).exists());
    }
    assert_eq!(harness.state.reading_asked(), Some(&Shown::Page(landed)));
    // Shown again only when there is something new to show.
    harness.settle();
    harness.act(Action::ClockTick(common::NOW + 5));
    assert_eq!(pages(&harness).len(), 2);
}

#[test]
fn a_page_that_arrives_for_a_message_the_cursor_has_left_is_not_shown() {
    let mut harness = reading();
    harness.keys("j j");
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    let left = selected(&harness);
    // The cursor rests, and the page is asked for...
    let effects = harness.state.update(&harness.env, Action::SelectionSettled);
    assert_eq!(
        effects.data,
        [DataRequest::WebPage {
            message: left,
            remote: RemoteContent::IfSenderAllowed,
            then: PageUse::Reading,
        }]
    );
    // ...and before it is written, the cursor moves on.
    harness.keys("j");
    assert_eq!(harness.selected_subject(), "Trail run Saturday");
    harness.pump(effects.data);
    // The late page is dropped: the pane still shows what it showed, and
    // the message that was only passed is not read.
    assert_eq!(pages(&harness).len(), 1);
    assert_eq!(showing(&harness).title, "Q4 roadmap");
    assert!(!is_read(&harness, "Sunday dinner"));

    // The one the cursor came to rest on is what is shown next.
    harness.settle();
    assert_eq!(pages(&harness).len(), 2);
    assert_eq!(showing(&harness).title, "Trail run Saturday");

    // The same for a page that arrives after web reading was switched off.
    harness.keys("j");
    let effects = harness.state.update(&harness.env, Action::SelectionSettled);
    assert_eq!(effects.data.len(), 1);
    harness.keys("w");
    harness.pump(effects.data);
    assert_eq!(pages(&harness).len(), 2);
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::CloseReading)
    );
}

#[test]
fn a_message_gone_to_is_read_once_it_is_shown_as_one_opened_in_the_reader_is() {
    // With the reader drawn here, moving the cursor reads nothing, and
    // enter opens the message and reads it.
    let mut text = Harness::in_the_app(200, 60, false);
    text.keys("j j");
    assert!(!is_read(&text, "Sunday dinner"));
    text.keys("enter");
    assert!(is_read(&text, "Sunday dinner"));

    // In the pane, a message is opened when it has been shown: not while
    // the cursor is only passing.
    let mut harness = reading();
    harness.keys("j j");
    assert!(!is_read(&harness, "Sunday dinner"));
    harness.settle();
    assert_eq!(showing(&harness).title, "Sunday dinner");
    assert!(is_read(&harness, "Sunday dinner"));
    assert!(!harness.row_with("Sunday dinner").contains('*'));

    // Marked unread again, it stays unread while it stays on show.
    harness.keys("u");
    assert!(!is_read(&harness, "Sunday dinner"));
    harness.settle();
    harness.act(Action::ClockTick(common::NOW + 5));
    assert!(!is_read(&harness, "Sunday dinner"));
    assert_eq!(pages(&harness).len(), 2);

    // Enter opens the message at once, as it does with the reader: shown
    // now, read now, and the keys stay with the list.
    harness.keys("j");
    assert!(!is_read(&harness, "Trail run Saturday"));
    harness.keys("enter");
    assert!(is_read(&harness, "Trail run Saturday"));
    assert_eq!(showing(&harness).title, "Trail run Saturday");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(harness.state.focus, Pane::List);
    assert_eq!(harness.state.settle_due(), None);
}

#[test]
fn a_list_that_opens_or_a_wheel_that_passes_reads_nothing_and_a_click_does() {
    let mut harness = reading();
    // The wheel moves the cursor through the list without going to
    // anything: what it rests on is shown, and stays unread.
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::WheelDown,
        column: 60,
        row: 3,
    }));
    assert_eq!(harness.selected_subject(), "Trail run Saturday");
    harness.settle();
    assert_eq!(showing(&harness).title, "Trail run Saturday");
    assert!(!is_read(&harness, "Trail run Saturday"));

    // A click on a message is going to it: the third row of the list.
    harness.act(Action::Mouse(Mouse {
        kind: MouseKind::Press,
        column: 60,
        row: 3,
    }));
    assert_eq!(harness.selected_subject(), "Sunday dinner");
    assert!(!is_read(&harness, "Sunday dinner"));
    harness.settle();
    assert_eq!(showing(&harness).title, "Sunday dinner");
    assert!(is_read(&harness, "Sunday dinner"));

    // Another mailbox: the message its cursor starts on is shown, unread.
    harness.keys("1 j j j");
    assert_eq!(harness.state.mailbox_title(), "Work / Drafts");
    harness.keys("k");
    assert_eq!(harness.state.mailbox_title(), "Work / Inbox");
    harness.keys("2");
    harness.settle();
    assert_eq!(harness.selected_subject(), "Q4 roadmap");
    assert_eq!(showing(&harness).title, "Q4 roadmap");
    assert!(!is_read(&harness, "Q4 roadmap"));

    // And so is what a search finds.
    harness.keys("/");
    harness.type_text("offsite");
    harness.settle();
    assert_eq!(showing(&harness).title, "Re: Offsite agenda");
    assert!(!is_read(&harness, "Re: Offsite agenda"));
    // Until the user goes to it.
    harness.keys("enter enter");
    assert!(is_read(&harness, "Re: Offsite agenda"));
}

#[test]
fn choosing_several_messages_reads_none_of_them() {
    let mut harness = reading();
    harness.keys("j j");
    // Selected, and on to the next, twice over: each is shown as the
    // cursor rests on it, and neither is read for it.
    harness.keys("x");
    harness.settle();
    assert_eq!(showing(&harness).title, "Trail run Saturday");
    harness.keys("x");
    harness.settle();
    assert_eq!(showing(&harness).title, "Photos from Sat");
    for subject in ["Sunday dinner", "Trail run Saturday", "Photos from Sat"] {
        assert!(!is_read(&harness, subject), "{subject}");
    }
    // Read together, when that is what is asked.
    harness.keys("u");
    assert!(is_read(&harness, "Sunday dinner") && is_read(&harness, "Trail run Saturday"));
    assert!(!is_read(&harness, "Photos from Sat"));
}

#[test]
fn the_keys_for_the_message_work_from_the_list() {
    let mut harness = reading();
    // The newsletter: HTML, with a picture that reports its opening.
    harness.keys("G k k");
    harness.settle();
    assert_eq!(harness.state.context(), Context::List);
    let page = showing(&harness);
    assert_eq!(page.title, "Issue 88: Parsing email is still hard");
    assert!(!page.remote);
    let text = html(&page);
    assert!(!text.contains("open.gif"), "{text}");
    assert!(text.contains("Content-Security-Policy"), "{text}");
    assert!(!text.to_lowercase().contains("<script"));
    // The pane says what was left out, in a bar above the page, with a
    // button for each way of having it. The status line is left alone.
    assert_eq!(
        page.notice,
        Some(PageNotice {
            text: "1 remote picture was left out.".to_owned(),
            actions: vec![
                ("message.load_remote", "Load Pictures"),
                ("sender.always_remote", "Always from This Sender"),
            ],
        })
    );
    assert!(
        !status(&harness).contains("left out"),
        "{}",
        status(&harness)
    );

    // Space and its counterpart page the message, not the list.
    let at = harness.state.list.selected;
    let asked = harness.host_requests.len();
    harness.keys("space ctrl-d ctrl-u pageup");
    assert_eq!(
        harness.host_requests[asked..],
        [
            HostRequest::ScrollReading { down: true },
            HostRequest::ScrollReading { down: true },
            HostRequest::ScrollReading { down: false },
            HostRequest::ScrollReading { down: false },
        ]
    );
    assert_eq!(harness.state.list.selected, at);

    // i loads the remote pictures, in the pane.
    harness.keys("i");
    let page = showing(&harness);
    assert!(page.remote);
    assert_eq!(page.notice, None);
    assert!(html(&page).contains("https://example.com/open.gif"));
    assert_eq!(page.title, "Issue 88: Parsing email is still hard");

    // B opens the same safe page in the browser, and the pane stays.
    harness.keys("B");
    assert!(matches!(
        harness.host_requests.last(),
        Some(HostRequest::OpenFile(_))
    ));

    // o lists its links, over the panes.
    harness.keys("o");
    assert_eq!(harness.state.context(), Context::Links);
    assert!(harness.screen().contains("Links"));
    harness.keys("enter");
    assert!(matches!(
        harness.host_requests.last(),
        Some(HostRequest::OpenUrl(url)) if url.starts_with("https://")
    ));
    assert_eq!(harness.state.context(), Context::List);

    // There are no versions to switch between or reader to go to.
    for id in [CommandId::MessageToggleHtml, CommandId::FocusReader] {
        assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
    }
    let before = harness.host_requests.len();
    harness.keys("t 3");
    assert_eq!(harness.host_requests.len(), before);
    assert_eq!(harness.state.focus, Pane::List);

    // Tab has nowhere to take the keys from the list, with the message
    // in the host's pane, and says so by being off; shift-tab goes to the
    // accounts, and either comes back from them.
    assert!(!harness.state.is_enabled(&harness.env, CommandId::FocusNext));
    harness.keys("tab");
    assert_eq!(harness.state.focus, Pane::List);
    harness.keys("shift-tab");
    assert_eq!(harness.state.focus, Pane::Sidebar);
    harness.keys("tab");
    assert_eq!(harness.state.focus, Pane::List);
    harness.keys("shift-tab shift-tab");
    assert_eq!(harness.state.focus, Pane::List);

    // Star and unread.
    let subject = "Issue 88: Parsing email is still hard";
    harness.keys("s");
    assert!(harness.row_with(subject).contains('!'));
    harness.keys("s u");
    assert!(!is_read(&harness, subject));
    harness.keys("u");
    assert!(is_read(&harness, subject));

    // a lists the files of the message under the cursor.
    harness.keys("g g");
    harness.settle();
    assert_eq!(showing(&harness).title, "Q4 roadmap");
    harness.keys("a");
    assert_eq!(harness.state.context(), Context::Attachments);
    harness.keys("esc");

    // Reply: the message is written in the list's place, and the one being
    // answered stays in the pane.
    let shown = pages(&harness).len();
    harness.keys("r");
    assert_eq!(harness.state.context(), Context::Compose);
    let screen = harness.screen();
    assert!(screen.contains("Re: Q4 roadmap"), "{screen}");
    assert!(screen.contains("[1] Accounts"), "{screen}");
    assert!(!screen.contains("[2] Inbox"), "{screen}");
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.screen().contains("[2] Inbox"));
    assert_eq!(pages(&harness).len(), shown);
    for keys in ["R", "f"] {
        harness.keys(keys);
        assert_eq!(harness.state.context(), Context::Compose, "{keys}");
        harness.keys("esc");
    }

    // Move: the folders to choose from.
    harness.keys("m");
    assert_eq!(harness.state.context(), Context::Folders);
    harness.keys("esc");

    // Archive: it goes, and the one that takes its place is shown, and
    // read, once the cursor has rested on it.
    harness.keys("e");
    assert_ne!(harness.selected_subject(), "Q4 roadmap");
    let next = harness.selected_subject().to_owned();
    assert_eq!(next, "[quill] PR #42 passed");
    assert_eq!(pages(&harness).len(), shown);
    harness.settle();
    assert_eq!(showing(&harness).title, next);

    // Delete, the same.
    harness.keys("d");
    let next = harness.selected_subject().to_owned();
    assert_eq!(next, "Sunday dinner");
    assert!(!is_read(&harness, &next));
    harness.settle();
    assert_eq!(showing(&harness).title, next);
    assert!(is_read(&harness, &next));
}

#[test]
fn with_remote_pictures_set_to_never_the_pane_says_so_and_offers_nothing() {
    let mut harness = reading();
    harness.env.remote_never = true;
    harness.keys("G k k");
    harness.settle();
    let page = showing(&harness);
    assert_eq!(page.title, "Issue 88: Parsing email is still hard");
    assert_eq!(
        page.notice,
        Some(PageNotice {
            text: "1 remote picture was left out.".to_owned(),
            actions: Vec::new(),
        })
    );
    // A message with nothing left out of it has nothing said above it.
    harness.keys("g g");
    harness.settle();
    assert_eq!(showing(&harness).notice, None);
}

#[test]
fn a_plain_message_a_draft_and_an_empty_folder_all_have_a_page() {
    let mut harness = reading();
    // Plain text is a page of its text: the layout does not jump.
    harness.keys("j j");
    harness.settle();
    let page = showing(&harness);
    assert_eq!(page.title, "Sunday dinner");
    let text = html(&page);
    assert!(text.contains("<pre>"), "{text}");
    assert!(text.contains("Content-Security-Policy"), "{text}");

    // A draft never arrived, so there is no copy to make a page from: its
    // text is the page.
    harness.keys("c");
    harness.type_text("pri");
    harness.keys("enter tab tab");
    harness.type_text("Lunch <soon>");
    harness.keys("tab");
    harness.type_text("Are you free & hungry?");
    harness.keys("ctrl-s esc");
    assert_eq!(harness.state.context(), Context::List);
    harness.keys("1 j 2");
    assert_eq!(harness.state.mailbox_title(), "Drafts");
    harness.settle();
    let page = showing(&harness);
    assert_eq!(page.title, "Lunch <soon>");
    let text = html(&page);
    assert!(text.contains("Are you free &amp; hungry?"), "{text}");
    assert!(text.contains("<title>Lunch &lt;soon&gt;</title>"), "{text}");
    assert!(text.contains("Content-Security-Policy"), "{text}");
    assert_eq!(
        harness.state.reading_asked(),
        Some(&Shown::Text(selected(&harness)))
    );

    // A folder with nothing in it: the pane says so, and stays.
    harness.keys("1 j j j j j j 2");
    assert_eq!(harness.state.mailbox_title(), "Work / Trash");
    assert!(harness.state.list.rows.is_empty());
    harness.settle();
    let page = showing(&harness);
    assert_eq!((page.title.as_str(), page.remote), ("", false));
    assert!(page.header.is_empty());
    let text = html(&page);
    assert!(text.contains("No message selected"), "{text}");
    assert!(text.contains("Content-Security-Policy"), "{text}");
    assert_ne!(
        harness.host_requests.last(),
        Some(&HostRequest::CloseReading)
    );
}

#[test]
fn a_message_that_cannot_be_shown_says_why_in_the_pane() {
    let mut harness = reading();
    harness.keys("j");
    let effects = harness.state.update(&harness.env, Action::SelectionSettled);
    assert_eq!(effects.data.len(), 1);
    // The page could not be written.
    harness.act(Action::Data(DataEvent::PageFailed {
        message: selected(&harness),
        problem: "That message cannot be read.\u{1b}[31m".to_owned(),
    }));
    let page = showing(&harness);
    assert_eq!(page.title, "");
    let text = html(&page);
    assert!(text.contains("That message cannot be read."), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
    // Not the message before it, and nothing of this one's.
    assert!(page.header.is_empty());
    // It is not asked for over and over: the reason stays until the
    // cursor moves on.
    let shown = pages(&harness).len();
    harness.settle();
    harness.act(Action::ClockTick(common::NOW + 5));
    assert_eq!(pages(&harness).len(), shown);
    harness.keys("j");
    harness.settle();
    assert_eq!(showing(&harness).title, "Sunday dinner");
    harness.keys("k");
    harness.settle();
    assert_eq!(showing(&harness).title, "[quill] PR #42 passed");

    // A failure for a message the cursor has left changes nothing.
    let shown = pages(&harness).len();
    harness.act(Action::Data(DataEvent::PageFailed {
        message: LocalMessageId(999_999),
        problem: "Nope.".to_owned(),
    }));
    assert_eq!(pages(&harness).len(), shown);
}

#[test]
fn the_choice_not_being_written_down_is_said_and_holds_for_the_run() {
    let mut harness = Harness::in_the_app(200, 60, false);
    harness.keys("w");
    harness.act(Action::Account(AccountEvent::Remembered(Ok(()))));
    assert!(harness.state.notices.is_empty());
    assert!(
        status(&harness).contains("Messages are shown as web pages."),
        "{}",
        status(&harness)
    );
    harness.act(Action::Account(AccountEvent::Remembered(Err(
        "config.toml cannot be read: line 3\u{7}".to_owned(),
    ))));
    assert!(harness.state.web_reading);
    assert!(harness.state.notices.is_empty());
    assert!(
        status(&harness)
            .contains("That holds until Quill is closed: config.toml cannot be read: line 3"),
        "{}",
        status(&harness)
    );
}

#[test]
fn switching_on_from_the_reader_leaves_the_keys_with_the_list() {
    let mut harness = Harness::in_the_app(80, 24, false);
    harness.keys("j enter");
    assert_eq!(harness.state.context(), Context::Reader);
    harness.keys("w");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(showing(&harness).title, "[quill] PR #42 passed");
    let screen = harness.screen();
    assert!(
        screen.contains("[2] Inbox") && !screen.contains("[3]"),
        "{screen}"
    );
    // Zoom is of the list, and back.
    harness.keys("z");
    assert!(!harness.screen().contains("[1] Accounts"));
    harness.keys("z");
    assert!(harness.screen().contains("[1] Accounts"));
}

/// How wide the accounts pane and the list are drawn, and whether a
/// reader is drawn beside them.
fn widths(harness: &Harness) -> (u16, u16, bool) {
    let areas =
        quill_tui::layout::compute(&harness.state, harness.state.size, harness.env.reader_width);
    (
        areas.sidebar.map_or(0, |rect| rect.width),
        areas.list.map_or(0, |rect| rect.width),
        areas.reader.is_some(),
    )
}

/// The rows of the panes, without the bottom bar: the cells of each from
/// column `from` up to `to`, trailing spaces trimmed.
fn cells(harness: &Harness, from: u16, to: u16) -> Vec<String> {
    let buffer = harness.buffer();
    let width = usize::from(buffer.area.width);
    let mut rows: Vec<String> = buffer
        .content
        .chunks(width.max(1))
        .map(|row| {
            row.iter()
                .take(usize::from(to))
                .skip(usize::from(from))
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    rows.pop();
    rows
}

/// What is drawn in the first `columns` columns of the panes.
fn panes(harness: &Harness, columns: u16) -> Vec<String> {
    cells(harness, 0, columns)
}

#[test]
fn the_reading_pane_takes_the_readers_place_and_the_list_keeps_its_width() {
    for window in [100_u16, 120, 147, 200, 233] {
        // The app says how wide its window is; with the reader drawn
        // here that changes nothing, and there is no place to tell.
        let mut harness = Harness::in_the_app(window, 40, false);
        harness.keys("j j");
        let before = harness.screen();
        harness.act(Action::WindowColumns(window));
        assert_eq!(harness.screen(), before);
        assert_eq!(harness.state.reading_place(&harness.env), None);
        assert!(!harness.state.resize_expected());
        let (accounts, list, reader) = widths(&harness);
        assert!(reader, "{window}");
        let kept = accounts + list;
        let drawn = panes(&harness, kept);

        // Switched on and off, over and over: the place is the same
        // every time, and the accounts and the list do not move by a
        // cell, at any moment.
        for round in 0..6 {
            harness.keys("w");
            harness.settle();
            assert_eq!(
                harness.state.reading_place(&harness.env),
                Some(kept),
                "{window}, round {round}"
            );
            // The terminal still has the whole window: the pane is not
            // up yet. What it will cover is already empty.
            assert!(harness.state.resize_expected());
            assert_eq!(widths(&harness), (accounts, list, false));
            assert_eq!(panes(&harness, kept), drawn, "{window}, round {round}");
            assert!(
                cells(&harness, kept, window).iter().all(String::is_empty),
                "{window}: something is drawn where the pane goes"
            );

            // The app puts the pane up, and the terminal is narrower.
            harness.resize(kept, 40);
            assert!(!harness.state.resize_expected());
            assert_eq!(widths(&harness), (accounts, list, false));
            assert_eq!(panes(&harness, kept), drawn, "{window}, round {round}");

            // Off: the pane goes, and until the terminal has the window
            // back a width is expected.
            harness.keys("w");
            assert_eq!(harness.state.reading_place(&harness.env), None);
            assert!(harness.state.resize_expected());
            harness.resize(window, 40);
            assert!(!harness.state.resize_expected());
            assert_eq!(widths(&harness), (accounts, list, true));
            assert_eq!(panes(&harness, kept), drawn, "{window}, round {round}");
        }
    }
}

#[test]
fn with_the_accounts_hidden_the_reading_pane_has_their_columns_and_the_list_keeps_its() {
    for window in [100_u16, 147, 200, 233] {
        let mut harness = Harness::in_the_app(window, 40, true);
        harness.act(Action::WindowColumns(window));
        harness.settle();
        let place = |harness: &Harness| harness.state.reading_place(&harness.env).unwrap();
        let kept = place(&harness);
        harness.resize(kept, 40);
        let (accounts, list, _) = widths(&harness);
        assert!(accounts > 0 && accounts + list == kept, "{window}");

        // Hidden: the terminal is to keep the list's columns alone.
        harness.keys("\\");
        assert_eq!(place(&harness), list, "{window}");
        harness.resize(list, 40);
        assert_eq!(widths(&harness), (0, list, false), "{window}");
        assert!(!harness.screen().contains("[1] Accounts"));
        // Gone to, the accounts come up over the list, and tab puts
        // them away again.
        harness.keys("shift-tab");
        assert!(harness.screen().contains("[1] Accounts"));
        assert_eq!(place(&harness), list, "{window}");
        harness.keys("tab");
        assert_eq!(harness.state.focus, Pane::List);
        assert!(!harness.screen().contains("[1] Accounts"));

        // Put back: the place it had, and the list no wider or narrower.
        harness.keys("\\");
        assert_eq!(place(&harness), kept, "{window}");
        harness.resize(kept, 40);
        assert_eq!(widths(&harness), (accounts, list, false), "{window}");
    }
}

#[test]
fn the_place_follows_the_window_as_it_is_resized_with_web_reading_on() {
    let mut harness = Harness::in_the_app(147, 40, true);
    harness.act(Action::WindowColumns(147));
    harness.settle();
    let place = |harness: &Harness| harness.state.reading_place(&harness.env).unwrap();
    let kept = place(&harness);
    harness.resize(kept, 40);

    for window in [180_u16, 233, 120, 100, 147] {
        // What the text reader's layout would be in a window that wide.
        let mut text = Harness::in_the_app(window, 40, false);
        text.keys("j");
        let (accounts, list, _) = widths(&text);

        // The window is resized: the app says so, is told the new
        // place, and puts the terminal there.
        harness.act(Action::WindowColumns(window));
        let kept = place(&harness);
        assert_eq!(kept, accounts + list, "{window}");
        harness.resize(kept, 40);
        assert_eq!(widths(&harness), (accounts, list, false), "{window}");
    }
    // Back at the first width, the first place: nothing has crept.
    assert_eq!(place(&harness), kept);

    // The same number again is nothing new, and draws nothing.
    harness.state.dirty = false;
    harness.act(Action::WindowColumns(147));
    assert!(!harness.state.dirty);
}

#[test]
fn in_a_narrow_window_the_reading_pane_has_the_readers_share() {
    // Too narrow for a reader beside the list: there is no place of the
    // reader's to take, so the pane has its share of the window and the
    // list is drawn in what is left.
    let mut harness = Harness::in_the_app(80, 24, true);
    harness.act(Action::WindowColumns(80));
    harness.settle();
    assert_eq!(harness.state.reading_place(&harness.env), Some(40));
    harness.resize(40, 24);
    assert_eq!(widths(&harness), (0, 40, false));
    // The accounts come over the list when they are gone to, and esc
    // puts them away again, as in any narrow terminal.
    harness.keys("1");
    assert!(harness.screen().contains("[1] Accounts"));
    harness.keys("esc");
    assert!(!harness.screen().contains("[1] Accounts"));
    assert_eq!(harness.state.focus, Pane::List);
}

#[test]
fn a_host_that_has_not_said_how_wide_its_window_is_gets_no_place() {
    let mut harness = Harness::in_the_app(200, 60, true);
    harness.settle();
    assert_eq!(harness.state.reading_place(&harness.env), None);
    assert!(!harness.state.resize_expected());
    // Drawn by the terminal's own width, as before.
    assert_eq!(widths(&harness), (28, 172, false));
}

#[test]
fn the_app_has_one_command_for_the_web_pane_and_no_page_beside_the_reader() {
    let mut harness = Harness::in_the_app(200, 60, false);
    // The key the page beside the reader had does nothing here.
    harness.keys("enter v");
    assert!(!harness.state.web_reading);
    assert!(harness.host_requests.is_empty());
    assert!(harness.screen().contains("[3] Reader"));

    // What the app builds its menus and toolbar from names every command
    // once, and none of them twice under another name.
    let announced = quill_tui::app::command_infos(&harness.env);
    for (index, command) in announced.iter().enumerate() {
        assert_ne!(command.name, "message.view_html");
        for other in &announced[index + 1..] {
            assert_ne!(command.name, other.name);
            assert_ne!(command.title, other.title);
        }
    }
    let web: Vec<_> = announced
        .iter()
        .filter(|command| command.title.to_lowercase().contains("web"))
        .collect();
    assert_eq!(web.len(), 1, "{web:?}");
    assert_eq!(web[0].name, "view.web_reading");
    assert_eq!(web[0].keys, ["w"]);

    // And the help lists each thing once, with w for the web pane and B
    // for the browser.
    for context in [Context::List, Context::Reader] {
        let lines = quill_tui::view::help::lines(&harness.env, context);
        let entries: Vec<(&str, &str)> = lines
            .iter()
            .filter_map(|line| match line {
                quill_tui::view::help::HelpLine::Entry { keys, title, .. } => {
                    Some((keys.as_str(), *title))
                }
                _ => None,
            })
            .collect();
        for (index, (keys, title)) in entries.iter().enumerate() {
            assert_ne!(*keys, "v", "{title}");
            assert!(
                entries[index + 1..].iter().all(|(_, other)| other != title),
                "{title} is listed twice"
            );
        }
        assert!(entries.contains(&("w", "Web reading pane")), "{entries:?}");
        assert!(
            entries.contains(&("B", "Open in the browser")),
            "{entries:?}"
        );
    }
}

#[test]
fn screens_with_web_reading_on() {
    for (width, height) in SIZES {
        // The accounts and the list, and nothing where the reader was.
        let mut harness = Harness::in_the_app(width, height, true);
        harness.settle();
        harness.keys("j j");
        harness.settle();
        insta::assert_snapshot!(
            format!("web_reading_list_{width}x{height}"),
            harness.screen()
        );

        // A reply is written in the list's place.
        harness.keys("r");
        insta::assert_snapshot!(
            format!("web_reading_reply_{width}x{height}"),
            harness.screen()
        );
    }
}
