//! Writing a message in Vim: the message goes out as a draft file, Vim is
//! asked to run on it, and what comes back is sent, saved, thrown away or
//! left open, as Vim said.
//!
//! Most of these stand in for Vim by editing the draft file themselves.
//! Two at the end run the real plugin under the Vim on this machine, and
//! pass with a note when there is none.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::Harness;
use quill_store::{DraftState, Outgoing};
use quill_tui::command::{CommandId, Context};
use quill_tui::compose::{Away, Field};
use quill_tui::editor::Editor;
use quill_tui::setup::{AccountRequest, Preference};
use quill_tui::state::{Action, HostRequest, Mouse, MouseKind};
use quill_tui::vim::ExternalEditor;

/// Wide enough that nothing in the compose pane is cut short.
fn wide() -> Harness {
    Harness::demo(160, 30)
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
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

/// Writes to Priya and hands the message to Vim. Returns the draft file
/// and its folder.
fn in_vim(harness: &mut Harness) -> (PathBuf, PathBuf) {
    write_to_priya(harness);
    harness.keys("ctrl-v");
    harness.in_vim().expect("Vim was not asked for")
}

fn away(harness: &Harness) -> Option<Away> {
    harness
        .state
        .compose
        .as_ref()
        .and_then(|compose| compose.away)
}

/// What Vim would leave: the draft file rewritten, and a verdict or none.
fn vim_writes(path: &Path, dir: &Path, text: &str, verdict: Option<&str>) {
    std::fs::write(path, text).unwrap();
    if let Some(verdict) = verdict {
        std::fs::write(dir.join("verdict"), format!("{verdict}\n")).unwrap();
    }
}

const REWRITTEN: &str = "From: Work <robin@example.com>\n\
                         To: Priya Nair <priya.nair@example.com>\n\
                         Cc: sam.lee@example.com\n\
                         Bcc:\n\
                         Subject: Lunch on Friday\n\
                         \n\
                         Friday instead?\n";

/// Takes the one message waiting in the outbox as the engine would, and
/// returns it with its finished text.
fn outgoing(harness: &mut Harness) -> (Outgoing, String) {
    let outbox = harness.store.outbox().unwrap();
    assert_eq!(outbox.len(), 1, "{outbox:?}");
    assert_eq!(outbox[0].state, DraftState::Outbox);
    let claimed = harness
        .store
        .claim_send(outbox[0].account, i64::MAX / 2)
        .unwrap()
        .unwrap();
    let raw = harness.store.blob(&claimed.raw_hash).unwrap().unwrap();
    (claimed, String::from_utf8(raw).unwrap())
}

#[test]
fn the_message_goes_out_as_a_draft_file_and_vim_is_asked_to_run_on_it() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    assert_eq!(away(&harness), Some(Away::InVim));

    // One request, naming the file, its folder, and the message.
    let asked: Vec<&HostRequest> = harness
        .host_requests
        .iter()
        .filter(|request| matches!(request, HostRequest::EditDraft { .. }))
        .collect();
    assert_eq!(
        asked,
        [&HostRequest::EditDraft {
            path: path.clone(),
            dir: dir.clone(),
            title: "Lunch".to_owned(),
            editor: ExternalEditor::Vim,
            body_line: 7,
        }]
    );
    assert_eq!(path, dir.join("message.quill"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "From: Work <robin@example.com>\n\
         To: Priya Nair <priya.nair@example.com>,\n\
         Cc:\n\
         Bcc:\n\
         Subject: Lunch\n\
         \n\
         Are you free on *Thursday*?\n"
    );
    // In a terminal the user's own Vim is run: it is given the plugin,
    // and the people it can complete addresses from.
    assert!(dir.join("quill.vim").is_file());
    let contacts = std::fs::read_to_string(dir.join("contacts")).unwrap();
    assert!(
        contacts.contains("Priya Nair <priya.nair@example.com>\n"),
        "{contacts}"
    );
    // What had been written is a draft before Vim ever starts.
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.subject, "Lunch");

    // The pane says where the message is and how it comes back.
    let screen = harness.screen();
    assert!(
        screen.contains("This message is being written in Vim."),
        "{screen}"
    );
    for command in [
        ":QuillSend",
        ":QuillPostpone",
        ":QuillDiscard",
        ":wq",
        ":cq",
    ] {
        assert!(screen.contains(command), "{command}: {screen}");
    }
    // The fields, stale now, are not drawn, and there is no cursor.
    assert!(!screen.contains("Subject:"), "{screen}");
    assert_eq!(harness.cursor(), None);
}

#[test]
fn quills_own_app_has_the_plugin_already() {
    let mut harness = wide();
    harness.env.editor_pane = true;
    let (path, dir) = in_vim(&mut harness);
    assert!(path.is_file());
    assert!(!dir.join("quill.vim").exists());
}

#[test]
fn quills_own_app_is_asked_for_the_editor_the_config_names() {
    for editor in [
        ExternalEditor::Vim,
        ExternalEditor::Nano,
        ExternalEditor::Pico,
        ExternalEditor::Mg,
        ExternalEditor::Script,
    ] {
        let mut harness = wide();
        harness.env.editor_pane = true;
        harness.env.external = editor;
        let (path, dir) = in_vim(&mut harness);
        let asked: Vec<&HostRequest> = harness
            .host_requests
            .iter()
            .filter(|request| matches!(request, HostRequest::EditDraft { .. }))
            .collect();
        assert_eq!(
            asked,
            [&HostRequest::EditDraft {
                path,
                dir: dir.clone(),
                title: "Lunch".to_owned(),
                editor,
                // The file is the text alone: it starts on its first line.
                body_line: 1,
            }]
        );
        // The Vim the app starts itself finds the plugin in the app. The
        // user's own, run by their script, is given a copy beside the
        // draft, as in a terminal. No other editor has a use for one.
        assert_eq!(
            dir.join("quill.vim").exists(),
            editor == ExternalEditor::Script
        );
        // Above the editor's place, the pane says how the text comes
        // back, in that editor's own terms.
        let screen = harness.screen();
        assert!(screen.contains("Quill's own editor"), "{screen}");
        assert_eq!(screen.contains(":QuillSend"), editor.is_vim(), "{screen}");
        assert_eq!(
            screen.contains("ctrl-x"),
            !editor.is_vim(),
            "{editor:?}: {screen}"
        );
        assert!(
            status(&harness).contains(editor.close_hint()),
            "{}",
            status(&harness)
        );
        // Only a Vim can be asked from here to send or save: it has
        // Quill's commands in it.
        for id in [CommandId::ComposeSend, CommandId::ComposeSave] {
            assert_eq!(
                harness.state.is_enabled(&harness.env, id),
                editor.is_vim(),
                "{editor:?} {id:?}"
            );
        }
        // And the user is told what to do about it when there is none to
        // run.
        harness.act(Action::EditorExited {
            ran: false,
            status: 0,
        });
        assert_eq!(away(&harness), None);
        assert!(!dir.exists());
        let screen = harness.screen();
        if editor == ExternalEditor::Script {
            assert!(screen.contains("editor-command.sh"), "{screen}");
            assert!(screen.contains("Show Editor Script Folder"), "{screen}");
        } else if editor == ExternalEditor::Mg {
            // The app cannot run this one at all, and says which it can.
            assert!(
                screen.contains("mg does not run inside Quill's app"),
                "{screen}"
            );
        } else {
            assert!(
                screen.contains(&format!(
                    "The {} that comes with macOS could not be started",
                    editor.name()
                )),
                "{screen}"
            );
        }
    }
}

