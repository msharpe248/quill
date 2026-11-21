//! Adding an account in Quill's own app, where the form is the host's to
//! show, in a sheet on its window. It is the same form as in a terminal:
//! what is checked here is which of the two shows it, that the terminal
//! draws nothing of it when the host does, and that what the user does in
//! the sheet ends where the keys do.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::CertificateProblem;
use quill_core::config::TlsMode;
use quill_host::bridge::{FormField, FormShow, FormUsed};
use quill_sync::SyncEvent;
use quill_tui::command::{CommandId, Context};
use quill_tui::setup::{
    AccountDraft, AccountEvent, AccountRequest, Discovery, Press, ServerDraft, SignInDraft, Step,
};
use quill_tui::state::{Action, HostRequest, Mouse, MouseKind};

fn found(address: &str, host: &str) -> AccountEvent {
    AccountEvent::Discovered {
        address: address.to_owned(),
        found: Discovery {
            draft: AccountDraft {
                name: "Example".to_owned(),
                address: address.to_owned(),
                imap: ServerDraft {
                    host: host.to_owned(),
                    port: 993,
                    tls: TlsMode::Implicit,
                    username: address.to_owned(),
                },
                smtp: None,
            },
            found: true,
            note: Some("Found in the DNS records of example.com.".to_owned()),
            microsoft_application: None,
            microsoft_own: false,
            google_application: None,
            google_own: false,
        },
    }
}

/// The demo mailbox in Quill's own app, with a service to add accounts
/// through.
fn in_the_app(width: u16, height: u16) -> Harness {
    let mut harness = Harness::with_accounts(width, height);
    harness.env.sheets = true;
    harness
}

/// The form as the host is told to show it.
fn sheet(harness: &Harness) -> Option<FormShow> {
    harness.state.form(&harness.env)
}

fn ids(form: &FormShow) -> Vec<&str> {
    form.fields().iter().map(FormField::id).collect()
}

/// What the sheet sends: a button, or none for a changed choice.
fn press(
    harness: &mut Harness,
    button: Option<Press>,
    values: &[(&str, &str)],
    secrets: &[(&str, &str)],
) {
    let step = sheet(harness).unwrap().step().to_owned();
    let used = FormUsed::new(&step, button.map(Press::id), values, secrets).unwrap();
    harness.act(Action::FormUsed(used));
}

/// The panes, without the status bar under them.
fn panes(harness: &Harness) -> String {
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    rows[..rows.len() - 1].join("\n")
}

/// The status bar.
fn status(harness: &Harness) -> String {
    harness
        .screen()
        .lines()
        .last()
        .unwrap_or_default()
        .to_owned()
}

/// Where the terminal draws a form or a question over the panes.
fn dialog(harness: &Harness) -> Option<ratatui::layout::Rect> {
    quill_tui::layout::compute(&harness.state, harness.state.size, 0.5).dialog
}

#[test]
fn in_a_terminal_the_form_is_drawn_and_no_host_is_told_of_it() {
    let mut harness = Harness::with_accounts(120, 30);
    assert!(!harness.env.sheets);
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::Setup);
    assert!(harness.screen().contains(" Add an account "));
    assert!(dialog(&harness).is_some());
    assert_eq!(sheet(&harness), None);
    // At every step.
    harness.type_text("robin@example.com");
    harness.keys("enter");
    harness.act(Action::Account(found(
        "robin@example.com",
        "imap.example.com",
    )));
    assert!(harness.screen().contains(" Add an account: the server "));
    assert_eq!(sheet(&harness), None);
    harness.keys("enter");
    assert!(harness.screen().contains(" Add an account: sign in "));
    assert_eq!(sheet(&harness), None);
    assert!(!harness.state.form_hosted && !harness.state.form_in_sheet());
    // And the status bar names the keys that work it.
    assert!(status(&harness).contains("enter continue"));
}

