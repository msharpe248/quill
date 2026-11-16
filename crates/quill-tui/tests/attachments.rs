//! A message's files: listed in the reader, and looked at, played, opened
//! and saved from the list.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;

use common::Harness;
use quill_tui::command::{CommandId, Context};
use quill_tui::state::{Action, HostRequest};

/// The row of the attachment list for the file called `name`: the one that
/// also says what kind of file it is.
fn listed(harness: &Harness, name: &str, kind: &str) -> String {
    harness
        .screen()
        .lines()
        .find(|line| line.contains(name) && line.contains(kind))
        .unwrap_or_else(|| panic!("{name} is not listed:\n{}", harness.screen()))
        .to_owned()
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// The demo mailbox with the message whose subject starts with `subject`
/// open in the reader.
fn reading(subject: &str) -> Harness {
    reading_in("", subject)
}

/// As [`reading`], after `keys` have chosen a mailbox.
fn reading_in(keys: &str, subject: &str) -> Harness {
    opened(Harness::demo(200, 60), keys, subject)
}

/// As [`reading`], in a terminal of this size that draws pictures.
fn reading_with_pictures(width: u16, height: u16, subject: &str) -> Harness {
    opened(Harness::with_pictures(width, height, false), "", subject)
}

/// The picture the reader last asked to be made ready: its id, the file
/// and what kind of file it is. Messages passed on the way to the one being
/// read asked for theirs too.
fn picture_asked_for(harness: &Harness) -> (u32, std::path::PathBuf, String) {
    harness
        .host_requests
        .iter()
        .rev()
        .find_map(|request| match request {
            HostRequest::LoadPicture {
                id,
                path,
                content_type,
            } => Some((*id, path.clone(), content_type.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no picture was asked for: {:?}", harness.host_requests))
}

fn opened(mut harness: Harness, keys: &str, subject: &str) -> Harness {
    harness.keys(keys);
    let index = harness
        .state
        .list
        .rows
        .iter()
        .position(|row| row.headers.subject.starts_with(subject))
        .unwrap_or_else(|| panic!("no message called {subject}:\n{}", harness.screen()));
    harness.keys("2 g g");
    for _ in 0..index {
        harness.keys("j");
    }
    harness.keys("enter");
    assert!(harness.selected_subject().starts_with(subject));
    harness
}

#[test]
fn the_reader_names_a_messages_files() {
    let harness = reading("Q4 roadmap");
    let row = harness.row_with("Attach:");
    assert!(row.contains("roadmap.pdf (937 bytes)"), "{row}");
    assert!(row.contains("(a lists them)"), "{row}");
    // A message with none has no such line.
    let harness = reading("[quill] PR");
    assert!(!harness.screen().contains("Attach:"));
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageAttachments)
    );
}

#[test]
fn a_lists_the_files_with_what_they_are_and_how_big() {
    let mut harness = reading("Photos from Sat");
    harness.keys("a");
    assert_eq!(harness.state.context(), Context::Attachments);
    let screen = harness.screen();
    assert!(screen.contains(" Attachments "), "{screen}");
    let first = listed(&harness, "harbour.png", "image/png");
    assert!(
        first.contains("> harbour.png") && first.contains("1 KB"),
        "{first}"
    );
    let second = listed(&harness, "ridge.jpg", "image/jpeg");
    assert!(
        second.contains("  ridge.jpg") && second.contains("4 KB"),
        "{second}"
    );
    assert!(status(&harness).contains("enter view  s save  O open in its app  esc close"));

    // The cursor moves and stops at the ends; esc closes the list only.
    harness.keys("j j j");
    assert!(listed(&harness, "ridge.jpg", "image/jpeg").contains("> ridge.jpg"));
    harness.keys("k k");
    assert!(listed(&harness, "harbour.png", "image/png").contains("> harbour.png"));
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::Reader);
    assert!(harness.screen().contains("Photos from Sat"));
}

#[test]
fn enter_shows_a_file_with_the_others_a_step_away() {
    let mut harness = reading("Photos from Sat");
    harness.keys("a j enter");
    let [HostRequest::Preview { files, index }] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    // Both pictures, starting with the one chosen.
    assert_eq!(*index, 1);
    let names: Vec<&str> = files.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(names, ["harbour.png", "ridge.jpg"]);
    // They are on disk, whole, in a folder nobody else can enter, and
    // readable by nobody else.
    let (path, _) = &files[0];
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.starts_with(b"\x89PNG\r\n"));
    assert_eq!(bytes.len(), 1656);
    assert!(
        std::fs::read(&files[1].0)
            .unwrap()
            .starts_with(&[0xFF, 0xD8])
    );
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert!(path.starts_with(harness.files.view_dir.as_ref().unwrap()));
    // The list stays open behind the viewer, for the next file.
    assert_eq!(harness.state.context(), Context::Attachments);
}

#[test]
fn sound_is_played_and_is_not_one_of_the_files_to_page_through() {
    let mut harness = reading("Recipe you asked for");
    harness.keys("a enter");
    let [HostRequest::Play { path, title }] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    assert_eq!(title, "kitchen-timer.wav");
    assert!(std::fs::read(path).unwrap().starts_with(b"RIFF"));

    // A message with a text file and a picture: looking at one offers both.
    // It is in Work's Sent folder.
    let mut harness = reading_in("1 j j j j enter", "Offsite agenda");
    harness.keys("a enter");
    let [HostRequest::Preview { files, index }] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    assert_eq!((files.len(), *index), (2, 0));
}

#[test]
fn shift_o_opens_one_file_in_its_own_application() {
    let mut harness = reading("Q4 roadmap");
    harness.keys("a O");
    let [HostRequest::OpenFile(path)] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    assert!(path.ends_with("roadmap.pdf"));
    assert!(std::fs::read(path).unwrap().starts_with(b"%PDF-1.4"));
}

#[test]
fn in_a_terminal_s_saves_to_downloads_and_never_over_a_file() {
    let mut harness = reading("Q4 roadmap");
    harness.keys("a s");
    assert!(
        status(&harness).contains("Saved roadmap.pdf in Downloads."),
        "{}",
        status(&harness)
    );
    let downloads = harness.files.downloads.clone().unwrap();
    assert!(
        std::fs::read(downloads.join("roadmap.pdf"))
            .unwrap()
            .starts_with(b"%PDF-1.4")
    );
    // Saved again, the first copy is left alone.
    std::fs::write(downloads.join("roadmap.pdf"), "mine").unwrap();
    harness.keys("s");
    assert!(
        status(&harness).contains("Saved roadmap 2.pdf in Downloads."),
        "{}",
        status(&harness)
    );
    assert_eq!(
        std::fs::read(downloads.join("roadmap.pdf")).unwrap(),
        b"mine"
    );
    assert!(harness.host_requests.is_empty());
}

#[test]
fn in_the_app_s_asks_where_with_its_own_panel() {
    let mut harness = reading("Q4 roadmap");
    harness.env.file_panel = true;
    harness.keys("a s");
    let [HostRequest::SaveFile { path, name }] = harness.host_requests.as_slice() else {
        panic!("{:?}", harness.host_requests);
    };
    assert_eq!(name, "roadmap.pdf");
    assert!(path.exists());
    // Nothing went to Downloads: where it goes is the user's to say.
    assert!(!harness.files.downloads.as_ref().unwrap().exists());

    harness.act(Action::FileSaved {
        name: "Q4 plan.pdf".to_owned(),
        saved: true,
    });
    assert!(
        status(&harness).contains("Saved Q4 plan.pdf."),
        "{}",
        status(&harness)
    );
    harness.act(Action::FileSaved {
        name: "roadmap.pdf".to_owned(),
        saved: false,
    });
    assert!(
        status(&harness).contains("Not saved."),
        "{}",
        status(&harness)
    );
}

#[test]
fn the_list_belongs_to_the_message_it_was_opened_on() {
    let mut harness = reading("Q4 roadmap");
    harness.keys("a");
    assert_eq!(harness.state.context(), Context::Attachments);
    // Typing a command of the reader's does nothing here.
    harness.keys("d r c");
    assert_eq!(harness.state.context(), Context::Attachments);
    assert_eq!(harness.store.drafts().unwrap().len(), 0);
    // From the list pane, `a` works only for the message that is open.
    harness.keys("esc esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::MessageAttachments)
    );
    harness.keys("j");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageAttachments)
    );
}

