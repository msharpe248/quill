//! Spell checking of a message being written: when the body is sent to
//! the host's checker, what is done with its answers, and choosing another
//! spelling.
//!
//! The host here is the harness: it collects what the UI asks for, and a
//! test answers by hand.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_tui::command::{CommandId, Context};
use quill_tui::editor::Editor;
use quill_tui::state::{Action, HostRequest, MOST_SPELL_CHECKED};
use ratatui::style::Modifier;

/// The demo mailbox in Quill's own window, where there is a checker, with
/// a new message open and the cursor in its body.
fn writing(width: u16, height: u16) -> Harness {
    let mut harness = Harness::demo(width, height);
    harness.env.spell_check = true;
    harness.keys("c");
    // Ending in a comma, so nobody is on offer to complete it with.
    harness.act(Action::Paste("sam.lee@example.com, ".to_owned()));
    harness.keys("tab tab");
    harness.type_text("Lunhc");
    harness.keys("tab");
    let compose = harness.state.compose.as_ref().unwrap();
    assert_eq!(compose.field, quill_tui::compose::Field::Body);
    assert_eq!(compose.subject.text(), "Lunhc");
    harness
}

fn status(harness: &Harness) -> String {
    harness.screen().lines().last().unwrap().to_owned()
}

fn body(harness: &Harness) -> String {
    harness.state.compose.as_ref().unwrap().body.text()
}

/// Every text sent for checking so far, with the id it went under.
fn checks(harness: &Harness) -> Vec<(u64, String)> {
    harness
        .host_requests
        .iter()
        .filter_map(|request| match request {
            HostRequest::SpellCheck { id, text } => Some((*id, text.clone())),
            _ => None,
        })
        .collect()
}

/// Every word whose other spellings were asked for.
fn asked(harness: &Harness) -> Vec<(u64, String)> {
    harness
        .host_requests
        .iter()
        .filter_map(|request| match request {
            HostRequest::SpellSuggest { id, word } => Some((*id, word.clone())),
            _ => None,
        })
        .collect()
}

/// The keys go quiet: the run loop's timer fires.
fn pause(harness: &mut Harness) {
    harness.act(Action::TypingPaused);
}

/// The checker answers the check last sent with these words of its text.
fn answer(harness: &mut Harness, words: &[&str]) {
    let (id, text) = checks(harness).pop().expect("nothing was sent to check");
    let words = words
        .iter()
        .map(|word| {
            let start = text
                .find(word)
                .unwrap_or_else(|| panic!("{word} in {text}"));
            (start, start + word.len())
        })
        .collect();
    harness.act(Action::Spelling { id, words });
}

/// The words marked as misspelt in the body, as text.
fn marked(harness: &Harness) -> Vec<String> {
    let compose = harness.state.compose.as_ref().unwrap();
    let text = compose.body.text();
    compose
        .body
        .misspellings()
        .into_iter()
        .map(|range| text[range].to_owned())
        .collect()
}

/// The text drawn underlined in the row of the screen that holds `needle`,
/// in runs.
fn underlined(harness: &Harness, needle: &str) -> Vec<String> {
    let screen = harness.screen();
    let row = screen
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} is not on screen:\n{screen}"));
    let buffer = harness.buffer();
    let mut runs: Vec<String> = Vec::new();
    let mut open = false;
    let mut covered = 0;
    for x in 0..buffer.area.width {
        let cell = &buffer[(x, row as u16)];
        // A wide character owns the cell after it.
        if covered > 0 {
            covered -= 1;
            continue;
        }
        covered = quill_tui::text::width(cell.symbol()).saturating_sub(1);
        if cell.modifier.contains(Modifier::UNDERLINED) {
            if !open {
                runs.push(String::new());
            }
            runs.last_mut().unwrap().push_str(cell.symbol());
            open = true;
        } else {
            open = false;
        }
    }
    runs
}