#[test]
fn in_the_app_the_form_is_the_hosts_and_the_terminal_draws_none_of_it() {
    let mut harness = in_the_app(120, 30);
    let behind = panes(&harness);
    assert_eq!(sheet(&harness), None);

    harness.keys("A");
    // The form is open, and it is what has the keyboard as far as the
    // host's menus go.
    assert_eq!(harness.state.context(), Context::Setup);
    assert_eq!(
        harness.state.host_state(&harness.env).context,
        Context::Setup
    );
    let shown = sheet(&harness).unwrap();
    assert_eq!((shown.step(), shown.title()), ("address", "Add an Account"));
    assert_eq!(ids(&shown), ["address"]);
    // Nothing of it is drawn: the panes behind are as they were.
    assert_eq!(dialog(&harness), None);
    assert_eq!(panes(&harness), behind);
    assert_eq!(harness.cursor(), None);
    // The status bar names no keys: the sheet has buttons of its own, and
    // the keys that work the form in a terminal are not pressed there.
    assert_eq!(status(&harness).trim(), "");

    // The menu and the toolbar open the same form: it is the one command.
    let mut by_menu = in_the_app(120, 30);
    by_menu.act(Action::Command(CommandId::AccountAdd));
    assert_eq!(sheet(&by_menu), Some(shown));
}

#[test]
fn a_first_run_in_the_app_opens_the_sheet_by_itself_once() {
    let mut harness = Harness::first_run_in_the_app(120, 30);
    assert_eq!(harness.state.context(), Context::Setup);
    assert_eq!(sheet(&harness).unwrap().step(), "address");
    assert!(!harness.screen().contains("Add an Account"));
    assert!(!harness.screen().contains("Type the email address"));

    // Cancelled is cancelled: the host is told to put it away, and it
    // does not come back on the next refresh.
    press(&mut harness, Some(Press::Cancel), &[], &[]);
    assert_eq!(harness.state.context(), Context::List);
    assert_eq!(sheet(&harness), None);
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    assert_eq!(sheet(&harness), None);
    // It is still there to ask for.
    harness.keys("A");
    assert_eq!(sheet(&harness).unwrap().step(), "address");
}

#[test]
fn a_password_account_is_added_from_the_sheet() {
    let mut harness = in_the_app(120, 30);
    let behind = panes(&harness);
    harness.keys("A");
    press(
        &mut harness,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Discover {
            address: "robin@example.com".to_owned()
        }]
    );
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        shown.busy(),
        Some("Looking for the mail server of example.com…")
    );
    // Not drawn in the terminal either, where a wait is said in the form.
    assert_eq!(panes(&harness), behind);

    harness.act(Action::Account(found(
        "robin@example.com",
        "imap.example.com",
    )));
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.step(), "server");
    assert_eq!(shown.busy(), None);
    assert_eq!(
        ids(&shown),
        [
            "type",
            "name",
            "letter",
            "host",
            "port",
            "security",
            "username",
            "smtp_host",
            "smtp_port",
            "smtp_security"
        ]
    );

    press(
        &mut harness,
        Some(Press::Continue),
        &[("name", "Work")],
        &[],
    );
    assert_eq!(sheet(&harness).unwrap().step(), "password");
    press(
        &mut harness,
        Some(Press::Continue),
        &[],
        &[("password", "hunter2")],
    );
    let Some(AccountRequest::Verify {
        draft, password, ..
    }) = harness.account_requests.last()
    else {
        panic!("{:?}", harness.account_requests);
    };
    assert_eq!(draft.name, "Work");
    assert_eq!(password.0, "hunter2");
    // The password is nowhere it could be read back: not in what the host
    // is told to show, not on the screen, and not in the state printed
    // whole.
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.busy(), Some("Signing in to imap.example.com…"));
    let carried = Action::FormUsed(
        FormUsed::new(
            "password",
            Some("continue"),
            &[],
            &[("password", "hunter2")],
        )
        .unwrap(),
    );
    for written in [
        format!("{carried:?}"),
        format!("{shown:?}"),
        harness.screen(),
        format!("{:?}", harness.state.setup),
        format!("{:?}", harness.account_requests),
    ] {
        assert!(!written.contains("hunter2"), "{written}");
    }

    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    assert!(matches!(
        harness.account_requests.last(),
        Some(AccountRequest::Save { .. })
    ));
    harness.act(Action::Account(AccountEvent::Saved(Ok("Work".to_owned()))));
    // Over: the host is told to put the sheet away, and the terminal says
    // what became of it.
    assert!(harness.state.setup.is_none());
    assert_eq!(sheet(&harness), None);
    assert!(
        harness
            .screen()
            .contains("Added Work. Its mail is on the way.")
    );
}

