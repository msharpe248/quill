use jiff::tz::TimeZone;
use quill_core::CertificateDetails;
use quill_host::bridge::{self, CoreToShell};

use super::super::tests::{
    at_google, at_microsoft, at_password, found, found_at_microsoft, keys, problem,
};
use super::super::{
    AccountDraft, AccountEvent, GOOGLE_NOTE, GOOGLE_OWN_NOTE, Outcome, Secret, SignInDraft,
    Trusting, VerifyProblem,
};
use super::*;

fn form(setup: &Setup) -> FormShow {
    setup.form(&TimeZone::UTC).unwrap()
}

/// The names of the form's fields, in order.
fn ids(form: &FormShow) -> Vec<&str> {
    form.fields().iter().map(FormField::id).collect()
}

fn field<'a>(form: &'a FormShow, id: &str) -> &'a FormField {
    form.fields()
        .iter()
        .find(|field| field.id() == id)
        .unwrap_or_else(|| panic!("no field {id} in {form:?}"))
}

/// The field with the focus.
fn focus(form: &FormShow) -> Option<&str> {
    form.fields()
        .iter()
        .find(|field| field.focus())
        .map(FormField::id)
}

/// Each button as its name, the key that presses it and whether it can be
/// pressed.
fn buttons(form: &FormShow) -> Vec<(&str, Option<FormRole>, bool)> {
    form.buttons()
        .iter()
        .map(|button| (button.id.as_str(), button.role, button.enabled))
        .collect()
}

/// What the sheet sends: a button, or none for a changed choice, with what
/// its fields held.
fn press(
    setup: &mut Setup,
    button: Option<Press>,
    values: &[(&str, &str)],
    secrets: &[(&str, &str)],
) -> Used {
    let used = FormUsed::new(setup.step.id(), button.map(Press::id), values, secrets).unwrap();
    setup.used(&used)
}

/// The form at the server step, for an address a password server was found
/// for.
fn at_server() -> Setup {
    let mut setup = Setup::default();
    setup.address.set("robin@example.com");
    setup.advance();
    setup.on_event(found("robin@example.com"));
    assert_eq!(setup.step, Step::Server);
    setup
}

fn at_certificate() -> Setup {
    let mut setup = at_password();
    setup.advance();
    let mut problem = problem();
    problem.details = Some(CertificateDetails {
        subject: "CN=imap.example.com".to_owned(),
        issuer: "CN=Example Home Server".to_owned(),
        not_before: 1_794_562_200,
        not_after: 1_797_240_600,
    });
    setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
        problem,
    ))));
    assert_eq!(setup.step, Step::Certificate);
    setup
}

const CANCEL: (&str, Option<FormRole>, bool) = ("cancel", Some(FormRole::Cancel), true);

#[test]
fn the_first_step_asks_for_the_address() {
    let mut setup = Setup::default();
    let shown = form(&setup);
    assert_eq!((shown.step(), shown.title()), ("address", "Add an Account"));
    assert_eq!(shown.note(), Some(ADDRESS_NOTE));
    assert_eq!((shown.error(), shown.busy()), (None, None));
    assert_eq!(ids(&shown), ["address"]);
    assert_eq!(
        field(&shown, "address"),
        &FormField::Text {
            id: "address".to_owned(),
            label: "Email Address:".to_owned(),
            value: String::new(),
            focus: true,
            help: None,
        }
    );
    // There is no step to go back to: cancel, and continue on return.
    assert_eq!(
        buttons(&shown),
        [CANCEL, ("continue", Some(FormRole::Default), true)]
    );

    // What is not an address is said under the field, which keeps what
    // was typed.
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[("address", "robin")],
        &[],
    );
    assert_eq!(used, Used::default());
    let shown = form(&setup);
    assert_eq!(
        shown.error(),
        Some("That does not look like an email address.")
    );
    assert_eq!(field(&shown, "address").value(), Some("robin"));

    // An address: Quill goes looking, and says so. Nothing can be pressed
    // meanwhile but cancel.
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[("address", " robin@example.com ")],
        &[],
    );
    assert_eq!(
        used.requests,
        [AccountRequest::Discover {
            address: "robin@example.com".to_owned()
        }]
    );
    assert!(!used.close);
    let shown = form(&setup);
    assert_eq!(
        shown.busy(),
        Some("Looking for the mail server of example.com…")
    );
    assert_eq!(shown.error(), None);
    // What the core made of the address is what the field now holds.
    assert_eq!(field(&shown, "address").value(), Some("robin@example.com"));
    assert_eq!(
        buttons(&shown),
        [CANCEL, ("continue", Some(FormRole::Default), false)]
    );
}

