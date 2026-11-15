//! The SMTP sender against a scripted server in this process.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{
    Certificates, PASSWORD, Scram, Script, Server, Starttls, TOKEN, USER, envelope, message, send,
    with_password, with_token,
};
use quill_core::config::TlsMode;
use quill_core::{Envelope, Error, RefusedRecipient};
use quill_smtp::{Credential, Mechanism, SmtpAuth, SmtpConfig, SmtpSender, Trust};

const MODES: [TlsMode; 3] = [TlsMode::Implicit, TlsMode::Starttls, TlsMode::None];
const ENCRYPTED: [TlsMode; 2] = [TlsMode::Implicit, TlsMode::Starttls];

/// A message with everything the wire could get wrong: a folded `Bcc`, a
/// line that is one dot, lines that start with dots, and body text that
/// looks like a header.
const AWKWARD: &str = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
    To: Robin Vale <robin@example.com>\r\n\
    Cc: Kit Moss <kit@example.com>\r\n\
    Bcc: Sam Okoro <sam@example.com>,\r\n Lee Tran <lee@example.com>\r\n\
    Subject: Dots and all\r\n\
    Date: Sat, 14 Nov 2026 09:00:00 +0000\r\n\
    Message-ID: <dots-and-all@example.com>\r\n\
    MIME-Version: 1.0\r\n\
    Content-Type: text/plain; charset=us-ascii\r\n\
    \r\n\
    First line.\r\n\
    .\r\n\
    ..two dots\r\n\
    .leading dot\r\n\
    Bcc: this line is body text\r\n\
    \r\n\
    Last line.\r\n";

/// The same message as a server should receive it: no `Bcc` header, and
/// not one byte different otherwise.
const AWKWARD_AS_SENT: &str = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
    To: Robin Vale <robin@example.com>\r\n\
    Cc: Kit Moss <kit@example.com>\r\n\
    Subject: Dots and all\r\n\
    Date: Sat, 14 Nov 2026 09:00:00 +0000\r\n\
    Message-ID: <dots-and-all@example.com>\r\n\
    MIME-Version: 1.0\r\n\
    Content-Type: text/plain; charset=us-ascii\r\n\
    \r\n\
    First line.\r\n\
    .\r\n\
    ..two dots\r\n\
    .leading dot\r\n\
    Bcc: this line is body text\r\n\
    \r\n\
    Last line.\r\n";

const EVERYONE: [&str; 4] = [
    "robin@example.com",
    "kit@example.com",
    "sam@example.com",
    "lee@example.com",
];

fn hello() -> Vec<u8> {
    message("Hello", "Hello, Robin.")
}

fn to_robin() -> Envelope {
    envelope(&["robin@example.com"])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A sender whose credential provider counts how often it is asked.
fn counting(config: SmtpConfig) -> (SmtpSender, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&asked);
    let sender = SmtpSender::new(
        config,
        Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Credential::Password(PASSWORD.to_owned()))
        }),
    );
    (sender, asked)
}

#[tokio::test]
async fn every_tls_mode_delivers_the_message_byte_for_byte() {
    for mode in MODES {
        let server = Server::start(mode, Script::default()).await;
        send(&server.sender(), AWKWARD.as_bytes(), &envelope(&EVERYONE))
            .await
            .unwrap_or_else(|err| panic!("{mode:?}: {err:?}"));

        let log = server.log();
        assert_eq!(log.delivered.len(), 1, "{mode:?}");
        let delivered = &log.delivered[0];
        assert_eq!(delivered.from, USER);
        // Bcc recipients are in the envelope like everyone else.
        assert_eq!(delivered.to, EVERYONE);
        assert_eq!(
            String::from_utf8_lossy(&delivered.data),
            AWKWARD_AS_SENT,
            "{mode:?}"
        );
        // The size announced is the size of what was sent.
        assert_eq!(
            delivered.parameters,
            format!("SIZE={}", AWKWARD_AS_SENT.len())
        );

        // On the wire the dots were doubled, and the Bcc names were not
        // there in any form.
        let wire = String::from_utf8_lossy(&log.wire[0]).into_owned();
        assert!(
            wire.contains("First line.\r\n..\r\n...two dots\r\n..leading dot\r\n"),
            "{wire}"
        );
        assert!(!wire.contains("Sam Okoro") && !wire.contains("lee@example.com"));
        assert!(!wire.contains("\r\nBcc: Sam"));
        assert_eq!(wire.matches('\n').count(), wire.matches("\r\n").count());

        assert_eq!(log.signed_in, ["SCRAM-SHA-256"], "{mode:?}");
        assert!(log.saw("QUIT"));
    }
}

