//! OAuth for the accounts in `config.toml`: which provider and application
//! an account signs in with, its tokens, and the Microsoft Graph backend
//! built on them.

use std::sync::Arc;

use async_trait::async_trait;
use quill_auth::{
    OAuthClient, OAuthHttp, OAuthProvider, OAuthTokens, SecretStore, SignInMethod, oauth_name,
};
use quill_core::config::{AccountConfig, AccountType, AuthMethod};
use quill_core::types::LocalAccountId;
use quill_core::{Error, TokenSource};
use quill_graph::{GraphBackend, GraphConfig};
use quill_sync::SyncAccount;

/// Who an account signs in with, and as which application.
#[derive(Debug, Clone)]
pub struct OAuthSetup {
    pub provider: OAuthProvider,
    pub client: OAuthClient,
}

impl OAuthSetup {
    /// The name the account's sign-in is kept under in the Keychain. It
    /// goes by the address, which stays when the account is renamed.
    pub fn secret_name(&self, account: &AccountConfig) -> String {
        oauth_name(&self.provider.name, &account.address)
    }
}

/// Whether the account signs in with OAuth and not a password.
pub fn wanted(account: &AccountConfig) -> bool {
    account.kind == AccountType::Graph || account.auth == AuthMethod::Oauth
}

/// The application Quill is registered as with Microsoft, which every
/// Microsoft account signs in through unless its config names another
/// (`oauth_client_id`).
///
/// The owner registered it on 2026-11-15 as a public client, for any
/// organisation's accounts and personal ones. It is not a secret: a
/// desktop application cannot keep one, and this id is in the address of
/// every sign-in page Quill opens. It says which application is asking,
/// and proves nothing.
pub const MICROSOFT_APPLICATION: &str = "1f673243-3634-498b-8b7f-2425ba6f2490";