#[test]
fn the_server_step_of_a_password_account_has_its_servers() {
    let setup = at_server();
    let shown = form(&setup);
    assert_eq!(
        (shown.step(), shown.title()),
        ("server", "Add an Account: The Server")
    );
    assert_eq!(
        shown.note(),
        Some("Found in the DNS records of example.com.")
    );
    assert_eq!(
        ids(&shown),
        [
            "type",
            "name",
            "host",
            "port",
            "security",
            "username",
            "smtp_host",
            "smtp_port",
            "smtp_security"
        ]
    );
    let labels: Vec<&str> = shown.fields().iter().map(FormField::label).collect();
    assert_eq!(
        labels,
        [
            "Type:",
            "Name:",
            "Incoming Server:",
            "Port:",
            "Security:",
            "User Name:",
            "Outgoing Server:",
            "Port:",
            "Security:"
        ]
    );
    let values: Vec<Option<&str>> = shown.fields().iter().map(FormField::value).collect();
    assert_eq!(
        values,
        [
            Some("imap"),
            Some("Example"),
            Some("imap.example.com"),
            Some("993"),
            Some("tls"),
            Some("robin@example.com"),
            Some("smtp.example.com"),
            Some("465"),
            Some("tls")
        ]
    );
    // The type is one of the kinds there are, by the names the terminal
    // gives them.
    let FormField::Choice { options, .. } = field(&shown, "type") else {
        panic!("{shown:?}");
    };
    let named: Vec<(&str, &str)> = options
        .iter()
        .map(|option| (option.id.as_str(), option.title.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("imap", "IMAP (password)"),
            ("microsoft", "Microsoft (sign in)"),
            ("google", "Google (sign in)")
        ]
    );
    let FormField::Choice { options, .. } = field(&shown, "smtp_security") else {
        panic!("{shown:?}");
    };
    let named: Vec<(&str, &str)> = options
        .iter()
        .map(|option| (option.id.as_str(), option.title.as_str()))
        .collect();
    assert_eq!(
        named,
        [("tls", "TLS"), ("starttls", "STARTTLS"), ("none", "None")]
    );
    // The cursor is where the terminal has it: on the name.
    assert_eq!(focus(&shown), Some("name"));
    assert_eq!(field(&shown, "smtp_host").help(), Some(NO_SENDING));
    assert_eq!(shown.error(), None);
    assert_eq!(
        buttons(&shown),
        [
            CANCEL,
            ("back", None, true),
            ("continue", Some(FormRole::Default), true)
        ]
    );

    // Nothing found: the cursor is on the server to fill in.
    let mut setup = Setup::default();
    setup.address.set("robin@example.com");
    setup.advance();
    let AccountEvent::Discovered { address, mut found } = found("robin@example.com") else {
        unreachable!()
    };
    found.found = false;
    found.draft.imap.host = String::new();
    found.note = None;
    setup.on_event(AccountEvent::Discovered { address, found });
    let shown = form(&setup);
    assert_eq!((shown.note(), focus(&shown)), (None, Some("host")));
}

#[test]
fn the_server_step_of_a_microsoft_account_has_no_servers() {
    let setup = at_microsoft(None);
    let shown = form(&setup);
    assert_eq!(
        (shown.step(), shown.title()),
        ("server", "Add an Account: Microsoft")
    );
    assert_eq!(
        shown.note(),
        Some("Mail for example.com is hosted by Microsoft.")
    );
    assert_eq!(
        ids(&shown),
        ["type", "name", "address", "application", "tenant"]
    );
    assert_eq!(field(&shown, "type").value(), Some("microsoft"));
    // The address is shown, and is not there to be changed.
    assert_eq!(
        field(&shown, "address"),
        &FormField::Static {
            id: "address".to_owned(),
            label: "Address:".to_owned(),
            value: "robin@example.com".to_owned(),
            help: None,
        }
    );
    assert_eq!(field(&shown, "application").label(), "Application ID:");
    assert_eq!(field(&shown, "application").value(), Some(""));
    assert_eq!(field(&shown, "tenant").help(), Some(MICROSOFT_NOTE));
    // The cursor is on the one thing it still needs.
    assert_eq!(focus(&shown), Some("application"));

    // An application id the config already has is offered, and then the
    // name is what is left to choose.
    let shown = form(&at_microsoft(Some("app-1")));
    assert_eq!(field(&shown, "application").value(), Some("app-1"));
    assert_eq!(focus(&shown), Some("name"));
}

