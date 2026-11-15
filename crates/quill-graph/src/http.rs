//! The HTTP side of Graph: one request at a time, signed with the account's
//! token, with the waiting and the second tries the service asks for.
//!
//! Three rules are kept here so that nothing above has to remember them.
//! The token goes to the configured service and nowhere else: links the
//! service hands out are followed only when they lead back to it, and no
//! redirect is followed at all. The token is never part of an error. And
//! every request has a time limit, so a silent server cannot hang a sync.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use quill_core::{Error, Result, TokenSource};
use quill_net::Trust;
use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Asked of every request: with it a message keeps its id when it is moved
/// to another folder.
const IMMUTABLE_IDS: &str = "IdType=\"ImmutableId\"";

/// How long the service may stay silent in the middle of a request before
/// the connection is given up as lost.
const SILENCE: Duration = Duration::from_secs(60);

/// How long one request may take from start to finish, a large message on
/// a slow line included.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The longest wait the service can ask for and get. Asked to wait longer,
/// the request is given up as `Temporary` and whoever called decides when
/// to come back.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// How many times a request is sent again after the service said to wait.
const WAITS: u32 = 3;

/// The most of the service's own words put in an error.
const WORDS_LIMIT: usize = 300;

/// Which servers a client may be pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The real service: HTTPS and nothing else.
    Https,
    /// A stand-in on this machine, for tests. Plain HTTP is allowed.
    Loopback,
}

pub(crate) struct Client {
    http: reqwest::Client,
    base: Url,
    tokens: Arc<dyn TokenSource>,
    timeout: Duration,
}

impl Client {
    pub fn new(base_url: &str, reach: Reach, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        let base = Url::parse(base_url.trim_end_matches('/'))
            .ok()
            .filter(|url| !url.cannot_be_a_base() && url.host_str().is_some())
            .ok_or_else(|| {
                Error::Backend(format!("\"{base_url}\" is not the address of a server"))
            })?;
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::Backend(
                "the Microsoft Graph address must be a plain one: no user name, query or fragment"
                    .to_owned(),
            ));
        }
        match reach {
            Reach::Https if base.scheme() != "https" => {
                return Err(Error::Insecure(format!(
                    "Microsoft Graph is reached over HTTPS only, and {base} is not"
                )));
            }
            Reach::Loopback if !is_loopback(&base) => {
                return Err(Error::Insecure(format!(
                    "{base} is not on this machine; only a test server on this machine may be reached this way"
                )));
            }
            _ => {}
        }

        let (tls, _presented) = quill_net::tls::client_config(&Trust::default())?;
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(Arc::unwrap_or_clone(tls))
            .https_only(reach == Reach::Https)
            // A redirect would carry the request, and perhaps the token, to
            // a server nobody chose. The mail endpoints never redirect.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(quill_net::PATIENCE)
            .read_timeout(SILENCE);
        if reach == Reach::Loopback {
            // A proxy from the environment must not come between a test and
            // its server.
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|err| Error::Backend(format!("cannot set up HTTPS: {err}")))?;
        Ok(Self {
            http,
            base,
            tokens,
            timeout: REQUEST_TIMEOUT,
        })
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn base(&self) -> &str {
        self.base.as_str()
    }

    /// The address of something under the base. Each part of the path is
    /// escaped, so an id may hold any character; the query is written as
    /// the service documents it and is the caller's to get right.
    pub fn url(&self, path: &[&str], query: &str) -> Url {
        let mut url = self.base.clone();
        // `new` made sure the base can have a path.
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.pop_if_empty().extend(path);
        }
        if !query.is_empty() {
            url.set_query(Some(query));
        }
        url
    }

    /// A link the service handed out, if it leads back to the service.
    /// Links come from response bodies and from cursors kept in the store,
    /// and the token must not follow one to some other server.
    pub fn link(&self, link: &str) -> Option<Url> {
        let url = Url::parse(link).ok()?;
        (url.origin() == self.base.origin()).then_some(url)
    }

    /// Sends `request` and returns the service's answer, whatever it is.
    ///
    /// Two answers are dealt with here and never returned. A refused token
    /// is forgotten and the request sent once more with a new one; a second
    /// refusal is `Error::Auth`. And when the service says it is busy, the
    /// request waits as long as it was asked to and goes again, a few times
    /// over, before giving up with `Error::Temporary`. Asked to wait longer
    /// than a minute, it gives up at once: going again sooner than asked
    /// only makes the service's patience shorter.
    pub async fn send(&self, request: &Request) -> Result<Reply> {
        let mut renewed = false;
        let mut waits = 0;
        loop {
            // Asked for every time: the source knows when a token is about
            // to run out, and this does not.
            let token = self.tokens.access_token().await?;
            let reply = self.attempt(request, &token).await?;
            match reply.status {
                StatusCode::UNAUTHORIZED if !renewed => {
                    self.tokens.invalidate().await;
                    renewed = true;
                }
                StatusCode::UNAUTHORIZED => {
                    return Err(Error::Auth(format!(
                        "Microsoft Graph would not accept the account's sign-in: {}",
                        reply.words()
                    )));
                }
                // Both say the request was turned away untouched, so even
                // a message being sent can go again without going twice.
                // Other failures of the service do not say that, and are
                // left to the caller.
                StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                    // Without a time from the service: two seconds, then
                    // four, then eight.
                    let wait = reply
                        .wait
                        .unwrap_or_else(|| Duration::from_secs(2 << waits.min(4)));
                    if waits == WAITS || wait > LONGEST_WAIT {
                        return Err(Error::Temporary(format!(
                            "Microsoft Graph is busy and asked to wait {} seconds: {}",
                            wait.as_secs(),
                            reply.words()
                        )));
                    }
                    waits += 1;
                    tokio::time::sleep(wait).await;
                }
                _ => return Ok(reply),
            }
        }
    }

    async fn attempt(&self, request: &Request, token: &str) -> Result<Reply> {
        let mut prefer = IMMUTABLE_IDS.to_owned();
        if let Some(more) = &request.prefer {
            prefer.push_str(", ");
            prefer.push_str(more);
        }
        let builder = self
            .http
            .request(request.method.clone(), request.url.clone())
            .timeout(self.timeout)
            .bearer_auth(token)
            .header("Prefer", prefer);
        let builder = match &request.body {
            Body::None => builder,
            Body::Json(value) => builder.json(value),
            // Graph takes a whole message as base64 text, and tells it from
            // JSON by this content type.
            Body::Mime(encoded) => builder
                .header(CONTENT_TYPE, "text/plain")
                .body(encoded.clone()),
        };
        let response = builder.send().await.map_err(unreachable)?;
        let status = response.status();
        let wait = retry_after(response.headers());
        let body = response.bytes().await.map_err(unreachable)?;
        let complaint = if status.is_success() {
            Complaint::default()
        } else {
            Complaint::read(&body, token)
        };
        Ok(Reply {
            status,
            wait,
            body,
            complaint,
        })
    }
}

