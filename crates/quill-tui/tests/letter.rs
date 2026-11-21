//! The letter that stands for an account where mail of several accounts is
//! listed together: the first of its name unless the user chooses another,
//! from the accounts pane, from the settings window's list of accounts, or
//! as the account is added.
//!
//! The service here is the harness: what the UI asked of it is looked at,
//! and its answers are handed in. What the service writes is tested where
//! it is, in `quill-cli`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::config::TlsMode;
use quill_host::bridge::{FormField, FormRole, FormShow, FormUsed, SettingsUsed};
use quill_tui::command::{CommandId, Context};
use quill_tui::keymap::Keymap;
use quill_tui::settings::{AccountLine, Settings, SettingsRead};
use quill_tui::setup::{AccountDraft, AccountEvent, AccountRequest, Discovery, ServerDraft};
use quill_tui::state::{Action, NoticeKind, SettingsAct};
use quill_tui::theme::Theme;

/// The demo mailbox with a service behind it, the accounts pane on Work.
fn on_work(width: u16, height: u16) -> Harness {
    let mut harness = Harness::with_accounts(width, height);
    harness.keys("1 j");
    assert!(
        harness.screen().contains("> ▾ Work"),
        "{}",
        harness.screen()
    );
    harness
}

/// The letters asked of the service, in order.
fn asked(harness: &Harness) -> Vec<(&str, Option<char>)> {
    harness
        .account_requests
        .iter()
        .filter_map(|request| match request {
            AccountRequest::Letter { account, letter } => Some((account.as_str(), *letter)),
            _ => None,
        })
        .collect()
}

/// The service's part, done: the store has the letter, and the UI is told.
fn set(harness: &mut Harness, account: &str, letter: Option<char>) {
    let text = letter.map(String::from);
    assert!(
        harness
            .store
            .set_account_letter(account, text.as_deref())
            .unwrap()
    );
    harness.act(Action::Account(AccountEvent::LetterSet {
        account: account.to_owned(),
        letter,
        result: Ok(()),
    }));
}

fn sheet(harness: &Harness) -> Option<FormShow> {
    harness.state.form(&harness.env)
}

fn press(harness: &mut Harness, button: Option<&str>, letter: &str) {
    let step = sheet(harness).unwrap().step().to_owned();
    harness.act(Action::FormUsed(
        FormUsed::new(&step, button, &[("letter", letter)], &[]).unwrap(),
    ));
}

fn last_notice(harness: &Harness) -> (NoticeKind, String) {
    let notice = harness.state.notices.back().unwrap();
    (notice.kind, notice.text.clone())
}

/// The letters the list marks two messages with where every account's
/// mail is listed together: one of Work's, and one of Personal's. The
/// accounts pane has the keys and is on Work: it goes up to every inbox
/// to look, and back.
fn marks(harness: &mut Harness) -> (char, char) {
    harness.keys("k");
    let screen = harness.screen();
    assert!(screen.contains("Inbox (unified)"), "{screen}");
    harness.keys("j");
    let mark = |subject: &str| {
        let line = screen
            .lines()
            .find(|line| line.contains(subject))
            .unwrap_or_else(|| panic!("{subject} is not listed\n{screen}"));
        // The list's own columns: the pane the subject is in. Its marks
        // are five columns and a space, and then the account's letter.
        let row = line
            .split(['│', '┃'])
            .find(|pane| pane.contains(subject))
            .unwrap();
        row.chars().nth(6).unwrap()
    };
    (mark("Q4 roadmap"), mark("Book club pick"))
}

