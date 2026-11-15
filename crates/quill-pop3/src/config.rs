//! What it takes to reach one POP3 account.

use quill_core::config::TlsMode;
pub use quill_net::Trust;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pop3Config {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub trust: Trust,
}

impl Pop3Config {
    /// The standard port for a TLS mode: 995 for implicit TLS, 110 otherwise.
    pub fn default_port(tls: TlsMode) -> u16 {
        match tls {
            TlsMode::Implicit => 995,
            TlsMode::Starttls | TlsMode::None => 110,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_ports_are_the_standard_ones() {
        assert_eq!(Pop3Config::default_port(TlsMode::Implicit), 995);
        assert_eq!(Pop3Config::default_port(TlsMode::Starttls), 110);
        assert_eq!(Pop3Config::default_port(TlsMode::None), 110);
    }
}