#[test]
fn each_editor_is_one_the_app_can_run_and_a_program_a_terminal_can() {
    use quill_host::bridge::EditorKind;
    for (editor, in_the_app, program) in [
        (ExternalEditor::Vim, Some(EditorKind::Vim), "vim"),
        // On a Mac nano is Pico: one word for both in the app, and each
        // by its own name in a terminal, where nano may be the real one.
        (ExternalEditor::Nano, Some(EditorKind::Nano), "nano"),
        (ExternalEditor::Pico, Some(EditorKind::Nano), "pico"),
        // mg is a terminal's alone: the Mac's own does not survive the
        // app's sandbox.
        (ExternalEditor::Mg, None, "mg"),
        // The script is how the app reaches the user's own Vim. A
        // terminal needs none.
        (ExternalEditor::Script, Some(EditorKind::Script), "vim"),
    ] {
        assert_eq!(editor.in_the_app(), in_the_app);
        assert_eq!(editor.program(), program);
        // QUILL_VIM names another Vim, and only a Vim.
        let named = std::ffi::OsString::from("/opt/homebrew/bin/nvim");
        let chosen = quill_tui::vim::program_name(editor, Some(&named));
        if editor.is_vim() {
            assert_eq!(chosen, "/opt/homebrew/bin/nvim");
        } else {
            assert_eq!(chosen, program);
        }
        // What it is started with: the draft last, whole, and for the
        // editors that are not Vim one option and nothing else.
        let command = quill_tui::vim::command(
            editor,
            Path::new("/usr/bin/x"),
            Path::new("/d/it's 1/message.quill"),
            Path::new("/d/it's 1"),
            9,
        );
        let arguments: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(arguments.last().unwrap(), &"/d/it's 1/message.quill");
        match editor {
            ExternalEditor::Nano | ExternalEditor::Pico => {
                assert_eq!(arguments, ["-t", "+9", "/d/it's 1/message.quill"]);
            }
            ExternalEditor::Mg => {
                assert_eq!(arguments, ["-n", "+9", "/d/it's 1/message.quill"]);
            }
            ExternalEditor::Vim | ExternalEditor::Script => {
                assert_eq!(arguments[0], "-N");
                assert_eq!(arguments[arguments.len() - 2], "--");
            }
        }
    }
}

#[test]
fn nothing_typed_at_quill_reaches_a_message_that_is_in_vim() {
    let mut harness = wide();
    let (_, _) = in_vim(&mut harness);
    let before = harness.state.compose.as_ref().unwrap().content();
    // Letters, some of them commands elsewhere; editing keys; a paste; a
    // click in the pane; and the keys that move between fields.
    harness.type_text("jkdeq?z");
    harness.keys("enter backspace delete tab shift-tab up down");
    harness.act(Action::Paste("pasted".to_owned()));
    harness.act(Action::Mouse(quill_tui::state::Mouse {
        kind: quill_tui::state::MouseKind::Press,
        column: 120,
        row: 10,
    }));
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), Some(Away::InVim));
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.content(), before);
    assert_eq!(compose.field, quill_tui::compose::Field::Body);
    // Nor do the commands that change the message here apply.
    for id in [
        CommandId::ComposeVim,
        CommandId::ComposeAttach,
        CommandId::ComposeBcc,
        CommandId::ComposeSendLater,
        CommandId::ComposeTemplate,
        CommandId::EditSelectAll,
    ] {
        assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
    }
    harness.keys("ctrl-v ctrl-t ctrl-o ctrl-l cmd-a");
    assert_eq!(harness.state.compose.as_ref().unwrap().content(), before);
    assert!(harness.state.when.is_none());
    // Vim was asked for once.
    let asked = harness
        .host_requests
        .iter()
        .filter(|request| matches!(request, HostRequest::EditDraft { .. }))
        .count();
    assert_eq!(asked, 1);
}

#[test]
fn an_answer_about_an_address_that_arrives_late_offers_nobody() {
    let mut harness = wide();
    harness.keys("c");
    harness.type_text("pri");
    assert!(harness.state.compose.as_ref().unwrap().completion.is_some());
    harness.keys("ctrl-v");
    let (_, dir) = harness.in_vim().unwrap();
    assert!(harness.state.compose.as_ref().unwrap().completion.is_none());
    // The store's answer for the last letter typed comes after the
    // message has left: it is not put up over a field that is not there.
    let people = harness.store.contacts_matching("pri", 6).unwrap();
    assert!(!people.is_empty());
    harness.act(Action::Data(quill_tui::data::DataEvent::Contacts {
        typed: "pri".to_owned(),
        people,
    }));
    assert!(harness.state.compose.as_ref().unwrap().completion.is_none());
    // So when Vim gives up, escape closes the message, not a list.
    harness.vim_exits(1);
    assert!(!dir.exists());
    harness.keys("escape");
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn wq_brings_the_message_back_to_send_save_or_close() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    vim_writes(&path, &dir, REWRITTEN, None);
    harness.vim_exits(0);

    // Open, in the built-in editor, with what the file said.
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), None);
    let content = harness.state.compose.as_ref().unwrap().content();
    assert_eq!(content.to, "Priya Nair <priya.nair@example.com>");
    assert_eq!(content.cc, "sam.lee@example.com");
    assert_eq!(content.subject, "Lunch on Friday");
    assert_eq!(content.body, "Friday instead?\n");
    let screen = harness.screen();
    assert!(harness.row_with("Subject:").contains("Lunch on Friday"));
    assert!(screen.contains("Friday instead?"), "{screen}");
    assert!(status(&harness).contains("Back from Vim."), "{screen}");
    // The bar is the prompt: what can be done with it next.
    let bar = status(&harness);
    assert!(bar.contains("send") && bar.contains("save draft") && bar.contains("close"));
    // Nothing was sent, and the folder has gone.
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(!dir.exists());

    // It is a message like any other again: typed into, and sent.
    harness.type_text(" Noon?");
    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Subject: Lunch on Friday\r\n"), "{raw}");
    assert!(raw.contains("Friday instead?"), "{raw}");
    assert!(raw.contains("Noon?"), "{raw}");
}

