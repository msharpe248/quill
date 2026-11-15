//! Signing in without a browser on this computer: the device code flow
//! (RFC 8628).
//!
//! Over SSH there is no browser to open and nothing a browser on another
//! machine could be redirected to. Instead the provider gives a short code;
//! the user types it into the provider's page on any device, and Quill asks
//! the token server every few seconds whether they have done so yet.

use std::fmt;
use std::time::Duration;

use quill_core::{Error, Result};
use tokio::time::{Instant, sleep};

use super::code::refused;
use super::http::{Fields, Grant, OAuthHttp, Refusal, seconds};
use super::provider::{Endpoints, OAuthClient, OAuthProvider};
use super::secret::{Secret, words};

/// How often to ask when the provider does not say (RFC 8628, section 3.2).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// How much slower to ask each time the provider says `slow_down` (RFC
/// 8628, section 3.5).
const SLOW_DOWN: Duration = Duration::from_secs(5);

/// The longest gap between questions, whatever the provider says. Past
/// this the user would finish and then sit waiting.
const LONGEST_INTERVAL: Duration = Duration::from_secs(60);

/// How long a code is taken to last when the provider does not say, and
/// the longest one is believed to. Both real providers say fifteen minutes.
const ASSUMED_CODE_LIFE: Duration = Duration::from_secs(15 * 60);
const LONGEST_CODE_LIFE: Duration = Duration::from_secs(30 * 60);

/// Shows the user where to go and what to type there. It is called once,
/// before the waiting starts.
pub type ShowDeviceCode = Box<dyn FnOnce(&DevicePrompt) + Send>;

/// A sign-in by a code typed into the provider's page on another device.
pub struct DeviceSignIn {
    pub show: ShowDeviceCode,
}

impl DeviceSignIn {
    pub fn new(show: impl FnOnce(&DevicePrompt) + Send + 'static) -> Self {
        Self {
            show: Box::new(show),
        }
    }
}

impl fmt::Debug for DeviceSignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceSignIn").finish_non_exhaustive()
    }
}

/// What the user needs to finish signing in on another device. All of it is
/// the provider's text with control characters removed, so it can be drawn
/// in a terminal as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePrompt {
    /// The page to open.
    pub verification_uri: String,
    /// The same page with the code already in it, when the provider gives
    /// one. Good for a QR code; the user should still compare the code.
    pub verification_uri_complete: Option<String>,
    /// The code to type there. It is meant to be seen: it is no use to
    /// anyone without the device code that Quill keeps to itself.
    pub user_code: String,
    /// The provider's own instructions, when it gives them. Microsoft does.
    pub message: Option<String>,
    /// How long the code is good for.
    pub expires_in: Duration,
}

/// A device authorization in progress.
pub(crate) struct DeviceAuthorization {
    /// What the token server is asked with. Whoever has it gets the tokens
    /// once the user approves, so it is a secret.
    device_code: Secret,
    pub prompt: DevicePrompt,
    interval: Duration,
    /// When the code stops being good.
    deadline: Instant,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("prompt", &self.prompt)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// Asks the provider for a device code and a user code.
pub(crate) async fn start(
    http: &OAuthHttp,
    provider: &OAuthProvider,
    endpoints: &Endpoints,
    client: &OAuthClient,
) -> Result<DeviceAuthorization> {
    let Some(url) = &endpoints.device else {
        return Err(Error::Unsupported(format!(
            "{} does not offer sign-in by code. Sign in on a computer with a browser",
            provider.name
        )));
    };
    let scope = provider.scopes.join(" ");
    let mut form = Vec::new();
    if !scope.is_empty() {
        form.push(("scope", scope.as_str()));
    }
    let fields = http.post(url, client, &form).await.map_err(refused)?;
    let host = url.host_str().unwrap_or("the provider");
    DeviceAuthorization::read(host, &fields)
}

impl DeviceAuthorization {
    fn read(host: &str, fields: &Fields) -> Result<Self> {
        // Google's older answers call the address `verification_url`.
        let (Some(device_code), Some(user_code), Some(verification_uri)) = (
            fields.text("device_code"),
            fields.text("user_code"),
            fields
                .text("verification_uri")
                .or_else(|| fields.text("verification_url")),
        ) else {
            return Err(Error::Temporary(format!(
                "{host} gave an answer with no code to sign in with"
            )));
        };
        let expires_in = seconds(fields.get("expires_in")).map_or(ASSUMED_CODE_LIFE, |life| {
            Duration::from_secs_f64(life.clamp(0.0, LONGEST_CODE_LIFE.as_secs_f64()))
        });
        // A pace of nothing would be a flood.
        let interval = seconds(fields.get("interval")).map_or(DEFAULT_INTERVAL, |pace| {
            Duration::from_secs_f64(pace.clamp(1.0, LONGEST_INTERVAL.as_secs_f64()))
        });
        Ok(Self {
            device_code: Secret::new(device_code),
            prompt: DevicePrompt {
                verification_uri: words(verification_uri),
                verification_uri_complete: fields.text("verification_uri_complete").map(words),
                user_code: words(user_code),
                message: fields.text("message").map(words),
                expires_in,
            },
            interval,
            deadline: Instant::now() + expires_in,
        })
    }
}

/// Asks the token server, at the pace it sets, until the user has approved
/// or refused, or the code has expired.
///
/// Dropping the future stops the asking.
pub(crate) async fn poll(
    http: &OAuthHttp,
    endpoints: &Endpoints,
    client: &OAuthClient,
    authorization: &DeviceAuthorization,
) -> Result<Grant> {
    let mut interval = authorization.interval;
    // Trouble reaching the server does not end the sign-in: the user may be
    // half way through typing the code. It is what is reported if the code
    // runs out before the server is heard from again.
    let mut trouble: Option<Error> = None;
    loop {
        if Instant::now() + interval >= authorization.deadline {
            return Err(trouble.unwrap_or_else(expired));
        }
        sleep(interval).await;
        let asked = http
            .token(
                &endpoints.token,
                client,
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", authorization.device_code.expose()),
                ],
            )
            .await;
        match asked {
            Ok(grant) => return Ok(grant),
            Err(Refusal::Provider(said)) => match said.code.as_str() {
                "authorization_pending" => trouble = None,
                "slow_down" => {
                    trouble = None;
                    interval = (interval + SLOW_DOWN).min(LONGEST_INTERVAL);
                }
                "expired_token" | "code_expired" => return Err(expired()),
                // Microsoft's word for it is `authorization_declined`.
                "access_denied" | "authorization_declined" => {
                    return Err(Error::Auth(format!(
                        "the sign-in was refused on the other device ({})",
                        said.words()
                    )));
                }
                _ => return Err(refused(Refusal::Provider(said))),
            },
            Err(Refusal::Other(err @ (Error::Connection(_) | Error::Temporary(_)))) => {
                trouble = Some(err);
            }
            Err(Refusal::Other(err)) => return Err(err),
        }
    }
}

