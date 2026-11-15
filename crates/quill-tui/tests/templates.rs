//! Templates: text kept ready, chosen from a list while a message is being
//! written, with what Quill knows about the message filled in.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_tui::command::{CommandId, Context};
use quill_tui::compose::Field;
use quill_tui::editor::Editor;
use quill_tui::state::{Action, Identity, Mouse, MouseKind};
use quill_tui::template::Template;

const FOLLOW_UP: &str = "Subject: Following up on {{topic}}\n\
                         \n\
                         Hi {{first_name}},\n\
                         \n\
                         Thanks for meeting on {{date}}. As promised: {{what}}\n\
                         \n\
                         Best,\n\
                         {{from_name}}\n";

/// The demo mailbox with three templates, and a name to sign with.
fn with_templates(width: u16, height: u16) -> Harness {
    let mut harness = Harness::demo(width, height);
    harness.env.templates = vec![
        Template::parse("Meeting follow-up", FOLLOW_UP),
        Template::parse("Out of office", "I am away until {{back}}.\n"),
        Template::parse("thanks", "Thanks,\n{{from_name}}\n"),
    ];
    harness.env.identities.insert(
        "Work".to_owned(),
        Identity {
            full_name: Some("Robin Vale".to_owned()),
            signature: None,
        },
    );
    harness
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

fn body(harness: &Harness) -> String {
    harness.state.compose.as_ref().unwrap().body.text()
}

/// What is selected in the body, as text.
fn selected(harness: &Harness) -> Option<String> {
    let compose = harness.state.compose.as_ref().unwrap();
    compose
        .body
        .selection()
        .map(|range| compose.body.text()[range].to_owned())
}

#[test]
fn with_no_templates_there_is_nothing_to_insert_and_nothing_said_of_it() {
    let mut harness = Harness::demo(200, 60);
    harness.keys("c");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeTemplate)
    );
    assert!(
        !status(&harness).contains("template"),
        "{}",
        status(&harness)
    );
    harness.keys("ctrl-r cmd-T");
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.state.templates.is_none());
    // Nothing was typed into the message by the keys either.
    assert!(harness.state.compose.as_ref().unwrap().is_blank());
}

#[test]
fn with_templates_the_status_bar_names_the_key_that_works_here() {
    let mut harness = with_templates(200, 60);
    harness.keys("c");
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeTemplate)
    );
    assert!(
        status(&harness).contains("cmd-T template"),
        "{}",
        status(&harness)
    );
    // In a terminal, which keeps cmd for itself, it is the ctrl key.
    harness.env.cmd_keys = false;
    assert!(
        status(&harness).contains("ctrl-r template"),
        "{}",
        status(&harness)
    );
}

#[test]
fn a_template_is_filled_with_what_is_known_and_the_first_gap_is_selected() {
    let mut harness = with_templates(200, 60);
    harness.keys("c");
    harness.act(Action::Paste("Ana Ruiz <ana.ruiz@example.com>".to_owned()));
    harness.keys("ctrl-r");
    assert_eq!(harness.state.context(), Context::Templates);
    let screen = harness.screen();
    assert!(screen.contains("Insert a template"), "{screen}");
    for name in ["Meeting follow-up", "Out of office", "thanks"] {
        assert!(screen.contains(name), "{screen}");
    }
    // A template's subject is shown beside its name.
    assert!(
        harness
            .row_with("Meeting follow-up")
            .contains("Following up on {{topic}}")
    );

    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Compose);
    let compose = harness.state.compose.as_ref().unwrap();
    // The message had no subject, so it takes the template's. Nobody
    // knows the topic but the user: that is left as written.
    assert_eq!(compose.subject.text(), "Following up on {{topic}}");
    assert_eq!(
        body(&harness),
        "Hi Ana,\n\nThanks for meeting on 14 November 2026. As promised: {{what}}\n\nBest,\nRobin Vale"
    );
    // The first thing still to fill in is selected, in the body, so that
    // typing replaces it.
    assert_eq!(compose.field, Field::Body);
    assert_eq!(selected(&harness).as_deref(), Some("{{what}}"));
    harness.type_text("the notes.");
    assert!(
        body(&harness).contains("As promised: the notes.\n\nBest,"),
        "{}",
        body(&harness)
    );
    assert_eq!(selected(&harness), None);
    // What went in is part of the message: it is worth saving.
    assert!(harness.state.compose.as_ref().unwrap().is_modified());
}

