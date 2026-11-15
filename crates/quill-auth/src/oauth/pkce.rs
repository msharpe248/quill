//! The two random values a browser sign-in is held together by.
//!
//! The verifier (RFC 7636) proves that whoever redeems the authorization
//! code is whoever asked for it: only its hash goes through the browser,
//! and the verifier itself goes straight to the token server. The state
//! proves that the redirect answers the request Quill made, and not one a
//! web page made for it.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use quill_core::{Error, Result};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest, hmac};

use super::secret::Secret;

/// A verifier and the challenge made from it.
pub(crate) struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Result<Self> {
        let verifier = random_token()?;
        let challenge = challenge_for(&verifier);
        Ok(Self {
            verifier: Secret::new(verifier),
            challenge,
        })
    }
}

impl fmt::Debug for Pkce {
    /// The challenge is public; the verifier is the secret behind it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// The S256 challenge for a verifier: its SHA-256, in base64url without
/// padding.
pub(crate) fn challenge_for(verifier: &str) -> String {
    BASE64URL.encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
}

/// 256 random bits as 43 characters of base64url. That is the shortest
/// verifier RFC 7636 allows and all the entropy it asks for, in an alphabet
/// that needs no escaping in a URL.
pub(crate) fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Backend("the system has no random numbers to give".to_owned()))?;
    Ok(BASE64URL.encode(bytes))
}

/// Recognises the state sent with a sign-in when it comes back.
///
/// What is kept is a keyed hash of the state, and what is compared is the
/// keyed hash of the value received. The comparison takes the same time
/// wherever the two differ, so a local program guessing at the listener
/// learns nothing from how long a refusal takes.
pub(crate) struct StateCheck {
    key: hmac::Key,
    tag: hmac::Tag,
}

impl StateCheck {
    pub fn new(state: &str) -> Result<Self> {
        let key = hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new())
            .map_err(|_| Error::Backend("the system has no random numbers to give".to_owned()))?;
        let tag = hmac::sign(&key, state.as_bytes());
        Ok(Self { key, tag })
    }

    pub fn matches(&self, received: &str) -> bool {
        hmac::verify(&self.key, received.as_bytes(), self.tag.as_ref()).is_ok()
    }
}

impl fmt::Debug for StateCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StateCheck(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_rfcs_for_the_rfcs_verifier() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_has_the_rfcs_length_and_alphabet_and_is_new_each_time() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let pkce = Pkce::new().unwrap();
            let verifier = pkce.verifier.expose();
            // RFC 7636, section 4.1: 43 to 128 characters, unreserved only.
            assert!((43..=128).contains(&verifier.len()), "{}", verifier.len());
            assert!(
                verifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
            );
            assert_eq!(pkce.challenge, challenge_for(verifier));
            assert_eq!(pkce.challenge.len(), 43);
            assert!(seen.insert(verifier.to_owned()), "a verifier came twice");
        }
    }

    #[test]
    fn a_verifier_does_not_print() {
        let pkce = Pkce::new().unwrap();
        let shown = format!("{pkce:?}");
        assert!(!shown.contains(pkce.verifier.expose()), "{shown}");
    }

    #[test]
    fn only_the_state_that_was_sent_is_recognised() {
        let state = random_token().unwrap();
        let check = StateCheck::new(&state).unwrap();
        assert!(check.matches(&state));
        assert!(!check.matches(""));
        assert!(!check.matches(&state[1..]));
        assert!(!check.matches(&format!("{state}x")));
        assert!(!check.matches(&state.to_uppercase()));
        assert!(!check.matches(&random_token().unwrap()));
        assert!(!format!("{check:?}").contains(&state));
    }
}
