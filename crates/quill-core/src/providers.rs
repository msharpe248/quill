//! The mail providers Quill knows by name: where their servers are, and
//! what a user needs to know before signing in.
//!
//! This table answers "which server does `@gmail.com` use" without asking
//! the network. It is deliberately short. A domain it does not list is
//! looked up in DNS, and whatever is found here or there is checked by
//! connecting before the user is shown it.

use crate::config::TlsMode;

/// Where one of a provider's servers listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub host: &'static str,
    pub port: u16,
    pub tls: TlsMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub name: &'static str,
    /// The address domains this provider owns.
    pub domains: &'static [&'static str],
    /// What the mail exchangers of a custom domain hosted here end with:
    /// how `you@yourcompany.com` is recognised as, say, Google Workspace.
    pub mx_suffixes: &'static [&'static str],
    pub imap: Option<Endpoint>,
    pub smtp: Option<Endpoint>,
    /// Something to tell the user at setup, such as the need for an app
    /// password.
    pub note: Option<&'static str>,
}

const fn tls(host: &'static str, port: u16) -> Option<Endpoint> {
    Some(Endpoint {
        host,
        port,
        tls: TlsMode::Implicit,
    })
}

const fn starttls(host: &'static str, port: u16) -> Option<Endpoint> {
    Some(Endpoint {
        host,
        port,
        tls: TlsMode::Starttls,
    })
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        name: "Gmail",
        domains: &["gmail.com", "googlemail.com"],
        mx_suffixes: &["google.com", "googlemail.com"],
        imap: tls("imap.gmail.com", 993),
        smtp: tls("smtp.gmail.com", 465),
        note: Some(
            "Google does not accept your normal password here. Create an app password in your Google account (Security, 2-Step Verification, App passwords) and use that.",
        ),
    },
    Provider {
        name: "iCloud",
        domains: &["icloud.com", "me.com", "mac.com"],
        mx_suffixes: &["mail.icloud.com"],
        imap: tls("imap.mail.me.com", 993),
        smtp: starttls("smtp.mail.me.com", 587),
        note: Some(
            "Apple does not accept your Apple Account password here. Create an app-specific password at account.apple.com and use that.",
        ),
    },
    Provider {
        name: "Fastmail",
        domains: &["fastmail.com", "fastmail.fm"],
        mx_suffixes: &["messagingengine.com"],
        imap: tls("imap.fastmail.com", 993),
        smtp: tls("smtp.fastmail.com", 465),
        note: Some(
            "Fastmail needs an app password: create one under Settings, Privacy & Security, Integrations.",
        ),
    },
    Provider {
        name: "Microsoft",
        domains: &["outlook.com", "hotmail.com", "live.com", "msn.com"],
        mx_suffixes: &["outlook.com"],
        imap: tls("outlook.office365.com", 993),
        smtp: starttls("smtp.office365.com", 587),
        note: Some(
            "Microsoft takes no password from a mail program. Its accounts sign in with Microsoft.",
        ),
    },
    Provider {
        name: "Yahoo",
        domains: &["yahoo.com", "ymail.com", "rocketmail.com"],
        mx_suffixes: &["yahoodns.net"],
        imap: tls("imap.mail.yahoo.com", 993),
        smtp: tls("smtp.mail.yahoo.com", 465),
        note: Some("Yahoo needs an app password: create one under Account security."),
    },
    Provider {
        name: "AOL",
        domains: &["aol.com"],
        mx_suffixes: &[],
        imap: tls("imap.aol.com", 993),
        smtp: tls("smtp.aol.com", 465),
        note: Some("AOL needs an app password: create one under Account security."),
    },
    Provider {
        name: "Zoho",
        domains: &["zoho.com", "zohomail.com"],
        mx_suffixes: &["zoho.com"],
        imap: tls("imap.zoho.com", 993),
        smtp: tls("smtp.zoho.com", 465),
        note: None,
    },
    Provider {
        name: "GMX",
        domains: &["gmx.com", "gmx.us"],
        mx_suffixes: &[],
        imap: tls("imap.gmx.com", 993),
        smtp: starttls("mail.gmx.com", 587),
        note: None,
    },
];

/// Whether the mail server `host` is one that takes no password at all from
/// a mail program, and whose it is. Microsoft's do not: every mailbox there,
/// Outlook.com or Microsoft 365, signs in with Microsoft.
pub fn takes_no_password(host: &str) -> Option<&'static str> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["office365.com", "outlook.com", "hotmail.com", "live.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        .then_some("Microsoft")
}

/// Whether `host` is one of Google's mail servers: the ones a Gmail or
/// Google Workspace mailbox is read from and sent through, which take a
/// sign-in with Google in place of a password.
pub fn signs_in_with_google(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["gmail.com", "googlemail.com"]
        .iter()
        .any(|domain| host.ends_with(&format!(".{domain}")))
}

