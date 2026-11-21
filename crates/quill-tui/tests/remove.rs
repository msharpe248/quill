//! Removing an account, from the UI's side: that it is asked about first,
//! with what would be lost said, that only a deliberate yes sends it to the
//! service, and that the UI lets go of the account when the service says it
//! has gone.
//!
//! The service here is the harness: what the UI asked of it is looked at,
//! and its answers are handed in. What the service does about a removal is
//! tested where it is, in `quill-cli`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_host::bridge::{FormRole, FormShow, FormUsed, SettingsUsed};
use quill_tui::command::{CommandId, Context};
use quill_tui::keymap::Keymap;
use quill_tui::settings::{AccountLine, Settings, SettingsRead};
use quill_tui::setup::{AccountEvent, AccountRequest, Leaving};
use quill_tui::state::{Action, NoticeKind, SettingsAct};
use quill_tui::theme::Theme;

/// The demo mailbox with a service behind it, the accounts pane on Work.
fn on_work(width: u16, height: u16) -> Harness {
    let mut harness = Harness::with_accounts(width, height);
    harness.keys("1 j");
    assert!(
        harness.screen().contains("> ▾ W Work"),
        "{}",
        harness.screen()
    );
    harness
}

fn asked(harness: &Harness) -> Vec<&AccountRequest> {
    harness
        .account_requests
        .iter()
        .filter(|request| {
            matches!(
                request,
                AccountRequest::RemoveAsk { .. } | AccountRequest::Remove { .. }
            )
        })
        .collect()
}

fn leaving() -> Leaving {
    Leaving {
        address: "robin@example.com".to_owned(),
        ..Leaving::default()
    }
}

fn found(harness: &mut Harness, account: &str, leaving: Leaving) {
    harness.act(Action::Account(AccountEvent::RemoveAsked {
        account: account.to_owned(),
        found: Ok(leaving),
    }));
}

/// The service's part, done: the account is gone from the store, and the
/// UI is told.
fn removed(harness: &mut Harness, account: &str) {
    let id = harness
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|row| row.name == account)
        .unwrap()
        .id;
    assert!(harness.store.remove_account(id).unwrap());
    harness.act(Action::Account(AccountEvent::Removed {
        account: account.to_owned(),
        result: Ok(None),
    }));
}

fn sheet(harness: &Harness) -> Option<FormShow> {
    harness.state.form(&harness.env)
}

fn press(harness: &mut Harness, button: &str) {
    let step = sheet(harness).unwrap().step().to_owned();
    harness.act(Action::FormUsed(
        FormUsed::new(&step, Some(button), &[], &[]).unwrap(),
    ));
}

fn last_notice(harness: &Harness) -> (NoticeKind, String) {
    let notice = harness.state.notices.back().unwrap();
    (notice.kind, notice.text.clone())
}

#[test]
fn in_a_terminal_it_is_asked_first_and_only_y_says_yes() {
    let mut harness = on_work(120, 40);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountRemove)
    );
    harness.keys("D");
    // Nothing is asked of the user until the service has said what would
    // be lost, and nothing has been removed.
    assert_eq!(
        asked(&harness),
        [&AccountRequest::RemoveAsk {
            account: "Work".to_owned()
        }]
    );
    assert_eq!(harness.state.context(), Context::Sidebar);
    assert!(harness.state.remove.is_none());
    // And it is asked once: the command is off until it is answered.
    harness.keys("D");
    assert_eq!(asked(&harness).len(), 1);

    found(
        &mut harness,
        "Work",
        Leaving {
            drafts: 1,
            outbox: 2,
            queued: 1,
            ..leaving()
        },
    );
    assert_eq!(harness.state.context(), Context::RemoveAccount);
    let screen = harness.screen();
    for said in [
        "Remove Work?",
        "The account for robin@example.com is taken out of Quill",
        "Mail on its server is not touched",
        "2 messages waiting in the Outbox will not be sent.",
        "1 draft will be deleted.",
        "1 change made here has not reached the server yet and will be",
        "y remove it",
        "esc keep it",
    ] {
        assert!(screen.contains(said), "{said}: {screen}");
    }

    // Neither enter nor any other letter is a yes, and none of them
    // reaches what is behind the question.
    let (selected, mailbox) = (harness.state.list.selected, harness.state.mailbox);
    harness.keys("enter j d Y n space");
    assert_eq!(harness.state.context(), Context::RemoveAccount);
    assert_eq!(asked(&harness).len(), 1);
    assert_eq!(
        (harness.state.list.selected, harness.state.mailbox),
        (selected, mailbox)
    );
    // esc keeps the account: nothing more was asked of the service.
    harness.keys("esc");
    assert!(harness.state.remove.is_none());
    assert_eq!(asked(&harness).len(), 1);
    assert_eq!(harness.store.accounts().unwrap().len(), 2);

    // Asked again, and answered yes: the service is told to, once.
    harness.keys("D");
    found(&mut harness, "Work", leaving());
    assert!(!harness.screen().contains("will not be sent"));
    harness.keys("y");
    assert_eq!(
        asked(&harness).last(),
        Some(&&AccountRequest::Remove {
            account: "Work".to_owned()
        })
    );
    assert_eq!(asked(&harness).len(), 3);
    assert!(
        harness.screen().contains("Removing…"),
        "{}",
        harness.screen()
    );
    // While it is at it there is no saying yes again, and no going back.
    harness.keys("y esc");
    assert_eq!(asked(&harness).len(), 3);
    assert!(
        harness
            .state
            .remove
            .as_ref()
            .is_some_and(|prompt| prompt.busy)
    );
}

