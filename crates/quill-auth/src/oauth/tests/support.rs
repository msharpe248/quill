//! What the tests share: a provider on a mock server, a user with a
//! browser, and ways to look at what was sent.

use std::fmt::Debug;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quill_core::Result;
use quill_net::Trust;
use reqwest::Url;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::oauth::{OAuthHttp, OAuthProvider, RedirectHost};
use crate::{AuthError, MemorySecrets, SecretStore};

pub(crate) const ACCOUNT: &str = "robin@example.com";
/// The name the account's refresh token is stored under.
pub(crate) const NAME: &str = "oauth:test:robin@example.com";
pub(crate) const CLIENT_ID: &str = "11111111-2222-3333-4444-555555555555";
pub(crate) const CLIENT_SECRET: &str = "GOCSPX-client-secret-SSSS";
pub(crate) const CODE: &str = "0.AXEA-authorization-code-CCCC";
pub(crate) const ACCESS: &str = "eyJ0eXAi.access-token-AAAA";
pub(crate) const REFRESH: &str = "1//0g-refresh-token-RRRR";
pub(crate) const DEVICE_CODE: &str = "GAQABAAEAAAD-device-code-DDDD";
pub(crate) const USER_CODE: &str = "HJKL-MNPQ";

/// The client as the app builds it, timeouts and all.
pub(crate) fn http() -> OAuthHttp {
    OAuthHttp::new(&Trust::default()).unwrap()
}

/// A provider whose every address is on `server`.
pub(crate) fn provider(server: &MockServer) -> OAuthProvider {
    OAuthProvider {
        name: "test".to_owned(),
        authorize_url: format!("{}/authorize", server.uri()),
        token_url: format!("{}/token", server.uri()),
        device_url: Some(format!("{}/device", server.uri())),
        scopes: vec![
            "https://mail.example.com/".to_owned(),
            "offline_access".to_owned(),
        ],
        extra_authorize_params: vec![("prompt".to_owned(), "consent".to_owned())],
        redirect_host: RedirectHost::Ipv4,
    }
}

pub(crate) fn store() -> Arc<MemorySecrets> {
    Arc::new(MemorySecrets::default())
}

/// A store that already holds a refresh token for the account.
pub(crate) fn signed_in_store(refresh: &str) -> Arc<MemorySecrets> {
    let store = store();
    store.set(NAME, refresh).unwrap();
    store
}

/// A store that fails where it is told to.
#[derive(Debug, Default)]
pub(crate) struct BrokenStore {
    pub inner: MemorySecrets,
    pub refuse_get: bool,
    pub refuse_set: bool,
}

impl SecretStore for BrokenStore {
    fn get(&self, name: &str) -> std::result::Result<Option<String>, AuthError> {
        if self.refuse_get {
            return Err(AuthError::Keychain("the keychain is locked".to_owned()));
        }
        self.inner.get(name)
    }

    fn set(&self, name: &str, secret: &str) -> std::result::Result<(), AuthError> {
        if self.refuse_set {
            return Err(AuthError::Keychain("the keychain is locked".to_owned()));
        }
        self.inner.set(name, secret)
    }

    fn delete(&self, name: &str) -> std::result::Result<(), AuthError> {
        self.inner.delete(name)
    }
}

/// A token endpoint's success.
pub(crate) fn tokens(access: &str, refresh: Option<&str>) -> ResponseTemplate {
    let mut body = json!({
        "token_type": "Bearer",
        "access_token": access,
        "expires_in": 3600,
        "scope": "https://mail.example.com/",
    });
    if let Some(refresh) = refresh {
        body["refresh_token"] = json!(refresh);
    }
    ResponseTemplate::new(200).set_body_json(body)
}

/// An OAuth error answer.
pub(crate) fn refusal(status: u16, code: &str, description: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error": code, "error_description": description}))
}

/// Mounts a token endpoint that always answers `response`.
pub(crate) async fn token_endpoint(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(response)
        .mount(server)
        .await;
}

/// Mounts an authorize endpoint that behaves as a provider does once the
/// user has agreed: it sends the browser back to the redirect address with
/// a code and the state it was given.
pub(crate) async fn consenting_provider(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/authorize"))
        .respond_with(|request: &Request| {
            let mut back = Url::parse(&query(&request.url, "redirect_uri")).unwrap();
            back.query_pairs_mut()
                .append_pair("code", CODE)
                .append_pair("state", &query(&request.url, "state"));
            ResponseTemplate::new(302).insert_header("Location", back.as_str())
        })
        .mount(server)
        .await;
}

