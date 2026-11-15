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

/// A server without some of Dovecot's extensions: a relay in front of it
/// that takes the named capabilities out of what Dovecot advertises and
/// answers BAD to the commands they add, as a server that had never heard
/// of them would. It keeps every command it is sent, so a test can see what
/// the backend really did.
///
/// It can also be told to misbehave in ways Dovecot will not: to answer NO
/// to commands of the test's choosing, or to stop answering altogether. And
/// it knows how many clients are connected to it, which is how a test sees
/// a connection being closed.
pub struct Without {
    /// Plaintext, like Dovecot's own port.
    pub port: u16,
    control: Arc<Control>,
}

/// What a test has asked of a [`Without`], shared with its connections.
#[derive(Default)]
struct Control {
    /// Capability names, upper case.
    hidden: Vec<String>,
    commands: std::sync::Mutex<Vec<String>>,
    /// Commands starting with any of these are answered NO.
    refused: std::sync::Mutex<Vec<String>>,
    /// While set, commands are taken and never answered.
    silent: std::sync::atomic::AtomicBool,
    /// How many clients are connected.
    connections: std::sync::atomic::AtomicUsize,
}

impl Without {
    /// `hidden` are capability names: `MOVE`, `UIDPLUS`, `IDLE`.
    pub async fn start(upstream: u16, hidden: &[&str]) -> Self {
        let control = Arc::new(Control {
            hidden: hidden.iter().map(|name| name.to_uppercase()).collect(),
            ..Control::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::clone(&control);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let control = Arc::clone(&shared);
                tokio::spawn(async move {
                    use std::sync::atomic::Ordering;
                    control.connections.fetch_add(1, Ordering::SeqCst);
                    let _ = relay(client, upstream, Arc::clone(&control)).await;
                    control.connections.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self { port, control }
    }

    /// Every command sent so far, without its tag, oldest first.
    pub fn commands(&self) -> Vec<String> {
        self.control.commands.lock().unwrap().clone()
    }

    /// How many clients are connected now. A client that has closed its
    /// end is no longer counted, whatever the server behind has done.
    pub fn connections(&self) -> usize {
        self.control
            .connections
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Forgets the commands seen so far.
    pub fn clear(&self) {
        self.control.commands.lock().unwrap().clear();
    }

    /// From now on, answers NO to every command that starts with one of
    /// `commands`, without passing it on. An empty list refuses nothing.
    pub fn refuse(&self, commands: &[&str]) {
        *self.control.refused.lock().unwrap() = commands
            .iter()
            .map(|command| command.to_uppercase())
            .collect();
    }

    /// Stops answering, or starts again. While silent the relay takes what
    /// it is sent and says nothing, like a server that has hung.
    pub fn fall_silent(&self, silent: bool) {
        self.control
            .silent
            .store(silent, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The length of the literal a line announces at its end, if it does.
fn literal_length(line: &[u8]) -> Option<usize> {
    let line = line.strip_suffix(b"\r\n")?.strip_suffix(b"}")?;
    let open = line.iter().rposition(|byte| *byte == b'{')?;
    let digits = std::str::from_utf8(&line[open + 1..]).ok()?;
    digits.strip_suffix('+').unwrap_or(digits).parse().ok()
}

/// Whether `command` is one that only a hidden extension provides.
fn needs_hidden(command: &str, hidden: &[String]) -> bool {
    let command = command.to_uppercase();
    let has = |name: &str| hidden.iter().any(|hidden| hidden == name);
    (has("MOVE") && (command.starts_with("UID MOVE ") || command.starts_with("MOVE ")))
        || (has("UIDPLUS") && command.starts_with("UID EXPUNGE"))
}

/// Takes the hidden capabilities out of a line that lists capabilities, and
/// with UIDPLUS hidden, the response codes only UIDPLUS adds.
fn hide(line: Vec<u8>, hidden: &[String]) -> Vec<u8> {
    let Ok(mut text) = String::from_utf8(line.clone()) else {
        return line;
    };
    if text.contains("CAPABILITY") {
        for name in hidden {
            for (end, kept) in [(" ", " "), ("]", "]"), ("\r\n", "\r\n")] {
                text = text.replace(&format!(" {name}{end}"), kept);
            }
        }
    }
    if hidden.iter().any(|name| name == "UIDPLUS") {
        for code in ["[COPYUID ", "[APPENDUID "] {
            if let Some(start) = text.find(code)
                && let Some(length) = text[start..].find("] ")
            {
                text.replace_range(start..start + length + 2, "");
            }
        }
    }
    text.into_bytes()
}

/// One connection through a [`Without`].
async fn relay(client: TcpStream, upstream: u16, control: Arc<Control>) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let server = TcpStream::connect(("127.0.0.1", upstream)).await?;
    let (client_read, client_write) = client.into_split();
    let (server_read, mut server_write) = server.into_split();
    let client_write = Arc::new(tokio::sync::Mutex::new(client_write));
    // How the server answered each line: `true` when it asked for a literal,
    // `false` when it ended the command.
    let (answered, mut answers) = tokio::sync::mpsc::unbounded_channel::<bool>();

    let to_client = Arc::clone(&client_write);
    let shared = Arc::clone(&control);
    let down = tokio::spawn(async move {
        let mut server = BufReader::new(server_read);
        // False while inside a response that a literal has interrupted.
        let mut fresh = true;
        loop {
            let mut line = Vec::new();
            if server.read_until(b'\n', &mut line).await? == 0 {
                return std::io::Result::Ok(());
            }
            let literal = literal_length(&line);
            if fresh {
                if line.starts_with(b"+") {
                    let _ = answered.send(true);
                } else if !line.starts_with(b"*") {
                    let _ = answered.send(false);
                }
                line = hide(line, &shared.hidden);
            }
            let mut client = to_client.lock().await;
            client.write_all(&line).await?;
            if let Some(length) = literal {
                let mut bytes = vec![0; length];
                server.read_exact(&mut bytes).await?;
                client.write_all(&bytes).await?;
            }
            fresh = literal.is_none();
        }
    });

    let mut client = BufReader::new(client_read);
    loop {
        let mut line = Vec::new();
        if client.read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line).trim_end().to_owned();
        // A line of one word has no tag: it is the `DONE` that ends an
        // `IDLE`, and is kept as the command it is.
        let (tag, command) = text.split_once(' ').unwrap_or(("", &text));
        control.commands.lock().unwrap().push(command.to_owned());
        if control.silent.load(std::sync::atomic::Ordering::SeqCst) {
            continue;
        }
        let refused = {
            let upper = command.to_uppercase();
            let refused = control.refused.lock().unwrap();
            refused.iter().any(|start| upper.starts_with(start))
        };
        let answer = if needs_hidden(command, &control.hidden) {
            Some("BAD Unknown command")
        } else if refused {
            Some("NO Refused for the test")
        } else {
            None
        };
        if let Some(answer) = answer {
            client_write
                .lock()
                .await
                .write_all(format!("{tag} {answer}\r\n").as_bytes())
                .await?;
            continue;
        }
        // Answers to earlier commands are of no interest any more.
        while answers.try_recv().is_ok() {}
        server_write.write_all(&line).await?;
        // The client sends a literal, and the line after it, only once the
        // server has asked. If the server ends the command instead, what
        // comes next is a new command.
        let mut literal = literal_length(&line);
        while let Some(length) = literal {
            if answers.recv().await != Some(true) {
                break;
            }
            let mut bytes = vec![0; length];
            client.read_exact(&mut bytes).await?;
            server_write.write_all(&bytes).await?;
            let mut rest = Vec::new();
            client.read_until(b'\n', &mut rest).await?;
            server_write.write_all(&rest).await?;
            literal = literal_length(&rest);
        }
    }
    down.abort();
    Ok(())
}
