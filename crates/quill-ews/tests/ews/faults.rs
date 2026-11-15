//! Signing in, a busy server, refusals, answers that are not what they
//! should be, and a server that cannot be reached.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::types::{Flags, FolderId, Operation, RemoteId};
use quill_core::{Backend, Credential, Credentials, Error, Result};
use quill_ews::{EwsBackend, EwsConfig};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{
    self, ENDPOINT, Rig, Secrets, USER, answer, busy, failure, fault, mime, received, soap,
};

const RAW: &[u8] = b"Subject: Minutes\r\n\r\nHello\r\n";

fn m1() -> RemoteId {
    RemoteId("m1".to_owned())
}

fn inbox() -> FolderId {
    FolderId("folder-inbox".to_owned())
}

/// The message asked for, as Exchange hands it over.
fn body() -> ResponseTemplate {
    answer("GetItem", &[mime("m1", RAW)])
}

/// A refused sign-in, as IIS in front of Exchange words one when Basic
/// authentication is switched on.
fn unauthorized() -> ResponseTemplate {
    ResponseTemplate::new(401)
        .append_header("WWW-Authenticate", "Negotiate")
        .append_header("WWW-Authenticate", "NTLM")
        .append_header("WWW-Authenticate", "Basic realm=\"mail.example.com\"")
        .set_body_raw(
            "<html>401 - Unauthorized: Access is denied due to invalid credentials.</html>",
            "text/html",
        )
}

fn config(server: &MockServer) -> EwsConfig {
    EwsConfig::new(format!("{}{ENDPOINT}", server.uri()), USER)
}

#[tokio::test]
async fn every_request_carries_the_password_the_source_has_just_then() {
    let rig = Rig::start().await;
    rig.answers("GetItem", body()).await;

    rig.ews.fetch_raw(&m1()).await.unwrap();
    // The password is changed between two requests.
    rig.secrets.refused().await;
    rig.ews.fetch_raw(&m1()).await.unwrap();

    let requests = rig.requests().await;
    // Basic authentication, written out: the name, a colon, the password.
    assert_eq!(
        common::one_header(&requests[0], "authorization"),
        format!(
            "Basic {}",
            BASE64.encode("robin@example.com:test-password-1")
        )
    );
    assert_eq!(
        common::one_header(&requests[1], "authorization"),
        format!(
            "Basic {}",
            BASE64.encode("robin@example.com:test-password-2")
        )
    );
    rig.check().await;
}

#[tokio::test]
async fn an_account_with_a_token_signs_in_with_it_and_sends_no_user_name() {
    let rig = Rig::with_tokens().await;
    rig.answers("GetItem", body()).await;

    rig.ews.fetch_raw(&m1()).await.unwrap();

    let requests = rig.requests().await;
    assert_eq!(
        common::one_header(&requests[0], "authorization"),
        "Bearer test-access-token-1"
    );
    rig.check().await;
}

#[tokio::test]
async fn a_refused_secret_is_reported_and_the_request_sent_again_with_the_next() {
    for rig in [Rig::start().await, Rig::with_tokens().await] {
        rig.on("GetItem")
            .and(header(
                "authorization",
                rig.secrets.authorization(1).as_str(),
            ))
            .respond_with(unauthorized().append_header("WWW-Authenticate", "Bearer"))
            .mount(&rig.server)
            .await;
        rig.on("GetItem")
            .and(header(
                "authorization",
                rig.secrets.authorization(2).as_str(),
            ))
            .respond_with(body())
            .mount(&rig.server)
            .await;

        let raw = rig.ews.fetch_raw(&m1()).await.unwrap();
        assert_eq!(&raw[..], RAW);
        assert_eq!(rig.secrets.refused.load(Ordering::SeqCst), 1);
        assert_eq!(rig.check().await.len(), 2);
    }
}

