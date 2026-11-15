//! Signing in through the browser, from the address the user is sent to,
//! to the tokens that come of it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quill_core::{Error, TokenSource};
use reqwest::Url;
use serde_json::json;
use wiremock::{MockServer, ResponseTemplate};

use super::support::*;
use crate::oauth::pkce::challenge_for;
use crate::oauth::{
    BrowserSignIn, OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens, RedirectHost, SignInMethod,
    sign_in,
};
use crate::{MemorySecrets, SecretStore};

/// Signs in through `open`, with a public client.
async fn sign_in_with(
    server: &MockServer,
    store: &Arc<MemorySecrets>,
    open: impl FnOnce(&str) -> quill_core::Result<()> + Send + 'static,
) -> quill_core::Result<OAuthTokens> {
    sign_in(
        &http(),
        &provider(server),
        &OAuthClient::public(CLIENT_ID),
        SignInMethod::Browser(BrowserSignIn::new(open)),
        store.clone(),
        ACCOUNT,
    )
    .await
}

#[tokio::test]
async fn the_browser_flow_signs_in_and_keeps_only_the_refresh_token() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();

    let (open, shown) = user(follow);
    let tokens = sign_in_with(&server, &store, open).await.unwrap();

    // The user was told to go back to Quill.
    let (status, response) = page(&shown).await.unwrap();
    assert_eq!(status, 200);
    assert!(response.contains("Return to Quill"), "{response}");

    // What the browser was sent to the provider with.
    let authorize = &requests(&server, "/authorize").await[0].url;
    let mut asked: Vec<(String, String)> = authorize
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    asked.sort();
    let redirect_uri = query(authorize, "redirect_uri");
    let state = query(authorize, "state");
    let challenge = query(authorize, "code_challenge");
    assert_eq!(
        asked,
        fields(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect_uri),
            ("scope", "https://mail.example.com/ offline_access"),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "consent"),
        ])
    );
    assert!(state.len() >= 43, "a state of {} characters", state.len());
    let port = Url::parse(&redirect_uri).unwrap().port().unwrap();
    assert_eq!(redirect_uri, format!("http://127.0.0.1:{port}"));

    // What the code was exchanged with: exactly this, as a form, once.
    let exchanges = requests(&server, "/token").await;
    assert_eq!(exchanges.len(), 1);
    let exchange = form(&exchanges[0]);
    let verifier = exchange
        .iter()
        .find(|(name, _)| name == "code_verifier")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert_eq!(
        exchange,
        fields(&[
            ("grant_type", "authorization_code"),
            ("code", CODE),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
            ("client_id", CLIENT_ID),
        ])
    );
    // RFC 7636: the challenge the browser carried is the S256 of the
    // verifier the token server was given, and the verifier is 43 to 128
    // unreserved characters.
    assert_eq!(challenge, challenge_for(&verifier));
    assert!((43..=128).contains(&verifier.len()));
    assert!(
        verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
    );
    assert_ne!(verifier, state);

    // The refresh token is in the store, under the account's name, and the
    // access token is not there.
    assert_eq!(tokens.secret_name(), NAME);
    assert_eq!(store.get(NAME).unwrap().as_deref(), Some(REFRESH));
    // The access token is ready, with no further request.
    assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    assert_eq!(requests(&server, "/token").await.len(), 1);
    // And the listener is gone.
    assert!(same_port_is_free(port).await);
    assert_clean(&tokens, &[ACCESS, REFRESH, CODE, &verifier]);
}

#[tokio::test]
async fn a_client_secret_a_hint_and_a_localhost_redirect_go_where_they_belong() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();
    let provider = OAuthProvider {
        redirect_host: RedirectHost::Localhost,
        ..provider(&server)
    };

    let (open, _) = user(follow);
    let mut request = BrowserSignIn::new(open);
    request.login_hint = Some(ACCOUNT.to_owned());
    sign_in(
        &http(),
        &provider,
        &OAuthClient::with_secret(CLIENT_ID, CLIENT_SECRET),
        SignInMethod::Browser(request),
        store.clone(),
        ACCOUNT,
    )
    .await
    .unwrap();

    let authorize = &requests(&server, "/authorize").await[0].url;
    let redirect_uri = query(authorize, "redirect_uri");
    assert!(
        redirect_uri.starts_with("http://localhost:"),
        "{redirect_uri}"
    );
    assert_eq!(query(authorize, "login_hint"), ACCOUNT);
    // The secret is for the token server. The browser never sees it.
    assert!(!authorize.as_str().contains(CLIENT_SECRET));

    let exchange = form(&requests(&server, "/token").await[0]);
    assert!(exchange.contains(&("client_secret".to_owned(), CLIENT_SECRET.to_owned())));
    assert!(exchange.contains(&("redirect_uri".to_owned(), redirect_uri)));
    assert_eq!(exchange.len(), 6);
}

