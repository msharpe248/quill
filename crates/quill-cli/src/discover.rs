//! Finding an account's mail servers from its address.
//!
//! In order: the table of providers Quill knows by name; the domain's own
//! DNS records (the SRV records of RFC 6186, then its mail exchanger, which
//! says who hosts it); and last the names servers are usually given. A
//! candidate is offered only after something answered there. Encrypted
//! connections are tried first and plaintext is never chosen.
//!
//! Nothing is sent to anyone but the domain's own DNS and servers.

use std::future::Future;
use std::pin::Pin;

use quill_core::config::TlsMode;
use quill_core::providers::{self, Endpoint, Provider};
use quill_tui::setup::{AccountDraft, Discovery, ServerDraft};

type Answer<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One SRV record: where a service is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub host: String,
    pub port: u16,
    pub priority: u16,
}

/// The DNS questions discovery asks.
pub trait Dns: Send + Sync {
    /// The SRV records of `name`, such as `_imaps._tcp.example.com`.
    fn services<'a>(&'a self, name: &'a str) -> Answer<'a, Vec<Service>>;
    /// The domain's mail exchangers, most preferred first.
    fn mail_exchangers<'a>(&'a self, domain: &'a str) -> Answer<'a, Vec<String>>;
}

/// Whether a server answers.
pub trait Prober: Send + Sync {
    fn imap<'a>(&'a self, host: &'a str, port: u16, tls: TlsMode) -> Answer<'a, bool>;
}

/// Finds the servers for `address`. Always comes back with a draft: when
/// nothing is found it is empty of a server, for the user to fill in.
pub async fn discover(address: &str, dns: &dyn Dns, prober: &dyn Prober) -> Discovery {
    let domain = address
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let draft =
        |name: String, imap: (String, u16, TlsMode), smtp: Option<(String, u16, TlsMode)>| {
            let server = |(host, port, tls): (String, u16, TlsMode)| ServerDraft {
                host,
                port,
                tls,
                username: address.to_owned(),
            };
            AccountDraft {
                name,
                address: address.to_owned(),
                imap: server(imap),
                smtp: smtp.map(server),
            }
        };
    let from_provider = |provider: &Provider, imap: Endpoint, how: String| Discovery {
        draft: draft(
            provider.name.to_owned(),
            endpoint(imap),
            provider.smtp.map(endpoint),
        ),
        found: true,
        note: Some(match provider.note {
            Some(note) => format!("{how} {note}"),
            None => how,
        }),
        microsoft_application: None,
        microsoft_own: false,
        google_application: None,
        google_own: false,
    };

    // 1. A provider known by name.
    if let Some(provider) = providers::by_domain(&domain)
        && let Some(imap) = provider.imap
    {
        return from_provider(
            provider,
            imap,
            format!("{} is a {} address.", address, provider.name),
        );
    }

    // 2. What the domain publishes about itself.
    let mut imap: Option<(String, u16, TlsMode)> = None;
    for (label, tls) in [("_imaps", TlsMode::Implicit), ("_imap", TlsMode::Starttls)] {
        let mut records = dns.services(&format!("{label}._tcp.{domain}")).await;
        records.sort_by_key(|record| record.priority);
        // A target of "." says the service is deliberately not offered.
        if let Some(record) = records.into_iter().find(|record| is_host(&record.host)) {
            imap = Some((record.host, record.port, tls));
            break;
        }
    }
    let mut smtp: Option<(String, u16, TlsMode)> = None;
    for (label, tls) in [
        ("_submissions", TlsMode::Implicit),
        ("_submission", TlsMode::Starttls),
    ] {
        let mut records = dns.services(&format!("{label}._tcp.{domain}")).await;
        records.sort_by_key(|record| record.priority);
        if let Some(record) = records.into_iter().find(|record| is_host(&record.host)) {
            smtp = Some((record.host, record.port, tls));
            break;
        }
    }
    if let Some(found) = imap {
        return Discovery {
            draft: draft(account_name(&domain), found, smtp),
            found: true,
            note: Some(format!("Found in the DNS records of {domain}.")),
            microsoft_application: None,
            microsoft_own: false,
            google_application: None,
            google_own: false,
        };
    }

    // 3. Who hosts the domain's mail.
    let exchangers = dns.mail_exchangers(&domain).await;
    for exchanger in &exchangers {
        if let Some(provider) = providers::by_mail_exchanger(exchanger)
            && let Some(imap) = provider.imap
        {
            let mut found = from_provider(
                provider,
                imap,
                format!("Mail for {domain} is hosted by {}.", provider.name),
            );
            found.draft.name = account_name(&domain);
            return found;
        }
    }

    // 4. The usual names, at the domain and at whoever takes its mail.
    let mut bases = vec![domain.clone()];
    for exchanger in &exchangers {
        if let Some(base) = registrable(exchanger)
            && !bases.contains(&base)
        {
            bases.push(base);
        }
    }
    for base in &bases {
        for prefix in ["imap", "mail"] {
            let host = format!("{prefix}.{base}");
            for (port, tls) in [(993, TlsMode::Implicit), (143, TlsMode::Starttls)] {
                if prober.imap(&host, port, tls).await {
                    // Submission is usually beside it; offered, not checked,
                    // since nothing is sent until the first message is.
                    let smtp = smtp.clone().or_else(|| {
                        Some(match tls {
                            TlsMode::Implicit => (format!("smtp.{base}"), 465, TlsMode::Implicit),
                            _ => (format!("smtp.{base}"), 587, TlsMode::Starttls),
                        })
                    });
                    return Discovery {
                        draft: draft(account_name(&domain), (host.clone(), port, tls), smtp),
                        found: true,
                        note: Some(format!("A mail server answered at {host}.")),
                        microsoft_application: None,
                        microsoft_own: false,
                        google_application: None,
                        google_own: false,
                    };
                }
            }
        }
    }

    Discovery {
        draft: draft(
            account_name(&domain),
            (String::new(), 993, TlsMode::Implicit),
            smtp,
        ),
        found: false,
        note: Some(format!(
            "Quill could not find the mail server for {domain}. Your provider's help pages will list it as the IMAP server."
        )),
        microsoft_application: None,
        microsoft_own: false,
        google_application: None,
        google_own: false,
    }
}

