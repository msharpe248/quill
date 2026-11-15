//! What it takes to reach one IMAP account.

use quill_core::config::TlsMode;
pub use quill_net::Trust;

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
