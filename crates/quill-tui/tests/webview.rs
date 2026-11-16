//! A message as a web page in the browser: sanitized, with remote content
//! blocked until the user says otherwise.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::{Harness, NOW};
use quill_core::types::{
    Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_tui::data::DataEvent;
use quill_tui::state::{Action, HostRequest};

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

/// The newsletter of the demo mailbox, open in the reader. It is HTML and
/// carries a one-pixel picture that would tell its sender it was opened.
fn newsletter() -> Harness {
    let mut harness = Harness::demo(200, 60);
    harness.keys("G k k enter");
    harness
}

/// The page last handed to the browser: where it is and whether it may
/// load remote content. One that may is written to a folder of its own.
fn page(harness: &Harness) -> (PathBuf, bool) {
    match harness.host_requests.last() {
        Some(HostRequest::OpenFile(path)) => {
            (path.clone(), path.to_string_lossy().contains("page-remote"))
        }
        other => panic!("no page was shown: {other:?}"),
    }
}

/// Puts a message of our own at the top of the Work inbox and opens it.
fn reading_raw(raw: &str, subject: &str) -> Harness {
    let mut harness = Harness::demo(200, 60);
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("web-1".to_owned());
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
                        from: Vec::new(),
                        to: Vec::new(),
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

#[test]
fn the_page_is_shown_with_remote_content_left_out_and_says_so() {
    let mut harness = newsletter();
    harness.keys("B");
    let (path, remote) = page(&harness);
    assert!(!remote);
    let html = std::fs::read_to_string(&path).unwrap();
    // The picture that reports the opening is not in the page at all, and
    // the page is told it may fetch nothing.
    assert!(!html.contains("open.gif"), "{html}");
    assert!(html.contains("Content-Security-Policy"), "{html}");
    assert!(!html.to_lowercase().contains("<script"));
    assert!(
        status(&harness)
            .contains("1 remote picture was left out. i loads them, I always for this sender."),
        "{}",
        status(&harness)
    );
    // The page is private to the user, in a folder of its own.
    use std::os::unix::fs::PermissionsExt;
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn remote_content_is_loaded_for_this_message_when_asked() {
    let mut harness = newsletter();
    harness.keys("B i");
    let (path, remote) = page(&harness);
    assert!(remote);
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(html.contains("https://example.com/open.gif"), "{html}");
    // Still no scripts, whatever else is let in.
    assert!(!html.to_lowercase().contains("<script"));
    assert!(!status(&harness).contains("left out"));

    // It was for that message only: nothing was remembered about its
    // sender, and another message starts blocked again.
    let sender = harness.state.reader.row.as_ref().unwrap().headers.from[0]
        .email
        .clone();
    assert!(!harness.store.allows_remote_content(&sender).unwrap());
    harness.keys("esc k enter B");
    harness.keys("esc j enter B");
    assert!(!page(&harness).1);
}

#[test]
fn a_sender_can_be_trusted_with_remote_content_from_now_on() {
    let mut harness = newsletter();
    let sender = harness.state.reader.row.as_ref().unwrap().headers.from[0]
        .email
        .clone();
    harness.keys("I");
    assert!(harness.store.allows_remote_content(&sender).unwrap());
    assert!(page(&harness).1);
    assert!(
        status(&harness).contains(&format!(
            "Mail from {sender} loads remote pictures from now on."
        )),
        "{}",
        status(&harness)
    );
    // Coming back to the message later, it loads without being asked.
    harness.keys("esc k enter esc j enter B");
    assert!(page(&harness).1);
}

#[test]
fn with_remote_content_set_to_never_nothing_loads_it() {
    let mut harness = newsletter();
    harness.env.remote_never = true;
    harness.keys("B");
    assert!(!page(&harness).1);
    let said = status(&harness);
    assert!(said.contains("1 remote picture was left out."), "{said}");
    assert!(!said.contains("loads them"), "{said}");
    let shown = harness.host_requests.len();
    harness.keys("i I");
    assert_eq!(harness.host_requests.len(), shown);
    let sender = harness.state.reader.row.as_ref().unwrap().headers.from[0]
        .email
        .clone();
    assert!(!harness.store.allows_remote_content(&sender).unwrap());
}

#[test]
fn the_browser_gets_the_same_safe_page() {
    let mut harness = newsletter();
    harness.keys("B");
    let Some(HostRequest::OpenFile(path)) = harness.host_requests.last() else {
        panic!("nothing was opened: {:?}", harness.host_requests.last());
    };
    let html = std::fs::read_to_string(path).unwrap();
    assert!(!html.contains("open.gif") && html.contains("Content-Security-Policy"));
    // Loading the remote content shows the page there again, with it.
    harness.keys("i");
    assert!(matches!(
        harness.host_requests.last(),
        Some(HostRequest::OpenFile(path)) if path.to_string_lossy().contains("page-remote")
    ));
}

const WITH_PICTURES: &str = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
To: Robin Vale <robin@example.com>\r\n\
Subject: The chart\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Here it is.</p>\
<img src=\"cid:chart@example.com\" alt=\"Chart\">\
<img src=\"https://example.com/logo.png\" alt=\"Our logo\">\
<img src=\"cid:../../etc/passwd\" alt=\"Nope\">\
<script>alert(1)</script></body></html>\r\n\
--b1\r\n\
Content-Type: image/png\r\n\
Content-ID: <chart@example.com>\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: inline; filename=\"../../../evil name.png\"\r\n\
\r\n\
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\r\n\
--b1--\r\n";

#[test]
fn pictures_the_message_carries_are_written_beside_the_page_under_safe_names() {
    let mut harness = reading_raw(WITH_PICTURES, "The chart");
    harness.keys("B");
    let (path, remote) = page(&harness);
    assert!(!remote);
    let folder = path.parent().unwrap();
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    // The page and the one picture it has, named here and not by the
    // message.
    assert_eq!(names, ["message.html", "picture-1.png"]);
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(html.contains("picture-1.png"), "{html}");
    assert!(!html.contains("cid:") && !html.contains("passwd"), "{html}");
    // The remote one is named in words where it would have been.
    assert!(
        html.contains("Our logo") && !html.contains("logo.png"),
        "{html}"
    );
    assert!(!html.contains("alert(1)"));
    let picture = std::fs::read(folder.join("picture-1.png")).unwrap();
    assert!(picture.starts_with(b"\x89PNG"));
}

#[test]
fn a_message_with_no_html_is_a_page_of_its_text() {
    let mut harness = reading_raw(
        "From: Ana Ruiz <ana.ruiz@example.com>\r\nSubject: Plain\r\n\r\nOne < two & three.\r\n",
        "Plain",
    );
    harness.keys("B");
    let html = std::fs::read_to_string(page(&harness).0).unwrap();
    assert!(html.contains("One &lt; two &amp; three."), "{html}");
    assert!(!status(&harness).contains("left out"));
}

#[test]
fn a_page_is_for_the_message_being_read() {
    let mut harness = Harness::demo(200, 60);
    // From the sidebar there is no message to make a page of.
    harness.keys("1 B i I");
    assert!(harness.host_requests.is_empty());
}

#[test]
fn remote_content_asked_for_from_the_reader_is_shown_in_the_browser() {
    // The reader fetches nothing, so there is nowhere here to show it.
    let mut harness = newsletter();
    harness.keys("i");
    let (path, remote) = page(&harness);
    assert!(remote);
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(html.contains("https://example.com/open.gif"), "{html}");
}

#[test]
fn there_is_one_command_for_the_browser_and_none_for_a_page_beside_the_reader() {
    use quill_core::config::KeymapPreset;
    use quill_tui::command::Registry;
    use quill_tui::keymap::Keymap;

    let registry = Registry::builtin();
    assert!(registry.by_name("message.view_html").is_none());
    assert!(registry.by_name("message.open_in_browser").is_some());
    // The keys it had are nobody's, in either set.
    for (preset, key) in [(KeymapPreset::Vim, "v"), (KeymapPreset::Mutt, "H")] {
        let keymap = Keymap::preset(preset);
        let bound = registry.specs().iter().any(|spec| {
            keymap
                .keys_for(spec.id)
                .iter()
                .any(|sequence| sequence.to_string() == key)
        });
        assert!(!bound, "{key} is still bound in {preset:?}");
    }
    // And pressing it where it used to work does nothing.
    let mut harness = newsletter();
    harness.keys("v");
    assert!(harness.host_requests.is_empty());
    assert!(!harness.state.web_reading);
}
