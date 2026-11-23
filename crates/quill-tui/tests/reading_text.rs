//! Selecting the words of the message on show with the mouse, copying
//! them, and saving the message as a file of its own.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataEvent;
use quill_tui::state::{Action, HostRequest, Mouse, MouseKind, Part, Selection};

fn mouse(harness: &mut Harness, kind: MouseKind, (column, row): (u16, u16)) {
    harness.act(Action::Mouse(Mouse { kind, column, row }));
}

/// The button down at `from`, taken to `to`, and let go there.
fn drag(harness: &mut Harness, from: (u16, u16), to: (u16, u16)) {
    mouse(harness, MouseKind::Press, from);
    mouse(harness, MouseKind::Drag, to);
    mouse(harness, MouseKind::Release, to);
}

fn click(harness: &mut Harness, at: (u16, u16)) {
    mouse(harness, MouseKind::Press, at);
    mouse(harness, MouseKind::Release, at);
}

/// The cell where `needle` first appears on screen, at or below `from_row`.
fn find_from(harness: &Harness, needle: &str, from_row: usize) -> (u16, u16) {
    for (row, line) in harness.screen().lines().enumerate().skip(from_row) {
        if let Some(at) = line.find(needle) {
            let column = quill_tui::text::width(&line[..at]);
            return (column as u16, row as u16);
        }
    }
    panic!("{needle:?} is not on screen:\n{}", harness.screen());
}