fn endpoint(endpoint: Endpoint) -> (String, u16, TlsMode) {
    (endpoint.host.to_owned(), endpoint.port, endpoint.tls)
}

/// A name for the account, from its domain: `example.com` is "Example".
fn account_name(domain: &str) -> String {
    let label = domain.split('.').next().unwrap_or(domain);
    let mut letters = label.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => domain.to_owned(),
    }
}

/// Whether an SRV target names a host. The root, ".", means "no such
/// service here".
fn is_host(target: &str) -> bool {
    !target.trim_end_matches('.').is_empty()
}

/// The last two labels of a host name: where a hosting company's other
/// servers are likely to be. A guess, checked by connecting.
fn registrable(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    (labels.len() >= 2).then(|| labels[labels.len() - 2..].join(".").to_ascii_lowercase())
}

/// DNS through the system's own resolver settings.
pub struct SystemDns {
    resolver: Option<hickory_resolver::TokioResolver>,
}

impl SystemDns {
    pub fn new() -> Self {
        // With no usable resolver settings the DNS steps find nothing, and
        // discovery goes on to the other ways. No other resolver is asked:
        // which addresses a user has is not for a third party to hear.
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .ok()
            .and_then(|builder| builder.build().ok());
        Self { resolver }
    }
}

impl Default for SystemDns {
    fn default() -> Self {
        Self::new()
    }
}

/// How long one DNS question or one connection attempt may take.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(6);

