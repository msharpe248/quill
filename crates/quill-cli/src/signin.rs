//! `quill account sign-in`: signing an account in to its provider from a
//! terminal, in a browser on this Mac or with a code on another device.

use std::sync::Arc;

use anyhow::Context;
use quill_auth::{BrowserSignIn, DevicePrompt, DeviceSignIn, OAuthHttp, SecretStore, SignInMethod};
use quill_core::config::AccountConfig;
use quill_net::Trust;

use crate::oauth;

/// Where the words go, and how a browser is opened.
pub struct Terminal {
    /// Prints one line for the person signing in.
    pub say: Arc<dyn Fn(String) + Send + Sync>,
    /// Opens an address in the browser. It has been printed already, for
    /// when this does nothing.
    pub open: Box<dyn FnOnce(&str) + Send>,
    /// Whether this is a session from another computer, where a browser
    /// opened here would be no use to the person typing.
    pub remote: bool,
}

/// Signs `account` in and keeps the sign-in in `secrets`.
///
/// `by_code` asks for the code-on-another-device way even at the Mac
/// itself. Over SSH it is used without asking, where the provider has it.
pub async fn sign_in(
    account: &AccountConfig,
    secrets: Arc<dyn SecretStore>,
    by_code: bool,
    terminal: Terminal,
) -> anyhow::Result<()> {
    let name = &account.name;
    let setup = oauth::setup(account)
        .map_err(|reason| anyhow::anyhow!("{name} cannot sign in: {reason}"))?
        .with_context(|| {
            format!("{name} signs in with a password: quill account password \"{name}\"")
        })?;
    let http = OAuthHttp::new(&Trust::default())?;
    let has_code = setup.provider.device_url.is_some();
    anyhow::ensure!(
        has_code || !by_code,
        "this provider has no sign-in by code. Sign in at the Mac itself, or use an app password"
    );
    let say = terminal.say;
    let method = if by_code || (terminal.remote && has_code) {
        let say = Arc::clone(&say);
        SignInMethod::DeviceCode(DeviceSignIn::new(move |prompt: &DevicePrompt| {
            say(format!(
                "To sign in, open {} on any device and enter the code {}",
                prompt.verification_uri, prompt.user_code
            ));
            say(format!(
                "Waiting for you to finish there, for up to {} minutes. ctrl-c gives up.",
                prompt.expires_in.as_secs().div_ceil(60)
            ));
        }))
    } else {
        let say = Arc::clone(&say);
        let open = terminal.open;
        let mut browser = BrowserSignIn::new(move |url: &str| {
            say("Sign in in your browser. If it has not opened, go to:".to_owned());
            say(url.to_owned());
            say("Waiting for you to finish there. ctrl-c gives up.".to_owned());
            // Only ever a web address, whatever a provider description says.
            if quill_core::links::is_openable(url) {
                open(url);
            }
            Ok(())
        });
        // Fills in the address on the provider's page.
        browser.login_hint = Some(account.address.clone());
        SignInMethod::Browser(browser)
    };
    oauth::sign_in(&http, &setup, method, secrets, account).await?;
    say(format!(
        "Signed in to {name}. The sign-in is kept in the Keychain."
    ));
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Mutex;

    use quill_auth::MemorySecrets;
    use quill_core::config::Config;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn account(text: &str) -> AccountConfig {
        Config::parse(text).unwrap().accounts.remove(0)
    }

    struct Said(Arc<Mutex<Vec<String>>>);

    impl Said {
        fn new() -> Self {
            Self(Arc::default())
        }

        fn terminal(&self, remote: bool, open: impl FnOnce(&str) + Send + 'static) -> Terminal {
            let lines = Arc::clone(&self.0);
            Terminal {
                say: Arc::new(move |line| lines.lock().unwrap().push(line)),
                open: Box::new(open),
                remote,
            }
        }

        fn text(&self) -> String {
            self.0.lock().unwrap().join("\n")
        }
    }

    async fn provider() -> (MockServer, AccountConfig) {
        let server = MockServer::start().await;
        let account = account(&format!(
            "[[account]]\nname = \"Work\"\ntype = \"graph\"\naddress = \"robin@example.com\"\noauth_client_id = \"app-1\"\n\n[account.oauth]\nauthorize_url = \"{0}/authorize\"\ntoken_url = \"{0}/token\"\ndevice_url = \"{0}/devicecode\"\n",
            server.uri()
        ));
        (server, account)
    }

    #[tokio::test]
    async fn over_ssh_the_code_is_shown_and_the_sign_in_kept() {
        let (server, account) = provider().await;
        Mock::given(method("POST"))
            .and(path("/devicecode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "long-and-private",
                "user_code": "WXYZ-1234",
                "verification_uri": "https://login.example.com/device",
                "expires_in": 600,
                "interval": 1,
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("device_code=long-and-private"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "short-lived",
                "refresh_token": "to-keep",
                "token_type": "Bearer",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;
        let secrets = Arc::new(MemorySecrets::default());
        let said = Said::new();
        let opened = Arc::new(Mutex::new(false));
        let terminal = said.terminal(true, {
            let opened = Arc::clone(&opened);
            move |_| *opened.lock().unwrap() = true
        });
        sign_in(&account, secrets.clone(), false, terminal)
            .await
            .unwrap();
        let text = said.text();
        assert!(
            text.contains(
                "open https://login.example.com/device on any device and enter the code WXYZ-1234"
            ),
            "{text}"
        );
        assert!(text.contains("Signed in to Work."), "{text}");
        // Nothing private was printed, no browser was opened on a Mac
        // nobody is sitting at, and the sign-in is where it belongs.
        assert!(!text.contains("long-and-private") && !text.contains("to-keep"));
        assert!(!*opened.lock().unwrap());
        assert_eq!(
            secrets.get("oauth:microsoft:robin@example.com").unwrap(),
            Some("to-keep".to_owned())
        );
    }

    #[tokio::test]
    async fn at_the_mac_the_browser_is_opened_and_its_answer_taken() {
        let (server, account) = provider().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code=from-the-browser"))
            .and(body_string_contains("code_verifier="))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "short-lived",
                "refresh_token": "to-keep",
                "token_type": "Bearer",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;
        let secrets = Arc::new(MemorySecrets::default());
        let said = Said::new();
        // The browser's part: the provider sends it back to Quill's
        // listener with a code and the state it was given.
        let terminal = said.terminal(false, |url: &str| {
            let query = url.split_once('?').unwrap().1.to_owned();
            let value = |name: &str| -> String {
                let pair = query
                    .split('&')
                    .find(|pair| pair.starts_with(&format!("{name}=")))
                    .unwrap();
                pair.split_once('=').unwrap().1.to_owned()
            };
            assert_eq!(value("login_hint"), "robin%40example.com");
            let back = value("redirect_uri")
                .replace("%3A", ":")
                .replace("%2F", "/");
            let state = value("state");
            tokio::spawn(async move {
                let address = back.strip_prefix("http://").unwrap().replace("localhost", "127.0.0.1");
                let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                stream
                    .write_all(
                        format!(
                            "GET /?code=from-the-browser&state={state} HTTP/1.1\r\nHost: {address}\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut answer = Vec::new();
                let _ = stream.read_to_end(&mut answer).await;
            });
        });
        sign_in(&account, secrets.clone(), false, terminal)
            .await
            .unwrap();
        let text = said.text();
        assert!(text.contains("/authorize?"), "{text}");
        assert!(text.contains("Signed in to Work."), "{text}");
        assert_eq!(
            secrets.get("oauth:microsoft:robin@example.com").unwrap(),
            Some("to-keep".to_owned())
        );
    }

    #[tokio::test]
    async fn an_account_with_a_password_is_pointed_at_the_right_command() {
        let account = account(
            "[[account]]\nname = \"Personal\"\ntype = \"imap\"\naddress = \"robin@example.com\"\nimap = { host = \"mail.example.com\" }\n",
        );
        let said = Said::new();
        let err = sign_in(
            &account,
            Arc::new(MemorySecrets::default()),
            false,
            said.terminal(false, |_| {}),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("quill account password \"Personal\""),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_provider_without_codes_says_so_before_anything_is_asked() {
        let account = account(
            "[[account]]\nname = \"Mail\"\ntype = \"imap\"\naddress = \"robin@example.com\"\nauth = \"oauth\"\noauth_client_id = \"app-2\"\nimap = { host = \"imap.gmail.com\" }\n",
        );
        let said = Said::new();
        let err = sign_in(
            &account,
            Arc::new(MemorySecrets::default()),
            true,
            said.terminal(false, |_| {}),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no sign-in by code"), "{err}");
        assert!(said.text().is_empty());
    }
}