#[test]
fn typing_sends_nothing_and_a_pause_sends_the_body_once() {
    let mut harness = writing(200, 60);
    harness.type_text("I saw teh fox.");
    // Not a request per key: nothing has gone yet.
    assert!(checks(&harness).is_empty());
    assert!(harness.state.spell_check_due(&harness.env));

    pause(&mut harness);
    // The body, and nothing but the body: not who it is to, nor its
    // subject, misspelt as that is.
    assert_eq!(checks(&harness), [(1, "I saw teh fox.".to_owned())]);
    assert!(!harness.state.spell_check_due(&harness.env));
    // Another pause with nothing new asks nothing.
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 1);

    answer(&mut harness, &["teh"]);
    assert_eq!(marked(&harness), ["teh"]);
    // Answered, and unchanged since: there is nothing to ask.
    assert!(!harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 1);

    // More typing wants another look, with the next id.
    harness.type_text(" Agian.");
    assert!(harness.state.spell_check_due(&harness.env));
    assert_eq!(checks(&harness).len(), 1);
    pause(&mut harness);
    assert_eq!(
        checks(&harness).last(),
        Some(&(2, "I saw teh fox. Agian.".to_owned()))
    );
}

#[test]
fn nothing_is_checked_without_a_checker_or_when_switched_off_or_when_not_writing() {
    // A terminal: no checker.
    let mut harness = writing(200, 60);
    harness.env.spell_check = false;
    harness.type_text("teh");
    assert!(!harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert!(checks(&harness).is_empty());
    for id in [CommandId::ComposeSpellSuggest, CommandId::ComposeSpellNext] {
        assert!(!harness.state.is_enabled(&harness.env, id));
    }

    // `spell_check = false` in the config.
    harness.env.spell_check = true;
    harness.env.compose.spell_check = false;
    assert!(!harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert!(checks(&harness).is_empty());

    // Reading mail, nothing is ever sent, whatever is on screen.
    let mut harness = Harness::demo(200, 60);
    harness.env.spell_check = true;
    harness.keys("enter j j");
    assert!(!harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    harness.keys("/");
    harness.type_text("teh");
    pause(&mut harness);
    assert!(harness.host_requests.is_empty());
}

#[test]
fn one_check_at_a_time_and_a_newer_text_waits_for_the_answer_to_the_older() {
    let mut harness = writing(200, 60);
    harness.type_text("teh");
    pause(&mut harness);
    assert_eq!(checks(&harness), [(1, "teh".to_owned())]);

    // Typed on before the answer: nothing more goes while one is out.
    harness.type_text(" brwon");
    assert!(!harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 1);

    // The answer is for a text that is no longer the text: it marks
    // nothing. Now the newer text can go.
    answer(&mut harness, &["teh"]);
    assert_eq!(marked(&harness), Vec::<String>::new());
    assert!(harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert_eq!(checks(&harness).last(), Some(&(2, "teh brwon".to_owned())));
    answer(&mut harness, &["teh", "brwon"]);
    assert_eq!(marked(&harness), ["teh", "brwon"]);
}

#[test]
fn an_answer_nobody_is_waiting_for_is_dropped() {
    let mut harness = writing(200, 60);
    harness.type_text("teh fox");
    // Before anything was asked.
    harness.act(Action::Spelling {
        id: 1,
        words: vec![(0, 3)],
    });
    assert!(marked(&harness).is_empty());
    pause(&mut harness);
    // Under an id that was never given out, and one from the future.
    for id in [0, 7, u64::MAX] {
        harness.act(Action::Spelling {
            id,
            words: vec![(0, 3)],
        });
        assert!(marked(&harness).is_empty());
    }
    // The real one.
    harness.act(Action::Spelling {
        id: 1,
        words: vec![(0, 3)],
    });
    assert_eq!(marked(&harness), ["teh"]);
    // The same answer a second time changes nothing, even with other
    // words in it.
    harness.act(Action::Spelling {
        id: 1,
        words: vec![(4, 7)],
    });
    assert_eq!(marked(&harness), ["teh"]);
}

#[test]
fn a_check_that_is_never_answered_does_not_stop_checking_for_good() {
    let mut harness = writing(200, 60);
    harness.type_text("teh");
    pause(&mut harness);
    harness.type_text(" fox");
    assert!(!harness.state.spell_check_due(&harness.env));
    // A while later, with no answer: the newer text goes anyway.
    harness.act(Action::ClockTick(common::NOW + 60));
    assert!(harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert_eq!(checks(&harness).last(), Some(&(2, "teh fox".to_owned())));
    // The late answer to the first is not taken for the second's.
    harness.act(Action::Spelling {
        id: 1,
        words: vec![(4, 7)],
    });
    assert!(marked(&harness).is_empty());
    answer(&mut harness, &["teh"]);
    assert_eq!(marked(&harness), ["teh"]);
}

#[test]
fn offsets_that_do_not_fit_the_text_are_not_used() {
    let mut harness = writing(200, 60);
    // Bytes: "日本語" is 0..9, a space, "wrod" 10..14, a space, the emoji
    // 15..19, a space, "teh" 20..23.
    harness.type_text("日本語 wrod 👍 teh");
    assert_eq!(body(&harness).len(), 23);
    pause(&mut harness);
    harness.act(Action::Spelling {
        id: 1,
        words: vec![
            // Good.
            (10, 14),
            // Inside a character at either end.
            (1, 9),
            (0, 4),
            (16, 19),
            // Empty, backwards, past the end, absurd.
            (5, 5),
            (14, 10),
            (20, 24),
            (usize::MAX - 1, usize::MAX),
            // Good, and out of order.
            (20, 23),
        ],
    });
    assert_eq!(marked(&harness), ["wrod", "teh"]);
}

#[test]
fn misspelt_words_are_underlined_where_they_are_drawn() {
    let mut harness = writing(200, 60);
    harness.type_text("日本語 wrod 👍 teh ok");
    harness.keys("enter");
    harness.type_text("> a qouted line");
    pause(&mut harness);
    answer(&mut harness, &["wrod", "teh", "qouted"]);
    // Exactly those cells, with wide characters and an emoji before them
    // on the row.
    assert_eq!(underlined(&harness, "wrod"), ["wrod", "teh"]);
    assert_eq!(underlined(&harness, "qouted"), ["qouted"]);
    // Not by colour alone: the underline is there with colour off too.
    let mut plain = Harness::build(
        200,
        60,
        quill_tui::keymap::Keymap::preset(quill_core::config::KeymapPreset::Vim),
        quill_tui::theme::ColorMode::None,
    );
    plain.env.spell_check = true;
    plain.keys("c tab tab tab");
    plain.type_text("a wrod");
    pause(&mut plain);
    answer(&mut plain, &["wrod"]);
    assert_eq!(underlined(&plain, "a wrod"), ["wrod"]);

    // A word typed against is no longer the word that was checked: its
    // mark goes, and the others on the line move with their words.
    harness.keys("up cmd-left");
    harness.type_text("語 ");
    assert_eq!(underlined(&harness, "wrod"), ["wrod", "teh"]);
    assert_eq!(marked(&harness), ["wrod", "teh", "qouted"]);
    harness.keys("alt-right alt-right");
    harness.type_text("s");
    assert_eq!(
        body(&harness).lines().next(),
        Some("語 日本語 wrods 👍 teh ok")
    );
    assert_eq!(underlined(&harness, "wrods"), ["teh"]);
    // A line taken out above moves the marks below with their words.
    harness.keys("cmd-up shift-down backspace");
    assert_eq!(body(&harness), "> a qouted line");
    assert_eq!(marked(&harness), ["qouted"]);
    assert_eq!(underlined(&harness, "qouted"), ["qouted"]);
}

#[test]
fn a_misspelt_word_is_replaced_from_a_list_as_one_step_to_undo() {
    let mut harness = writing(200, 60);
    harness.type_text("I saw teh fox");
    pause(&mut harness);
    answer(&mut harness, &["teh"]);
    // At the end, on "fox": there is no misspelt word here to ask about.
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeSpellSuggest)
    );
    assert!(
        !status(&harness).contains("spelling"),
        "{}",
        status(&harness)
    );
    harness.keys("ctrl-g");
    assert!(asked(&harness).is_empty());

    // Just after the word, and on it.
    harness.keys("alt-left left");
    assert_eq!(
        harness.state.compose.as_ref().unwrap().body.cursor_offset(),
        9
    );
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeSpellSuggest)
    );
    assert!(
        status(&harness).contains("cmd-G spelling"),
        "{}",
        status(&harness)
    );
    harness.keys("left");
    harness.keys("ctrl-g");
    assert_eq!(asked(&harness), [(2, "teh".to_owned())]);
    // Nothing shows until the host answers.
    assert_eq!(harness.state.context(), Context::Compose);

    harness.act(Action::Suggestions {
        id: 2,
        words: vec![
            "the".to_owned(),
            "ten".to_owned(),
            // What is there already, a repeat, nothing, and something
            // that is not a word are not offered.
            "teh".to_owned(),
            "the".to_owned(),
            "  ".to_owned(),
            "tea\u{1b}[2J\ntee".to_owned(),
        ],
    });
    assert_eq!(harness.state.context(), Context::Suggestions);
    let picker = harness.state.suggestions.as_ref().unwrap();
    assert_eq!(picker.words, ["the", "ten", "tea tee"]);
    let screen = harness.screen();
    assert!(screen.contains("Spelling of \"teh\""), "{screen}");
    assert!(harness.row_with("> the").contains("the"), "{screen}");
    assert!(
        status(&harness).contains("enter use this spelling"),
        "{}",
        status(&harness)
    );

    // Letters are not typed into the message behind the list.
    harness.type_text("zx");
    assert_eq!(body(&harness), "I saw teh fox");
    // Moving stays inside the list.
    harness.keys("j j j k");
    assert_eq!(harness.state.suggestions.as_ref().unwrap().selected, 1);
    harness.keys("g g enter");
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(body(&harness), "I saw the fox");
    // The cursor is after the new word, and its mark is gone.
    assert_eq!(
        harness.state.compose.as_ref().unwrap().body.cursor_offset(),
        9
    );
    assert!(marked(&harness).is_empty());
    // The changed text wants checking again.
    assert!(harness.state.spell_check_due(&harness.env));

    // One undo puts the word back as it was, one redo the choice.
    harness.keys("cmd-z");
    assert_eq!(body(&harness), "I saw teh fox");
    harness.keys("cmd-Z");
    assert_eq!(body(&harness), "I saw the fox");
    // And typing after it is a step of its own.
    harness.type_text("m");
    harness.keys("cmd-z");
    assert_eq!(body(&harness), "I saw the fox");
}

#[test]
fn the_list_closes_without_changing_anything() {
    let mut harness = writing(200, 60);
    harness.type_text("teh");
    pause(&mut harness);
    answer(&mut harness, &["teh"]);
    harness.keys("cmd-G");
    assert_eq!(asked(&harness), [(2, "teh".to_owned())]);
    harness.act(Action::Suggestions {
        id: 2,
        words: vec!["the".to_owned()],
    });
    assert_eq!(harness.state.context(), Context::Suggestions);
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::Compose);
    assert_eq!(body(&harness), "teh");
    assert_eq!(marked(&harness), ["teh"]);
    // The message is still being written: esc closed the list only.
    assert!(harness.state.compose.is_some());
}

