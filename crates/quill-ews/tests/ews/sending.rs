//! Sending through `CreateItem`: the message as it stands, and what the
//! server's refusals mean.

use quill_core::{Envelope, Error, Sender};
use quill_ews::EwsBackend;
use wiremock::ResponseTemplate;

use crate::common::{
    Rig, answer, busy, failure, fault, large_message, posted_message, soap, success,
};

/// A message as Quill keeps it: with its `Bcc` header.
const MESSAGE: &[u8] = b"From: Robin Okafor <robin@example.com>\r\n\
    To: Ana Ruiz <ana.ruiz@example.com>\r\n\
    Cc: \"Haddad, Omar\" <omar.haddad@example.com>\r\n\
    Bcc: Sam Park <sam.park@example.com>\r\n\
    Subject: Minutes, 30 October\r\n\
    Message-ID: <minutes.30@example.com>\r\n\
    MIME-Version: 1.0\r\n\
    Content-Type: text/plain; charset=utf-8\r\n\
    \r\n\
    Attached are the minutes.\r\n\
    .\r\n\
    A line with a dot of its own above.\r\n";

fn envelope(to: &[&str]) -> Envelope {
    Envelope {
        from: "robin@example.com".to_owned(),
        to: to.iter().map(|address| (*address).to_owned()).collect(),
    }
}

fn everyone() -> Envelope {
    envelope(&[
        "ana.ruiz@example.com",
        "omar.haddad@example.com",
        "sam.park@example.com",
    ])
}

/// Exchange's answer to a message it sent and filed.
fn sent() -> ResponseTemplate {
    answer("CreateItem", &[success("CreateItem", "<m:Items/>")])
}

/// Exchange's answer to a message it would not send.
fn refused(code: &str, text: &str) -> ResponseTemplate {
    answer("CreateItem", &[failure("CreateItem", code, text)])
}

// Exchange files what it sends. Whoever sets the account up passes this to
// the sync engine, so that no second copy is saved.
const _: () = assert!(EwsBackend::SAVES_SENT_COPY);

#[tokio::test]
async fn a_message_is_sent_as_it_stands_with_its_bcc_header() {
    let rig = Rig::start().await;
    rig.answers("CreateItem", sent()).await;

    rig.ews.send(MESSAGE, &everyone()).await.unwrap();

    let requests = rig.requests().await;
    // Exactly the message, and nothing done to it.
    let posted = posted_message(&requests[0]);
    assert!(posted == MESSAGE, "the message changed on the way");
    // Exchange learns who the blind recipients are from this header
    // alone, and leaves it out of what it delivers.
    assert!(
        posted.windows(5).any(|window| window == b"\r\nBcc"),
        "the Bcc header was taken out"
    );
    let asked = rig.check().await;
    assert_eq!(asked.len(), 1);
    // Sent, and filed in Sent Items, in one request.
    assert!(
        asked[0].starts_with(
            "<m:CreateItem MessageDisposition=\"SendAndSaveCopy\"><m:SavedItemFolderId>\
             <t:DistinguishedFolderId Id=\"sentitems\"/></m:SavedItemFolderId><m:Items>\
             <t:Message><t:MimeContent CharacterSet=\"UTF-8\">"
        ),
        "{}",
        asked[0]
    );
    assert!(
        asked[0].ends_with("</t:MimeContent></t:Message></m:Items></m:CreateItem>"),
        "{}",
        asked[0]
    );
}

#[tokio::test]
async fn a_large_message_is_sent_byte_for_byte() {
    let rig = Rig::start().await;
    rig.answers("CreateItem", sent()).await;
    let raw = large_message();

    rig.ews
        .send(&raw, &envelope(&["ana.ruiz@example.com"]))
        .await
        .unwrap();

    let requests = rig.requests().await;
    assert!(
        posted_message(&requests[0]) == raw,
        "the message changed on the way"
    );
    rig.check().await;
}