#[test]
fn when_it_has_gone_the_ui_lets_go_of_it() {
    let mut harness = on_work(120, 40);
    // On the account's own inbox, which will not be there.
    harness.keys("enter");
    assert_eq!(harness.state.mailbox_title(), "Inbox (Work)");
    harness.keys("1 D");
    found(&mut harness, "Work", leaving());
    harness.keys("y");
    removed(&mut harness, "Work");

    assert!(harness.state.remove.is_none());
    assert_eq!(harness.state.context(), Context::Sidebar);
    assert_eq!(
        last_notice(&harness),
        (NoticeKind::Info, "Removed Work.".to_owned())
    );
    // The accounts pane and the list are what the store now has: the
    // other account, and every inbox that is left.
    let screen = harness.screen();
    assert!(!screen.contains("▾ W Work"), "{screen}");
    assert!(screen.contains("▾ P Personal"), "{screen}");
    assert!(screen.contains("Removed Work."), "{screen}");
    assert_eq!(harness.state.mailbox_title(), "Inbox (unified)");
    assert_eq!(harness.state.accounts.len(), 1);
    assert!(
        harness
            .state
            .list
            .rows
            .iter()
            .all(|row| row.account == harness.state.accounts[0].id)
    );
    // What could not be cleared up after it is said with it.
    let mut harness = on_work(120, 40);
    harness.keys("D");
    found(&mut harness, "Work", leaving());
    harness.keys("y");
    let id = harness.store.accounts().unwrap()[0].id;
    harness.store.remove_account(id).unwrap();
    harness.act(Action::Account(AccountEvent::Removed {
        account: "Work".to_owned(),
        result: Ok(Some(
            "Its password could not be taken out of the Keychain.".to_owned(),
        )),
    }));
    assert_eq!(
        last_notice(&harness),
        (
            NoticeKind::Info,
            "Removed Work. Its password could not be taken out of the Keychain.".to_owned()
        )
    );
}

#[test]
fn an_account_that_could_not_be_removed_is_still_there_and_why_is_said() {
    let mut harness = on_work(120, 40);
    harness.keys("D");
    // The service cannot even say what would be lost: no question.
    harness.act(Action::Account(AccountEvent::RemoveAsked {
        account: "Work".to_owned(),
        found: Err("The settings file has a mistake to fix first: line 3.".to_owned()),
    }));
    assert!(harness.state.remove.is_none());
    assert_eq!(
        last_notice(&harness),
        (
            NoticeKind::Error,
            "Work was not removed. The settings file has a mistake to fix first: line 3."
                .to_owned()
        )
    );
    // It said yes and the file could not be written: the question goes,
    // the account stays, and it can be asked again.
    harness.keys("D");
    found(&mut harness, "Work", leaving());
    harness.keys("y");
    harness.act(Action::Account(AccountEvent::Removed {
        account: "Work".to_owned(),
        result: Err("The settings file cannot be written: Permission denied".to_owned()),
    }));
    assert!(harness.state.remove.is_none());
    assert_eq!(last_notice(&harness).0, NoticeKind::Error);
    assert!(last_notice(&harness).1.starts_with("Work was not removed."));
    assert_eq!(harness.state.accounts.len(), 2);
    assert!(harness.screen().contains("Work"));
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountRemove)
    );
    // An answer nobody is waiting for asks nothing.
    found(&mut harness, "Personal", leaving());
    assert!(harness.state.remove.is_none());
}

