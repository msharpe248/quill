//! Who is signed in to: an identity provider's addresses and the
//! permissions asked of it, and the application Quill presents itself as.

use std::net::IpAddr;

use quill_core::{Error, Result};
use reqwest::Url;

use super::secret::Secret;

/// The application registered with a provider. The id comes from the
/// user's config: Quill ships none of its own.
#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub id: String,
    /// Google gives an installed application a "secret" and wants it sent,
    /// though it cannot be kept secret in a program anyone can download and
    /// Google does not treat it as one. Microsoft's public clients have
    /// none, and must not send one.
    pub secret: Option<Secret>,
}

impl OAuthClient {
    /// A public client: an id and nothing else.
    pub fn public(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            secret: None,
        }
    }

    /// A client whose provider also wants a secret sent.
    pub fn with_secret(id: impl Into<String>, secret: impl Into<Secret>) -> Self {
        Self {
            id: id.into(),
            secret: Some(secret.into()),
        }
    }
}

/// How this computer is named in the address the browser is sent back to.
/// Providers compare that address with what was registered, each in its
/// own way, so the form matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectHost {
    /// `http://localhost:PORT`. Microsoft registers `http://localhost` for
    /// a desktop application and ignores the port when comparing.
    Localhost,
    /// `http://127.0.0.1:PORT`. Google accepts this for a desktop
    /// application with no registration, on any port.
    Ipv4,
}

/// An identity provider: where its endpoints are and what to ask it for.
///
/// Every address must be `https`. Plain `http` is accepted only for this
/// computer itself (`127.0.0.1`, `::1`, `localhost`), which is how tests
/// stand in for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProvider {
    /// A short name, lower case, such as `microsoft`. It is part of the
    /// name the refresh token is kept under.
    pub name: String,
    /// Where the browser is sent for the user to sign in.
    pub authorize_url: String,
    /// Where codes and refresh tokens are exchanged for access tokens.
    pub token_url: String,
    /// Where a device code is asked for, if the provider offers that.
    pub device_url: Option<String>,
    pub scopes: Vec<String>,
    /// More parameters for the address the browser is sent to, such as
    /// `prompt`. The ones the flow sets itself cannot be given here.
    pub extra_authorize_params: Vec<(String, String)>,
    pub redirect_host: RedirectHost,
}

/// The parameters the flow sets. A provider description that set one of
/// these too would send it twice, and which one a server believed would be
/// anyone's guess.
const RESERVED: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
    "login_hint",
];

impl OAuthProvider {
    /// Microsoft 365 and Outlook.com mail through Microsoft Graph, for
    /// accounts of any organisation and personal ones.
    pub fn microsoft() -> Self {
        Self::entra("common")
    }

    /// The same for one tenant: its id, one of its domain names, or
    /// `organizations` or `consumers`.
    pub fn microsoft_tenant(tenant: &str) -> Result<Self> {
        let plain = !tenant.is_empty()
            && tenant
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if !plain {
            return Err(Error::Backend(
                "a Microsoft tenant is an id, a domain name, or one of common, organizations and consumers"
                    .to_owned(),
            ));
        }
        Ok(Self::entra(tenant))
    }

    fn entra(tenant: &str) -> Self {
        let base = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0");
        Self {
            name: "microsoft".to_owned(),
            authorize_url: format!("{base}/authorize"),
            token_url: format!("{base}/token"),
            device_url: Some(format!("{base}/devicecode")),
            // Graph's permissions are written out in full so that the token
            // is unmistakably for Graph. `offline_access` belongs to the
            // sign-in service, not to Graph, and has no longer form: it is
            // what makes Microsoft issue a refresh token.
            scopes: vec![
                "https://graph.microsoft.com/Mail.ReadWrite".to_owned(),
                "https://graph.microsoft.com/Mail.Send".to_owned(),
                "https://graph.microsoft.com/User.Read".to_owned(),
                "offline_access".to_owned(),
            ],
            extra_authorize_params: vec![
                // The code must come back in the query, the only place the
                // listener looks.
                ("response_mode".to_owned(), "query".to_owned()),
                // Without this a browser already signed in to one Microsoft
                // account uses it without asking, and that may not be the
                // account being added.
                ("prompt".to_owned(), "select_account".to_owned()),
            ],
            redirect_host: RedirectHost::Localhost,
        }
    }