#[test]
fn without_a_folder_for_viewing_the_reason_is_said() {
    let mut harness = reading("Q4 roadmap");
    harness.files.view_dir = None;
    harness.keys("a enter");
    assert!(harness.host_requests.is_empty());
    assert!(
        status(&harness).contains("nowhere to put files"),
        "{}",
        status(&harness)
    );
}

#[test]
fn the_first_picture_is_drawn_above_the_text() {
    let mut harness = reading_with_pictures(200, 60, "Photos from Sat");
    // The file is taken out and asked to be decoded; until that is done
    // there is nothing to draw and the text starts under the headers.
    let (id, path, kind) = picture_asked_for(&harness);
    assert_eq!(kind, "image/png");
    assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    assert!(harness.placements().is_empty());

    harness.act(Action::PictureLoaded {
        id,
        size: Some((320, 200)),
    });
    let images = harness.placements();
    assert_eq!(images.len(), 1);
    let image = images[0];
    assert_eq!(image.id, id);
    // Twenty rows at twice its size, and sixty-four columns to keep the
    // shape in cells twice as tall as wide.
    assert_eq!((image.area.width, image.area.height), (64, 20));

    // The cells under it are left empty, between the headers and the text.
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    let top = usize::from(image.area.y);
    assert!(rows[top - 2].contains("Attach:"), "{}", rows[top - 2]);
    for row in &rows[top..top + 20] {
        let inside: String = row
            .chars()
            .skip(usize::from(image.area.x))
            .take(64)
            .collect();
        assert!(inside.trim().is_empty(), "{row}");
    }
    assert!(!rows[top + 21].trim_matches(['│', '┃', ' ']).is_empty());
}