#[test]
fn a_subject_already_written_is_kept_and_can_be_used() {
    let mut harness = with_templates(200, 60);
    harness.env.templates = vec![Template::parse(
        "re",
        "Subject: From the template\n\nAbout \"{{subject}}\": {{answer}}",
    )];
    harness.keys("c tab tab");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().field,
        Field::Subject
    );
    harness.type_text("Q4 roadmap");
    harness.keys("cmd-T enter");
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.subject.text(), "Q4 roadmap");
    assert_eq!(body(&harness), "About \"Q4 roadmap\": {{answer}}");
    assert_eq!(selected(&harness).as_deref(), Some("{{answer}}"));
}

#[test]
fn with_nobody_to_write_to_yet_their_name_is_left_to_fill_in() {
    let mut harness = with_templates(200, 60);
    harness.keys("c ctrl-r enter");
    assert!(
        body(&harness).starts_with("Hi {{first_name}},\n\nThanks for meeting on 14 November 2026."),
        "{}",
        body(&harness)
    );
    assert_eq!(selected(&harness).as_deref(), Some("{{first_name}}"));
}

#[test]
fn typing_narrows_the_list_and_the_arrows_choose() {
    let mut harness = with_templates(200, 60);
    harness.keys("c ctrl-r");
    harness.type_text("o");
    let picker = harness.state.templates.as_ref().unwrap();
    assert_eq!(harness.state.template_choices(&harness.env, picker), [0, 1]);
    assert!(!harness.screen().contains("  thanks"));
    harness.keys("down enter");
    assert_eq!(body(&harness), "I am away until {{back}}.");
    assert_eq!(selected(&harness).as_deref(), Some("{{back}}"));

    // Letters that match nothing leave nothing to choose, and say so.
    harness.keys("ctrl-r");
    harness.type_text("zzz");
    assert!(harness.screen().contains("No template matches."));
    let before = body(&harness);
    harness.keys("enter");
    assert_eq!(harness.state.context(), Context::Templates);
    assert_eq!(body(&harness), before);
    // Rubbing them out brings the list back; pasted letters narrow it too.
    harness.keys("backspace backspace backspace");
    harness.act(Action::Paste("thanks".to_owned()));
    let picker = harness.state.templates.as_ref().unwrap();
    assert_eq!(harness.state.template_choices(&harness.env, picker), [2]);
}

#[test]
fn esc_closes_the_list_and_leaves_the_message_as_it_was() {
    let mut harness = with_templates(200, 60);
    harness.keys("c tab tab tab");
    harness.type_text("Dear all,");
    harness.keys("ctrl-r down esc");
    // One esc closes the list, not the message with it.
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.state.templates.is_none());
    assert_eq!(body(&harness), "Dear all,");
    // And typing goes to the message again.
    harness.type_text("!");
    assert_eq!(body(&harness), "Dear all,!");
}

