//! Adding an account from inside the UI, and answering a certificate
//! question: the keys, the screens and what is asked of the service.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use quill_core::config::TlsMode;
use quill_core::{CertificateDetails, CertificateProblem};
use quill_sync::SyncEvent;
use quill_tui::command::{CommandId, Context};
use quill_tui::setup::{
    AccountDraft, AccountEvent, AccountRequest, Discovery, ServerDraft, SignInDraft, Trusting,
    VerifyProblem,
};
use quill_tui::state::Action;

fn found(address: &str) -> AccountEvent {
    AccountEvent::Discovered {
        address: address.to_owned(),
        found: Discovery {
            draft: AccountDraft {
                name: "Example".to_owned(),
                address: address.to_owned(),
                imap: ServerDraft {
                    host: "imap.example.com".to_owned(),
                    port: 993,
                    tls: TlsMode::Implicit,
                    username: address.to_owned(),
                },
                smtp: Some(ServerDraft {
                    host: "smtp.example.com".to_owned(),
                    port: 465,
                    tls: TlsMode::Implicit,
                    username: address.to_owned(),
                }),
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

/// What discovery says of an address whose mail Microsoft hosts, with the
/// application id the config already has, if it has one.
fn found_at_microsoft(address: &str, application: Option<&str>) -> AccountEvent {
    let AccountEvent::Discovered { address, mut found } = found(address) else {
        unreachable!()
    };
    found.draft.imap.host = "outlook.office365.com".to_owned();
    found.draft.smtp = Some(ServerDraft {
        host: "smtp.office365.com".to_owned(),
        port: 587,
        tls: TlsMode::Starttls,
        username: address.clone(),
    });
    found.note = Some(
        "Mail for example.com is hosted by Microsoft. Microsoft takes no password from a mail program. Its accounts sign in with Microsoft."
            .to_owned(),
    );
    found.microsoft_application = application.map(str::to_owned);
    AccountEvent::Discovered { address, found }
}

/// The form open at the server step for an address Microsoft hosts.
fn at_microsoft(width: u16, height: u16, application: Option<&str>) -> Harness {
    let mut harness = Harness::with_accounts(width, height);
    harness.keys("A");
    harness.type_text("robin@example.com");
    harness.keys("enter");
    harness.act(Action::Account(found_at_microsoft(
        "robin@example.com",
        application,
    )));
    harness
}

fn problem() -> Box<CertificateProblem> {
    Box::new(CertificateProblem {
        host: "imap.example.com".to_owned(),
        reason: "UnknownIssuer".to_owned(),
        fingerprint: Some("0123456789abcdef".repeat(4)),
        certificate: None,
        details: Some(CertificateDetails {
            subject: "CN=imap.example.com".to_owned(),
            issuer: "CN=Example Home Server".to_owned(),
            not_before: common::NOW - 86_400,
            not_after: common::NOW + 30 * 86_400,
        }),
    })
}

/// The form open at the password step, with a password typed.
fn at_password(width: u16, height: u16) -> Harness {
    let mut harness = Harness::with_accounts(width, height);
    harness.keys("A");
    harness.type_text("robin@example.com");
    harness.keys("enter");
    harness.act(Action::Account(found("robin@example.com")));
    harness.keys("enter");
    harness.type_text("hunter2");
    harness
}

#[test]
fn the_demo_mailbox_cannot_take_accounts() {
    let mut harness = Harness::demo(120, 30);
    assert!(
        !harness
            .state
            .is_enabled(&harness.env, CommandId::AccountAdd)
    );
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn an_account_is_added_from_address_to_syncing() {
    let mut harness = Harness::with_accounts(120, 30);
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::Setup);
    assert!(harness.screen().contains(" Add an account "));

    // Letters that are commands elsewhere are typing here.
    let selected = harness.state.list.selected;
    harness.type_text("jq?@example.com");
    assert_eq!(
        harness.state.setup.as_ref().unwrap().address.text(),
        "jq?@example.com"
    );
    assert_eq!(harness.state.list.selected, selected);
    assert!(harness.state.help.is_none() && !harness.state.should_quit);

    harness.keys("ctrl-u");
    harness.type_text("robin@example.com");
    harness.keys("enter");
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Discover {
            address: "robin@example.com".to_owned()
        }]
    );
    assert!(
        harness
            .screen()
            .contains("Looking for the mail server of example.com")
    );

    harness.act(Action::Account(found("robin@example.com")));
    let screen = harness.screen();
    assert!(screen.contains("Found in the DNS records of example.com."));
    assert!(
        screen.contains("imap.example.com") && screen.contains("993") && screen.contains("TLS")
    );

    harness.keys("enter");
    assert!(harness.screen().contains("Keychain and nowhere else"));
    harness.type_text("hunter2");
    // The password is never on screen.
    assert!(!harness.screen().contains("hunter2"));
    assert!(harness.screen().contains("•••••••"));

    harness.keys("enter");
    let Some(AccountRequest::Verify {
        draft,
        password,
        trusting,
    }) = harness.account_requests.last()
    else {
        panic!("{:?}", harness.account_requests);
    };
    assert_eq!(
        (draft.name.as_str(), draft.imap.host.as_str()),
        ("Example", "imap.example.com")
    );
    assert_eq!(password.0, "hunter2");
    assert_eq!(*trusting, Trusting::System);
    assert!(harness.screen().contains("Signing in to imap.example.com"));

    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    assert!(matches!(
        harness.account_requests.last(),
        Some(AccountRequest::Save { .. })
    ));
    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "Example".to_owned()
    ))));
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.state.setup.is_none());
    assert!(
        harness
            .screen()
            .contains("Added Example. Its mail is on the way.")
    );
}

