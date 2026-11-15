//! Asking a provider's token server, and reading what it says.
//!
//! Everything a token server is sent is a form, and everything it answers
//! is JSON: a grant of tokens, or an `error` with a few words about it.
//! This module turns that into a [`Grant`] or a [`Refusal`], and takes care
//! that neither the request's secrets nor the answer's can end up in an
//! error message on the way.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use quill_core::{Error, Result};
use quill_net::Trust;
use reqwest::Url;
use reqwest::header::ACCEPT;
use serde_json::{Map, Value};

use super::provider::OAuthClient;
use super::secret::{Secret, words};

/// The most a token server may say. Microsoft's tokens run to a few
/// kilobytes each; nothing honest comes near this.
const MAX_ANSWER: usize = 256 * 1024;

/// How long a token is taken to last when the server does not say. Guessing
/// short costs a few needless renewals; guessing long would hand out a dead
/// token.
pub(crate) const ASSUMED_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// The longest a token is believed to last, whatever the server says.
/// Microsoft's last an hour or so and can be configured up to a day;
/// Google's last an hour.
pub(crate) const LONGEST_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The form fields whose values are secrets. If a server quotes one back
/// in its refusal, it is cut out before the words go anywhere.
const SECRET_FIELDS: &[&str] = &[
    "code",
    "code_verifier",
    "refresh_token",
    "device_code",
    "client_secret",
];

/// The HTTP client the OAuth endpoints are reached with.
///
/// TLS is Quill's own configuration: the system's trust store and whatever
/// `trust` adds, through rustls. Redirects are not followed, because a
/// token server has no business sending a form full of secrets somewhere
/// else. No proxy is used, like the mail connections. Connections are not
/// kept: a token server is asked about once an hour.
#[derive(Clone)]
pub struct OAuthHttp {
    client: reqwest::Client,
}

impl OAuthHttp {
    pub fn new(trust: &Trust) -> Result<Self> {
        Self::build(trust, Some(quill_net::PATIENCE))
    }

    /// The same client with no limit on how long a server may take.
    ///
    /// Tests that stop the clock need it: with time paused, tokio moves the
    /// clock to the next timer whenever nothing else is ready, so a timeout
    /// that is pending while a real socket is waited on fires at once.
    #[cfg(test)]
    pub(crate) fn unhurried() -> Self {
        Self::build(&Trust::default(), None).expect("an HTTP client")
    }

    /// The same client with less patience than the app has, so a test of
    /// what happens when it runs out does not take twenty seconds.
    #[cfg(test)]
    pub(crate) fn impatient(patience: Duration) -> Self {
        Self::build(&Trust::default(), Some(patience)).expect("an HTTP client")
    }

    fn build(trust: &Trust, patience: Option<Duration>) -> Result<Self> {
        let (tls, _presented) = quill_net::tls::client_config(trust)?;
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(Arc::unwrap_or_clone(tls))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .pool_idle_timeout(None)
            .user_agent(concat!("Quill/", env!("CARGO_PKG_VERSION")));
        if let Some(patience) = patience {
            builder = builder.timeout(patience);
        }
        let client = builder
            .build()
            .map_err(|err| Error::Backend(format!("cannot set up the HTTP client: {err}")))?;
        Ok(Self { client })
    }

    /// Asks the token endpoint for tokens. `grant` says on what grounds;
    /// who is asking is added here.
    pub(crate) async fn token(
        &self,
        url: &Url,
        client: &OAuthClient,
        grant: &[(&str, &str)],
    ) -> std::result::Result<Grant, Refusal> {
        let fields = self.post(url, client, grant).await?;
        Grant::read(host(url), &fields).map_err(Refusal::Other)
    }

