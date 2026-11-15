//! An account's tokens over time: handed out, renewed, and given up on.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use quill_core::{Error, TokenSource};
use serde_json::{Value, json};
use tokio::time::advance;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::support::*;
use crate::oauth::{OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens};
use crate::{MemorySecrets, SecretStore};

/// The tokens of the account, picked up from `store` as on a later run.
async fn resumed(
    http: &OAuthHttp,
    server: &MockServer,
    store: Arc<dyn SecretStore>,
) -> OAuthTokens {
    OAuthTokens::resume(
        http,
        &provider(server),
        &OAuthClient::public(CLIENT_ID),
        store,
        ACCOUNT,
    )
    .await
    .unwrap()
}

/// Mounts a token endpoint that gives a different access token each time
/// (`access-1-AAAA`, `access-2-AAAA`, ...), each lasting `expires_in`.
/// Returns the count of requests it has answered.
async fn numbered_tokens(server: &MockServer, expires_in: Value) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &Request| {
            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let mut body = json!({
                "token_type": "Bearer",
                "access_token": format!("access-{number}-AAAA"),
            });
            if !expires_in.is_null() {
                body["expires_in"] = expires_in.clone();
            }
            ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(server)
        .await;
    count
}

#[tokio::test]
async fn an_account_that_never_signed_in_cannot_be_resumed() {
    let server = MockServer::start().await;
    let err = OAuthTokens::resume(
        &http(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        store(),
        ACCOUNT,
    )
    .await
    .unwrap_err();
    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(
        said.contains(ACCOUNT) && said.contains("Sign in again"),
        "{said}"
    );

    // An empty secret is no sign-in either.
    let err = OAuthTokens::resume(
        &http(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        signed_in_store(""),
        ACCOUNT,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_first_use_renews_and_the_next_uses_what_is_in_hand() {
    let server = MockServer::start().await;
    let count = numbered_tokens(&server, json!(3600)).await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(&http(), &server, store.clone()).await;
    // Resuming asks nothing of the provider.
    assert_eq!(count.load(Ordering::SeqCst), 0);

    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // The renewal was exactly this, as a form: no secret that was not
    // given, no scope, nothing else.
    let renewals = requests(&server, "/token").await;
    assert_eq!(
        form(&renewals[0]),
        fields(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", REFRESH),
            ("client_id", CLIENT_ID),
        ])
    );
    // The access token went nowhere near the store.
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
    assert_clean(&tokens, &[REFRESH, "access-1-AAAA"]);
}

#[tokio::test]
async fn a_client_secret_is_sent_with_a_renewal_when_there_is_one() {
    let server = MockServer::start().await;
    numbered_tokens(&server, json!(3600)).await;
    let tokens = OAuthTokens::resume(
        &http(),
        &provider(&server),
        &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
        signed_in_store(REFRESH),
        ACCOUNT,
    )
    .await
    .unwrap();
    tokens.access_token().await.unwrap();
    assert_eq!(
        form(&requests(&server, "/token").await[0]),
        fields(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", REFRESH),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
    );
    assert_clean(&tokens, &[CLIENT_SECRET, REFRESH]);
}

#[tokio::test(start_paused = true)]
async fn a_token_is_renewed_when_it_has_under_a_minute_left() {
    let server = MockServer::start().await;
    let count = numbered_tokens(&server, json!(3600)).await;
    let tokens = resumed(&OAuthHttp::unhurried(), &server, signed_in_store(REFRESH)).await;

    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    // 61 seconds left: still good.
    advance(Duration::from_secs(3539)).await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    // 59 seconds left: not enough to be sure it outlives its use.
    advance(Duration::from_secs(2)).await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_token_a_server_refused_is_not_offered_again() {
    let server = MockServer::start().await;
    let count = numbered_tokens(&server, json!(3600)).await;
    let tokens = resumed(&http(), &server, signed_in_store(REFRESH)).await;

    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    tokens.invalidate().await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
    assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    // Invalidating twice, or with nothing in hand, costs one renewal.
    tokens.invalidate().await;
    tokens.invalidate().await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-3-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_callers_at_once_make_one_request() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &Request| {
            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
            // Slow enough that every caller arrives while it is in flight.
            tokens(&format!("access-{number}-AAAA"), None).set_delay(Duration::from_millis(300))
        })
        .mount(&server)
        .await;
    let tokens = Arc::new(resumed(&http(), &server, signed_in_store(REFRESH)).await);

    for round in 1..=2 {
        let callers: Vec<_> = (0..10)
            .map(|_| {
                let tokens = Arc::clone(&tokens);
                tokio::spawn(async move { tokens.access_token().await })
            })
            .collect();
        for caller in callers {
            assert_eq!(
                caller.await.unwrap().unwrap(),
                format!("access-{round}-AAAA")
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), round);
        // The same again when all ten were refused and say so.
        for _ in 0..10 {
            tokens.invalidate().await;
        }
    }
}

#[tokio::test]
async fn a_new_refresh_token_replaces_the_old_and_is_used_next() {
    let server = MockServer::start().await;
    // Each renewal must present the token the last one issued.
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |request: &Request| {
            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let presented = form(request)
                .into_iter()
                .find(|(name, _)| name == "refresh_token")
                .unwrap()
                .1;
            if presented != format!("refresh-{}-RRRR", number - 1) {
                return refusal(400, "invalid_grant", "That refresh token was replaced.");
            }
            tokens(
                &format!("access-{number}-AAAA"),
                Some(&format!("refresh-{number}-RRRR")),
            )
        })
        .mount(&server)
        .await;
    let store = signed_in_store("refresh-0-RRRR");
    let tokens = resumed(&http(), &server, store.clone()).await;

    for number in 1..=3 {
        assert_eq!(
            tokens.access_token().await.unwrap(),
            format!("access-{number}-AAAA")
        );
        assert_eq!(
            store.get(NAME).unwrap().unwrap(),
            format!("refresh-{number}-RRRR")
        );
        tokens.invalidate().await;
    }
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_renewal_without_a_new_refresh_token_keeps_the_old() {
    let server = MockServer::start().await;
    numbered_tokens(&server, json!(3600)).await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(&http(), &server, store.clone()).await;

    tokens.access_token().await.unwrap();
    tokens.invalidate().await;
    tokens.access_token().await.unwrap();
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
    // And the same token again is not a new one worth another write.
    let renewals = requests(&server, "/token").await;
    assert_eq!(renewals.len(), 2);
    assert!(form(&renewals[1]).contains(&("refresh_token".to_owned(), REFRESH.to_owned())));
}

#[tokio::test(start_paused = true)]
async fn a_token_with_no_stated_lifetime_is_taken_to_be_short() {
    let server = MockServer::start().await;
    let count = numbered_tokens(&server, Value::Null).await;
    let tokens = resumed(&OAuthHttp::unhurried(), &server, signed_in_store(REFRESH)).await;

    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    // Five minutes are assumed, and the last of them is not trusted.
    advance(Duration::from_secs(239)).await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
    advance(Duration::from_secs(2)).await;
    assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_token_that_is_spent_on_arrival_is_used_once_and_not_kept() {
    for expires_in in [json!(0), json!(-3600), json!("0"), json!(30)] {
        let server = MockServer::start().await;
        let count = numbered_tokens(&server, expires_in.clone()).await;
        let tokens = resumed(&http(), &server, signed_in_store(REFRESH)).await;

        // Each call gets a token, and none loops waiting for a better one.
        assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
        assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
        assert_eq!(count.load(Ordering::SeqCst), 2, "{expires_in}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_token_is_never_believed_to_last_longer_than_a_day() {
    for expires_in in [json!(315_360_000_u64), json!(1e300), json!("99999999999")] {
        let server = MockServer::start().await;
        let count = numbered_tokens(&server, expires_in.clone()).await;
        let tokens = resumed(&OAuthHttp::unhurried(), &server, signed_in_store(REFRESH)).await;

        assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
        advance(Duration::from_secs(24 * 3600 - 61)).await;
        assert_eq!(tokens.access_token().await.unwrap(), "access-1-AAAA");
        advance(Duration::from_secs(2)).await;
        assert_eq!(tokens.access_token().await.unwrap(), "access-2-AAAA");
        assert_eq!(count.load(Ordering::SeqCst), 2, "{expires_in}");
    }
}

#[tokio::test]
async fn a_dead_refresh_token_means_signing_in_again_and_is_removed() {
    let server = MockServer::start().await;
    token_endpoint(
        &server,
        refusal(
            400,
            "invalid_grant",
            "AADSTS700082: The refresh token has expired due to inactivity.",
        ),
    )
    .await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(&http(), &server, store.clone()).await;

    let err = tokens.access_token().await.unwrap_err();
    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains(ACCOUNT), "{said}");
    assert!(said.contains("Sign in again"), "{said}");
    assert!(
        said.contains("AADSTS700082: The refresh token has expired"),
        "{said}"
    );
    assert_clean_error(&err, &[REFRESH]);
    assert_eq!(store.get(NAME).unwrap(), None);

    // From now on the answer is the same without asking the provider.
    let err = tokens.access_token().await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert_eq!(requests(&server, "/token").await.len(), 1);
    assert_clean(&tokens, &[REFRESH]);
}

#[tokio::test]
async fn a_refused_refresh_token_does_not_take_a_newer_one_with_it() {
    let server = MockServer::start().await;
    let store = signed_in_store(REFRESH);
    // While this renewal is in flight, another Quill process renews and
    // stores the token it was given.
    let other_process = store.clone();
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &Request| {
            other_process
                .set(NAME, "refresh-from-the-other-RRRR")
                .unwrap();
            refusal(400, "invalid_grant", "That refresh token was replaced.")
        })
        .mount(&server)
        .await;
    let tokens = resumed(&http(), &server, store.clone()).await;

    let err = tokens.access_token().await.unwrap_err();
    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert_eq!(
        store.get(NAME).unwrap().as_deref(),
        Some("refresh-from-the-other-RRRR")
    );
}

#[tokio::test]
async fn a_token_server_having_a_bad_day_is_tried_again_later() {
    for (answer, status) in [
        (
            ResponseTemplate::new(500).set_body_string("Internal Server Error"),
            "500",
        ),
        (refusal(503, "temporarily_unavailable", "Try again."), "503"),
        (
            ResponseTemplate::new(429).insert_header("Retry-After", "30"),
            "429",
        ),
        (refusal(400, "temporarily_unavailable", "Try again."), "400"),
        (
            ResponseTemplate::new(200).set_body_string("<html>Hotel Wi-Fi</html>"),
            "200",
        ),
        (
            ResponseTemplate::new(403).set_body_string("Forbidden by the proxy"),
            "403",
        ),
    ] {
        let server = MockServer::start().await;
        // The first answer is the trouble; after that the server is well.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(answer)
            .up_to_n_times(1)
            .mount(&server)
            .await;
        token_endpoint(&server, tokens(ACCESS, None)).await;
        let store = signed_in_store(REFRESH);
        let tokens = resumed(&http(), &server, store.clone()).await;

        let err = tokens.access_token().await.unwrap_err();
        let Error::Temporary(said) = &err else {
            panic!("{status}: {err:?}")
        };
        assert!(said.contains(status), "{said}");
        assert_clean_error(&err, &[REFRESH]);
        // Nothing was given up: the sign-in is still there, and works.
        assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
        assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    }
}

#[tokio::test]
async fn a_token_server_that_hangs_up_is_a_connection_problem() {
    // A server of the test's own that takes each connection and drops it.
    // (Not a mock server that was started and dropped: wiremock keeps those
    // and hands them to the next test that asks.)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hanging_up = tokio::spawn(async move {
        loop {
            drop(listener.accept().await);
        }
    });
    let unreachable = OAuthProvider {
        name: "test".to_owned(),
        authorize_url: format!("http://127.0.0.1:{port}/authorize"),
        token_url: format!("http://127.0.0.1:{port}/token"),
        device_url: None,
        scopes: Vec::new(),
        extra_authorize_params: Vec::new(),
        redirect_host: crate::oauth::RedirectHost::Ipv4,
    };
    let store = signed_in_store(REFRESH);
    let tokens = OAuthTokens::resume(
        &http(),
        &unreachable,
        &OAuthClient::public(CLIENT_ID),
        store.clone(),
        ACCOUNT,
    )
    .await
    .unwrap();

    let err = tokens.access_token().await.unwrap_err();
    let Error::Connection(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.starts_with("127.0.0.1: "), "{said}");
    assert_clean_error(&err, &[REFRESH]);
    // Nothing is wrong with the account, and nothing was given up.
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
    hanging_up.abort();
}

#[tokio::test]
async fn https_goes_through_quills_tls_and_a_failed_handshake_is_a_connection_problem() {
    // The mock server speaks plain HTTP, so a TLS handshake with it cannot
    // succeed. That it is attempted, and fails as a connection problem
    // instead of a panic for want of a TLS library, is the point.
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, None)).await;
    let https = OAuthProvider {
        token_url: format!("https://127.0.0.1:{}/token", server.address().port()),
        ..provider(&server)
    };
    let tokens = OAuthTokens::resume(
        &http(),
        &https,
        &OAuthClient::public(CLIENT_ID),
        signed_in_store(REFRESH),
        ACCOUNT,
    )
    .await
    .unwrap();

    let err = tokens.access_token().await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    assert_clean_error(&err, &[REFRESH]);
    // The form, with the refresh token in it, was never sent in the clear.
    assert!(requests(&server, "/token").await.is_empty());
}

#[tokio::test]
async fn a_refusal_that_is_not_about_the_refresh_token_keeps_it() {
    let server = MockServer::start().await;
    token_endpoint(
        &server,
        refusal(
            401,
            "invalid_client",
            "AADSTS700016: Application was not found.",
        ),
    )
    .await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(&http(), &server, store.clone()).await;

    let err = tokens.access_token().await.unwrap_err();
    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("invalid_client: AADSTS700016"), "{said}");
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
}