#[test]
fn a_microsoft_account_is_added_without_a_password_and_goes_on_to_signing_in() {
    let mut harness = at_microsoft(120, 30, None);
    let screen = harness.screen();
    assert!(screen.contains(" Add an account: Microsoft "), "{screen}");
    assert!(screen.contains("Microsoft (sign in)"), "{screen}");
    assert!(screen.contains("robin@example.com"), "{screen}");
    assert!(
        screen.contains("Application id:") && screen.contains("Tenant:"),
        "{screen}"
    );
    assert!(screen.contains("docs/oauth.md"), "{screen}");
    // It has no servers to confirm.
    for absent in ["Server:", "Port:", "Security:", "Send via:", "Sign in:"] {
        assert!(!screen.contains(absent), "{absent}\n{screen}");
    }

    // The cursor is in the application id, and letters are typing there.
    let selected = harness.state.list.selected;
    harness.type_text("jq-app-1");
    assert_eq!(
        harness.state.setup.as_ref().unwrap().application.text(),
        "jq-app-1"
    );
    assert_eq!(harness.state.list.selected, selected);
    assert!(!harness.state.should_quit);

    harness.keys("enter");
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::SaveSignIn {
            draft: SignInDraft {
                name: "Example".to_owned(),
                address: "robin@example.com".to_owned(),
                application_id: "jq-app-1".to_owned(),
                tenant: None,
                google: None,
            }
        })
    );
    assert!(harness.screen().contains("Saving Example"));

    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "Example".to_owned()
    ))));
    // The form is gone and the sign-in has its place, for that account.
    assert!(harness.state.setup.is_none());
    assert_eq!(harness.state.context(), Context::SignIn);
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::SignIn {
            account: Some("Example".to_owned())
        })
    );
    let screen = harness.screen();
    assert!(screen.contains(" Sign in to Example "), "{screen}");
    assert!(screen.contains("Getting ready to sign in"), "{screen}");
    // On the way no password was asked for, checked or saved.
    assert!(!harness.account_requests.iter().any(|request| matches!(
        request,
        AccountRequest::Verify { .. } | AccountRequest::Save { .. }
    )));

    // The new account says at once that nobody has signed in to it. That
    // is what is being done, and it is not put in front of the user.
    harness.act(Action::Sync(SyncEvent::Problem {
        account: "Example".to_owned(),
        message: "nobody has signed in to it yet.".to_owned(),
    }));
    assert!(harness.state.notices.is_empty());

    // From here it is the sign-in there always was.
    harness.act(Action::Account(AccountEvent::SignInBrowser {
        account: "Example".to_owned(),
        url: "https://login.example.com/authorize?client_id=jq-app-1".to_owned(),
    }));
    assert_eq!(
        harness.host_requests.last(),
        Some(&quill_tui::state::HostRequest::OpenUrl(
            "https://login.example.com/authorize?client_id=jq-app-1".to_owned()
        ))
    );
    harness.act(Action::Account(AccountEvent::SignedIn {
        account: "Example".to_owned(),
        result: Ok(()),
    }));
    assert_ne!(harness.state.context(), Context::SignIn);
    assert!(
        harness
            .screen()
            .contains("Signed in to Example. Its mail is on the way.")
    );
    assert_eq!(
        harness.sync_requests.last(),
        Some(&quill_sync::SyncRequest::SyncNow)
    );
    // Signed in, the account is heard like any other.
    harness.keys("esc");
    harness.act(Action::Sync(SyncEvent::Problem {
        account: "Example".to_owned(),
        message: "Cannot reach the server.".to_owned(),
    }));
    assert!(
        harness
            .screen()
            .contains("Example: Cannot reach the server.")
    );
}

