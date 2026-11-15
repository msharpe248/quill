//! Tokens, throttling, refusals, and a service that cannot be reached.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use quill_core::types::{FolderId, RemoteId};
use quill_core::{Backend, Error, Result, TokenSource};
use quill_graph::{GraphBackend, GraphConfig};
use wiremock::matchers::header;
use wiremock::{MockServer, ResponseTemplate};

use crate::common::{self, Rig, Tokens, received, refusal};

const RAW: &str = "/me/messages/m1/$value";

fn m1() -> RemoteId {
    RemoteId("m1".to_owned())
}

fn body() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw("Subject: Minutes\r\n\r\nHello\r\n", "text/plain")
}

fn expired_token() -> ResponseTemplate {
    refusal(
        401,
        "InvalidAuthenticationToken",
        "Lifetime validation failed, the token is expired.",
    )
}

fn throttled() -> ResponseTemplate {
    refusal(
        429,
        "ApplicationThrottled",
        "Application is over its MailboxConcurrency limit.",
    )
}

#[tokio::test]
async fn every_request_carries_the_token_the_source_has_just_then() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(body())
        .mount(&rig.server)
        .await;

    rig.graph.fetch_raw(&m1()).await.unwrap();
    // The source renews its token between two requests, as it does when
    // one is about to run out.
    rig.tokens.invalidate().await;
    rig.graph.fetch_raw(&m1()).await.unwrap();

    let requests = rig.requests().await;
    assert_eq!(
        common::header(&requests[0], "authorization"),
        "Bearer test-access-token-1"
    );
    assert_eq!(
        common::header(&requests[1], "authorization"),
        "Bearer test-access-token-2"
    );
    rig.check().await;
}

#[tokio::test]
async fn a_refused_token_is_forgotten_and_the_request_sent_again_with_a_new_one() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .and(header("authorization", "Bearer test-access-token-1"))
        .respond_with(expired_token())
        .mount(&rig.server)
        .await;
    rig.on("GET", RAW)
        .and(header("authorization", "Bearer test-access-token-2"))
        .respond_with(body())
        .mount(&rig.server)
        .await;

    let raw = rig.graph.fetch_raw(&m1()).await.unwrap();
    assert_eq!(&raw[..], b"Subject: Minutes\r\n\r\nHello\r\n");
    assert_eq!(rig.tokens.refused.load(Ordering::SeqCst), 1);
    assert_eq!(rig.check().await.len(), 2);
}

#[tokio::test]
async fn a_second_refusal_is_a_failed_sign_in_and_the_token_is_not_in_the_error() {
    let rig = Rig::start().await;
    // A service that echoes what it was sent.
    rig.on("GET", RAW)
        .respond_with(refusal(
            401,
            "InvalidAuthenticationToken",
            "Access token test-access-token-2 has expired or is not yet valid.",
        ))
        .mount(&rig.server)
        .await;

    let err = rig.graph.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    // The token was renewed once, and that was all.
    assert_eq!(rig.tokens.refused.load(Ordering::SeqCst), 1);
    assert_eq!(rig.check().await.len(), 2);
    for shown in [err.to_string(), format!("{err:?}")] {
        assert!(!shown.contains("test-access-token"), "{shown}");
        assert!(!shown.contains("Bearer"), "{shown}");
        assert!(shown.contains("InvalidAuthenticationToken"), "{shown}");
    }
}