/// Whether `url` names this machine by address. A name such as `localhost`
/// is not taken on trust: what it resolves to is somebody else's decision.
fn is_loopback(url: &Url) -> bool {
    url.host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .ok()
        })
        .is_some_and(|address| address.is_loopback())
}

/// How long a `Retry-After` header says to wait, when it says it in
/// seconds. The other form, a date, is not one Graph sends.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

/// A request that could not be made or finished. The address is left out:
/// it is long, and no help to the person reading.
fn unreachable(err: reqwest::Error) -> Error {
    if err.is_timeout() {
        return Error::Connection("Microsoft Graph did not answer in time".to_owned());
    }
    let refused_here = err.is_builder();
    let err = err.without_url();
    let mut text = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    if refused_here {
        // Never sent: the address was not one this client will go to.
        Error::Backend(text)
    } else {
        Error::Connection(text)
    }
}

pub(crate) struct Request {
    method: Method,
    url: Url,
    body: Body,
    /// What to prefer beyond immutable ids, such as a page size.
    prefer: Option<String>,
}

enum Body {
    None,
    Json(serde_json::Value),
    /// A whole message, already base64. Kept as `Bytes` so a second try
    /// does not copy it.
    Mime(Bytes),
}

impl Request {
    fn new(method: Method, url: Url, body: Body) -> Self {
        Self {
            method,
            url,
            body,
            prefer: None,
        }
    }

    pub fn get(url: Url) -> Self {
        Self::new(Method::GET, url, Body::None)
    }

    pub fn delete(url: Url) -> Self {
        Self::new(Method::DELETE, url, Body::None)
    }

    pub fn post(url: Url, body: serde_json::Value) -> Self {
        Self::new(Method::POST, url, Body::Json(body))
    }

    pub fn patch(url: Url, body: serde_json::Value) -> Self {
        Self::new(Method::PATCH, url, Body::Json(body))
    }

    /// Posts a complete RFC 5322 message, byte for byte.
    pub fn post_message(url: Url, raw: &[u8]) -> Self {
        Self::new(Method::POST, url, Body::Mime(BASE64.encode(raw).into()))
    }