#[test]
fn what_was_written_in_vim_can_be_undone_here() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    vim_writes(&path, &dir, REWRITTEN, None);
    harness.vim_exits(0);
    harness.keys("cmd-z");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().body.text(),
        "Are you free on *Thursday*?"
    );
}

#[test]
fn a_message_that_comes_back_unchanged_is_not_a_change() {
    let mut harness = wide();
    let (_, dir) = in_vim(&mut harness);
    let before = harness.state.compose.as_ref().unwrap().content();
    // `:wq` on the file as it was written out.
    harness.vim_exits(0);
    let compose = harness.state.compose.as_ref().unwrap();
    // The line break a file ends with is not text, nor is the gap after
    // a completed address a difference: nothing here has changed.
    assert_eq!(compose.content(), before);
    assert!(!compose.is_modified());
    assert!(!dir.exists());
    // So closing it saves nothing new.
    harness.keys("escape");
    assert_eq!(harness.state.context(), Context::List);
    assert!(!status(&harness).contains("Saved"), "{}", status(&harness));
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
}

#[test]
fn quill_send_sends_what_was_written() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    vim_writes(&path, &dir, REWRITTEN, Some("send"));
    harness.vim_exits(0);

    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.compose.is_none());
    let (sent, raw) = outgoing(&mut harness);
    assert!(raw.contains("Subject: Lunch on Friday\r\n"), "{raw}");
    assert!(raw.contains("Friday instead?"), "{raw}");
    assert!(raw.contains("Cc: <sam.lee@example.com>\r\n"), "{raw}");
    // The draft saved before Vim became the message: there is not a
    // second copy left behind.
    assert!(harness.store.drafts().unwrap().is_empty());
    assert_eq!(sent.to.len(), 2, "{:?}", sent.to);
    assert!(!dir.exists());
}

#[test]
fn a_send_from_vim_is_checked_like_any_other() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    // No subject: something to look at before it goes.
    vim_writes(
        &path,
        &dir,
        "To: priya.nair@example.com\nSubject:\n\nFriday instead?\n",
        Some("send"),
    );
    harness.vim_exits(0);
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), None);
    let screen = harness.screen();
    assert!(screen.contains("There is no subject."), "{screen}");
    assert!(harness.store.outbox().unwrap().is_empty());
    // Asked again, here, it goes as it is.
    harness.keys("cmd-enter");
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Friday instead?"), "{raw}");

    // And what cannot be sent at all says why and stays open.
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    vim_writes(
        &path,
        &dir,
        "To: priya\nSubject: Lunch\n\nFriday instead?\n",
        Some("send"),
    );
    harness.vim_exits(0);
    assert!(
        harness
            .screen()
            .contains("\"priya\" is not an email address"),
        "{}",
        harness.screen()
    );
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn quill_postpone_saves_the_draft_and_closes() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    vim_writes(&path, &dir, REWRITTEN, Some("postpone"));
    harness.vim_exits(0);

    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.compose.is_none());
    assert!(
        status(&harness).contains("Saved to Drafts."),
        "{}",
        status(&harness)
    );
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.subject, "Lunch on Friday");
    assert_eq!(drafts[0].content.body, "Friday instead?\n");
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(!dir.exists());
}

#[test]
fn postponing_a_message_left_as_it_was_still_says_it_is_saved() {
    let mut harness = wide();
    let (_, dir) = in_vim(&mut harness);
    std::fs::write(dir.join("verdict"), "postpone\n").unwrap();
    harness.vim_exits(0);
    assert!(harness.state.compose.is_none());
    assert!(
        status(&harness).contains("Saved to Drafts."),
        "{}",
        status(&harness)
    );
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
}

#[test]
fn quill_discard_throws_the_draft_away_and_closes() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    assert_eq!(harness.store.drafts().unwrap().len(), 1);
    // Whatever the file says, it is not kept, and nothing it names is
    // attached on the way out.
    vim_writes(
        &path,
        &dir,
        "To: priya.nair@example.com\nAttach: /nonexistent/quill-test/x.pdf\n\nNever mind.\n",
        Some("discard"),
    );
    harness.vim_exits(0);

    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.compose.is_none());
    assert!(harness.store.drafts().unwrap().is_empty());
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(
        status(&harness).contains("Thrown away."),
        "{}",
        status(&harness)
    );
    assert!(harness.state.notices.is_empty());
    assert!(!dir.exists());
}

#[test]
fn discarding_a_message_that_was_never_saved_leaves_nothing() {
    let mut harness = wide();
    // Nothing written yet: there is no draft to save before Vim.
    harness.keys("c ctrl-v");
    let (path, dir) = harness.in_vim().unwrap();
    assert!(harness.store.drafts().unwrap().is_empty());
    vim_writes(
        &path,
        &dir,
        "To: ana.ruiz@example.com\n\nHello.\n",
        Some("discard"),
    );
    harness.vim_exits(0);
    assert!(harness.state.compose.is_none());
    assert!(harness.store.drafts().unwrap().is_empty());
    assert!(status(&harness).contains("Thrown away."));
}

