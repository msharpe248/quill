//! Changing an account, from the UI's side: that the form that adds an
//! account opens on the one picked with its settings in it, that its name
//! and kind are not asked for again, that the password may be left as it
//! is, and that an account the form cannot describe is said so.
//!
//! The service here is the harness: what the UI asked of it is looked at,
//! and its answers are handed in. What the service does about a change is
//! tested where it is, in `quill-cli`.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::config::TlsMode;
use quill_host::bridge::{FormField, FormShow, FormUsed, SettingsUsed};
use quill_tui::command::{CommandId, Context};
use quill_tui::keymap::Keymap;
use quill_tui::settings::{AccountLine, Settings, SettingsRead};
use quill_tui::setup::{
    AccountDraft, AccountEvent, AccountRequest, Secret, ServerDraft, Trusting, VerifyProblem,
};
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

/// Work as the settings file has it: read at one server, sent nowhere.
fn as_it_is() -> AccountDraft {
    AccountDraft {
        name: "Work".to_owned(),
        address: "robin@example.com".to_owned(),
        imap: ServerDraft {
            host: "mail.example.com".to_owned(),
            port: 993,
            tls: TlsMode::Implicit,
            username: "robin@example.com".to_owned(),
        },
        smtp: None,
    }
}

/// The same with the server it is given to send through.
fn with_sending() -> AccountDraft {
    AccountDraft {
        smtp: Some(ServerDraft {
            host: "mail.example.com".to_owned(),
            port: 465,
            tls: TlsMode::Implicit,
            username: "robin@example.com".to_owned(),
        }),
        ..as_it_is()
    }
}

fn edits(harness: &Harness) -> Vec<&AccountRequest> {
    harness
        .account_requests
        .iter()
        .filter(|request| {
            matches!(
                request,
                AccountRequest::EditAsk { .. }
                    | AccountRequest::VerifyChange { .. }
                    | AccountRequest::Change { .. }
            )
        })
        .collect()
}

fn found(harness: &mut Harness, account: &str) {
    harness.act(Action::Account(AccountEvent::EditAsked {
        account: account.to_owned(),
        found: Ok(Box::new(as_it_is())),
    }));
}

fn last_notice(harness: &Harness) -> (NoticeKind, String) {
    let notice = harness.state.notices.back().unwrap();
    (notice.kind, notice.text.clone())
}

#[test]
fn an_account_is_changed_in_the_form_that_adds_one_from_its_servers_on() {
    let mut harness = on_work(120, 40);
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountEdit)
    );
    harness.keys("E");
    // No form until the service has said how the account is set up.
    assert_eq!(
        edits(&harness),
        [&AccountRequest::EditAsk {
            account: "Work".to_owned()
        }]
    );
    assert!(harness.state.setup.is_none());
    // Asked once: the command is off until it is answered.
    harness.keys("E");
    assert_eq!(edits(&harness).len(), 1);

    found(&mut harness, "Work");
    assert_eq!(harness.state.context(), Context::Setup);
    let screen = harness.screen();
    for said in [
        "Edit an account: the server",
        "Work, robin@example.com. Its name and address stay as they are.",
        "mail.example.com",
        "Empty: the account will read mail and not send it.",
    ] {
        assert!(screen.contains(said), "{said}\n{screen}");
    }
    // Its name and its kind are not asked for again.
    assert!(!screen.contains("Type:"), "{screen}");
    assert!(!screen.contains("Name:"), "{screen}");

    // The cursor is on the server; the sending server is four fields on.
    harness.keys("tab tab tab tab");
    harness.type_text("mail.example.com");
    harness.keys("enter");
    let screen = harness.screen();
    assert!(screen.contains("Edit an account: sign in"), "{screen}");
    // Said where the password is typed, in however many lines it takes.
    let note = harness.state.setup.as_ref().unwrap().password_note();
    assert!(
        note.ends_with("Leave it empty to keep the password Quill has."),
        "{note}"
    );
    assert!(screen.contains("Leave it empty"), "{screen}");
    // Left empty: the settings are tried with the password Quill has.
    harness.keys("enter");
    assert_eq!(
        edits(&harness).last().unwrap(),
        &&AccountRequest::VerifyChange {
            account: "Work".to_owned(),
            draft: with_sending(),
            password: None,
            trusting: Trusting::System,
        }
    );
    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    assert_eq!(
        edits(&harness).last().unwrap(),
        &&AccountRequest::Change {
            account: "Work".to_owned(),
            draft: with_sending(),
            password: None,
            trusting: Trusting::System,
        }
    );
    // Nothing is added: no request to save a new account was ever made.
    assert!(!harness.account_requests.iter().any(|request| matches!(
        request,
        AccountRequest::Save { .. } | AccountRequest::Verify { .. }
    )));

    harness.act(Action::Account(AccountEvent::Saved(Ok("Work".to_owned()))));
    assert!(harness.state.setup.is_none());
    assert_eq!(
        last_notice(&harness),
        (NoticeKind::Info, "Saved the changes to Work.".to_owned())
    );
    // And it can be changed again.
    assert!(
        harness
            .state
            .is_enabled(&harness.env, CommandId::AccountEdit)
    );
}