    /// Posts a form and returns the fields of a successful answer.
    pub(crate) async fn post(
        &self,
        url: &Url,
        client: &OAuthClient,
        params: &[(&str, &str)],
    ) -> std::result::Result<Fields, Refusal> {
        let mut form = params.to_vec();
        form.push(("client_id", client.id.as_str()));
        if let Some(secret) = &client.secret {
            form.push(("client_secret", secret.expose()));
        }
        let host = host(url);

        let mut response = self
            .client
            .post(url.clone())
            .header(ACCEPT, "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|err| Refusal::Other(unreachable(host, err)))?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| Refusal::Other(unreachable(host, err)))?
        {
            if body.len() + chunk.len() > MAX_ANSWER {
                return Err(Refusal::Other(Error::Temporary(format!(
                    "{host} sent an answer too large to be one"
                ))));
            }
            body.extend_from_slice(&chunk);
        }
        // What went wrong with the JSON is not repeated: the parser quotes
        // what it tripped over, and that could be a token.
        let fields = match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(fields)) => Some(Fields(fields)),
            _ => None,
        };
        settle(host, status, fields, &form)
    }
}

impl fmt::Debug for OAuthHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthHttp")
    }
}

fn host(url: &Url) -> &str {
    url.host_str().unwrap_or("the token server")
}

/// A request that got no answer: the network's fault, not the account's.
fn unreachable(host: &str, err: reqwest::Error) -> Error {
    let timed_out = err.is_timeout();
    let err = err.without_url();
    let mut reason = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(inner) = source {
        reason.push_str(": ");
        reason.push_str(&inner.to_string());
        source = inner.source();
    }
    if timed_out {
        reason = "timed out waiting for an answer".to_owned();
    }
    Error::Connection(format!("{host}: {}", words(&reason)))
}

/// Sorts an answer into the fields of a success, the provider's refusal, or
/// a reason to try again later.
fn settle(
    host: &str,
    status: u16,
    fields: Option<Fields>,
    form: &[(&str, &str)],
) -> std::result::Result<Fields, Refusal> {
    let refusal = fields.as_ref().and_then(|fields| {
        let code = scrub(fields.text("error")?, form);
        let description = fields
            .text("error_description")
            .map(|text| scrub(text, form))
            .filter(|text| !text.is_empty());
        Some(ProviderError { code, description })
    });
    let overloaded = status == 429
        || status >= 500
        || refusal.as_ref().is_some_and(|refusal| {
            matches!(
                refusal.code.as_str(),
                "temporarily_unavailable" | "server_error"
            )
        });
    if overloaded {
        let said = refusal.map_or(String::new(), |refusal| format!(": {}", refusal.words()));
        return Err(Refusal::Other(Error::Temporary(format!(
            "{host} cannot answer just now (HTTP {status}{said})"
        ))));
    }
    if let Some(refusal) = refusal {
        return Err(Refusal::Provider(refusal));
    }
    match fields {
        Some(fields) if (200..300).contains(&status) => Ok(fields),
        // Not the provider's JSON at all: something between here and there
        // answered in its place.
        _ => Err(Refusal::Other(Error::Temporary(format!(
            "{host} gave an answer that could not be read (HTTP {status})"
        )))),
    }
}

/// A server's words, with anything that was a secret in the request cut
/// out of them.
fn scrub(text: &str, form: &[(&str, &str)]) -> String {
    let mut text = text.to_owned();
    for (name, value) in form {
        if SECRET_FIELDS.contains(name) && !value.is_empty() {
            text = text.replace(value, "…");
        }
    }
    words(&text)
}

/// Why no tokens were given.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The provider answered, and the answer was no.
    Provider(ProviderError),
    /// The provider could not be asked, or what it said made no sense.
    Other(Error),
}

/// An OAuth error answer (RFC 6749, section 5.2): a code that programs act
/// on and, usually, a sentence for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderError {
    pub code: String,
    pub description: Option<String>,
}

impl ProviderError {
    /// The provider's words, for showing to the user.
    pub fn words(&self) -> String {
        match &self.description {
            Some(description) => format!("{}: {description}", self.code),
            None => self.code.clone(),
        }
    }
}

/// The fields of a JSON answer. Some of them are tokens, so they are not
/// printed.
pub(crate) struct Fields(Map<String, Value>);

impl Fields {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// A field that is text, and not empty.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }
}

#[cfg(test)]
impl Fields {
    /// The fields of a JSON object, for tests.
    pub(crate) fn from_json(value: Value) -> Self {
        match value {
            Value::Object(fields) => Self(fields),
            other => panic!("not an object: {other}"),
        }
    }
}