#[tokio::test]
async fn a_token_source_that_fails_is_the_error_and_nothing_is_sent() {
    struct SignedOut;
    #[async_trait]
    impl TokenSource for SignedOut {
        async fn access_token(&self) -> Result<String> {
            Err(Error::Auth(
                "the account must be signed in to again".to_owned(),
            ))
        }
        async fn invalidate(&self) {}
    }
    let server = MockServer::start().await;
    let graph = GraphBackend::loopback(
        GraphConfig {
            base_url: format!("{}/v1.0", server.uri()),
            page_size: 50,
        },
        Arc::new(SignedOut),
    )
    .unwrap();

    let err = graph.list_folders().await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_throttled_request_waits_as_long_as_it_is_told_and_goes_again() {
    let rig = Arc::new(Rig::start().await);
    rig.on("GET", RAW)
        .respond_with(throttled().insert_header("Retry-After", "2"))
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    rig.on("GET", RAW)
        .respond_with(body())
        .mount(&rig.server)
        .await;

    let call = tokio::spawn({
        let rig = rig.clone();
        async move { rig.graph.fetch_raw(&m1()).await }
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
    assert_eq!(&raw[..], b"Subject: Minutes\r\n\r\nHello\r\n");
    let requests = rig.check().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn an_unavailable_service_is_waited_for_too() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(
            refusal(
                503,
                "serviceNotAvailable",
                "The service is temporarily unavailable.",
            )
            .insert_header("Retry-After", "0"),
        )
        .up_to_n_times(2)
        .mount(&rig.server)
        .await;
    rig.on("GET", RAW)
        .respond_with(body())
        .mount(&rig.server)
        .await;

    rig.graph.fetch_raw(&m1()).await.unwrap();
    assert_eq!(rig.check().await.len(), 3);
}

#[tokio::test]
async fn a_service_that_stays_busy_is_given_up_on_for_now() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(throttled().insert_header("Retry-After", "0"))
        .mount(&rig.server)
        .await;

    // The sync engine comes back to it later, with its own patience.
    let err = rig.graph.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert!(err.to_string().contains("ApplicationThrottled"), "{err}");
    // The first try and three more.
    assert_eq!(rig.check().await.len(), 4);
}

#[tokio::test]
async fn a_wait_too_long_to_sit_through_is_handed_back_at_once() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(throttled().insert_header("Retry-After", "600"))
        .mount(&rig.server)
        .await;

    let err = rig.graph.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert!(err.to_string().contains("600 seconds"), "{err}");
    // Asked once, and not again before the time it named.
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn each_refusal_is_the_error_the_sync_engine_expects() {
    type Check = fn(&Error) -> bool;
    let cases: [(ResponseTemplate, Check, &str); 7] = [
        // The token was good; what it may not do is not a failed sign-in.
        (
            refusal(
                403,
                "ErrorAccessDenied",
                "Access is denied. Check credentials and try again.",
            ),
            |err| matches!(err, Error::Backend(_)),
            "Access is denied. Check credentials and try again. (ErrorAccessDenied)",
        ),
        // Not sent again by itself: nothing says the first try did nothing.
        (
            refusal(504, "UnknownError", "The gateway timed out."),
            |err| matches!(err, Error::Temporary(_)),
            "The gateway timed out. (UnknownError)",
        ),
        (
            refusal(
                404,
                "ErrorItemNotFound",
                "The specified object was not found in the store.",
            ),
            |err| matches!(err, Error::NotFound(_)),
            "(ErrorItemNotFound)",
        ),
        (
            refusal(400, "BadRequest", "Invalid request."),
            |err| matches!(err, Error::Backend(_)),
            "Invalid request. (BadRequest)",
        ),
        (
            refusal(
                500,
                "ErrorInternalServerError",
                "An internal server error occurred.",
            ),
            |err| matches!(err, Error::Temporary(_)),
            "(ErrorInternalServerError)",
        ),
        // No words from Graph at all: a gateway's page.
        (
            ResponseTemplate::new(502).set_body_raw("<html>Bad Gateway</html>", "text/html"),
            |err| matches!(err, Error::Temporary(_)),
            "HTTP 502 Bad Gateway",
        ),
        (
            ResponseTemplate::new(400)
                .set_body_raw("{\"error\": {\"code\": \"OnlyACode\"}}", "application/json"),
            |err| matches!(err, Error::Backend(_)),
            "OnlyACode (HTTP 400)",
        ),
    ];
    for (answer, is_expected, words) in cases {
        let rig = Rig::start().await;
        rig.on("GET", RAW)
            .respond_with(answer)
            .mount(&rig.server)
            .await;

        let err = rig.graph.fetch_raw(&m1()).await.unwrap_err();
        assert!(is_expected(&err), "{err:?}");
        assert!(err.to_string().contains(words), "{err}");
        assert!(!format!("{err:?}").contains("test-access-token"));
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let elsewhere = MockServer::start().await;
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/v1.0{RAW}", elsewhere.uri())),
        )
        .mount(&rig.server)
        .await;

    // The token goes to the service it was got for and to no other.
    let err = rig.graph.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_connection_that_drops_is_a_connection_error() {
    let tokens = Arc::new(Tokens::default());
    // Something on this machine that takes a connection and hangs up
    // without a word. Not a port with nothing behind it: a port that has
    // just been let go may be the next test's server by the time it is
    // tried.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1.0", listener.local_addr().unwrap());
    let hanging_up = std::thread::spawn(move || {
        for connection in listener.incoming().take(2) {
            drop(connection);
        }
    });
    let graph = GraphBackend::loopback(
        GraphConfig {
            base_url,
            page_size: 50,
        },
        tokens.clone(),
    )
    .unwrap();

    let err = graph.fetch_raw(&m1()).await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    let err = graph
        .sync_folder(&FolderId("folder-inbox".to_owned()), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    assert!(!format!("{err:?}").contains("test-access-token"));
    // Tried once each: the sync engine decides when to come back.
    assert_eq!(tokens.handed_out.load(Ordering::SeqCst), 2);
    hanging_up.join().unwrap();
}

#[tokio::test]
async fn a_server_that_says_nothing_does_not_hang_the_request() {
    let rig = Rig::start().await;
    rig.on("GET", RAW)
        .respond_with(body().set_delay(Duration::from_secs(30)))
        .mount(&rig.server)
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
async fn the_real_service_is_never_reached_in_plain_http() {
    let tokens = Arc::new(Tokens::default());
    for base_url in [
        "http://graph.microsoft.com/v1.0",
        "http://127.0.0.1:8080/v1.0",
    ] {
        let err = GraphBackend::new(
            GraphConfig {
                base_url: base_url.to_owned(),
                page_size: 50,
            },
            tokens.clone(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err:?}");
    }
    // And the way in for tests leads to this machine only.
    for base_url in [
        "http://graph.microsoft.com/v1.0",
        "https://graph.microsoft.com/v1.0",
    ] {
        let err = GraphBackend::loopback(
            GraphConfig {
                base_url: base_url.to_owned(),
                page_size: 50,
            },
            tokens.clone(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err:?}");
    }
    assert!(GraphBackend::new(GraphConfig::default(), tokens.clone()).is_ok());
    assert_eq!(tokens.handed_out.load(Ordering::SeqCst), 0);
}