#[test]
fn a_password_typed_goes_with_the_change_and_esc_at_the_servers_puts_the_form_away() {
    let mut harness = on_work(120, 40);
    harness.keys("E");
    found(&mut harness, "Work");
    harness.keys("enter");
    harness.type_text("correct horse");
    harness.keys("enter");
    match edits(&harness).last().unwrap() {
        AccountRequest::VerifyChange {
            account,
            draft,
            password,
            ..
        } => {
            assert_eq!(account, "Work");
            assert_eq!(draft, &as_it_is());
            assert_eq!(password, &Some(Secret("correct horse".to_owned())));
        }
        other => panic!("{other:?}"),
    }
    // Nothing typed and nothing kept: said where the password is typed.
    harness.act(Action::Account(AccountEvent::Verified(Err(
        VerifyProblem::Password(
            "Quill has no password kept for this account. Type it here.".to_owned(),
        ),
    ))));
    let screen = harness.screen();
    assert!(screen.contains("Edit an account: sign in"), "{screen}");
    assert!(screen.contains("Quill has no password kept"), "{screen}");

    // Back to the servers, which is where this form began: one more
    // `esc` puts it away, with nothing asked of the service.
    let asked = harness.account_requests.len();
    harness.keys("esc");
    assert!(
        harness.screen().contains("Edit an account: the server"),
        "{}",
        harness.screen()
    );
    harness.keys("esc");
    assert!(harness.state.setup.is_none());
    assert_eq!(harness.account_requests.len(), asked);
}

#[test]
fn an_account_the_form_cannot_describe_is_said_so_and_no_form_opens() {
    let mut harness = on_work(120, 40);
    harness.keys("E");
    harness.act(Action::Account(AccountEvent::EditAsked {
        account: "Work".to_owned(),
        found: Err(
            "It signs in with its provider, and has no servers or password of its own \
                    to change."
                .to_owned(),
        ),
    }));
    assert!(harness.state.setup.is_none());
    let (kind, text) = last_notice(&harness);
    assert_eq!(kind, NoticeKind::Error);
    assert!(
        text.starts_with("Work cannot be changed here. It signs in with its provider"),
        "{text}"
    );
    // An answer nobody is waiting for opens nothing.
    found(&mut harness, "Work");
    assert!(harness.state.setup.is_none());
}

fn sheet(harness: &Harness) -> FormShow {
    harness.state.form(&harness.env).unwrap()
}

