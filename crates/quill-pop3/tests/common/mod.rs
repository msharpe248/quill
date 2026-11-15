//! A real POP3 server for the tests: Dovecot, started unprivileged in a
//! temporary directory on a port of its own.
//!
//! Dovecot speaks plaintext POP3 here and the tests put TLS in front of it
//! themselves ([`TlsFront`]). Run unprivileged on macOS, Dovecot's own TLS
//! relay drops every session straight after sign-in; and owning the TLS end
//! lets a test choose exactly which certificate a client is shown.
//!
//! Every user name signs in with the same password and gets an empty
//! mailbox, so tests do not share state. Mail is delivered the way a mail
//! server delivers it, by writing a file into the user's Maildir. Everyone
//! in the mail is invented and lives at example.com.
//!
//! [`RawPop3`] is a second, deliberately simple POP3 client. Tests use it
//! to look at the mailbox and to change it behind the backend's back, so
//! that what the backend reports is checked against something other than
//! the backend.

#![allow(dead_code)]

pub mod fake;

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use quill_core::config::TlsMode;
use quill_pop3::{Pop3Backend, Pop3Config, Trust};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub const PASSWORD: &str = "test-password";

/// A server certificate for `localhost`, and the authority that signed it
/// when there is one.
pub struct Certificates {
    /// Empty for a self-signed certificate.
    pub ca_der: Vec<u8>,
    pub leaf_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Certificates {
    fn server_params(names: &[&str], days_ago: i64, days: i64) -> CertificateParams {
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
        params
    }

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
        let leaf = Self::server_params(names, days_ago, days)
            .signed_by(&key, &issuer)
            .unwrap();
        Self {
            ca_der: ca.der().to_vec(),
            leaf_der: leaf.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }

    /// A certificate that vouches for itself, as a home server's does.
    pub fn self_signed() -> Self {
        let key = KeyPair::generate().unwrap();
        let leaf = Self::server_params(&["localhost"], 1, 90)
            .self_signed(&key)
            .unwrap();
        Self {
            ca_der: Vec::new(),
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
    pub pop3_port: u16,
    /// TLS in front of it, with a certificate the test CA signed.
    pub front: TlsFront,
    pub certificates: Certificates,
    delivered: AtomicU64,
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
            .prefix("qp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = dir.path().display().to_string();
        let certificates = Certificates::valid();
        let pop3_port = free_port();
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
  pop3 = yes
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
service pop3-login {{
  chroot =
  inet_listener pop3 {{
    port = {pop3_port}
  }}
  inet_listener pop3s {{
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
        let front = TlsFront::start(pop3_port, &certificates, Stls::Offered).await;
        let server = Self {
            dir,
            child,
            pop3_port,
            front,
            certificates,
            delivered: AtomicU64::new(0),
        };
        server.wait_until_ready().await;
        Some(server)
    }

    /// The server answers before its sign-in service does, so wait for a
    /// sign-in to succeed, not just for the port to open.
    async fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(session) = RawPop3::sign_in(self.pop3_port, "ready-check@example.com").await
            {
                session.quit().await;
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

    /// Delivers `raw` to `user`'s inbox the way a mail server does: a new
    /// file in the Maildir's `new` directory. The file names sort in the
    /// order of delivery, which is the order Dovecot numbers them in.
    pub fn deliver(&self, user: &str, raw: &str) {
        let maildir = self.dir.path().join("mail").join(user).join("Maildir");
        for part in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(part)).unwrap();
        }
        let n = self.delivered.fetch_add(1, Ordering::SeqCst) + 1;
        let name = format!("{}.M{n:06}P1Q{n}.localhost", 1_794_600_000 + n);
        // Written elsewhere and moved in, so the server never sees half a file.
        let staged = maildir.join("tmp").join(&name);
        std::fs::write(&staged, raw).unwrap();
        std::fs::rename(staged, maildir.join("new").join(name)).unwrap();
    }

    /// Delivers `count` numbered messages, continuing from `first`.
    pub fn fill(&self, user: &str, first: usize, count: usize) {
        for n in first..first + count {
            self.deliver(user, &numbered(n));
        }
    }

    /// A raw session for looking at the mailbox and changing it behind the
    /// backend's back, the way another mail program would.
    pub async fn session(&self, user: &str) -> RawPop3 {
        RawPop3::sign_in(self.pop3_port, user)
            .await
            .expect("a test session")
    }

    /// The unique ids on the server right now, in message-number order.
    pub async fn uidls(&self, user: &str) -> Vec<String> {
        let mut session = self.session(user).await;
        let ids = session.uidls().await;
        session.quit().await;
        ids
    }

    /// Deletes the messages with these unique ids, as another mail program
    /// would: `DELE` each, then `QUIT`.
    pub async fn delete_elsewhere(&self, user: &str, ids: &[&str]) {
        let mut session = self.session(user).await;
        let listed = session.uidls().await;
        for id in ids {
            let number = listed.iter().position(|listed| listed == id).unwrap() + 1;
            let reply = session.command(&format!("DELE {number}")).await;
            assert!(reply.starts_with("+OK"), "{reply}");
        }
        session.quit().await;
    }

    /// How to reach `user`'s account in the given mode, trusting the test CA.
    pub fn config(&self, tls: TlsMode, user: &str) -> Pop3Config {
        Pop3Config {
            host: "localhost".to_owned(),
            port: match tls {
                TlsMode::Implicit => self.front.implicit_port,
                TlsMode::Starttls => self.front.stls_port,
                TlsMode::None => self.pop3_port,
            },
            tls,
            username: user.to_owned(),
            trust: Trust {
                extra_roots: vec![self.certificates.ca_der.clone()],
                pinned_sha256: None,
            },
        }
    }

    pub fn backend(&self, tls: TlsMode, user: &str) -> Pop3Backend {
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

/// A second POP3 client, as plain as can be: one line out, lines back.
pub struct RawPop3 {
    io: BufReader<TcpStream>,
}

impl RawPop3 {
    async fn line(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), self.io.read_line(&mut line))
            .await
            .ok()?
            .ok()?;
        (read > 0).then(|| line.trim_end().to_owned())
    }

    /// Connects, reads the greeting and signs in. `None` if any of it fails.
    pub async fn sign_in(port: u16, user: &str) -> Option<Self> {
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let mut session = Self {
            io: BufReader::new(tcp),
        };
        if !session.line().await?.starts_with("+OK") {
            return None;
        }
        for command in [format!("USER {user}"), format!("PASS {PASSWORD}")] {
            if !session.try_command(&command).await?.starts_with("+OK") {
                return None;
            }
        }
        Some(session)
    }

    async fn try_command(&mut self, command: &str) -> Option<String> {
        self.io
            .get_mut()
            .write_all(format!("{command}\r\n").as_bytes())
            .await
            .ok()?;
        self.line().await
    }

    /// Sends a command and returns the status line.
    pub async fn command(&mut self, command: &str) -> String {
        self.try_command(command).await.expect("a reply")
    }

    /// Sends a command with a multi-line reply and returns its lines, as
    /// sent, without the closing dot.
    pub async fn listing(&mut self, command: &str) -> Vec<String> {
        let status = self.command(command).await;
        assert!(status.starts_with("+OK"), "{command}: {status}");
        let mut lines = Vec::new();
        loop {
            let line = self.line().await.expect("the rest of the reply");
            if line == "." {
                return lines;
            }
            lines.push(line);
        }
    }

    pub async fn uidls(&mut self) -> Vec<String> {
        self.listing("UIDL")
            .await
            .iter()
            .map(|line| line.split_once(' ').unwrap().1.to_owned())
            .collect()
    }

    pub async fn quit(mut self) {
        let reply = self.command("QUIT").await;
        assert!(reply.starts_with("+OK"), "{reply}");
    }
}

/// Whether a [`TlsFront`]'s STLS port agrees to upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stls {
    Offered,
    /// The server answers STLS with -ERR, as one without TLS would.
    Refused,
    /// The upgrade is offered, and someone on the wire slips a forged
    /// reply in behind the server's +OK, before TLS starts.
    Tampered,
}

/// What an attacker between client and server might add in the clear after
/// the server agrees to upgrade: an answer to the CAPA the client is about
/// to send, claiming a server that can do nothing.
pub const FORGED: &str = "+OK forged\r\n.\r\n-ERR forged\r\n-ERR forged\r\n";

/// TLS in front of a plaintext POP3 server: one port that is TLS from the
/// first byte, and one that starts in the clear and upgrades on STLS.
pub struct TlsFront {
    pub implicit_port: u16,
    pub stls_port: u16,
    /// Every line a client sent in the clear on the STLS port.
    pub cleartext: Arc<std::sync::Mutex<Vec<String>>>,
}

impl TlsFront {
    pub async fn start(upstream: u16, certificates: &Certificates, stls: Stls) -> Self {
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
        let stls_port = plain.local_addr().unwrap().port();
        let seen = Arc::clone(&cleartext);
        tokio::spawn(async move {
            while let Ok((client, _)) = plain.accept().await {
                let (acceptor, seen) = (acceptor.clone(), Arc::clone(&seen));
                tokio::spawn(async move {
                    let _ = upgrade(client, upstream, acceptor, stls, seen).await;
                });
            }
        });

        Self {
            implicit_port,
            stls_port,
            cleartext,
        }
    }
}

/// One connection to the STLS port: relay the server's greeting, answer the
/// client's STLS, then carry on as TLS in front of the server.
async fn upgrade(
    mut client: TcpStream,
    upstream: u16,
    acceptor: tokio_rustls::TlsAcceptor,
    stls: Stls,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
) -> std::io::Result<()> {
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
        if !text.eq_ignore_ascii_case("STLS") {
            client
                .write_all(b"-ERR Only STLS is served here\r\n")
                .await?;
            continue;
        }
        let agreed = match stls {
            Stls::Refused => {
                client.write_all(b"-ERR Unknown command\r\n").await?;
                continue;
            }
            Stls::Offered => "+OK Begin TLS negotiation now\r\n".to_owned(),
            // One write, so the forgery reaches the client in the same
            // packet as the +OK it hides behind.
            Stls::Tampered => format!("+OK Begin TLS negotiation now\r\n{FORGED}"),
        };
        client.write_all(agreed.as_bytes()).await?;
        let mut tls = acceptor.accept(client).await?;
        tokio::io::copy_bidirectional(&mut tls, &mut server).await?;
        return Ok(());
    }
}

pub fn backend(config: Pop3Config, password: &'static str) -> Pop3Backend {
    Pop3Backend::new(config, quill_core::credentials::password(password))
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

/// The `n`th of a run of numbered messages.
pub fn numbered(n: usize) -> String {
    message(
        "Ana Ruiz <ana.ruiz@example.com>",
        &format!("Message {n}"),
        &format!(
            "Sat, 14 Nov 2026 {:02}:{:02}:{:02} +0000",
            (n / 3600) % 24,
            (n / 60) % 60,
            n % 60
        ),
        &format!("Body of message {n}."),
    )
}
