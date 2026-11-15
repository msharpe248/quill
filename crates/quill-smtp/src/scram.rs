//! SCRAM-SHA-256 (RFC 5802, RFC 7677), the client's side.
//!
//! The password never crosses the wire. Each side proves it knows the
//! password to the other, so a server that only pretends to check it is
//! caught as well. Channel binding is not used.
//!
//! Nothing here touches the network: these are the messages and the sums,
//! which the sign-in code carries back and forth.

use std::num::NonZeroU32;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::{Error, Result};
use ring::{digest, hmac, pbkdf2};

/// The most rounds of key stretching a server may ask for. Real servers ask
/// for thousands to hundreds of thousands; a far larger number is a way to
/// keep this machine busy for hours.
const MAX_ITERATIONS: u32 = 10_000_000;

/// The start of every client-first message: no channel binding, no
/// authorisation identity. "biws" is its base64, sent back in the final
/// message.
const GS2_HEADER: &str = "n,,";

/// The client's side of one sign-in.
pub(crate) struct Client {
    password: String,
    nonce: String,
    client_first_bare: String,
}

/// What the server must sign to prove it knows the password too.
pub(crate) struct Expected {
    server_key: hmac::Key,
    auth_message: String,
}

impl Client {
    /// `nonce` is this sign-in's random string, printable and without
    /// commas.
    pub fn new(username: &str, password: &str, nonce: &str) -> Self {
        Self {
            password: prepared(password),
            nonce: nonce.to_owned(),
            client_first_bare: format!("n={},r={nonce}", sasl_name(&prepared(username))),
        }
    }

    /// The first message: who is signing in, and our nonce.
    pub fn client_first(&self) -> String {
        format!("{GS2_HEADER}{}", self.client_first_bare)
    }

    /// Answers the server's first message with the proof that we know the
    /// password. Returns that final message and what the server must reply.
    ///
    /// This is where the key stretching happens, so it takes a while and
    /// belongs off the async threads.
    pub fn client_final(&self, server_first: &str) -> Result<(String, Expected)> {
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for (index, attribute) in server_first.split(',').enumerate() {
            match attribute.split_once('=') {
                // An extension we must understand to go on, and do not.
                Some(("m", _)) if index == 0 => {
                    return Err(refused("the server requires a SCRAM extension"));
                }
                Some(("e", error)) if index == 0 => {
                    return Err(Error::Auth(format!("the server refused: {error}")));
                }
                Some(("r", value)) => nonce = Some(value),
                Some(("s", value)) => salt = BASE64.decode(value).ok(),
                Some(("i", value)) => iterations = value.parse::<u32>().ok(),
                _ => {}
            }
        }
        let (Some(nonce), Some(salt), Some(iterations)) = (nonce, salt, iterations) else {
            return Err(refused("the server's challenge is incomplete"));
        };
        // The server's nonce must build on ours, or this is an answer to
        // somebody else's sign-in.
        if nonce.len() <= self.nonce.len() || !nonce.starts_with(&self.nonce) {
            return Err(refused("the server's challenge is not for this sign-in"));
        }
        let iterations = NonZeroU32::new(iterations)
            .filter(|count| count.get() <= MAX_ITERATIONS)
            .ok_or_else(|| refused("the server asked for an unreasonable amount of work"))?;

        let mut salted_password = [0u8; digest::SHA256_OUTPUT_LEN];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            iterations,
            &salt,
            self.password.as_bytes(),
            &mut salted_password,
        );
        let client_key = hmac_sha256(&salted_password, b"Client Key");
        let stored_key = digest::digest(&digest::SHA256, &client_key);
        let server_key = hmac::Key::new(
            hmac::HMAC_SHA256,
            &hmac_sha256(&salted_password, b"Server Key"),
        );

        let without_proof = format!("c={},r={nonce}", BASE64.encode(GS2_HEADER));
        let auth_message = format!("{},{server_first},{without_proof}", self.client_first_bare);
        let client_signature = hmac_sha256(stored_key.as_ref(), auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(&client_signature)
            .map(|(key, signature)| key ^ signature)
            .collect();

        Ok((
            format!("{without_proof},p={}", BASE64.encode(proof)),
            Expected {
                server_key,
                auth_message,
            },
        ))
    }
}

impl Expected {
    /// Checks the server's final message. An error means the server does
    /// not know the password, whatever it says about ours.
    pub fn verify(&self, server_final: &str) -> Result<()> {
        let first = server_final.split(',').next().unwrap_or_default();
        match first.split_once('=') {
            Some(("v", value)) => {
                let signature = BASE64.decode(value).unwrap_or_default();
                // `verify` compares in constant time, as a secret should be.
                hmac::verify(&self.server_key, self.auth_message.as_bytes(), &signature)
                    .map_err(|_| refused("the server could not prove it knows the password"))
            }
            Some(("e", error)) => Err(Error::Auth(format!("the server refused: {error}"))),
            _ => Err(refused("the server's final message could not be read")),
        }
    }
}