#[test]
fn the_password_step_never_says_what_the_password_is() {
    let setup = at_password();
    assert_eq!(setup.password.text(), "hunter2");
    let shown = form(&setup);
    assert_eq!(
        (shown.step(), shown.title()),
        ("password", "Add an Account: Sign In")
    );
    assert_eq!(
        shown.note(),
        Some(
            "The password for robin@example.com at imap.example.com. It is kept in your Keychain and nowhere else."
        )
    );
    assert_eq!(
        shown.fields(),
        [FormField::Secret {
            id: "password".to_owned(),
            label: "Password:".to_owned(),
            focus: true,
            help: None,
        }]
    );
    assert_eq!(field(&shown, "password").value(), None);
    // Not in the description, however it is written out: on the bridge,
    // or printed.
    let line = bridge::encode(&CoreToShell::FormShow(shown.clone())).unwrap();
    for written in [line, format!("{shown:?}"), format!("{shown:#?}")] {
        assert!(!written.contains("hunter2"), "{written}");
    }
    // Nor is it printed with the form itself, or with what the form asks
    // for.
    let mut setup = setup;
    let requests = setup.advance();
    for printed in [format!("{setup:?}"), format!("{requests:?}")] {
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}

#[test]
fn the_certificate_step_shows_the_certificate_and_its_three_answers() {
    let setup = at_certificate();
    let shown = form(&setup);
    assert_eq!(
        (shown.step(), shown.title()),
        ("certificate", "Add an Account: Certificate")
    );
    assert_eq!(
        shown.note(),
        Some(
            "imap.example.com presented a certificate that was not accepted. Nothing has been sent to it."
        )
    );
    assert!(shown.error().unwrap().contains("self-signed"));
    assert_eq!(ids(&shown), ["issued_to", "issued_by", "valid", "sha256"]);
    // Nothing in it is there to be typed into.
    assert!(
        shown
            .fields()
            .iter()
            .all(|field| matches!(field, FormField::Static { .. }))
    );
    assert_eq!(
        field(&shown, "issued_to").value(),
        Some("CN=imap.example.com")
    );
    assert_eq!(
        field(&shown, "valid").value(),
        Some("Fri 13 Nov 2026 09:30 to Mon 14 Dec 2026 09:30")
    );
    // The fingerprint in two rows of sixteen pairs, as the terminal has it.
    let pairs = ["ab"; 16].join(":");
    assert_eq!(
        field(&shown, "sha256").value(),
        Some(format!("{pairs}\n{pairs}").as_str())
    );
    // Esc rejects, and return accepts nothing.
    assert_eq!(
        buttons(&shown),
        [
            ("reject", Some(FormRole::Cancel), true),
            ("trust_once", None, true),
            ("trust_always", None, true)
        ]
    );
    let titles: Vec<&str> = shown
        .buttons()
        .iter()
        .map(|button| button.title.as_str())
        .collect();
    assert_eq!(titles, ["Reject", "Trust Once", "Always Trust"]);

    // A certificate that could not be read can only be rejected.
    let mut setup = at_password();
    setup.advance();
    let mut unreadable = problem();
    unreadable.fingerprint = None;
    setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Certificate(
        unreadable,
    ))));
    let shown = form(&setup);
    assert_eq!(buttons(&shown), [("reject", Some(FormRole::Cancel), true)]);
    assert!(shown.error().unwrap().ends_with(UNREADABLE_CERTIFICATE));
    assert!(ids(&shown).is_empty());
    assert_eq!(
        press(&mut setup, Some(Press::TrustOnce), &[], &[]),
        Used::default()
    );
    assert_eq!(setup.step, Step::Certificate);
}