/// What to know about the password a known provider's server takes, by
/// the server's name: that the user's normal password is not it, and where
/// the one that is comes from. `None` for a server Quill does not know, and
/// for one that takes no password at all ([`takes_no_password`]).
///
/// Said where the password is typed and again when it is refused: a
/// refusal by such a server is nearly always the normal password, typed
/// correctly.
pub fn password_advice(host: &str) -> Option<&'static str> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || takes_no_password(&host).is_some() {
        return None;
    }
    PROVIDERS
        .iter()
        .find(|provider| {
            [provider.imap, provider.smtp]
                .into_iter()
                .flatten()
                .any(|endpoint| endpoint.host == host)
        })
        .and_then(|provider| provider.note)
}

/// The provider that owns an address domain, such as `gmail.com`.
pub fn by_domain(domain: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| {
        provider
            .domains
            .iter()
            .any(|known| known.eq_ignore_ascii_case(domain))
    })
}

/// The provider hosting a domain whose mail exchanger is `exchanger`, such
/// as `aspmx.l.google.com`.
pub fn by_mail_exchanger(exchanger: &str) -> Option<&'static Provider> {
    let exchanger = exchanger.trim_end_matches('.').to_ascii_lowercase();
    PROVIDERS.iter().find(|provider| {
        provider.mx_suffixes.iter().any(|suffix| {
            exchanger == *suffix
                || exchanger
                    .strip_suffix(suffix)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn googles_mail_servers_are_known_by_name() {
        for host in [
            "imap.gmail.com",
            "smtp.gmail.com",
            "IMAP.Gmail.com.",
            "imap.googlemail.com",
        ] {
            assert!(signs_in_with_google(host), "{host}");
        }
        for host in [
            "",
            "gmail.com",
            "imap.gmail.com.example.com",
            "notgmail.com",
            "imap.fastmail.com",
        ] {
            assert!(!signs_in_with_google(host), "{host}");
        }
    }

    #[test]
    fn a_known_server_that_wants_a_password_of_its_own_has_advice_and_no_other_has() {
        for (host, about) in [
            (
                "imap.gmail.com",
                "Google does not accept your normal password",
            ),
            (
                "smtp.gmail.com",
                "Google does not accept your normal password",
            ),
            (
                "IMAP.Gmail.com.",
                "Google does not accept your normal password",
            ),
            ("imap.mail.me.com", "app-specific password"),
            ("imap.fastmail.com", "Fastmail needs an app password"),
            ("imap.mail.yahoo.com", "Yahoo needs an app password"),
        ] {
            let advice = password_advice(host).unwrap_or_default();
            assert!(advice.contains(about), "{host}: {advice}");
        }
        // A server Quill does not know, one whose provider has nothing to
        // say, and one that takes no password at all.
        for host in [
            "",
            "mail.example.com",
            "gmail.com",
            "imap.gmail.com.example.com",
            "imap.zoho.com",
            "outlook.office365.com",
        ] {
            assert_eq!(password_advice(host), None, "{host}");
        }
    }

    #[test]
    fn microsofts_servers_take_no_password() {
        for host in [
            "outlook.office365.com",
            "smtp.office365.com",
            "OUTLOOK.OFFICE365.COM",
            "imap-mail.outlook.com",
            "outlook.com",
            "smtp.live.com.",
        ] {
            assert_eq!(takes_no_password(host), Some("Microsoft"), "{host}");
        }
        for host in [
            "imap.gmail.com",
            "imap.example.com",
            "notoutlook.com",
            "office365.com.example.com",
            "",
        ] {
            assert_eq!(takes_no_password(host), None, "{host}");
        }
    }

    #[test]
    fn a_provider_is_found_by_its_address_domain() {
        assert_eq!(by_domain("gmail.com").map(|p| p.name), Some("Gmail"));
        assert_eq!(by_domain("ME.com").map(|p| p.name), Some("iCloud"));
        assert_eq!(by_domain("example.com"), None);
        // Only whole domains: a lookalike is not Gmail.
        assert_eq!(by_domain("notgmail.com"), None);
        assert_eq!(by_domain("gmail.com.example.org"), None);
    }

    #[test]
    fn a_hosted_domain_is_recognised_by_its_mail_exchanger() {
        let name = |host| by_mail_exchanger(host).map(|provider| provider.name);
        assert_eq!(name("aspmx.l.google.com."), Some("Gmail"));
        assert_eq!(
            name("example-com.mail.protection.outlook.com"),
            Some("Microsoft")
        );
        assert_eq!(name("in1-smtp.messagingengine.com"), Some("Fastmail"));
        assert_eq!(name("mx01.mail.icloud.com"), Some("iCloud"));
        assert_eq!(name("mx.example.com"), None);
        // A suffix must sit on a label boundary.
        assert_eq!(name("evilgoogle.com"), None);
        assert_eq!(name("mx.notoutlook.com"), None);
    }

    #[test]
    fn every_provider_is_reachable_over_tls_and_owns_its_domains_alone() {
        let mut seen = std::collections::HashSet::new();
        for provider in PROVIDERS {
            assert!(provider.imap.is_some(), "{}", provider.name);
            for endpoint in [provider.imap, provider.smtp].into_iter().flatten() {
                assert_ne!(endpoint.tls, TlsMode::None, "{}", provider.name);
                assert!(endpoint.host.contains('.'));
            }
            for domain in provider.domains {
                assert!(seen.insert(*domain), "{domain} is listed twice");
            }
        }
    }
}
