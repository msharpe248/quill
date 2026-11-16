//! The questions Quill asks, in Quill's own app: a sign-in, the certificate
//! of an account that is already there, a time to snooze until or to send
//! at, and unsubscribing. In a terminal each is drawn over the panes. In
//! the app each is the host's to show, in a sheet on its window, as the
//! form that adds an account is (`sheet.rs`).
//!
//! They are the same questions. What is checked here is what the host is
//! told to show for each of them in each of its states, that an answer from
//! the sheet does exactly what the keys do in a terminal, that the terminal
//! draws nothing of a question the host shows, and which question has the
//! one sheet when there are several.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Harness, NOW};
use quill_core::types::{
    Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_core::{CertificateDetails, CertificateProblem};
use quill_host::bridge::{self, CoreToShell, FormField, FormRole, FormShow, FormUsed, Secret};
use quill_store::{DraftState, Mailbox};
use quill_sync::SyncEvent;
use quill_tui::command::{CommandId, Context};
use quill_tui::data::DataEvent;
use quill_tui::setup::{AccountEvent, AccountRequest, SignInStage, VerifyProblem};
use quill_tui::state::{Action, HostRequest, NoticeKind};

/// The demo mailbox with a service to add accounts through: in Quill's own
/// app, or in a terminal.
fn mailbox(in_the_app: bool) -> Harness {
    let mut harness = Harness::with_accounts(120, 40);
    harness.env.sheets = in_the_app;
    harness
}

/// The demo mailbox itself, which has no such service.
fn demo(in_the_app: bool) -> Harness {
    let mut harness = Harness::demo(120, 40);
    harness.env.sheets = in_the_app;
    harness
}

/// The form as the host is told to show it.
fn sheet(harness: &Harness) -> Option<FormShow> {
    harness.state.form(&harness.env)
}

fn ids(form: &FormShow) -> Vec<&str> {
    form.fields().iter().map(FormField::id).collect()
}

fn field<'a>(form: &'a FormShow, id: &str) -> &'a FormField {
    form.fields()
        .iter()
        .find(|field| field.id() == id)
        .unwrap_or_else(|| panic!("no field {id} in {:?}", ids(form)))
}

/// The buttons from left to right: name, title, the key that presses it,
/// and whether it can be pressed.
fn buttons(form: &FormShow) -> Vec<(&str, &str, Option<FormRole>, bool)> {
    form.buttons()
        .iter()
        .map(|button| {
            (
                button.id.as_str(),
                button.title.as_str(),
                button.role,
                button.enabled,
            )
        })
        .collect()
}

/// What the sheet sends: a button, or none for a changed choice, with what
/// its fields held.
fn press(harness: &mut Harness, button: Option<&str>, values: &[(&str, &str)]) {
    let step = sheet(harness).unwrap().step().to_owned();
    harness.act(Action::FormUsed(
        FormUsed::new(&step, button, values, &[]).unwrap(),
    ));
}

/// The panes, without the status bar under them.
fn panes(harness: &Harness) -> String {
    let screen = harness.screen();
    let rows: Vec<&str> = screen.lines().collect();
    rows[..rows.len() - 1].join("\n")
}

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

fn subjects(harness: &Harness) -> Vec<String> {
    harness
        .state
        .list
        .rows
        .iter()
        .map(|row| row.headers.subject.clone())
        .collect()
}

fn notices(harness: &Harness) -> Vec<(NoticeKind, String)> {
    harness
        .state
        .notices
        .iter()
        .map(|notice| (notice.kind, notice.text.clone()))
        .collect()
}

/// The question is the host's: it is described, nothing of it is drawn,
/// the status bar names none of its keys, and the keys and menu items that
/// answer it in a terminal are off.
fn is_the_hosts(harness: &Harness, behind: &str) {
    assert!(sheet(harness).is_some());
    assert!(harness.state.form_in_sheet());
    assert_eq!(dialog(harness), None);
    assert_eq!(panes(harness), behind);
    assert_eq!(harness.cursor(), None);
    assert_eq!(status(harness).trim(), "");
    for id in [
        CommandId::NavOpen,
        CommandId::NavNext,
        CommandId::NavPrev,
        CommandId::FocusNext,
        CommandId::FocusPrev,
    ] {
        assert!(!harness.state.is_enabled(&harness.env, id), "{id:?}");
    }
    assert!(
        !harness
            .state
            .host_state(&harness.env)
            .enabled
            .contains(&CommandId::NavOpen)
    );
    // `esc` is still the way out, as it is of every question.
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavBack));
}

/// The question is drawn in the terminal, and no host is told of it.
fn is_the_terminals(harness: &Harness) {
    assert_eq!(sheet(harness), None);
    assert!(!harness.state.form_in_sheet());
    assert!(dialog(harness).is_some());
}

// ---------------------------------------------------------------------
// Signing in
// ---------------------------------------------------------------------

const AUTHORIZE: &str = "https://login.example.com/authorize?client_id=1&state=s3cr3t";

fn browser(account: &str) -> Action {
    Action::Account(AccountEvent::SignInBrowser {
        account: account.to_owned(),
        url: AUTHORIZE.to_owned(),
    })
}

fn with_a_code(account: &str) -> Action {
    Action::Account(AccountEvent::SignInCode {
        account: account.to_owned(),
        address: "https://login.example.com/device".to_owned(),
        code: "HJKL-MNPQ".to_owned(),
        minutes: 15,
    })
}

const CANCEL: (&str, &str, Option<FormRole>, bool) =
    ("cancel", "Cancel", Some(FormRole::Cancel), true);