fn find(harness: &Harness, needle: &str) -> (u16, u16) {
    find_from(harness, needle, 0)
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// What has been put on the clipboard so far, oldest first.
fn copied(harness: &Harness) -> Vec<&str> {
    harness
        .host_requests
        .iter()
        .filter_map(|request| match request {
            HostRequest::Copy(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The demo mailbox with its first message open in the reader.
fn reading() -> Harness {
    let mut harness = Harness::demo(200, 60);
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Reader);
    harness.host_requests.clear();
    harness
}

/// The text of line `index` of the message on show.
fn line(harness: &Harness, index: usize) -> String {
    harness.state.reader.lines[index]
        .spans
        .iter()
        .map(|span| span.text.as_str())
        .collect()
}

/// The first line of the text with at least `least` characters in it, all
/// of them one cell wide: its place among the lines and its words.
fn a_line_of(harness: &Harness, least: usize) -> (usize, String) {
    (0..harness.state.reader.lines.len())
        .map(|index| (index, line(harness, index)))
        .find(|(_, words)| words.len() >= least && words.is_ascii() && !words.starts_with(' '))
        .unwrap()
}

/// Puts a message of our own at the top of the Work inbox, as downloaded
/// mail is kept, and opens it.
fn reading_our_own(raw: &str, subject: &str) -> Harness {
    let mut harness = Harness::demo(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("ours-1".to_owned());
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
    harness.host_requests.clear();
    harness
}

#[test]
fn words_dragged_over_are_selected_and_copy_copies_them() {
    let mut harness = reading();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    // Nothing is selected, so there is nothing for Copy to copy.
    assert!(!harness.state.is_enabled(&harness.env, CommandId::EditCopy));
    let plain = harness.buffer()[(at.0 + 4, at.1)].style();

    // From the third character to the eighth, both taken in.
    drag(&mut harness, (at.0 + 2, at.1), (at.0 + 7, at.1));
    assert!(harness.state.is_enabled(&harness.env, CommandId::EditCopy));
    // Drawn as selected, and nothing beside it is.
    let buffer = harness.buffer();
    assert_ne!(buffer[(at.0 + 2, at.1)].style(), plain);
    assert_ne!(buffer[(at.0 + 7, at.1)].style(), plain);
    assert_eq!(buffer[(at.0 + 1, at.1)].style(), plain);
    assert_eq!(buffer[(at.0 + 8, at.1)].style(), plain);
    // Nothing is copied until it is asked for.
    assert!(copied(&harness).is_empty());

    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [&words[2..8]]);
    assert!(status(&harness).contains("Copied."), "{}", status(&harness));
    // It stays selected, to be copied again.
    harness.keys("cmd-c");
    assert_eq!(copied(&harness).len(), 2);

    // Dragged the other way it is the same words.
    harness.host_requests.clear();
    drag(&mut harness, (at.0 + 7, at.1), (at.0 + 2, at.1));
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [&words[2..8]]);
}

#[test]
fn a_selection_over_several_lines_is_copied_a_row_to_a_line() {
    let mut harness = reading();
    let (index, first) = a_line_of(&harness, 12);
    let second = line(&harness, index + 1);
    let third = line(&harness, index + 2);
    let at = find_from(&harness, &first, 6);
    // From the fourth character of one line to the third cell of the
    // line two below it.
    drag(&mut harness, (at.0 + 3, at.1), (at.0 + 2, at.1 + 2));
    harness.keys("cmd-c");
    let wanted = format!(
        "{}\n{}\n{}",
        &first[3..],
        second.trim_end(),
        quill_tui::text::cells(&third, 0, 3).trim_end()
    );
    assert_eq!(copied(&harness), [wanted.as_str()]);
}

#[test]
fn a_click_selects_nothing_and_lets_go_of_what_was_selected() {
    let mut harness = reading();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 5, at.1));
    assert!(harness.state.reader.selection.is_some());
    click(&mut harness, (at.0 + 9, at.1));
    assert_eq!(harness.state.reader.selection, None);
    assert!(!harness.state.is_enabled(&harness.env, CommandId::EditCopy));
    harness.keys("cmd-c");
    assert!(copied(&harness).is_empty());
    // A press and a drag that never leaves its cell is a click too.
    mouse(&mut harness, MouseKind::Press, at);
    mouse(&mut harness, MouseKind::Drag, at);
    mouse(&mut harness, MouseKind::Release, at);
    assert_eq!(harness.state.reader.selection, None);
}

#[test]
fn a_double_click_selects_a_word_and_a_triple_click_the_line() {
    let mut harness = reading();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    let word = words.split(' ').next().unwrap();

    click(&mut harness, (at.0 + 1, at.1));
    click(&mut harness, (at.0 + 1, at.1));
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [word]);

    click(&mut harness, (at.0 + 1, at.1));
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [word, words.trim_end()]);

    // A fourth press starts again: a click, and nothing selected.
    click(&mut harness, (at.0 + 1, at.1));
    assert_eq!(harness.state.reader.selection, None);

    // Two presses in different places are two clicks.
    click(&mut harness, (at.0 + 1, at.1));
    click(&mut harness, (at.0 + 2, at.1));
    assert_eq!(harness.state.reader.selection, None);
    // And so are two too far apart in time.
    click(&mut harness, (at.0 + 1, at.1));
    harness.act(Action::ClockTick(NOW + 5));
    click(&mut harness, (at.0 + 1, at.1));
    assert_eq!(harness.state.reader.selection, None);
}

#[test]
fn the_fields_above_the_text_can_be_selected_too() {
    let mut harness = reading();
    let subject = harness.selected_subject().to_owned();
    let label = find(&harness, "Subject:");
    let value = find_from(&harness, &subject, usize::from(label.1));
    assert_eq!(value.1, label.1);
    let end = value.0 + subject.len() as u16 - 1;
    drag(&mut harness, value, (end, value.1));
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [subject.as_str()]);

    // Dragged on down into the text it stops at the last of the rows
    // above it: the text is another part.
    harness.host_requests.clear();
    drag(&mut harness, value, (value.0 + 3, value.1 + 30));
    assert!(matches!(
        harness.state.reader.selection,
        Some(Selection::Some {
            part: Part::Head,
            ..
        })
    ));
    harness.keys("cmd-c");
    let copied = copied(&harness)[0];
    assert!(copied.starts_with(&subject), "{copied}");
    assert!(!copied.contains(line(&harness, 0).trim()), "{copied}");
}

