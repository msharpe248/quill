//! The listener the browser is sent back to.
//!
//! After the user signs in, the provider redirects their browser to an
//! address on this computer, with the authorization code in the query. This
//! is the small web server that receives that one request. It lives for one
//! sign-in: it is bound just before the browser is opened and closed as
//! soon as the redirect has arrived, or the wait is over, or the sign-in is
//! dropped.
//!
//! It is reachable by any program on this computer and, through the
//! browser, by any web page that guesses its port. So it listens on the
//! loopback address only, reads a bounded amount, answers one path, says
//! nothing that came from the request, and believes a request only if it
//! carries the state that was sent out.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use quill_core::{Error, Result};
use reqwest::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

use super::pkce::StateCheck;
use super::secret::{Secret, words};

/// The most that is read of a request: its first line and its headers. The
/// first line carries the code, which for Microsoft can pass a kilobyte,
/// and a browser sends `localhost` every cookie any local web server ever
/// set, so the usual request is a few kilobytes. Past this it is not a
/// redirect.
const MAX_REQUEST: usize = 16 * 1024;

/// How long one connection may take to send its request or take its
/// answer. Browsers open connections they never use; those are not waited
/// on for long, and never hold up the others.
const CONNECTION_PATIENCE: Duration = Duration::from_secs(10);

/// How many connections are attended to at once. A browser uses one or
/// two.
const MAX_CONNECTIONS: usize = 8;

/// The path the redirect comes to. The redirect address has no path, so
/// this is the root.
const CALLBACK_PATH: &str = "/";

const DONE_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>Quill</title></head>\n<body>\n<h1>Return to Quill</h1>\n<p>Quill has what it needs from the browser. You can close this window.</p>\n</body>\n</html>\n";

const FAILED_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>Quill</title></head>\n<body>\n<h1>Sign-in did not finish</h1>\n<p>Return to Quill to see what went wrong. You can close this window.</p>\n</body>\n</html>\n";

const NOTHING_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>Quill</title></head>\n<body>\n<p>There is nothing here.</p>\n</body>\n</html>\n";

/// A bound listener, waiting to be told what to expect.
#[derive(Debug)]
pub(crate) struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    /// Binds `127.0.0.1` on `port`, or on a port the system picks.
    pub async fn bind(port: Option<u16>) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port.unwrap_or(0)))
            .await
            .map_err(|err| {
                Error::Backend(match port {
                    Some(port) => {
                        format!("cannot listen on port {port} for the browser's answer: {err}")
                    }
                    None => format!("cannot listen for the browser's answer: {err}"),
                })
            })?;
        let port = listener
            .local_addr()
            .map_err(|err| {
                Error::Backend(format!("cannot listen for the browser's answer: {err}"))
            })?
            .port();
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Waits for the redirect and returns the authorization code it
    /// carries.
    ///
    /// The first request for the callback path that carries anything of a
    /// redirect settles the matter, for better or worse: if its state is
    /// not `state`, or the provider reports an error in it, that is the
    /// outcome. Requests for anything else are refused and waited past.
    /// The listener is closed when this returns, and also if it is dropped
    /// before it does.
    pub async fn redirect(self, state: StateCheck, wait: Duration) -> Result<Secret> {
        let state = Arc::new(state);
        let mut connections = JoinSet::new();
        let give_up = sleep(wait);
        tokio::pin!(give_up);
        loop {
            tokio::select! {
                () = &mut give_up => {
                    return Err(Error::Auth(format!(
                        "nobody finished signing in in the browser within {}",
                        minutes(wait)
                    )));
                }
                accepted = self.listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                    match accepted {
                        // The listener is bound to the loopback address, so
                        // nothing else should arrive. This is the same rule
                        // said twice, in case the binding ever changes.
                        Ok((stream, peer)) if peer.ip().is_loopback() => {
                            connections.spawn(serve(stream, Arc::clone(&state)));
                        }
                        Ok(_) => {}
                        // Out of file descriptors, most likely. Give the
                        // system a moment instead of spinning.
                        Err(_) => sleep(Duration::from_millis(50)).await,
                    }
                }
                Some(served) = connections.join_next() => {
                    if let Ok(Some(outcome)) = served {
                        return outcome;
                    }
                }
            }
        }
    }
}

fn minutes(wait: Duration) -> String {
    match wait.as_secs() {
        seconds if seconds < 120 => format!("{seconds} seconds"),
        seconds => format!("{} minutes", seconds / 60),
    }
}