#[test]
fn a_sign_in_is_a_sheet_that_says_what_is_happening_at_each_stage() {
    let mut harness = mailbox(true);
    let behind = panes(&harness);
    harness.keys("S");
    assert_eq!(harness.state.context(), Context::SignIn);
    assert!(matches!(
        harness.account_requests.as_slice(),
        [AccountRequest::SignIn { .. }]
    ));

    // Asked for, and nothing heard yet: whose it is is not known.
    let shown = sheet(&harness).unwrap();
    assert_eq!((shown.step(), shown.title()), ("sign_in", "Sign In"));
    assert_eq!(shown.note(), None);
    assert_eq!(shown.busy(), Some("Getting ready to sign in..."));
    assert!(shown.fields().is_empty());
    assert_eq!(buttons(&shown), [CANCEL]);
    is_the_hosts(&harness, &behind);

    // In the browser: the account's name, as it was written, what has
    // happened and what to do, and that Quill is waiting.
    harness.act(browser("work mail"));
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        (shown.step(), shown.title()),
        ("sign_in", "Sign In to work mail")
    );
    assert_eq!(
        shown.note(),
        Some(
            "Your browser has been opened at the provider's sign-in page.\n\n\
             Sign in there. When the page says you can close it, come back: Quill is waiting, \
             and takes it from there.\n\n\
             Your password goes to the provider, never to Quill. Quill keeps only the sign-in \
             the provider hands back, in the Keychain."
        )
    );
    assert_eq!(shown.busy(), Some("Waiting for you to sign in…"));
    assert_eq!(shown.error(), None);
    assert!(shown.fields().is_empty());
    assert_eq!(buttons(&shown), [CANCEL]);
    is_the_hosts(&harness, &behind);
    // The address went to the host to be opened, and is in no form.
    assert_eq!(
        harness.host_requests.last(),
        Some(&HostRequest::OpenUrl(AUTHORIZE.to_owned()))
    );
    let line = bridge::encode(&CoreToShell::FormShow(shown)).unwrap();
    assert!(!line.contains("login.example.com"), "{line}");
    assert!(!line.contains("s3cr3t"), "{line}");
}

#[test]
fn a_sign_in_with_a_code_shows_the_page_and_the_code_and_prints_neither() {
    let mut harness = mailbox(true);
    let behind = panes(&harness);
    harness.keys("S");
    harness.act(with_a_code("Work"));
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        (shown.step(), shown.title()),
        ("sign_in", "Sign In to Work")
    );
    assert_eq!(
        shown.note(),
        Some("On any device, open this page and enter the code:")
    );
    assert_eq!(ids(&shown), ["page", "code"]);
    let (page, code) = (field(&shown, "page"), field(&shown, "code"));
    assert!(matches!(page, FormField::Code { .. }) && matches!(code, FormField::Code { .. }));
    assert_eq!(
        (page.label(), page.code().map(Secret::reveal), page.help()),
        ("Page:", Some("https://login.example.com/device"), None)
    );
    assert_eq!(
        (code.label(), code.code().map(Secret::reveal), code.help()),
        (
            "Code:",
            Some("HJKL-MNPQ"),
            Some("The code is good for 15 minutes.")
        )
    );
    assert_eq!(
        shown.busy(),
        Some("Quill is waiting, and takes it from there.")
    );
    assert_eq!(buttons(&shown), [CANCEL]);
    is_the_hosts(&harness, &behind);

    // They go to the host on the bridge, whole.
    let message = CoreToShell::FormShow(shown.clone());
    let line = bridge::encode(&message).unwrap();
    assert!(line.contains("HJKL-MNPQ") && line.contains("https://login.example.com/device"));
    // And are in nothing that prints the form, the message, or the
    // sign-in the state holds.
    for printed in [
        format!("{shown:?}"),
        format!("{shown:#?}"),
        format!("{message:?}"),
        format!("{:?}", harness.state.sign_in),
        format!("{:#?}", harness.state.sign_in),
    ] {
        assert!(!printed.contains("HJKL"), "{printed}");
        assert!(!printed.contains("login.example.com"), "{printed}");
    }
    // What the state prints still says where the sign-in stands.
    assert!(format!("{:?}", harness.state.sign_in).contains("Code { minutes: 15, .. }"));
    assert!(matches!(
        harness.state.sign_in.as_ref().map(|sign_in| &sign_in.stage),
        Some(SignInStage::Code { .. })
    ));

    // What a provider sends is a stranger's words.
    let mut harness = mailbox(true);
    harness.keys("S");
    harness.act(Action::Account(AccountEvent::SignInCode {
        account: "Work".to_owned(),
        address: "https://login.example.com/device\u{1b}[2J".to_owned(),
        code: "HJKL\nMNPQ".to_owned(),
        minutes: 5,
    }));
    let shown = sheet(&harness).unwrap();
    assert_eq!(
        field(&shown, "page").code().map(Secret::reveal),
        Some("https://login.example.com/device")
    );
    assert_eq!(
        field(&shown, "code").code().map(Secret::reveal),
        Some("HJKL MNPQ")
    );
}

#[test]
fn cancel_in_the_sign_in_sheet_gives_up_as_esc_does() {
    let mut terminal = mailbox(false);
    terminal.keys("S");
    terminal.act(browser("Work"));
    is_the_terminals(&terminal);
    terminal.keys("esc");

    let mut app = mailbox(true);
    app.keys("S");
    app.act(browser("Work"));
    press(&mut app, Some("cancel"), &[]);

    for harness in [&terminal, &app] {
        assert!(harness.state.sign_in.is_none());
        assert_ne!(harness.state.context(), Context::SignIn);
        assert_eq!(
            harness.account_requests.last(),
            Some(&AccountRequest::CancelSignIn)
        );
        assert_eq!(sheet(harness), None);
    }
    assert_eq!(app.account_requests, terminal.account_requests);
    assert_eq!(notices(&app), notices(&terminal));
    assert_eq!(app.screen(), terminal.screen());

    // An answer that comes for a sign-in already given up on is dropped,
    // and a second press of the button has nothing to act on.
    let before = app.account_requests.len();
    app.act(Action::FormUsed(
        FormUsed::new("sign_in", Some("cancel"), &[], &[]).unwrap(),
    ));
    assert_eq!(app.account_requests.len(), before);
    // It has the one button: anything else is no answer.
    app.keys("S");
    press(&mut app, Some("confirm"), &[]);
    press(&mut app, None, &[]);
    assert_eq!(app.state.context(), Context::SignIn);
}

#[test]
fn the_sign_in_sheet_is_put_away_when_the_sign_in_ends_either_way() {
    for result in [Ok(()), Err("the provider said no".to_owned())] {
        let mut terminal = mailbox(false);
        let mut app = mailbox(true);
        for harness in [&mut terminal, &mut app] {
            harness.keys("S");
            harness.act(browser("Work"));
            harness.act(Action::Account(AccountEvent::SignedIn {
                account: "Work".to_owned(),
                result: result.clone(),
            }));
            assert!(harness.state.sign_in.is_none());
            assert_eq!(sheet(harness), None);
            assert_eq!(dialog(harness), None);
        }
        // How it ended is said as a terminal says it: in a notice.
        let said = notices(&app);
        assert_eq!(said, notices(&terminal));
        assert_eq!(
            said.last().unwrap(),
            &match &result {
                Ok(()) => (
                    NoticeKind::Info,
                    "Signed in to Work. Its mail is on the way.".to_owned()
                ),
                Err(_) => (
                    NoticeKind::Error,
                    "Work is not signed in: the provider said no".to_owned()
                ),
            }
        );
        assert_eq!(app.screen(), terminal.screen());
    }
}

