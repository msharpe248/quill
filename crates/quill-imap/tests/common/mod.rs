//! A real IMAP server for the tests: Dovecot, started unprivileged in a
//! temporary directory on a port of its own.
//!
//! Dovecot speaks plaintext IMAP here and the tests put TLS in front of it
//! themselves ([`TlsFront`]). Run unprivileged on macOS, Dovecot's own TLS
//! relay drops every session straight after sign-in; and owning the TLS end
//! lets a test choose exactly which certificate a client is shown.
//!
//! Every user name signs in with the same password and gets an empty
//! mailbox, so tests do not share state. Everyone in the mail is invented
//! and lives at example.com.

#![allow(dead_code)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_imap::Session;
use quill_core::config::TlsMode;
use quill_imap::{ImapBackend, ImapConfig, Trust};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use tokio::net::TcpStream;

pub const PASSWORD: &str = "test-password";

/// A certificate authority and a certificate it signed for `localhost`.
pub struct Certificates {
    pub ca_der: Vec<u8>,
    pub leaf_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Certificates {
    /// A CA-signed server certificate for `names`, valid from `days_ago` days
    /// ago for `days` days.
    ///
    /// macOS only accepts a server certificate that says it is one (the
    /// server-auth extended key usage) and is valid for at most 398 days.
    pub fn issue(names: &[&str], days_ago: i64, days: i64) -> Self {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Quill Test CA");
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let start = time::OffsetDateTime::now_utc() - time::Duration::days(days_ago);
        params.not_before = start;
        params.not_after = start + time::Duration::days(days);
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Quill Test Server");
        let leaf = params.signed_by(&key, &issuer).unwrap();
        Self {
            ca_der: ca.der().to_vec(),
            leaf_der: leaf.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }

    pub fn valid() -> Self {
        Self::issue(&["localhost"], 1, 90)
    }
}

pub struct Dovecot {
    dir: tempfile::TempDir,
    child: Child,
    /// Dovecot's plaintext port.
    pub imap_port: u16,
    /// TLS in front of it, with a certificate the test CA signed.
    pub front: TlsFront,
    pub certificates: Certificates,
}

fn binary() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("QUILL_DOVECOT").map(PathBuf::from),
        Some(PathBuf::from("/opt/homebrew/opt/dovecot/sbin/dovecot")),
        Some(PathBuf::from("/usr/local/opt/dovecot/sbin/dovecot")),
        Some(PathBuf::from("/usr/sbin/dovecot")),
    ];
    candidates.into_iter().flatten().find(|path| path.is_file())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn id(flag: &str) -> String {
    let output = Command::new("id").arg(flag).output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Dovecot {
    /// Starts a server, or returns `None` when Dovecot is not installed.
    /// Set `QUILL_REQUIRE_DOVECOT=1` (CI does) to make that a failure.
    pub async fn start() -> Option<Self> {
        let Some(binary) = binary() else {
            assert!(
                std::env::var_os("QUILL_REQUIRE_DOVECOT").is_none(),
                "QUILL_REQUIRE_DOVECOT is set but dovecot was not found"
            );
            eprintln!("skipping: dovecot is not installed (brew install dovecot)");
            return None;
        };
        // Short, because Dovecot's own sockets live under it and a socket
        // path has little more than a hundred bytes.
        let dir = tempfile::Builder::new()
            .prefix("qd")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = dir.path().display().to_string();
        let certificates = Certificates::valid();
        let imap_port = free_port();
        let (user, group) = (id("-un"), id("-gn"));

        // `default_vsz_limit`: macOS rejects every finite address-space limit
        // and Dovecot's "unlimited" is a larger number than macOS allows, so
        // the limit is set to the largest value macOS accepts.
        let config = format!(
            r"dovecot_config_version = 2.4.5
dovecot_storage_version = 2.4.5
base_dir = {root}/run
state_dir = {root}/state
log_path = {root}/dovecot.log
default_internal_user = {user}
default_internal_group = {group}
default_login_user = {user}
default_vsz_limit = 8388607 T
protocols {{
  imap = yes
}}
listen = 127.0.0.1
mail_uid = {user}
mail_gid = {group}
mail_home = {root}/mail/%{{user}}
mail_driver = maildir
mail_path = ~/Maildir
first_valid_uid = 1
auth_allow_cleartext = yes
ssl = no
namespace inbox {{
  inbox = yes
  separator = /
  mailbox Drafts {{
    special_use = \Drafts
    auto = subscribe
  }}
  mailbox Sent {{
    special_use = \Sent
    auto = subscribe
  }}
  mailbox Archive {{
    special_use = \Archive
    auto = subscribe
  }}
  mailbox Trash {{
    special_use = \Trash
    auto = subscribe
  }}
  mailbox Junk {{
    special_use = \Junk
    auto = subscribe
  }}
}}
passdb static {{
  password = {PASSWORD}
}}
userdb static {{
  fields {{
    uid = {user}
    gid = {group}
  }}
}}
service imap-login {{
  chroot =
  inet_listener imap {{
    port = {imap_port}
  }}
  inet_listener imaps {{
    port = 0
  }}
}}
service anvil {{
  chroot =
}}
"
        );
        let config_path = dir.path().join("dovecot.conf");
        std::fs::write(&config_path, config).unwrap();
        let child = Command::new(binary)
            .arg("-F")
            .arg("-c")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let front = TlsFront::start(imap_port, &certificates, Starttls::Offered).await;
        let server = Self {
            dir,
            child,
            imap_port,
            front,
            certificates,
        };
        server.wait_until_ready().await;
        Some(server)
    }

    /// The server answers before its sign-in service does, so wait for a
    /// sign-in to succeed, not just for the port to open.
    async fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(mut session) = self.try_session("ready-check@example.com").await {
                let _ = session.logout().await;
                return;
            }
            assert!(
                Instant::now() < deadline,
                "dovecot did not come up; see {}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("dovecot.log")).unwrap_or_default()
    }

    async fn try_session(&self, user: &str) -> Option<Session<TcpStream>> {
        let tcp = TcpStream::connect(("127.0.0.1", self.imap_port))
            .await
            .ok()?;
        let mut client = async_imap::Client::new(tcp);
        let greeting = tokio::time::timeout(Duration::from_secs(2), client.read_response())
            .await
            .ok()?
            .ok()??;
        if !format!("{:?}", greeting.parsed()).contains("ready") {
            return None;
        }
        client.login(user, PASSWORD).await.ok()
    }

    /// A raw session for arranging the mailbox behind the backend's back, the
    /// way another mail client would.
    pub async fn session(&self, user: &str) -> Session<TcpStream> {
        self.try_session(user).await.expect("a test session")
    }

    /// How to reach `user`'s account in the given mode, trusting the test CA.
    pub fn config(&self, tls: TlsMode, user: &str) -> ImapConfig {
        ImapConfig {
            host: "localhost".to_owned(),
            port: match tls {
                TlsMode::Implicit => self.front.implicit_port,
                TlsMode::Starttls => self.front.starttls_port,
                TlsMode::None => self.imap_port,
            },
            tls,
            username: user.to_owned(),
            trust: Trust {
                extra_roots: vec![self.certificates.ca_der.clone()],
                pinned_sha256: None,
            },
        }
    }

    pub fn backend(&self, tls: TlsMode, user: &str) -> ImapBackend {
        backend(self.config(tls, user), PASSWORD)
    }
}

impl Drop for Dovecot {
    fn drop(&mut self) {
        // SIGTERM lets the master stop its children; SIGKILL would orphan them.
        let _ = Command::new("kill")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
    }
}

/// Whether a [`TlsFront`]'s STARTTLS port agrees to upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Starttls {
    Offered,
    /// The server answers STARTTLS with BAD, as one without TLS would.
    Refused,
}

/// TLS in front of a plaintext IMAP server: one port that is TLS from the
/// first byte, and one that starts in the clear and upgrades on STARTTLS.
pub struct TlsFront {
    pub implicit_port: u16,
    pub starttls_port: u16,
    /// Every line a client sent in the clear on the STARTTLS port.
    pub cleartext: Arc<std::sync::Mutex<Vec<String>>>,
}

impl TlsFront {
    pub async fn start(upstream: u16, certificates: &Certificates, starttls: Starttls) -> Self {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let key = PrivateKeyDer::try_from(certificates.key_der.clone()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificates.leaf_der.clone())],
            key,
        )
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let cleartext = Arc::new(std::sync::Mutex::new(Vec::new()));