#[test]
fn what_is_wrong_is_said_under_the_fields() {
    // A complaint of the form's own.
    let mut setup = at_server();
    let used = press(&mut setup, Some(Press::Continue), &[("port", "70000")], &[]);
    assert_eq!(used, Used::default());
    assert_eq!(
        form(&setup).error(),
        Some("The port is a number from 1 to 65535.")
    );

    // No encryption is warned of there too, with whatever else is wrong.
    press(&mut setup, None, &[("security", "none")], &[]);
    assert_eq!(form(&setup).error(), Some(NO_ENCRYPTION));
    press(&mut setup, Some(Press::Continue), &[("host", "")], &[]);
    let said = form(&setup).error().unwrap().to_owned();
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        [
            NO_ENCRYPTION,
            "Enter the mail server's name, such as imap.example.com."
        ]
    );

    // A complaint that moves the cursor moves the focus.
    let mut setup = at_microsoft(Some("app-1"));
    press(
        &mut setup,
        Some(Press::Continue),
        &[("tenant", "not a tenant")],
        &[],
    );
    let shown = form(&setup);
    assert_eq!(focus(&shown), Some("tenant"));
    assert!(shown.error().unwrap().contains("tenant"));
    assert_eq!(field(&shown, "tenant").value(), Some("not a tenant"));
}

#[test]
fn what_a_server_said_reaches_the_sheet_clean() {
    let mut setup = Setup::default();
    setup.address.set("robin@example.com");
    setup.advance();
    let AccountEvent::Discovered { address, mut found } = found("robin@example.com") else {
        unreachable!()
    };
    found.draft.name = "Exa\u{1b}[31mmple\u{7}".to_owned();
    found.note = Some("Found\nin \u{1b}]0;pwned\u{7}DNS.".to_owned());
    setup.on_event(AccountEvent::Discovered { address, found });
    let shown = form(&setup);
    assert_eq!(shown.note(), Some("Found in DNS."));
    assert_eq!(field(&shown, "name").value(), Some("Example"));

    let mut setup = at_password();
    setup.advance();
    setup.on_event(AccountEvent::Verified(Err(VerifyProblem::Auth(
        "Basic authentication\nis disabled.\u{1b}[31m".to_owned(),
    ))));
    let said = form(&setup).error().unwrap().to_owned();
    assert!(!said.contains('\n') && !said.contains('\u{1b}'), "{said:?}");
}

#[test]
fn what_the_fields_held_is_taken_and_then_the_button_is_pressed() {
    // Everything the server step's fields held, in one answer.
    let mut setup = at_server();
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[
            ("type", "imap"),
            ("name", " Work "),
            ("host", "mail.example.com"),
            ("port", "1143"),
            ("security", "starttls"),
            ("username", "robin"),
            ("smtp_host", "send.example.com"),
            ("smtp_port", "2525"),
            ("smtp_security", "none"),
        ],
        &[],
    );
    // On to the password, as `enter` goes: nothing is asked for yet.
    assert_eq!(used, Used::default());
    assert_eq!(setup.step, Step::Password);
    assert_eq!(
        (
            setup.name.text(),
            setup.host.text(),
            setup.port.text(),
            setup.tls,
            setup.username.text()
        ),
        (
            " Work ",
            "mail.example.com",
            "1143",
            TlsMode::Starttls,
            "robin"
        )
    );
    assert_eq!(
        (
            setup.smtp_host.text(),
            setup.smtp_port.text(),
            setup.smtp_tls
        ),
        ("send.example.com", "2525", TlsMode::None)
    );

    // The password arrives with the button that uses it, and the request
    // is the one the terminal's `enter` makes.
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[],
        &[("password", "hunter2")],
    );
    let [
        AccountRequest::Verify {
            draft,
            password,
            trusting,
        },
    ] = used.requests.as_slice()
    else {
        panic!("{used:?}");
    };
    assert_eq!(
        *draft,
        AccountDraft {
            name: "Work".to_owned(),
            address: "robin@example.com".to_owned(),
            imap: super::super::ServerDraft {
                host: "mail.example.com".to_owned(),
                port: 1143,
                tls: TlsMode::Starttls,
                username: "robin".to_owned(),
            },
            smtp: Some(super::super::ServerDraft {
                host: "send.example.com".to_owned(),
                port: 2525,
                tls: TlsMode::None,
                username: "robin".to_owned(),
            }),
        }
    );
    assert_eq!(*password, Secret("hunter2".to_owned()));
    assert_eq!(*trusting, Trusting::System);
    assert_eq!(form(&setup).busy(), Some("Signing in to mail.example.com…"));
    // The answer that carried it does not print it.
    assert!(!format!("{used:?}").contains("hunter2"));

    // Signed in: saved, and the form is over, as in a terminal.
    let (requests, outcome) = setup.on_event(AccountEvent::Verified(Ok(())));
    assert!(matches!(requests.as_slice(), [AccountRequest::Save { .. }]));
    assert_eq!(outcome, Outcome::Open);
    let (_, outcome) = setup.on_event(AccountEvent::Saved(Ok("Work".to_owned())));
    assert_eq!(outcome, Outcome::Added("Work".to_owned()));
}