#[test]
fn no_suggestions_is_said_and_a_late_answer_is_dropped() {
    let mut harness = writing(200, 60);
    harness.type_text("so xqzv");
    pause(&mut harness);
    answer(&mut harness, &["xqzv"]);
    harness.keys("ctrl-g");
    harness.act(Action::Suggestions {
        id: 2,
        words: Vec::new(),
    });
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(
        status(&harness).contains("No suggestions for \"xqzv\"."),
        "{}",
        status(&harness)
    );

    // Asked, and then typed on: the answer is for a text that has moved
    // on, and is not offered.
    harness.keys("ctrl-g");
    assert_eq!(asked(&harness).last(), Some(&(3, "xqzv".to_owned())));
    harness.keys("cmd-left");
    harness.type_text("al");
    harness.act(Action::Suggestions {
        id: 3,
        words: vec!["quiz".to_owned()],
    });
    assert_eq!(harness.state.context(), Context::Compose);
    assert!(harness.state.suggestions.is_none());
    assert_eq!(body(&harness), "also xqzv");
    // The word kept its mark as it moved, and can be asked about again.
    assert_eq!(marked(&harness), ["xqzv"]);

    // An answer nobody asked for, and one that arrives when the message
    // is no longer in front of the user, open nothing.
    harness.act(Action::Suggestions {
        id: 99,
        words: vec!["quiz".to_owned()],
    });
    assert!(harness.state.suggestions.is_none());
    harness.keys("cmd-right ctrl-g");
    assert_eq!(asked(&harness).last(), Some(&(4, "xqzv".to_owned())));
    // Help, from the menu: `?` here is a character to type.
    harness.act(Action::Command(CommandId::HelpToggle));
    assert_eq!(harness.state.context(), Context::Help);
    harness.act(Action::Suggestions {
        id: 4,
        words: vec!["quiz".to_owned()],
    });
    assert!(harness.state.suggestions.is_none());
    harness.keys("esc ctrl-g");
    assert_eq!(asked(&harness).last(), Some(&(5, "xqzv".to_owned())));
    harness.keys("esc");
    assert!(harness.state.compose.is_none());
    harness.act(Action::Suggestions {
        id: 5,
        words: vec!["quiz".to_owned()],
    });
    assert!(harness.state.suggestions.is_none());
    assert_ne!(harness.state.context(), Context::Suggestions);
}