#[tokio::test]
async fn a_store_that_fails_is_reported_and_no_token_is_handed_out_on_its_back() {
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, Some("refresh-new-RRRR"))).await;

    // Reading fails: nothing can be resumed, and it is not called a
    // sign-in problem, because signing in again would not mend it.
    let locked = Arc::new(BrokenStore {
        refuse_get: true,
        ..BrokenStore::default()
    });
    let err = OAuthTokens::resume(
        &http(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        locked,
        ACCOUNT,
    )
    .await
    .unwrap_err();
    let Error::Backend(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("the keychain is locked"), "{said}");

    // Writing fails: the renewal that brought a new refresh token fails as
    // a whole, and the next call starts over instead of using a token whose
    // successor was lost.
    let read_only = Arc::new(BrokenStore {
        refuse_set: true,
        ..BrokenStore::default()
    });
    read_only.inner.set(NAME, REFRESH).unwrap();
    let tokens = resumed(&http(), &server, read_only.clone()).await;
    for expected_requests in 1..=2 {
        let err = tokens.access_token().await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
        assert_clean_error(&err, &[REFRESH, ACCESS, "refresh-new-RRRR"]);
        assert_eq!(requests(&server, "/token").await.len(), expected_requests);
    }
    assert_eq!(read_only.inner.get(NAME).unwrap().as_deref(), Some(REFRESH));
}