fn refused(why: &str) -> Error {
    Error::Auth(format!("SCRAM-SHA-256: {why}"))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data)
        .as_ref()
        .to_vec()
}

/// A name or password as SCRAM wants it: normalised with SASLprep, so the
/// same password typed two ways gives the same key. A string SASLprep
/// rejects is used as it is; the server will judge it.
fn prepared(text: &str) -> String {
    match stringprep::saslprep(text) {
        Ok(prepared) => prepared.into_owned(),
        Err(_) => text.to_owned(),
    }
}

/// A user name inside a SCRAM message, where a comma and an equals sign
/// have meanings of their own.
fn sasl_name(name: &str) -> String {
    name.replace('=', "=3D").replace(',', "=2C")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The example exchange from RFC 7677, section 3.
    const NONCE: &str = "rOprNGfwEbeRWgbNEkqO";
    const SERVER_FIRST: &str = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
                                s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
    const CLIENT_FINAL: &str = "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
                                p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=";
    const SERVER_FINAL: &str = "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=";

    #[test]
    fn the_rfc_7677_exchange_is_reproduced() {
        let client = Client::new("user", "pencil", NONCE);
        assert_eq!(client.client_first(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let (client_final, expected) = client.client_final(SERVER_FIRST).unwrap();
        assert_eq!(client_final, CLIENT_FINAL);
        expected.verify(SERVER_FINAL).unwrap();
    }

    #[test]
    fn a_wrong_password_gives_a_different_proof() {
        let client = Client::new("user", "pencils", NONCE);
        let (client_final, expected) = client.client_final(SERVER_FIRST).unwrap();
        assert_ne!(client_final, CLIENT_FINAL);
        // And the real server's signature no longer matches ours.
        assert!(matches!(expected.verify(SERVER_FINAL), Err(Error::Auth(_))));
    }

    #[test]
    fn a_server_that_cannot_sign_is_refused() {
        let client = Client::new("user", "pencil", NONCE);
        let (_, expected) = client.client_final(SERVER_FIRST).unwrap();
        for server_final in [
            "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G5=",
            "v=",
            "v=not base64",
            "",
            "ok",
        ] {
            assert!(
                matches!(expected.verify(server_final), Err(Error::Auth(_))),
                "{server_final:?}"
            );
        }
        match expected.verify("e=invalid-proof") {
            Err(Error::Auth(text)) => assert!(text.contains("invalid-proof"), "{text}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_challenge_for_another_nonce_is_refused() {
        let client = Client::new("user", "pencil", NONCE);
        for server_first in [
            // Not our nonce at all.
            "r=somebodyElse%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
            // Ours, with nothing of the server's added.
            "r=rOprNGfwEbeRWgbNEkqO,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
        ] {
            assert!(
                matches!(client.client_final(server_first), Err(Error::Auth(_))),
                "{server_first}"
            );
        }
    }

    #[test]
    fn a_broken_or_abusive_challenge_is_refused() {
        let client = Client::new("user", "pencil", NONCE);
        for server_first in [
            "",
            "r=rOprNGfwEbeRWgbNEkqOabc",
            "r=rOprNGfwEbeRWgbNEkqOabc,s=W22ZaJ0SNY7soEsUEjb6gQ==",
            "r=rOprNGfwEbeRWgbNEkqOabc,s=!!!,i=4096",
            "r=rOprNGfwEbeRWgbNEkqOabc,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=0",
            "r=rOprNGfwEbeRWgbNEkqOabc,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=many",
            "r=rOprNGfwEbeRWgbNEkqOabc,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4000000000",
            "m=ext,r=rOprNGfwEbeRWgbNEkqOabc,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
            "e=unknown-user",
        ] {
            assert!(
                matches!(client.client_final(server_first), Err(Error::Auth(_))),
                "{server_first}"
            );
        }
    }

    #[test]
    fn commas_and_equals_signs_in_a_name_are_escaped() {
        let client = Client::new("a=b,c@example.com", "pencil", "nonce");
        assert_eq!(client.client_first(), "n,,n=a=3Db=2Cc@example.com,r=nonce");
    }

    #[test]
    fn passwords_are_normalised_before_use() {
        // SASLprep: a soft hyphen is dropped, and compatibility forms are
        // folded (the Roman numeral "Ⅸ" becomes "IX"). RFC 4013, section 3.
        assert_eq!(prepared("I\u{ad}X"), "IX");
        assert_eq!(prepared("\u{2168}"), "IX");
        assert_eq!(prepared("pencil"), "pencil");
        // Forbidden by SASLprep (a control character): used as typed.
        assert_eq!(prepared("a\u{7}b"), "a\u{7}b");

        let typed = Client::new("user", "I\u{ad}X", NONCE);
        let plain = Client::new("user", "IX", NONCE);
        assert_eq!(
            typed.client_final(SERVER_FIRST).unwrap().0,
            plain.client_final(SERVER_FIRST).unwrap().0
        );
    }
}