    pub fn prefer(mut self, preference: String) -> Self {
        self.prefer = Some(preference);
        self
    }
}

/// The service's answer to a request: any status but the ones `send` deals
/// with itself.
pub(crate) struct Reply {
    status: StatusCode,
    wait: Option<Duration>,
    body: Bytes,
    complaint: Complaint,
}

impl Reply {
    /// The thing asked about is not there. Whether that is an error depends
    /// on what was being done: deleting a message that is already gone is
    /// not one.
    pub fn missing(&self) -> bool {
        self.status == StatusCode::NOT_FOUND
    }

    /// The place a folder sync was to resume from is no longer kept by the
    /// service, and the folder has to be read again from the start.
    pub fn expired(&self) -> bool {
        self.status == StatusCode::GONE
            || (self.status.is_client_error()
                && self
                    .complaint
                    .code
                    .eq_ignore_ascii_case("SyncStateNotFound"))
    }

    /// The body, when the request worked; otherwise why `doing` did not.
    pub fn bytes(self, doing: &str) -> Result<Bytes> {
        if self.status.is_success() {
            Ok(self.body)
        } else {
            Err(self.error(doing))
        }
    }

    /// The body read as JSON, when the request worked.
    pub fn json<T: DeserializeOwned>(self, doing: &str) -> Result<T> {
        let body = self.bytes(doing)?;
        // The parser's message names a line and a column, never the text.
        serde_json::from_slice(&body).map_err(|err| {
            Error::Backend(format!(
                "Microsoft Graph's answer could not be read while trying to {doing}: {err}"
            ))
        })
    }

    pub fn done(self, doing: &str) -> Result<()> {
        self.bytes(doing).map(drop)
    }

    /// Why `doing` failed, as the kind of error the sync engine acts on.
    pub fn error(&self, doing: &str) -> Error {
        let text = format!("Microsoft Graph could not {doing}: {}", self.words());
        match self.status {
            StatusCode::NOT_FOUND => Error::NotFound(text),
            // The service's own trouble, not this request's.
            status if status.is_server_error() => Error::Temporary(text),
            // "Forbidden" is here too, and is not `Error::Auth`: the token
            // was accepted, and what it may not do can be this one message
            // as well as the whole mailbox. The sync engine holds an
            // account's whole queue back on `Auth`, which one message that
            // may not be touched must not be able to do.
            _ => Error::Backend(text),
        }
    }

    /// Whether a message handed over for sending was taken.
    ///
    /// Anything the service holds against the message or its sender is a
    /// refusal: the message stays in the Outbox with the service's words,
    /// and is not sent again by itself.
    ///
    /// The service's own failure is another matter. It does not say
    /// whether the message went before things went wrong, and the one
    /// error that makes no promise about that is `Error::Connection`.
    pub fn accepted(self) -> Result<()> {
        if self.status.is_success() {
            Ok(())
        } else if self.status.is_server_error() {
            Err(Error::Connection(format!(
                "Microsoft Graph failed while it was given the message, and did not say whether it was sent: {}",
                self.words()
            )))
        } else {
            Err(Error::Refused(format!("Microsoft Graph: {}", self.words())))
        }
    }

    /// What the service said went wrong, or the bare status when it sent
    /// no words.
    fn words(&self) -> String {
        let Complaint { code, message } = &self.complaint;
        match (message.is_empty(), code.is_empty()) {
            (false, false) => format!("{message} ({code})"),
            (false, true) => message.clone(),
            (true, false) => format!("{code} (HTTP {})", self.status.as_u16()),
            (true, true) => format!("HTTP {}", self.status),
        }
    }
}

/// Graph's error body: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Default)]
struct Complaint {
    code: String,
    message: String,
}