#[test]
fn an_empty_password_field_is_no_password() {
    // The sheet clears its field once it has sent what was in it. Pressed
    // again with nothing typed, there is nothing to sign in with.
    let mut setup = at_password();
    let used = press(&mut setup, Some(Press::Continue), &[], &[("password", "")]);
    assert_eq!(used, Used::default());
    assert_eq!(form(&setup).error(), Some("Enter the account's password."));
    // What cannot be on one line is not in a password either, as when it
    // is pasted into the terminal.
    press(&mut setup, None, &[], &[("password", "hun\nter\u{7}2")]);
    assert_eq!(setup.password.text(), "hun ter2");
}

#[test]
fn a_microsoft_account_is_saved_from_the_sheet_and_goes_on_to_signing_in() {
    let mut setup = at_microsoft(None);
    // No id: nothing is asked for, and the focus is where it goes.
    assert_eq!(
        press(&mut setup, Some(Press::Continue), &[("name", "Work")], &[]),
        Used::default()
    );
    let shown = form(&setup);
    assert!(shown.error().unwrap().contains("application id"));
    assert_eq!(focus(&shown), Some("application"));
    assert_eq!(field(&shown, "name").value(), Some("Work"));

    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[
            ("type", "microsoft"),
            ("name", "Work"),
            ("application", " 11111111-2222-3333-4444-555555555555 "),
            ("tenant", ""),
        ],
        &[],
    );
    assert_eq!(
        used.requests,
        [AccountRequest::SaveSignIn {
            draft: SignInDraft {
                name: "Work".to_owned(),
                address: "robin@example.com".to_owned(),
                application_id: "11111111-2222-3333-4444-555555555555".to_owned(),
                tenant: None,
                google: None,
            }
        }]
    );
    assert!(!used.close);
    assert_eq!(form(&setup).busy(), Some("Saving Work…"));
    // Saved: the form is over and the sign-in is asked for, as ever.
    let (requests, outcome) = setup.on_event(AccountEvent::Saved(Ok("Work".to_owned())));
    assert_eq!(
        requests,
        [AccountRequest::SignIn {
            account: Some("Work".to_owned())
        }]
    );
    assert_eq!(outcome, Outcome::SigningIn("Work".to_owned()));
}

