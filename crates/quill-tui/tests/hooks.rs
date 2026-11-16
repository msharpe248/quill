//! Hooks: giving the message on show to one of the user's own programs.
//!
//! The state machine is driven through the harness; where a program is
//! run, it is a small `/bin/sh` script written into a temporary folder.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use common::Harness;
use quill_core::config::KeymapPreset;
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataRequest;
use quill_tui::hook::{Hook, HookEnd, HookInput, HookOutcome, Limits};
use quill_tui::keymap::Keymap;
use quill_tui::state::{Action, HostRequest, NoticeKind};
use quill_tui::theme::ColorMode;

fn hook(name: &str, command: &[&str], input: HookInput) -> Hook {
    Hook {
        name: name.to_owned(),
        command: command.iter().map(|part| (*part).to_owned()).collect(),
        input,
    }
}

/// The demo mailbox with three hooks set up.
fn with_hooks(width: u16, height: u16) -> Harness {
    let mut harness = Harness::demo(width, height);
    harness.env.hooks = vec![
        hook(
            "Save to notes",
            &["/usr/local/bin/save-note", "--from-mail"],
            HookInput::Raw,
        ),
        hook("Word count", &["/usr/bin/wc", "-w"], HookInput::Text),
        hook("Print", &["/usr/bin/lp"], HookInput::Text),
    ];
    harness
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

fn runs_asked(harness: &Harness) -> usize {
    harness
        .host_requests
        .iter()
        .filter(|request| matches!(request, HostRequest::RunHook(_)))
        .count()
}

/// Writes an executable `/bin/sh` script and returns where it is.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// Does what the run loop does with a hook the UI asked for: runs it,
/// and tells the UI how it went.
fn run_asked(harness: &mut Harness) -> HookOutcome {
    let run = harness.hook_asked().expect("no hook was asked for");
    let outcome = quill_tui::hook::run(&run, Limits::default(), &AtomicBool::new(false));
    harness.act(Action::HookDone {
        name: run.name,
        outcome: outcome.clone(),
    });
    outcome
}

#[test]
fn the_key_opens_a_list_that_names_each_program() {
    for preset in [KeymapPreset::Vim, KeymapPreset::Mutt] {
        let mut harness = Harness::build(120, 30, Keymap::preset(preset), ColorMode::TrueColor);
        harness.env.hooks = with_hooks(120, 30).env.hooks;
        assert!(
            harness
                .state
                .is_enabled(&harness.env, CommandId::MessageHook)
        );
        harness.keys("|");
        assert_eq!(harness.state.context(), Context::Hooks, "{preset:?}");
        let screen = harness.screen();
        assert!(
            screen.contains("Give this message to a program"),
            "{screen}"
        );
        // What will run is said beside each name, arguments and all.
        assert!(
            harness
                .row_with("> Save to notes")
                .contains("/usr/local/bin/save-note --from-mail"),
            "{screen}"
        );
        assert!(harness.row_with("Word count").contains("/usr/bin/wc -w"));
        assert!(status(&harness).contains("run it"), "{}", status(&harness));
        // Opening the list ran nothing and read nothing.
        assert_eq!(runs_asked(&harness), 0);

        // Esc closes it, and still nothing has run.
        harness.keys("escape");
        assert_eq!(harness.state.context(), Context::List);
        assert_eq!(runs_asked(&harness), 0);
    }
}

#[test]
fn typing_narrows_the_list_and_the_arrows_choose() {
    let mut harness = with_hooks(120, 30);
    harness.keys("|");
    // Letters are typed, not run as the commands they are in the list.
    harness.type_text("d");
    assert_eq!(harness.state.context(), Context::Hooks);
    let screen = harness.screen();
    assert!(screen.contains("Word count"), "{screen}");
    assert!(!screen.contains("Save to notes"), "{screen}");
    harness.type_text("zzz");
    assert!(harness.screen().contains("No hook matches."));
    assert!(!harness.state.is_enabled(&harness.env, CommandId::NavOpen));
    harness.keys("enter");
    assert_eq!(runs_asked(&harness), 0);
    harness.keys("backspace backspace backspace backspace down down down up");
    // Down as far as it goes, and one back up.
    assert!(
        harness.screen().contains("> Word count  /usr/bin/wc -w"),
        "{}",
        harness.screen()
    );
    harness.act(Action::Paste("print".to_owned()));
    assert!(
        harness.screen().contains("> Print  /usr/bin/lp"),
        "{}",
        harness.screen()
    );
}

#[test]
fn choosing_one_gives_it_the_message_on_show_as_it_arrived() {
    let mut harness = with_hooks(120, 30);
    // The message under the cursor is the one in the reader beside it.
    let row = harness.state.list.rows[harness.state.list.selected].clone();
    harness.keys("| enter");
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        status(&harness).contains("Running Save to notes…"),
        "{}",
        status(&harness)
    );
    assert_eq!(runs_asked(&harness), 1);
    let run = harness.hook_asked().unwrap();
    assert_eq!(run.name, "Save to notes");
    assert_eq!(run.command, ["/usr/local/bin/save-note", "--from-mail"]);
    // Byte for byte what the store holds for that message.
    assert_eq!(Some(run.input), harness.store.raw(row.id).unwrap());
    assert_eq!(
        run.about,
        [
            ("QUILL_SUBJECT", row.headers.subject.clone()),
            ("QUILL_FROM", row.headers.from[0].email.clone()),
            (
                "QUILL_MESSAGE_ID",
                row.headers.message_id.clone().unwrap_or_default()
            ),
        ]
    );
}