/// The application Quill is registered as with Google, in a build that
/// was given one: its client id and the "client secret" Google gives a
/// desktop application with it.
///
/// It is not in the source. A build is given it in two variables,
/// `QUILL_GOOGLE_CLIENT_ID` and `QUILL_GOOGLE_CLIENT_SECRET`, as the
/// builds Quill is given out as are (`app/build-app.sh` reads them from a
/// file git ignores). The second is not a secret in the way a password is:
/// every copy of a desktop application carries it, and Google says so. It
/// is kept out of the source all the same, so that a build someone else
/// makes from it does not sign in to Google as Quill.
///
/// With it, every account that signs in with Google signs in through it
/// unless its config names another, as with [`MICROSOFT_APPLICATION`], and
/// the form that adds an account asks for nothing but the sign-in.
/// Without it (`None`: a build from the source alone) an account names an
/// application of its own, and the form asks for one.
pub const GOOGLE_APPLICATION: Option<(&str, &str)> = match (
    option_env!("QUILL_GOOGLE_CLIENT_ID"),
    option_env!("QUILL_GOOGLE_CLIENT_SECRET"),
) {
    (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Some((id, secret)),
    _ => None,
};

/// How the account signs in with OAuth, from its config: `None` for an
/// account that uses a password, and the reason for one whose settings
/// cannot work.
pub fn setup(account: &AccountConfig) -> Result<Option<OAuthSetup>, String> {
    setup_as(account, GOOGLE_APPLICATION)
}

/// [`setup`], with `google` as the application Quill is registered as
/// with Google, if it is: what the build was given, or what a test says.
fn setup_as(
    account: &AccountConfig,
    google: Option<(&'static str, &'static str)>,
) -> Result<Option<OAuthSetup>, String> {
    if !wanted(account) {
        return Ok(None);
    }
    // Quill is registered with Microsoft, and may be with Google.
    let mut own_application = None;
    let mut own_secret = None;
    let mut provider = match account.kind {
        AccountType::Graph => {
            own_application = Some(MICROSOFT_APPLICATION);
            match account.oauth_tenant.as_deref() {
                Some(tenant) => {
                    OAuthProvider::microsoft_tenant(tenant).map_err(|err| err.to_string())?
                }
                None => OAuthProvider::microsoft(),
            }
        }
        // Mail servers that take a token in place of a password: Gmail's.
        // A Microsoft 365 mailbox is a `graph` account instead.
        AccountType::Imap => {
            if account.oauth_tenant.is_some() {
                return Err(
                    "`oauth_tenant` is for Microsoft accounts, which are `type = \"graph\"`"
                        .to_owned(),
                );
            }
            if let Some((id, secret)) = google {
                own_application = Some(id);
                own_secret = Some(secret);
            }
            OAuthProvider::google()
        }
        other => {
            return Err(format!(
                "{} accounts do not sign in with OAuth",
                other.as_str()
            ));
        }
    };
    if let Some(addresses) = &account.oauth {
        if let Some(url) = &addresses.authorize_url {
            provider.authorize_url.clone_from(url);
        }
        if let Some(url) = &addresses.token_url {
            provider.token_url.clone_from(url);
        }
        if let Some(url) = &addresses.device_url {
            provider.device_url = Some(url.clone());
        }
    }
    let named = account
        .oauth_client_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    // Quill's own secret goes with Quill's own id, and with no other.
    if named.is_some() {
        own_secret = None;
    }
    let id = named
        .or(own_application)
        .ok_or(
            "it has no `oauth_client_id`: the id of the application registered for Quill with the provider (see docs/oauth.md)",
        )?;
    let secret = account
        .oauth_client_secret
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
        .or(own_secret);
    let client = match secret {
        Some(secret) => OAuthClient::with_secret(id, secret),
        None => OAuthClient::public(id),
    };
    Ok(Some(OAuthSetup { provider, client }))
}

/// An account's tokens, picked up from the Keychain when first asked for.
///
/// The account may not be signed in yet when Quill starts, and may be
/// signed in to while it runs. So nothing is read until a token is wanted,
/// and a refusal is forgotten: the next request looks in the Keychain
/// again and finds whatever a sign-in has put there since.
pub struct AccountTokens {
    http: OAuthHttp,
    setup: OAuthSetup,
    secrets: Arc<dyn SecretStore>,
    /// The mail address: what the sign-in is stored under.
    address: String,
    /// The account's name, to say which account needs signing in to.
    name: String,
    tokens: tokio::sync::Mutex<Option<Arc<OAuthTokens>>>,
}

impl AccountTokens {
    pub fn new(
        http: OAuthHttp,
        setup: OAuthSetup,
        secrets: Arc<dyn SecretStore>,
        account: &AccountConfig,
    ) -> Self {
        Self {
            http,
            setup,
            secrets,
            address: account.address.clone(),
            name: account.name.clone(),
            tokens: tokio::sync::Mutex::new(None),
        }
    }

    async fn current(&self) -> quill_core::Result<Arc<OAuthTokens>> {
        let mut held = self.tokens.lock().await;
        if let Some(tokens) = &*held {
            return Ok(Arc::clone(tokens));
        }
        let resumed = OAuthTokens::resume(
            &self.http,
            &self.setup.provider,
            &self.setup.client,
            Arc::clone(&self.secrets),
            &self.address,
        )
        .await
        .map_err(|err| match err {
            Error::Auth(_) => Error::Auth(not_signed_in(&self.name)),
            other => other,
        })?;
        let tokens = Arc::new(resumed);
        *held = Some(Arc::clone(&tokens));
        Ok(tokens)
    }
}

/// What to say, and do, about an account nobody has signed in to.
pub fn not_signed_in(name: &str) -> String {
    format!(
        "nobody has signed in to it yet. Use \"Sign in to an account\" in the palette or the File menu, or run: quill account sign-in \"{name}\""
    )
}

#[async_trait]
impl TokenSource for AccountTokens {
    async fn access_token(&self) -> quill_core::Result<String> {
        let tokens = self.current().await?;
        match tokens.access_token().await {
            Err(Error::Auth(reason)) => {
                // The provider no longer honours the sign-in. Start from
                // the Keychain next time, where a new one will be.
                *self.tokens.lock().await = None;
                Err(Error::Auth(reason))
            }
            other => other,
        }
    }

    async fn invalidate(&self) {
        let held = self.tokens.lock().await.clone();
        if let Some(tokens) = held {
            tokens.invalidate().await;
        }
    }
}

/// The account's tokens, as something its backends can ask: `None` for an
/// account that signs in with a password.
///
/// `http` is what the sign-in service is reached with. It is made when the
/// first account needs it and shared by the rest.
pub fn tokens(
    account: &AccountConfig,
    secrets: &Arc<dyn SecretStore>,
    http: &mut Option<OAuthHttp>,
) -> Result<Option<Arc<dyn TokenSource>>, String> {
    let Some(setup) = setup(account)? else {
        return Ok(None);
    };
    let http = match http {
        Some(http) => http.clone(),
        None => {
            let made =
                OAuthHttp::new(&quill_net::Trust::default()).map_err(|err| err.to_string())?;
            *http = Some(made.clone());
            made
        }
    };
    Ok(Some(Arc::new(AccountTokens::new(
        http,
        setup,
        Arc::clone(secrets),
        account,
    ))))
}

/// Everything the sync engine needs for a Microsoft 365 account: Graph
/// reads the mail and sends it.
pub fn graph_account(
    id: LocalAccountId,
    account: &AccountConfig,
    tokens: Arc<dyn TokenSource>,
) -> Result<SyncAccount, String> {
    let address = account
        .oauth
        .as_ref()
        .and_then(|addresses| addresses.graph_url.clone());
    let config = GraphConfig {
        base_url: address
            .clone()
            .unwrap_or_else(|| GraphConfig::BASE_URL.to_owned()),
        ..GraphConfig::default()
    };
    // Plain http is taken only for this computer itself, which the backend
    // checks: a stand-in for the service, for trying things out.
    let backend = if config.base_url.starts_with("http://") {
        GraphBackend::loopback(config, tokens)
    } else {
        GraphBackend::new(config, tokens)
    }
    .map_err(|err| err.to_string())?;
    let backend = Arc::new(backend);
    Ok(SyncAccount {
        id,
        name: account.name.clone(),
        backend: Arc::clone(&backend) as Arc<dyn quill_core::Backend>,
        sender: Some(backend),
        server_saves_sent: GraphBackend::SAVES_SENT_COPY,
        server_copies: quill_sync::ServerCopies::Keep,
    })
}

/// Signs the account in and keeps the sign-in in `secrets`. This waits for
/// the person, for minutes; dropping it gives up and keeps nothing.
pub async fn sign_in(
    http: &OAuthHttp,
    setup: &OAuthSetup,
    method: SignInMethod,
    secrets: Arc<dyn SecretStore>,
    account: &AccountConfig,
) -> quill_core::Result<()> {
    quill_auth::sign_in(
        http,
        &setup.provider,
        &setup.client,
        method,
        secrets,
        &account.address,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use quill_auth::MemorySecrets;
    use quill_core::config::Config;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn account(text: &str) -> AccountConfig {
        Config::parse(text).unwrap().accounts.remove(0)
    }

    const GRAPH: &str = "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n";

    #[test]
    fn a_graph_account_signs_in_with_microsoft() {
        let setup = setup(&account(GRAPH)).unwrap().unwrap();
        assert_eq!(setup.provider.name, "microsoft");
        assert!(setup.provider.authorize_url.contains("/common/"));
        assert_eq!(setup.client.id, "app-1");
        assert!(setup.client.secret.is_none());
        assert_eq!(
            setup.secret_name(&account(GRAPH)),
            "oauth:microsoft:robin@example.com"
        );

        let one = account(&format!("{GRAPH}oauth_tenant = \"example.com\"\n"));
        let setup = super::setup(&one).unwrap().unwrap();
        assert!(setup.provider.authorize_url.contains("/example.com/"));
    }

    #[test]
    fn an_imap_account_asks_for_oauth_and_gets_google() {
        let password = account(
            "[[account]]\nname = \"P\"\ntype = \"imap\"\naddress = \"robin@example.com\"\nimap = { host = \"imap.gmail.com\" }\n",
        );
        assert!(setup(&password).unwrap().is_none());

        let oauth = account(
            "[[account]]\nname = \"P\"\ntype = \"imap\"\naddress = \"robin@example.com\"\nauth = \"oauth\"\noauth_client_id = \"app-2\"\noauth_client_secret = \"not-really\"\nimap = { host = \"imap.gmail.com\" }\n",
        );
        let setup = setup(&oauth).unwrap().unwrap();
        assert_eq!(setup.provider.name, "google");
        assert_eq!(setup.client.secret.unwrap().expose(), "not-really");

        // In a build that was given Quill's own application, one that
        // names none signs in as Quill itself, with both halves of it; so
        // does one whose id is only spaces.
        let quills = Some(("quill.apps.googleusercontent.com", "the-other-half"));
        let unnamed = |id: &str| {
            account(&format!(
                "[[account]]\nname = \"Mail\"\ntype = \"imap\"\naddress = \"robin@example.com\"\n\
                 auth = \"oauth\"\n{id}imap = {{ host = \"imap.gmail.com\" }}\n"
            ))
        };
        for id in ["", "oauth_client_id = \"  \"\n"] {
            let setup = setup_as(&unnamed(id), quills).unwrap().unwrap();
            assert_eq!(setup.client.id, "quill.apps.googleusercontent.com");
            assert_eq!(setup.client.secret.unwrap().expose(), "the-other-half");
            // In a build from the source alone there is no such
            // application, and the account has to name one.
            assert!(
                setup_as(&unnamed(id), None)
                    .unwrap_err()
                    .contains("oauth_client_id")
            );
        }
        // An application of the user's own is not lent Quill's secret.
        let theirs = unnamed("oauth_client_id = \"app-2\"\n");
        for google in [quills, None] {
            let setup = setup_as(&theirs, google).unwrap().unwrap();
            assert_eq!(setup.client.id, "app-2");
            assert!(setup.client.secret.is_none());
        }
        // What this build was given, if anything, is both halves or none.
        if let Some((id, secret)) = GOOGLE_APPLICATION {
            assert!(!id.is_empty() && !secret.is_empty());
        }
    }

    #[test]
    fn settings_that_cannot_work_say_why() {
        let tenant = account(&format!("{GRAPH}oauth_tenant = \"not a tenant/\"\n"));
        assert!(setup(&tenant).unwrap_err().contains("tenant"));
        let pop = account(
            "[[account]]\nname = \"Old\"\ntype = \"pop3\"\naddress = \"robin@example.com\"\nauth = \"oauth\"\noauth_client_id = \"x\"\n",
        );
        assert!(setup(&pop).unwrap_err().contains("pop3"));
    }

    #[test]
    fn a_microsoft_account_signs_in_as_quill_unless_it_names_another_application() {
        // The id is a GUID, as Microsoft's are.
        assert_eq!(MICROSOFT_APPLICATION.len(), 36);
        assert!(
            MICROSOFT_APPLICATION
                .chars()
                .all(|letter| letter.is_ascii_hexdigit() || letter == '-')
        );
        for text in [
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\n",
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"  \"\n",
        ] {
            let found = setup(&account(text)).unwrap().unwrap();
            assert_eq!(found.client.id, MICROSOFT_APPLICATION, "{text}");
        }
        // One named in the config is the one used.
        let named = setup(&account(GRAPH)).unwrap().unwrap();
        assert_eq!(named.client.id, "app-1");
    }

    #[test]
    fn other_addresses_replace_the_usual_ones() {
        let cloud = account(&format!(
            "{GRAPH}\n[account.oauth]\nauthorize_url = \"https://login.example.com/authorize\"\ntoken_url = \"https://login.example.com/token\"\ngraph_url = \"https://graph.example.com/v1.0\"\n"
        ));
        let setup = setup(&cloud).unwrap().unwrap();
        assert_eq!(
            setup.provider.authorize_url,
            "https://login.example.com/authorize"
        );
        assert_eq!(setup.provider.token_url, "https://login.example.com/token");
        // The one not given stays Microsoft's.
        assert!(
            setup
                .provider
                .device_url
                .unwrap()
                .contains("microsoftonline")
        );
    }

    /// A stand-in for both the sign-in service and Graph, and an account
    /// pointed at it.
    async fn stand_in() -> (MockServer, AccountConfig) {
        let server = MockServer::start().await;
        let account = account(&format!(
            "{GRAPH}\n[account.oauth]\ntoken_url = \"{0}/token\"\ngraph_url = \"{0}/v1.0\"\n",
            server.uri()
        ));
        (server, account)
    }

    fn built(account: &AccountConfig, secrets: &Arc<dyn SecretStore>) -> SyncAccount {
        let tokens = tokens(account, secrets, &mut None).unwrap().unwrap();
        graph_account(LocalAccountId(1), account, tokens).unwrap()
    }

    #[tokio::test]
    async fn an_account_nobody_signed_in_to_says_how_to() {
        let (server, account) = stand_in().await;
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
        let built = built(&account, &secrets);
        let Err(Error::Auth(reason)) = built.backend.list_folders().await else {
            panic!("expected to be asked to sign in");
        };
        assert!(
            reason.contains("quill account sign-in \"Work\""),
            "{reason}"
        );
        // Nothing was asked of anyone.
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_sign_in_made_while_running_is_picked_up_and_used_for_graph() {
        let (server, account) = stand_in().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=kept-in-the-keychain"))
            .and(body_string_contains("client_id=app-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "for-graph",
                "token_type": "Bearer",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1.0/me/mailFolders"))
            .and(header("authorization", "Bearer for-graph"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "value": [] })),
            )
            .mount(&server)
            .await;
        // The by-name lookups for Inbox, Sent and the rest: none here.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": { "code": "ErrorItemNotFound", "message": "none" }
            })))
            .mount(&server)
            .await;

        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
        let built = built(&account, &secrets);
        assert!(matches!(
            built.backend.list_folders().await,
            Err(Error::Auth(_))
        ));
        // What a sign-in leaves behind.
        secrets
            .set("oauth:microsoft:robin@example.com", "kept-in-the-keychain")
            .unwrap();
        assert!(built.backend.list_folders().await.unwrap().is_empty());
        // One token served both; it was not asked for twice.
        assert!(built.backend.list_folders().await.unwrap().is_empty());
        assert!(built.sender.is_some());
        assert!(built.server_saves_sent);
    }

    #[test]
    fn graph_is_reached_over_https_or_not_at_all() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
        let elsewhere = account(&format!(
            "{GRAPH}\n[account.oauth]\ngraph_url = \"http://graph.example.com/v1.0\"\n"
        ));
        let tokens = tokens(&elsewhere, &secrets, &mut None).unwrap().unwrap();
        assert!(graph_account(LocalAccountId(1), &elsewhere, tokens).is_err());
    }
}