#[test]
fn a_gmail_account_can_sign_in_with_google_and_asks_for_no_password() {
    let mut harness = Harness::first_run(120, 30);
    harness.type_text("robin@gmail.com");
    harness.keys("enter");
    let AccountEvent::Discovered { address, mut found } = found("robin@gmail.com") else {
        unreachable!()
    };
    found.draft.name = "Gmail".to_owned();
    found.draft.imap.host = "imap.gmail.com".to_owned();
    found.draft.smtp.as_mut().unwrap().host = "smtp.gmail.com".to_owned();
    found.note =
        Some("Gmail's usual servers. Google does not accept your normal password here.".to_owned());
    harness.act(Action::Account(AccountEvent::Discovered { address, found }));
    // With no application to sign in as, it is offered with a password.
    let screen = harness.screen();
    assert!(screen.contains("IMAP (password)"), "{screen}");
    assert!(screen.contains("imap.gmail.com"), "{screen}");

    // The type is the user's to change: the first field, one to the left.
    harness.keys("shift-tab left");
    let screen = harness.screen();
    assert!(screen.contains(" Add an account: Google "), "{screen}");
    assert!(screen.contains("Google (sign in)"), "{screen}");
    assert!(screen.contains("robin@gmail.com"), "{screen}");
    assert!(
        screen.contains("Client ID:") && screen.contains("Client secret:"),
        "{screen}"
    );
    assert!(screen.contains("docs/oauth.md"), "{screen}");
    // It has no servers to confirm, and what was said of passwords is
    // not said of an account that has none.
    for absent in [
        "Server:",
        "Port:",
        "Security:",
        "Send via:",
        "normal password",
    ] {
        assert!(!screen.contains(absent), "{absent}\n{screen}");
    }
    harness.keys("tab tab");
    harness.type_text("123-abc.apps.googleusercontent.com");
    harness.keys("tab");
    harness.type_text("GOCSPX-not-a-real-one");
    harness.keys("enter");
    let Some(AccountRequest::SaveSignIn { draft }) = harness.account_requests.last() else {
        panic!("{:?}", harness.account_requests.last());
    };
    assert_eq!(draft.application_id, "123-abc.apps.googleusercontent.com");
    let google = draft.google.as_ref().unwrap();
    assert_eq!(
        google.client_secret.as_deref(),
        Some("GOCSPX-not-a-real-one")
    );
    assert_eq!(google.imap.host, "imap.gmail.com");
    assert!(harness.screen().contains("Saving Gmail"));
    // Saved: the sign-in takes the form's place, and is asked for.
    harness.act(Action::Account(AccountEvent::Saved(Ok("Gmail".to_owned()))));
    assert!(harness.state.setup.is_none());
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::SignIn {
            account: Some("Gmail".to_owned())
        })
    );
    assert_eq!(harness.state.context(), quill_tui::command::Context::SignIn);
}