#[tokio::test]
async fn a_token_server_that_says_nothing_is_a_connection_problem() {
    let server = MockServer::start().await;
    token_endpoint(
        &server,
        tokens(ACCESS, None).set_delay(Duration::from_secs(5)),
    )
    .await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(
        &OAuthHttp::impatient(Duration::from_millis(200)),
        &server,
        store.clone(),
    )
    .await;

    let err = tokens.access_token().await.unwrap_err();
    let Error::Connection(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("timed out"), "{said}");
    assert_clean_error(&err, &[REFRESH]);
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
}

#[tokio::test]
async fn a_renewal_finishes_even_if_whoever_asked_stops_waiting() {
    let server = MockServer::start().await;
    token_endpoint(
        &server,
        tokens(ACCESS, Some("refresh-new-RRRR")).set_delay(Duration::from_millis(300)),
    )
    .await;
    let store = signed_in_store(REFRESH);
    let tokens = resumed(&http(), &server, store.clone()).await;

    // The caller gives up while the token server is still thinking.
    let gave_up = tokio::time::timeout(Duration::from_millis(50), tokens.access_token()).await;
    assert!(gave_up.is_err());
    // The answer is not lost with it: the new refresh token, which may be
    // the only one the provider will take from now on, reaches the store.
    for _ in 0..200 {
        if store.get(NAME).unwrap().as_deref() != Some(REFRESH) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        store.get(NAME).unwrap().as_deref(),
        Some("refresh-new-RRRR")
    );
    // And the access token it brought serves the next caller.
    assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    assert_eq!(requests(&server, "/token").await.len(), 1);
}