// ---------------------------------------------------------------------
// The certificate of an account that is already there
// ---------------------------------------------------------------------

fn problem(host: &str) -> CertificateProblem {
    CertificateProblem {
        host: host.to_owned(),
        reason: "UnknownIssuer".to_owned(),
        fingerprint: Some("0123456789abcdef".repeat(4)),
        certificate: None,
        details: Some(CertificateDetails {
            subject: "CN=imap.example.com".to_owned(),
            issuer: "CN=Example Home Server".to_owned(),
            not_before: 1_794_562_200,
            not_after: 1_797_240_600,
        }),
    }
}

fn presented(account: &str, problem: CertificateProblem, sending: bool) -> Action {
    Action::Sync(SyncEvent::Certificate {
        account: account.to_owned(),
        problem: Box::new(problem),
        sending,
    })
}

const REJECT: (&str, &str, Option<FormRole>, bool) =
    ("reject", "Reject", Some(FormRole::Cancel), true);

#[test]
fn a_certificate_of_an_existing_account_is_asked_about_in_the_sheet() {
    let mut harness = mailbox(true);
    let behind = panes(&harness);
    harness.act(presented("Work", problem("imap.example.com"), false));
    assert_eq!(harness.state.context(), Context::Certificate);
    let shown = sheet(&harness).unwrap();
    assert!(shown.step().starts_with("account_certificate_"));
    assert_eq!(shown.title(), "Work: Certificate");
    assert_eq!(
        shown.note(),
        Some(
            "imap.example.com presented a certificate that was not accepted. Nothing has been \
             sent to it."
        )
    );
    assert_eq!(
        shown.error(),
        Some(
            "It was issued by an authority this Mac does not trust. A self-signed certificate \
             reads this way."
        )
    );
    assert_eq!(shown.busy(), None);
    assert_eq!(ids(&shown), ["issued_to", "issued_by", "valid", "sha256"]);
    assert_eq!(
        field(&shown, "issued_to").value(),
        Some("CN=imap.example.com")
    );
    assert_eq!(
        field(&shown, "sha256").value(),
        Some(
            "01:23:45:67:89:ab:cd:ef:01:23:45:67:89:ab:cd:ef\n\
             01:23:45:67:89:ab:cd:ef:01:23:45:67:89:ab:cd:ef"
        )
    );
    assert_eq!(
        buttons(&shown),
        [
            REJECT,
            ("trust_once", "Trust Once", None, true),
            ("trust_always", "Always Trust", None, true),
        ]
    );
    is_the_hosts(&harness, &behind);

    // Of the server the account sends through, it says so first.
    let mut harness = mailbox(true);
    harness.act(presented("Work", problem("smtp.example.com"), true));
    let sending = sheet(&harness).unwrap();
    assert_eq!(sending.title(), "Work: Certificate for Sending");
    assert_eq!(
        sending.note(),
        Some(
            "This is the server the account sends mail through. A message is waiting in the \
             Outbox for it.\n\n\
             smtp.example.com presented a certificate that was not accepted. Nothing has been \
             sent to it."
        )
    );
    assert_ne!(sending.step(), shown.step());
}

#[test]
fn it_is_described_as_the_one_met_while_adding_an_account_is() {
    // The same certificate, met at the last step of adding an account.
    let mut adding = mailbox(true);
    adding.keys("A");
    press(
        &mut adding,
        Some("continue"),
        &[("address", "robin@example.com")],
    );
    adding.act(Action::Account(AccountEvent::Discovered {
        address: "robin@example.com".to_owned(),
        found: quill_tui::setup::Discovery {
            draft: quill_tui::setup::AccountDraft {
                name: "Example".to_owned(),
                address: "robin@example.com".to_owned(),
                imap: quill_tui::setup::ServerDraft {
                    host: "imap.example.com".to_owned(),
                    port: 993,
                    tls: quill_core::config::TlsMode::Implicit,
                    username: "robin@example.com".to_owned(),
                },
                smtp: None,
            },
            found: true,
            note: None,
            microsoft_application: None,
            microsoft_own: false,
        },
    }));
    press(&mut adding, Some("continue"), &[]);
    let step = sheet(&adding).unwrap().step().to_owned();
    adding.act(Action::FormUsed(
        FormUsed::new(
            &step,
            Some("continue"),
            &[],
            &[("password", "not-a-real-password")],
        )
        .unwrap(),
    ));
    adding.act(Action::Account(AccountEvent::Verified(Err(
        VerifyProblem::Certificate(Box::new(problem("imap.example.com"))),
    ))));
    let while_adding = sheet(&adding).unwrap();
    assert_eq!(while_adding.step(), "certificate");

    let mut existing = mailbox(true);
    existing.act(presented("Work", problem("imap.example.com"), false));
    let of_existing = sheet(&existing).unwrap();

    // What it says of the certificate, the certificate in its own words
    // and the three things to do about it are the same description.
    assert_eq!(of_existing.note(), while_adding.note());
    assert_eq!(of_existing.error(), while_adding.error());
    assert_eq!(of_existing.fields(), while_adding.fields());
    assert_eq!(of_existing.buttons(), while_adding.buttons());
    // They are two questions all the same: an answer to one is not one to
    // the other.
    assert_ne!(of_existing.step(), while_adding.step());
    let stale = FormUsed::new("certificate", Some("trust_always"), &[], &[]).unwrap();
    existing.act(Action::FormUsed(stale));
    assert!(existing.account_requests.is_empty());
    assert_eq!(existing.state.certificates.len(), 1);
}