#[test]
fn only_the_account_just_added_is_hushed_while_it_is_signed_in_to() {
    let mut harness = at_microsoft(120, 30, Some("app-1"));
    harness.keys("enter");
    harness.act(Action::Account(AccountEvent::Saved(Ok(
        "Example".to_owned()
    ))));
    assert_eq!(harness.state.context(), Context::SignIn);
    // Another account's trouble is still told.
    harness.act(Action::Sync(SyncEvent::Problem {
        account: "Work".to_owned(),
        message: "Cannot reach the server.".to_owned(),
    }));
    assert!(harness.screen().contains("Work: Cannot reach the server."));

    // Giving up on the sign-in keeps the account, and says what it still
    // needs, since the account itself was not heard.
    harness.keys("esc");
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::CancelSignIn)
    );
    assert_ne!(harness.state.context(), Context::SignIn);
    assert!(harness.state.setup.is_none());
    harness.keys("esc");
    let screen = harness.screen();
    assert!(
        screen.contains("Added Example, with nobody signed in to it yet."),
        "{screen}"
    );

    // And a sign-in asked for with `S` hushes nobody, and giving up on it
    // says nothing.
    let mut harness = Harness::with_accounts(120, 30);
    harness.keys("S");
    harness.act(Action::Account(AccountEvent::SignInBrowser {
        account: "Work".to_owned(),
        url: "https://login.example.com/authorize".to_owned(),
    }));
    harness.act(Action::Sync(SyncEvent::Problem {
        account: "Work".to_owned(),
        message: "Cannot reach the server.".to_owned(),
    }));
    assert!(harness.screen().contains("Work: Cannot reach the server."));
    harness.keys("esc");
    assert_eq!(harness.state.notices.len(), 1);
}

#[test]
fn the_type_is_changed_in_the_form_and_a_microsoft_server_takes_no_password_account() {
    // An address Quill takes for a password account can be made a
    // Microsoft one: the type is the field above the name.
    let mut harness = Harness::with_accounts(120, 30);
    harness.keys("A");
    harness.type_text("robin@example.com");
    harness.keys("enter");
    harness.act(Action::Account(found("robin@example.com")));
    let screen = harness.screen();
    assert!(screen.contains("IMAP (password)"), "{screen}");
    assert!(screen.contains("Server:"), "{screen}");
    harness.keys("shift-tab");
    // A choice has no cursor, and says how it is changed.
    assert_eq!(harness.cursor(), None);
    assert!(harness.screen().contains("←/→ to change"));
    harness.keys("right");
    let screen = harness.screen();
    assert!(
        screen.contains("Microsoft (sign in)") && screen.contains("Application id:"),
        "{screen}"
    );
    assert!(!screen.contains("Server:"), "{screen}");
    harness.keys("left");
    assert!(harness.screen().contains("Server:"));

    // At one of Microsoft's servers a password account goes no further.
    let mut harness = at_microsoft(120, 30, None);
    harness.keys("shift-tab shift-tab left enter");
    let setup = harness.state.setup.as_ref().unwrap();
    assert_eq!(setup.step, quill_tui::setup::Step::Server);
    let screen = harness.screen();
    assert!(
        screen.contains("Microsoft takes no password. Change Type to Microsoft."),
        "{screen}"
    );
    assert!(!screen.contains("Password:"), "{screen}");
    // Nothing was asked of anyone but to find the server.
    assert!(
        harness
            .account_requests
            .iter()
            .all(|request| matches!(request, AccountRequest::Discover { .. }))
    );
    // The cursor is on the type: one key, and the account can be added.
    harness.keys("right");
    assert!(harness.screen().contains("Application id:"));
}

#[test]
fn in_a_narrow_window_what_the_form_says_last_is_not_cut_off() {
    // 58 columns of text in a dialog 16 rows deep: the app's window as it
    // first opens. What is said under the fields has to fit there.
    let narrow = |application| {
        let mut harness = Harness::with_accounts(66, 24);
        harness.keys("A");
        harness.type_text("robin.vale@outlook.example");
        harness.keys("enter");
        let AccountEvent::Discovered { address, mut found } =
            found_at_microsoft("robin.vale@outlook.example", application)
        else {
            unreachable!()
        };
        // As long as the note gets: the one for a Microsoft address.
        found.note = Some(
            "robin.vale@outlook.example is a Microsoft address. Microsoft takes no password from a mail program. Its accounts sign in with Microsoft."
                .to_owned(),
        );
        harness.act(Action::Account(AccountEvent::Discovered { address, found }));
        harness
    };

    // The Microsoft account, with nothing typed and the id asked for.
    let mut harness = narrow(None);
    harness.keys("enter");
    let screen = harness.screen();
    assert!(screen.contains("Tenant may be left empty."), "{screen}");
    assert!(screen.contains("for Quill."), "{screen}");

    // A password account at Microsoft's server: every field of a server,
    // and still the way out under them.
    let mut harness = narrow(Some("app-1"));
    harness.keys("shift-tab left enter");
    let screen = harness.screen();
    assert!(screen.contains("Security:  STARTTLS"), "{screen}");
    assert!(
        screen.contains("Microsoft takes no password. Change Type to Microsoft."),
        "{screen}"
    );
}