#[test]
fn cq_or_a_crash_leaves_the_message_as_it_was() {
    // `:cq` is status 1; a Vim that was killed has none of its own.
    for exit in [1, -1, 137] {
        let mut harness = wide();
        let (path, dir) = in_vim(&mut harness);
        let before = harness.state.compose.as_ref().unwrap().content();
        // Even written, and even with a verdict: an error means never mind.
        vim_writes(&path, &dir, REWRITTEN, Some("send"));
        harness.vim_exits(exit);

        assert_eq!(harness.state.context(), Context::Compose, "{exit}");
        assert_eq!(away(&harness), None);
        assert_eq!(harness.state.compose.as_ref().unwrap().content(), before);
        assert!(harness.row_with("Subject:").contains("Lunch"));
        assert!(
            status(&harness).contains("it is as it was"),
            "{}",
            status(&harness)
        );
        assert!(harness.store.outbox().unwrap().is_empty());
        assert_eq!(harness.store.drafts().unwrap().len(), 1);
        assert!(!dir.exists());
        // And it can be written here, or handed to Vim again.
        harness.type_text(" Or Friday?");
        assert!(harness.screen().contains("Or Friday?"));
        harness.keys("ctrl-v");
        assert_eq!(away(&harness), Some(Away::InVim));
        assert!(harness.in_vim().unwrap().0.is_file());
    }
}

#[test]
fn with_no_vim_to_start_the_message_stays_in_the_built_in_editor() {
    let mut harness = wide();
    let (_, dir) = in_vim(&mut harness);
    let before = harness.state.compose.as_ref().unwrap().content();
    harness.act(Action::EditorExited {
        ran: false,
        status: 0,
    });
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), None);
    assert_eq!(harness.state.compose.as_ref().unwrap().content(), before);
    let screen = harness.screen();
    assert!(screen.contains("Vim could not be started"), "{screen}");
    assert!(!dir.exists());
    // Said in the message's own pane: the next key clears it and is typed.
    harness.type_text("!");
    let screen = harness.screen();
    assert!(!screen.contains("Vim could not be started"), "{screen}");
    assert!(screen.contains("Thursday*?!"), "{screen}");
}

#[test]
fn a_draft_file_that_cannot_be_read_keeps_the_message_and_says_why() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let before = harness.state.compose.as_ref().unwrap().content();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(dir.join("verdict"), "send\n").unwrap();
    harness.vim_exits(0);

    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), None);
    assert_eq!(harness.state.compose.as_ref().unwrap().content(), before);
    let screen = harness.screen();
    assert!(
        screen.contains("What was written in Vim cannot be read"),
        "{screen}"
    );
    // The verdict went with it: a message that was not read is not sent.
    assert!(harness.store.outbox().unwrap().is_empty());
    assert!(!dir.exists());
}

#[test]
fn files_named_in_vim_are_attached_and_files_no_longer_named_are_removed() {
    let scratch = tempfile::tempdir().unwrap();
    let old = scratch.path().join("agenda.txt");
    std::fs::write(&old, "1. Lunch\n").unwrap();
    let new = scratch.path().join("new notes.txt");
    std::fs::write(&new, "Bring the notes.\n").unwrap();

    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-A");
    harness.act(Action::Paste(old.to_str().unwrap().to_owned()));
    harness.keys("enter ctrl-v");
    let (path, dir) = harness.in_vim().unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("\nAttach: agenda.txt\n"), "{written}");

    // The old file's line deleted; a new one added, as `:QuillAttach`
    // writes it: a whole path, spaces and all.
    let rewritten = written.replace(
        "Attach: agenda.txt\n",
        &format!("Attach: {}\n", new.display()),
    );
    vim_writes(&path, &dir, &rewritten, Some("send"));
    harness.vim_exits(0);

    assert!(harness.state.compose.is_none(), "{}", harness.screen());
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("new notes.txt"), "{raw}");
    assert!(!raw.contains("agenda.txt"), "{raw}");
}

#[test]
fn a_file_already_attached_stays_while_its_line_does() {
    let scratch = tempfile::tempdir().unwrap();
    let old = scratch.path().join("agenda.txt");
    std::fs::write(&old, "1. Lunch\n").unwrap();

    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("cmd-A");
    harness.act(Action::Paste(old.to_str().unwrap().to_owned()));
    harness.keys("enter ctrl-v");
    let (path, dir) = harness.in_vim().unwrap();
    // The file itself can be gone by now: what was attached is kept by
    // name, not read again.
    std::fs::remove_file(&old).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    vim_writes(&path, &dir, &written.replace("Thursday", "Friday"), None);
    harness.vim_exits(0);
    assert!(
        harness.row_with("Attach:").contains("agenda.txt (9 bytes)"),
        "{}",
        harness.screen()
    );
    assert!(harness.screen().contains("Friday"));
}

#[test]
fn a_file_that_cannot_be_attached_keeps_the_message_from_going() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let written = std::fs::read_to_string(&path).unwrap();
    for verdict in ["send", "postpone"] {
        vim_writes(
            &path,
            &dir,
            &written.replace(
                "Subject: Lunch\n",
                "Subject: Lunch\nAttach: /nonexistent/quill-test/agenda.pdf\n",
            ),
            Some(verdict),
        );
        harness.vim_exits(0);
        // Not sent or put away without the file it was meant to carry:
        // open, with the reason on show, as for a path typed here.
        assert_eq!(harness.state.context(), Context::Compose, "{verdict}");
        assert_eq!(away(&harness), None);
        let screen = harness.screen();
        assert!(screen.contains("agenda.pdf cannot be attached"), "{screen}");
        assert!(!screen.contains("Attach:"), "{screen}");
        assert!(harness.store.outbox().unwrap().is_empty());
        if verdict == "send" {
            // Back to Vim for the second way of leaving it.
            harness.keys("ctrl-v");
            let (again, folder) = harness.in_vim().unwrap();
            assert_eq!((again.clone(), folder.clone()), (path.clone(), dir.clone()));
        }
    }
}

#[test]
fn a_file_dropped_on_the_window_while_vim_runs_is_kept() {
    let scratch = tempfile::tempdir().unwrap();
    let dropped = scratch.path().join("photo.png");
    std::fs::write(&dropped, [0x89, b'P', b'N', b'G']).unwrap();

    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    harness.act(Action::Files {
        paths: vec![dropped.to_str().unwrap().to_owned()],
        temporary: false,
    });
    // The draft file knows nothing of it, and does not take it away.
    vim_writes(&path, &dir, REWRITTEN, None);
    harness.vim_exits(0);
    assert!(
        harness.row_with("Attach:").contains("photo.png"),
        "{}",
        harness.screen()
    );
    assert_eq!(
        harness.state.compose.as_ref().unwrap().content().subject,
        "Lunch on Friday"
    );
}