#[test]
fn a_text_hook_gets_the_headers_and_the_plain_text() {
    let mut harness = with_hooks(120, 30);
    let row = harness.state.list.rows[harness.state.list.selected].clone();
    harness.keys("| down enter");
    let run = harness.hook_asked().unwrap();
    assert_eq!(run.name, "Word count");
    let text = String::from_utf8(run.input).unwrap();
    let (head, body) = text.split_once("\n\n").unwrap();
    assert!(
        head.starts_with("From: Ana Ruiz <ana.ruiz@example.com>\n"),
        "{head}"
    );
    assert!(
        head.contains(&format!("\nSubject: {}", row.headers.subject)),
        "{head}"
    );
    assert_eq!(
        body.trim_end(),
        harness.store.body(row.id).unwrap().unwrap().trim_end()
    );
}

#[test]
fn from_the_reader_it_is_the_message_being_read() {
    let mut harness = with_hooks(120, 30);
    harness.keys("j j enter");
    assert_eq!(harness.state.context(), Context::Reader);
    let reading = harness.state.reader.row.clone().unwrap();
    harness.keys("| enter");
    let run = harness.hook_asked().unwrap();
    assert_eq!(run.about[0].1, reading.headers.subject);
    assert_eq!(Some(run.input), harness.store.raw(reading.id).unwrap());
}

/// Puts a message at the top of the inbox as a server first lists it:
/// its headers, and with `body` its text, but never the message itself.
fn list_only(harness: &mut Harness, subject: &str, body: Option<&str>) {
    use quill_core::types::{
        Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
    };
    let folder = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let id = RemoteId(format!("added-{subject}"));
    let delta = SyncDelta {
        added: vec![RemoteMessage {
            id: id.clone(),
            headers: Headers {
                message_id: None,
                subject: subject.to_owned(),
                from: vec![Address::new("Mallory \u{1b}[5mVane", "mallory@example.com")],
                to: vec![Address::new("Robin Vale", "robin@example.com")],
                cc: Vec::new(),
                date: common::NOW - 30,
            },
            flags: Flags::default(),
            size: 0,
            has_attachments: false,
            thread: Vec::new(),
        }],
        changed: Vec::new(),
        removed: Vec::new(),
        reset: false,
        has_more: false,
        next_cursor: SyncCursor(format!("after-{subject}")),
    };
    harness.store.apply_delta(folder, &delta).unwrap();
    if let Some(body) = body {
        let local = harness
            .store
            .local_message_id(folder, &id)
            .unwrap()
            .unwrap();
        harness.store.put_body(local, body).unwrap();
    }
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    harness.keys("g g");
    assert_eq!(harness.selected_subject(), subject);
}