#[test]
fn the_cursor_sits_in_the_field_being_typed_into() {
    let mut harness = Harness::with_accounts(120, 30);
    assert_eq!(harness.cursor(), None);
    harness.keys("A");
    let empty = harness.cursor().expect("a cursor in the address field");
    harness.type_text("robin");
    let typed = harness.cursor().unwrap();
    assert_eq!((typed.0, typed.1), (empty.0 + 5, empty.1));
    harness.keys("left left");
    assert_eq!(harness.cursor().unwrap().0, empty.0 + 3);

    // In the server step it follows the field, and a choice has none.
    harness.type_text("@example.com");
    harness.keys("enter");
    harness.act(Action::Account(found("robinrobin@example.com")));
    harness.act(Action::Account(found("ro@example.combin")));
    let address = harness
        .state
        .setup
        .as_ref()
        .unwrap()
        .address
        .text()
        .to_owned();
    harness.act(Action::Account(found(&address)));
    let name_row = harness.cursor().unwrap().1;
    harness.keys("tab");
    assert_eq!(harness.cursor().unwrap().1, name_row + 1);
    harness.keys("tab tab");
    assert_eq!(
        harness.cursor(),
        None,
        "{:?}\n{}",
        harness
            .state
            .setup
            .as_ref()
            .map(|setup| (setup.step, setup.field)),
        harness.screen()
    );
    // Waiting, there is nothing to type into.
    harness.keys("enter");
    harness.type_text("pw");
    harness.keys("enter");
    assert_eq!(harness.cursor(), None);
}

#[test]
fn pasted_text_goes_into_the_field() {
    let mut harness = Harness::with_accounts(120, 30);
    // With no field open a paste is not a string of commands.
    harness.act(Action::Paste("qqq".to_owned()));
    assert!(!harness.state.should_quit);
    harness.keys("A");
    harness.act(Action::Paste("robin@example.com\n".to_owned()));
    assert_eq!(
        harness.state.setup.as_ref().unwrap().address.text(),
        "robin@example.com "
    );
}

#[test]
fn esc_steps_back_and_then_closes() {
    let mut harness = at_password(120, 30);
    harness.keys("esc");
    assert!(harness.screen().contains("the server"));
    harness.keys("esc");
    assert!(harness.screen().contains("Type the email address"));
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    // Nothing was verified or saved on the way.
    assert!(
        harness
            .account_requests
            .iter()
            .all(|request| matches!(request, AccountRequest::Discover { .. }))
    );
}

#[test]
fn a_first_run_opens_the_form_once() {
    let mut harness = Harness::first_run(120, 30);
    assert_eq!(harness.state.context(), Context::Setup);
    assert!(harness.screen().contains("Type the email address"));
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    // Closed is closed: it does not come back on the next refresh.
    harness.act(Action::Data(quill_tui::data::DataEvent::StoreChanged));
    assert_eq!(harness.state.context(), Context::List);
    // It is still there to ask for.
    harness.keys("A");
    assert_eq!(harness.state.context(), Context::Setup);
}

#[test]
fn an_untrusted_certificate_at_setup_can_be_pinned() {
    let mut harness = at_password(120, 40);
    harness.keys("enter");
    harness.act(Action::Account(AccountEvent::Verified(Err(
        VerifyProblem::Certificate(problem()),
    ))));
    let screen = harness.screen();
    assert!(screen.contains("presented a certificate that was not accepted"));
    assert!(screen.contains("self-signed"));
    assert!(screen.contains("CN=imap.example.com") && screen.contains("CN=Example Home Server"));
    assert!(screen.contains("01:23:45:67:89:ab:cd:ef"));

    harness.keys("p");
    let Some(AccountRequest::Verify { trusting, .. }) = harness.account_requests.last() else {
        panic!("{:?}", harness.account_requests);
    };
    assert_eq!(*trusting, Trusting::Pinned("0123456789abcdef".repeat(4)));
    harness.act(Action::Account(AccountEvent::Verified(Ok(()))));
    assert!(matches!(
        harness.account_requests.last(),
        Some(AccountRequest::Save {
            trusting: Trusting::Pinned(_),
            ..
        })
    ));
}