/// The spec's acceptance for M5: "tokens refresh unattended for a week".
///
/// A week of a provider like Microsoft: every access token lasts an hour,
/// every renewal brings a new refresh token and retires the old. The
/// account is used three times an hour. Nobody is asked anything: there is
/// no callback here that could ask.
#[tokio::test(start_paused = true)]
async fn a_week_of_hourly_expiries_is_one_renewal_each_and_nobody_is_asked() {
    const HOURS: usize = 7 * 24;
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |request: &Request| {
            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
            let presented = form(request)
                .into_iter()
                .find(|(name, _)| name == "refresh_token")
                .unwrap()
                .1;
            if presented != format!("refresh-{}-RRRR", number - 1) {
                return refusal(400, "invalid_grant", "That refresh token was replaced.");
            }
            tokens(
                &format!("access-{number}-AAAA"),
                Some(&format!("refresh-{number}-RRRR")),
            )
        })
        .mount(&server)
        .await;
    let store: Arc<MemorySecrets> = signed_in_store("refresh-0-RRRR");
    let tokens = resumed(&OAuthHttp::unhurried(), &server, store.clone()).await;
    let started = tokio::time::Instant::now();

    for hour in 1..=HOURS {
        // On the hour the last token has run out, and a new one is got.
        let token = tokens.access_token().await.unwrap();
        assert_eq!(token, format!("access-{hour}-AAAA"));
        assert_eq!(count.load(Ordering::SeqCst), hour);
        // It serves for the rest of the hour.
        for _ in 0..2 {
            advance(Duration::from_secs(20 * 60)).await;
            assert_eq!(tokens.access_token().await.unwrap(), token);
        }
        advance(Duration::from_secs(20 * 60)).await;
    }

    assert_eq!(started.elapsed(), Duration::from_secs(7 * 24 * 3600));
    assert_eq!(count.load(Ordering::SeqCst), HOURS);
    assert_eq!(
        store.get(NAME).unwrap().unwrap(),
        format!("refresh-{HOURS}-RRRR")
    );
}