#[test]
fn send_and_save_from_outside_are_passed_on_to_the_running_vim() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    // The toolbar's Send, as the app delivers it, and the key.
    for (press, word) in [(Some(CommandId::ComposeSend), "send"), (None, "send")] {
        match press {
            Some(id) => harness.act(Action::Command(id)),
            None => harness.keys("ctrl-x"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("command")).unwrap(),
            format!("{word}\n")
        );
        std::fs::remove_file(dir.join("command")).unwrap();
    }
    harness.keys("ctrl-s");
    assert_eq!(
        std::fs::read_to_string(dir.join("command")).unwrap(),
        "postpone\n"
    );
    // Nothing was done with the message as it stood before Vim: it is
    // still Vim's, and nothing is in the outbox.
    assert_eq!(away(&harness), Some(Away::InVim));
    assert!(harness.store.outbox().unwrap().is_empty());
    let drafts = harness.store.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.subject, "Lunch");
    assert!(status(&harness).contains("Asked Vim to save it as a draft."));

    // Vim does as it was told and goes; then the rest happens.
    vim_writes(&path, &dir, REWRITTEN, Some("send"));
    harness.vim_exits(0);
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Subject: Lunch on Friday\r\n"), "{raw}");
}

#[test]
fn closing_while_vim_runs_asks_vim_to_go_and_waits() {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    harness.keys("escape");
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::ClosePane),
        "{:?}",
        harness.host_requests
    );
    // Still open, still Vim's: nothing is closed on what it was before.
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(away(&harness), Some(Away::InVim));
    assert!(dir.exists());

    // The pane saves and closes with no verdict: the message is back,
    // open, for the user to say what becomes of it.
    vim_writes(&path, &dir, REWRITTEN, None);
    harness.vim_exits(0);
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.row_with("Subject:").contains("Lunch on Friday"));
    // And now escape closes it, as a draft.
    harness.keys("escape");
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(
        harness.store.drafts().unwrap()[0].content.subject,
        "Lunch on Friday"
    );
}

#[test]
fn with_vim_as_the_editor_every_message_starts_there() {
    let vim = |harness: &mut Harness| {
        harness.env.compose.vim = true;
    };

    // A new message.
    let mut harness = wide();
    vim(&mut harness);
    harness.keys("c");
    assert_eq!(away(&harness), Some(Away::InVim));
    let (path, _) = harness.in_vim().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("From: Work <robin@example.com>\nTo:\n"),
        "{text}"
    );

    // A reply, with its quotation already in the file.
    let mut harness = wide();
    vim(&mut harness);
    harness.keys("r");
    assert_eq!(away(&harness), Some(Away::InVim));
    let (path, dir) = harness.in_vim().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\nSubject: Re: Q4 roadmap\n"), "{text}");
    assert!(text.contains("\n> "), "{text}");
    // Sent from Vim, it is still a reply to that message.
    vim_writes(
        &path,
        &dir,
        &text.replacen("\n\n", "\n\nLooks good.\n", 1),
        Some("send"),
    );
    harness.vim_exits(0);
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("In-Reply-To: "), "{raw}");
    assert!(raw.contains("Looks good."), "{raw}");

    // A forward.
    let mut harness = wide();
    vim(&mut harness);
    harness.keys("f");
    assert_eq!(away(&harness), Some(Away::InVim));
    let text = std::fs::read_to_string(harness.in_vim().unwrap().0).unwrap();
    assert!(text.contains("\nSubject: Fwd: Q4 roadmap\n"), "{text}");

    // A draft taken up again.
    let mut harness = wide();
    write_to_priya(&mut harness);
    harness.keys("escape");
    vim(&mut harness);
    harness.keys("1 g g j enter enter");
    assert_eq!(away(&harness), Some(Away::InVim));
    let text = std::fs::read_to_string(harness.in_vim().unwrap().0).unwrap();
    assert!(text.contains("\nSubject: Lunch\n"), "{text}");

    // A mail link.
    let mut harness = wide();
    vim(&mut harness);
    harness.act(Action::Mailto(
        "mailto:ana.ruiz@example.com?subject=Hello".to_owned(),
    ));
    assert_eq!(away(&harness), Some(Away::InVim));
    let text = std::fs::read_to_string(harness.in_vim().unwrap().0).unwrap();
    assert!(text.contains("\nTo: ana.ruiz@example.com\n"), "{text}");
    assert!(text.contains("\nSubject: Hello\n"), "{text}");
}

#[test]
fn with_the_built_in_editor_only_the_command_goes_to_vim() {
    let mut harness = wide();
    harness.keys("c");
    assert_eq!(away(&harness), None);
    assert!(harness.in_vim().is_none());
    assert!(status(&harness).contains("Vim"), "{}", status(&harness));
    // Both keys: the one a terminal passes on, and the app's.
    harness.keys("cmd-E");
    assert_eq!(away(&harness), Some(Away::InVim));
    // And a message that came back with nothing said does not go
    // straight out again, whatever the config: it is the prompt.
    let mut harness = wide();
    harness.env.compose.vim = true;
    harness.keys("c");
    let (path, dir) = harness.in_vim().unwrap();
    vim_writes(&path, &dir, "To: ana.ruiz@example.com\n\nHello.\n", None);
    harness.vim_exits(0);
    assert_eq!(away(&harness), None);
    assert_eq!(harness.state.context(), Context::Compose);
}

#[test]
fn an_answer_with_no_message_waiting_only_clears_up() {
    let mut harness = wide();
    // Out of nowhere: nothing changes and nothing is asked for.
    harness.vim_exits(0);
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.notices.is_empty());

    // The message went elsewhere while Vim ran (the app is the only
    // place another key can arrive): its folder is still cleared away.
    let (path, dir) = in_vim(&mut harness);
    vim_writes(&path, &dir, REWRITTEN, Some("send"));
    harness.state.compose = None;
    harness.vim_exits(0);
    assert!(!dir.exists());
    assert!(harness.store.outbox().unwrap().is_empty());
}

#[test]
fn a_message_in_vim_is_not_put_aside_for_another() {
    let mut harness = wide();
    let (_, dir) = in_vim(&mut harness);
    // A draft opened from somewhere else while Vim runs.
    let draft = harness.store.drafts().unwrap()[0].id;
    harness.pump(vec![quill_tui::data::DataRequest::OpenDraft(draft)]);
    assert_eq!(away(&harness), Some(Away::InVim));
    assert!(dir.exists());
    assert!(
        status(&harness).contains("A message is being written in Vim."),
        "{}",
        status(&harness)
    );
}

// --- The real plugin, under the Vim on this machine -----------------------

/// Quill's own app, where an editor runs in the text's place in the
/// message's pane.
fn in_the_app() -> Harness {
    let mut harness = wide();
    harness.env.editor_pane = true;
    harness
}