#[test]
fn a_microsoft_account_goes_from_the_form_to_the_sign_in_in_the_same_sheet() {
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    press(
        &mut harness,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    harness.act(Action::Account(found(
        "robin@example.com",
        "outlook.office365.com",
    )));
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.title(), "Add an Account: Microsoft");
    assert_eq!(
        ids(&shown),
        ["type", "name", "letter", "address", "application", "tenant"]
    );

    press(
        &mut harness,
        Some(Press::Continue),
        &[
            ("type", "microsoft"),
            ("name", "Example"),
            ("application", "11111111-2222-3333-4444-555555555555"),
            ("tenant", ""),
        ],
        &[],
    );
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::SaveSignIn {
            draft: SignInDraft {
                name: "Example".to_owned(),
                address: "robin@example.com".to_owned(),
                application_id: "11111111-2222-3333-4444-555555555555".to_owned(),
                tenant: None,
                google: None,
            }
        })
    );
    assert_eq!(sheet(&harness).unwrap().busy(), Some("Saving Example…"));

    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "Example".to_owned()
    ))));
    // The form is over and the sign-in takes its place in the sheet, in
    // the one update: the host is never told to put the sheet away in
    // between, so there is no moment with nothing on the window.
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        (shown.step(), shown.title()),
        ("sign_in", "Sign In to Example")
    );
    assert_eq!(harness.state.context(), Context::SignIn);
    // Nothing of it is drawn in the terminal.
    assert_eq!(dialog(&harness), None);
    assert!(!harness.screen().contains("Sign in to Example"));
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::SignIn {
            account: Some("Example".to_owned())
        })
    );
    harness.act(Action::Account(AccountEvent::SignInBrowser {
        account: "Example".to_owned(),
        url: "https://login.example.com/authorize?client_id=1".to_owned(),
    }));
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::OpenUrl(
            "https://login.example.com/authorize?client_id=1".to_owned()
        ))
    );
    let shown = sheet(&harness).unwrap();
    assert!(
        shown
            .note()
            .is_some_and(|note| note.contains("Your browser has been opened"))
    );
    assert!(!harness.screen().contains("Your browser has been opened"));
}

#[test]
fn changing_the_type_in_the_sheet_changes_what_the_host_is_told_to_show() {
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    press(
        &mut harness,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    harness.act(Action::Account(found(
        "robin@example.com",
        "imap.example.com",
    )));
    let before = sheet(&harness).unwrap();
    // The pop-up changed: no button was pressed.
    press(&mut harness, None, &[("type", "microsoft")], &[]);
    let after = sheet(&harness).unwrap();
    assert_ne!(before, after);
    assert_eq!(
        ids(&after),
        ["type", "name", "letter", "address", "application", "tenant"]
    );
    assert!(harness.account_requests.len() == 1);
    assert_eq!(
        harness.state.setup.as_ref().map(|setup| setup.step),
        Some(Step::Server)
    );
}

#[test]
fn a_question_that_comes_first_takes_the_sheet_and_the_form_comes_back_after() {
    let mut harness = in_the_app(120, 40);
    harness.keys("A");
    press(
        &mut harness,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    harness.act(Action::Account(found(
        "robin@example.com",
        "imap.example.com",
    )));
    assert_eq!(sheet(&harness).unwrap().step(), "server");

    // A certificate of an account already there is asked about before
    // anything else, as in a terminal. The sheet is the question's for as
    // long as it is asked: the host is given it in the form's place.
    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: Box::new(CertificateProblem {
            host: "imap.example.com".to_owned(),
            reason: "UnknownIssuer".to_owned(),
            fingerprint: Some("0123456789abcdef".repeat(4)),
            certificate: None,
            details: None,
        }),
        sending: false,
    }));
    assert_eq!(harness.state.context(), Context::Certificate);
    let asked = sheet(&harness).unwrap();
    assert!(asked.step().starts_with("account_certificate_"));
    assert_eq!(asked.title(), "Work: Certificate");
    assert_eq!(dialog(&harness), None);
    assert!(!harness.screen().contains("Work: certificate"));
    // The form is not answered while it is not what is in front.
    let stale = FormUsed::new("server", Some("continue"), &[("name", "Late")], &[]).unwrap();
    harness.act(Action::FormUsed(stale));
    assert_eq!(
        harness.state.setup.as_ref().map(|setup| setup.step),
        Some(Step::Server)
    );
    assert_eq!(harness.state.context(), Context::Certificate);

    // Answered, the form is the host's to show again, where it was.
    press(&mut harness, Some(Press::Reject), &[], &[]);
    assert_eq!(harness.state.context(), Context::Setup);
    assert_eq!(sheet(&harness).unwrap().step(), "server");
    assert_eq!(dialog(&harness), None);
}