#[test]
fn the_list_marks_mail_with_the_first_letter_of_the_name_until_another_is_chosen() {
    let mut harness = on_work(120, 40);
    let before = marks(&mut harness);
    assert_eq!(before, ('W', 'P'));

    harness.keys("Y");
    assert_eq!(harness.state.context(), Context::AccountLetter);
    let screen = harness.screen();
    assert!(screen.contains(" Letter for Work "), "{screen}");
    // What leaving it empty comes to is said, with the letter.
    assert!(screen.contains("Left empty, it is W,"), "{screen}");
    // Typing goes to the letter, and runs nothing behind the question.
    harness.type_text("7");
    assert_eq!(harness.state.context(), Context::AccountLetter);
    harness.keys("enter");
    assert_eq!(asked(&harness), [("Work", Some('7'))]);
    // Nothing is changed until the service says it is.
    assert_eq!(harness.state.context(), Context::Sidebar);
    assert_eq!(marks(&mut harness), before);

    set(&mut harness, "Work", Some('7'));
    assert_eq!(marks(&mut harness), ('7', 'P'));
    assert_eq!(
        last_notice(&harness),
        (
            NoticeKind::Info,
            "Mail of Work is marked 7 in lists.".to_owned()
        )
    );

    // Asked again, the question starts with the letter the account has,
    // and emptied it is the first of the name again.
    harness.keys("Y");
    let prompt = harness.state.letter_prompt.as_ref().unwrap();
    assert_eq!(prompt.input.text(), "7");
    harness.keys("backspace enter");
    assert_eq!(asked(&harness).last(), Some(&("Work", None)));
    set(&mut harness, "Work", None);
    assert_eq!(marks(&mut harness), before);
    assert_eq!(
        last_notice(&harness).1,
        "Mail of Work is marked with the first letter of its name in lists."
    );
}

#[test]
fn only_one_letter_or_number_will_do_and_esc_changes_nothing() {
    let mut harness = on_work(120, 40);
    for typed in ["ab", "@", "7 7", "-"] {
        harness.keys("Y");
        harness.type_text(typed);
        harness.keys("enter");
        assert_eq!(harness.state.context(), Context::AccountLetter, "{typed:?}");
        let prompt = harness.state.letter_prompt.as_ref().unwrap();
        assert_eq!(
            prompt.problem.as_deref(),
            Some("The letter is one letter or number, or nothing."),
            "{typed:?}"
        );
        assert!(
            harness.screen().contains("one letter or number"),
            "{typed:?}"
        );
        // Typing again takes the complaint away.
        harness.keys("backspace");
        assert_eq!(harness.state.letter_prompt.as_ref().unwrap().problem, None);
        harness.keys("esc");
        assert_eq!(harness.state.context(), Context::Sidebar);
    }
    assert!(asked(&harness).is_empty());

    // A letter with spaces about it is the letter; any script's will do.
    for (typed, letter) in [(" g ", 'g'), ("É", 'É'), ("9", '9')] {
        harness.keys("Y");
        harness.type_text(typed);
        harness.keys("enter");
        assert_eq!(asked(&harness).last(), Some(&("Work", Some(letter))));
    }
}

#[test]
fn a_letter_that_could_not_be_kept_is_said_and_nothing_is_read_again() {
    let mut harness = on_work(120, 40);
    harness.keys("Y");
    harness.type_text("x");
    harness.keys("enter");
    let before = marks(&mut harness);
    harness.act(Action::Account(AccountEvent::LetterSet {
        account: "Work".to_owned(),
        letter: Some('x'),
        result: Err("config.toml cannot be written: Permission denied".to_owned()),
    }));
    assert_eq!(
        last_notice(&harness),
        (
            NoticeKind::Error,
            "The letter for Work was not changed. config.toml cannot be written: \
             Permission denied"
                .to_owned()
        )
    );
    assert_eq!(marks(&mut harness), before);
}

#[test]
fn the_command_is_for_the_account_the_accounts_pane_is_on_where_there_are_settings() {
    // The demo mailbox has no settings file to write a letter in.
    let mut demo = Harness::demo(120, 40);
    demo.keys("1 j");
    assert!(!demo.state.is_enabled(&demo.env, CommandId::AccountLetter));
    demo.keys("Y");
    assert!(demo.state.letter_prompt.is_none());

    let mut harness = on_work(120, 40);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountLetter)
    );
    // On every inbox there is no account to give a letter to.
    harness.keys("k");
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::AccountLetter)
    );
    // And not while a message is being written.
    harness.keys("j 2 c");
    assert!(harness.state.compose.is_some());
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::AccountLetter)
    );
}