/// Attends to one connection. `Some` if it was the redirect, with what
/// came of it; `None` if it was anything else.
async fn serve(mut stream: TcpStream, state: Arc<StateCheck>) -> Option<Result<Secret>> {
    let request = timeout(CONNECTION_PATIENCE, read_request(&mut stream))
        .await
        .ok()??;
    let (status, page, outcome) = match judge(&request, &state) {
        Request::Redirect(Ok(code)) => ("200 OK", DONE_PAGE, Some(Ok(code))),
        Request::Redirect(Err(err)) => ("400 Bad Request", FAILED_PAGE, Some(Err(err))),
        Request::WrongMethod => ("405 Method Not Allowed", NOTHING_PAGE, None),
        Request::Elsewhere => ("404 Not Found", NOTHING_PAGE, None),
        Request::Garbled => ("400 Bad Request", NOTHING_PAGE, None),
    };
    // The outcome stands whether or not the browser takes its page.
    let _ = timeout(CONNECTION_PATIENCE, answer(&mut stream, status, page)).await;
    outcome
}

/// Reads a request's first line, and the rest of its headers so the
/// connection can be closed cleanly. `None` for a connection that closes
/// early, is not HTTP, or sends more than a redirect ever would: those get
/// no answer.
async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > MAX_REQUEST {
            return None;
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let line_end = request.windows(2).position(|window| window == b"\r\n")?;
    String::from_utf8(request[..line_end].to_vec()).ok()
}

async fn answer(stream: &mut TcpStream, status: &str, page: &str) -> std::io::Result<()> {
    // The page is fixed text. The headers tell the browser not to keep it,
    // not to load anything for it, and not to pass this address (which has
    // the code in it) on to anyone.
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Content-Security-Policy: default-src 'none'\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n\
         {page}",
        page.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// What a request turned out to be.
enum Request {
    /// The redirect: a code, or the reason there is none.
    Redirect(Result<Secret>),
    WrongMethod,
    Elsewhere,
    Garbled,
}

fn judge(request_line: &str, state: &StateCheck) -> Request {
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Request::Garbled;
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Request::Garbled;
    }
    if method != "GET" {
        return Request::WrongMethod;
    }
    // The target is a path and a query. Parsing it against a base is the
    // way to get both decoded by the same rules a browser encoded them by.
    let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Request::Garbled;
    };
    if url.path() != CALLBACK_PATH {
        return Request::Elsewhere;
    }
    let all = |name: &str| -> Vec<String> {
        url.query_pairs()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .collect()
    };
    let (states, codes, errors) = (all("state"), all("code"), all("error"));
    // The bare address, as someone looking around would ask for it. Not a
    // redirect, and not the end of the wait for one.
    if states.is_empty() && codes.is_empty() && errors.is_empty() {
        return Request::Elsewhere;
    }
    Request::Redirect(redirect(
        &states,
        &codes,
        &errors,
        &all("error_description"),
        state,
    ))
}

fn redirect(
    states: &[String],
    codes: &[String],
    errors: &[String],
    descriptions: &[String],
    state: &StateCheck,
) -> Result<Secret> {
    // The state comes first: until it matches, nothing else in the request
    // is the provider's word, and none of it is repeated.
    let [received] = states else {
        return Err(not_ours());
    };
    if !state.matches(received) {
        return Err(not_ours());
    }
    if let Some(error) = errors.first() {
        let said = match descriptions.first().map(|text| words(text)) {
            Some(description) if !description.is_empty() => {
                format!("{}: {description}", words(error))
            }
            _ => words(error),
        };
        return Err(Error::Auth(format!(
            "the sign-in was not completed ({said})"
        )));
    }
    match codes {
        [code] if !code.is_empty() => Ok(Secret::new(code.as_str())),
        _ => Err(Error::Auth(
            "the browser came back without an authorization code".to_owned(),
        )),
    }
}

