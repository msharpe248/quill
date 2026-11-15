//! Signing in: which mechanism, and its exchange with the server.

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::{Error, Result};
use ring::rand::{SecureRandom, SystemRandom};

use crate::config::SmtpConfig;
use crate::connection::Connection;
use crate::reply::Reply;
use crate::scram;

/// A way of signing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// User name and password in one message (RFC 4616).
    Plain,
    /// User name and password in two. Obsolete, and the only thing some
    /// servers offer.
    Login,
    /// An OAuth access token, in Google's and Microsoft's own form.
    Xoauth2,
    /// An OAuth access token, in the standard form (RFC 7628).
    OAuthBearer,
    /// A password that is proved and never sent (RFC 7677).
    ScramSha256,
}

impl Mechanism {
    /// The name the protocol knows it by.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::Login => "LOGIN",
            Self::Xoauth2 => "XOAUTH2",
            Self::OAuthBearer => "OAUTHBEARER",
            Self::ScramSha256 => "SCRAM-SHA-256",
        }
    }
}

/// The secret an account signs in with.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Password(String),
    /// An OAuth access token, already fresh. Refreshing is the provider's
    /// business.
    OAuthToken(String),
}

impl fmt::Debug for Credential {
    /// Never the secret itself: this ends up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Password(_) => "Password(..)",
            Self::OAuthToken(_) => "OAuthToken(..)",
        })
    }
}

/// Where the secret comes from: the Keychain in the app, a constant in
/// tests. Asked for at each sign-in, once the channel is secure, and never
/// kept.
pub type CredentialProvider = Arc<dyn Fn() -> Result<Credential> + Send + Sync>;

/// The strongest mechanism among those `offered` that works with
/// `credential`. Mechanism names are compared in upper case.
///
/// For a password, strongest means the one that reveals least: SCRAM proves
/// the password without sending it, PLAIN and LOGIN send it. For a token,
/// the standard form is preferred to the vendors' own.
pub(crate) fn choose(offered: &[String], credential: &Credential) -> Option<Mechanism> {
    let preference: &[Mechanism] = match credential {
        Credential::Password(_) => &[Mechanism::ScramSha256, Mechanism::Plain, Mechanism::Login],
        Credential::OAuthToken(_) => &[Mechanism::OAuthBearer, Mechanism::Xoauth2],
    };
    preference
        .iter()
        .copied()
        .find(|mechanism| offered.iter().any(|name| name == mechanism.name()))
}

/// Signs in with `mechanism`. `Ok` means the server said yes.
pub(crate) async fn sign_in(
    connection: &mut Connection,
    mechanism: Mechanism,
    config: &SmtpConfig,
    credential: Credential,
) -> Result<()> {
    let user = config.username.as_str();
    match (mechanism, credential) {
        (Mechanism::Plain, Credential::Password(password)) => {
            let line = format!("AUTH PLAIN {}", BASE64.encode(plain(user, &password)));
            let reply = connection.command(&line, DOING).await?;
            accepted(reply)
        }
        (Mechanism::Login, Credential::Password(password)) => {
            let reply = connection.command("AUTH LOGIN", DOING).await?;
            challenge(reply)?;
            let reply = connection.command(&BASE64.encode(user), DOING).await?;
            challenge(reply)?;
            let reply = connection.command(&BASE64.encode(password), DOING).await?;
            accepted(reply)
        }
        (Mechanism::Xoauth2, Credential::OAuthToken(token)) => {
            let line = format!("AUTH XOAUTH2 {}", BASE64.encode(xoauth2(user, &token)));
            let reply = connection.command(&line, DOING).await?;
            // On failure the server first sends its reasons as a challenge,
            // and wants an empty line before it gives the real reply.
            token_outcome(connection, reply, "").await
        }
        (Mechanism::OAuthBearer, Credential::OAuthToken(token)) => {
            let message = oauthbearer(user, &config.host, config.port, &token);
            let line = format!("AUTH OAUTHBEARER {}", BASE64.encode(message));
            let reply = connection.command(&line, DOING).await?;
            // The same, but the line it wants is a single ^A ("AQ==").
            token_outcome(connection, reply, "AQ==").await
        }
        (Mechanism::ScramSha256, Credential::Password(password)) => {
            scram_sha_256(connection, user, &password).await
        }
        (mechanism, Credential::Password(_)) => Err(Error::Auth(format!(
            "{} needs an OAuth token, and this account signs in with a password",
            mechanism.name()
        ))),
        (mechanism, Credential::OAuthToken(_)) => Err(Error::Auth(format!(
            "{} needs a password, and this account signs in with an OAuth token",
            mechanism.name()
        ))),
    }
}

