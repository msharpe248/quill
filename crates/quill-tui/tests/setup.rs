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
    AccountDraft, AccountEvent, AccountRequest, Discovery, ServerDraft, Trusting, VerifyProblem,
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
                smtp: None,
            },
            found: true,
            note: Some("Found in the DNS records of example.com.".to_owned()),
        },
    }
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