#[test]
fn the_next_misspelling_is_a_key_away_and_goes_round() {
    let mut harness = writing(200, 60);
    harness.type_text("teh quick brwon");
    harness.keys("enter");
    harness.type_text("fox jumsp");
    // Nothing is marked yet: there is nowhere to go.
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::ComposeSpellNext)
    );
    pause(&mut harness);
    answer(&mut harness, &["teh", "brwon", "jumsp"]);
    let cursor = |harness: &Harness| harness.state.compose.as_ref().unwrap().body.cursor_offset();
    // From the end, round to the first.
    harness.keys("ctrl-y");
    assert_eq!(cursor(&harness), 0);
    harness.keys("cmd-;");
    assert_eq!(cursor(&harness), 10);
    harness.keys("ctrl-y");
    assert_eq!(cursor(&harness), 20);
    // On a word, its other spellings can be asked for at once.
    harness.keys("ctrl-g");
    assert_eq!(asked(&harness), [(2, "jumsp".to_owned())]);
    harness.keys("ctrl-y");
    assert_eq!(cursor(&harness), 0);
    // From a header field it goes to the body.
    harness.keys("shift-tab");
    harness.keys("ctrl-y");
    assert_eq!(cursor(&harness), 10);
    assert_eq!(
        harness.state.compose.as_ref().unwrap().field,
        quill_tui::compose::Field::Body
    );
}

