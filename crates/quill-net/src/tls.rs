//! TLS: which certificates are accepted, and what is recorded when one is
//! not.
//!
//! Certificates are verified against the system trust store, with SNI and
//! hostname checks, through rustls. An account may add its own certificate
//! authorities, or pin the one certificate of a self-signed server. A
//! certificate that fails is never accepted quietly: the error carries it,
//! so the user can be shown what the server presented and decide.

use std::fmt;
use std::sync::{Arc, Mutex};

use quill_core::{CertificateProblem, Error, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::Trust;

/// The certificate a server presented, kept for the error report.
pub type Presented = Arc<Mutex<Option<Vec<u8>>>>;

/// A client configuration for `trust`, and where the server's certificate
/// will be recorded during the handshake.
pub fn client_config(trust: &Trust) -> Result<(Arc<ClientConfig>, Presented)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let system = rustls_platform_verifier::Verifier::new_with_extra_roots(
        trust
            .extra_roots
            .iter()
            .map(|der| CertificateDer::from(der.clone())),
        Arc::clone(&provider),
    )
    .map_err(|err| Error::Backend(format!("cannot load the trusted certificates: {err}")))?;
    let presented = Presented::default();
    let verifier = Verifier {
        system: Arc::new(system),
        pinned: trust.pinned_sha256,
        presented: Arc::clone(&presented),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| Error::Backend(format!("cannot set up TLS: {err}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok((Arc::new(config), presented))
}

/// SHA-256 of a certificate, in lower-case hex.
pub fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Describes a refused certificate for the user.
pub fn problem(host: &str, reason: String, presented: &Presented) -> Error {
    let certificate = presented.lock().ok().and_then(|slot| slot.clone());
    Error::Certificate(Box::new(CertificateProblem {
        host: host.to_owned(),
        reason,
        fingerprint: certificate.as_deref().map(fingerprint),
        certificate,
    }))
}

struct Verifier {
    system: Arc<dyn ServerCertVerifier>,
    pinned: Option<[u8; 32]>,
    presented: Presented,
}

impl fmt::Debug for Verifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Verifier")
            .field("pinned", &self.pinned.is_some())
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut slot) = self.presented.lock() {
            *slot = Some(end_entity.as_ref().to_vec());
        }
        match self.pinned {
            // A pin names one exact certificate. Its name, dates and issuer
            // are beside the point: the user chose this certificate.
            Some(pin) => {
                if Sha256::digest(end_entity.as_ref()).as_slice() == pin {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::General(
                        "the certificate is not the one pinned for this account".to_owned(),
                    ))
                }
            }
            None => self.system.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
        }
    }

    // A pinned certificate still has to prove it holds its key: the
    // handshake signatures are always checked.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.system.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.system.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.system.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_lower_case_sha256_hex() {
        assert_eq!(
            fingerprint(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(fingerprint(b"abc").len(), 64);
    }

    #[test]
    fn a_problem_carries_the_presented_certificate() {
        let presented = Presented::default();
        *presented.lock().unwrap() = Some(b"der bytes".to_vec());
        match problem("mail.example.com", "expired".to_owned(), &presented) {
            Error::Certificate(problem) => {
                assert_eq!(problem.host, "mail.example.com");
                assert_eq!(problem.reason, "expired");
                assert_eq!(problem.certificate.as_deref(), Some(&b"der bytes"[..]));
                assert_eq!(problem.fingerprint.unwrap(), fingerprint(b"der bytes"));
            }
            other => panic!("{other:?}"),
        }
        // A handshake that failed before any certificate arrived.
        match problem("mail.example.com", "x".to_owned(), &Presented::default()) {
            Error::Certificate(problem) => assert_eq!(problem.fingerprint, None),
            other => panic!("{other:?}"),
        }
    }
}
