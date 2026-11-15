//! The error type shared by backends.

/// An error from a mail backend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The folder or message named in the request does not exist remotely.
    #[error("not found: {0}")]
    NotFound(String),
    /// The backend failed for a reason of its own (protocol, server).
    #[error("backend error: {0}")]
    Backend(String),
    /// The server could not be reached, or the connection dropped. Worth
    /// trying again later; nothing is wrong with the account.
    #[error("cannot reach the server: {0}")]
    Connection(String),
    /// The server refused the credentials.
    #[error("sign-in failed: {0}")]
    Auth(String),
    /// The server's certificate was not accepted. Never bypassed silently:
    /// the user is shown the certificate and decides.
    #[error("the server's certificate was not accepted: {}", .0.reason)]
    Certificate(Box<CertificateProblem>),
    /// The connection would not be secure in the way the account requires,
    /// such as a server that does not offer STARTTLS.
    #[error("the connection is not secure: {0}")]
    Insecure(String),
    /// The backend cannot do this.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// The server could not take the message just now (an SMTP 4xx reply,
    /// throttling). Nothing was sent. Keep the message and try again later.
    #[error("the server is not accepting mail right now: {0}")]
    Temporary(String),
    /// The server refused the message for good (an SMTP 5xx reply). Nothing
    /// was sent. Keep the message and show the server's words.
    #[error("the server refused the message: {0}")]
    Refused(String),
    /// The server refused some of the recipients. Nothing was sent to
    /// anyone: a message goes to everyone it is addressed to or to no one,
    /// unless the user says otherwise.
    #[error("the server refused {} of the recipients", .0.len())]
    Recipients(Vec<RefusedRecipient>),
    /// The message is larger than the server takes. Known before any of it
    /// was uploaded.
    #[error("the message is {size} bytes and the server accepts at most {limit}")]
    TooLarge { size: u64, limit: u64 },
}

/// One address a server would not deliver to, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedRecipient {
    pub address: String,
    pub reason: String,
}

/// A certificate the client would not trust, for the user to judge.
#[derive(Clone, PartialEq, Eq)]
pub struct CertificateProblem {
    pub host: String,
    /// Why it was refused, in the verifier's words.
    pub reason: String,
    /// SHA-256 of the certificate the server presented, in lower-case hex.
    /// This is what an account pins to trust a self-signed server.
    pub fingerprint: Option<String>,
    /// The certificate itself, DER encoded, for showing its subject, issuer
    /// and dates.
    pub certificate: Option<Vec<u8>>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Debug for CertificateProblem {
    /// The certificate itself is a kilobyte of bytes nobody reads in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateProblem")
            .field("host", &self.host)
            .field("reason", &self.reason)
            .field("fingerprint", &self.fingerprint)
            .field(
                "certificate",
                &self
                    .certificate
                    .as_ref()
                    .map(|der| format!("{} bytes", der.len())),
            )
            .finish()
    }
}
