//! Signing in by a code typed on another device.
//!
//! Time is paused in these tests: the waits between questions pass at
//! once, and how much time the flow believes has gone by can be checked.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quill_core::{Error, TokenSource};
use serde_json::json;
use tokio::time::Instant;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::support::*;
use crate::oauth::{
    DevicePrompt, DeviceSignIn, OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens, SignInMethod,
    sign_in,
};
use crate::{MemorySecrets, SecretStore};

/// Mounts a device endpoint that issues the code, good for `expires_in`
/// seconds and to be asked about every `interval`.
async fn device_endpoint(server: &MockServer, expires_in: u64, interval: u64) {
    Mock::given(method("POST"))
        .and(path("/device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": DEVICE_CODE,
            "user_code": USER_CODE,
            "verification_uri": "https://login.example.com/device",
            "expires_in": expires_in,
            "interval": interval,
            "message": format!("To sign in, open https://login.example.com/device and enter {USER_CODE}."),
        })))
        .mount(server)
        .await;
}

/// Mounts a token endpoint that gives `answers` in turn, and the last of
/// them for ever after. Returns the count of questions it has answered.
async fn token_answers(server: &MockServer, answers: Vec<ResponseTemplate>) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &Request| {
            let asked = counter.fetch_add(1, Ordering::SeqCst);
            answers[asked.min(answers.len() - 1)].clone()
        })
        .mount(server)
        .await;
    count
}

fn pending() -> ResponseTemplate {
    refusal(
        400,
        "authorization_pending",
        "The user has not signed in yet.",
    )
}

/// What the user was shown, once they have been.
type Prompted = Arc<Mutex<Option<DevicePrompt>>>;

async fn sign_in_by_code(
    server: &MockServer,
    client: &OAuthClient,
    store: &Arc<MemorySecrets>,
) -> (quill_core::Result<OAuthTokens>, Prompted) {
    let prompted = Prompted::default();
    let slot = Arc::clone(&prompted);
    let result = sign_in(
        &OAuthHttp::unhurried(),
        &provider(server),
        client,
        SignInMethod::DeviceCode(DeviceSignIn::new(move |prompt| {
            *slot.lock().unwrap() = Some(prompt.clone());
        })),
        store.clone(),
        ACCOUNT,
    )
    .await;
    (result, prompted)
}

#[tokio::test(start_paused = true)]
async fn waiting_slowing_down_and_then_signed_in() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    let asked = token_answers(
        &server,
        vec![
            pending(),
            refusal(400, "slow_down", "You are asking too often."),
            pending(),
            tokens(ACCESS, Some(REFRESH)),
        ],
    )
    .await;
    let store = store();
    let started = Instant::now();

    let (result, prompted) =
        sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store).await;
    let tokens = result.unwrap();

    // Asked at 5 s (wait), at 10 s (slow down), then every 10 s: at 20 s
    // (wait) and at 30 s (yes).
    assert_eq!(asked.load(Ordering::SeqCst), 4);
    assert_eq!(started.elapsed(), Duration::from_secs(30));

    // The user was shown where to go and what to type, and not the code
    // that is Quill's own.
    let prompt = prompted.lock().unwrap().take().unwrap();
    assert_eq!(prompt.verification_uri, "https://login.example.com/device");
    assert_eq!(prompt.user_code, USER_CODE);
    assert_eq!(prompt.expires_in, Duration::from_secs(900));
    assert!(prompt.message.as_deref().unwrap().contains(USER_CODE));
    assert_clean(&prompt, &[DEVICE_CODE]);

    // The code was asked for with exactly this.
    let begun = requests(&server, "/device").await;
    assert_eq!(begun.len(), 1);
    assert_eq!(
        form(&begun[0]),
        fields(&[
            ("client_id", CLIENT_ID),
            ("scope", "https://mail.example.com/ offline_access"),
        ])
    );
    // And every question to the token server was exactly this.
    let questions = requests(&server, "/token").await;
    assert_eq!(questions.len(), 4);
    for question in &questions {
        assert_eq!(
            form(question),
            fields(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", DEVICE_CODE),
                ("client_id", CLIENT_ID),
            ])
        );
    }

    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
    assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    assert_eq!(asked.load(Ordering::SeqCst), 4);
    assert_clean(&tokens, &[ACCESS, REFRESH, DEVICE_CODE]);
}

#[tokio::test(start_paused = true)]
async fn a_client_secret_goes_with_both_requests_when_there_is_one() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    token_answers(&server, vec![tokens(ACCESS, Some(REFRESH))]).await;

    let (result, _) = sign_in_by_code(
        &server,
        &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
        &store(),
    )
    .await;
    result.unwrap();

    let secret = ("client_secret".to_owned(), CLIENT_SECRET.to_owned());
    assert!(form(&requests(&server, "/device").await[0]).contains(&secret));
    assert!(form(&requests(&server, "/token").await[0]).contains(&secret));
}