fn field(harness: &Harness) -> Field {
    harness.state.compose.as_ref().unwrap().field
}

fn tips_remembered(harness: &Harness) -> Vec<bool> {
    harness
        .account_requests
        .iter()
        .filter_map(|request| match request {
            AccountRequest::Remember(Preference::EditorTips(on)) => Some(*on),
            _ => None,
        })
        .collect()
}

#[test]
fn in_the_app_the_editor_has_the_text_alone_and_the_fields_stay_where_they_are() {
    let mut harness = in_the_app();
    let (path, dir) = in_vim(&mut harness);
    assert_eq!(away(&harness), Some(Away::InVim));
    // The file is the text and nothing else, and the folder says so.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "Are you free on *Thursday*?\n"
    );
    assert!(dir.join("body-only").exists());

    // The fields are still drawn, with what is in them, and nothing says
    // the message is somewhere else.
    let screen = harness.screen();
    assert!(harness.row_with("To:").contains("priya.nair@example.com"));
    assert!(harness.row_with("Subject:").contains("Lunch"));
    assert!(
        !screen.contains("This message is being written in Vim."),
        "{screen}"
    );
    // The text is not drawn here: its place is the editor's.
    assert!(!screen.contains("Are you free"), "{screen}");

    // The editor is given the text's place, and has the keys: the cursor
    // was in the text.
    let spot = harness.state.editor_spot(&harness.env);
    let place = spot.place.unwrap();
    assert!(spot.keys);
    assert_eq!(spot.covered, 0);
    let areas =
        quill_tui::layout::compute(&harness.state, harness.state.size, harness.env.reader_width);
    let pane = areas.reader.unwrap();
    // Inside the pane's border and padding, below the fields, to its foot.
    assert_eq!(place.x, pane.x + 2);
    assert_eq!(place.width, pane.width - 4);
    assert_eq!(place.y + place.height, pane.y + pane.height - 1);
    assert!(place.y > pane.y + 5 && place.height >= 3);
    // No cursor of Quill's is drawn while the editor has the keys.
    assert_eq!(harness.cursor(), None);
}

#[test]
fn in_the_app_the_fields_are_typed_in_while_the_editor_runs_and_what_it_wrote_joins_them() {
    let mut harness = in_the_app();
    let (path, dir) = in_vim(&mut harness);

    // Up to the fields: the cursor is in the subject, and the keys are
    // Quill's.
    harness.keys("cmd-up");
    assert_eq!(field(&harness), Field::Subject);
    assert!(!harness.state.editor_spot(&harness.env).keys);
    assert!(harness.state.editor_spot(&harness.env).place.is_some());
    assert!(harness.cursor().is_some());
    harness.type_text(" on Friday");
    assert!(harness.row_with("Subject:").contains("Lunch on Friday"));
    // Tab and shift-tab go between the fields, as with Quill's own editor.
    harness.keys("shift-tab");
    assert_eq!(field(&harness), Field::Cc);
    harness.type_text("sam.lee@example.com");
    harness.keys("escape");
    assert_eq!(away(&harness), Some(Away::InVim));

    // And down to the text again: the keys are the editor's.
    harness.keys("cmd-down");
    assert_eq!(field(&harness), Field::Body);
    assert!(harness.state.editor_spot(&harness.env).keys);

    // The editor writes other text and says to send. What goes is the
    // fields as they are here now, with the text as the editor left it.
    // A first line that looks like a field is text all the same.
    vim_writes(
        &path,
        &dir,
        "Subject: not a field\n\nFriday instead?\n",
        Some("send"),
    );
    harness.vim_exits(0);
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Subject: Lunch on Friday\r\n"), "{raw}");
    assert!(raw.contains("sam.lee@example.com"), "{raw}");
    assert!(raw.contains("Subject: not a field"), "{raw}");
    assert!(raw.contains("Friday instead?"), "{raw}");
    assert!(!raw.contains("Thursday"), "{raw}");
}

#[test]
fn in_the_app_wq_brings_the_text_into_quills_editor_and_the_command_goes_back_and_forth() {
    let mut harness = in_the_app();
    let (path, dir) = in_vim(&mut harness);
    harness.keys("cmd-up");
    harness.type_text("!");

    // `:wq`: the text is here, in Quill's own editor, and the fields are
    // as they were left here.
    vim_writes(&path, &dir, "Friday instead?\n", None);
    harness.vim_exits(0);
    assert_eq!(away(&harness), None);
    assert!(harness.screen().contains("Friday instead?"));
    assert!(harness.row_with("Subject:").contains("Lunch!"));
    assert!(harness.state.editor_spot(&harness.env).place.is_none());

    // The command takes it to the editor again, as it stands.
    harness.keys("ctrl-v");
    let (again, folder) = harness.in_vim().unwrap();
    assert_eq!(away(&harness), Some(Away::InVim));
    assert_eq!(
        std::fs::read_to_string(&again).unwrap(),
        "Friday instead?\n"
    );

    // And from the editor, the same command asks it to save and hand the
    // text back: the other editor is Quill's own.
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeVim)
    );
    let before = harness.host_requests.len();
    harness.act(Action::Command(CommandId::ComposeVim));
    assert_eq!(&harness.host_requests[before..], [HostRequest::ClosePane]);
    assert!(status(&harness).contains("Asked Vim to save and hand the message back."));
    assert_eq!(away(&harness), Some(Away::InVim));
    vim_writes(&again, &folder, "Saturday, then.\n", None);
    harness.vim_exits(0);
    assert_eq!(away(&harness), None);
    assert!(harness.screen().contains("Saturday, then."));
}