#[test]
fn a_body_too_long_to_send_is_not_checked() {
    let mut harness = writing(200, 60);
    harness.type_text("teh");
    pause(&mut harness);
    answer(&mut harness, &["teh"]);
    assert_eq!(marked(&harness), ["teh"]);
    // Exactly the most that is sent, still goes.
    harness.act(Action::Paste("a".repeat(MOST_SPELL_CHECKED - 3)));
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 2);
    assert_eq!(checks(&harness)[1].1.len(), MOST_SPELL_CHECKED);
    answer(&mut harness, &["teh"]);
    // One byte more does not, and what was marked is no longer said to
    // be: nothing has looked at this text.
    harness.type_text("b");
    assert!(harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 2);
    assert!(!harness.state.spell_check_due(&harness.env));
    assert!(marked(&harness).is_empty());
    // Shorter again, it is checked again.
    harness.keys("backspace");
    pause(&mut harness);
    assert_eq!(checks(&harness).len(), 3);
}

#[test]
fn an_answer_for_one_message_is_never_applied_to_the_next() {
    let mut harness = writing(200, 60);
    harness.type_text("teh");
    pause(&mut harness);
    assert_eq!(checks(&harness), [(1, "teh".to_owned())]);
    // The message is closed, kept as a draft, before the answer comes,
    // and another started whose body is the same three letters long.
    harness.keys("esc c tab tab tab");
    harness.type_text("the");
    // The old check is still out: the new text waits for its answer.
    assert!(!harness.state.spell_check_due(&harness.env));
    harness.act(Action::Spelling {
        id: 1,
        words: vec![(0, 3)],
    });
    assert!(marked(&harness).is_empty());
    // And then goes, under an id of its own.
    assert!(harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    assert_eq!(checks(&harness).last(), Some(&(2, "the".to_owned())));
    answer(&mut harness, &[]);
    assert!(marked(&harness).is_empty());
}

#[test]
fn a_reply_is_checked_as_it_opens_once_the_keys_are_quiet() {
    let mut harness = Harness::demo(200, 60);
    harness.env.spell_check = true;
    harness.keys("r");
    assert_eq!(harness.state.context(), Context::Compose);
    // Nothing typed yet, and the quoted message is there to look at.
    assert!(harness.state.spell_check_due(&harness.env));
    pause(&mut harness);
    let (_, text) = checks(&harness).pop().unwrap();
    assert_eq!(text, body(&harness));
    assert!(text.contains("> Hi Robin,"), "{text}");
}

#[test]
fn snapshot_of_the_spelling_list() {
    for (width, height) in [(80, 24), (200, 60)] {
        let mut harness = writing(width, height);
        harness.type_text("I saw teh fox");
        pause(&mut harness);
        answer(&mut harness, &["teh"]);
        harness.keys("alt-left left ctrl-g");
        harness.act(Action::Suggestions {
            id: 2,
            words: ["the", "ten", "tea", "tech"].map(str::to_owned).to_vec(),
        });
        harness.keys("j");
        insta::assert_snapshot!(format!("spelling_list_{width}x{height}"), harness.screen());
    }
}