#[test]
fn it_is_the_account_the_accounts_pane_is_on_and_not_in_the_middle_of_anything() {
    let enabled = |harness: &Harness| {
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountRemove)
    };
    // On every inbox there is no one account to mean.
    let mut harness = Harness::with_accounts(120, 40);
    assert!(!enabled(&harness));
    harness.keys("D");
    assert!(asked(&harness).is_empty());
    // On one of an account's folders it is that account.
    harness.keys("1 j j");
    assert!(enabled(&harness));
    harness.keys("D");
    assert_eq!(
        asked(&harness),
        [&AccountRequest::RemoveAsk {
            account: "Work".to_owned()
        }]
    );
    // The demo mailbox has no accounts of the user's to remove.
    let mut demo = Harness::demo(120, 40);
    demo.keys("1 j");
    assert!(!enabled(&demo));
    demo.keys("D");
    assert!(asked(&demo).is_empty());
    // Nor while a message is being written: it is being written from one.
    let mut writing = on_work(120, 40);
    writing.keys("2 c");
    assert_eq!(writing.state.context(), Context::Compose);
    assert!(!writing.state.can_remove_account(&writing.env));
    // It is a command like any other: in the list of them, with its key.
    let spec = harness
        .env
        .registry
        .by_name("account.remove")
        .expect("the command is registered");
    assert_eq!(spec.id, CommandId::AccountRemove);
    for preset in [
        quill_core::config::KeymapPreset::Vim,
        quill_core::config::KeymapPreset::Mutt,
    ] {
        let keys: Vec<String> = Keymap::preset(preset)
            .keys_for(CommandId::AccountRemove)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(keys, ["D"], "{preset:?}");
    }
}

#[test]
fn a_pop3_accounts_mail_is_said_to_be_the_only_copy() {
    let mut harness = on_work(120, 40);
    harness.keys("D");
    found(
        &mut harness,
        "Work",
        Leaving {
            only_copy: true,
            ..leaving()
        },
    );
    let prompt = harness.state.remove.clone().unwrap();
    assert!(prompt.loses_something());
    assert!(prompt.keeps().contains("may be the only one"));
    let screen = harness.screen();
    assert!(screen.contains("POP3"), "{screen}");
    assert!(
        !screen.contains("Mail on its server is not touched"),
        "{screen}"
    );
    // An account with nothing of its own here loses nothing but the copy.
    harness.keys("esc D");
    found(&mut harness, "Work", leaving());
    assert!(!harness.state.remove.as_ref().unwrap().loses_something());
}

#[test]
fn in_the_app_it_is_a_sheet_with_no_button_for_return() {
    let mut harness = on_work(120, 40);
    harness.env.sheets = true;
    harness.keys("D");
    found(
        &mut harness,
        "Work",
        Leaving {
            outbox: 1,
            ..leaving()
        },
    );
    let form = sheet(&harness).expect("the question is the host's to show");
    assert_eq!(form.step(), "remove_account");
    assert_eq!(form.title(), "Remove Work?");
    let note = form.note().unwrap();
    assert!(note.contains("is taken out of Quill"), "{note}");
    assert!(note.contains("Mail on its server is not touched"), "{note}");
    assert!(
        note.ends_with("1 message waiting in the Outbox will not be sent."),
        "{note}"
    );
    assert!(form.fields().is_empty());
    let buttons: Vec<(&str, &str, Option<FormRole>, bool)> = form
        .buttons()
        .iter()
        .map(|button| {
            (
                button.id.as_str(),
                button.title.as_str(),
                button.role,
                button.enabled,
            )
        })
        .collect();
    // esc cancels. Nothing is the button return presses.
    assert_eq!(
        buttons,
        [
            ("cancel", "Cancel", Some(FormRole::Cancel), true),
            ("confirm", "Remove", None, true),
        ]
    );
    // Nothing of it is drawn in the terminal, and a letter typed there is
    // no answer to a question that is not on that screen.
    assert!(!harness.screen().contains("Remove Work?"));
    harness.keys("y");
    assert_eq!(asked(&harness).len(), 1);

    // Cancel keeps the account.
    press(&mut harness, "cancel");
    assert!(sheet(&harness).is_none());
    assert_eq!(asked(&harness).len(), 1);

    // Remove removes it, once, and the sheet says so until it is done.
    harness.keys("D");
    found(&mut harness, "Work", leaving());
    press(&mut harness, "confirm");
    assert_eq!(
        asked(&harness).last(),
        Some(&&AccountRequest::Remove {
            account: "Work".to_owned()
        })
    );
    let form = sheet(&harness).unwrap();
    assert_eq!(form.busy(), Some("Removing…"));
    assert!(form.buttons().iter().all(|button| !button.enabled));
    press(&mut harness, "confirm");
    press(&mut harness, "cancel");
    assert_eq!(asked(&harness).len(), 3);
    assert!(sheet(&harness).is_some());
    removed(&mut harness, "Work");
    assert!(sheet(&harness).is_none());
}