const DOING: &str = "the sign-in";

/// The reply that ends a sign-in: 235 is yes.
fn accepted(reply: Reply) -> Result<()> {
    if reply.code == 235 {
        Ok(())
    } else {
        Err(failed(reply))
    }
}

/// A reply in the middle of a sign-in must be a challenge (334). Returns
/// its text, which is base64.
fn challenge(reply: Reply) -> Result<String> {
    if reply.code == 334 {
        Ok(reply.lines.into_iter().next().unwrap_or_default())
    } else {
        Err(failed(reply))
    }
}

/// What a reply that did not accept the sign-in means. A 4xx is the
/// server's trouble and worth another try; a 5xx is about the credentials.
fn failed(reply: Reply) -> Error {
    match reply.code {
        400..500 => Error::Temporary(reply.text()),
        500..600 => Error::Auth(reply.text()),
        _ => Error::Backend(format!("unexpected reply to {DOING}: {}", reply.text())),
    }
}

/// Finishes a token sign-in. A refusal comes in two steps: a challenge
/// holding the server's reasons, answered with `acknowledge`, then the
/// reply proper.
async fn token_outcome(connection: &mut Connection, reply: Reply, acknowledge: &str) -> Result<()> {
    if reply.code != 334 {
        return accepted(reply);
    }
    let reply = connection.command(acknowledge, DOING).await?;
    accepted(reply)
}

async fn scram_sha_256(connection: &mut Connection, user: &str, password: &str) -> Result<()> {
    let client = scram::Client::new(user, password, &nonce()?);
    let line = format!(
        "AUTH SCRAM-SHA-256 {}",
        BASE64.encode(client.client_first())
    );
    let reply = connection.command(&line, DOING).await?;
    let server_first = decoded(&challenge(reply)?)?;

    // Stretching the password takes a moment by design. It is done on a
    // thread for blocking work so that nothing else waits on it.
    let answer = tokio::task::spawn_blocking(move || client.client_final(&server_first)).await;
    let (client_final, expected) = match answer {
        Ok(Ok(answer)) => answer,
        Ok(Err(err)) => {
            cancel(connection).await;
            return Err(err);
        }
        Err(_) => {
            cancel(connection).await;
            return Err(Error::Backend(
                "the sign-in could not be computed".to_owned(),
            ));
        }
    };

    let reply = connection
        .command(&BASE64.encode(client_final), DOING)
        .await?;
    match reply.code {
        334 => {}
        // Yes without the server's own proof is not yes: anything can say
        // it, including a server that never knew the password.
        235 => {
            return Err(Error::Auth(
                "SCRAM-SHA-256: the server did not prove it knows the password".to_owned(),
            ));
        }
        _ => return Err(failed(reply)),
    }
    let server_final = decoded(&challenge(reply)?)?;
    if let Err(err) = expected.verify(&server_final) {
        cancel(connection).await;
        return Err(err);
    }
    // The server's proof came as a challenge; an empty line answers it.
    let reply = connection.command("", DOING).await?;
    accepted(reply)
}

/// Calls off a sign-in part way through.
async fn cancel(connection: &mut Connection) {
    let _ = connection.command("*", DOING).await;
}

/// A challenge's base64, as text.
fn decoded(challenge: &str) -> Result<String> {
    BASE64
        .decode(challenge.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            Error::Backend("the server's sign-in challenge could not be read".to_owned())
        })
}

/// A fresh random nonce for SCRAM: 18 bytes, which base64 turns into 24
/// printable characters with no comma among them.
fn nonce() -> Result<String> {
    let mut bytes = [0u8; 18];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Backend("the system gave no random numbers".to_owned()))?;
    Ok(BASE64.encode(bytes))
}

/// RFC 4616: no authorisation identity, then the user, then the password.
fn plain(user: &str, password: &str) -> String {
    format!("\0{user}\0{password}")
}

fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// RFC 7628, section 3.1. The user goes in the GS2 header, where a comma
/// and an equals sign have to be escaped.
fn oauthbearer(user: &str, host: &str, port: u16, token: &str) -> String {
    let user = user.replace('=', "=3D").replace(',', "=2C");
    format!("n,a={user},\x01host={host}\x01port={port}\x01auth=Bearer {token}\x01\x01")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn password() -> Credential {
        Credential::Password("correct horse".to_owned())
    }

    fn token() -> Credential {
        Credential::OAuthToken("ya29.token".to_owned())
    }

    #[test]
    fn a_password_prefers_scram_then_plain_then_login() {
        let all = offered(&["LOGIN", "PLAIN", "XOAUTH2", "OAUTHBEARER", "SCRAM-SHA-256"]);
        assert_eq!(choose(&all, &password()), Some(Mechanism::ScramSha256));
        assert_eq!(
            choose(&offered(&["LOGIN", "PLAIN", "XOAUTH2"]), &password()),
            Some(Mechanism::Plain)
        );
        assert_eq!(
            choose(&offered(&["XOAUTH2", "LOGIN"]), &password()),
            Some(Mechanism::Login)
        );
    }

    #[test]
    fn a_token_prefers_oauthbearer_then_xoauth2() {
        let all = offered(&["LOGIN", "PLAIN", "XOAUTH2", "OAUTHBEARER", "SCRAM-SHA-256"]);
        assert_eq!(choose(&all, &token()), Some(Mechanism::OAuthBearer));
        assert_eq!(
            choose(&offered(&["PLAIN", "XOAUTH2"]), &token()),
            Some(Mechanism::Xoauth2)
        );
    }

    #[test]
    fn a_mechanism_the_credential_cannot_use_is_never_chosen() {
        assert_eq!(
            choose(&offered(&["XOAUTH2", "OAUTHBEARER"]), &password()),
            None
        );
        assert_eq!(
            choose(&offered(&["PLAIN", "LOGIN", "SCRAM-SHA-256"]), &token()),
            None
        );
        // Mechanisms Quill does not speak, and near misses.
        assert_eq!(
            choose(
                &offered(&["CRAM-MD5", "GSSAPI", "SCRAM-SHA-1", "SCRAM-SHA-256-PLUS"]),
                &password()
            ),
            None
        );
        assert_eq!(choose(&[], &password()), None);
    }

    #[test]
    fn the_initial_responses_have_the_documented_shape() {
        assert_eq!(
            plain("robin@example.com", "pass word"),
            "\0robin@example.com\0pass word"
        );
        // RFC 4616's own example.
        assert_eq!(
            BASE64.encode(plain("tim", "tanstaaftanstaaf")),
            "AHRpbQB0YW5zdGFhZnRhbnN0YWFm"
        );
        assert_eq!(
            xoauth2("robin@example.com", "ya29.token"),
            "user=robin@example.com\x01auth=Bearer ya29.token\x01\x01"
        );
        assert_eq!(
            oauthbearer("robin@example.com", "smtp.example.com", 587, "ya29.token"),
            "n,a=robin@example.com,\x01host=smtp.example.com\x01port=587\x01auth=Bearer ya29.token\x01\x01"
        );
        assert_eq!(
            oauthbearer("a=b,c", "h", 1, "t"),
            "n,a=a=3Db=2Cc,\x01host=h\x01port=1\x01auth=Bearer t\x01\x01"
        );
    }

    #[test]
    fn nonces_are_fresh_and_safe_to_put_in_a_scram_message() {
        let (first, second) = (nonce().unwrap(), nonce().unwrap());
        assert_ne!(first, second);
        assert_eq!(first.len(), 24);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b',')
        );
    }

    #[test]
    fn a_credential_does_not_print_its_secret() {
        assert_eq!(format!("{:?}", password()), "Password(..)");
        assert_eq!(format!("{:?}", token()), "OAuthToken(..)");
    }

    #[test]
    fn sign_in_failures_are_sorted_by_whose_fault_they_are() {
        let reply = |code| Reply {
            code,
            lines: vec!["5.7.8 words".to_owned()],
        };
        assert!(matches!(failed(reply(535)), Error::Auth(text) if text == "535 5.7.8 words"));
        assert!(matches!(failed(reply(504)), Error::Auth(_)));
        assert!(matches!(failed(reply(454)), Error::Temporary(_)));
        assert!(matches!(failed(reply(250)), Error::Backend(_)));
        assert!(accepted(reply(235)).is_ok());
        assert!(matches!(challenge(reply(334)), Ok(text) if text == "5.7.8 words"));
        assert!(matches!(challenge(reply(535)), Err(Error::Auth(_))));
    }
}