#[test]
fn a_google_account_in_the_sheet_asks_for_its_application_or_for_nothing() {
    // Offered with a password, the type changed in the sheet: the fields
    // are the application's, and what was said about passwords is gone.
    let mut setup = at_google(None, false);
    assert!(form(&setup).note().is_some());
    press(&mut setup, None, &[("type", "google")], &[]);
    let shown = form(&setup);
    assert_eq!(shown.title(), "Add an Account: Google");
    assert_eq!(shown.note(), None);
    assert_eq!(
        ids(&shown),
        ["type", "name", "address", "google_client", "google_secret"]
    );
    assert_eq!(field(&shown, "address").value(), Some("robin@gmail.com"));
    assert!(matches!(
        field(&shown, "google_secret"),
        FormField::Text { label, help: Some(help), .. }
            if label == "Client Secret:" && help == GOOGLE_NOTE
    ));
    // Continue with nothing typed: said, with the focus where it goes.
    press(&mut setup, Some(Press::Continue), &[("name", "Gmail")], &[]);
    let shown = form(&setup);
    assert!(shown.error().unwrap().contains("client ID"));
    assert_eq!(focus(&shown), Some("google_client"));

    // What the fields held is taken, and the account is saved and then
    // signed in to.
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[
            ("type", "google"),
            ("name", "Personal"),
            ("google_client", " 123-abc.apps.googleusercontent.com "),
            ("google_secret", "GOCSPX-not-a-real-one"),
        ],
        &[],
    );
    let [AccountRequest::SaveSignIn { draft }] = used.requests.as_slice() else {
        panic!("{:?}", used.requests);
    };
    assert_eq!(draft.name, "Personal");
    assert_eq!(draft.application_id, "123-abc.apps.googleusercontent.com");
    let google = draft.google.as_ref().unwrap();
    assert_eq!(
        google.client_secret.as_deref(),
        Some("GOCSPX-not-a-real-one")
    );
    assert_eq!(google.imap.host, "imap.gmail.com");
    assert_eq!(form(&setup).busy(), Some("Saving Personal…"));
    let (requests, outcome) = setup.on_event(AccountEvent::Saved(Ok("Personal".to_owned())));
    assert_eq!(
        requests,
        [AccountRequest::SignIn {
            account: Some("Personal".to_owned())
        }]
    );
    assert_eq!(outcome, Outcome::SigningIn("Personal".to_owned()));

    // Signing in as Quill itself: its name, its address, and what
    // happens next.
    let setup = at_google(Some(("quill.apps.googleusercontent.com", None)), true);
    let shown = form(&setup);
    assert_eq!(ids(&shown), ["type", "name", "address"]);
    assert!(matches!(
        field(&shown, "address"),
        FormField::Static { help: Some(help), .. } if help == GOOGLE_OWN_NOTE
    ));
    // And it is a message the bridge carries.
    let message = CoreToShell::FormShow(shown);
    let line = bridge::encode(&message).unwrap();
    assert_eq!(bridge::decode::<CoreToShell>(&line).unwrap(), message);
}

#[test]
fn changing_the_type_swaps_the_fields_at_once() {
    let mut setup = at_server();
    setup.error = Some("said of the other type".to_owned());
    // The pop-up changed: no button, and what the fields held comes too.
    let used = press(
        &mut setup,
        None,
        &[
            ("type", "microsoft"),
            ("name", "Work"),
            ("host", "mail.example.com"),
        ],
        &[],
    );
    assert_eq!(used, Used::default());
    assert_eq!((setup.step, setup.kind), (Step::Server, Kind::Microsoft));
    let shown = form(&setup);
    assert_eq!(
        ids(&shown),
        ["type", "name", "address", "application", "tenant"]
    );
    assert_eq!(shown.title(), "Add an Account: Microsoft");
    // What was typed is kept, and what was said of the old type is not.
    assert_eq!(field(&shown, "name").value(), Some("Work"));
    assert_eq!(shown.error(), None);

    // The cursor was in a field that has gone, and is not left there.
    let mut setup = at_server();
    setup.field = ServerField::Host.index();
    press(&mut setup, None, &[("type", "microsoft")], &[]);
    assert_eq!(focus(&form(&setup)), Some("type"));

    // And back: the servers are as they were left.
    press(
        &mut setup,
        None,
        &[("type", "imap"), ("application", "app-1")],
        &[],
    );
    assert_eq!(setup.kind, Kind::Imap);
    assert_eq!(
        field(&form(&setup), "host").value(),
        Some("imap.example.com")
    );
    // What was typed for the other kind is kept for it, too.
    assert_eq!(setup.application.text(), "app-1");

    // A type that is none of them changes nothing. The bridge lets a name
    // through; it is the form that knows which names are kinds.
    press(&mut setup, None, &[("type", "pop3")], &[]);
    assert_eq!(setup.kind, Kind::Imap);
}

