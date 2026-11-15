//! Sending through `sendMail`: the message as it stands, and what the
//! service's refusals mean.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::{Envelope, Error, Sender};
use wiremock::ResponseTemplate;

use crate::common::{Rig, header, large_message, refusal};

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

async fn accepting(rig: &Rig) {
    rig.on("POST", "/me/sendMail")
        .respond_with(ResponseTemplate::new(202))
        .mount(&rig.server)
        .await;
}

#[tokio::test]
async fn a_message_is_sent_as_it_stands_with_its_bcc_header() {
    let rig = Rig::start().await;
    accepting(&rig).await;

    rig.graph.send(MESSAGE, &everyone()).await.unwrap();

    let requests = rig.requests().await;
    assert_eq!(rig.check().await, ["POST /v1.0/me/sendMail"]);
    assert_eq!(header(&requests[0], "content-type"), "text/plain");
    // Exactly the message in base64, and nothing around it.
    assert_eq!(requests[0].body, BASE64.encode(MESSAGE).into_bytes());
    // Graph learns who the blind recipients are from this header alone,
    // and leaves it out of what it delivers.
    let sent = BASE64.decode(&requests[0].body).unwrap();
    assert!(
        sent.windows(5).any(|window| window == b"\r\nBcc"),
        "the Bcc header was taken out"
    );
}

#[tokio::test]
async fn a_large_message_is_sent_byte_for_byte() {
    let rig = Rig::start().await;
    accepting(&rig).await;
    let raw = large_message();

    rig.graph
        .send(&raw, &envelope(&["ana.ruiz@example.com"]))
        .await
        .unwrap();

    let requests = rig.requests().await;
    assert!(
        BASE64.decode(&requests[0].body).unwrap() == raw,
        "the message changed on the way"
    );
    rig.check().await;
}

#[tokio::test]
async fn a_recipient_the_headers_do_not_name_stops_the_send() {
    let rig = Rig::start().await;
    accepting(&rig).await;
    // The same message with `Bcc` taken out, as SMTP wants it. Graph would
    // deliver it to Ana and Omar and never tell anyone Sam got nothing.
    let stripped: Vec<u8> = String::from_utf8(MESSAGE.to_vec())
        .unwrap()
        .replace("Bcc: Sam Park <sam.park@example.com>\r\n", "")
        .into_bytes();

    let err = rig.graph.send(&stripped, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    assert!(err.to_string().contains("sam.park@example.com"), "{err}");
    // Nothing was sent to anyone.
    assert!(rig.check().await.is_empty());
}

#[tokio::test]
async fn what_the_service_holds_against_a_message_is_a_refusal() {
    for (answer, words) in [
        (
            refusal(
                400,
                "ErrorInvalidRecipients",
                "At least one recipient isn't valid.",
            ),
            "At least one recipient isn't valid. (ErrorInvalidRecipients)",
        ),
        (
            refusal(
                403,
                "ErrorSendAsDenied",
                "The user account which was used to submit this request does not have the right to send mail on behalf of the specified sending account.",
            ),
            "(ErrorSendAsDenied)",
        ),
        (
            refusal(
                413,
                "ErrorMessageSizeExceeded",
                "The message exceeds the maximum supported size.",
            ),
            "The message exceeds the maximum supported size. (ErrorMessageSizeExceeded)",
        ),
        (ResponseTemplate::new(400), "HTTP 400 Bad Request"),
    ] {
        let rig = Rig::start().await;
        rig.on("POST", "/me/sendMail")
            .respond_with(answer)
            .mount(&rig.server)
            .await;

        // Kept in the Outbox with the service's words, not sent again.
        let err = rig.graph.send(MESSAGE, &everyone()).await.unwrap_err();
        assert!(matches!(err, Error::Refused(_)), "{err:?}");
        assert!(err.to_string().contains(words), "{err}");
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_service_that_fails_midway_does_not_promise_nothing_was_sent() {
    for status in [500, 504] {
        let rig = Rig::start().await;
        rig.on("POST", "/me/sendMail")
            .respond_with(refusal(
                status,
                "ErrorInternalServerError",
                "An internal server error occurred. The operation failed.",
            ))
            .mount(&rig.server)
            .await;

        // Kept and tried again later, like a connection that dropped; and
        // like one, not tried again here, where a second try could be a
        // second copy.
        let err = rig.graph.send(MESSAGE, &everyone()).await.unwrap_err();
        assert!(matches!(err, Error::Connection(_)), "{err:?}");
        assert!(
            err.to_string().contains("ErrorInternalServerError"),
            "{err}"
        );
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_service_too_busy_to_take_the_message_has_sent_nothing() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/sendMail")
        .respond_with(
            refusal(
                429,
                "ApplicationThrottled",
                "Application is over its MailboxConcurrency limit.",
            )
            .insert_header("Retry-After", "900"),
        )
        .mount(&rig.server)
        .await;

    let err = rig.graph.send(MESSAGE, &everyone()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_busy_service_gets_the_same_message_again() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/sendMail")
        .respond_with(
            refusal(
                429,
                "ApplicationThrottled",
                "Application is over its MailboxConcurrency limit.",
            )
            .insert_header("Retry-After", "0"),
        )
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    accepting(&rig).await;

    rig.graph.send(MESSAGE, &everyone()).await.unwrap();

    let requests = rig.requests().await;
    assert_eq!(rig.check().await.len(), 2);
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(requests[1].body, BASE64.encode(MESSAGE).into_bytes());
}
