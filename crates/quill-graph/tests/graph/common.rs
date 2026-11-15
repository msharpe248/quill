//! A stand-in for Microsoft Graph on this machine, and a token source that
//! counts.
//!
//! Nothing here or in the tests reaches Microsoft or any other host. The
//! backend is built with `GraphBackend::loopback`, which refuses an
//! address that is not this machine's, and every test ends with
//! [`Rig::check`]: each request the backend made took one token from the
//! source, so if the stand-in received as many requests as tokens were
//! handed out, none went anywhere else. A test that forgets the check
//! fails when its rig is dropped.
//!
//! Everyone in the mail is invented and lives at example.com.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use quill_core::{Result, TokenSource};
use quill_graph::{GraphBackend, GraphConfig};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockBuilder, MockServer, Request, ResponseTemplate};

/// What a folder sync asks for about each message.
pub const SUMMARY: &str = "id,subject,from,toRecipients,ccRecipients,receivedDateTime,\
                           sentDateTime,internetMessageId,isRead,flag,hasAttachments,isDraft,conversationId";

/// What every request must say about ids.
pub const IMMUTABLE_IDS: &str = "IdType=\"ImmutableId\"";

/// Hands out `test-access-token-1` until told it was refused, then
/// `test-access-token-2`, and so on.
#[derive(Default)]
pub struct Tokens {
    pub handed_out: AtomicUsize,
    pub refused: AtomicUsize,
}

#[async_trait]
impl TokenSource for Tokens {
    async fn access_token(&self) -> Result<String> {
        self.handed_out.fetch_add(1, Ordering::SeqCst);
        Ok(format!(
            "test-access-token-{}",
            self.refused.load(Ordering::SeqCst) + 1
        ))
    }

    async fn invalidate(&self) {
        self.refused.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct Rig {
    pub server: MockServer,
    pub tokens: Arc<Tokens>,
    pub graph: GraphBackend,
    page_size: usize,
    checked: AtomicBool,
}

impl Rig {
    pub async fn start() -> Self {
        Self::with_page_size(50).await
    }

    pub async fn with_page_size(page_size: usize) -> Self {
        let server = MockServer::start().await;
        let tokens = Arc::new(Tokens::default());
        let graph = backend(&server, page_size, &tokens);
        Self {
            server,
            tokens,
            graph,
            page_size,
            checked: AtomicBool::new(false),
        }
    }

    /// Another backend for the same stand-in and token source, to be set
    /// up differently from the rig's own.
    pub fn backend(&self) -> GraphBackend {
        backend(&self.server, self.page_size, &self.tokens)
    }

    /// The address of `path_and_query` on the stand-in, as Graph would
    /// write it in a link.
    pub fn link(&self, path_and_query: &str) -> String {
        format!("{}/v1.0{path_and_query}", self.server.uri())
    }

    /// Starts a mock for `verb` on `path` under the service's base.
    pub fn on(&self, verb: &str, path_under_base: &str) -> MockBuilder {
        Mock::given(method(verb)).and(path(format!("/v1.0{path_under_base}")))
    }

    /// Everything the stand-in has received, oldest first.
    pub async fn requests(&self) -> Vec<Request> {
        self.server.received_requests().await.unwrap_or_default()
    }

    /// Checks that every request the backend made came here, carrying a
    /// token from the source and the wish for immutable ids. Returns the
    /// requests, as `VERB path?query` lines, for the test to look at.
    pub async fn check(&self) -> Vec<String> {
        self.check_but(0).await
    }

    /// As [`Rig::check`], for a test in which `lost` requests were meant
    /// never to arrive.
    pub async fn check_but(&self, lost: usize) -> Vec<String> {
        self.checked.store(true, Ordering::SeqCst);
        let requests = self.requests().await;
        assert_eq!(
            requests.len() + lost,
            self.tokens.handed_out.load(Ordering::SeqCst),
            "a request went somewhere other than the test's own server"
        );
        for request in &requests {
            let authorization = header(request, "authorization");
            assert!(
                authorization.starts_with("Bearer test-access-token-"),
                "{} {} was not signed with a token from the source: {authorization:?}",
                request.method,
                request.url
            );
            let prefer = header(request, "prefer");
            assert!(
                prefer.split(',').any(|wish| wish.trim() == IMMUTABLE_IDS),
                "{} {} did not ask for immutable ids: {prefer:?}",
                request.method,
                request.url
            );
        }
        requests.iter().map(line).collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.checked.load(Ordering::SeqCst),
                "the test did not check where its requests went"
            );
        }
    }
}

fn backend(server: &MockServer, page_size: usize, tokens: &Arc<Tokens>) -> GraphBackend {
    GraphBackend::loopback(
        GraphConfig {
            base_url: format!("{}/v1.0", server.uri()),
            page_size,
        },
        tokens.clone(),
    )
    .unwrap()
}

/// A request as one line: `GET /v1.0/me/messages/m1?$select=id`.
pub fn line(request: &Request) -> String {
    match request.url.query() {
        Some(query) => format!("{} {}?{query}", request.method, request.url.path()),
        None => format!("{} {}", request.method, request.url.path()),
    }
}

/// The one value of a request header, or nothing.
pub fn header(request: &Request, name: &str) -> String {
    let mut values = request.headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.to_str().unwrap_or_default().to_owned(),
        (None, _) => String::new(),
        (Some(_), Some(_)) => panic!("{name} was sent more than once"),
    }
}

pub fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

/// An error as Graph words one.
pub fn refusal(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {
            "code": code,
            "message": message,
            "innerError": {
                "date": "2026-11-03T09:20:00",
                "request-id": "5f1c4c6e-0b7a-4d43-9a3e-000000000001",
                "client-request-id": "5f1c4c6e-0b7a-4d43-9a3e-000000000001"
            }
        }
    }))
}

/// A message in a folder listing, as Graph sends one when asked for
/// [`SUMMARY`].
pub fn message(id: &str, subject: &str, from: (&str, &str), received: &str, read: bool) -> Value {
    json!({
        "@odata.type": "#microsoft.graph.message",
        "@odata.etag": format!("W/\"etag-{id}\""),
        "id": id,
        "subject": subject,
        "from": {"emailAddress": {"name": from.0, "address": from.1}},
        "toRecipients": [
            {"emailAddress": {"name": "Robin Okafor", "address": "robin@example.com"}}
        ],
        "ccRecipients": [],
        "receivedDateTime": received,
        "sentDateTime": received,
        "internetMessageId": format!("<{id}@example.com>"),
        "isRead": read,
        "flag": {"flagStatus": "notFlagged"},
        "hasAttachments": false,
        "isDraft": false
    })
}

/// A message of two megabytes with every byte value in it, so that
/// anything done to it on the way shows.
pub fn large_message() -> Vec<u8> {
    let mut raw = b"From: Robin Okafor <robin@example.com>\r\n\
        To: Ana Ruiz <ana.ruiz@example.com>\r\n\
        Subject: Survey data\r\n\
        Message-ID: <survey.data@example.com>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: application/octet-stream\r\n\
        Content-Transfer-Encoding: binary\r\n\
        \r\n"
        .to_vec();
    raw.extend((0..2 * 1024 * 1024u32).map(|i| (i.wrapping_mul(31) % 256) as u8));
    raw
}

/// Waits, in real time, until the stand-in has received `count` requests.
pub async fn received(server: &MockServer, count: usize) {
    for _ in 0..1000 {
        if server.received_requests().await.unwrap_or_default().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the server never received {count} requests");
}