#[tokio::test]
async fn a_port_can_be_chosen_for_the_redirect() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    // A port that was free a moment ago.
    let port = {
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        probe.local_addr().unwrap().port()
    };

    let (open, _) = user(follow);
    let mut request = BrowserSignIn::new(open);
    request.port = Some(port);
    sign_in(
        &http(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        SignInMethod::Browser(request),
        store(),
        ACCOUNT,
    )
    .await
    .unwrap();

    let authorize = &requests(&server, "/authorize").await[0].url;
    assert_eq!(
        query(authorize, "redirect_uri"),
        format!("http://127.0.0.1:{port}")
    );
}

#[tokio::test]
async fn a_redirect_with_another_state_is_refused_and_nothing_is_exchanged() {
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();

    // Someone who knows the port but not the state: another program, or a
    // web page in the user's browser.
    let (open, shown) = user(|authorize: Url| async move {
        visit(&back_to_quill(
            &authorize,
            &format!("code={CODE}&state=not-the-state-that-was-sent"),
        ))
        .await
    });
    let err = sign_in_with(&server, &store, open).await.unwrap_err();

    assert!(matches!(err, Error::Auth(_)), "{err:?}");
    assert!(
        err.to_string().contains("does not belong to this sign-in"),
        "{err}"
    );
    assert_clean_error(&err, &[CODE, "not-the-state-that-was-sent"]);
    let (status, response) = page(&shown).await.unwrap();
    assert_eq!(status, 400);
    assert!(response.contains("Sign-in did not finish"), "{response}");
    assert!(requests(&server, "/token").await.is_empty());
    assert_eq!(store.get(NAME).unwrap(), None);
}

#[tokio::test]
async fn a_refusal_in_the_redirect_is_reported_in_the_providers_words() {
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();

    let (open, shown) = user(|authorize: Url| async move {
        let state = query(&authorize, "state");
        visit(&back_to_quill(
            &authorize,
            &format!(
                "error=access_denied&error_description=The+user+denied+the+request.&state={state}"
            ),
        ))
        .await
    });
    let err = sign_in_with(&server, &store, open).await.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(
        said.contains("access_denied: The user denied the request."),
        "{said}"
    );
    let (status, response) = page(&shown).await.unwrap();
    assert_eq!(status, 400);
    assert!(!response.contains("access_denied"), "{response}");
    assert!(requests(&server, "/token").await.is_empty());
    assert_eq!(store.get(NAME).unwrap(), None);
}

#[tokio::test]
async fn a_request_for_another_path_does_not_end_the_sign_in() {
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();

    let (open, shown) = user(|authorize: Url| async move {
        let state = query(&authorize, "state");
        let mut elsewhere = back_to_quill(&authorize, &format!("code={CODE}&state={state}"));
        elsewhere.set_path("/callback");
        let (status, _) = visit(&elsewhere).await.unwrap();
        assert_eq!(status, 404);
        // Nothing was exchanged for that. Then the real redirect arrives,
        // and after it nobody is listening for another.
        let real = back_to_quill(&authorize, &format!("code={CODE}&state={state}"));
        let first = visit(&real).await;
        assert_eq!(visit(&real).await, None, "a second redirect was served");
        first
    });
    let tokens = sign_in_with(&server, &store, open).await.unwrap();

    assert_eq!(page(&shown).await.unwrap().0, 200);
    assert_eq!(tokens.access_token().await.unwrap(), ACCESS);
    assert_eq!(requests(&server, "/token").await.len(), 1);
}

#[tokio::test]
async fn a_code_the_provider_will_not_redeem_is_a_failed_sign_in() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(
        &server,
        refusal(
            400,
            "invalid_grant",
            &format!("AADSTS70008: The code {CODE} has expired."),
        ),
    )
    .await;
    let store = store();

    let (open, _) = user(follow);
    let err = sign_in_with(&server, &store, open).await.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(
        said.contains("invalid_grant: AADSTS70008: The code … has expired."),
        "{said}"
    );
    assert_clean_error(&err, &[CODE]);
    assert_eq!(store.get(NAME).unwrap(), None);
}