#[test]
fn a_file_that_turns_out_not_to_be_a_picture_is_just_a_file() {
    let mut harness = reading_with_pictures(200, 60, "Photos from Sat");
    let (id, _, _) = picture_asked_for(&harness);
    // An answer about some other picture changes nothing.
    harness.act(Action::PictureLoaded {
        id: id ^ 2,
        size: Some((320, 200)),
    });
    assert!(harness.placements().is_empty());
    harness.act(Action::PictureLoaded { id, size: None });
    assert!(harness.placements().is_empty());
    assert!(harness.state.reader.picture.is_none());
    assert!(harness.row_with("Attach:").contains("harbour.png"));
}

#[test]
fn a_terminal_that_draws_no_pictures_is_asked_for_none() {
    let harness = reading("Photos from Sat");
    assert!(harness.host_requests.is_empty());
    assert!(harness.placements().is_empty());
    assert!(harness.state.reader.picture.is_none());
    // A message with no picture among its files has none to draw, in any
    // terminal.
    let harness = reading_with_pictures(200, 60, "Recipe you asked for");
    assert!(harness.state.reader.picture.is_none());
    assert!(harness.placements().is_empty());
}

#[test]
fn the_host_pictures_what_quill_cannot_decode() {
    // A PDF's first page, where there is a host to make it.
    let harness = opened(Harness::with_pictures(200, 60, true), "", "Q4 roadmap");
    let (id, path, kind) = picture_asked_for(&harness);
    assert_eq!(kind, "application/pdf");
    assert!(path.ends_with("roadmap.pdf"));
    assert_eq!(harness.state.reader.picture.as_ref().unwrap().id, id);
    // Without one, a PDF is a file in the list and no more.
    let harness = reading_with_pictures(200, 60, "Q4 roadmap");
    assert!(harness.state.reader.picture.is_none());
    assert!(harness.host_requests.is_empty());
}

#[test]
fn a_picture_is_not_placed_under_an_overlay_or_in_a_pane_too_small() {
    let mut harness = reading_with_pictures(200, 60, "Photos from Sat");
    let (id, _, _) = picture_asked_for(&harness);
    harness.act(Action::PictureLoaded {
        id,
        size: Some((320, 200)),
    });
    assert_eq!(harness.placements().len(), 1);
    // Help sits in the middle of the screen, clear of the reader's
    // picture, at this size; at 100 columns it covers it.
    harness.resize(100, 30);
    assert_eq!(harness.placements().len(), 1);
    harness.keys("?");
    assert!(harness.placements().is_empty());
    harness.keys("?");
    assert_eq!(harness.placements().len(), 1);
    // A reader too short for a picture shows only text.
    harness.resize(120, 10);
    assert!(harness.placements().is_empty());
    harness.resize(200, 60);
    assert_eq!(harness.placements().len(), 1);
    // Another message: this picture is not its picture.
    harness.keys("esc j");
    assert!(harness.placements().is_empty());
}

#[test]
fn the_picture_takes_room_from_the_text_and_the_scroll_knows() {
    let mut harness = reading_with_pictures(80, 24, "Photos from Sat");
    let lines = harness.state.reader.lines.len();
    let before = harness.state.reader.scroll;
    assert_eq!(before, 0);
    let (id, _, _) = picture_asked_for(&harness);
    harness.keys("G");
    let end_without = harness.state.reader.scroll;
    harness.act(Action::PictureLoaded {
        id,
        size: Some((320, 200)),
    });
    let image = harness.placements()[0];
    // Never more than half the pane.
    assert!(image.area.height <= 8, "{:?}", image.area);
    // With fewer rows for the text there is further to scroll, and the end
    // of the message can still be reached.
    harness.keys("G");
    assert!(
        harness.state.reader.scroll >= end_without,
        "{lines} lines: {} then {}",
        end_without,
        harness.state.reader.scroll
    );
}
