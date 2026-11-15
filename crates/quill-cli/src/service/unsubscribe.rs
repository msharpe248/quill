//! One-click unsubscribe (RFC 8058): telling a mailing list's server to
//! stop, with the one request the standard describes.
//!
//! This is the only request Quill makes to a server the user did not set
//! up, at an address that came in a message. So it is kept as small as it
//! can be: a POST of `List-Unsubscribe=One-Click` and nothing else, over
//! HTTPS only, to a server named plainly in the address, with no cookies,
//! no sign-in and no redirect followed, and a short time to answer in. The
//! answer's body is never read. The UI asks the user before asking for it.

use std::sync::Arc;
use std::time::Duration;

use quill_net::Trust;
use reqwest::Url;
use reqwest::header::CONTENT_TYPE;

/// What the standard says the request's body is, to the letter.
const BODY: &str = "List-Unsubscribe=One-Click";

/// How long the list's server has, from start to finish.
const PATIENCE: Duration = Duration::from_secs(15);

/// Whether plain HTTP to this machine is allowed: only in this crate's own
/// tests, which stand a server on `127.0.0.1` in for a list's.
const LOOPBACK_HTTP: bool = cfg!(test);

/// Where the request goes, once the address has been found fit to use.
#[derive(Debug)]
struct Target {
    url: Url,
    /// The server, as the user is told of it.
    host: String,
    /// It is a stand-in on this machine, reached without TLS.
    loopback: bool,
}

/// Checks the address the list gave. `loopback_http` lets a test's server
/// on this machine through; everything else must be HTTPS to a server the
/// address names plainly, exactly as the UI showed it to the user.
fn target(address: &str, loopback_http: bool) -> Result<Target, String> {
    const UNFIT: &str = "The list's address for unsubscribing is not one Quill will use: it must be an https address that names its server plainly.";
    let url = Url::parse(address).map_err(|_| UNFIT.to_owned())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UNFIT.to_owned());
    }
    if loopback_http && url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
        return Ok(Target {
            url,
            host: "127.0.0.1".to_owned(),
            loopback: true,
        });
    }
    // Two readings of the address, the one the user was shown and the one
    // the request will follow, must name the same server.
    let shown = quill_mime::unsubscribe::web_host(address).ok_or_else(|| UNFIT.to_owned())?;
    let followed = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || !followed.eq_ignore_ascii_case(shown) {
        return Err(UNFIT.to_owned());
    }
    Ok(Target {
        host: followed.to_owned(),
        url,
        loopback: false,
    })
}

/// Asks the server at `address` to unsubscribe the user. Returns the
/// server's name, as far as it could be read, and how it went: `Ok` when
/// the server said it took the request, otherwise why not, as a sentence.
pub(super) async fn one_click(address: &str) -> (String, Result<(), String>) {
    match target(address, LOOPBACK_HTTP) {
        Ok(target) => {
            let result = post(&target, PATIENCE).await;
            (target.host, result)
        }
        Err(reason) => (String::new(), Err(reason)),
    }
}