#[test]
fn in_the_app_the_question_is_a_sheet_with_one_field() {
    let mut harness = on_work(120, 40);
    harness.env.sheets = true;
    harness.keys("Y");
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        (shown.step(), shown.title()),
        ("account_letter", "Letter for Work")
    );
    let [
        FormField::Text {
            id,
            label,
            value,
            focus: true,
            help: Some(help),
        },
    ] = shown.fields()
    else {
        panic!("{shown:?}");
    };
    assert_eq!(
        (id.as_str(), label.as_str(), value.as_str()),
        ("letter", "Letter:", "")
    );
    assert!(help.starts_with("One letter or number."), "{help}");
    let buttons: Vec<(&str, Option<FormRole>)> = shown
        .buttons()
        .iter()
        .map(|button| (button.title.as_str(), button.role))
        .collect();
    assert_eq!(
        buttons,
        [
            ("Cancel", Some(FormRole::Cancel)),
            ("Use It", Some(FormRole::Default))
        ]
    );
    // It is not drawn in the terminal as well.
    assert!(!harness.screen().contains("Letter for Work"));

    // What will not do is said in the sheet, and the sheet stays.
    press(&mut harness, Some("confirm"), "work");
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        shown.error(),
        Some("The letter is one letter or number, or nothing.")
    );
    assert!(asked(&harness).is_empty());
    // Typing in the sheet takes the complaint away.
    press(&mut harness, None, "w");
    assert_eq!(sheet(&harness).unwrap().error(), None);

    press(&mut harness, Some("confirm"), "k");
    assert_eq!(asked(&harness), [("Work", Some('k'))]);
    assert_eq!(sheet(&harness), None);

    // Cancel asks for nothing.
    harness.keys("Y");
    press(&mut harness, Some("cancel"), "z");
    assert_eq!(sheet(&harness), None);
    assert_eq!(asked(&harness).len(), 1);
}

/// What the settings file is read as saying, for the settings window.
fn settings_read(personal: Option<&str>) -> Box<SettingsRead> {
    let config = quill_core::config::Config::parse("").unwrap();
    Box::new(SettingsRead {
        settings: Settings::of(&config),
        accounts: vec![
            AccountLine {
                name: "Work".to_owned(),
                kind: quill_core::config::AccountType::Graph,
                address: "robin@example.com".to_owned(),
                signs_in: true,
                letter: None,
            },
            AccountLine {
                name: "Personal".to_owned(),
                kind: quill_core::config::AccountType::Imap,
                address: "robin@example.org".to_owned(),
                signs_in: false,
                letter: personal.map(str::to_owned),
            },
        ],
        themes: quill_tui::settings::ThemeLine::builtins(),
        theme_light: Theme::builtin("light").unwrap(),
        theme_light_problem: None,
        theme_dark: Theme::builtin("dark").unwrap(),
        theme_dark_problem: None,
        keymap: Keymap::preset(config.ui.keymap),
        keys_problem: None,
    })
}

#[test]
fn the_settings_window_says_each_accounts_letter_and_has_a_button_to_choose_it() {
    let mut harness = Harness::with_accounts(200, 60);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(settings_read(Some("h"))),
        changed: None,
    }));
    let list = |harness: &Harness| {
        let shown = harness.state.settings_shown(&harness.env).unwrap().1;
        match shown.field("accounts", "accounts").unwrap().clone() {
            FormField::List {
                rows,
                actions,
                help,
                ..
            } => (rows, actions, help.unwrap_or_default()),
            other => panic!("{other:?}"),
        }
    };
    let (rows, actions, help) = list(&harness);
    // An account with no letter of its own says the one it is marked
    // with, and one with its own says that.
    assert!(
        rows[0]
            .detail
            .as_deref()
            .is_some_and(|said| said.ends_with("Letter in lists: W.")),
        "{:?}",
        rows[0].detail
    );
    assert!(
        rows[1]
            .detail
            .as_deref()
            .is_some_and(|said| said.ends_with("Letter in lists: h.")),
        "{:?}",
        rows[1].detail
    );
    // Whichever way an account signs in, it has a letter to choose.
    assert!(
        rows.iter()
            .all(|row| row.actions.iter().any(|action| action == "letter"))
    );
    let letter = actions.iter().find(|action| action.id == "letter").unwrap();
    assert_eq!(
        (letter.title.as_str(), letter.row, letter.enabled),
        ("Letter…", true, true)
    );
    assert!(
        help.contains("Letter… chooses the letter or number"),
        "{help}"
    );

    // Pressed for Personal, it is Personal that is asked about, whatever
    // the accounts pane behind the window is on, and the question starts
    // with the letter the file gives it.
    let personal = rows[1].id.clone();
    let used = SettingsUsed::action("accounts", "accounts", "letter", Some(&personal)).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used.clone())));
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.title(), "Letter for Personal");
    assert!(
        matches!(shown.fields(), [FormField::Text { value, .. }] if value == "h"),
        "{shown:?}"
    );
    // With the question up the button is off.
    assert!(
        !list(&harness)
            .1
            .iter()
            .any(|action| action.id == "letter" && action.enabled)
    );

    // Chosen: the window reads the file again, for its list.
    press(&mut harness, Some("confirm"), "2");
    assert_eq!(asked(&harness), [("Personal", Some('2'))]);
    let reads = |harness: &Harness| {
        harness
            .account_requests
            .iter()
            .filter(|request| matches!(request, AccountRequest::Settings(None)))
            .count()
    };
    let before = reads(&harness);
    set(&mut harness, "Personal", Some('2'));
    assert_eq!(reads(&harness), before + 1);

    // One that could not be kept is said in the window too.
    harness.act(Action::Settings(SettingsAct::Used(used)));
    press(&mut harness, Some("confirm"), "3");
    harness.act(Action::Account(AccountEvent::LetterSet {
        account: "Personal".to_owned(),
        letter: Some('3'),
        result: Err("config.toml cannot be written: Permission denied".to_owned()),
    }));
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    assert!(
        shown
            .problem()
            .is_some_and(|said| said.starts_with("The letter for Personal was not changed.")),
        "{:?}",
        shown.problem()
    );
}