    /// Gmail over IMAP and SMTP.
    ///
    /// There is no device code address: Google offers that flow only for a
    /// short list of permissions, and mail is not among them.
    pub fn google() -> Self {
        Self {
            name: "google".to_owned(),
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
            token_url: "https://oauth2.googleapis.com/token".to_owned(),
            device_url: None,
            // The one permission Google's IMAP and SMTP servers accept.
            scopes: vec!["https://mail.google.com/".to_owned()],
            extra_authorize_params: vec![
                // Google issues a refresh token only when asked for offline
                // access, and only when the user is shown the consent
                // screen: a second sign-in without `prompt=consent` would
                // come back with no refresh token at all.
                ("access_type".to_owned(), "offline".to_owned()),
                ("prompt".to_owned(), "consent".to_owned()),
            ],
            redirect_host: RedirectHost::Ipv4,
        }
    }

    /// The address the browser is sent back to, for a listener on `port`.
    /// It has no path: both providers' conventions for desktop
    /// applications are the bare host and port.
    pub fn redirect_uri(&self, port: u16) -> String {
        match self.redirect_host {
            RedirectHost::Localhost => format!("http://localhost:{port}"),
            RedirectHost::Ipv4 => format!("http://127.0.0.1:{port}"),
        }
    }

    /// Checks the description and parses its addresses. Every flow starts
    /// here, so nothing is ever sent to an address that fails this.
    pub(crate) fn endpoints(&self) -> Result<Endpoints> {
        let plain = !self.name.is_empty()
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !plain {
            return Err(Error::Backend(
                "an OAuth provider's name is lower-case letters, digits, - and _".to_owned(),
            ));
        }
        if let Some((name, _)) = self
            .extra_authorize_params
            .iter()
            .find(|(name, _)| RESERVED.contains(&name.as_str()))
        {
            return Err(Error::Backend(format!(
                "the sign-in parameter {name} is set by Quill and cannot be given for a provider"
            )));
        }
        Ok(Endpoints {
            authorize: endpoint(&self.authorize_url, "sign-in")?,
            token: endpoint(&self.token_url, "token")?,
            device: self
                .device_url
                .as_deref()
                .map(|url| endpoint(url, "device code"))
                .transpose()?,
        })
    }
}

impl OAuthClient {
    pub(crate) fn check(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::Backend(
                "no OAuth client id is configured for this account".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A provider's addresses, checked.
#[derive(Debug, Clone)]
pub(crate) struct Endpoints {
    pub authorize: Url,
    pub token: Url,
    pub device: Option<Url>,
}

/// Parses one address and refuses it unless it is `https`, or `http` to
/// this computer. `what` names it in the refusal.
fn endpoint(address: &str, what: &str) -> Result<Url> {
    let url = Url::parse(address)
        .map_err(|_| Error::Backend(format!("the {what} address is not a URL")))?;
    // Credentials in an address end up wherever addresses are shown.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Backend(format!(
            "the {what} address must not contain a user name or password"
        )));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => Err(Error::Insecure(format!("the {what} address must be https"))),
    }
}

fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsoft_is_the_v2_endpoints_of_a_tenant_and_graphs_mail_permissions() {
        let provider = OAuthProvider::microsoft();
        assert_eq!(
            provider.authorize_url,
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
        );
        assert_eq!(
            provider.token_url,
            "https://login.microsoftonline.com/common/oauth2/v2.0/token"
        );
        assert_eq!(
            provider.device_url.as_deref(),
            Some("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode")
        );
        assert_eq!(
            provider.scopes,
            [
                "https://graph.microsoft.com/Mail.ReadWrite",
                "https://graph.microsoft.com/Mail.Send",
                "https://graph.microsoft.com/User.Read",
                "offline_access",
            ]
        );
        assert_eq!(provider.redirect_uri(50123), "http://localhost:50123");
        provider.endpoints().unwrap();

        let tenant = OAuthProvider::microsoft_tenant("contoso.example.com").unwrap();
        assert_eq!(
            tenant.token_url,
            "https://login.microsoftonline.com/contoso.example.com/oauth2/v2.0/token"
        );
        tenant.endpoints().unwrap();
    }

    #[test]
    fn a_tenant_cannot_smuggle_in_another_address() {
        for tenant in ["", "a/b", "common/../x", "a b", "a?b", "a#b", "a@b"] {
            assert!(
                matches!(
                    OAuthProvider::microsoft_tenant(tenant),
                    Err(Error::Backend(_))
                ),
                "{tenant:?}"
            );
        }
    }

    #[test]
    fn google_asks_for_mail_offline_and_with_consent() {
        let provider = OAuthProvider::google();
        assert_eq!(
            provider.authorize_url,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(provider.token_url, "https://oauth2.googleapis.com/token");
        assert_eq!(provider.device_url, None);
        assert_eq!(provider.scopes, ["https://mail.google.com/"]);
        assert!(
            provider
                .extra_authorize_params
                .contains(&("access_type".to_owned(), "offline".to_owned()))
        );
        assert!(
            provider
                .extra_authorize_params
                .contains(&("prompt".to_owned(), "consent".to_owned()))
        );
        assert_eq!(provider.redirect_uri(50123), "http://127.0.0.1:50123");
        provider.endpoints().unwrap();
    }

    fn with_token_url(url: &str) -> OAuthProvider {
        OAuthProvider {
            token_url: url.to_owned(),
            ..OAuthProvider::google()
        }
    }

    #[test]
    fn plain_http_is_for_this_computer_only() {
        for url in [
            "http://127.0.0.1:8080/token",
            "http://localhost:8080/token",
            "http://[::1]:8080/token",
            "https://login.example.com/token",
        ] {
            with_token_url(url).endpoints().expect(url);
        }
        for url in [
            "http://login.example.com/token",
            "http://127.0.0.1.example.com/token",
            "http://localhost.example.com/token",
            "http://192.168.1.10/token",
            "ftp://127.0.0.1/token",
        ] {
            assert!(
                matches!(with_token_url(url).endpoints(), Err(Error::Insecure(_))),
                "{url}"
            );
        }
        for url in [
            "",
            "not a url",
            "/token",
            "https://user:pass@login.example.com/token",
        ] {
            assert!(
                matches!(with_token_url(url).endpoints(), Err(Error::Backend(_))),
                "{url}"
            );
        }
    }

    #[test]
    fn a_provider_cannot_override_what_the_flow_sets() {
        for name in RESERVED {
            let mut provider = OAuthProvider::google();
            provider
                .extra_authorize_params
                .push(((*name).to_owned(), "x".to_owned()));
            assert!(
                matches!(provider.endpoints(), Err(Error::Backend(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn a_providers_name_is_safe_inside_a_secrets_name() {
        for name in ["", "Micro soft", "a:b", "Google"] {
            let provider = OAuthProvider {
                name: name.to_owned(),
                ..OAuthProvider::google()
            };
            assert!(
                matches!(provider.endpoints(), Err(Error::Backend(_))),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_client_needs_an_id_and_never_prints_its_secret() {
        assert!(OAuthClient::public("  ").check().is_err());
        OAuthClient::public("client-1").check().unwrap();
        let client = OAuthClient::with_secret("client-1", "GOCSPX-not-for-printing");
        let shown = format!("{client:?}");
        assert!(shown.contains("client-1"));
        assert!(!shown.contains("GOCSPX"), "{shown}");
    }
}