#[tokio::test]
async fn a_sign_in_that_yields_no_refresh_token_is_not_kept() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, None)).await;
    let store = store();

    let (open, _) = user(follow);
    let err = sign_in_with(&server, &store, open).await.unwrap_err();

    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("no refresh token"), "{said}");
    assert_clean_error(&err, &[ACCESS, CODE]);
    assert_eq!(store.get(NAME).unwrap(), None);
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_stored_is_not_a_sign_in() {
    let server = MockServer::start().await;
    consenting_provider(&server).await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let locked = Arc::new(BrokenStore {
        refuse_set: true,
        ..BrokenStore::default()
    });

    let (open, _) = user(follow);
    let err = sign_in(
        &http(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        SignInMethod::Browser(BrowserSignIn::new(open)),
        locked,
        ACCOUNT,
    )
    .await
    .unwrap_err();

    let Error::Backend(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("the keychain is locked"), "{said}");
    assert_clean_error(&err, &[ACCESS, REFRESH, CODE]);
}

#[tokio::test]
async fn an_unreadable_answer_to_the_exchange_is_not_a_sign_in() {
    for answer in [
        ResponseTemplate::new(200).set_body_string("<html>Welcome to the hotel Wi-Fi</html>"),
        ResponseTemplate::new(200).set_body_json(json!({"token_type": "Bearer"})),
        ResponseTemplate::new(200)
            .set_body_json(json!({"access_token": ACCESS, "token_type": "mac"})),
        ResponseTemplate::new(503).set_body_string("Service Unavailable"),
        ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1:9/elsewhere"),
    ] {
        let server = MockServer::start().await;
        consenting_provider(&server).await;
        token_endpoint(&server, answer).await;
        let store = store();

        let (open, _) = user(follow);
        let err = sign_in_with(&server, &store, open).await.unwrap_err();

        assert!(matches!(err, Error::Temporary(_)), "{err:?}");
        assert_clean_error(&err, &[ACCESS, CODE]);
        assert_eq!(store.get(NAME).unwrap(), None);
        // A redirect from the token server is not followed.
        assert_eq!(requests(&server, "/token").await.len(), 1);
    }
}

#[tokio::test]
async fn a_browser_that_cannot_be_opened_ends_the_sign_in_and_frees_the_port() {
    let server = MockServer::start().await;
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let err = sign_in_with(&server, &store(), move |address| {
        *slot.lock().unwrap() = Some(Url::parse(address).unwrap());
        Err(Error::Unsupported("there is no browser here".to_owned()))
    })
    .await
    .unwrap_err();

    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    let authorize = seen.lock().unwrap().take().unwrap();
    let port = Url::parse(&query(&authorize, "redirect_uri"))
        .unwrap()
        .port()
        .unwrap();
    assert!(same_port_is_free(port).await);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn nobody_coming_back_ends_the_sign_in_and_frees_the_port() {
    let server = MockServer::start().await;
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let mut request = BrowserSignIn::new(move |address| {
        *slot.lock().unwrap() = Some(Url::parse(address).unwrap());
        Ok(())
    });
    request.wait = Duration::from_secs(90);
    let started = tokio::time::Instant::now();
    let err = sign_in(
        &OAuthHttp::unhurried(),
        &provider(&server),
        &OAuthClient::public(CLIENT_ID),
        SignInMethod::Browser(request),
        store(),
        ACCOUNT,
    )
    .await
    .unwrap_err();

    assert_eq!(started.elapsed(), Duration::from_secs(90));
    let Error::Auth(said) = &err else {
        panic!("{err:?}")
    };
    assert!(said.contains("within 90 seconds"), "{said}");
    let authorize = seen.lock().unwrap().take().unwrap();
    let port = Url::parse(&query(&authorize, "redirect_uri"))
        .unwrap()
        .port()
        .unwrap();
    assert!(same_port_is_free(port).await);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn dropping_the_sign_in_cancels_it_and_frees_the_port() {
    let server = MockServer::start().await;
    token_endpoint(&server, tokens(ACCESS, Some(REFRESH))).await;
    let store = store();
    let (opened, mut address) = tokio::sync::mpsc::unbounded_channel();

    let provider = provider(&server);
    let task_store = store.clone();
    let signing_in = tokio::spawn(async move {
        sign_in(
            &http(),
            &provider,
            &OAuthClient::public(CLIENT_ID),
            SignInMethod::Browser(BrowserSignIn::new(move |address| {
                opened.send(Url::parse(address).unwrap()).unwrap();
                Ok(())
            })),
            task_store,
            ACCOUNT,
        )
        .await
    });
    let authorize = address.recv().await.unwrap();
    let port = Url::parse(&query(&authorize, "redirect_uri"))
        .unwrap()
        .port()
        .unwrap();
    assert!(!same_port_is_free(port).await, "the listener should be up");

    signing_in.abort();
    assert!(signing_in.await.unwrap_err().is_cancelled());

    assert!(same_port_is_free(port).await);
    // A redirect that arrives late finds nobody, and nothing comes of it.
    let state = query(&authorize, "state");
    assert_eq!(
        visit(&back_to_quill(
            &authorize,
            &format!("code={CODE}&state={state}")
        ))
        .await,
        None
    );
    assert!(requests(&server, "/token").await.is_empty());
    assert_eq!(store.get(NAME).unwrap(), None);
}

/// A provider that is not on this computer and is never contacted: the
/// tests that use it expect to be refused before anything is sent.
fn elsewhere() -> OAuthProvider {
    OAuthProvider {
        name: "test".to_owned(),
        authorize_url: "https://login.example.com/authorize".to_owned(),
        token_url: "https://login.example.com/token".to_owned(),
        device_url: None,
        scopes: Vec::new(),
        extra_authorize_params: Vec::new(),
        redirect_host: RedirectHost::Ipv4,
    }
}

#[tokio::test]
async fn nothing_is_asked_of_a_provider_that_is_not_https() {
    let opened = Arc::new(AtomicBool::new(false));
    for (change, expect_insecure) in [
        (
            Box::new(|provider: &mut OAuthProvider| {
                provider.token_url = "http://login.example.com/token".to_owned();
            }) as Box<dyn Fn(&mut OAuthProvider)>,
            true,
        ),
        (
            Box::new(|provider: &mut OAuthProvider| {
                provider.authorize_url = "http://login.example.com/authorize".to_owned();
            }),
            true,
        ),
        (
            Box::new(|provider: &mut OAuthProvider| {
                provider.device_url = Some("http://login.example.com/device".to_owned());
            }),
            true,
        ),
        (
            Box::new(|provider: &mut OAuthProvider| {
                provider
                    .extra_authorize_params
                    .push(("state".to_owned(), "fixed".to_owned()));
            }),
            false,
        ),
    ] {
        let mut provider = elsewhere();
        change(&mut provider);
        let flag = Arc::clone(&opened);
        let err = sign_in(
            &http(),
            &provider,
            &OAuthClient::public(CLIENT_ID),
            SignInMethod::Browser(BrowserSignIn::new(move |_| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })),
            store(),
            ACCOUNT,
        )
        .await
        .unwrap_err();
        if expect_insecure {
            assert!(matches!(err, Error::Insecure(_)), "{err:?}");
        } else {
            assert!(matches!(err, Error::Backend(_)), "{err:?}");
        }
    }
    // No client id, or no account name: also found before anyone is asked.
    for (client, account) in [("", ACCOUNT), (CLIENT_ID, " ")] {
        let flag = Arc::clone(&opened);
        let err = sign_in(
            &http(),
            &elsewhere(),
            &OAuthClient::public(client),
            SignInMethod::Browser(BrowserSignIn::new(move |_| {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })),
            store(),
            account,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
    }
    assert!(!opened.load(Ordering::SeqCst), "the browser was opened");
}

#[tokio::test]
async fn a_sign_in_can_be_run_on_another_task() {
    fn assert_send<T: Send>(_: &T) {}
    let server = MockServer::start().await;
    let (http, provider, client) = (http(), provider(&server), OAuthClient::public(CLIENT_ID));
    let signing_in = sign_in(
        &http,
        &provider,
        &client,
        SignInMethod::Browser(BrowserSignIn::new(|_| Ok(()))),
        store(),
        ACCOUNT,
    );
    assert_send(&signing_in);
    drop(signing_in);
}