#[test]
fn changing_the_security_takes_the_usual_port_with_it() {
    let mut setup = at_server();
    // The port the sheet shows is sent along with the new choice.
    press(
        &mut setup,
        None,
        &[("port", "993"), ("security", "starttls")],
        &[],
    );
    assert_eq!((setup.tls, setup.port.text()), (TlsMode::Starttls, "143"));
    assert_eq!(field(&form(&setup), "port").value(), Some("143"));
    // Its own for the sending server.
    press(
        &mut setup,
        None,
        &[("smtp_port", "465"), ("smtp_security", "starttls")],
        &[],
    );
    assert_eq!(
        (setup.smtp_tls, setup.smtp_port.text()),
        (TlsMode::Starttls, "587")
    );
    // A port the user chose is left alone, typed in the same breath or
    // before.
    press(
        &mut setup,
        None,
        &[("port", "1143"), ("security", "tls")],
        &[],
    );
    assert_eq!((setup.tls, setup.port.text()), (TlsMode::Implicit, "1143"));
    // The same answer again changes nothing: it is not a change.
    setup.error = Some("still wrong".to_owned());
    press(
        &mut setup,
        None,
        &[("port", "1143"), ("security", "tls")],
        &[],
    );
    assert_eq!(setup.error.as_deref(), Some("still wrong"));
}

#[test]
fn back_is_esc_and_cancel_puts_the_form_away() {
    // Back a step at a time, keeping what the fields held.
    let mut setup = at_password();
    let used = press(
        &mut setup,
        Some(Press::Back),
        &[],
        &[("password", "hunter3")],
    );
    assert_eq!(used, Used::default());
    assert_eq!(setup.step, Step::Server);
    assert_eq!(setup.password.text(), "hunter3");
    let used = press(&mut setup, Some(Press::Back), &[("name", "Work")], &[]);
    assert_eq!(used, Used::default());
    assert_eq!((setup.step, setup.name.text()), (Step::Address, "Work"));
    // From the first step there is nowhere back to go.
    assert!(press(&mut setup, Some(Press::Back), &[], &[]).close);

    // Cancel closes from any step, and takes nothing in on the way.
    for mut setup in [
        Setup::default(),
        at_server(),
        at_password(),
        at_certificate(),
    ] {
        let before = setup.clone();
        let used = press(
            &mut setup,
            Some(Press::Cancel),
            &[("name", "typed and thrown away")],
            &[("password", "thrown away too")],
        );
        assert_eq!(
            used,
            Used {
                requests: Vec::new(),
                close: true
            }
        );
        assert_eq!(setup, before);
    }
}

#[test]
fn while_quill_is_busy_only_cancel_does_anything() {
    let mut setup = Setup::default();
    setup.address.set("robin@example.com");
    setup.advance();
    assert!(setup.busy.is_some());
    let before = setup.clone();
    // The sheet's fields take no typing meanwhile; should an answer come
    // all the same, nothing in it is taken.
    for button in [Some(Press::Continue), None] {
        let used = press(
            &mut setup,
            button,
            &[("address", "someone@example.org")],
            &[],
        );
        assert_eq!(used, Used::default());
        assert_eq!(setup, before);
    }
    // Cancel closes, and the answer on its way finds nothing to change.
    assert!(press(&mut setup, Some(Press::Cancel), &[], &[]).close);
    // Back, were it pressed, is `esc`: the wait is given up, the form
    // stays, and nothing typed is taken.
    let used = press(
        &mut setup,
        Some(Press::Back),
        &[("address", "someone@example.org")],
        &[],
    );
    assert_eq!(used, Used::default());
    assert_eq!(setup.busy, None);
    assert_eq!(setup.address.text(), "robin@example.com");
    setup.on_event(found("robin@example.com"));
    assert_eq!(setup.step, Step::Address);
}

#[test]
fn the_certificate_buttons_are_the_three_keys() {
    // Reject: back to the server's details, nothing saved.
    let mut setup = at_certificate();
    assert_eq!(
        press(&mut setup, Some(Press::Reject), &[], &[]),
        Used::default()
    );
    assert_eq!(setup.step, Step::Server);
    assert!(form(&setup).error().unwrap().contains("not accepted"));

    // Trust once: sign in again holding the server to that certificate,
    // with the password the form has kept.
    let mut setup = at_certificate();
    let used = press(&mut setup, Some(Press::TrustOnce), &[], &[]);
    assert!(matches!(
        used.requests.as_slice(),
        [AccountRequest::Verify { trusting: Trusting::Once(fingerprint), password, .. }]
            if *fingerprint == "ab".repeat(32) && password.0 == "hunter2"
    ));
    assert_eq!(form(&setup).step(), "password");

    let mut setup = at_certificate();
    let used = press(&mut setup, Some(Press::TrustAlways), &[], &[]);
    assert!(matches!(
        used.requests.as_slice(),
        [AccountRequest::Verify {
            trusting: Trusting::Pinned(_),
            ..
        }]
    ));

    // Esc is reject there, and so is back.
    let mut setup = at_certificate();
    assert!(!press(&mut setup, Some(Press::Back), &[], &[]).close);
    assert_eq!(setup.step, Step::Server);

    // Return goes nowhere: a certificate is not accepted by default.
    let mut setup = at_certificate();
    assert_eq!(
        press(&mut setup, Some(Press::Continue), &[], &[]),
        Used::default()
    );
    assert_eq!(setup.step, Step::Certificate);

    // The certificate's buttons are no other step's.
    let mut setup = at_password();
    for button in [Press::Reject, Press::TrustOnce, Press::TrustAlways] {
        let before = setup.clone();
        assert_eq!(press(&mut setup, Some(button), &[], &[]), Used::default());
        assert_eq!(setup, before);
    }
}