#[test]
fn text_goes_in_at_the_cursor_and_one_undo_takes_it_out() {
    let mut harness = with_templates(200, 60);
    harness.keys("c tab tab tab");
    harness.type_text("Dear all,");
    harness.keys("enter enter");
    harness.type_text("PS: see you there.");
    // Back up to the empty line between the two.
    harness.keys("up");
    harness.keys("cmd-T");
    harness.type_text("thanks");
    harness.keys("enter");
    assert_eq!(
        body(&harness),
        "Dear all,\nThanks,\nRobin Vale\nPS: see you there."
    );
    // Nothing was left to fill in: the cursor is after what went in.
    assert_eq!(selected(&harness), None);
    harness.type_text("!");
    assert!(
        body(&harness).contains("Robin Vale!\nPS"),
        "{}",
        body(&harness)
    );

    // The exclamation mark, then the whole template, in two steps.
    harness.keys("cmd-z cmd-z");
    assert_eq!(body(&harness), "Dear all,\n\nPS: see you there.");
    // Doing it again puts it back with the same thing selected.
    let mut harness = with_templates(200, 60);
    harness.keys("c ctrl-r down enter");
    assert_eq!(selected(&harness).as_deref(), Some("{{back}}"));
    harness.keys("cmd-z");
    assert_eq!(body(&harness), "");
    harness.keys("cmd-Z");
    assert_eq!(body(&harness), "I am away until {{back}}.");
    assert_eq!(selected(&harness).as_deref(), Some("{{back}}"));
}

#[test]
fn a_template_replaces_what_is_selected() {
    let mut harness = with_templates(200, 60);
    harness.keys("c tab tab tab");
    harness.type_text("placeholder words");
    harness.keys("cmd-a ctrl-r");
    harness.type_text("thanks");
    harness.keys("enter");
    assert_eq!(body(&harness), "Thanks,\nRobin Vale");
}

#[test]
fn a_click_chooses_as_the_keys_do() {
    let mut harness = with_templates(200, 60);
    harness.keys("c ctrl-r");
    let screen = harness.screen();
    let (row, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Out of office"))
        .unwrap();
    // Counted in cells: the frame's own characters are one cell each.
    let column = line[..line.find("Out of office").unwrap()].chars().count();
    let click = Action::Mouse(Mouse {
        kind: MouseKind::Press,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
    });
    harness.act(click.clone());
    assert_eq!(harness.state.templates.as_ref().unwrap().selected, 1);
    // A click on what is chosen is `enter`.
    harness.act(click);
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(body(&harness), "I am away until {{back}}.");
}

#[test]
fn what_a_template_holds_is_only_ever_text() {
    let mut harness = with_templates(200, 60);
    harness.env.templates = vec![Template::parse(
        "sly",
        "{{> /etc/passwd}} {{#each to}}x{{/each}} {{ `id` }} {{$(id)}} {{to}} {{to_name}}\u{1b}]0;t\u{7}\n{{from_name}}",
    )];
    harness.keys("c");
    // A recipient whose name is itself a placeholder is put in as it is.
    harness.act(Action::Paste(
        "\"{{from_name}}\" <ana.ruiz@example.com>".to_owned(),
    ));
    harness.keys("ctrl-r enter");
    let text = body(&harness);
    assert_eq!(
        text,
        "{{> /etc/passwd}} {{#each to}}x{{/each}} {{ `id` }} {{$(id)}} ana.ruiz@example.com {{from_name}}]0;t\nRobin Vale"
    );
    // None of those is a placeholder, so nothing is selected.
    assert_eq!(selected(&harness), None);
    assert!(!harness.screen().contains('\u{1b}'));
}

#[test]
fn the_help_overlay_lists_the_command_with_both_its_keys() {
    let mut harness = with_templates(200, 60);
    harness.keys("c cmd-T");
    assert_eq!(harness.state.context(), Context::Templates);
    harness.keys("esc");
    // `?` is a letter while writing; help is asked for from the app's menu.
    harness.act(Action::Command(CommandId::HelpToggle));
    let row = harness.row_with("Insert a template");
    assert!(row.contains("cmd-T, ctrl-r"), "{row}");
}

#[test]
fn snapshot_of_the_template_picker() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let mut harness = with_templates(width, height);
        harness.keys("c ctrl-r");
        harness.type_text("o");
        insta::assert_snapshot!(
            format!("template_picker_{width}x{height}"),
            harness.screen()
        );
    }
}