#[test]
fn a_message_that_is_not_downloaded_says_so_and_runs_nothing() {
    let mut harness = with_hooks(140, 30);
    // Only its text is here: there is no message as it arrived to give.
    list_only(&mut harness, "Old news", Some("The text that was kept."));
    harness.keys("| enter");
    assert_eq!(runs_asked(&harness), 0);
    assert!(
        status(&harness).contains("has not been downloaded yet"),
        "{}",
        status(&harness)
    );
    // The next hook is not held up by it, and the text form can be had:
    // with a sender's name made one clean line.
    harness.keys("| down enter");
    assert_eq!(runs_asked(&harness), 1);
    let run = harness.hook_asked().unwrap();
    let text = String::from_utf8(run.input).unwrap();
    assert!(
        text.starts_with("From: Mallory Vane <mallory@example.com>\n"),
        "{text:?}"
    );
    assert!(text.ends_with("\n\nThe text that was kept.\n"), "{text:?}");
    assert!(!text.contains('\u{1b}'));

    // With not even its text, there is nothing to give in either form.
    let mut harness = with_hooks(140, 30);
    list_only(&mut harness, "Just listed", None);
    for keys in ["| enter", "| down enter"] {
        harness.keys(keys);
        assert_eq!(runs_asked(&harness), 0);
        assert!(
            status(&harness).contains("has not been downloaded yet"),
            "{}",
            status(&harness)
        );
    }
}

#[test]
fn a_message_that_has_gone_says_so() {
    let mut harness = with_hooks(120, 30);
    harness.pump(vec![DataRequest::ReadForHook {
        message: quill_core::types::LocalMessageId(999_999),
        hook: Box::new(harness.env.hooks[0].clone()),
    }]);
    assert_eq!(runs_asked(&harness), 0);
    assert!(
        status(&harness).contains("no longer here"),
        "{}",
        status(&harness)
    );
}

#[test]
fn how_it_went_is_said_and_a_failure_stays_until_dismissed() {
    let mut harness = with_hooks(140, 30);
    harness.keys("| enter");
    harness.act(Action::HookDone {
        name: "Save to notes".to_owned(),
        outcome: HookOutcome {
            end: HookEnd::Exited(0),
            stdout: b"Saved as \x1b[1mQ4 roadmap\x1b[0m\nsecond line\n".to_vec(),
            stderr: Vec::new(),
        },
    });
    // The first line of what it printed, with nothing in it that a
    // terminal would act on.
    assert!(
        status(&harness).contains("Save to notes: Saved as Q4 roadmap"),
        "{}",
        status(&harness)
    );
    assert!(harness.state.notices.is_empty());

    harness.keys("| enter");
    harness.act(Action::HookDone {
        name: "Save to notes".to_owned(),
        outcome: HookOutcome {
            end: HookEnd::Exited(2),
            stdout: Vec::new(),
            stderr: b"no such notebook\n".to_vec(),
        },
    });
    let notice = harness.state.notices.front().unwrap();
    assert_eq!(notice.kind, NoticeKind::Error);
    assert_eq!(
        notice.text,
        "Save to notes failed (exit status 2): no such notebook"
    );
    assert!(status(&harness).contains("failed (exit status 2)"));

    harness.keys("escape | enter");
    harness.act(Action::HookDone {
        name: "Save to notes".to_owned(),
        outcome: HookOutcome {
            end: HookEnd::TimedOut,
            stdout: Vec::new(),
            stderr: Vec::new(),
        },
    });
    assert_eq!(
        harness.state.notices.front().unwrap().text,
        "Save to notes took too long and was stopped."
    );
}

#[test]
fn one_hook_runs_at_a_time() {
    let mut harness = with_hooks(120, 30);
    harness.keys("| enter");
    assert_eq!(runs_asked(&harness), 1);
    // Still running: a second is not started, and says why.
    harness.keys("| down enter");
    assert_eq!(runs_asked(&harness), 1);
    assert!(
        status(&harness).contains("Save to notes is still running."),
        "{}",
        status(&harness)
    );
    harness.act(Action::HookDone {
        name: "Save to notes".to_owned(),
        outcome: HookOutcome {
            end: HookEnd::Exited(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        },
    });
    assert!(status(&harness).contains("Save to notes is done."));
    harness.keys("| down enter");
    assert_eq!(runs_asked(&harness), 2);
}

#[test]
fn in_quills_own_app_hooks_are_off_and_the_list_never_opens() {
    let mut harness = with_hooks(120, 30);
    // As main.rs sets it when the host is the app.
    harness.env.hooks_enabled = false;
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageHook)
    );
    // The key, and the command arriving any other way (the palette, a
    // menu item that should have been greyed out).
    harness.keys("|");
    harness.act(Action::Command(CommandId::MessageHook));
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.hooks.is_none());
    assert_eq!(runs_asked(&harness), 0);
    // The app's menus are told it is not on offer.
    let enabled = harness.state.host_state(&harness.env).enabled;
    assert!(!enabled.contains(&CommandId::MessageHook));

    // Even a message read for a hook, arriving from the store, is not
    // turned into a program to run.
    let message = harness.state.list.rows[0].id;
    harness.pump(vec![DataRequest::ReadForHook {
        message,
        hook: Box::new(harness.env.hooks[0].clone()),
    }]);
    assert_eq!(runs_asked(&harness), 0);
    // And what the run loop says, should it ever be asked there.
    assert_eq!(
        HookOutcome::not_allowed().said("Save to notes"),
        "Hooks run only when Quill is in a terminal: the app starts no other programs."
    );
}