fn expired() -> Error {
    Error::Auth(
        "the code expired before anyone signed in with it. Start again for a new one".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::oauth::http::Fields;

    #[test]
    fn a_device_answer_is_read_in_either_providers_spelling() {
        let microsoft = DeviceAuthorization::read(
            "login.example.com",
            &Fields::from_json(json!({
                "device_code": "device-DDDD",
                "user_code": "HJKL-MNPQ",
                "verification_uri": "https://login.example.com/device",
                "expires_in": 900,
                "interval": 5,
                "message": "To sign in, open https://login.example.com/device\r\nand enter HJKL-MNPQ.\x1b[2J",
            })),
        )
        .unwrap();
        assert_eq!(
            microsoft.prompt,
            DevicePrompt {
                verification_uri: "https://login.example.com/device".to_owned(),
                verification_uri_complete: None,
                user_code: "HJKL-MNPQ".to_owned(),
                message: Some(
                    "To sign in, open https://login.example.com/device and enter HJKL-MNPQ. [2J"
                        .to_owned()
                ),
                expires_in: Duration::from_secs(900),
            }
        );
        assert_eq!(microsoft.interval, Duration::from_secs(5));
        let shown = format!("{microsoft:?}");
        assert!(shown.contains("HJKL-MNPQ"), "{shown}");
        assert!(!shown.contains("device-DDDD"), "{shown}");

        let google = DeviceAuthorization::read(
            "login.example.com",
            &Fields::from_json(json!({
                "device_code": "device-DDDD",
                "user_code": "HJKL-MNPQ",
                "verification_url": "https://login.example.com/device",
                "verification_uri_complete": "https://login.example.com/device?user_code=HJKL-MNPQ",
            })),
        )
        .unwrap();
        assert_eq!(
            google.prompt.verification_uri,
            "https://login.example.com/device"
        );
        assert_eq!(
            google.prompt.verification_uri_complete.as_deref(),
            Some("https://login.example.com/device?user_code=HJKL-MNPQ")
        );
        // Nothing said about pace or expiry: the RFC's pace, a usual expiry.
        assert_eq!(google.interval, DEFAULT_INTERVAL);
        assert_eq!(google.prompt.expires_in, ASSUMED_CODE_LIFE);
    }

    #[test]
    fn a_device_answers_pace_and_expiry_are_kept_within_reason() {
        let read = |interval: serde_json::Value, expires_in: serde_json::Value| {
            DeviceAuthorization::read(
                "login.example.com",
                &Fields::from_json(json!({
                    "device_code": "device-DDDD",
                    "user_code": "HJKL-MNPQ",
                    "verification_uri": "https://login.example.com/device",
                    "interval": interval,
                    "expires_in": expires_in,
                })),
            )
            .unwrap()
        };
        // A pace of nothing would be a flood.
        assert_eq!(read(json!(0), json!(900)).interval, Duration::from_secs(1));
        assert_eq!(read(json!(-5), json!(900)).interval, Duration::from_secs(1));
        assert_eq!(read(json!(3600), json!(900)).interval, LONGEST_INTERVAL);
        assert_eq!(
            read(json!("10"), json!(900)).interval,
            Duration::from_secs(10)
        );
        assert_eq!(
            read(json!("often"), json!("a while")).interval,
            DEFAULT_INTERVAL
        );
        assert_eq!(
            read(json!(null), json!("a while")).prompt.expires_in,
            ASSUMED_CODE_LIFE
        );
        assert_eq!(
            read(json!(5), json!(86_400)).prompt.expires_in,
            LONGEST_CODE_LIFE
        );
        assert_eq!(read(json!(5), json!(0)).prompt.expires_in, Duration::ZERO);
    }

    #[test]
    fn a_device_answer_without_its_codes_is_not_one() {
        for answer in [
            json!({}),
            json!({"user_code": "HJKL-MNPQ", "verification_uri": "https://login.example.com/device"}),
            json!({"device_code": "device-DDDD", "verification_uri": "https://login.example.com/device"}),
            json!({"device_code": "device-DDDD", "user_code": "HJKL-MNPQ"}),
        ] {
            let err = DeviceAuthorization::read("login.example.com", &Fields::from_json(answer))
                .unwrap_err();
            assert!(matches!(err, Error::Temporary(_)), "{err:?}");
            assert!(!format!("{err:?}").contains("device-DDDD"));
        }
    }
}