#[test]
fn its_buttons_do_what_r_o_and_p_do() {
    for (key, button, pin) in [
        ("o", "trust_once", Some(false)),
        ("p", "trust_always", Some(true)),
        ("r", "reject", None),
        ("esc", "reject", None),
    ] {
        let mut terminal = mailbox(false);
        terminal.act(presented("Work", problem("imap.example.com"), false));
        is_the_terminals(&terminal);
        terminal.keys(key);

        let mut app = mailbox(true);
        app.act(presented("Work", problem("imap.example.com"), false));
        press(&mut app, Some(button), &[]);

        assert_eq!(app.account_requests, terminal.account_requests, "{key}");
        assert_eq!(app.state.certificates, terminal.state.certificates, "{key}");
        match pin {
            Some(pin) => {
                assert_eq!(
                    app.account_requests,
                    [AccountRequest::Trust {
                        account: "Work".to_owned(),
                        fingerprint: "0123456789abcdef".repeat(4),
                        pin,
                        sending: false,
                    }]
                );
                // Being acted on: it says so, and only giving up can be
                // pressed. A second press asks for nothing more.
                let shown = sheet(&app).unwrap();
                assert_eq!(shown.busy(), Some("Working…"));
                assert_eq!(
                    buttons(&shown),
                    [
                        REJECT,
                        ("trust_once", "Trust Once", None, false),
                        ("trust_always", "Always Trust", None, false),
                    ]
                );
                press(&mut app, Some(button), &[]);
                assert_eq!(app.account_requests.len(), 1);
                // Done: the sheet is put away.
                for harness in [&mut terminal, &mut app] {
                    harness.act(Action::Account(AccountEvent::Trusted {
                        account: "Work".to_owned(),
                        sending: false,
                        result: Ok(()),
                    }));
                    assert!(harness.state.certificates.is_empty());
                    assert_eq!(sheet(harness), None);
                }
            }
            None => {
                assert!(app.account_requests.is_empty());
                assert!(app.state.certificates.is_empty());
                assert_eq!(sheet(&app), None);
            }
        }
        assert_eq!(app.screen(), terminal.screen(), "{key}");
    }
}

#[test]
fn a_certificate_that_cannot_be_read_can_only_be_rejected() {
    let mut harness = mailbox(true);
    let mut unreadable = problem("imap.example.com");
    unreadable.fingerprint = None;
    unreadable.details = None;
    harness.act(presented("Work", unreadable, false));
    let shown = sheet(&harness).unwrap();
    assert!(shown.fields().is_empty());
    assert_eq!(buttons(&shown), [REJECT]);
    assert!(
        shown
            .error()
            .unwrap()
            .ends_with("\nThe certificate could not be read, so it cannot be accepted.")
    );
    // A button it does not have accepts nothing.
    press(&mut harness, Some("trust_always"), &[]);
    press(&mut harness, Some("continue"), &[]);
    assert!(harness.account_requests.is_empty());
    assert_eq!(harness.state.certificates.len(), 1);
    assert!(!harness.state.certificates[0].busy);
    press(&mut harness, Some("reject"), &[]);
    assert!(harness.state.certificates.is_empty());
}

#[test]
fn an_answer_to_one_certificate_is_never_taken_for_the_next() {
    let mut harness = mailbox(true);
    harness.act(presented("Work", problem("imap.example.com"), false));
    harness.act(presented("Personal", problem("mail.example.org"), false));
    let first = sheet(&harness).unwrap();
    assert_eq!(first.title(), "Work: Certificate");

    // Rejected. The next question is in the sheet at once, in place of
    // the first.
    press(&mut harness, Some("reject"), &[]);
    let second = sheet(&harness).unwrap();
    assert_eq!(second.title(), "Personal: Certificate");
    assert_ne!(first.step(), second.step());

    // A second click on its way to the first question arrives now. It is
    // not an answer to a certificate nobody has read yet: not a rejection,
    // and above all not an acceptance.
    for button in ["reject", "trust_once", "trust_always"] {
        let late = FormUsed::new(first.step(), Some(button), &[], &[]).unwrap();
        harness.act(Action::FormUsed(late));
        assert_eq!(harness.state.certificates.len(), 1, "{button}");
        assert!(!harness.state.certificates[0].busy, "{button}");
        assert!(harness.account_requests.is_empty(), "{button}");
        assert_eq!(sheet(&harness), Some(second.clone()));
    }
    // The same certificate asked about again is the same question.
    harness.act(presented("Personal", problem("mail.example.org"), false));
    assert_eq!(sheet(&harness).unwrap().step(), second.step());
}

#[test]
fn a_letter_typed_at_the_terminal_accepts_no_certificate_the_sheet_is_asking_about() {
    let mut harness = mailbox(true);
    harness.act(presented("Work", problem("imap.example.com"), false));
    harness.keys("o p r");
    assert!(harness.account_requests.is_empty());
    assert_eq!(harness.state.certificates.len(), 1);
    assert!(!harness.state.certificates[0].busy);
    assert_eq!(harness.state.context(), Context::Certificate);
}

// ---------------------------------------------------------------------
// A time: snoozing, and sending later
// ---------------------------------------------------------------------

const EXAMPLES: &str = "Such as: 30m  2h  3d  tonight  tomorrow  fri 14:30  2026-12-01 9:00";

fn offered(form: &FormShow) -> (&str, Vec<(&str, &str)>) {
    let FormField::Choice { value, options, .. } = field(form, "offered") else {
        panic!("not a choice");
    };
    (
        value.as_str(),
        options
            .iter()
            .map(|option| (option.id.as_str(), option.title.as_str()))
            .collect(),
    )
}

/// Writes a message as far as the time it is to go at being asked for.
fn writing(harness: &mut Harness) {
    harness.keys("c");
    harness.type_text("ana.ruiz@example.com");
    // Past the completed address and the empty Cc, to the subject.
    harness.keys("tab tab tab");
    harness.type_text("Later");
    harness.keys("tab");
    harness.type_text("Not yet.");
    harness.keys("ctrl-l");
    assert_eq!(harness.state.context(), Context::When);
}