impl fmt::Debug for Fields {
    /// The names say what kind of answer it was; the values stay out.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// What a token server grants: an access token, how long it lasts, and
/// perhaps a refresh token to get the next one with.
#[derive(Debug, Clone)]
pub(crate) struct Grant {
    pub access: Secret,
    pub refresh: Option<Secret>,
    pub lifetime: Duration,
}

impl Grant {
    fn read(host: &str, fields: &Fields) -> Result<Self> {
        let Some(access) = fields.text("access_token") else {
            return Err(Error::Temporary(format!(
                "{host} gave an answer with no access token in it"
            )));
        };
        // Mail servers take bearer tokens. Anything else needs signing
        // Quill does not do, and offering it as a bearer token would only
        // be refused later with less to go on.
        if let Some(kind) = fields.text("token_type")
            && !kind.eq_ignore_ascii_case("bearer")
        {
            return Err(Error::Temporary(format!(
                "{host} gave a kind of token Quill cannot use"
            )));
        }
        Ok(Self {
            access: Secret::new(access),
            refresh: fields.text("refresh_token").map(Secret::new),
            lifetime: lifetime(fields.get("expires_in")),
        })
    }
}

/// How long a token lasts, from the server's `expires_in`.
///
/// It should be a number of seconds. Some servers send it as text. One that
/// is missing or unreadable is taken to be short; zero or less means the
/// token is spent already, so it is used once and not kept; and nothing is
/// believed to last longer than a day.
pub(crate) fn lifetime(expires_in: Option<&Value>) -> Duration {
    match seconds(expires_in) {
        Some(seconds) if seconds > 0.0 => {
            Duration::from_secs_f64(seconds.min(LONGEST_LIFETIME.as_secs_f64()))
        }
        Some(_) => Duration::ZERO,
        None => ASSUMED_LIFETIME,
    }
}

/// A number of seconds from an answer, whether sent as a number or as text.
/// `None` if it is missing or is not a number at all.
pub(crate) fn seconds(value: Option<&Value>) -> Option<f64> {
    let seconds = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    seconds.filter(|seconds| seconds.is_finite())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_lifetime_is_read_leniently_and_believed_within_reason() {
        let hour = Duration::from_secs(3600);
        assert_eq!(lifetime(Some(&json!(3600))), hour);
        assert_eq!(lifetime(Some(&json!("3600"))), hour);
        assert_eq!(lifetime(Some(&json!(" 3600 "))), hour);
        assert_eq!(
            lifetime(Some(&json!(3599.5))),
            Duration::from_millis(3_599_500)
        );
        // Missing or unreadable: short.
        assert_eq!(lifetime(None), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!(null))), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!("soon"))), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!("NaN"))), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!("inf"))), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!([3600]))), ASSUMED_LIFETIME);
        assert_eq!(lifetime(Some(&json!(true))), ASSUMED_LIFETIME);
        // Spent already.
        assert_eq!(lifetime(Some(&json!(0))), Duration::ZERO);
        assert_eq!(lifetime(Some(&json!(-3600))), Duration::ZERO);
        assert_eq!(lifetime(Some(&json!("-1"))), Duration::ZERO);
        // Too good to be true.
        assert_eq!(lifetime(Some(&json!(315_360_000))), LONGEST_LIFETIME);
        assert_eq!(lifetime(Some(&json!(1e300))), LONGEST_LIFETIME);
        assert_eq!(lifetime(Some(&json!(u64::MAX))), LONGEST_LIFETIME);
    }

    fn fields(value: Value) -> Option<Fields> {
        match value {
            Value::Object(map) => Some(Fields(map)),
            _ => None,
        }
    }

    #[test]
    fn answers_are_sorted_by_what_the_caller_can_do_about_them() {
        let form = [("refresh_token", "1//refresh-AAAA")];
        // The provider said no.
        let refusal = settle(
            "login.example.com",
            400,
            fields(json!({"error": "invalid_grant", "error_description": "Token has been\r\nrevoked."})),
            &form,
        )
        .unwrap_err();
        match refusal {
            Refusal::Provider(refusal) => {
                assert_eq!(refusal.code, "invalid_grant");
                assert_eq!(refusal.words(), "invalid_grant: Token has been revoked.");
            }
            other => panic!("{other:?}"),
        }
        // The provider is having a bad day, whatever else it says.
        for (status, body) in [
            (500, json!({"error": "invalid_grant"})),
            (503, json!("Service Unavailable")),
            (429, json!({"error": "slow_down"})),
            (400, json!({"error": "temporarily_unavailable"})),
            (400, json!({"error": "server_error"})),
        ] {
            assert!(
                matches!(
                    settle("login.example.com", status, fields(body), &form),
                    Err(Refusal::Other(Error::Temporary(_)))
                ),
                "{status}"
            );
        }
        // Something that is not the provider answered.
        for (status, body) in [
            (200, json!("<html>Sign in to the hotel Wi-Fi</html>")),
            (403, json!({"message": "Forbidden"})),
            (302, json!(null)),
            (404, json!({})),
        ] {
            assert!(
                matches!(
                    settle("login.example.com", status, fields(body), &form),
                    Err(Refusal::Other(Error::Temporary(_)))
                ),
                "{status}"
            );
        }
        // A refusal is a refusal even when it is sent as a success.
        assert!(matches!(
            settle(
                "login.example.com",
                200,
                fields(json!({"error": "invalid_request"})),
                &form
            ),
            Err(Refusal::Provider(_))
        ));
        assert!(
            settle(
                "login.example.com",
                200,
                fields(json!({"access_token": "a"})),
                &form
            )
            .is_ok()
        );
    }

    #[test]
    fn a_server_that_quotes_a_secret_back_is_not_repeated() {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", "1//refresh-AAAA"),
            ("client_secret", "GOCSPX-zzzz"),
        ];
        let refusal = settle(
            "login.example.com",
            400,
            fields(json!({
                "error": "invalid_grant",
                "error_description": "Bad token 1//refresh-AAAA for secret GOCSPX-zzzz (refresh_token)"
            })),
            &form,
        )
        .unwrap_err();
        let shown = format!("{refusal:?}");
        assert!(!shown.contains("1//refresh-AAAA"), "{shown}");
        assert!(!shown.contains("GOCSPX-zzzz"), "{shown}");
        // What was not secret stays readable.
        assert!(
            shown.contains("Bad token … for secret … (refresh_token)"),
            "{shown}"
        );
    }

    #[test]
    fn a_grant_needs_a_bearer_access_token_and_never_prints() {
        let host = "login.example.com";
        let grant = Grant::read(
            host,
            &fields(json!({
                "access_token": "ya29.access-BBBB",
                "refresh_token": "1//refresh-AAAA",
                "token_type": "Bearer",
                "expires_in": 3599,
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(grant.access.expose(), "ya29.access-BBBB");
        assert_eq!(grant.refresh.as_ref().unwrap().expose(), "1//refresh-AAAA");
        assert_eq!(grant.lifetime, Duration::from_secs(3599));
        let shown = format!("{grant:?}");
        assert!(!shown.contains("ya29") && !shown.contains("1//"), "{shown}");

        let answer =
            fields(json!({"access_token": "ya29.access-BBBB", "id_token": "eyJ.x.y"})).unwrap();
        let shown = format!("{answer:?}");
        assert!(shown.contains("access_token"), "{shown}");
        assert!(!shown.contains("ya29") && !shown.contains("eyJ"), "{shown}");

        // No refresh token, no token type, no lifetime: still a grant.
        let bare = Grant::read(host, &fields(json!({"access_token": "t"})).unwrap()).unwrap();
        assert!(bare.refresh.is_none());
        assert_eq!(bare.lifetime, ASSUMED_LIFETIME);
        // An empty refresh token is no refresh token.
        let empty = Grant::read(
            host,
            &fields(json!({"access_token": "t", "refresh_token": ""})).unwrap(),
        )
        .unwrap();
        assert!(empty.refresh.is_none());

        for unusable in [
            json!({}),
            json!({"access_token": ""}),
            json!({"access_token": 12345}),
            json!({"access_token": "ya29.access-BBBB", "token_type": "mac"}),
        ] {
            let err = Grant::read(host, &fields(unusable).unwrap()).unwrap_err();
            assert!(matches!(err, Error::Temporary(_)), "{err:?}");
            assert!(!format!("{err:?}").contains("ya29"));
        }
    }
}