#[test]
fn an_existing_accounts_certificate_is_asked_about_before_anything_else() {
    let mut harness = Harness::with_accounts(120, 40);
    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: problem(),
        sending: false,
    }));
    assert_eq!(harness.state.context(), Context::Certificate);
    let screen = harness.screen();
    assert!(screen.contains(" Work: certificate "));
    assert!(screen.contains("trust once") && screen.contains("always trust this certificate"));

    // Keys answer the question or do nothing: none reaches the list.
    let selected = harness.state.list.selected;
    harness.type_text("jGqA?");
    assert_eq!(harness.state.list.selected, selected);
    assert!(!harness.state.should_quit && harness.state.setup.is_none());
    assert!(harness.account_requests.is_empty());

    harness.keys("o");
    assert_eq!(
        harness.account_requests,
        [AccountRequest::Trust {
            account: "Work".to_owned(),
            fingerprint: "0123456789abcdef".repeat(4),
            pin: false,
            sending: false,
        }]
    );
    // Answered once: a second press asks for nothing more.
    harness.keys("p");
    assert_eq!(harness.account_requests.len(), 1);
    assert!(harness.screen().contains("Working…"));

    harness.act(Action::Account(AccountEvent::Trusted {
        account: "Work".to_owned(),
        sending: false,
        result: Ok(()),
    }));
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn rejecting_a_certificate_sends_nothing_and_a_failure_is_reported() {
    let mut harness = Harness::with_accounts(120, 40);
    for account in ["Work", "Personal"] {
        harness.act(Action::Sync(SyncEvent::Certificate {
            account: account.to_owned(),
            problem: problem(),
            sending: false,
        }));
    }
    // A repeat for the same account replaces its question; it does not queue.
    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: problem(),
        sending: false,
    }));
    assert_eq!(harness.state.certificates.len(), 2);

    // The first question is Personal's now; `r` and `esc` both reject.
    assert!(harness.screen().contains(" Personal: certificate "));
    harness.keys("r");
    assert!(harness.screen().contains(" Work: certificate "));
    harness.keys("esc");
    assert_eq!(harness.state.context(), Context::List);
    assert!(harness.account_requests.is_empty());

    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: problem(),
        sending: false,
    }));
    harness.keys("p");
    harness.act(Action::Account(AccountEvent::Trusted {
        account: "Work".to_owned(),
        sending: false,
        result: Err("config.toml is read-only".to_owned()),
    }));
    assert_eq!(harness.state.context(), Context::List);
    assert!(
        harness
            .screen()
            .contains("the certificate could not be accepted: config.toml is read-only")
    );
}