#[test]
fn select_all_takes_the_whole_message_and_its_fields_whole() {
    let mut harness = reading();
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::EditSelectAll)
    );
    harness.keys("cmd-a cmd-c");
    assert_eq!(harness.state.reader.selection, Some(Selection::All));
    let all = copied(&harness)[0].to_owned();
    let subject = harness.selected_subject().to_owned();
    assert!(all.starts_with("From:    "), "{all}");
    assert!(all.contains(&format!("Subject: {subject}\n")), "{all}");
    for index in 0..harness.state.reader.lines.len() {
        let words = line(&harness, index);
        assert!(all.contains(words.trim_end()), "{words:?} is not in {all}");
    }
    // The fields, a blank line, then the text.
    assert!(all.contains("\n\n"), "{all}");
    // It is the reader's alone: the list keeps its own Select All.
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::EditSelectAll)
    );
}

#[test]
fn one_key_copies_what_is_selected_or_else_the_whole_message() {
    let mut harness = reading();
    harness.keys("ctrl-y");
    let all = copied(&harness)[0].to_owned();
    assert!(all.starts_with("From:    "), "{all}");
    assert!(all.contains(line(&harness, 0).trim_end()), "{all}");
    assert!(
        status(&harness).contains("Copied the message."),
        "{}",
        status(&harness)
    );

    harness.host_requests.clear();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 3, at.1));
    harness.keys("ctrl-y");
    assert_eq!(copied(&harness), [&words[..4]]);

    // From the list too, for the message the cursor is on.
    harness.host_requests.clear();
    harness.keys("esc ctrl-y");
    assert_eq!(copied(&harness).len(), 1);
}

#[test]
fn a_selection_stays_with_its_words_when_the_text_is_scrolled() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    harness.host_requests.clear();
    let (index, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 5, at.1));
    mouse(&mut harness, MouseKind::WheelDown, at);
    assert!(harness.state.reader.scroll > 0);
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [&words[..6]]);
    // What is drawn as selected has moved with the line, or gone with it.
    let selected = harness.state.reader.selection.unwrap();
    assert_eq!(selected.columns(Part::Body, index), Some((0, 6)));
}

#[test]
fn dragging_past_the_foot_of_the_pane_brings_more_of_the_text_up() {
    let mut harness = Harness::demo(80, 24);
    harness.keys("enter");
    harness.host_requests.clear();
    let (index, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    let lines = harness.state.reader.lines.len();
    mouse(&mut harness, MouseKind::Press, at);
    // The pointer is taken below the pane and moved about there.
    for step in 0..(lines as u16 + 5) {
        mouse(&mut harness, MouseKind::Drag, (at.0 + step % 3, 30));
    }
    mouse(&mut harness, MouseKind::Release, (at.0, 30));
    assert!(harness.state.reader.scroll > 0);
    harness.keys("cmd-c");
    let copied = copied(&harness)[0];
    // From where it began to the end of the message.
    assert!(copied.starts_with(words.trim_end()), "{copied}");
    assert!(
        copied.ends_with(line(&harness, lines - 1).trim_end()),
        "{copied}"
    );
    assert_eq!(copied.lines().count(), lines - index);
}

#[test]
fn a_drag_that_begins_on_a_link_selects_it_and_does_not_open_it() {
    let mut harness = Harness::demo(200, 60);
    // The newsletter has links in its HTML.
    harness.keys("G k k enter");
    harness.host_requests.clear();
    let text = harness
        .state
        .reader
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.link.is_some() && span.style.link && span.text.trim().len() > 4)
        .map(|span| span.text.trim().to_owned())
        .unwrap();
    let at = find_from(&harness, &text, 6);
    drag(&mut harness, at, (at.0 + 3, at.1));
    assert!(
        harness.host_requests.is_empty(),
        "{:?}",
        harness.host_requests
    );
    harness.keys("cmd-c");
    assert_eq!(copied(&harness), [&text[..4]]);
    // A click on it still opens it, when the button is let go.
    harness.host_requests.clear();
    mouse(&mut harness, MouseKind::Press, at);
    assert!(harness.host_requests.is_empty());
    mouse(&mut harness, MouseKind::Release, at);
    assert!(matches!(
        harness.host_requests.as_slice(),
        [HostRequest::OpenUrl(_)]
    ));
}