#[test]
fn snoozing_asks_when_in_the_sheet() {
    let mut harness = demo(true);
    let behind = panes(&harness);
    harness.keys("b");
    assert_eq!(harness.state.context(), Context::When);
    let shown = sheet(&harness).unwrap();
    assert_eq!((shown.step(), shown.title()), ("snooze", "Snooze Until"));
    assert_eq!(
        (shown.note(), shown.error(), shown.busy()),
        (None, None, None)
    );
    assert_eq!(ids(&shown), ["when", "offered"]);
    // The line to type on has the keyboard, and the terminal's own words
    // under it.
    let when = field(&shown, "when");
    assert!(matches!(when, FormField::Text { .. }) && when.focus());
    assert_eq!((when.label(), when.value()), ("When:", Some("")));
    assert_eq!(
        when.help(),
        Some(format!("{EXAMPLES}\n\"now\" brings a snoozed message straight back.").as_str())
    );
    // The first of those examples are there to choose, each called what
    // it is; none is chosen.
    assert_eq!(field(&shown, "offered").label(), "Preset:");
    assert_eq!(
        offered(&shown),
        (
            "typed",
            vec![
                ("typed", "None"),
                ("in_30m", "In 30 Minutes"),
                ("in_2h", "In 2 Hours"),
                ("in_3d", "In 3 Days"),
                ("tonight", "Tonight"),
                ("tomorrow", "Tomorrow"),
            ]
        )
    );
    assert_eq!(
        buttons(&shown),
        [CANCEL, ("confirm", "Snooze", Some(FormRole::Default), true)]
    );
    is_the_hosts(&harness, &behind);

    // Several at once say how many.
    let mut harness = demo(true);
    harness.keys("x x x b");
    assert_eq!(sheet(&harness).unwrap().title(), "Snooze 3 Messages Until");
}

#[test]
fn a_time_from_the_sheet_snoozes_as_typing_it_and_enter_do() {
    for typed in ["tomorrow 9", "2h", " FRI 14:30 "] {
        let mut terminal = demo(false);
        terminal.keys("j b");
        is_the_terminals(&terminal);
        terminal.type_text(typed);
        terminal.keys("enter");

        let mut app = demo(true);
        app.keys("j b");
        press(
            &mut app,
            Some("confirm"),
            &[("when", typed), ("offered", "typed")],
        );

        for harness in [&terminal, &app] {
            assert_ne!(harness.state.context(), Context::When, "{typed}");
            assert_eq!(sheet(harness), None);
        }
        let snoozed = |harness: &Harness| {
            harness
                .store
                .messages(Mailbox::Snoozed)
                .unwrap()
                .iter()
                .map(|row| (row.headers.subject.clone(), row.snoozed_until))
                .collect::<Vec<_>>()
        };
        assert_eq!(snoozed(&app).len(), 1, "{typed}");
        assert_eq!(snoozed(&app), snoozed(&terminal), "{typed}");
        assert_eq!(subjects(&app), subjects(&terminal), "{typed}");
        assert_eq!(app.screen(), terminal.screen(), "{typed}");
    }
}

#[test]
fn a_time_chosen_in_the_sheet_is_typed_on_its_line() {
    let mut harness = demo(true);
    let all = subjects(&harness);
    harness.keys("b");
    // The pop-up changed, with something already typed: no button.
    press(
        &mut harness,
        None,
        &[("when", "fri 9"), ("offered", "tomorrow")],
    );
    // Nothing is snoozed by choosing. The line says what was chosen, as
    // typing it would have, and the choice is the one made.
    assert_eq!(harness.state.context(), Context::When);
    assert_eq!(subjects(&harness), all);
    let shown = sheet(&harness).unwrap();
    assert_eq!(field(&shown, "when").value(), Some("tomorrow"));
    assert_eq!(offered(&shown).0, "tomorrow");
    assert_eq!(
        harness.state.when.as_ref().unwrap().input.text(),
        "tomorrow"
    );
    // One with a name of its own on the bridge: what it types is the
    // terminal's "2h".
    press(
        &mut harness,
        None,
        &[("when", "tomorrow"), ("offered", "in_2h")],
    );
    let shown = sheet(&harness).unwrap();
    assert_eq!(field(&shown, "when").value(), Some("2h"));
    assert_eq!(offered(&shown).0, "in_2h");
    // Choosing none of them, or something that is not one, leaves the
    // line as the user has it.
    for none in ["typed", "next_year"] {
        press(&mut harness, None, &[("when", "2h 30"), ("offered", none)]);
        let shown = sheet(&harness).unwrap();
        assert_eq!(field(&shown, "when").value(), Some("2h 30"));
        assert_eq!(offered(&shown).0, "typed");
    }

    // The button then reads the line, here and nowhere else: what was
    // chosen and then typed over is not what counts.
    let mut terminal = demo(false);
    terminal.keys("b");
    terminal.type_text("2h");
    terminal.keys("enter");
    press(
        &mut harness,
        Some("confirm"),
        &[("when", "2h"), ("offered", "tomorrow")],
    );
    assert_eq!(subjects(&harness), subjects(&terminal));
    assert_eq!(
        harness.store.messages(Mailbox::Snoozed).unwrap()[0].snoozed_until,
        Some(NOW + 2 * 3600)
    );
    assert_eq!(harness.screen(), terminal.screen());
}

#[test]
fn a_time_quill_cannot_read_is_said_in_the_sheet_and_nothing_happens() {
    for (typed, complaint) in [
        ("", "Type a time."),
        (
            "whenever",
            "That is not a time to come that Quill can read.",
        ),
        (
            "2026-11-01",
            "That is not a time to come that Quill can read.",
        ),
    ] {
        let mut terminal = demo(false);
        terminal.keys("b");
        terminal.type_text(typed);
        terminal.keys("enter");
        assert!(terminal.screen().contains(complaint));

        let mut app = demo(true);
        let all = subjects(&app);
        app.keys("b");
        press(&mut app, Some("confirm"), &[("when", typed)]);
        // Still asked, with the terminal's own complaint, and with what
        // was typed still on the line to correct.
        assert_eq!(app.state.context(), Context::When);
        let shown = sheet(&app).unwrap();
        assert_eq!(shown.error(), Some(complaint));
        assert_eq!(field(&shown, "when").value(), Some(typed));
        assert_eq!(
            app.state.when.as_ref().map(|when| when.problem.clone()),
            terminal
                .state
                .when
                .as_ref()
                .map(|when| when.problem.clone())
        );
        assert_eq!(subjects(&app), all);
        assert!(app.store.messages(Mailbox::Snoozed).unwrap().is_empty());

        // Choosing a time takes the complaint away, as typing does.
        press(&mut app, None, &[("when", typed), ("offered", "tonight")]);
        assert_eq!(sheet(&app).unwrap().error(), None);
        // And a time it can read then goes through.
        press(&mut app, Some("confirm"), &[("when", "tonight")]);
        assert_eq!(sheet(&app), None);
        assert_eq!(app.store.messages(Mailbox::Snoozed).unwrap().len(), 1);
    }
}