#[tokio::test]
async fn starttls_sends_only_the_hello_and_the_upgrade_request_in_the_clear() {
    let server = Server::start(TlsMode::Starttls, Script::default()).await;
    send(&server.sender(), &hello(), &to_robin()).await.unwrap();
    let log = server.log();
    assert_eq!(log.cleartext, ["EHLO [127.0.0.1]", "STARTTLS"]);
    // The hello was said again once the channel was encrypted.
    let hellos = log
        .commands
        .iter()
        .filter(|line| line.starts_with("EHLO"))
        .count();
    assert_eq!(hellos, 2, "{:?}", log.commands);
    assert_eq!(log.delivered.len(), 1);
}

#[tokio::test]
async fn implicit_tls_sends_nothing_in_the_clear() {
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    send(&server.sender(), &hello(), &to_robin()).await.unwrap();
    assert!(server.log().cleartext.is_empty());
    assert_eq!(server.log().delivered.len(), 1);
}

#[tokio::test]
async fn a_server_without_starttls_is_insecure_and_sees_no_credentials_or_mail() {
    for starttls in [Starttls::Missing, Starttls::Unavailable] {
        // The server is glad to take a password in the clear. It must not
        // be given one.
        let script = Script {
            starttls,
            before_tls: vec!["AUTH PLAIN LOGIN", "8BITMIME"],
            ..Script::default()
        };
        let server = Server::start(TlsMode::Starttls, script).await;
        let (sender, asked) = counting(server.config());
        for _ in 0..2 {
            match send(&sender, &hello(), &to_robin()).await {
                Err(Error::Insecure(reason)) => {
                    assert!(reason.contains("TLS"), "{reason}");
                    assert!(reason.contains("localhost"), "{reason}");
                }
                other => panic!("{starttls:?}: {other:?}"),
            }
        }
        // There is no fallback to plaintext: no sign-in, no mail, and the
        // password was not even fetched.
        let log = server.log();
        assert!(!log.saw("AUTH") && !log.saw("MAIL") && !log.saw("RCPT") && !log.saw("DATA"));
        assert_eq!(
            log.cleartext,
            [
                "EHLO [127.0.0.1]",
                "STARTTLS",
                "QUIT",
                "EHLO [127.0.0.1]",
                "STARTTLS",
                "QUIT"
            ],
            "{starttls:?}"
        );
        assert!(log.delivered.is_empty() && log.wire.is_empty());
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn what_a_server_said_before_the_upgrade_is_forgotten_after_it() {
    // In the clear: PLAIN sign-in, a ten-byte limit, 8BITMIME and SMTPUTF8.
    // Over TLS: LOGIN only, and none of the rest.
    let script = Script {
        before_tls: vec!["AUTH PLAIN", "SIZE 10", "8BITMIME", "SMTPUTF8"],
        extensions: vec!["AUTH LOGIN"],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Starttls, script).await;
    send(&server.sender(), &hello(), &to_robin()).await.unwrap();
    let log = server.log();
    // Not PLAIN, and not stopped by the ten-byte limit.
    assert_eq!(log.signed_in, ["LOGIN"]);
    assert_eq!(log.delivered.len(), 1);
    assert_eq!(log.delivered[0].parameters, "");

    // 8BITMIME and SMTPUTF8 were only offered in the clear, so they are not
    // offered at all.
    let eight_bit = message("Coffee", "Caf\u{e9} at ten?");
    assert!(matches!(
        send(&server.sender(), &eight_bit, &to_robin()).await,
        Err(Error::Refused(text)) if text.contains("8BITMIME")
    ));
    assert!(matches!(
        send(&server.sender(), &hello(), &envelope(&["zo\u{eb}@example.com"])).await,
        Err(Error::Refused(text)) if text.contains("SMTPUTF8")
    ));
    assert_eq!(server.log().delivered.len(), 1);
}

#[tokio::test]
async fn input_sent_in_the_clear_behind_the_go_ahead_is_discarded() {
    // Right behind "go ahead" comes a forged reply claiming a ten-byte
    // limit and PLAIN sign-in. A client that kept its read buffer across
    // the upgrade would take it for the answer to its next EHLO.
    let script = Script {
        starttls: Starttls::Injecting,
        extensions: vec!["AUTH LOGIN", "SIZE 10240000"],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Starttls, script).await;
    send(&server.sender(), &hello(), &to_robin()).await.unwrap();
    let log = server.log();
    assert_eq!(log.signed_in, ["LOGIN"]);
    assert_eq!(log.delivered.len(), 1);
    assert_eq!(
        log.delivered[0].parameters,
        format!("SIZE={}", hello().len())
    );
    assert_eq!(log.cleartext, ["EHLO [127.0.0.1]", "STARTTLS"]);
}

#[tokio::test]
async fn an_untrusted_certificate_is_refused_and_reported_with_its_fingerprint() {
    for mode in ENCRYPTED {
        let server = Server::start(mode, Script::default()).await;
        // No extra roots: the test CA is unknown to the system.
        let mut config = server.config();
        config.trust.extra_roots.clear();
        match send(&with_password(config, PASSWORD), &hello(), &to_robin()).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(problem.host, "localhost");
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(server.certificates.leaf_der.as_slice())
                );
                assert_eq!(
                    problem.fingerprint,
                    Some(hex(&server.certificates.fingerprint()))
                );
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        let log = server.log();
        assert!(!log.saw("AUTH") && !log.saw("MAIL"), "{mode:?}");
    }
}

#[tokio::test]
async fn an_expired_certificate_is_refused() {
    // Valid for thirty days, starting sixty days ago.
    let expired = Certificates::issue(&["localhost"], 60, 30);
    for mode in ENCRYPTED {
        let server = Server::start_with(mode, Script::default(), expired.clone()).await;
        // The CA is trusted; the dates are the only thing wrong.
        match send(&server.sender(), &hello(), &to_robin()).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(expired.leaf_der.as_slice())
                );
                assert_eq!(problem.fingerprint, Some(hex(&expired.fingerprint())));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        assert!(!server.log().saw("AUTH") && server.log().delivered.is_empty());
    }
}