/// The one value of a query parameter.
pub(crate) fn query(url: &Url, name: &str) -> String {
    let mut values = url.query_pairs().filter(|(key, _)| key == name);
    let value = values
        .next()
        .unwrap_or_else(|| panic!("no {name} in the query"))
        .1
        .into_owned();
    assert!(values.next().is_none(), "{name} was sent twice");
    value
}

/// The requests the server got for `path`, in order.
pub(crate) async fn requests(server: &MockServer, path: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == path)
        .collect()
}

/// A form body's fields, sorted, after checking it was sent as a form.
pub(crate) fn form(request: &Request) -> Vec<(String, String)> {
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(
        request.headers.get("content-type").unwrap(),
        "application/x-www-form-urlencoded"
    );
    assert!(
        request.url.query().is_none(),
        "nothing belongs in the query"
    );
    let body = std::str::from_utf8(&request.body).unwrap();
    let mut fields: Vec<(String, String)> = Url::parse(&format!("http://127.0.0.1/?{body}"))
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    fields.sort();
    fields
}

/// The same shape for what a test expects.
pub(crate) fn fields(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = expected
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    fields.sort();
    fields
}

/// Fails if printing `what` for debugging would show any of `secrets`.
pub(crate) fn assert_clean(what: &dyn Debug, secrets: &[&str]) {
    let shown = format!("{what:?} {what:#?}");
    for secret in secrets {
        assert!(!secret.is_empty());
        assert!(!shown.contains(secret), "{secret} is shown in {shown}");
    }
}

/// The same for an error, whose message is as public as its `Debug`.
pub(crate) fn assert_clean_error(err: &quill_core::Error, secrets: &[&str]) {
    assert_clean(err, secrets);
    assert_clean(&err.to_string(), secrets);
}

/// Sends `request` to this computer's `port` and returns the status and
/// the whole response. `None` if nobody is listening, or the connection is
/// closed without an HTTP answer.
pub(crate) async fn raw_request(port: u16, request: &[u8]) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.ok()?;
    // A server that has heard enough may close before all of it is sent.
    let _ = stream.write_all(request).await;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response.strip_prefix("HTTP/1.1 ")?.get(..3)?.parse().ok()?;
    Some((status, response))
}

/// Whether `port` can be bound: whether whatever listened there has gone.
pub(crate) async fn same_port_is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await.is_ok()
}

/// Fetches `url` as a browser would, without following redirects.
///
/// It will not go anywhere but this computer: a test that tried to reach a
/// real provider fails here.
pub(crate) async fn visit(url: &Url) -> Option<(u16, String)> {
    let host = url.host_str().unwrap();
    assert!(
        url.scheme() == "http" && (host == "127.0.0.1" || host == "localhost"),
        "a test tried to leave this computer for {host}"
    );
    let port = url.port().unwrap();
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    raw_request(
        port,
        format!("GET {target} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
}

/// What a browser does with the address it is given: fetches it, and if
/// sent elsewhere, goes there. Returns the last page it was shown.
pub(crate) async fn follow(url: Url) -> Option<(u16, String)> {
    let (status, response) = visit(&url).await?;
    if status != 302 {
        return Some((status, response));
    }
    let location = response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("location")
                .then(|| value.trim().to_owned())
        })
        .expect("a redirect says where to");
    visit(&Url::parse(&location).unwrap()).await
}

/// What the user's browser showed at the end, once it has.
pub(crate) type Shown = Arc<Mutex<Option<Option<(u16, String)>>>>;

/// An `open` callback that plays the user: it is handed the address, and
/// does `act` with it on a task of its own, as a browser is its own
/// program. What `act` returns can be read from the second value.
pub(crate) fn user<Act, Fut>(act: Act) -> (impl FnOnce(&str) -> Result<()> + Send + 'static, Shown)
where
    Act: FnOnce(Url) -> Fut + Send + 'static,
    Fut: Future<Output = Option<(u16, String)>> + Send + 'static,
{
    let shown = Shown::default();
    let slot = Arc::clone(&shown);
    let open = move |address: &str| {
        let url = Url::parse(address).unwrap();
        tokio::spawn(async move {
            let page = act(url).await;
            *slot.lock().unwrap() = Some(page);
        });
        Ok(())
    };
    (open, shown)
}

/// Waits, in real time, for `shown` to be filled in.
pub(crate) async fn page(shown: &Shown) -> Option<(u16, String)> {
    for _ in 0..500 {
        if let Some(page) = shown.lock().unwrap().take() {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the browser never finished");
}

/// The address the browser would be sent back to, with `query` in place of
/// what a provider would have put there.
pub(crate) fn back_to_quill(authorize: &Url, query_string: &str) -> Url {
    let mut back = Url::parse(&query(authorize, "redirect_uri")).unwrap();
    back.set_query(Some(query_string));
    back
}
