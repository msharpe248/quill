//! Nothing that is printed, and nothing that is reported, contains a
//! secret: not even when the provider quotes one back.
//!
//! The other tests check the errors they provoke as they go. These sweep
//! the public types, and then play a provider that repeats everything it
//! was sent in its refusals.

use quill_core::{Error, TokenSource};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request};

use super::support::*;
use crate::SecretStore;
use crate::oauth::{
    BrowserSignIn, DeviceSignIn, OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens, RedirectHost,
    Secret, SignInMethod, sign_in,
};

const SECRETS: &[&str] = &[CLIENT_SECRET, CODE, ACCESS, REFRESH, DEVICE_CODE];

#[tokio::test]
async fn no_public_type_prints_a_secret() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let client = OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET);
    let http = http();
    let provider = provider(&server);

    assert_clean(&Secret::new(REFRESH), SECRETS);
    assert_clean(&client, SECRETS);
    assert_clean(&http, SECRETS);
    assert_clean(&provider, SECRETS);
    assert_clean(&OAuthProvider::microsoft(), SECRETS);
    assert_clean(&OAuthProvider::google(), SECRETS);
    assert_clean(&RedirectHost::Localhost, SECRETS);
    assert_clean(
        &SignInMethod::DeviceCode(DeviceSignIn::new(|_| {})),
        SECRETS,
    );

    let (open, _) = user(follow);
    let method = SignInMethod::Browser(BrowserSignIn::new(open));
    assert_clean(&method, SECRETS);
    let store = store();
    let tokens = sign_in(&http, &provider, &client, method, store.clone(), ACCOUNT)
        .await
        .unwrap();

    // Signed in, with an access token in hand and every secret of the flow
    // having passed through.
    let verifier = form(&requests(&server, "/token").await[0])
        .into_iter()
        .find(|(name, _)| name == "code_verifier")
        .unwrap()
        .1;
    assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    assert_clean(&tokens, SECRETS);
    assert_clean(&tokens, &[&verifier]);
    // The store is the one place the refresh token is, and it is the
    // caller's to guard.
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
}

/// Mounts a token endpoint that refuses everything and says exactly what
/// it was sent while doing so.
async fn indiscreet_provider(server: &MockServer, error: &'static str) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(move |request: &Request| {
            let sent: Vec<String> = form(request)
                .into_iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            refusal(400, error, &format!("Refused: {}", sent.join(", ")))
        })
        .mount(server)
        .await;
}

/// Every secret the token endpoint was sent, read back from its requests.
async fn secrets_sent(server: &MockServer) -> Vec<String> {
    let mut sent = Vec::new();
    for request in requests(server, "/token").await {
        for (name, value) in form(&request) {
            if matches!(
                name.as_str(),
                "code" | "code_verifier" | "refresh_token" | "device_code" | "client_secret"
            ) {
                sent.push(value);
            }
        }
    }
    assert!(!sent.is_empty());
    sent
}

fn assert_none_of(err: &Error, secrets: &[String]) {
    let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
    assert_clean_error(err, &secrets);
    // The provider's words are still there, around the holes.
    assert!(err.to_string().contains("Refused: "), "{err}");
}

#[tokio::test]
async fn a_provider_that_quotes_the_code_and_verifier_back_is_not_repeated() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    indiscreet_provider(&server, "invalid_grant").await;

    let (open, _) = user(follow);
    let err = sign_in(
        &http(),
        &provider(&server),
        &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
        SignInMethod::Browser(BrowserSignIn::new(open)),
        store(),
        ACCOUNT,
    )
    .await
    .unwrap_err();

    let sent = secrets_sent(&server).await;
    // The code, the verifier and the client secret all went, and none came
    // back out.
    assert_eq!(sent.len(), 3);
    assert_none_of(&err, &sent);
}

#[tokio::test]
async fn a_provider_that_quotes_the_refresh_token_back_is_not_repeated() {
    for error in ["invalid_grant", "invalid_client", "temporarily_unavailable"] {
        let server = MockServer::start().await;
        indiscreet_provider(&server, error).await;
        let tokens = OAuthTokens::resume(
            &http(),
            &provider(&server),
            &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
            signed_in_store(REFRESH),
            ACCOUNT,
        )
        .await
        .unwrap();

        let err = tokens.access_token().await.unwrap_err();

        let sent = secrets_sent(&server).await;
        assert_eq!(sent.len(), 2);
        assert_none_of(&err, &sent);
        assert_clean(&tokens, SECRETS);
    }
}

#[tokio::test(start_paused = true)]
async fn a_provider_that_quotes_the_device_code_back_is_not_repeated() {
    for error in [
        "access_denied",
        "expired_token",
        "invalid_grant",
        "server_error",
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/device"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "device_code": DEVICE_CODE,
                    "user_code": USER_CODE,
                    "verification_uri": "https://login.example.com/device",
                    "expires_in": 8,
                    "interval": 5,
                })),
            )
            .mount(&server)
            .await;
        indiscreet_provider(&server, error).await;

        let err = sign_in(
            &OAuthHttp::unhurried(),
            &provider(&server),
            &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
            SignInMethod::DeviceCode(DeviceSignIn::new(|prompt| {
                assert_clean(prompt, &[DEVICE_CODE, CLIENT_SECRET]);
            })),
            store(),
            ACCOUNT,
        )
        .await
        .unwrap_err();

        let sent = secrets_sent(&server).await;
        assert_eq!(sent.len(), 2);
        let secrets: Vec<&str> = sent.iter().map(String::as_str).collect();
        assert_clean_error(&err, &secrets);
    }
}