#[test]
fn a_certificate_that_cannot_be_read_can_only_be_rejected() {
    let mut harness = Harness::with_accounts(120, 40);
    let mut unreadable = problem();
    unreadable.fingerprint = None;
    unreadable.details = None;
    harness.act(Action::Sync(SyncEvent::Certificate {
        account: "Work".to_owned(),
        problem: unreadable,
        sending: false,
    }));
    assert!(harness.screen().contains("could not be read"));
    harness.keys("o p");
    assert!(harness.account_requests.is_empty());
    assert_eq!(harness.state.context(), Context::Certificate);
    harness.keys("r");
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn hostile_text_from_a_server_is_made_safe_on_these_screens() {
    let mut harness = at_password(120, 40);
    harness.keys("enter");
    let mut hostile = problem();
    hostile.host = "imap.example.com\u{1b}[2J".to_owned();
    hostile.reason = "weird\u{7}\u{1b}]0;title\u{7}".to_owned();
    hostile.details.as_mut().unwrap().subject = "CN=evil\u{1b}[31m".to_owned();
    harness.act(Action::Account(AccountEvent::Verified(Err(
        VerifyProblem::Certificate(hostile),
    ))));
    let screen = harness.screen();
    assert!(!screen.contains('\u{1b}') && !screen.contains('\u{7}'));
    assert!(screen.contains("CN=evil") && !screen.contains("[31m"));

    harness.keys("r");
    harness.keys("enter enter");
    harness.act(Action::Account(AccountEvent::Verified(Err(
        VerifyProblem::Auth("NO \u{1b}[5mAUTHENTICATIONFAILED".to_owned()),
    ))));
    let screen = harness.screen();
    assert!(!screen.contains('\u{1b}'));
    assert!(screen.contains("AUTHENTICATIONFAILED"));
}

#[test]
fn snapshots_of_the_form_and_the_certificate_question() {
    for (width, height) in [(80u16, 24u16), (200, 60)] {
        let snap = |name: &str, harness: &Harness| {
            insta::assert_snapshot!(format!("{name}_{width}x{height}"), harness.screen());
        };
        let mut harness = Harness::with_accounts(width, height);
        harness.keys("A");
        harness.type_text("robin@example.com");
        snap("setup_address", &harness);
        harness.keys("enter");
        harness.act(Action::Account(found("robin@example.com")));
        harness.keys("tab tab tab");
        snap("setup_server", &harness);
        harness.keys("right right");
        snap("setup_server_plaintext", &harness);
        harness.keys("right enter");
        harness.type_text("hunter2");
        snap("setup_password", &harness);
        harness.keys("enter");
        harness.act(Action::Account(AccountEvent::Verified(Err(
            VerifyProblem::Certificate(problem()),
        ))));
        snap("setup_certificate", &harness);

        // Reading works and sending does not: back to the sending server.
        let mut harness = Harness::with_accounts(width, height);
        harness.keys("A");
        harness.type_text("robin@example.com");
        harness.keys("enter");
        harness.act(Action::Account(found("robin@example.com")));
        harness.keys("enter");
        harness.type_text("hunter2");
        harness.keys("enter");
        harness.act(Action::Account(AccountEvent::Verified(Err(
            VerifyProblem::Sending("Cannot reach the server: connection refused.".to_owned()),
        ))));
        snap("setup_sending_failed", &harness);

        // An address Microsoft hosts: the account that signs in with
        // Microsoft, first as it opens and then filled in but for the id.
        let mut harness = at_microsoft(width, height, None);
        snap("setup_microsoft", &harness);
        harness.keys("tab");
        harness.type_text("example.com");
        harness.keys("enter");
        snap("setup_microsoft_no_id", &harness);

        // A password account asked for at Microsoft's server all the same.
        let mut harness = at_microsoft(width, height, Some("app-1"));
        harness.keys("shift-tab left enter");
        snap("setup_microsoft_as_imap", &harness);

        let mut harness = Harness::with_accounts(width, height);
        harness.act(Action::Sync(SyncEvent::Certificate {
            account: "Work".to_owned(),
            problem: problem(),
            sending: false,
        }));
        snap("certificate_prompt", &harness);
    }
}

#[test]
fn the_server_mail_is_sent_through_is_asked_about_separately() {
    let mut harness = Harness::with_accounts(120, 40);
    // Both of one account's servers can be in question at once.
    for sending in [false, true] {
        harness.act(Action::Sync(SyncEvent::Certificate {
            account: "Work".to_owned(),
            problem: problem(),
            sending,
        }));
    }
    assert_eq!(harness.state.certificates.len(), 2);
    assert!(harness.screen().contains(" Work: certificate "));
    // Answering the first leaves the second, which says what it is for.
    harness.keys("p");
    harness.act(Action::Account(AccountEvent::Trusted {
        account: "Work".to_owned(),
        sending: false,
        result: Ok(()),
    }));
    let screen = harness.screen();
    assert!(
        screen.contains(" Work: certificate for sending "),
        "{screen}"
    );
    assert!(screen.contains("sends mail through"), "{screen}");
    assert!(screen.contains("in the Outbox for it."), "{screen}");

    harness.keys("o");
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::Trust {
            account: "Work".to_owned(),
            fingerprint: "0123456789abcdef".repeat(4),
            pin: false,
            sending: true,
        })
    );
    harness.act(Action::Account(AccountEvent::Trusted {
        account: "Work".to_owned(),
        sending: true,
        result: Ok(()),
    }));
    assert_eq!(harness.state.context(), Context::List);
}