#[test]
fn a_selection_is_let_go_when_the_text_is_laid_out_again_or_another_message_is_shown() {
    let mut harness = reading();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 5, at.1));
    assert!(harness.state.reader.selection.is_some());
    // A different width: the lines are other lines now.
    harness.resize(120, 40);
    assert_eq!(harness.state.reader.selection, None);

    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 5, at.1));
    assert!(harness.state.reader.selection.is_some());
    harness.keys("esc j");
    assert_eq!(harness.state.reader.selection, None);
}

#[test]
fn a_message_being_written_keeps_copy_and_select_all_for_itself() {
    let mut harness = reading();
    let (_, words) = a_line_of(&harness, 12);
    let at = find_from(&harness, &words, 6);
    drag(&mut harness, at, (at.0 + 5, at.1));
    harness.keys("r");
    assert_eq!(harness.state.context(), Context::Compose);
    // The reader's selection is out of sight, and is not what Copy means.
    assert!(!harness.state.is_enabled(&harness.env, CommandId::EditCopy));
}

/// The kinds of file on offer, as the list shows them.
fn offered(harness: &Harness) -> Vec<String> {
    harness
        .screen()
        .lines()
        .filter(|line| line.contains(".txt ") || line.contains(".html ") || line.contains(".eml "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_message_is_saved_as_text_in_downloads_and_never_over_a_file() {
    let mut harness = reading();
    let subject = harness.selected_subject().to_owned();
    harness.keys("ctrl-s");
    let kinds = offered(&harness);
    assert_eq!(kinds.len(), 3, "{}", harness.screen());
    assert!(kinds[0].contains("> Text"), "{kinds:?}");
    assert!(kinds[1].contains("Web page"), "{kinds:?}");
    assert!(kinds[2].contains("Message file"), "{kinds:?}");
    // Choosing one saves, and the keys say so.
    assert!(
        status(&harness).contains("enter save"),
        "{}",
        status(&harness)
    );

    harness.keys("enter");
    let name = format!("{subject}.txt");
    assert!(
        status(&harness).contains(&format!("Saved {name} in Downloads.")),
        "{}",
        status(&harness)
    );
    let downloads = harness.files.downloads.clone().unwrap();
    let saved = std::fs::read_to_string(downloads.join(&name)).unwrap();
    assert!(saved.starts_with("From:    "), "{saved}");
    assert!(saved.contains(&format!("Subject: {subject}\n")), "{saved}");
    assert!(saved.contains("Attach:  roadmap.pdf"), "{saved}");
    // The text is laid out for a page and not for this wide pane.
    assert!(
        saved
            .lines()
            .all(|line| quill_tui::text::width(line) <= 80 || line.starts_with("To:"))
    );
    assert!(saved.contains(line(&harness, 0).split(' ').next().unwrap()));

    // Saved again, the first is left alone.
    std::fs::write(downloads.join(&name), "mine").unwrap();
    harness.keys("ctrl-s enter");
    assert!(
        status(&harness).contains(&format!("Saved {subject} 2.txt in Downloads.")),
        "{}",
        status(&harness)
    );
    assert_eq!(std::fs::read(downloads.join(&name)).unwrap(), b"mine");
}

#[test]
fn a_message_is_saved_as_it_was_sent_for_any_mail_program() {
    let mut harness = reading();
    let subject = harness.selected_subject().to_owned();
    let id = harness.state.reader.row.as_ref().unwrap().id;
    // The third of the list: the message itself.
    harness.keys("ctrl-s down down enter");
    let downloads = harness.files.downloads.clone().unwrap();
    let saved = std::fs::read(downloads.join(format!("{subject}.eml"))).unwrap();
    assert_eq!(saved, harness.store.raw(id).unwrap().unwrap());
    // Typing narrows the list, as it does every list of its kind.
    harness.keys("ctrl-s");
    harness.type_text("mess");
    assert_eq!(offered(&harness).len(), 1, "{}", harness.screen());
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::Reader);
}

#[test]
fn in_the_app_the_mac_asks_where_a_saved_message_goes() {
    let mut harness = reading();
    harness.env.file_panel = true;
    let subject = harness.selected_subject().to_owned();
    harness.keys("cmd-S down enter");
    let [HostRequest::SaveFile { path, name }] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    assert_eq!(*name, format!("{subject}.html"));
    assert!(path.exists());
    // Nothing went to Downloads: where it goes is the user's to say.
    assert!(!harness.files.downloads.as_ref().unwrap().exists());
}

/// A message whose page shows a picture it carries, and tries a script.
const WITH_A_PICTURE: &str = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
    To: Robin Vale <robin@example.com>\r\n\
    Subject: Plans: Q3/Q4 <b>\r\n\
    MIME-Version: 1.0\r\n\
    Content-Type: multipart/related; boundary=\"r\"\r\n\
    \r\n\
    --r\r\n\
    Content-Type: text/html; charset=utf-8\r\n\
    \r\n\
    <html><body><p>The plan is below.</p><script>steal()</script>\
    <img src=\"cid:plan@example.com\" alt=\"The plan\">\
    <img src=\"https://example.com/pixel.gif\" alt=\"\"></body></html>\r\n\
    --r\r\n\
    Content-Type: image/png\r\n\
    Content-ID: <plan@example.com>\r\n\
    Content-Transfer-Encoding: base64\r\n\
    \r\n\
    iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\r\n\
    --r--\r\n";

#[test]
fn a_message_saved_as_a_page_is_one_safe_file_with_its_picture_inside() {
    let mut harness = reading_our_own(WITH_A_PICTURE, "Plans: Q3/Q4 <b>");
    harness.keys("ctrl-s down enter");
    // The subject names the file, with nothing of a path left in it.
    let name = "Plans- Q3-Q4 <b>.html";
    assert!(
        status(&harness).contains(&format!("Saved {name} in Downloads.")),
        "{}",
        status(&harness)
    );
    let downloads = harness.files.downloads.clone().unwrap();
    let page = std::fs::read_to_string(downloads.join(name)).unwrap();
    // The picture it carries is in the page, and nothing beside it is
    // named.
    assert!(
        page.contains("src=\"data:image/png;base64,iVBORw0KGgo"),
        "{page}"
    );
    assert!(!page.contains("./quill-"), "{page}");
    assert!(!page.contains("cid:"), "{page}");
    // It is the page the reading pane shows: nothing runs, and nothing is
    // fetched.
    assert!(!page.contains("steal()"), "{page}");
    assert!(!page.contains("https://example.com/pixel.gif"), "{page}");
    assert!(page.contains("Content-Security-Policy"), "{page}");
    assert!(page.contains("The plan is below."), "{page}");
    // Above the message, who it is from and what it is about: a
    // sender's words, and only ever text.
    let fields = page.find("Ana Ruiz &lt;ana.ruiz@example.com&gt;").unwrap();
    assert!(fields < page.find("The plan is below.").unwrap());
    assert!(page.contains("Plans: Q3/Q4 &lt;b&gt;</div>"), "{page}");
}

#[test]
fn only_mail_that_is_here_can_be_saved_or_copied() {
    let mut harness = Harness::demo(200, 60);
    // With the list's cursor on a message its text is in the reader.
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageSave)
    );
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageCopyText)
    );
    // In the accounts pane there is no message to mean.
    harness.keys("1");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageSave)
    );
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageCopyText)
    );
}
