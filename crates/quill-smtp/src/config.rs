//! What it takes to reach one SMTP submission server.

use quill_core::config::TlsMode;
pub use quill_net::Trust;

use crate::auth::Mechanism;

/// The name given in `EHLO` when the account does not set one. An address
/// literal says nothing about the machine: its real host name is nobody
/// else's business, and inside the App Sandbox it is not ours to ask for.
pub const DEFAULT_HELLO_NAME: &str = "[127.0.0.1]";

/// How to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SmtpAuth {
    /// The strongest mechanism the server offers that the credential can be
    /// used with.
    #[default]
    Automatic,
    /// This mechanism, whatever the server advertises.
    Mechanism(Mechanism),
    /// No sign-in at all, for a relay that takes mail from this machine
    /// without one. The credential provider is never asked.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub trust: Trust,
    pub auth: SmtpAuth,
    /// The name this client gives in `EHLO`: a host name or an address
    /// literal such as [`DEFAULT_HELLO_NAME`].
    pub hello_name: String,
}

impl SmtpConfig {
    /// The standard submission port for a TLS mode: 465 for implicit TLS,
    /// 587 otherwise.
    pub fn default_port(tls: TlsMode) -> u16 {
        match tls {
            TlsMode::Implicit => 465,
            TlsMode::Starttls | TlsMode::None => 587,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ports_follow_the_tls_mode() {
        assert_eq!(SmtpConfig::default_port(TlsMode::Implicit), 465);
        assert_eq!(SmtpConfig::default_port(TlsMode::Starttls), 587);
        assert_eq!(SmtpConfig::default_port(TlsMode::None), 587);
    }
}