        let implicit = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let implicit_port = implicit.local_addr().unwrap().port();
        let implicit_acceptor = acceptor.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = implicit.accept().await {
                let acceptor = implicit_acceptor.clone();
                tokio::spawn(async move {
                    let (Ok(mut tls), Ok(mut server)) = (
                        acceptor.accept(client).await,
                        TcpStream::connect(("127.0.0.1", upstream)).await,
                    ) else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut tls, &mut server).await;
                });
            }
        });

        let plain = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let starttls_port = plain.local_addr().unwrap().port();
        let seen = Arc::clone(&cleartext);
        tokio::spawn(async move {
            while let Ok((client, _)) = plain.accept().await {
                let (acceptor, seen) = (acceptor.clone(), Arc::clone(&seen));
                tokio::spawn(async move {
                    let _ = upgrade(client, upstream, acceptor, starttls, seen).await;
                });
            }
        });

        Self {
            implicit_port,
            starttls_port,
            cleartext,
        }
    }
}

/// One connection to the STARTTLS port: relay the server's greeting, answer
/// the client's STARTTLS, then carry on as TLS in front of the server.
async fn upgrade(
    mut client: TcpStream,
    upstream: u16,
    acceptor: tokio_rustls::TlsAcceptor,
    starttls: Starttls,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut server = TcpStream::connect(("127.0.0.1", upstream)).await?;
    let mut greeting = String::new();
    BufReader::new(&mut server).read_line(&mut greeting).await?;
    client.write_all(greeting.as_bytes()).await?;

    loop {
        // Read exactly one line, a byte at a time, so that nothing of the
        // TLS handshake that follows is swallowed into a buffer.
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            if tokio::io::AsyncReadExt::read(&mut client, &mut byte).await? == 0 {
                return Ok(());
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let text = String::from_utf8_lossy(&line).trim_end().to_owned();
        seen.lock().unwrap().push(text.clone());
        let mut words = text.split_whitespace();
        let (tag, command) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        if !command.eq_ignore_ascii_case("STARTTLS") {
            client
                .write_all(format!("{tag} BAD Only STARTTLS is served here\r\n").as_bytes())
                .await?;
            continue;
        }
        match starttls {
            Starttls::Refused => {
                client
                    .write_all(format!("{tag} BAD Unknown command\r\n").as_bytes())
                    .await?;
            }
            Starttls::Offered => {
                client
                    .write_all(format!("{tag} OK Begin TLS negotiation now\r\n").as_bytes())
                    .await?;
                let mut tls = acceptor.accept(client).await?;
                tokio::io::copy_bidirectional(&mut tls, &mut server).await?;
                return Ok(());
            }
        }
    }
}

pub fn backend(config: ImapConfig, password: &'static str) -> ImapBackend {
    ImapBackend::new(config, Arc::new(move || Ok(password.to_owned())))
}

/// A plain-text message from an invented sender.
pub fn message(from: &str, subject: &str, date: &str, body: &str) -> String {
    format!(
        "From: {from}\r\nTo: Robin Vale <robin@example.com>\r\nSubject: {subject}\r\n\
         Date: {date}\r\nMessage-ID: <{}@example.com>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
        subject
            .to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
    )
}

/// Appends `count` numbered messages to `folder`.
pub async fn fill(session: &mut Session<TcpStream>, folder: &str, count: usize) {
    for n in 1..=count {
        let raw = message(
            "Ana Ruiz <ana.ruiz@example.com>",
            &format!("Message {n}"),
            &format!(
                "Sat, 14 Nov 2026 09:{:02}:{:02} +0000",
                (n / 60) % 60,
                n % 60
            ),
            &format!("Body of message {n}."),
        );
        session
            .append(folder, None, None, raw.as_bytes())
            .await
            .unwrap();
    }
}