#[tokio::test]
async fn a_certificate_naming_another_host_is_refused() {
    let other = Certificates::issue(&["mail.example.com"], 1, 90);
    for mode in ENCRYPTED {
        let server = Server::start_with(mode, Script::default(), other.clone()).await;
        match send(&server.sender(), &hello(), &to_robin()).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(problem.host, "localhost");
                assert_eq!(problem.fingerprint, Some(hex(&other.fingerprint())));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        assert!(!server.log().saw("AUTH") && server.log().delivered.is_empty());
    }

    // And the other way about: the right certificate, reached by a name it
    // does not carry.
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    let mut config = server.config();
    config.host = "127.0.0.1".to_owned();
    assert!(matches!(
        send(&with_password(config, PASSWORD), &hello(), &to_robin()).await,
        Err(Error::Certificate(_))
    ));
}

#[tokio::test]
async fn a_self_signed_certificate_is_refused_until_it_is_pinned() {
    let own = Certificates::self_signed();
    for mode in ENCRYPTED {
        let server = Server::start_with(mode, Script::default(), own.clone()).await;
        let mut config = server.config();
        assert_eq!(config.trust, Trust::default());
        match send(
            &with_password(config.clone(), PASSWORD),
            &hello(),
            &to_robin(),
        )
        .await
        {
            Err(Error::Certificate(problem)) => {
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(own.leaf_der.as_slice())
                );
                assert_eq!(problem.fingerprint, Some(hex(&own.fingerprint())));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        assert!(server.log().delivered.is_empty());

        // Pinned by the fingerprint that error reported, it is accepted.
        config.trust.pinned_sha256 = Some(own.fingerprint());
        send(
            &with_password(config.clone(), PASSWORD),
            &hello(),
            &to_robin(),
        )
        .await
        .unwrap_or_else(|err| panic!("{mode:?}: {err:?}"));
        assert_eq!(server.log().delivered.len(), 1);

        // A pin for some other certificate is not.
        config.trust.pinned_sha256 = Some([7; 32]);
        match send(&with_password(config, PASSWORD), &hello(), &to_robin()).await {
            Err(Error::Certificate(problem)) => {
                assert!(problem.reason.contains("pinned"), "{}", problem.reason);
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        assert_eq!(server.log().delivered.len(), 1);
    }
}

#[tokio::test]
async fn a_pin_wins_over_a_trusted_authority() {
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    // The CA is trusted and the certificate is fine, but it is not the one
    // this account pinned.
    let mut config = server.config();
    config.trust.pinned_sha256 = Some([7; 32]);
    assert!(matches!(
        send(&with_password(config, PASSWORD), &hello(), &to_robin()).await,
        Err(Error::Certificate(_))
    ));
}

#[tokio::test]
async fn each_password_mechanism_signs_in() {
    for (mechanism, name) in [
        (Mechanism::Plain, "PLAIN"),
        (Mechanism::Login, "LOGIN"),
        (Mechanism::ScramSha256, "SCRAM-SHA-256"),
    ] {
        assert_eq!(mechanism.name(), name);
        let server = Server::start(TlsMode::Implicit, Script::default()).await;
        let mut config = server.config();
        config.auth = SmtpAuth::Mechanism(mechanism);
        send(&with_password(config, PASSWORD), &hello(), &to_robin())
            .await
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let log = server.log();
        assert_eq!(log.signed_in, [name]);
        assert_eq!(log.delivered.len(), 1, "{name}");
    }
}

#[tokio::test]
async fn each_token_mechanism_signs_in() {
    for (mechanism, name) in [
        (Mechanism::Xoauth2, "XOAUTH2"),
        (Mechanism::OAuthBearer, "OAUTHBEARER"),
    ] {
        assert_eq!(mechanism.name(), name);
        let server = Server::start(TlsMode::Implicit, Script::default()).await;
        let mut config = server.config();
        config.auth = SmtpAuth::Mechanism(mechanism);
        send(&with_token(config, TOKEN), &hello(), &to_robin())
            .await
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let log = server.log();
        assert_eq!(log.signed_in, [name]);
        assert_eq!(log.delivered.len(), 1, "{name}");
    }
}

#[tokio::test]
async fn the_strongest_mechanism_on_offer_is_chosen() {
    for (offer, password, token) in [
        (
            "AUTH LOGIN PLAIN XOAUTH2 OAUTHBEARER SCRAM-SHA-256",
            "SCRAM-SHA-256",
            "OAUTHBEARER",
        ),
        ("AUTH LOGIN XOAUTH2 PLAIN", "PLAIN", "XOAUTH2"),
        ("AUTH XOAUTH2 LOGIN", "LOGIN", "XOAUTH2"),
    ] {
        let script = Script {
            extensions: vec![offer],
            ..Script::default()
        };
        let server = Server::start(TlsMode::Implicit, script).await;
        send(&server.sender(), &hello(), &to_robin()).await.unwrap();
        send(&with_token(server.config(), TOKEN), &hello(), &to_robin())
            .await
            .unwrap();
        assert_eq!(server.log().signed_in, [password, token], "{offer}");
    }
}

#[tokio::test]
async fn an_override_is_used_whatever_else_is_on_offer() {
    let server = Server::start(TlsMode::Starttls, Script::default()).await;
    let mut config = server.config();
    config.auth = SmtpAuth::Mechanism(Mechanism::Login);
    send(&with_password(config, PASSWORD), &hello(), &to_robin())
        .await
        .unwrap();
    assert_eq!(server.log().signed_in, ["LOGIN"]);
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error_with_every_mechanism() {
    for mechanism in [Mechanism::Plain, Mechanism::Login, Mechanism::ScramSha256] {
        let server = Server::start(TlsMode::Implicit, Script::default()).await;
        let mut config = server.config();
        config.auth = SmtpAuth::Mechanism(mechanism);
        match send(
            &with_password(config, "not-the-password"),
            &hello(),
            &to_robin(),
        )
        .await
        {
            Err(Error::Auth(text)) => {
                // The server's own words.
                assert_eq!(text, "535 5.7.8 Authentication credentials invalid");
                assert!(!text.contains("not-the-password"));
            }
            other => panic!("{mechanism:?}: {other:?}"),
        }
        let log = server.log();
        assert!(
            log.signed_in.is_empty() && !log.saw("MAIL"),
            "{mechanism:?}"
        );
        assert!(log.saw("QUIT"));
    }
}

#[tokio::test]
async fn a_wrong_token_is_an_auth_error_after_hearing_the_server_out() {
    // A refused token comes in two steps: the server's reasons as a
    // challenge, which each mechanism answers in its own way, then the no.
    for (mechanism, answer) in [(Mechanism::Xoauth2, ""), (Mechanism::OAuthBearer, "AQ==")] {
        let server = Server::start(TlsMode::Implicit, Script::default()).await;
        let mut config = server.config();
        config.auth = SmtpAuth::Mechanism(mechanism);
        match send(&with_token(config, "expired-token"), &hello(), &to_robin()).await {
            Err(Error::Auth(text)) => {
                assert_eq!(text, "535 5.7.8 Authentication credentials invalid");
            }
            other => panic!("{mechanism:?}: {other:?}"),
        }
        let log = server.log();
        assert_eq!(log.sign_in_lines, [answer], "{mechanism:?}");
        assert!(log.signed_in.is_empty() && !log.saw("MAIL"));
    }
}

#[tokio::test]
async fn a_server_that_cannot_prove_it_knows_the_password_is_refused() {
    for scram in [Scram::Forging, Scram::NotSigning] {
        let script = Script {
            scram,
            ..Script::default()
        };
        let server = Server::start(TlsMode::Implicit, script).await;
        // The password is right. It is the server that fails the check.
        match send(&server.sender(), &hello(), &to_robin()).await {
            Err(Error::Auth(text)) => assert!(text.contains("prove"), "{text}"),
            other => panic!("{scram:?}: {other:?}"),
        }
        let log = server.log();
        assert!(!log.saw("MAIL") && log.delivered.is_empty(), "{scram:?}");
        if scram == Scram::Forging {
            // The sign-in was called off, not completed.
            assert_eq!(log.sign_in_lines.last().map(String::as_str), Some("*"));
            assert!(log.signed_in.is_empty());
        }
    }
}

#[tokio::test]
async fn a_server_with_no_usable_mechanism_is_an_auth_error() {
    for (offer, mentions) in [
        (vec!["AUTH CRAM-MD5 GSSAPI"], "CRAM-MD5, GSSAPI"),
        (vec!["AUTH XOAUTH2 OAUTHBEARER"], "XOAUTH2"),
        (vec!["8BITMIME"], "no way to sign in"),
    ] {
        let script = Script {
            extensions: offer,
            ..Script::default()
        };
        let server = Server::start(TlsMode::Implicit, script).await;
        match send(&server.sender(), &hello(), &to_robin()).await {
            Err(Error::Auth(text)) => assert!(text.contains(mentions), "{text}"),
            other => panic!("{mentions}: {other:?}"),
        }
        // Mail is never sent unsigned because signing in was not possible.
        let log = server.log();
        assert!(!log.saw("AUTH") && !log.saw("MAIL"), "{mentions}");
    }
}

#[tokio::test]
async fn a_mechanism_the_credential_cannot_use_is_an_auth_error() {
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    let mut config = server.config();
    config.auth = SmtpAuth::Mechanism(Mechanism::Plain);
    match send(&with_token(config.clone(), TOKEN), &hello(), &to_robin()).await {
        Err(Error::Auth(text)) => assert!(text.contains("PLAIN needs a password"), "{text}"),
        other => panic!("{other:?}"),
    }
    config.auth = SmtpAuth::Mechanism(Mechanism::Xoauth2);
    match send(&with_password(config, PASSWORD), &hello(), &to_robin()).await {
        Err(Error::Auth(text)) => assert!(text.contains("XOAUTH2 needs an OAuth token"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(!server.log().saw("AUTH") && !server.log().saw("MAIL"));
}

#[tokio::test]
async fn a_relay_that_wants_no_sign_in_gets_none() {
    let script = Script {
        require_auth: false,
        extensions: vec!["8BITMIME"],
        ..Script::default()
    };
    let server = Server::start(TlsMode::None, script).await;
    let mut config = server.config();
    config.auth = SmtpAuth::None;
    let (sender, asked) = counting(config);
    send(&sender, &hello(), &to_robin()).await.unwrap();
    let log = server.log();
    assert_eq!(log.delivered.len(), 1);
    assert!(!log.saw("AUTH"));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_credential_is_fetched_for_each_send_and_its_failure_passed_on() {
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    let (sender, asked) = counting(server.config());
    send(&sender, &hello(), &to_robin()).await.unwrap();
    send(&sender, &message("Again", "Once more."), &to_robin())
        .await
        .unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert_eq!(server.log().delivered.len(), 2);

    let locked = SmtpSender::new(
        server.config(),
        Arc::new(|| Err(Error::Auth("the Keychain is locked".to_owned()))),
    );
    assert!(matches!(
        send(&locked, &hello(), &to_robin()).await,
        Err(Error::Auth(text)) if text == "the Keychain is locked"
    ));
    assert_eq!(server.log().delivered.len(), 2);
}

#[tokio::test]
async fn a_message_over_the_size_limit_is_refused_before_anything_is_sent() {
    // The limit applies to what is transmitted: the message without Bcc.
    let size = AWKWARD_AS_SENT.len();
    let limit: &'static str = Box::leak(format!("SIZE {}", size - 1).into_boxed_str());
    let script = Script {
        extensions: vec!["AUTH PLAIN", limit],
        ..Script::default()
    };
    for mode in MODES {
        let server = Server::start(mode, script.clone()).await;
        match send(&server.sender(), AWKWARD.as_bytes(), &envelope(&EVERYONE)).await {
            Err(Error::TooLarge { size: was, limit }) => {
                assert_eq!((was, limit), (size as u64, size as u64 - 1));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
        let log = server.log();
        assert!(!log.saw("MAIL") && !log.saw("DATA") && log.wire.is_empty());
        assert!(log.saw("QUIT"));
    }

    // One byte more room and it goes.
    let limit: &'static str = Box::leak(format!("SIZE {size}").into_boxed_str());
    let script = Script {
        extensions: vec!["AUTH PLAIN", limit],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    send(&server.sender(), AWKWARD.as_bytes(), &envelope(&EVERYONE))
        .await
        .unwrap();
    assert_eq!(server.log().delivered[0].data.len(), size);
}

#[tokio::test]
async fn one_refused_recipient_means_nothing_is_sent_to_anyone() {
    let script = Script {
        refuse: vec![(
            "kit@example.com",
            "550 5.1.1 <kit@example.com>: Recipient address rejected: User unknown",
        )],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    let everyone = envelope(&["robin@example.com", "kit@example.com", "sam@example.com"]);
    match send(&server.sender(), &hello(), &everyone).await {
        Err(Error::Recipients(refused)) => assert_eq!(
            refused,
            [RefusedRecipient {
                address: "kit@example.com".to_owned(),
                reason: "550 5.1.1 <kit@example.com>: Recipient address rejected: User unknown"
                    .to_owned(),
            }]
        ),
        other => panic!("{other:?}"),
    }
    let log = server.log();
    // Every recipient was asked about, the message was called off, and no
    // part of it was sent.
    let asked = log
        .commands
        .iter()
        .filter(|line| line.starts_with("RCPT"))
        .count();
    assert_eq!(asked, 3, "{:?}", log.commands);
    assert!(!log.saw("DATA") && log.wire.is_empty() && log.delivered.is_empty());
    assert!(log.saw("RSET") && log.saw("QUIT"));
}

#[tokio::test]
async fn every_refused_recipient_is_reported_with_the_servers_reason() {
    let script = Script {
        refuse: vec![
            ("kit@example.com", "550 5.1.1 User unknown"),
            ("sam@example.com", "452 4.2.2 Mailbox full"),
        ],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Starttls, script).await;
    let everyone = envelope(&["kit@example.com", "robin@example.com", "sam@example.com"]);
    match send(&server.sender(), &hello(), &everyone).await {
        Err(Error::Recipients(refused)) => {
            let refused: Vec<(&str, &str)> = refused
                .iter()
                .map(|one| (one.address.as_str(), one.reason.as_str()))
                .collect();
            assert_eq!(
                refused,
                [
                    ("kit@example.com", "550 5.1.1 User unknown"),
                    ("sam@example.com", "452 4.2.2 Mailbox full")
                ]
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!server.log().saw("DATA"));
}

#[tokio::test]
async fn a_4xx_after_the_message_is_temporary_and_a_5xx_is_final() {
    let script = Script {
        accepted_reply: Some("451 4.3.0 Mail server temporarily rejected message"),
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    match send(&server.sender(), &hello(), &to_robin()).await {
        Err(Error::Temporary(text)) => {
            assert_eq!(text, "451 4.3.0 Mail server temporarily rejected message");
        }
        other => panic!("{other:?}"),
    }
    assert!(server.log().delivered.is_empty());

    let script = Script {
        accepted_reply: Some("554 5.7.1 Message rejected as spam"),
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    match send(&server.sender(), &hello(), &to_robin()).await {
        Err(Error::Refused(text)) => assert_eq!(text, "554 5.7.1 Message rejected as spam"),
        other => panic!("{other:?}"),
    }
    assert!(server.log().delivered.is_empty());
}

#[tokio::test]
async fn a_refusal_at_any_earlier_step_is_sorted_the_same_way() {
    let script = Script {
        mail_reply: "450 4.7.1 Too many messages, slow down",
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    assert!(matches!(
        send(&server.sender(), &hello(), &to_robin()).await,
        Err(Error::Temporary(text)) if text == "450 4.7.1 Too many messages, slow down"
    ));
    assert!(!server.log().saw("RCPT") && !server.log().saw("DATA"));

    let script = Script {
        mail_reply: "550 5.7.1 Sender address rejected: not owned by user",
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    assert!(matches!(
        send(&server.sender(), &hello(), &to_robin()).await,
        Err(Error::Refused(text)) if text.starts_with("550 5.7.1 Sender address rejected")
    ));

    let script = Script {
        data_reply: "554 5.5.1 Not today",
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    assert!(matches!(
        send(&server.sender(), &hello(), &to_robin()).await,
        Err(Error::Refused(text)) if text == "554 5.5.1 Not today"
    ));
    // The message itself was never put on the wire.
    assert!(server.log().wire.is_empty());
}

#[tokio::test]
async fn an_international_address_needs_smtputf8() {
    let zoe = envelope(&["robin@example.com", "zo\u{eb}@example.com"]);

    let script = Script {
        extensions: vec!["AUTH PLAIN", "8BITMIME"],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    match send(&server.sender(), &hello(), &zoe).await {
        Err(Error::Refused(text)) => {
            assert!(text.contains("zo\u{eb}@example.com"), "{text}");
            assert!(text.contains("SMTPUTF8"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    let log = server.log();
    assert!(!log.saw("MAIL") && !log.saw("RCPT") && !log.saw("AUTH"));

    // Offered, it is asked for and the address goes as written.
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    send(&server.sender(), &hello(), &zoe).await.unwrap();
    let delivered = &server.log().delivered[0];
    assert_eq!(delivered.to, ["robin@example.com", "zo\u{eb}@example.com"]);
    assert!(
        delivered.parameters.ends_with(" SMTPUTF8"),
        "{}",
        delivered.parameters
    );
}

#[tokio::test]
async fn eight_bit_content_needs_8bitmime() {
    let coffee = message("Coffee", "Caf\u{e9} at ten? \u{2615}");

    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    send(&server.sender(), &coffee, &to_robin()).await.unwrap();
    let delivered = &server.log().delivered[0];
    assert_eq!(delivered.data, coffee);
    assert_eq!(
        delivered.parameters,
        format!("SIZE={} BODY=8BITMIME", coffee.len())
    );

    // Not offered: refused here, with nothing sent. The message is not
    // rewritten as 7-bit.
    let script = Script {
        extensions: vec!["AUTH PLAIN", "SMTPUTF8"],
        ..Script::default()
    };
    let server = Server::start(TlsMode::Implicit, script).await;
    match send(&server.sender(), &coffee, &to_robin()).await {
        Err(Error::Refused(text)) => assert!(text.contains("8BITMIME"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(!server.log().saw("MAIL"));
    // Plain ASCII still goes to the same server, with no BODY parameter.
    send(&server.sender(), &hello(), &to_robin()).await.unwrap();
    assert_eq!(server.log().delivered[0].parameters, "");
}

#[tokio::test]
async fn bare_line_endings_cannot_smuggle_a_second_message() {
    // Unix line endings, and a body that tries to end the message early
    // and start another.
    let raw = "From: ana.ruiz@example.com\nBcc: sam@example.com\nSubject: One\n\n\
               before\n.\nMAIL FROM:<mallory@example.com>\nRCPT TO:<victim@example.com>\n\
               DATA\nSubject: Two\n\nsmuggled\r.\rafter";
    let server = Server::start(TlsMode::None, Script::default()).await;
    send(&server.sender(), raw.as_bytes(), &to_robin())
        .await
        .unwrap();
    let log = server.log();
    assert_eq!(log.delivered.len(), 1);
    assert_eq!(
        String::from_utf8_lossy(&log.delivered[0].data),
        "From: ana.ruiz@example.com\r\nSubject: One\r\n\r\n\
         before\r\n.\r\nMAIL FROM:<mallory@example.com>\r\nRCPT TO:<victim@example.com>\r\n\
         DATA\r\nSubject: Two\r\n\r\nsmuggled\r\n.\r\nafter\r\n"
    );
    let mails = log
        .commands
        .iter()
        .filter(|line| line.starts_with("MAIL"))
        .count();
    assert_eq!(mails, 1, "{:?}", log.commands);
}

#[tokio::test]
async fn an_address_that_carries_a_command_never_reaches_the_server() {
    let server = Server::start(TlsMode::None, Script::default()).await;
    let bad = "robin@example.com>\r\nRCPT TO:<victim@example.com";
    match send(
        &server.sender(),
        &hello(),
        &envelope(&["kit@example.com", bad]),
    )
    .await
    {
        Err(Error::Recipients(refused)) => {
            assert_eq!(refused.len(), 1);
            assert_eq!(refused[0].address, bad);
        }
        other => panic!("{other:?}"),
    }
    let mut from_bad = to_robin();
    from_bad.from = "ana.ruiz@example.com> SIZE=1\r\nRSET".to_owned();
    assert!(matches!(
        send(&server.sender(), &hello(), &from_bad).await,
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        send(&server.sender(), &hello(), &envelope(&[])).await,
        Err(Error::Refused(text)) if text.contains("no recipients")
    ));
    // Nothing was even connected.
    assert!(server.log().commands.is_empty());
}

#[tokio::test]
async fn a_large_message_arrives_whole() {
    // Larger than one write, with dots in awkward places all through.
    let mut body = String::new();
    for n in 0..6000 {
        body.push_str(&format!(
            ".{n} a line that starts with a dot\r\nand one that does not {n}\r\n"
        ));
    }
    let raw = message("Large", &body);
    assert!(raw.len() > 300_000);
    let server = Server::start(TlsMode::Starttls, Script::default()).await;
    send(&server.sender(), &raw, &to_robin()).await.unwrap();
    assert!(server.log().delivered[0].data == raw);
}

#[tokio::test]
async fn a_server_that_never_greets_is_a_connection_error() {
    let script = Script {
        greets: false,
        ..Script::default()
    };
    let server = Server::start(TlsMode::None, script).await;
    let sender = server.sender().with_patience(Duration::from_millis(200));
    let started = Instant::now();
    match send(&sender, &hello(), &to_robin()).await {
        Err(Error::Connection(text)) => assert!(text.contains("greeting"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn a_server_that_goes_quiet_after_the_message_is_waited_for_longer_then_given_up_on() {
    let script = Script {
        accepted_reply: None,
        ..Script::default()
    };
    let server = Server::start(TlsMode::None, script).await;
    let patience = Duration::from_millis(50);
    let sender = server.sender().with_patience(patience);
    let started = Instant::now();
    match send(&sender, &hello(), &to_robin()).await {
        Err(Error::Connection(text)) => assert!(text.contains("accept the message"), "{text}"),
        other => panic!("{other:?}"),
    }
    // The server may have taken the message, so this wait is the long one:
    // thirty times the usual.
    assert!(
        started.elapsed() >= patience * 30,
        "{:?}",
        started.elapsed()
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(server.log().wire.len(), 1);
}

#[tokio::test]
async fn an_unreachable_server_is_a_connection_error() {
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = Server::start(TlsMode::None, Script::default()).await;
    let mut config = server.config();
    config.port = port;
    assert!(matches!(
        send(&with_password(config, PASSWORD), &hello(), &to_robin()).await,
        Err(Error::Connection(_))
    ));
}

#[tokio::test]
async fn a_server_that_hangs_up_is_a_connection_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            drop(client);
        }
    });
    let server = Server::start(TlsMode::None, Script::default()).await;
    let mut config = server.config();
    config.port = port;
    assert!(matches!(
        send(&with_password(config, PASSWORD), &hello(), &to_robin()).await,
        Err(Error::Connection(_))
    ));
}

#[tokio::test]
async fn something_that_is_not_an_smtp_server_is_an_error_not_a_hang() {
    // Plaintext spoken to a port that is TLS from the first byte: each side
    // waits for the other to begin.
    let server = Server::start(TlsMode::Implicit, Script::default()).await;
    let mut config = server.config();
    config.tls = TlsMode::None;
    let sender = with_password(config, PASSWORD).with_patience(Duration::from_millis(300));
    assert!(send(&sender, &hello(), &to_robin()).await.is_err());
    assert!(server.log().delivered.is_empty());
}
