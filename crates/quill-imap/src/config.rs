//! What it takes to reach one IMAP account.

use quill_core::config::TlsMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub trust: Trust,
}

impl ImapConfig {
    /// The standard port for a TLS mode: 993 for implicit TLS, 143 otherwise.
    pub fn default_port(tls: TlsMode) -> u16 {
        match tls {
            TlsMode::Implicit => 993,
            TlsMode::Starttls | TlsMode::None => 143,
        }
    }
}

/// Which server certificates to accept, beyond those the system trusts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trust {
    /// Extra certificate authorities, DER encoded: a custom CA bundle.
    pub extra_roots: Vec<Vec<u8>>,
    /// The SHA-256 of the one certificate to accept, for a self-signed
    /// server. When set, the certificate must match it exactly and nothing
    /// else about it is checked.
    pub pinned_sha256: Option<[u8; 32]>,
}