#[tokio::test]
async fn a_recipient_the_headers_do_not_name_stops_the_send() {
    let rig = Rig::start().await;
    rig.answers("CreateItem", sent()).await;
    // The same message with `Bcc` taken out, as SMTP wants it. Exchange
    // would deliver it to Ana and Omar and never tell anyone Sam got
    // nothing.
    let stripped: Vec<u8> = String::from_utf8(MESSAGE.to_vec())
        .unwrap()
        .replace("Bcc: Sam Park <sam.park@example.com>\r\n", "")
        .into_bytes();

    let err = rig.ews.send(&stripped, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    assert!(err.to_string().contains("sam.park@example.com"), "{err}");
    // Nothing was sent to anyone.
    assert!(rig.check().await.is_empty());
}

#[tokio::test]
async fn what_the_server_holds_against_a_message_is_a_refusal() {
    for (answer, words) in [
        (
            refused(
                "ErrorInvalidRecipients",
                "At least one recipient isn't valid.",
            ),
            "Exchange: At least one recipient isn't valid. (ErrorInvalidRecipients)",
        ),
        (
            refused(
                "ErrorSendAsDenied",
                "The user account which was used to submit this request does not have the right \
                 to send mail on behalf of the specified sending account.",
            ),
            "(ErrorSendAsDenied)",
        ),
        (
            refused(
                "ErrorMessageSizeExceeded",
                "The message exceeds the maximum supported size.",
            ),
            "The message exceeds the maximum supported size. (ErrorMessageSizeExceeded)",
        ),
        // The request itself was not one the server would read.
        (
            fault(
                "ErrorSchemaValidation",
                "The request failed schema validation.",
                None,
            ),
            "The request failed schema validation. (ErrorSchemaValidation)",
        ),
        // Too large for the web server in front, which says so in its own
        // way.
        (
            ResponseTemplate::new(413)
                .set_body_raw("<html>Request Entity Too Large</html>", "text/html"),
            "HTTP 413 Payload Too Large",
        ),
        (ResponseTemplate::new(400), "HTTP 400 Bad Request"),
    ] {
        let rig = Rig::start().await;
        rig.answers("CreateItem", answer).await;

        // Kept in the Outbox with the server's words, not sent again.
        let err = rig.ews.send(MESSAGE, &everyone()).await.unwrap_err();
        assert!(matches!(err, Error::Refused(_)), "{err:?}");
        assert!(err.to_string().contains(words), "{err}");
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_server_that_fails_midway_does_not_promise_nothing_was_sent() {
    for (answer, words) in [
        (
            fault(
                "ErrorInternalServerError",
                "An internal server error occurred. The operation failed.",
                None,
            ),
            "ErrorInternalServerError",
        ),
        (
            refused(
                "ErrorInternalServerTransientError",
                "An internal server error occurred. Try again later.",
            ),
            "ErrorInternalServerTransientError",
        ),
        (
            ResponseTemplate::new(500).set_body_raw("<html>Server Error</html>", "text/html"),
            "HTTP 500",
        ),
        (ResponseTemplate::new(504), "HTTP 504"),
        // "All is well", in an answer that cannot be read or does not say
        // what became of the message.
        (
            ResponseTemplate::new(200).set_body_raw("<html>Welcome</html>", "text/html"),
            "did not say whether the message was sent",
        ),
        (
            ResponseTemplate::new(200).set_body_raw("<s:Envelope", "text/xml"),
            "did not say whether the message was sent",
        ),
        (
            answer("CreateItem", &[]),
            "did not say whether the message was sent",
        ),
        (soap(200, ""), "did not say whether the message was sent"),
    ] {
        let rig = Rig::start().await;
        rig.answers("CreateItem", answer).await;

        // Kept and tried again later, like a connection that dropped; and
        // like one, not tried again here, where a second try could be a
        // second copy.
        let err = rig.ews.send(MESSAGE, &everyone()).await.unwrap_err();
        assert!(matches!(err, Error::Connection(_)), "{err:?}");
        assert!(err.to_string().contains(words), "{err}");
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_server_too_busy_to_take_the_message_has_sent_nothing() {
    let rig = Rig::start().await;
    // Five minutes: longer than anyone waits here.
    rig.answers("CreateItem", busy(Some(297_749))).await;

    let err = rig.ews.send(MESSAGE, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert!(err.to_string().contains("297 seconds"), "{err}");
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_sender_over_the_rate_limit_has_sent_nothing_and_may_later() {
    let rig = Rig::start().await;
    rig.answers(
        "CreateItem",
        refused(
            "ErrorSubmissionQuotaExceeded",
            "The message can't be submitted because the sender's submission quota was exceeded.",
        ),
    )
    .await;

    let err = rig.ews.send(MESSAGE, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert!(
        err.to_string().contains("ErrorSubmissionQuotaExceeded"),
        "{err}"
    );
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_busy_server_gets_the_same_message_again() {
    let rig = Rig::start().await;
    rig.on("CreateItem")
        .respond_with(busy(Some(0)))
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    rig.answers("CreateItem", sent()).await;

    rig.ews.send(MESSAGE, &everyone()).await.unwrap();

    let requests = rig.requests().await;
    assert_eq!(rig.check().await.len(), 2);
    assert_eq!(requests[0].body, requests[1].body);
    assert!(posted_message(&requests[1]) == MESSAGE);
}

#[tokio::test]
async fn a_refused_sign_in_while_sending_is_a_failed_sign_in() {
    let rig = Rig::start().await;
    rig.answers(
        "CreateItem",
        ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Basic realm=\"mail\""),
    )
    .await;

    let err = rig.ews.send(MESSAGE, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    // Turned away at the door both times: nothing was sent.
    assert_eq!(rig.check().await.len(), 2);
}