#[test]
fn in_the_app_what_the_editors_keys_do_is_said_once_and_then_only_when_asked_for() {
    let mut harness = in_the_app();
    let (path, dir) = in_vim(&mut harness);
    // The first message written in an editor: above it, what its keys do
    // and which of Quill's still work.
    let screen = harness.screen();
    assert!(screen.contains(":QuillSend sends the message"), "{screen}");
    assert!(screen.contains("goes to the fields above"), "{screen}");
    let with_tips = harness.state.editor_spot(&harness.env).place.unwrap();
    assert!(tips_remembered(&harness).is_empty());

    // That message done with, it has been seen: remembered, and the next
    // message has the editor from the rule down.
    vim_writes(&path, &dir, "Friday instead?\n", Some("discard"));
    harness.vim_exits(0);
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(tips_remembered(&harness), [false]);
    in_vim(&mut harness);
    let screen = harness.screen();
    assert!(!screen.contains(":QuillSend"), "{screen}");
    let without = harness.state.editor_spot(&harness.env).place.unwrap();
    assert!(without.y < with_tips.y);
    assert_eq!(without.y + without.height, with_tips.y + with_tips.height);

    // Asked for, it is there again, and the editor is given what is left;
    // asked again, it goes.
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeTips)
    );
    harness.keys("cmd-/");
    assert!(harness.screen().contains(":QuillSend sends the message"));
    assert_eq!(
        harness.state.editor_spot(&harness.env).place.unwrap(),
        with_tips
    );
    harness.keys("cmd-/");
    assert!(!harness.screen().contains(":QuillSend"));
    assert_eq!(
        harness.state.editor_spot(&harness.env).place.unwrap(),
        without
    );

    // With Quill's own editor there is nothing to say and the command is
    // off; and a config that says not to is obeyed from the start.
    let mut own = in_the_app();
    write_to_priya(&mut own);
    assert!(!own.state.is_enabled(&own.env, CommandId::ComposeTips));
    let mut told = in_the_app();
    told.env.editor_tips = false;
    in_vim(&mut told);
    assert!(!told.screen().contains(":QuillSend"));
}

#[test]
fn in_the_app_the_editor_gives_way_to_whatever_quill_draws_over_it() {
    let mut harness = in_the_app();
    harness.env.editor_tips = false;
    in_vim(&mut harness);
    let place = harness.state.editor_spot(&harness.env).place.unwrap();

    // The people on offer under an address are drawn over the editor's
    // top rows: it is told how many, and is no smaller for them.
    harness.keys("cmd-up shift-tab");
    assert_eq!(field(&harness), Field::Cc);
    harness.type_text("e");
    let people = harness
        .state
        .compose
        .as_ref()
        .unwrap()
        .completion
        .as_ref()
        .map_or(0, |completion| completion.people.len());
    assert!(people >= 3, "{people}");
    let spot = harness.state.editor_spot(&harness.env);
    assert_eq!(spot.place, Some(place));
    // Under Cc come the subject and the rule, then the text.
    assert_eq!(usize::from(spot.covered), people - 2);
    assert!(!spot.keys);
    harness.keys("escape");
    assert_eq!(harness.state.editor_spot(&harness.env).covered, 0);

    // The help is drawn over the pane: the editor is not on show, and
    // has no keys, until the help has gone.
    harness.keys("cmd-down");
    assert!(harness.state.editor_spot(&harness.env).keys);
    harness.act(Action::Command(CommandId::HelpToggle));
    let hidden = harness.state.editor_spot(&harness.env);
    assert_eq!((hidden.place, hidden.keys), (None, false));
    harness.act(Action::Command(CommandId::HelpToggle));
    let back = harness.state.editor_spot(&harness.env);
    assert_eq!((back.place, back.keys), (Some(place), true));
}

#[test]
fn in_the_app_a_click_on_a_field_or_on_the_text_takes_the_keys_there() {
    let mut harness = in_the_app();
    in_vim(&mut harness);
    let place = harness.state.editor_spot(&harness.env).place.unwrap();
    let areas =
        quill_tui::layout::compute(&harness.state, harness.state.size, harness.env.reader_width);
    let pane = areas.reader.unwrap();
    let click = |harness: &mut Harness, column, row| {
        harness.act(Action::Mouse(Mouse {
            kind: MouseKind::Press,
            column,
            row,
        }));
    };
    // The To field is the second row inside the pane.
    click(&mut harness, pane.x + 12, pane.y + 2);
    assert_eq!(field(&harness), Field::To);
    assert!(!harness.state.editor_spot(&harness.env).keys);
    // A click that reaches Quill over the text means the text.
    click(&mut harness, place.x + 3, place.y + 1);
    assert_eq!(field(&harness), Field::Body);
    assert!(harness.state.editor_spot(&harness.env).keys);
    // And the app's own word for that click is the command.
    harness.keys("cmd-up");
    harness.act(Action::Command(CommandId::ComposeBody));
    assert_eq!(field(&harness), Field::Body);
}

#[test]
fn in_quills_own_editor_the_two_keys_are_still_the_ends_of_the_text() {
    for app in [false, true] {
        let mut harness = wide();
        harness.env.editor_pane = app;
        write_to_priya(&mut harness);
        harness.keys("enter");
        harness.type_text("Second line.");
        // By the key, and as the app's menu runs the command.
        harness.keys("cmd-up");
        assert_eq!(field(&harness), Field::Body, "{app}");
        harness.type_text("First. ");
        harness.act(Action::Command(CommandId::ComposeBody));
        assert_eq!(field(&harness), Field::Body, "{app}");
        harness.type_text(" Last.");
        harness.act(Action::Command(CommandId::ComposeFields));
        harness.type_text("Very ");
        let body = harness.state.compose.as_ref().unwrap().body.text();
        assert_eq!(
            body, "Very First. Are you free on *Thursday*?\nSecond line. Last.",
            "{app}"
        );
    }
}

/// The Vim on PATH, or `None` (with a note) when there is none.
fn real_vim() -> Option<PathBuf> {
    let found = quill_tui::vim::find_program(
        std::ffi::OsStr::new("vim"),
        std::env::var_os("PATH").as_deref(),
    );
    if found.is_none() {
        eprintln!("skipped: there is no `vim` on PATH to run the plugin under");
    }
    found
}