#[test]
fn signing_in_opens_the_browser_and_waits_where_nothing_else_can_be_done() {
    let mut harness = Harness::with_accounts(120, 40);
    let first = harness.selected_subject().to_owned();
    harness.keys("S");
    assert_eq!(harness.state.context(), Context::SignIn);
    // The account under the cursor is named; the service decides.
    assert!(matches!(
        harness.account_requests.last(),
        Some(AccountRequest::SignIn { account: Some(_) })
    ));
    assert!(harness.screen().contains("Getting ready to sign in"));

    harness.act(Action::Account(AccountEvent::SignInBrowser {
        account: "Work".to_owned(),
        url: "https://login.example.com/authorize?client_id=app-1".to_owned(),
    }));
    assert_eq!(
        harness.host_requests.last(),
        Some(&quill_tui::state::HostRequest::OpenUrl(
            "https://login.example.com/authorize?client_id=app-1".to_owned()
        ))
    );
    let screen = harness.screen();
    assert!(screen.contains(" Sign in to Work "), "{screen}");
    assert!(screen.contains("Your browser has been opened"), "{screen}");
    assert!(screen.contains("esc give up"), "{screen}");

    // Letters do nothing behind the dialog: no delete, no archive.
    let before = harness.state.list.rows.len();
    harness.keys("d e j");
    assert_eq!(harness.state.list.rows.len(), before);
    assert_eq!(harness.selected_subject(), first);

    harness.act(Action::Account(AccountEvent::SignedIn {
        account: "Work".to_owned(),
        result: Ok(()),
    }));
    assert_ne!(harness.state.context(), Context::SignIn);
    assert!(
        harness
            .screen()
            .contains("Signed in to Work. Its mail is on the way.")
    );
    assert_eq!(
        harness.sync_requests.last(),
        Some(&quill_sync::SyncRequest::SyncNow)
    );
}

#[test]
fn a_code_to_type_elsewhere_is_shown_and_escape_gives_up() {
    let mut harness = Harness::with_accounts(120, 40);
    harness.act(Action::Command(CommandId::AccountSignIn));
    harness.act(Action::Account(AccountEvent::SignInCode {
        account: "Work".to_owned(),
        address: "https://login.example.com/device".to_owned(),
        code: "WXYZ-1234".to_owned(),
        minutes: 15,
    }));
    let screen = harness.screen();
    assert!(
        screen.contains("https://login.example.com/device"),
        "{screen}"
    );
    assert!(screen.contains("WXYZ-1234"), "{screen}");
    assert!(screen.contains("good for 15 minutes"), "{screen}");
    // No browser was opened on a Mac nobody is sitting at.
    assert!(harness.host_requests.is_empty());

    harness.keys("esc");
    assert_ne!(harness.state.context(), Context::SignIn);
    assert_eq!(
        harness.account_requests.last(),
        Some(&AccountRequest::CancelSignIn)
    );
    // An answer that was already on its way is dropped without a word.
    harness.act(Action::Account(AccountEvent::SignedIn {
        account: "Work".to_owned(),
        result: Err("Nobody finished in time.".to_owned()),
    }));
    assert!(!harness.screen().contains("Nobody finished"));
}

#[test]
fn a_sign_in_that_fails_says_why_and_an_address_that_is_no_web_page_is_not_opened() {
    let mut harness = Harness::with_accounts(120, 40);
    harness.keys("S");
    harness.act(Action::Account(AccountEvent::SignInBrowser {
        account: "Work".to_owned(),
        url: "file:///etc/passwd".to_owned(),
    }));
    assert!(harness.host_requests.is_empty());
    harness.act(Action::Account(AccountEvent::SignedIn {
        account: "Work".to_owned(),
        result: Err("The provider said no: access_denied.".to_owned()),
    }));
    assert!(
        harness
            .screen()
            .contains("Work is not signed in: The provider said no: access_denied."),
        "{}",
        harness.screen()
    );
    // With no account to name, the reason stands alone.
    harness.keys("esc S");
    harness.act(Action::Account(AccountEvent::SignedIn {
        account: String::new(),
        result: Err("No account here signs in with Microsoft or Google.".to_owned()),
    }));
    assert!(
        harness
            .screen()
            .contains("No account here signs in with Microsoft or Google.")
    );
}