#[test]
fn with_no_hooks_or_no_message_there_is_nothing_to_run() {
    let mut harness = Harness::demo(120, 30);
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageHook)
    );
    harness.keys("|");
    assert_eq!(harness.state.context(), Context::List);

    // A draft is not mail to hand to a program.
    let mut harness = with_hooks(160, 30);
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    harness.keys("escape escape");
    harness.keys("1 g g j enter");
    assert!(
        harness.screen().contains("[2] Drafts"),
        "{}",
        harness.screen()
    );
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::MessageHook)
    );
    harness.keys("|");
    assert_eq!(harness.state.context(), Context::List);
    // Nor is the sidebar a message.
    let mut harness = with_hooks(120, 30);
    harness.keys("1 |");
    assert!(harness.state.hooks.is_none());
}

#[test]
fn nothing_about_hooks_happens_without_the_key() {
    let mut harness = with_hooks(120, 30);
    // Moving about, opening mail, mail arriving: none of it runs one.
    harness.keys("j k enter j escape 1 j enter 2");
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    assert_eq!(runs_asked(&harness), 0);
    assert!(harness.hook_asked().is_none());
}

#[test]
fn a_real_program_gets_the_message_and_its_answer_is_shown() {
    let scratch = tempfile::tempdir().unwrap();
    let copy = scratch.path().join("copy.eml");
    let program = script(
        scratch.path(),
        "save",
        "cat > \"$1\"\necho \"saved $QUILL_SUBJECT from $QUILL_FROM\"",
    );
    let mut harness = Harness::demo(160, 30);
    harness.env.hooks = vec![hook(
        "Save a copy",
        &[program.to_str().unwrap(), copy.to_str().unwrap()],
        HookInput::Raw,
    )];
    let row = harness.state.list.rows[harness.state.list.selected].clone();
    harness.keys("| enter");
    let outcome = run_asked(&mut harness);
    assert_eq!(outcome.end, HookEnd::Exited(0), "{outcome:?}");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        harness.store.raw(row.id).unwrap().unwrap()
    );
    assert!(
        status(&harness).contains(&format!(
            "Save a copy: saved {} from {}",
            row.headers.subject, row.headers.from[0].email
        )),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_real_program_that_fails_is_reported() {
    let scratch = tempfile::tempdir().unwrap();
    let program = script(
        scratch.path(),
        "refuse",
        "cat > /dev/null\necho 'the notebook is locked' >&2\nexit 4",
    );
    let mut harness = Harness::demo(160, 30);
    harness.env.hooks = vec![hook(
        "File it",
        &[program.to_str().unwrap()],
        HookInput::Text,
    )];
    harness.keys("| enter");
    let outcome = run_asked(&mut harness);
    assert_eq!(outcome.end, HookEnd::Exited(4));
    assert_eq!(
        harness.state.notices.front().unwrap().text,
        "File it failed (exit status 4): the notebook is locked"
    );
}

#[test]
fn the_bar_offers_hooks_only_where_there_are_some_to_run() {
    let bar = |hooks: bool, enabled: bool| {
        let mut harness = Harness::demo(240, 30);
        if hooks {
            harness.env.hooks = with_hooks(240, 30).env.hooks;
        }
        harness.env.hooks_enabled = enabled;
        status(&harness)
    };
    assert!(bar(true, true).contains("| hooks"), "{}", bar(true, true));
    assert!(!bar(false, true).contains("hooks"));
    // Not in Quill's own app, whatever the file lists.
    assert!(!bar(true, false).contains("hooks"));
}