impl Complaint {
    /// Reads the error out of `body`. A body that is not Graph's error JSON
    /// (a proxy's HTML page, nothing at all) gives an empty complaint.
    ///
    /// The words go into errors people read, so they are kept short and
    /// free of control characters, and the token is struck out should a
    /// server ever echo it.
    fn read(body: &[u8], token: &str) -> Self {
        #[derive(Deserialize)]
        struct Envelope {
            error: Option<Inner>,
        }
        #[derive(Deserialize)]
        struct Inner {
            code: Option<String>,
            message: Option<String>,
        }
        let inner = serde_json::from_slice::<Envelope>(body)
            .ok()
            .and_then(|envelope| envelope.error);
        let tidy = |text: Option<String>| {
            let text = text.unwrap_or_default();
            let text = if token.is_empty() {
                text
            } else {
                text.replace(token, "[access token]")
            };
            text.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(WORDS_LIMIT)
                .collect::<String>()
                .trim()
                .to_owned()
        };
        match inner {
            Some(inner) => Self {
                code: tidy(inner.code),
                message: tidy(inner.message),
            },
            None => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    struct NoTokens;

    #[async_trait]
    impl TokenSource for NoTokens {
        async fn access_token(&self) -> Result<String> {
            Err(Error::Auth("no token in this test".to_owned()))
        }

        async fn invalidate(&self) {}
    }

    fn client(base: &str, reach: Reach) -> Result<Client> {
        Client::new(base, reach, Arc::new(NoTokens))
    }

    #[test]
    fn the_real_service_is_https_or_nothing() {
        assert!(client("https://graph.microsoft.com/v1.0", Reach::Https).is_ok());
        for plain in ["http://graph.microsoft.com/v1.0", "http://127.0.0.1:8080"] {
            assert!(
                matches!(client(plain, Reach::Https), Err(Error::Insecure(_))),
                "{plain}"
            );
        }
    }

    #[test]
    fn only_this_machine_is_reached_as_a_test_server() {
        assert!(client("http://127.0.0.1:8080/v1.0", Reach::Loopback).is_ok());
        assert!(client("http://[::1]:8080", Reach::Loopback).is_ok());
        for elsewhere in [
            "http://graph.microsoft.com/v1.0",
            "https://graph.microsoft.com/v1.0",
            "http://localhost:8080",
            "http://192.0.2.7:8080",
        ] {
            assert!(
                matches!(client(elsewhere, Reach::Loopback), Err(Error::Insecure(_))),
                "{elsewhere}"
            );
        }
    }

    #[test]
    fn what_is_not_a_plain_address_is_refused() {
        for bad in [
            "",
            "graph.microsoft.com",
            "mailto:robin@example.com",
            "https://robin:secret@graph.example.com/v1.0",
            "https://graph.example.com/v1.0?x=1",
        ] {
            assert!(
                matches!(client(bad, Reach::Https), Err(Error::Backend(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn ids_are_escaped_in_a_path_and_the_query_is_left_as_written() {
        let client = client("https://graph.example.com/v1.0/", Reach::Https).unwrap();
        assert_eq!(
            client
                .url(&["me", "messages", "AAMk/a+b=", "$value"], "")
                .as_str(),
            "https://graph.example.com/v1.0/me/messages/AAMk%2Fa+b=/$value"
        );
        assert_eq!(
            client
                .url(&["me", "mailFolders"], "$select=id,displayName&$top=100")
                .as_str(),
            "https://graph.example.com/v1.0/me/mailFolders?$select=id,displayName&$top=100"
        );
    }

    #[test]
    fn a_link_is_followed_only_back_to_the_service() {
        let client = client("https://graph.example.com/v1.0", Reach::Https).unwrap();
        assert!(
            client
                .link(
                    "https://graph.example.com/v1.0/me/mailFolders/x/messages/delta?$skiptoken=abc"
                )
                .is_some()
        );
        for elsewhere in [
            "https://graph.example.net/v1.0/me/messages",
            "http://graph.example.com/v1.0/me/messages",
            "https://graph.example.com:8443/v1.0/me/messages",
            "https://graph.example.com.example.net/v1.0",
            "not a link",
            "",
        ] {
            assert!(client.link(elsewhere).is_none(), "{elsewhere}");
        }
    }

    #[test]
    fn retry_after_is_read_in_seconds() {
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(RETRY_AFTER, value.parse().unwrap());
            retry_after(&headers)
        };
        assert_eq!(with("2"), Some(Duration::from_secs(2)));
        assert_eq!(with(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(with("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(with("-1"), None);
        assert_eq!(retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn a_complaint_is_short_clean_and_never_holds_the_token() {
        let body = serde_json::json!({
            "error": {
                "code": "InvalidAuthenticationToken",
                "message": format!("Bad token test-access-token-1\u{1b}[31m {}", "x".repeat(1000)),
            }
        })
        .to_string();
        let complaint = Complaint::read(body.as_bytes(), "test-access-token-1");
        assert_eq!(complaint.code, "InvalidAuthenticationToken");
        assert!(
            complaint
                .message
                .starts_with("Bad token [access token] [31m x")
        );
        assert!(!complaint.message.contains("test-access-token-1"));
        assert!(complaint.message.chars().count() <= WORDS_LIMIT);

        // Not Graph's JSON: nothing is made up from it.
        let complaint = Complaint::read(b"<html>Bad Gateway</html>", "test-access-token-1");
        assert!(complaint.code.is_empty() && complaint.message.is_empty());
    }
}