#[test]
fn cancel_in_the_snooze_sheet_snoozes_nothing_as_esc_does() {
    // Something said in the bottom bar while the question was up (mail
    // that went, say) has been seen by the time the question is answered,
    // whichever way it is answered.
    let said = Some(("Sent.".to_owned(), false));
    let mut terminal = demo(false);
    terminal.keys("b");
    terminal.type_text("2h");
    terminal.state.flash.clone_from(&said);
    terminal.keys("esc");

    let mut app = demo(true);
    let all = subjects(&app);
    app.keys("b");
    app.state.flash.clone_from(&said);
    // An answer to some other question is none, and clears nothing.
    app.act(Action::FormUsed(
        FormUsed::new("send_later", Some("cancel"), &[], &[]).unwrap(),
    ));
    assert_eq!(app.state.flash, said);
    // Cancel sends nothing that was typed.
    press(&mut app, Some("cancel"), &[]);
    assert_eq!((&app.state.flash, &terminal.state.flash), (&None, &None));
    assert_ne!(app.state.context(), Context::When);
    assert_eq!(sheet(&app), None);
    assert_eq!(subjects(&app), all);
    assert!(app.store.messages(Mailbox::Snoozed).unwrap().is_empty());
    assert_eq!(app.screen(), terminal.screen());
    // A second press of either button has no question to answer.
    for button in ["confirm", "cancel"] {
        app.act(Action::FormUsed(
            FormUsed::new("snooze", Some(button), &[("when", "2h")], &[]).unwrap(),
        ));
    }
    assert!(app.store.messages(Mailbox::Snoozed).unwrap().is_empty());
}

#[test]
fn now_from_the_sheet_brings_a_snoozed_message_back() {
    let mut terminal = demo(false);
    let mut app = demo(true);
    for (harness, in_the_app) in [(&mut terminal, false), (&mut app, true)] {
        harness.keys("x x b");
        if in_the_app {
            press(harness, Some("confirm"), &[("when", "fri")]);
        } else {
            harness.type_text("fri");
            harness.keys("enter");
        }
        // In the Snoozed mailbox: "now" for one of them.
        harness.keys("1 j enter");
        assert_eq!(harness.state.mailbox, Mailbox::Snoozed);
        harness.keys("b");
        if in_the_app {
            press(harness, Some("confirm"), &[("when", "Now")]);
        } else {
            harness.type_text("Now");
            harness.keys("enter");
        }
        assert_eq!(subjects(harness).len(), 1);
        assert!(status(harness).contains("Back in its folder."));
    }
    assert_eq!(app.screen(), terminal.screen());
}

#[test]
fn sending_later_asks_when_in_the_sheet_and_sends_as_enter_does() {
    let mut terminal = demo(false);
    writing(&mut terminal);
    is_the_terminals(&terminal);
    terminal.type_text("tomorrow 7");
    terminal.keys("enter");

    let mut app = demo(true);
    writing(&mut app);
    let shown = sheet(&app).unwrap();
    assert_eq!((shown.step(), shown.title()), ("send_later", "Send At"));
    assert_eq!(ids(&shown), ["when", "offered"]);
    assert_eq!(
        field(&shown, "when").help(),
        Some(
            format!(
                "{EXAMPLES}\nIt waits in the Outbox and goes at that time if Quill is running, \
                 or when it next is."
            )
            .as_str()
        )
    );
    assert_eq!(
        buttons(&shown),
        [
            CANCEL,
            ("confirm", "Send Later", Some(FormRole::Default), true)
        ]
    );
    assert!(app.state.form_in_sheet());
    assert_eq!(dialog(&app), None);
    assert!(!app.state.is_enabled(&app.env, CommandId::NavOpen));

    // An answer meant for the other question that asks a time is not one
    // for this.
    app.act(Action::FormUsed(
        FormUsed::new("snooze", Some("confirm"), &[("when", "2h")], &[]).unwrap(),
    ));
    assert_eq!(app.state.context(), Context::When);
    assert!(app.store.outbox().unwrap().is_empty());

    press(&mut app, Some("confirm"), &[("when", "tomorrow 7")]);
    for harness in [&terminal, &app] {
        assert_ne!(harness.state.context(), Context::Compose);
        assert_ne!(harness.state.context(), Context::When);
        let outbox = harness.store.outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].state, DraftState::Outbox);
    }
    // The same moment, to within the second or two the test took.
    let due = |harness: &Harness| harness.store.outbox().unwrap()[0].send_after.unwrap();
    assert!((due(&app) - due(&terminal)).abs() < 5);
    assert_eq!(status(&app), status(&terminal));

    // Cancelled, the message is still being written and nothing is sent.
    let mut app = demo(true);
    writing(&mut app);
    press(&mut app, Some("cancel"), &[]);
    assert_eq!(app.state.context(), Context::Compose);
    assert!(app.store.outbox().unwrap().is_empty());
    // And a time that is none leaves the question up, and says why.
    app.keys("ctrl-l");
    press(&mut app, Some("confirm"), &[("when", "yesterday")]);
    assert_eq!(
        sheet(&app).unwrap().error(),
        Some("That is not a time to come that Quill can read.")
    );
    assert!(app.store.outbox().unwrap().is_empty());
}

// ---------------------------------------------------------------------
// Unsubscribing
// ---------------------------------------------------------------------

const SUBJECT: &str = "New this November";
const PAGE: &str = "https://lists.harbourbooks.example.com/u/r0b1n";
const MAIL: &str = "mailto:leave@harbourbooks.example.com";
const ONE_CLICK: &str = "List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n";

