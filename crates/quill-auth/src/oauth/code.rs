//! Signing in through the browser: the authorization code flow with PKCE
//! (RFC 6749 section 4.1, RFC 7636, and RFC 8252 for doing it from a
//! desktop program).

use std::fmt;
use std::time::Duration;

use quill_core::{Error, Result};
use reqwest::Url;

use super::http::{Grant, OAuthHttp, Refusal};
use super::loopback::Loopback;
use super::pkce::{Pkce, StateCheck, random_token};
use super::provider::{Endpoints, OAuthClient, OAuthProvider};

/// How long the user is given to finish in the browser, unless the caller
/// says otherwise. Long enough for a password manager and a second factor.
pub const BROWSER_WAIT: Duration = Duration::from_secs(5 * 60);

/// Shows the user a web address: in the app, by asking the shell to open
/// it in their browser. It is called once, when the listener is ready. An
/// error from it ends the sign-in.
pub type OpenBrowser = Box<dyn FnOnce(&str) -> Result<()> + Send>;

/// A sign-in through the user's browser.
pub struct BrowserSignIn {
    pub open: OpenBrowser,
    /// The port to receive the redirect on. `None` lets the system pick a
    /// free one, which both Microsoft and Google allow for.
    pub port: Option<u16>,
    /// How long to wait for the user to finish in the browser.
    pub wait: Duration,
    /// The address being added, so the provider's page starts with it
    /// filled in.
    pub login_hint: Option<String>,
}

impl BrowserSignIn {
    pub fn new(open: impl FnOnce(&str) -> Result<()> + Send + 'static) -> Self {
        Self {
            open: Box::new(open),
            port: None,
            wait: BROWSER_WAIT,
            login_hint: None,
        }
    }
}

impl fmt::Debug for BrowserSignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserSignIn")
            .field("port", &self.port)
            .field("wait", &self.wait)
            .field("login_hint", &self.login_hint)
            .finish_non_exhaustive()
    }
}

/// Runs the flow: listens, sends the user to the provider, waits for them
/// to come back with a code, and exchanges it for tokens.
///
/// Dropping the future cancels the sign-in and closes the listener.
pub(crate) async fn authorize(
    http: &OAuthHttp,
    provider: &OAuthProvider,
    endpoints: &Endpoints,
    client: &OAuthClient,
    request: BrowserSignIn,
) -> Result<Grant> {
    let pkce = Pkce::new()?;
    let state = random_token()?;
    let check = StateCheck::new(&state)?;

    let listener = Loopback::bind(request.port).await?;
    let redirect_uri = provider.redirect_uri(listener.port());
    let url = authorize_url(
        provider,
        endpoints,
        client,
        &redirect_uri,
        &state,
        &pkce.challenge,
        request.login_hint.as_deref(),
    );

    // If the browser cannot be opened the listener goes with the error:
    // nothing is left waiting for a redirect that will never come.
    (request.open)(url.as_str())?;
    let code = listener.redirect(check, request.wait).await?;

    // The listener is closed. The code is good once, for a few minutes, and
    // only together with the verifier, which has not left this process.
    http.token(
        &endpoints.token,
        client,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", pkce.verifier.expose()),
        ],
    )
    .await
    .map_err(refused)
}

/// The address the browser is sent to.
fn authorize_url(
    provider: &OAuthProvider,
    endpoints: &Endpoints,
    client: &OAuthClient,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    login_hint: Option<&str>,
) -> Url {
    let mut url = endpoints.authorize.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", redirect_uri);
        if !provider.scopes.is_empty() {
            query.append_pair("scope", &provider.scopes.join(" "));
        }
        query
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        for (name, value) in &provider.extra_authorize_params {
            query.append_pair(name, value);
        }
        if let Some(hint) = login_hint.filter(|hint| !hint.is_empty()) {
            query.append_pair("login_hint", hint);
        }
    }
    url
}

/// A token server's no, at sign-in. Whatever the reason, the user is not
/// signed in and has to be told why in the provider's words.
pub(crate) fn refused(refusal: Refusal) -> Error {
    match refusal {
        Refusal::Provider(said) => {
            Error::Auth(format!("the sign-in was not accepted ({})", said.words()))
        }
        Refusal::Other(err) => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_is_sent_everything_the_provider_needs_and_no_secret() {
        let provider = OAuthProvider::google();
        let endpoints = provider.endpoints().unwrap();
        let client = OAuthClient::with_secret("client-1.apps.example.com", "GOCSPX-zzzz");
        let pkce = Pkce::new().unwrap();
        let url = authorize_url(
            &provider,
            &endpoints,
            &client,
            &provider.redirect_uri(50123),
            "state-1",
            &pkce.challenge,
            Some("robin@example.com"),
        );
        assert!(
            url.as_str()
                .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
        );
        let query: Vec<(String, String)> = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        let expected = [
            ("response_type", "code"),
            ("client_id", "client-1.apps.example.com"),
            ("redirect_uri", "http://127.0.0.1:50123"),
            ("scope", "https://mail.google.com/"),
            ("state", "state-1"),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("login_hint", "robin@example.com"),
        ];
        assert_eq!(
            query,
            expected.map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
        assert!(!url.as_str().contains("GOCSPX"));
        assert!(!url.as_str().contains(pkce.verifier.expose()));
    }

    #[test]
    fn the_address_is_one_the_shell_will_agree_to_open() {
        // Whatever goes into it, it comes out as plain printable ASCII,
        // which is the rule the shell opens links by.
        let provider = OAuthProvider::microsoft();
        let endpoints = provider.endpoints().unwrap();
        let url = authorize_url(
            &provider,
            &endpoints,
            &OAuthClient::public("client 1"),
            &provider.redirect_uri(50123),
            "state-1",
            "challenge-1",
            Some("r\u{f6}bin o'neil@example.com\u{1b}[2J"),
        );
        assert!(quill_core::links::is_openable(url.as_str()), "{url}");
    }

    #[test]
    fn microsofts_scopes_are_one_parameter_and_the_redirect_is_localhost() {
        let provider = OAuthProvider::microsoft();
        let endpoints = provider.endpoints().unwrap();
        let url = authorize_url(
            &provider,
            &endpoints,
            &OAuthClient::public("11111111-2222-3333-4444-555555555555"),
            &provider.redirect_uri(50123),
            "state-1",
            "challenge-1",
            None,
        );
        let get = |name: &str| -> Vec<String> {
            url.query_pairs()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .collect()
        };
        assert_eq!(
            get("scope"),
            [
                "https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/User.Read offline_access"
            ]
        );
        assert_eq!(get("redirect_uri"), ["http://localhost:50123"]);
        assert_eq!(get("response_mode"), ["query"]);
        assert_eq!(get("prompt"), ["select_account"]);
        assert!(get("login_hint").is_empty());
        assert!(get("client_secret").is_empty());
    }

    #[test]
    fn a_browser_sign_in_prints_without_its_callback() {
        let request = BrowserSignIn::new(|_| Ok(()));
        assert_eq!(request.wait, BROWSER_WAIT);
        assert!(format!("{request:?}").starts_with("BrowserSignIn"));
    }
}