#[tokio::test(start_paused = true)]
async fn a_code_the_provider_says_has_expired_ends_the_sign_in() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    let asked = token_answers(
        &server,
        vec![
            pending(),
            refusal(400, "expired_token", "The device code has expired."),
        ],
    )
    .await;
    let store = store();

    let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store).await;
    let err = result.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("the code expired"), "{said}");
    assert_clean_error(&err, &[DEVICE_CODE]);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert_eq!(store.get(NAME).unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn a_code_is_given_up_on_when_its_time_is_over_whatever_the_provider_says() {
    let server = MockServer::start().await;
    // Good for 12 seconds, asked about every 5: at 5 s and at 10 s. A
    // third question would come after the code had run out.
    device_endpoint(&server, 12, 5).await;
    let asked = token_answers(&server, vec![pending()]).await;
    let started = Instant::now();

    let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store()).await;
    let err = result.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("the code expired"), "{said}");
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert_eq!(started.elapsed(), Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn a_refusal_on_the_other_device_ends_the_sign_in() {
    // The RFC's word for it, and Microsoft's.
    for code in ["access_denied", "authorization_declined"] {
        let server = MockServer::start().await;
        device_endpoint(&server, 900, 5).await;
        let asked = token_answers(
            &server,
            vec![pending(), refusal(400, code, "The user declined.")],
        )
        .await;
        let store = store();

        let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store).await;
        let err = result.unwrap_err();

        let Error::Auth(said) = &err else {
            panic!("{err:?}")
        };
        assert!(said.contains("refused on the other device"), "{said}");
        assert!(
            said.contains(&format!("{code}: The user declined.")),
            "{said}"
        );
        assert_clean_error(&err, &[DEVICE_CODE]);
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(store.get(NAME).unwrap(), None);
    }
}

#[tokio::test(start_paused = true)]
async fn trouble_reaching_the_token_server_does_not_end_the_wait() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    let asked = token_answers(
        &server,
        vec![
            ResponseTemplate::new(503),
            pending(),
            ResponseTemplate::new(429),
            tokens(ACCESS, Some(REFRESH)),
        ],
    )
    .await;

    let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store()).await;

    assert_eq!(result.unwrap().access_token().await.unwrap(), ACCESS);
    assert_eq!(asked.load(Ordering::SeqCst), 4);
}

#[tokio::test(start_paused = true)]
async fn trouble_that_outlasts_the_code_is_what_is_reported() {
    let server = MockServer::start().await;
    device_endpoint(&server, 20, 5).await;
    let asked = token_answers(&server, vec![pending(), ResponseTemplate::new(503)]).await;

    let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store()).await;
    let err = result.unwrap_err();

    assert!(matches!(err, Error::Temporary(_)), "{err:?}");
    assert_clean_error(&err, &[DEVICE_CODE]);
    assert_eq!(asked.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn any_other_refusal_ends_the_sign_in_in_the_providers_words() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    let asked = token_answers(
        &server,
        vec![refusal(
            400,
            "invalid_grant",
            &format!("AADSTS70000: The code {DEVICE_CODE} is not valid."),
        )],
    )
    .await;

    let (result, _) = sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store()).await;
    let err = result.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(
        said.contains("invalid_grant: AADSTS70000: The code … is not valid."),
        "{said}"
    );
    assert_clean_error(&err, &[DEVICE_CODE]);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_provider_that_will_not_issue_a_code_shows_the_user_nothing() {
    for (answer, auth) in [
        (
            refusal(
                400,
                "unauthorized_client",
                "AADSTS7000218: The application is not a public client.",
            ),
            true,
        ),
        (ResponseTemplate::new(500), false),
        (
            ResponseTemplate::new(200).set_body_json(json!({"user_code": USER_CODE})),
            false,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/device"))
            .respond_with(answer)
            .mount(&server)
            .await;

        let (result, prompted) =
            sign_in_by_code(&server, &OAuthClient::public(CLIENT_ID), &store()).await;
        let err = result.unwrap_err();

        if auth {
            let Error::Auth(said) = &err else {
                panic!("{err:?}")
            };
            assert!(
                said.contains("unauthorized_client: AADSTS7000218"),
                "{said}"
            );
        } else {
            assert!(matches!(err, Error::Temporary(_)), "{err:?}");
        }
        assert!(prompted.lock().unwrap().is_none());
        assert!(requests(&server, "/token").await.is_empty());
    }
}

#[tokio::test]
async fn a_provider_without_a_device_endpoint_says_so_before_asking_anyone() {
    let server = MockServer::start().await;
    let shown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shown);
    let err = sign_in(
        &http(),
        &OAuthProvider {
            device_url: None,
            ..provider(&server)
        },
        &OAuthClient::public(CLIENT_ID),
        SignInMethod::DeviceCode(DeviceSignIn::new(move |_| {
            flag.store(true, Ordering::SeqCst)
        })),
        store(),
        ACCOUNT,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    assert!(!shown.load(Ordering::SeqCst));
    assert!(server.received_requests().await.unwrap().is_empty());
    // Google is such a provider: it issues device codes for a short list
    // of permissions, and mail is not on it.
    assert_eq!(OAuthProvider::google().device_url, None);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_sign_in_stops_the_asking() {
    let server = MockServer::start().await;
    device_endpoint(&server, 900, 5).await;
    // Each question is announced, so the test can wait for one without a
    // timer of its own: with time paused, a timer pending during a real
    // request would jump the clock.
    let (question, mut questions) = tokio::sync::mpsc::unbounded_channel();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |_: &Request| {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = question.send(());
            pending()
        })
        .mount(&server)
        .await;
    let store = store();

    let provider = provider(&server);
    let task_store = store.clone();
    let signing_in = tokio::spawn(async move {
        sign_in(
            &OAuthHttp::unhurried(),
            &provider,
            &OAuthClient::public(CLIENT_ID),
            SignInMethod::DeviceCode(DeviceSignIn::new(|_| {})),
            task_store,
            ACCOUNT,
        )
        .await
    });
    // Three questions go unanswered with a yes, then the user gives up.
    for _ in 0..3 {
        questions.recv().await.unwrap();
    }
    signing_in.abort();
    assert!(signing_in.await.unwrap_err().is_cancelled());
    assert_eq!(asked.load(Ordering::SeqCst), 3);

    // Time goes on, and nobody is asking any more.
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    assert_eq!(store.get(NAME).unwrap(), None);
}