/// Runs the Vim that a terminal would be given, but with no terminal and
/// no vimrc: the same argv from `quill_tui::vim::command`, with `-es`
/// (no screen), `-u NONE -i NONE` and the commands in `then`.
fn run_headless(vim: &Path, path: &Path, dir: &Path, then: &[&str]) -> i32 {
    let started = quill_tui::vim::command(ExternalEditor::Vim, vim, path, dir, 1);
    let mut command = Command::new(vim);
    command.args(["-es", "-u", "NONE", "-i", "NONE"]);
    // Everything Quill passes, up to the file: `--` ends the options.
    let arguments: Vec<_> = started.get_args().collect();
    let (options, file) = arguments.split_at(arguments.len() - 2);
    command.args(options);
    for line in then {
        command.arg("-c").arg(line);
    }
    command.args(file);
    for (name, value) in started.get_envs() {
        command.env(name, value.unwrap());
    }
    command
        .current_dir(started.get_current_dir().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.status().unwrap().code().unwrap_or(-1)
}

#[test]
fn the_real_plugin_attaches_completes_and_sends() {
    let Some(vim) = real_vim() else {
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let notes = scratch.path().join("notes for lunch.txt");
    std::fs::write(&notes, "Bring the notes.\n").unwrap();

    let mut harness = wide();
    harness.keys("c");
    harness.keys("tab tab");
    harness.type_text("Lunch");
    harness.keys("ctrl-v");
    let (path, dir) = harness.in_vim().unwrap();

    let attach = format!(
        "execute 'QuillAttach' fnameescape('{}')",
        notes.to_str().unwrap()
    );
    let status = run_headless(
        &vim,
        &path,
        &dir,
        &[
            // An address from the people Quill wrote out, as ctrl-x
            // ctrl-u would offer it.
            "call setline(2, 'To: ' . QuillComplete(0, 'priya')[0])",
            "call append(line('$'), 'Written in Vim.')",
            &attach,
            // One that is not there is refused, and names nothing.
            "QuillAttach /nonexistent/quill-test/nothing.pdf",
            "QuillSend",
        ],
    );
    harness.vim_exits(status);

    assert_eq!(status, 0);
    assert!(harness.state.compose.is_none(), "{}", harness.screen());
    let (sent, raw) = outgoing(&mut harness);
    assert!(
        raw.contains("To: \"Priya Nair\" <priya.nair@example.com>\r\n"),
        "{raw}"
    );
    assert!(raw.contains("Subject: Lunch\r\n"), "{raw}");
    assert!(raw.contains("Written in Vim."), "{raw}");
    assert!(raw.contains("notes for lunch.txt"), "{raw}");
    assert!(!raw.contains("nothing.pdf"), "{raw}");
    assert_eq!(sent.to.len(), 1);
    assert!(!dir.exists());
}

#[test]
fn the_real_plugin_postpones_discards_and_cancels() {
    let Some(vim) = real_vim() else {
        return;
    };
    // :QuillPostpone writes the file and says so.
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let status = run_headless(
        &vim,
        &path,
        &dir,
        &["call append(line('$'), 'More, later.')", "QuillPostpone"],
    );
    harness.vim_exits(status);
    assert!(harness.state.compose.is_none());
    let drafts = harness.store.drafts().unwrap();
    assert!(
        drafts[0].content.body.ends_with("More, later.\n"),
        "{drafts:?}"
    );

    // :QuillDiscard does not.
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let status = run_headless(&vim, &path, &dir, &["QuillDiscard"]);
    harness.vim_exits(status);
    assert!(harness.state.compose.is_none());
    assert!(harness.store.drafts().unwrap().is_empty());

    // :wq hands it back with nothing said.
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let status = run_headless(
        &vim,
        &path,
        &dir,
        &["call append(line('$'), 'One more thing.')", "wq"],
    );
    harness.vim_exits(status);
    assert_eq!(status, 0);
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.screen().contains("One more thing."));

    // :cq is an error, and the message is as it was.
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);
    let status = run_headless(
        &vim,
        &path,
        &dir,
        &["call append(line('$'), 'Never mind this.')", "write", "cq"],
    );
    harness.vim_exits(status);
    assert_ne!(status, 0);
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(!harness.screen().contains("Never mind this."));
    assert!(harness.screen().contains("Thursday"));
}

#[test]
fn a_quoted_line_that_looks_like_a_modeline_sets_nothing_in_the_real_vim() {
    let Some(vim) = real_vim() else {
        return;
    };
    let mut harness = wide();
    harness.keys("c");
    harness.keys("tab tab tab");
    // As a reply quotes it: someone else's text, shaped to set options.
    harness.act(Action::Paste("> vim: set shiftwidth=7 :\n".to_owned()));
    harness.keys("ctrl-v");
    let (path, dir) = harness.in_vim().unwrap();
    let out = dir.join("seen");
    let record = "call writefile([&modeline . ' ' . &shiftwidth], $QUILL_DRAFT_DIR . '/seen')";
    let status = run_headless(&vim, &path, &dir, &[record, "wq"]);
    assert_eq!(status, 0);
    // Modelines are off for the draft, and the option is as it was.
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "0 8");
    harness.vim_exits(status);
}

/// Runs the real Vim under a terminal of its own (`script` makes one),
/// and answers when it has gone. Only where `script` takes a command the
/// way macOS's does.
#[cfg(target_os = "macos")]
#[test]
fn send_from_outside_reaches_a_real_vim_through_the_command_file() {
    let Some(vim) = real_vim() else {
        return;
    };
    let script = Path::new("/usr/bin/script");
    if !script.is_file() {
        eprintln!("skipped: there is no /usr/bin/script to give Vim a terminal");
        return;
    }
    // As any Vim, and as the one that comes with macOS is inside Quill's
    // app, where it runs restricted whatever it was started as: there it
    // may not delete a file, and the plugin must not need to.
    for restricted in [false, true] {
        send_from_outside(&vim, script, restricted);
    }
}

fn send_from_outside(vim: &Path, script: &Path, restricted: bool) {
    let mut harness = wide();
    let (path, dir) = in_vim(&mut harness);

    // Vim as a terminal gets it, with no vimrc, waiting for keys that
    // never come.
    let started = quill_tui::vim::command(ExternalEditor::Vim, vim, &path, &dir, 1);
    let mut command = Command::new(script);
    command.args(["-q", "/dev/null"]).arg(vim);
    command.args(["-u", "NONE", "-i", "NONE"]);
    if restricted {
        command.arg("-Z");
    }
    command.args(started.get_args());
    for (name, value) in started.get_envs() {
        command.env(name, value.unwrap());
    }
    let mut child = command
        .env("TERM", "xterm-256color")
        .current_dir(started.get_current_dir().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // Send, from Quill: the toolbar's button in the app.
    std::thread::sleep(std::time::Duration::from_millis(700));
    harness.act(Action::Command(CommandId::ComposeSend));
    let began = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if began.elapsed() > std::time::Duration::from_secs(20) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Vim did not act on the command file (restricted: {restricted})");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    harness.vim_exits(status.code().unwrap_or(-1));

    // The plugin took the command, wrote the draft and its verdict, and
    // left; and the message went.
    assert!(harness.state.compose.is_none(), "{}", harness.screen());
    let (_, raw) = outgoing(&mut harness);
    assert!(raw.contains("Subject: Lunch\r\n"), "{raw}");
    assert!(raw.contains("Thursday"), "{raw}");
}