#[test]
fn an_answer_about_another_step_or_an_unknown_button_is_dropped() {
    // The form has moved on since the sheet was pressed.
    let mut setup = at_password();
    let before = setup.clone();
    let stale = FormUsed::new("server", Some("continue"), &[("name", "Late")], &[]).unwrap();
    assert_eq!(setup.used(&stale), Used::default());
    assert_eq!(setup, before);
    // A button the form does not have.
    let unknown = FormUsed::new("password", Some("save"), &[], &[("password", "x")]).unwrap();
    assert_eq!(setup.used(&unknown), Used::default());
    assert_eq!(setup, before);
    // Fields that are not this step's are not taken.
    press(
        &mut setup,
        None,
        &[("name", "Not here"), ("host", "h")],
        &[],
    );
    assert_eq!(setup, before);
    // Nor fields of the other kind of account.
    let mut setup = at_microsoft(None);
    press(&mut setup, None, &[("host", "mail.example.org")], &[]);
    assert_eq!(setup.host.text(), "outlook.office365.com");
}

#[test]
fn every_step_and_kind_is_a_form_the_bridge_takes() {
    let mut busy = at_password();
    busy.advance();
    for setup in [
        Setup::default(),
        at_server(),
        at_microsoft(None),
        at_password(),
        busy,
        at_certificate(),
    ] {
        let shown = form(&setup);
        let message = CoreToShell::FormShow(shown);
        let line = bridge::encode(&message).unwrap();
        assert_eq!(bridge::decode::<CoreToShell>(&line).unwrap(), message);
    }
    // Every button there is has a name the bridge takes.
    for press in Press::ALL {
        assert!(bridge::valid_form_id(press.id()), "{press:?}");
        assert_eq!(Press::named(press.id()), Some(press));
    }
}

#[test]
fn going_back_for_another_address_keeps_the_microsoft_flow_whole() {
    // The same journey the terminal's keys make, by the sheet's buttons.
    let mut setup = at_microsoft(Some("app-1"));
    press(
        &mut setup,
        Some(Press::Back),
        &[("application", "app-2")],
        &[],
    );
    assert_eq!(setup.step, Step::Address);
    let used = press(
        &mut setup,
        Some(Press::Continue),
        &[("address", "robin@example.com")],
        &[],
    );
    assert!(matches!(
        used.requests.as_slice(),
        [AccountRequest::Discover { .. }]
    ));
    setup.on_event(found_at_microsoft("robin@example.com", Some("app-1")));
    // An id typed before going back is kept.
    assert_eq!(field(&form(&setup), "application").value(), Some("app-2"));
    // The terminal's keys still work on the same form.
    keys(&mut setup, "x");
    assert_eq!(setup.name.text(), "Examplex");
}

#[test]
fn a_title_has_a_macs_capitals_and_the_terminals_words() {
    for (title, expected) in [
        ("Add an account", "Add an Account"),
        ("Add an account: the server", "Add an Account: The Server"),
        ("Add an account: sign in", "Add an Account: Sign In"),
        ("Add an account: Microsoft", "Add an Account: Microsoft"),
        ("the end of the road", "The End of the Road"),
        ("", ""),
    ] {
        assert_eq!(headline(title), expected);
    }
    // The terminal's own titles are as they were.
    assert_eq!(Setup::default().title(), "Add an account");
}