fn found(address: &str) -> AccountEvent {
    AccountEvent::Discovered {
        address: address.to_owned(),
        found: Discovery {
            draft: AccountDraft {
                name: address.to_owned(),
                address: address.to_owned(),
                imap: ServerDraft {
                    host: "imap.example.com".to_owned(),
                    port: 993,
                    tls: TlsMode::Implicit,
                    username: address.to_owned(),
                },
                smtp: None,
            },
            found: true,
            note: None,
            microsoft_application: None,
            microsoft_own: false,
            google_application: None,
            google_own: false,
        },
    }
}

/// The form that adds an account, at its server step with the cursor on
/// the name, for an account named by its address.
fn adding(address: &str) -> Harness {
    let mut harness = Harness::with_accounts(120, 40);
    harness.keys("A");
    harness.type_text(address);
    harness.keys("enter");
    harness.act(Action::Account(found(address)));
    assert!(harness.screen().contains("Letter:"), "{}", harness.screen());
    harness
}

#[test]
fn an_account_is_given_the_letter_typed_as_it_is_added() {
    let mut harness = adding("robin@example.net");
    // The field after the name.
    harness.keys("tab");
    harness.type_text("n");
    harness.keys("enter");
    harness.type_text("a password");
    harness.keys("enter");
    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    // Nothing is asked about a letter until the account is there to have
    // one.
    assert!(asked(&harness).is_empty());
    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "robin@example.net".to_owned(),
    ))));
    assert_eq!(asked(&harness), [("robin@example.net", Some('n'))]);

    // The service's answer is not announced: adding the account was.
    let notices = harness.state.notices.len();
    harness.act(Action::Account(AccountEvent::LetterSet {
        account: "robin@example.net".to_owned(),
        letter: Some('n'),
        result: Ok(()),
    }));
    assert_eq!(harness.state.notices.len(), notices);
}

#[test]
fn an_account_added_with_no_letter_typed_is_asked_for_none() {
    let mut harness = adding("robin@example.net");
    harness.keys("enter");
    harness.type_text("a password");
    harness.keys("enter");
    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "robin@example.net".to_owned(),
    ))));
    assert!(asked(&harness).is_empty());
}

#[test]
fn the_form_goes_no_further_with_a_letter_that_is_not_one() {
    let mut harness = adding("robin@example.net");
    harness.keys("tab");
    harness.type_text("net");
    harness.keys("enter");
    let setup = harness.state.setup.as_ref().unwrap();
    assert_eq!(setup.step, quill_tui::setup::Step::Server);
    assert_eq!(
        setup.error.as_deref(),
        Some("The letter is one letter or number, or nothing.")
    );
    assert_eq!(setup.server_field(), quill_tui::setup::ServerField::Letter);
    // Put right, it goes on.
    harness.keys("backspace backspace enter");
    let setup = harness.state.setup.as_ref().unwrap();
    assert_eq!(setup.step, quill_tui::setup::Step::Password);
}

#[test]
fn snapshot_of_the_question_in_a_terminal() {
    let mut harness = on_work(80, 24);
    harness.keys("Y");
    harness.type_text("7");
    insta::assert_snapshot!("letter_question_80x24", harness.screen());
}