/// What the settings file is read as saying, for the settings window.
fn settings_read() -> Box<SettingsRead> {
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
                letter: None,
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
fn the_settings_window_has_a_remove_button_for_the_account_picked() {
    use quill_host::bridge::FormField;

    let mut harness = Harness::with_accounts(200, 60);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(settings_read()),
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
    // Every account can be removed, whichever way it signs in, and the
    // button is one for the row picked.
    assert_eq!(rows[0].actions, ["sign_in", "letter", "remove"]);
    assert_eq!(rows[1].actions, ["edit", "letter", "remove"]);
    let remove = actions.iter().find(|action| action.id == "remove").unwrap();
    assert_eq!(
        (remove.title.as_str(), remove.row, remove.enabled),
        ("Remove…", true, true)
    );
    assert!(help.contains("Mail on its server is not touched"), "{help}");

    // Pressed for Personal, it is Personal that is asked about, whatever
    // the accounts pane behind the window is on.
    let personal = rows[1].id.clone();
    let used = SettingsUsed::action("accounts", "accounts", "remove", Some(&personal)).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used.clone())));
    assert_eq!(
        asked(&harness),
        [&AccountRequest::RemoveAsk {
            account: "Personal".to_owned()
        }]
    );
    found(&mut harness, "Personal", leaving());
    assert_eq!(sheet(&harness).unwrap().title(), "Remove Personal?");
    // With the question up the button is off, and pressing asks no more.
    assert!(
        !list(&harness)
            .1
            .iter()
            .any(|action| action.id == "remove" && action.enabled)
    );
    harness.act(Action::Settings(SettingsAct::Used(used)));
    assert_eq!(asked(&harness).len(), 1);

    // Removed: the window reads the file again, for its list.
    press(&mut harness, "confirm");
    let reads = |harness: &Harness| {
        harness
            .account_requests
            .iter()
            .filter(|request| matches!(request, AccountRequest::Settings(None)))
            .count()
    };
    let before = reads(&harness);
    removed(&mut harness, "Personal");
    assert_eq!(reads(&harness), before + 1);

    // One that could not be removed is said in the window too.
    let work = rows[0].id.clone();
    let used = SettingsUsed::action("accounts", "accounts", "remove", Some(&work)).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used)));
    found(&mut harness, "Work", leaving());
    press(&mut harness, "confirm");
    harness.act(Action::Account(AccountEvent::Removed {
        account: "Work".to_owned(),
        result: Err("The settings file cannot be written: Permission denied".to_owned()),
    }));
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    assert!(
        shown
            .problem()
            .is_some_and(|said| said.starts_with("Work was not removed.")),
        "{:?}",
        shown.problem()
    );
    // A row there is not, or no row at all, is nothing to remove.
    for row in [Some("account_nobody"), None] {
        let used = SettingsUsed::action("accounts", "accounts", "remove", row).unwrap();
        harness.act(Action::Settings(SettingsAct::Used(used)));
    }
    assert_eq!(asked(&harness).len(), 4);
}

#[test]
fn the_question_as_a_terminal_draws_it() {
    let mut harness = on_work(80, 24);
    harness.keys("D");
    found(
        &mut harness,
        "Work",
        Leaving {
            drafts: 2,
            outbox: 1,
            queued: 3,
            ..leaving()
        },
    );
    insta::assert_snapshot!("remove_account_question_80x24", harness.screen());
}