async fn post(target: &Target, patience: Duration) -> Result<(), String> {
    let host = &target.host;
    let (tls, _presented) = quill_net::tls::client_config(&Trust::default())
        .map_err(|err| format!("HTTPS could not be set up: {err}."))?;
    // No cookie store is ever built into this client, and nothing here
    // adds a header that says who is asking.
    let mut builder = reqwest::Client::builder()
        .tls_backend_preconfigured(Arc::unwrap_or_clone(tls))
        .https_only(!target.loopback)
        // A redirect would be a request to a server the list did not name
        // and the user was not asked about.
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(patience.min(quill_net::PATIENCE))
        .timeout(patience)
        .user_agent(concat!("Quill/", env!("CARGO_PKG_VERSION")));
    if target.loopback {
        // A proxy from the environment must not come between a test and
        // its server.
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|err| format!("HTTPS could not be set up: {err}."))?;
    let answer = client
        .post(target.url.clone())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(BODY)
        .send()
        .await;
    match answer {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                Ok(())
            } else if status.is_redirection() {
                Err(format!(
                    "{host} answered by pointing somewhere else ({}), which Quill does not follow. You are probably still subscribed.",
                    status.as_u16()
                ))
            } else {
                Err(format!(
                    "{host} refused the request ({}). You are probably still subscribed.",
                    status.as_u16()
                ))
            }
        }
        Err(err) if err.is_timeout() => Err(format!(
            "{host} did not answer within {} seconds. It may or may not have taken the request.",
            patience.as_secs().max(1)
        )),
        Err(err) => {
            // The address can hold a token that is the user's alone: it
            // is kept out of what is shown. The innermost error is the
            // one that says what went wrong.
            let err = err.without_url();
            let mut cause: &dyn std::error::Error = &err;
            while let Some(deeper) = cause.source() {
                cause = deeper;
            }
            Err(format!("{host} could not be reached: {cause}."))
        }
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_string, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn only_a_plain_https_address_is_fit_to_ask() {
        let fit = target("https://lists.example.com/u/abc?t=1", false).unwrap();
        assert_eq!(fit.host, "lists.example.com");
        assert!(!fit.loopback);
        assert_eq!(fit.url.as_str(), "https://lists.example.com/u/abc?t=1");
        assert_eq!(
            target("HTTPS://Lists.Example.COM:8443/u", false)
                .unwrap()
                .host,
            "lists.example.com"
        );
        for unfit in [
            // Not HTTPS. The production path never lets these through,
            // this machine included.
            "http://lists.example.com/u",
            "http://127.0.0.1:8080/u",
            "http://localhost/u",
            // Read one way by a person and another by a program.
            "https://lists.example.com@evil.example.net/u",
            "https://lists.example.com:pw@evil.example.net/u",
            "https://lists.example.com\\@evil.example.net/u",
            "https://evil.example.net\\.lists.example.com/u",
            // The user's own machine or network.
            "https://127.0.0.1/u",
            "https://[::1]/u",
            "https://localhost/u",
            "https://192.168.1.1/u",
            "https://2130706433/u",
            // Not addresses of a web server at all.
            "mailto:leave@lists.example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "lists.example.com/u",
            "",
        ] {
            let refused = target(unfit, false).unwrap_err();
            assert!(refused.contains("not one Quill will use"), "{unfit}");
        }
        // A test may reach its own stand-in, and nothing else, without TLS.
        let stand_in = target("http://127.0.0.1:8080/u", true).unwrap();
        assert!(stand_in.loopback);
        for unfit in [
            "http://lists.example.com/u",
            "http://localhost:8080/u",
            "http://127.0.0.2:8080/u",
            "http://user@127.0.0.1:8080/u",
            "ftp://127.0.0.1/u",
        ] {
            assert!(target(unfit, true).is_err(), "{unfit}");
        }
    }

    /// The address of something on a stand-in server, checked as a test's.
    fn at(server: &MockServer, place: &str) -> Target {
        target(&format!("{}{place}", server.uri()), true).unwrap()
    }

    #[tokio::test]
    async fn the_request_is_the_one_the_standard_describes_and_nothing_more() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/u/r0b1n"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(BODY))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "seen=1")
                    .set_body_string("You are unsubscribed."),
            )
            .expect(2)
            .mount(&server)
            .await;
        let target = at(&server, "/u/r0b1n");
        assert_eq!(post(&target, PATIENCE).await, Ok(()));
        // Asked again, nothing of the first answer comes back with it.
        assert_eq!(post(&target, PATIENCE).await, Ok(()));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2);
        for request in &received {
            for private in ["cookie", "authorization", "referer", "origin"] {
                assert!(!request.headers.contains_key(private), "{private}");
            }
            assert_eq!(
                request.headers.get("user-agent").unwrap(),
                concat!("Quill/", env!("CARGO_PKG_VERSION"))
            );
            assert!(request.url.query().is_none());
        }
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_and_a_refusal_is_said() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/moved"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/elsewhere", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Nothing ever arrives where the redirect points.
        Mock::given(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/gone"))
            .respond_with(ResponseTemplate::new(404).set_body_string("secret words"))
            .mount(&server)
            .await;

        let moved = post(&at(&server, "/moved"), PATIENCE).await.unwrap_err();
        assert!(
            moved.starts_with("127.0.0.1 answered by pointing somewhere else (302)"),
            "{moved}"
        );
        let gone = post(&at(&server, "/gone?token=s3cret"), PATIENCE)
            .await
            .unwrap_err();
        assert_eq!(
            gone,
            "127.0.0.1 refused the request (404). You are probably still subscribed."
        );
    }

    #[tokio::test]
    async fn a_server_that_is_silent_or_absent_is_given_up_on() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        let silent = post(&at(&server, "/u?token=s3cret"), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(silent.contains("did not answer"), "{silent}");
        assert!(!silent.contains("s3cret"), "{silent}");

        // Nothing listening at all.
        let closed = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        let absent = target(&format!("http://127.0.0.1:{closed}/u?token=s3cret"), true).unwrap();
        let reason = post(&absent, PATIENCE).await.unwrap_err();
        assert!(
            reason.starts_with("127.0.0.1 could not be reached: "),
            "{reason}"
        );
        // The address, and the token in it, stay out of what is shown.
        assert!(
            !reason.contains("s3cret") && !reason.contains("/u"),
            "{reason}"
        );
    }

    #[tokio::test]
    async fn the_production_path_asks_no_one_over_plain_http() {
        // What the service is handed outside its tests' own stand-in: an
        // address that is not HTTPS is refused before anything is sent.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let address = format!("{}/u", server.uri());
        assert!(target(&address, false).is_err());
        let (host, result) = one_click("http://lists.example.com/u").await;
        assert_eq!(host, "");
        assert!(result.unwrap_err().contains("not one Quill will use"));
    }
}