fn not_ours() -> Error {
    Error::Auth(
        "the browser came back with an answer that does not belong to this sign-in, so it was not used. Try again"
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::tests::support::{raw_request, same_port_is_free};

    const STATE: &str = "state-0123456789abcdef";
    const CODE: &str = "M.C507_BAY.2.U.code-CCCC";

    async fn listening() -> (u16, tokio::task::JoinHandle<Result<Secret>>) {
        let loopback = Loopback::bind(None).await.unwrap();
        let port = loopback.port();
        let check = StateCheck::new(STATE).unwrap();
        let waiting = tokio::spawn(loopback.redirect(check, Duration::from_secs(300)));
        (port, waiting)
    }

    async fn get(port: u16, target: &str) -> Option<(u16, String)> {
        raw_request(
            port,
            format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
        )
        .await
    }

    #[tokio::test]
    async fn the_redirect_gives_the_code_and_the_browser_a_plain_page() {
        let (port, waiting) = listening().await;
        let (status, response) = get(port, &format!("/?code={CODE}&state={STATE}"))
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert!(response.contains("Return to Quill"), "{response}");
        assert!(response.contains("Content-Security-Policy: default-src 'none'"));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(!response.to_lowercase().contains("<script"));
        // Nothing of the request is in the answer.
        assert!(!response.contains(CODE) && !response.contains(STATE));
        assert_eq!(waiting.await.unwrap().unwrap().expose(), CODE);
    }

    #[tokio::test]
    async fn a_code_is_decoded_as_the_browser_encoded_it() {
        let (port, waiting) = listening().await;
        get(
            port,
            &format!("/?state={STATE}&code=4%2F0A+b%3D&session_state=x"),
        )
        .await
        .unwrap();
        assert_eq!(waiting.await.unwrap().unwrap().expose(), "4/0A b=");
    }

    #[tokio::test]
    async fn a_redirect_with_the_wrong_state_is_refused_and_ends_the_wait() {
        for query in [
            format!("code={CODE}&state=someone-elses"),
            format!("code={CODE}"),
            format!("code={CODE}&state="),
            // The right state among others is not the right state.
            format!("code={CODE}&state={STATE}&state=someone-elses"),
            // Nor is a provider's error believed without it.
            "error=access_denied&error_description=Believe+me&state=nope".to_owned(),
        ] {
            let (port, waiting) = listening().await;
            let (status, response) = get(port, &format!("/?{query}")).await.unwrap();
            assert_eq!(status, 400, "{query}");
            assert!(response.contains("Sign-in did not finish"), "{response}");
            let err = waiting.await.unwrap().unwrap_err();
            assert!(matches!(err, Error::Auth(_)), "{err:?}");
            let shown = format!("{err:?} {err}");
            assert!(shown.contains("does not belong to this sign-in"), "{shown}");
            assert!(
                !shown.contains(CODE) && !shown.contains("Believe"),
                "{shown}"
            );
            assert!(!shown.contains(STATE), "{shown}");
        }
    }

    #[tokio::test]
    async fn a_refusal_is_reported_in_the_providers_words_and_not_echoed_to_the_browser() {
        let (port, waiting) = listening().await;
        let (status, response) = get(
            port,
            &format!(
                "/?error=access_denied&error_description=The+user+%3Cscript%3Ealert(1)%3C%2Fscript%3E+said+no%1B%5B2J&state={STATE}"
            ),
        )
        .await
        .unwrap();
        assert_eq!(status, 400);
        assert!(
            !response.contains("script") && !response.contains("said no"),
            "{response}"
        );
        assert!(!response.contains("access_denied"), "{response}");
        let err = waiting.await.unwrap().unwrap_err();
        let Error::Auth(said) = &err else {
            panic!("{err:?}")
        };
        assert!(said.contains("access_denied"), "{said}");
        assert!(
            said.contains("The user <script>alert(1)</script> said no"),
            "{said}"
        );
        // What a terminal would act on is gone from what will be shown in one.
        assert!(!said.contains('\x1b'), "{said:?}");
    }

    #[tokio::test]
    async fn a_redirect_with_the_right_state_and_no_code_is_an_error() {
        for query in [
            format!("state={STATE}"),
            format!("state={STATE}&code="),
            format!("state={STATE}&code=one&code=two"),
        ] {
            let (port, waiting) = listening().await;
            let (status, _) = get(port, &format!("/?{query}")).await.unwrap();
            assert_eq!(status, 400, "{query}");
            let err = waiting.await.unwrap().unwrap_err();
            assert!(matches!(err, Error::Auth(_)), "{err:?}");
            assert!(!format!("{err:?}").contains("one"), "{err:?}");
        }
    }

    #[tokio::test]
    async fn only_the_callback_path_is_served_and_nothing_else_ends_the_wait() {
        let (port, waiting) = listening().await;
        for target in [
            "/favicon.ico".to_owned(),
            format!("/callback?code={CODE}&state={STATE}"),
            format!("/index.html?state={STATE}"),
            format!("//?code={CODE}&state={STATE}"),
            // The bare address, as someone looking around would ask for it.
            "/".to_owned(),
            "/?utm_source=elsewhere".to_owned(),
        ] {
            let (status, response) = get(port, &target).await.unwrap();
            assert_eq!(status, 404, "{target}");
            assert!(response.contains("There is nothing here"), "{response}");
            assert!(!waiting.is_finished(), "{target} ended the wait");
        }
        for (request, expected) in [
            (
                format!("POST /?code={CODE}&state={STATE} HTTP/1.1\r\nContent-Length: 0\r\n\r\n"),
                405,
            ),
            (
                format!("HEAD /?code={CODE}&state={STATE} HTTP/1.1\r\n\r\n"),
                405,
            ),
            ("OPTIONS / HTTP/1.1\r\n\r\n".to_owned(), 405),
            ("GET http://example.com/ HTTP/1.1\r\n\r\n".to_owned(), 400),
            ("GET /?a b HTTP/1.1\r\n\r\n".to_owned(), 400),
            ("GET / SPDY/3\r\n\r\n".to_owned(), 400),
            ("hello\r\n\r\n".to_owned(), 400),
        ] {
            let (status, _) = raw_request(port, request.as_bytes()).await.unwrap();
            assert_eq!(status, expected, "{request:?}");
            assert!(!waiting.is_finished(), "{request:?} ended the wait");
        }
        // After all that, the real redirect is still welcome.
        let (status, _) = get(port, &format!("/?code={CODE}&state={STATE}"))
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(waiting.await.unwrap().unwrap().expose(), CODE);
    }

    #[tokio::test]
    async fn an_oversized_request_is_dropped_without_an_answer() {
        let (port, waiting) = listening().await;
        // A first line that never ends.
        let endless = format!(
            "GET /?state={STATE}&code={}",
            "A".repeat(MAX_REQUEST + 4096)
        );
        assert_eq!(raw_request(port, endless.as_bytes()).await, None);
        // Headers that never end, after a first line that would have done.
        let padded = format!(
            "GET /?code={CODE}&state={STATE} HTTP/1.1\r\nCookie: {}\r\n\r\n",
            "c".repeat(MAX_REQUEST)
        );
        assert_eq!(raw_request(port, padded.as_bytes()).await, None);
        // Not HTTP at all: bytes that are not text.
        assert_eq!(raw_request(port, b"\xff\xfe\x00\x01\r\n\r\n").await, None);
        assert!(!waiting.is_finished());

        // A request as large as a real one is fine.
        let cookies = "c".repeat(8 * 1024);
        let real = format!(
            "GET /?code={CODE}&state={STATE} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {cookies}\r\n\r\n"
        );
        let (status, _) = raw_request(port, real.as_bytes()).await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(waiting.await.unwrap().unwrap().expose(), CODE);
    }

    #[tokio::test]
    async fn a_connection_that_says_nothing_does_not_hold_up_the_redirect() {
        let (port, waiting) = listening().await;
        // Browsers open connections ahead of need and may never use them.
        let mut idle = Vec::new();
        for _ in 0..3 {
            idle.push(
                TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                    .await
                    .unwrap(),
            );
        }
        let (status, _) = get(port, &format!("/?code={CODE}&state={STATE}"))
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(waiting.await.unwrap().unwrap().expose(), CODE);
    }

    #[tokio::test]
    async fn a_second_redirect_is_not_served() {
        let (port, waiting) = listening().await;
        get(port, &format!("/?code={CODE}&state={STATE}"))
            .await
            .unwrap();
        assert_eq!(waiting.await.unwrap().unwrap().expose(), CODE);
        // The listener is gone: there is nobody to connect to.
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
        assert_eq!(
            get(port, &format!("/?code=another&state={STATE}")).await,
            None
        );
        assert!(same_port_is_free(port).await);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_ends_after_the_given_time_and_closes_the_listener() {
        let loopback = Loopback::bind(None).await.unwrap();
        let port = loopback.port();
        let started = tokio::time::Instant::now();
        let err = loopback
            .redirect(StateCheck::new(STATE).unwrap(), Duration::from_secs(300))
            .await
            .unwrap_err();
        assert_eq!(started.elapsed(), Duration::from_secs(300));
        let Error::Auth(said) = &err else {
            panic!("{err:?}")
        };
        assert!(said.contains("within 5 minutes"), "{said}");
        assert!(same_port_is_free(port).await);
    }

    #[tokio::test]
    async fn dropping_the_wait_closes_the_listener() {
        let (port, waiting) = listening().await;
        // Something half-said is pending when the sign-in is cancelled.
        let mut half = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        half.write_all(b"GET /?code=").await.unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert!(same_port_is_free(port).await);
    }

    #[tokio::test]
    async fn a_port_can_be_asked_for_and_a_taken_one_is_reported() {
        let first = Loopback::bind(None).await.unwrap();
        let port = first.port();
        let err = Loopback::bind(Some(port)).await.unwrap_err();
        let Error::Backend(said) = &err else {
            panic!("{err:?}")
        };
        assert!(said.contains(&port.to_string()), "{said}");
        drop(first);
        assert_eq!(Loopback::bind(Some(port)).await.unwrap().port(), port);
    }
}