impl Dns for SystemDns {
    fn services<'a>(&'a self, name: &'a str) -> Answer<'a, Vec<Service>> {
        Box::pin(async move {
            let Some(resolver) = &self.resolver else {
                return Vec::new();
            };
            let fqdn = format!("{}.", name.trim_end_matches('.'));
            match tokio::time::timeout(PATIENCE, resolver.srv_lookup(fqdn)).await {
                Ok(Ok(lookup)) => lookup
                    .answers()
                    .iter()
                    .filter_map(|record| match &record.data {
                        hickory_resolver::proto::rr::RData::SRV(srv) => Some(Service {
                            host: srv.target.to_utf8().trim_end_matches('.').to_owned(),
                            port: srv.port,
                            priority: srv.priority,
                        }),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        })
    }

    fn mail_exchangers<'a>(&'a self, domain: &'a str) -> Answer<'a, Vec<String>> {
        Box::pin(async move {
            let Some(resolver) = &self.resolver else {
                return Vec::new();
            };
            let fqdn = format!("{}.", domain.trim_end_matches('.'));
            match tokio::time::timeout(PATIENCE, resolver.mx_lookup(fqdn)).await {
                Ok(Ok(lookup)) => {
                    let mut exchangers: Vec<(u16, String)> = lookup
                        .answers()
                        .iter()
                        .filter_map(|record| match &record.data {
                            hickory_resolver::proto::rr::RData::MX(mx) => Some((
                                mx.preference,
                                mx.exchange.to_utf8().trim_end_matches('.').to_owned(),
                            )),
                            _ => None,
                        })
                        .collect();
                    exchangers.sort();
                    exchangers.into_iter().map(|(_, host)| host).collect()
                }
                _ => Vec::new(),
            }
        })
    }
}

/// Probing by really connecting.
pub struct NetworkProber;

impl Prober for NetworkProber {
    fn imap<'a>(&'a self, host: &'a str, port: u16, tls: TlsMode) -> Answer<'a, bool> {
        Box::pin(async move {
            let config = quill_imap::ImapConfig {
                host: host.to_owned(),
                port,
                tls,
                username: String::new(),
                trust: quill_imap::Trust::default(),
            };
            match tokio::time::timeout(PATIENCE, quill_imap::probe(&config)).await {
                Ok(Ok(())) => true,
                // A server whose certificate is not trusted is still the
                // server. The user is asked about it at sign-in.
                Ok(Err(quill_core::Error::Certificate(_))) => true,
                _ => false,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeDns {
        services: HashMap<String, Vec<Service>>,
        exchangers: HashMap<String, Vec<String>>,
        asked: Mutex<Vec<String>>,
    }

    impl Dns for FakeDns {
        fn services<'a>(&'a self, name: &'a str) -> Answer<'a, Vec<Service>> {
            self.asked.lock().unwrap().push(name.to_owned());
            Box::pin(async move { self.services.get(name).cloned().unwrap_or_default() })
        }

        fn mail_exchangers<'a>(&'a self, domain: &'a str) -> Answer<'a, Vec<String>> {
            self.asked.lock().unwrap().push(format!("MX {domain}"));
            Box::pin(async move { self.exchangers.get(domain).cloned().unwrap_or_default() })
        }
    }

    /// Answers at the listed places, and remembers what was tried.
    #[derive(Default)]
    struct FakeProber {
        answering: Vec<(&'static str, u16, TlsMode)>,
        tried: Mutex<Vec<(String, u16, TlsMode)>>,
    }

    impl Prober for FakeProber {
        fn imap<'a>(&'a self, host: &'a str, port: u16, tls: TlsMode) -> Answer<'a, bool> {
            self.tried
                .lock()
                .unwrap()
                .push((host.to_owned(), port, tls));
            let answers = self.answering.iter().any(|(known, known_port, known_tls)| {
                *known == host && *known_port == port && *known_tls == tls
            });
            Box::pin(async move { answers })
        }
    }

    fn service(host: &str, port: u16, priority: u16) -> Service {
        Service {
            host: host.to_owned(),
            port,
            priority,
        }
    }

    #[tokio::test]
    async fn a_known_provider_needs_no_lookups() {
        let dns = FakeDns::default();
        let prober = FakeProber::default();
        let found = discover("robin@gmail.com", &dns, &prober).await;
        assert!(found.found);
        assert_eq!(found.draft.name, "Gmail");
        assert_eq!(
            (
                found.draft.imap.host.as_str(),
                found.draft.imap.port,
                found.draft.imap.tls
            ),
            ("imap.gmail.com", 993, TlsMode::Implicit)
        );
        assert_eq!(found.draft.imap.username, "robin@gmail.com");
        assert_eq!(found.draft.smtp.unwrap().host, "smtp.gmail.com");
        assert!(found.note.unwrap().contains("app password"));
        assert!(dns.asked.lock().unwrap().is_empty());
        assert!(prober.tried.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn srv_records_are_used_with_tls_preferred_and_priority_respected() {
        let mut dns = FakeDns::default();
        dns.services.insert(
            "_imaps._tcp.example.com".to_owned(),
            vec![
                service("backup.example.com", 993, 20),
                service("mx.example.com", 1993, 10),
            ],
        );
        dns.services.insert(
            "_imap._tcp.example.com".to_owned(),
            vec![service("plain.example.com", 143, 1)],
        );
        dns.services.insert(
            "_submission._tcp.example.com".to_owned(),
            vec![service("send.example.com", 587, 1)],
        );
        let found = discover("robin@Example.COM", &dns, &FakeProber::default()).await;
        assert!(found.found);
        assert_eq!(found.draft.name, "Example");
        assert_eq!(
            (
                found.draft.imap.host.as_str(),
                found.draft.imap.port,
                found.draft.imap.tls
            ),
            ("mx.example.com", 1993, TlsMode::Implicit)
        );
        let smtp = found.draft.smtp.unwrap();
        assert_eq!(
            (smtp.host.as_str(), smtp.port, smtp.tls),
            ("send.example.com", 587, TlsMode::Starttls)
        );
        assert!(found.note.unwrap().contains("DNS records"));
    }

    #[tokio::test]
    async fn a_service_declared_absent_is_not_used() {
        let mut dns = FakeDns::default();
        dns.services.insert(
            "_imaps._tcp.example.com".to_owned(),
            vec![service(".", 0, 0)],
        );
        dns.services.insert(
            "_imap._tcp.example.com".to_owned(),
            vec![service("mail.example.com", 143, 0)],
        );
        let found = discover("robin@example.com", &dns, &FakeProber::default()).await;
        assert_eq!(
            (found.draft.imap.host.as_str(), found.draft.imap.tls),
            ("mail.example.com", TlsMode::Starttls)
        );
    }

    #[tokio::test]
    async fn a_hosted_domain_is_recognised_by_its_mail_exchanger() {
        let mut dns = FakeDns::default();
        dns.exchangers.insert(
            "example.com".to_owned(),
            vec!["aspmx.l.google.com".to_owned()],
        );
        let found = discover("robin@example.com", &dns, &FakeProber::default()).await;
        assert!(found.found);
        // Named for the user's own domain, reached at the host's servers.
        assert_eq!(found.draft.name, "Example");
        assert_eq!(found.draft.imap.host, "imap.gmail.com");
        let note = found.note.unwrap();
        assert!(
            note.contains("hosted by Gmail") && note.contains("app password"),
            "{note}"
        );
    }

    #[tokio::test]
    async fn the_usual_names_are_tried_encrypted_first_and_never_in_the_clear() {
        let mut dns = FakeDns::default();
        dns.exchangers.insert(
            "example.com".to_owned(),
            vec!["mx1.hoster.example.net".to_owned()],
        );
        let prober = FakeProber {
            answering: vec![("mail.example.net", 143, TlsMode::Starttls)],
            ..FakeProber::default()
        };
        let found = discover("robin@example.com", &dns, &prober).await;
        assert!(found.found);
        assert_eq!(
            (
                found.draft.imap.host.as_str(),
                found.draft.imap.port,
                found.draft.imap.tls
            ),
            ("mail.example.net", 143, TlsMode::Starttls)
        );
        assert_eq!(found.draft.smtp.unwrap().port, 587);
        let tried = prober.tried.lock().unwrap().clone();
        assert_eq!(
            tried[0],
            ("imap.example.com".to_owned(), 993, TlsMode::Implicit)
        );
        assert!(tried.iter().all(|(_, _, tls)| *tls != TlsMode::None));
        // Implicit TLS is asked before STARTTLS at every name.
        for pair in tried.chunks(2) {
            assert_eq!(pair[0].2, TlsMode::Implicit);
        }
    }

    #[tokio::test]
    async fn nothing_found_says_so_and_leaves_the_server_blank() {
        let found = discover(
            "robin@example.com",
            &FakeDns::default(),
            &FakeProber::default(),
        )
        .await;
        assert!(!found.found);
        assert_eq!(found.draft.imap.host, "");
        assert_eq!(found.draft.imap.tls, TlsMode::Implicit);
        assert_eq!(found.draft.address, "robin@example.com");
        assert!(found.note.unwrap().contains("could not find"));
    }

    #[test]
    fn names_and_bases() {
        assert_eq!(account_name("example.com"), "Example");
        assert_eq!(account_name("hillside.example"), "Hillside");
        assert_eq!(
            registrable("mx1.hoster.example.net."),
            Some("example.net".to_owned())
        );
        assert_eq!(registrable("localhost"), None);
        assert!(is_host("mail.example.com.") && !is_host(".") && !is_host(""));
    }
}