#[test]
fn nothing_behind_the_sheet_moves() {
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    let selected = harness.state.list.selected;
    let areas = quill_tui::layout::compute(&harness.state, harness.state.size, 0.5);
    let list = areas.list.unwrap();
    // The wheel still turns over a window with a sheet on it, and a click
    // may be on its way from before the sheet came down.
    for kind in [MouseKind::WheelDown, MouseKind::WheelDown, MouseKind::Press] {
        harness.act(Action::Mouse(Mouse {
            kind,
            column: list.x + 2,
            row: list.y + 4,
        }));
    }
    assert_eq!(harness.state.list.selected, selected);
    assert_eq!(harness.state.context(), Context::Setup);
    assert_eq!(sheet(&harness).unwrap().step(), "address");
}

#[test]
fn keys_that_reach_the_terminal_as_the_sheet_opens_are_typed_into_the_form() {
    // Typed ahead of the sheet: the keys go to the one form there is, and
    // the host is told what the field now holds.
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    harness.type_text("robin@");
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.fields()[0].value(), Some("robin@"));
    // The list behind did not take them for commands.
    assert_eq!(harness.state.context(), Context::Setup);
}

#[test]
fn the_form_in_the_sheet_is_not_worked_from_the_terminal_or_the_menus() {
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    // What the sheet's field holds is the sheet's until a button sends
    // it: `enter` here, or Open in the Message menu, would go on with
    // what the core holds, which is nothing yet.
    for id in [
        CommandId::NavOpen,
        CommandId::NavNext,
        CommandId::NavPrev,
        CommandId::FocusNext,
        CommandId::FocusPrev,
    ] {
        assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
        harness.act(Action::Command(id));
    }
    harness.keys("enter tab down");
    assert!(harness.account_requests.is_empty());
    let shown = sheet(&harness).unwrap();
    assert_eq!((shown.step(), shown.error()), ("address", None));
    // The host's menus hear the same.
    let enabled = harness.state.host_state(&harness.env).enabled;
    assert!(!enabled.contains(&CommandId::NavOpen));
    // `esc` is still the way out, as it is of every question.
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavBack));
    harness.keys("esc");
    assert_eq!(sheet(&harness), None);
    assert_eq!(harness.state.context(), Context::List);

    // In a terminal they are the keys that work the form.
    let mut harness = Harness::with_accounts(120, 30);
    harness.keys("A");
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavOpen));
    assert!(harness.state.is_enabled(&harness.env, CommandId::FocusNext));
}

#[test]
fn an_account_saved_after_the_form_was_closed_is_not_added_unseen() {
    let mut harness = in_the_app(120, 30);
    harness.keys("A");
    press(
        &mut harness,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    harness.act(Action::Account(found(
        "robin@example.com",
        "outlook.office365.com",
    )));
    press(
        &mut harness,
        Some(Press::Continue),
        &[("application", "app-1")],
        &[],
    );
    assert_eq!(sheet(&harness).unwrap().busy(), Some("Saving Example…"));
    // Cancelled while the account was being written.
    press(&mut harness, Some(Press::Cancel), &[], &[]);
    assert_eq!(sheet(&harness), None);
    assert_eq!(harness.state.context(), Context::List);
    // It was written all the same, and that is said.
    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "Example".to_owned()
    ))));
    assert!(
        harness
            .screen()
            .contains("Added Example: it was already being saved when the form was closed.")
    );
    // Nothing is signed in to behind the user's back.
    assert!(
        !harness
            .account_requests
            .iter()
            .any(|request| matches!(request, AccountRequest::SignIn { .. }))
    );
    // A save that failed after the form was closed added nothing, and
    // says nothing.
    let notices = harness.state.notices.len();
    harness.act(Action::Account(AccountEvent::Saved(Err("no".to_owned()))));
    assert_eq!(harness.state.notices.len(), notices);
}