#[tokio::test]
async fn a_second_refusal_is_a_failed_sign_in_and_the_secret_is_not_in_the_error() {
    for rig in [Rig::start().await, Rig::with_tokens().await] {
        rig.answers(
            "GetItem",
            unauthorized().append_header("WWW-Authenticate", "Bearer error=\"invalid_token\""),
        )
        .await;

        let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
        assert!(matches!(err, Error::Auth(_)), "{err:?}");
        // Asked for a new one once, and that was all.
        assert_eq!(rig.secrets.refused.load(Ordering::SeqCst), 1);
        assert_eq!(rig.check().await.len(), 2);
        for shown in [err.to_string(), format!("{err:?}")] {
            assert!(!shown.contains("test-password"), "{shown}");
            assert!(!shown.contains("test-access-token"), "{shown}");
            assert!(!shown.contains("Basic "), "{shown}");
            assert!(!shown.contains("Bearer"), "{shown}");
            assert!(
                shown.contains("would not accept the account's sign-in"),
                "{shown}"
            );
        }
    }
}

#[tokio::test]
async fn a_wrong_password_is_tried_once_and_not_a_second_time() {
    /// The same password however often it is refused, as the Keychain
    /// gives it.
    #[derive(Default)]
    struct Saved {
        handed_out: AtomicUsize,
        refused: AtomicUsize,
    }
    #[async_trait]
    impl Credentials for Saved {
        async fn get(&self) -> Result<Credential> {
            self.handed_out.fetch_add(1, Ordering::SeqCst);
            Ok(Credential::Password("test-password-1".to_owned()))
        }
        async fn refused(&self) {
            self.refused.fetch_add(1, Ordering::SeqCst);
        }
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(unauthorized())
        .mount(&server)
        .await;
    let saved = Arc::new(Saved::default());
    let ews = EwsBackend::loopback(config(&server), saved.clone()).unwrap();

    let err = ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(!format!("{err:?}").contains("test-password"));
    // Told once that it was refused, and asked what there is now: the
    // same. Sent again it would be refused again, and a server locks an
    // account out after so many wrong passwords.
    assert_eq!(saved.refused.load(Ordering::SeqCst), 1);
    assert_eq!(saved.handed_out.load(Ordering::SeqCst), 2);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_server_that_takes_only_windows_sign_in_is_reported_as_that() {
    let rig = Rig::start().await;
    rig.answers(
        "GetItem",
        ResponseTemplate::new(401)
            .append_header("WWW-Authenticate", "Negotiate")
            .append_header("WWW-Authenticate", "NTLM"),
    )
    .await;

    let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    let shown = err.to_string();
    assert!(shown.contains("NTLM"), "{shown}");
    assert!(shown.contains("which Quill does not speak"), "{shown}");
    assert!(
        shown.contains("Basic authentication has to be enabled for EWS on the server"),
        "{shown}"
    );
    // The password was not what was refused, and trying it again would
    // only count against the account.
    assert_eq!(rig.secrets.refused.load(Ordering::SeqCst), 0);
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_server_that_takes_no_token_is_reported_as_that() {
    let rig = Rig::with_tokens().await;
    rig.answers("GetItem", unauthorized()).await;

    let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(
        err.to_string()
            .contains("does not take an OAuth sign-in for Exchange Web Services"),
        "{err}"
    );
    assert_eq!(rig.secrets.refused.load(Ordering::SeqCst), 0);
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_source_of_passwords_that_fails_is_the_error_and_nothing_is_sent() {
    struct NoPassword;
    #[async_trait]
    impl Credentials for NoPassword {
        async fn get(&self) -> Result<Credential> {
            Err(Error::Auth(
                "no password is saved for the account".to_owned(),
            ))
        }
    }
    let server = MockServer::start().await;
    let ews = EwsBackend::loopback(config(&server), Arc::new(NoPassword)).unwrap();

    let err = ews.list_folders().await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_password_is_not_sent_without_a_user_name() {
    let server = MockServer::start().await;
    let ews = EwsBackend::loopback(
        EwsConfig::new(format!("{}{ENDPOINT}", server.uri()), ""),
        quill_core::credentials::password("test-password-1"),
    )
    .unwrap();

    let err = ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_busy_server_is_left_alone_for_as_long_as_it_asks_and_then_asked_again() {
    let rig = Arc::new(Rig::start().await);
    rig.on("GetItem")
        .respond_with(busy(Some(2000)))
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    rig.answers("GetItem", body()).await;

    let call = tokio::spawn({
        let rig = rig.clone();
        async move { rig.ews.fetch_raw(&m1()).await }
    });
    // In real time, until the refusal has been read and the wait begun.
    // The clock cannot be stopped sooner: a stopped clock runs ahead to the
    // next timer whenever nothing else is ready, and the request's own time
    // limit would strike while the answer was still on its way.
    received(&rig.server, 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!call.is_finished());

    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(1900)).await;
    // Not a moment before the two seconds are up.
    assert!(!call.is_finished());
    tokio::time::advance(Duration::from_millis(200)).await;
    tokio::time::resume();

    // And not long after them either.
    let raw = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(&raw[..], RAW);
    let requests = rig.check().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn busy_is_understood_however_the_server_says_it() {
    for said in [
        // A fault, with and without a time.
        busy(Some(0)),
        busy(None).insert_header("Retry-After", "0"),
        // An answer in which every part is "busy".
        answer(
            "GetItem",
            &[failure(
                "GetItem",
                "ErrorServerBusy",
                "The server cannot service this request right now. Try again later.",
            )
            .replace(
                "<m:DescriptiveLinkKey>",
                "<m:MessageXml><t:Value Name=\"BackOffMilliseconds\">0</t:Value></m:MessageXml>\
                 <m:DescriptiveLinkKey>",
            )],
        ),
        // The web server in front, with no word from Exchange at all.
        ResponseTemplate::new(503)
            .insert_header("Retry-After", "0")
            .set_body_raw("<html>Service Unavailable</html>", "text/html"),
        ResponseTemplate::new(429).insert_header("Retry-After", "0"),
    ] {
        let rig = Rig::start().await;
        rig.on("GetItem")
            .respond_with(said)
            .up_to_n_times(2)
            .mount(&rig.server)
            .await;
        rig.answers("GetItem", body()).await;

        rig.ews.fetch_raw(&m1()).await.unwrap();
        assert_eq!(rig.check().await.len(), 3);
    }
}

#[tokio::test]
async fn a_server_that_stays_busy_is_given_up_on_for_now() {
    let rig = Rig::start().await;
    rig.answers("GetItem", busy(Some(0))).await;

    // The sync engine comes back to it later, with its own patience.
    let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorServerBusy"), "{err}");
    // The first try and three more.
    assert_eq!(rig.check().await.len(), 4);
}

#[tokio::test]
async fn a_wait_too_long_to_sit_through_is_handed_back_at_once() {
    for said in [
        busy(Some(600_000)),
        ResponseTemplate::new(503).insert_header("Retry-After", "600"),
    ] {
        let rig = Rig::start().await;
        rig.answers("GetItem", said).await;

        let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
        assert!(matches!(err, Error::Temporary(_)), "{err:?}");
        assert!(err.to_string().contains("600 seconds"), "{err}");
        // Asked once, and not again before the time it named.
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn each_refusal_is_the_error_the_sync_engine_expects() {
    type Check = fn(&Error) -> bool;
    let cases: [(ResponseTemplate, Check, &str); 10] = [
        // The sign-in was good; what it may not do is not a failed
        // sign-in.
        (
            answer(
                "GetItem",
                &[failure(
                    "GetItem",
                    "ErrorAccessDenied",
                    "Access is denied. Check credentials and try again.",
                )],
            ),
            |err| matches!(err, Error::Backend(_)),
            "Access is denied. Check credentials and try again. (ErrorAccessDenied)",
        ),
        (
            ResponseTemplate::new(403).set_body_raw("<html>Forbidden</html>", "text/html"),
            |err| matches!(err, Error::Backend(_)),
            "HTTP 403 Forbidden",
        ),
        // A SOAP fault: the request as a whole was turned down.
        (
            fault(
                "ErrorSchemaValidation",
                "The request failed schema validation: The element 'ItemShape' has invalid \
                 child element 'Unheard'.",
                None,
            ),
            |err| matches!(err, Error::Backend(_)),
            "The request failed schema validation: The element 'ItemShape' has invalid child \
             element 'Unheard'. (ErrorSchemaValidation)",
        ),
        (
            fault(
                "ErrorInvalidServerVersion",
                "The specified server version is invalid.",
                None,
            ),
            |err| matches!(err, Error::Backend(_)),
            "(ErrorInvalidServerVersion)",
        ),
        // Not sent again by itself: nothing says the first try did
        // nothing.
        (
            fault(
                "ErrorInternalServerError",
                "An internal server error occurred. The operation failed.",
                None,
            ),
            |err| matches!(err, Error::Temporary(_)),
            "(ErrorInternalServerError)",
        ),
        (
            soap(
                500,
                "<s:Fault><faultcode>s:Server</faultcode>\
                 <faultstring>Server was unable to process request.</faultstring></s:Fault>",
            ),
            |err| matches!(err, Error::Temporary(_)),
            "Server was unable to process request. (Server)",
        ),
        (
            soap(
                500,
                "<s:Fault><faultcode>s:Client</faultcode>\
                 <faultstring>The request is not one this service takes.</faultstring></s:Fault>",
            ),
            |err| matches!(err, Error::Backend(_)),
            "(Client)",
        ),
        // No words from Exchange at all: a gateway's page.
        (
            ResponseTemplate::new(502).set_body_raw("<html>Bad Gateway</html>", "text/html"),
            |err| matches!(err, Error::Temporary(_)),
            "HTTP 502 Bad Gateway",
        ),
        // The address is wrong. Not "the message is gone": the sync
        // engine would forget a message it was told that about.
        (
            ResponseTemplate::new(404).set_body_raw("<html>Not Found</html>", "text/html"),
            |err| matches!(err, Error::Backend(_)),
            "HTTP 404 Not Found",
        ),
        (
            answer(
                "GetItem",
                &[failure(
                    "GetItem",
                    "ErrorPasswordExpired",
                    "The password has expired.",
                )],
            ),
            |err| matches!(err, Error::Auth(_)),
            "(ErrorPasswordExpired)",
        ),
    ];
    for (answer, is_expected, words) in cases {
        let rig = Rig::start().await;
        rig.answers("GetItem", answer).await;

        let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
        assert!(is_expected(&err), "{err:?}");
        assert!(err.to_string().contains(words), "{err}");
        assert!(!format!("{err:?}").contains("test-password"));
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn the_servers_words_are_cleaned_before_anyone_reads_them() {
    let rig = Rig::start().await;
    let pair = BASE64.encode("robin@example.com:test-password-1");
    // A server that echoes what it was sent, and more.
    rig.answers(
        "GetItem",
        fault(
            "ErrorInvalidRequest",
            &format!(
                "Bad request from test-password-1 (Basic {pair})&#x1b;[2J&#13;&#10;{}",
                "x".repeat(5000)
            ),
            None,
        ),
    )
    .await;

    let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    let shown = err.to_string();
    assert!(
        shown.contains("Bad request from [secret] (Basic [secret]) [2J  xxx"),
        "{shown}"
    );
    assert!(!shown.contains("test-password") && !shown.contains(&pair));
    assert!(!shown.contains('\u{1b}') && !shown.contains('\n'));
    assert!(shown.chars().count() < 500, "{}", shown.len());
    rig.check().await;
}

/// Every call that reads an answer, against whatever `answer` is. Returns
/// each one's error.
async fn everything_fails(rig: &Rig) -> Vec<Error> {
    let flags = Flags {
        seen: true,
        ..Flags::default()
    };
    vec![
        rig.ews.list_folders().await.unwrap_err(),
        rig.ews.sync_folder(&inbox(), None).await.unwrap_err(),
        rig.ews.fetch_raw(&m1()).await.unwrap_err(),
        rig.ews.fetch_many(&[m1(), m1()]).await.unwrap_err(),
        rig.ews
            .apply(&Operation::SetFlags { id: m1(), flags })
            .await
            .unwrap_err(),
        rig.ews.move_message(&m1(), &inbox()).await.unwrap_err(),
        rig.ews
            .apply(&Operation::Copy {
                id: m1(),
                to: inbox(),
            })
            .await
            .unwrap_err(),
        rig.ews.delete_many(&[m1()]).await.unwrap_err(),
        rig.ews.append(&inbox(), RAW, flags).await.unwrap_err(),
    ]
}

#[tokio::test]
async fn an_answer_that_is_not_what_it_should_be_is_a_backend_error_and_never_a_panic() {
    let whole = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
        <m:GetItemResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" \
        xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">\
        <m:ResponseMessages><m:GetItemResponseMessage ResponseClass=\"Success\">\
        <m:ResponseCode>NoError</m:ResponseCode><m:Items><t:Message>\
        <t:MimeContent CharacterSet=\"UTF-8\">U3ViamVjdDogeA0KDQo=</t:MimeContent>\
        <t:ItemId Id=\"m1\"/></t:Message></m:Items></m:GetItemResponseMessage>\
        </m:ResponseMessages></m:GetItemResponse></s:Body></s:Envelope>";
    let mut bodies: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"this is not XML".to_vec(),
        b"{\"error\": {\"code\": \"NotXml\"}}".to_vec(),
        b"<html><body>Please sign in</body></html>".to_vec(),
        // Cut off: in the middle of a tag, of text, of an attribute.
        whole.as_bytes()[..whole.len() / 2].to_vec(),
        whole.as_bytes()[..whole.len() - 20].to_vec(),
        whole.as_bytes()[..whole.find("NoError").unwrap() + 3].to_vec(),
        whole.as_bytes()[..whole.find("ResponseClass").unwrap() + 16].to_vec(),
        // Not text at all.
        vec![0xff, 0xfe, 0x00, 0x3c, 0x00, 0x61, 0x00, 0x3e],
        whole.replace("NoError", "No\u{0}Error").into_bytes(),
        [whole.as_bytes(), &[0xc3, 0x28][..]].concat(),
        // The right elements in nobody's namespace.
        whole
            .replace("http://schemas.microsoft.com/exchange/services/2006/messages", "urn:example")
            .into_bytes(),
        whole
            .replace("http://schemas.xmlsoap.org/soap/envelope/", "urn:example")
            .into_bytes(),
        // Two documents in one.
        format!("{whole}{whole}").into_bytes(),
        // A document type, with entities to expand: a few bytes that
        // would be gigabytes.
        format!(
            "<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\">\
             <!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\
             <!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\
             <!ENTITY lol4 \"&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;\">]>{}",
            whole
                .strip_prefix("<?xml version=\"1.0\" encoding=\"utf-8\"?>")
                .unwrap()
                .replace("NoError", "&lol4;")
        )
        .into_bytes(),
        // A file on this machine, by name.
        format!(
            "<!DOCTYPE a [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>{}",
            whole
                .strip_prefix("<?xml version=\"1.0\" encoding=\"utf-8\"?>")
                .unwrap()
                .replace("NoError", "&xxe;")
        )
        .into_bytes(),
        // An entity nobody defined.
        whole.replace("NoError", "&nbsp;").into_bytes(),
        // Nesting without end.
        format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000)).into_bytes(),
        "<a>".repeat(300_000).into_bytes(),
        // Elements without number.
        format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>{}</s:Body></s:Envelope>",
            "<a/>".repeat(1_100_000)
        )
        .into_bytes(),
    ];
    // Text without end, inside an answer that is otherwise in order.
    bodies.push(
        whole
            .replace("NoError", &"NoError".repeat(1_000_000))
            .into_bytes(),
    );

    for body in bodies {
        let rig = Rig::start().await;
        let shown: String = String::from_utf8_lossy(&body).chars().take(60).collect();
        always(
            &rig,
            ResponseTemplate::new(200).set_body_raw(body, "text/xml"),
        )
        .await;

        for err in everything_fails(&rig).await {
            assert!(matches!(err, Error::Backend(_)), "{shown}: {err:?}");
            // Nothing of a hostile answer is repeated at length.
            assert!(err.to_string().chars().count() < 600, "{shown}: {err}");
        }
        rig.check().await;
    }
}

/// Mounts one answer to every request, whatever it asks.
async fn always(rig: &Rig, answer: ResponseTemplate) {
    Mock::given(method("POST"))
        .respond_with(answer)
        .mount(&rig.server)
        .await;
}

#[tokio::test]
async fn answers_to_the_wrong_number_of_questions_are_a_backend_error() {
    for messages in [
        Vec::new(),
        vec![mime("m1", RAW), mime("m1", RAW), mime("m1", RAW)],
    ] {
        let rig = Rig::start().await;
        // Whatever is asked, it is answered as if it were `GetItem`.
        always(&rig, answer("GetItem", &messages)).await;

        for err in everything_fails(&rig).await {
            assert!(matches!(err, Error::Backend(_)), "{err:?}");
        }
        rig.check().await;
    }
}

#[tokio::test]
async fn huge_text_inside_an_answer_is_cut_to_size_and_not_passed_on() {
    let rig = Rig::start().await;
    let long = "Lorem ipsum ".repeat(200_000);
    let item = format!(
        "<t:Message><t:ItemId Id=\"m1\"/><t:Subject>{long}</t:Subject>\
         <t:From><t:Mailbox><t:Name>{long}</t:Name>\
         <t:EmailAddress>ana.ruiz@example.com</t:EmailAddress></t:Mailbox></t:From>\
         <t:References>&lt;{long}@example.com&gt;</t:References></t:Message>"
    );
    rig.answers(
        "SyncFolderItems",
        answer(
            "SyncFolderItems",
            &[common::success(
                "SyncFolderItems",
                &format!(
                    "<m:SyncState>H4sIAAA=</m:SyncState>\
                     <m:IncludesLastItemInRange>true</m:IncludesLastItemInRange>\
                     <m:Changes><t:Create>{item}</t:Create></m:Changes>"
                ),
            )],
        ),
    )
    .await;
    rig.answers("GetItem", answer("GetItem", &[common::got(&item)]))
        .await;

    let delta = rig.ews.sync_folder(&inbox(), None).await.unwrap();
    let message = &delta.added[0];
    assert!(message.headers.subject.len() < 1000);
    assert!(message.headers.from[0].name.as_ref().unwrap().len() < 1000);
    assert!(message.thread.is_empty());
    rig.check().await;
}

#[tokio::test]
async fn an_answer_larger_than_is_read_is_given_up_whether_or_not_it_says_how_large() {
    let rig = Rig::start().await;
    rig.answers(
        "GetItem",
        answer("GetItem", &[mime("m1", &vec![b'x'; 300_000])]),
    )
    .await;
    let cramped = rig.backend().with_answer_limit(100_000);

    let err = cramped.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(
        err.to_string().contains("larger than the 100000 bytes"),
        "{err}"
    );
    // And it is read when there is room for it.
    assert_eq!(rig.ews.fetch_raw(&m1()).await.unwrap().len(), 300_000);
    rig.check().await;

    // A server that sends without saying how much is coming.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}{ENDPOINT}", listener.local_addr().unwrap());
    let endless = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = connection.read(&mut request).await;
        let _ = connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nConnection: close\r\n\r\n<a>")
            .await;
        // Until the reader has had enough and hangs up.
        let chunk = vec![b'x'; 64 * 1024];
        let mut written = 0usize;
        while connection.write_all(&chunk).await.is_ok() {
            written += chunk.len();
            assert!(
                written < 64 * 1024 * 1024,
                "the answer was read without end"
            );
        }
    });
    let secrets = Arc::new(Secrets::default());
    let ews = EwsBackend::loopback(EwsConfig::new(url, USER), secrets.clone())
        .unwrap()
        .with_answer_limit(1024 * 1024);
    let err = ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("larger than"), "{err}");
    tokio::time::timeout(Duration::from_secs(30), endless)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(secrets.handed_out.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let elsewhere = MockServer::start().await;
    let rig = Rig::start().await;
    for status in [301, 302, 307, 308] {
        rig.on("GetItem")
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("Location", format!("{}{ENDPOINT}", elsewhere.uri())),
            )
            .up_to_n_times(1)
            .mount(&rig.server)
            .await;
    }

    // The password goes to the server it is for and to no other.
    for _ in 0..4 {
        let err = rig.ews.fetch_raw(&m1()).await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
    }
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
    assert_eq!(rig.check().await.len(), 4);
}

#[tokio::test]
async fn a_connection_that_drops_is_a_connection_error() {
    let secrets = Arc::new(Secrets::default());
    // Something on this machine that takes a connection and hangs up
    // without a word. Not a port with nothing behind it: a port that has
    // just been let go may be the next test's server by the time it is
    // tried.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}{ENDPOINT}", listener.local_addr().unwrap());
    let hanging_up = std::thread::spawn(move || {
        for connection in listener.incoming().take(2) {
            drop(connection);
        }
    });
    let ews = EwsBackend::loopback(EwsConfig::new(url, USER), secrets.clone()).unwrap();

    let err = ews.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    let err = ews.sync_folder(&inbox(), None).await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    assert!(!format!("{err:?}").contains("test-password"));
    // Tried once each: the sync engine decides when to come back.
    assert_eq!(secrets.handed_out.load(Ordering::SeqCst), 2);
    hanging_up.join().unwrap();
}

#[tokio::test]
async fn a_server_that_says_nothing_does_not_hang_the_request() {
    let rig = Rig::start().await;
    rig.answers("GetItem", body().set_delay(Duration::from_secs(30)))
        .await;
    let impatient = rig
        .backend()
        .with_request_timeout(Duration::from_millis(200));

    let started = std::time::Instant::now();
    let err = impatient.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_real_server_is_never_reached_in_plain_http() {
    let secrets = Arc::new(Secrets::default());
    for url in [
        "http://mail.example.com/EWS/Exchange.asmx",
        "http://127.0.0.1:8080/EWS/Exchange.asmx",
        "ftp://mail.example.com/EWS/Exchange.asmx",
    ] {
        let err = EwsBackend::new(EwsConfig::new(url, USER), secrets.clone()).unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err:?}");
    }
    // And the way in for tests leads to this machine only.
    for url in [
        "http://mail.example.com/EWS/Exchange.asmx",
        "https://mail.example.com/EWS/Exchange.asmx",
        "http://localhost:8080/EWS/Exchange.asmx",
    ] {
        let err = EwsBackend::loopback(EwsConfig::new(url, USER), secrets.clone()).unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err:?}");
    }
    assert!(
        EwsBackend::new(
            EwsConfig::new("https://mail.example.com/EWS/Exchange.asmx", USER),
            secrets.clone()
        )
        .is_ok()
    );
    assert_eq!(secrets.handed_out.load(Ordering::SeqCst), 0);
}