fn field_ids(form: &FormShow) -> Vec<String> {
    form.fields()
        .iter()
        .map(|field| match field {
            FormField::Text { id, .. }
            | FormField::Secret { id, .. }
            | FormField::Choice { id, .. }
            | FormField::Static { id, .. }
            | FormField::List { id, .. } => id.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn button_ids(form: &FormShow) -> Vec<&str> {
    form.buttons()
        .iter()
        .map(|button| button.id.as_str())
        .collect()
}

#[test]
fn in_the_app_it_is_the_sheet_without_a_kind_a_name_or_a_way_back_to_the_address() {
    let mut harness = on_work(160, 40);
    harness.env.sheets = true;
    harness.keys("E");
    found(&mut harness, "Work");
    let form = sheet(&harness);
    assert_eq!(form.step(), "server");
    assert_eq!(form.title(), "Edit an Account: The Server");
    assert_eq!(
        form.note(),
        Some("Work, robin@example.com. Its name and address stay as they are.")
    );
    assert_eq!(
        field_ids(&form),
        [
            "host",
            "port",
            "security",
            "username",
            "smtp_host",
            "smtp_port",
            "smtp_security"
        ]
    );
    // The first step: there is nothing to go back to.
    assert_eq!(button_ids(&form), ["cancel", "continue"]);

    // A sending server typed into the sheet, and on. A name or a kind in
    // the answer is not this form's, and changes nothing.
    harness.act(Action::FormUsed(
        FormUsed::new(
            "server",
            Some("continue"),
            &[
                ("smtp_host", "mail.example.com"),
                ("name", "Somebody Else"),
                ("type", "google"),
            ],
            &[],
        )
        .unwrap(),
    ));
    let form = sheet(&harness);
    assert_eq!(form.step(), "password");
    assert_eq!(button_ids(&form), ["cancel", "back", "continue"]);
    assert!(
        form.note()
            .unwrap()
            .contains("Leave it empty to keep the password Quill has."),
        "{:?}",
        form.note()
    );
    harness.act(Action::FormUsed(
        FormUsed::new("password", Some("continue"), &[], &[]).unwrap(),
    ));
    assert_eq!(
        edits(&harness).last().unwrap(),
        &&AccountRequest::VerifyChange {
            account: "Work".to_owned(),
            draft: with_sending(),
            password: None,
            trusting: Trusting::System,
        }
    );
}

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
            AccountLine {
                name: "Old Mail".to_owned(),
                kind: quill_core::config::AccountType::Pop3,
                address: "robin@example.net".to_owned(),
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
fn the_settings_window_has_an_edit_button_for_an_account_with_servers_of_its_own() {
    let mut harness = Harness::with_accounts(200, 60);
    harness.env.sheets = true;
    harness.act(Action::Settings(SettingsAct::Opened));
    harness.act(Action::Account(AccountEvent::Settings {
        read: Ok(settings_read()),
        changed: None,
    }));
    let shown = harness.state.settings_shown(&harness.env).unwrap().1;
    let (rows, actions) = match shown.field("accounts", "accounts").unwrap().clone() {
        FormField::List { rows, actions, .. } => (rows, actions),
        other => panic!("{other:?}"),
    };
    // Only an account read over IMAP with a password has anything the
    // form can change.
    assert_eq!(rows[0].actions, ["sign_in", "letter", "remove"]);
    assert_eq!(rows[1].actions, ["edit", "letter", "remove"]);
    assert_eq!(rows[2].actions, ["letter", "remove"]);
    let edit = actions.iter().find(|action| action.id == "edit").unwrap();
    assert_eq!(
        (edit.title.as_str(), edit.row, edit.enabled),
        ("Edit…", true, true)
    );

    // Pressed for Personal, it is Personal that is asked about, whatever
    // the accounts pane behind the window is on.
    let personal = rows[1].id.clone();
    let used = SettingsUsed::action("accounts", "accounts", "edit", Some(&personal)).unwrap();
    harness.act(Action::Settings(SettingsAct::Used(used)));
    assert_eq!(
        edits(&harness),
        [&AccountRequest::EditAsk {
            account: "Personal".to_owned()
        }]
    );
    // The form is the sheet, once the service has answered.
    harness.act(Action::Account(AccountEvent::EditAsked {
        account: "Personal".to_owned(),
        found: Ok(Box::new(AccountDraft {
            name: "Personal".to_owned(),
            address: "robin@example.org".to_owned(),
            ..as_it_is()
        })),
    }));
    assert_eq!(sheet(&harness).title(), "Edit an Account: The Server");
}