/// Puts a message from a list at the top of the Work inbox, with these
/// lines among its headers, and opens it. `service`: there is an accounts
/// service to make requests with, as there is for every mailbox but the
/// demo.
fn reading(headers: &str, service: bool, in_the_app: bool) -> Harness {
    let raw = format!(
        "From: Harbour Books\u{1b}[2J <news@harbourbooks.example.com>\r\n\
         To: Robin Vale <robin@example.com>\r\n\
         Subject: {SUBJECT}\r\n\
         {headers}\
         \r\n\
         Three new arrivals this month.\r\n"
    );
    let mut harness = if service {
        Harness::with_accounts(200, 60)
    } else {
        Harness::demo(200, 60)
    };
    harness.env.sheets = in_the_app;
    let inbox = harness
        .store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId("list-1".to_owned());
    let parsed = quill_mime::parse(raw.as_bytes(), NOW).unwrap();
    harness
        .store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![RemoteMessage {
                    id: remote.clone(),
                    headers: Headers {
                        date: NOW + 60,
                        ..parsed.headers.clone()
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
    harness
        .store
        .put_message(id, &parsed.body, raw.as_bytes())
        .unwrap();
    harness.act(Action::Data(DataEvent::StoreChanged));
    harness.keys("2 g g enter");
    assert_eq!(harness.selected_subject(), SUBJECT);
    harness
}

/// Whether anything at all was asked of the world outside: the accounts
/// service, the browser, or the outbox.
fn nothing_left_quill(harness: &Harness) -> bool {
    harness.account_requests.is_empty()
        && !harness
            .host_requests
            .iter()
            .any(|request| matches!(request, HostRequest::OpenUrl(_)))
        && harness.store.outbox().unwrap().is_empty()
}

#[test]
fn unsubscribing_is_asked_about_in_the_sheet_and_says_exactly_what_yes_does() {
    let one_click = format!("List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}");
    let mail = format!("List-Unsubscribe: <{MAIL}>\r\n");
    let page = format!("List-Unsubscribe: <{PAGE}>\r\n");
    for (headers, does, does_not, address, yes) in [
        (
            &one_click,
            "Quill will tell lists.harbourbooks.example.com to unsubscribe you.",
            "One request goes to the address below, which the list gave for this. It says \
             \"unsubscribe\" and nothing else: no cookies, and no page is opened.",
            ("Address:", PAGE),
            "Unsubscribe",
        ),
        (
            &mail,
            "A message will be written to leave@harbourbooks.example.com.",
            "It is not sent for you. Look it over, then send it yourself to unsubscribe, or \
             close it to do nothing.",
            ("To:", "leave@harbourbooks.example.com"),
            "Write the Message",
        ),
        (
            &page,
            "Your browser will open a page at lists.harbourbooks.example.com.",
            "This list has no way for Quill to unsubscribe you itself: its page says what to \
             do next. Quill sends nothing.",
            ("Address:", PAGE),
            "Open the Page",
        ),
    ] {
        let mut harness = reading(headers, true, true);
        let behind = panes(&harness);
        harness.keys("X");
        assert_eq!(harness.state.context(), Context::Unsubscribe);
        let shown = sheet(&harness).unwrap();
        // What yes does is what the sheet is called, in full.
        assert_eq!((shown.step(), shown.title()), ("unsubscribe", does));
        assert_eq!(
            shown.note(),
            Some(format!("This message is from a mailing list.\n\n{does_not}").as_str())
        );
        assert_eq!((shown.error(), shown.busy()), (None, None));
        // Who the list is, in a stranger's words made safe, and the one
        // address anything goes to.
        assert_eq!(ids(&shown), ["from", "address"]);
        let from = field(&shown, "from");
        assert!(matches!(from, FormField::Static { .. }));
        assert_eq!(
            (from.label(), from.value()),
            (
                "From:",
                Some("Harbour Books <news@harbourbooks.example.com>")
            )
        );
        let to = field(&shown, "address");
        assert_eq!((to.label(), to.value()), (address.0, Some(address.1)));
        // The button that says yes is named for what it does.
        assert_eq!(
            buttons(&shown),
            [CANCEL, ("confirm", yes, Some(FormRole::Default), true)]
        );
        is_the_hosts(&harness, &behind);
        // Asking is all that has happened.
        assert!(nothing_left_quill(&harness));
    }
}

#[test]
fn yes_in_the_unsubscribe_sheet_does_the_one_thing_enter_does() {
    let one_click = format!("List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}");
    let mail = format!("List-Unsubscribe: <{MAIL}>\r\n");
    let page = format!("List-Unsubscribe: <{PAGE}>\r\n");
    for (headers, service) in [
        (&one_click, true),
        (&mail, true),
        (&page, true),
        // The demo mailbox has nothing to make a request with: one click
        // is a message there.
        (&one_click, false),
    ] {
        let mut terminal = reading(headers, service, false);
        terminal.keys("X");
        is_the_terminals(&terminal);
        terminal.keys("enter");

        let mut app = reading(headers, service, true);
        app.keys("X");
        press(&mut app, Some("confirm"), &[]);

        for harness in [&terminal, &app] {
            assert!(harness.state.unsubscribe.is_none());
            assert_eq!(sheet(harness), None);
        }
        assert_eq!(app.account_requests, terminal.account_requests);
        let opened = |harness: &Harness| {
            harness
                .host_requests
                .iter()
                .filter(|request| matches!(request, HostRequest::OpenUrl(_)))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(opened(&app), opened(&terminal));
        assert_eq!(app.state.context(), terminal.state.context());
        assert_eq!(app.state.flash, terminal.state.flash);
        assert_eq!(app.screen(), terminal.screen());
        assert!(app.store.outbox().unwrap().is_empty());
        // One of the three, and only that one.
        let asked = usize::from(!app.account_requests.is_empty());
        let browser = usize::from(!opened(&app).is_empty());
        let written = usize::from(app.state.compose.is_some());
        assert_eq!(asked + browser + written, 1);
        if service && headers == &one_click {
            assert_eq!(
                app.account_requests,
                [AccountRequest::Unsubscribe {
                    url: PAGE.to_owned()
                }]
            );
        }
        // A second press has no question to answer.
        app.act(Action::FormUsed(
            FormUsed::new("unsubscribe", Some("confirm"), &[], &[]).unwrap(),
        ));
        assert_eq!(app.account_requests, terminal.account_requests);
        assert_eq!(opened(&app), opened(&terminal));
    }
}

#[test]
fn cancel_in_the_unsubscribe_sheet_asks_nothing_of_anyone() {
    let headers = format!("List-Unsubscribe: <{MAIL}>, <{PAGE}>\r\n{ONE_CLICK}");
    let mut terminal = reading(&headers, true, false);
    terminal.keys("X esc");

    let mut app = reading(&headers, true, true);
    app.keys("X");
    // Neither a changed choice nor a button it does not have is an answer.
    press(&mut app, None, &[]);
    press(&mut app, Some("continue"), &[]);
    assert_eq!(app.state.context(), Context::Unsubscribe);
    assert!(nothing_left_quill(&app));

    press(&mut app, Some("cancel"), &[]);
    assert!(app.state.unsubscribe.is_none());
    assert_eq!(app.state.context(), Context::Reader);
    assert_eq!(sheet(&app), None);
    assert!(nothing_left_quill(&app));
    assert!(app.state.compose.is_none());
    assert_eq!(app.selected_subject(), SUBJECT);
    assert_eq!(app.screen(), terminal.screen());
}

// ---------------------------------------------------------------------
// One sheet, and who has it
// ---------------------------------------------------------------------

#[test]
fn in_a_terminal_every_question_is_drawn_and_no_host_is_told_of_any() {
    let mut harness = mailbox(false);
    harness.keys("S");
    is_the_terminals(&harness);
    assert!(harness.screen().contains(" Sign in "));
    harness.act(with_a_code("Work"));
    is_the_terminals(&harness);
    assert!(harness.screen().contains("HJKL-MNPQ"));
    harness.keys("esc");

    harness.act(presented("Work", problem("imap.example.com"), false));
    is_the_terminals(&harness);
    assert!(harness.screen().contains(" Work: certificate "));
    harness.keys("r");

    harness.keys("b");
    is_the_terminals(&harness);
    assert!(harness.screen().contains(" Snooze until "));
    assert!(status(&harness).contains("enter set"));
    // The keys that answer it are on, and nothing a sheet might send is
    // taken for an answer.
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavOpen));
    let all = subjects(&harness);
    harness.act(Action::FormUsed(
        FormUsed::new("snooze", Some("confirm"), &[("when", "2h")], &[]).unwrap(),
    ));
    assert_eq!(harness.state.context(), Context::When);
    assert_eq!(subjects(&harness), all);
    harness.keys("esc");

    let mut harness = reading(&format!("List-Unsubscribe: <{PAGE}>\r\n"), true, false);
    harness.keys("X");
    is_the_terminals(&harness);
    assert!(harness.screen().contains(" Unsubscribe "));
    assert!(harness.state.is_enabled(&harness.env, CommandId::NavOpen));
    harness.act(Action::FormUsed(
        FormUsed::new("unsubscribe", Some("confirm"), &[], &[]).unwrap(),
    ));
    assert_eq!(harness.state.context(), Context::Unsubscribe);
    assert!(nothing_left_quill(&harness));
}

#[test]
fn there_is_one_sheet_and_the_question_a_terminal_has_in_front_has_it() {
    // Everything at once, as far as it can be: a message from a list being
    // unsubscribed from, then a time asked for, a sign-in, and a
    // certificate.
    let mut harness = reading(&format!("List-Unsubscribe: <{PAGE}>\r\n"), true, true);
    harness.keys("X");
    assert_eq!(sheet(&harness).unwrap().step(), "unsubscribe");

    // A sign-in begun elsewhere (the form that adds an account begins
    // one) comes before it.
    harness.state.sign_in = Some(quill_tui::setup::SignIn {
        account: "Work".to_owned(),
        stage: SignInStage::Browser,
        fresh: false,
    });
    assert_eq!(harness.state.context(), Context::SignIn);
    assert_eq!(sheet(&harness).unwrap().step(), "sign_in");
    // An answer to the question behind it is not taken meanwhile.
    harness.act(Action::FormUsed(
        FormUsed::new("unsubscribe", Some("confirm"), &[], &[]).unwrap(),
    ));
    assert!(harness.state.unsubscribe.is_some());
    assert!(nothing_left_quill(&harness));

    // And a certificate before everything.
    harness.act(presented("Work", problem("imap.example.com"), false));
    assert_eq!(harness.state.context(), Context::Certificate);
    let certificate = sheet(&harness).unwrap();
    assert!(certificate.step().starts_with("account_certificate_"));
    harness.act(Action::FormUsed(
        FormUsed::new("sign_in", Some("cancel"), &[], &[]).unwrap(),
    ));
    assert!(harness.state.sign_in.is_some());

    // Each answered, the one behind it has the sheet, in the same order a
    // terminal draws them in.
    press(&mut harness, Some("reject"), &[]);
    assert_eq!(sheet(&harness).unwrap().step(), "sign_in");
    press(&mut harness, Some("cancel"), &[]);
    assert_eq!(sheet(&harness).unwrap().step(), "unsubscribe");
    press(&mut harness, Some("cancel"), &[]);
    assert_eq!(sheet(&harness), None);
    assert_eq!(harness.state.context(), Context::Reader);

    // A time asked for comes before unsubscribing, and after a sign-in.
    let mut harness = demo(true);
    harness.keys("b");
    assert_eq!(sheet(&harness).unwrap().step(), "snooze");
    harness.state.sign_in = Some(quill_tui::setup::SignIn {
        account: String::new(),
        stage: SignInStage::Starting,
        fresh: false,
    });
    assert_eq!(sheet(&harness).unwrap().step(), "sign_in");
    harness.state.sign_in = None;
    assert_eq!(sheet(&harness).unwrap().step(), "snooze");
}

#[test]
fn the_help_is_no_sheet_and_the_question_waits_behind_it() {
    let mut harness = demo(true);
    harness.keys("b");
    press(&mut harness, None, &[("when", ""), ("offered", "tomorrow")]);
    assert_eq!(sheet(&harness).unwrap().step(), "snooze");

    // The help is drawn in the terminal, whatever the host: the sheet is
    // put away while it is read.
    harness.act(Action::Command(CommandId::HelpToggle));
    assert_eq!(harness.state.context(), Context::Help);
    assert_eq!(sheet(&harness), None);
    assert!(!harness.state.form_in_sheet());
    // The question itself is still not drawn under it.
    assert_eq!(dialog(&harness), None);
    harness.act(Action::FormUsed(
        FormUsed::new("snooze", Some("confirm"), &[("when", "2h")], &[]).unwrap(),
    ));
    assert!(harness.store.messages(Mailbox::Snoozed).unwrap().is_empty());

    // Closed, the question is the host's to show again, as the core has
    // it.
    harness.keys("esc");
    let shown = sheet(&harness).unwrap();
    assert_eq!(shown.step(), "snooze");
    assert_eq!(field(&shown, "when").value(), Some("tomorrow"));
}

#[test]
fn nothing_behind_a_question_in_the_sheet_moves() {
    use quill_tui::state::{Mouse, MouseKind};
    let mut harness = demo(true);
    harness.keys("b");
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
    harness.keys("down pagedown");
    for id in [CommandId::NavNext, CommandId::NavBottom, CommandId::NavOpen] {
        harness.act(Action::Command(id));
    }
    assert_eq!(harness.state.list.selected, selected);
    assert_eq!(harness.state.context(), Context::When);
    assert_eq!(sheet(&harness).unwrap().step(), "snooze");
    assert!(harness.store.messages(Mailbox::Snoozed).unwrap().is_empty());
}
