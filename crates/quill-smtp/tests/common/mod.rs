//! An SMTP submission server for the tests, in this process.
//!
//! It speaks enough of the protocol to take a message in each of the three
//! TLS modes and with each sign-in mechanism, follows a [`Script`] for
//! everything a test wants to go wrong, and keeps a [`Log`] of what the
//! client sent and on which side of the TLS upgrade.
//!
//! It is strict where the client must be right: a message ends only at a
//! dot between two CRLFs, and credentials are compared exactly. Everyone in
//! the mail is invented and lives at example.com. Nothing here listens
//! anywhere but the loopback address.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::config::TlsMode;
use quill_core::{Envelope, Sender};
use quill_smtp::{Credential, DEFAULT_HELLO_NAME, SmtpAuth, SmtpConfig, SmtpSender, Trust};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use ring::{digest, hmac, pbkdf2};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

pub const USER: &str = "ana.ruiz@example.com";
pub const PASSWORD: &str = "test-password";
pub const TOKEN: &str = "test-access-token";

/// A certificate authority and a certificate it signed.
#[derive(Clone)]
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
        let leaf = leaf_params(names, days_ago, days)
            .signed_by(&key, &issuer)
            .unwrap();
        Self {
            ca_der: ca.der().to_vec(),
            leaf_der: leaf.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }

    pub fn valid() -> Self {
        Self::issue(&["localhost"], 1, 90)
    }

    /// A certificate for `localhost` signed by its own key, as a home
    /// server has. There is no authority to trust; `ca_der` is empty.
    pub fn self_signed() -> Self {
        let key = KeyPair::generate().unwrap();
        let leaf = leaf_params(&["localhost"], 1, 90)
            .self_signed(&key)
            .unwrap();
        Self {
            ca_der: Vec::new(),
            leaf_der: leaf.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }

    /// SHA-256 of the server certificate: what an account pins.
    pub fn fingerprint(&self) -> [u8; 32] {
        digest::digest(&digest::SHA256, &self.leaf_der)
            .as_ref()
            .try_into()
            .unwrap()
    }

    fn acceptor(&self) -> TlsAcceptor {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let key = PrivateKeyDer::try_from(self.key_der.clone()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(self.leaf_der.clone())], key)
        .unwrap();
        TlsAcceptor::from(Arc::new(config))
    }
}

fn leaf_params(names: &[&str], days_ago: i64, days: i64) -> CertificateParams {
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

/// What a STARTTLS port does about the upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Starttls {
    Offered,
    /// Not advertised, and answered as an unknown command.
    Missing,
    /// Advertised, then answered with "not available just now".
    Unavailable,
    /// Offered, with a forged `EHLO` reply sent in the clear right behind
    /// the go-ahead, as someone in the middle of the connection could.
    Injecting,
}

/// How the server plays its part in SCRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scram {
    Honest,
    /// Accepts the client's proof, then signs with the wrong key: a server
    /// that does not know the password.
    Forging,
    /// Accepts the client's proof and says yes without signing anything.
    NotSigning,
}

/// What the server offers and where it departs from a plain success.
#[derive(Debug, Clone)]
pub struct Script {
    pub starttls: Starttls,
    /// Extensions advertised in the clear on a STARTTLS port, before the
    /// upgrade. `STARTTLS` itself is added when it is offered.
    pub before_tls: Vec<&'static str>,
    /// Extensions advertised on the channel mail is sent over.
    pub extensions: Vec<&'static str>,
    /// Whether `MAIL FROM` is refused until the client has signed in.
    pub require_auth: bool,
    pub scram: Scram,
    /// Recipients that are refused, each with the reply it gets.
    pub refuse: Vec<(&'static str, &'static str)>,
    pub mail_reply: &'static str,
    /// The reply to `DATA`. Anything but a 354 means no message is read.
    pub data_reply: &'static str,
    /// The reply once the whole message has arrived. `None` is a server
    /// that takes the message and never says so.
    pub accepted_reply: Option<&'static str>,
    /// Whether the server greets at all. If not it accepts connections and
    /// says nothing.
    pub greets: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            starttls: Starttls::Offered,
            before_tls: vec!["PIPELINING", "8BITMIME"],
            extensions: vec![
                "PIPELINING",
                "SIZE 10240000",
                "AUTH PLAIN LOGIN XOAUTH2 OAUTHBEARER SCRAM-SHA-256",
                "ENHANCEDSTATUSCODES",
                "8BITMIME",
                "SMTPUTF8",
            ],
            require_auth: true,
            scram: Scram::Honest,
            refuse: Vec::new(),
            mail_reply: "250 2.1.0 Ok",
            data_reply: "354 End data with <CR><LF>.<CR><LF>",
            accepted_reply: Some("250 2.0.0 Ok: queued as 4F2A91"),
            greets: true,
        }
    }
}

/// One message the server accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub from: String,
    /// The parameters after the address in `MAIL FROM`.
    pub parameters: String,
    pub to: Vec<String>,
    /// The message as received, dot-stuffing undone.
    pub data: Vec<u8>,
}

/// What the server saw.
#[derive(Debug, Clone, Default)]
pub struct Log {
    /// Every line received outside TLS, sign-in lines included.
    pub cleartext: Vec<String>,
    /// Every command line, on either side of the upgrade, in order.
    pub commands: Vec<String>,
    /// The mechanism of each sign-in that succeeded.
    pub signed_in: Vec<&'static str>,
    /// Lines the client sent during a sign-in after its first.
    pub sign_in_lines: Vec<String>,
    /// Each `DATA` body exactly as transmitted, without the final dot line.
    pub wire: Vec<Vec<u8>>,
    pub delivered: Vec<Delivered>,
}

impl Log {
    /// Whether any command began with `verb`.
    pub fn saw(&self, verb: &str) -> bool {
        self.commands
            .iter()
            .any(|line| line.to_ascii_uppercase().starts_with(verb))
    }
}

pub struct Server {
    pub port: u16,
    pub mode: TlsMode,
    pub certificates: Certificates,
    log: Arc<Mutex<Log>>,
}

impl Server {
    /// A server for one TLS mode, with a certificate the test CA signed for
    /// `localhost`.
    pub async fn start(mode: TlsMode, script: Script) -> Self {
        Self::start_with(mode, script, Certificates::valid()).await
    }

    pub async fn start_with(mode: TlsMode, script: Script, certificates: Certificates) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Log::default()));
        let acceptor = certificates.acceptor();
        let script = Arc::new(script);
        let shared = Arc::clone(&log);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (acceptor, script, log) =
                    (acceptor.clone(), Arc::clone(&script), Arc::clone(&shared));
                tokio::spawn(async move {
                    let _ = serve(client, mode, port, acceptor, &script, &log).await;
                });
            }
        });
        Self {
            port,
            mode,
            certificates,
            log,
        }
    }

    /// What the server has seen so far.
    pub fn log(&self) -> Log {
        self.log.lock().unwrap().clone()
    }

    /// How to reach the server, trusting the test CA.
    pub fn config(&self) -> SmtpConfig {
        SmtpConfig {
            host: "localhost".to_owned(),
            port: self.port,
            tls: self.mode,
            username: USER.to_owned(),
            trust: Trust {
                extra_roots: if self.certificates.ca_der.is_empty() {
                    Vec::new()
                } else {
                    vec![self.certificates.ca_der.clone()]
                },
                pinned_sha256: None,
            },
            auth: SmtpAuth::Automatic,
            hello_name: DEFAULT_HELLO_NAME.to_owned(),
        }
    }

    /// A sender for the test account, signing in with its password.
    pub fn sender(&self) -> SmtpSender {
        with_password(self.config(), PASSWORD)
    }
}

pub fn with_password(config: SmtpConfig, password: &'static str) -> SmtpSender {
    SmtpSender::new(
        config,
        Arc::new(move || Ok(Credential::Password(password.to_owned()))),
    )
}

pub fn with_token(config: SmtpConfig, token: &'static str) -> SmtpSender {
    SmtpSender::new(
        config,
        Arc::new(move || Ok(Credential::OAuthToken(token.to_owned()))),
    )
}

/// Sends, and fails the test instead of hanging if the client never comes
/// back.
pub async fn send(sender: &SmtpSender, raw: &[u8], envelope: &Envelope) -> quill_core::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), sender.send(raw, envelope))
        .await
        .expect("the send neither finished nor failed")
}

pub fn envelope(to: &[&str]) -> Envelope {
    Envelope {
        from: USER.to_owned(),
        to: to.iter().map(|address| (*address).to_owned()).collect(),
    }
}

/// A short plain-text message from the test account.
pub fn message(subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Ana Ruiz <{USER}>\r\nTo: Robin Vale <robin@example.com>\r\nSubject: {subject}\r\n\
         Date: Sat, 14 Nov 2026 09:00:00 +0000\r\nMessage-ID: <{}@example.com>\r\n\
         MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
        subject
            .to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
    )
    .into_bytes()
}

/// One connection, in the server's TLS mode.
async fn serve(
    mut client: TcpStream,
    mode: TlsMode,
    port: u16,
    acceptor: TlsAcceptor,
    script: &Script,
    log: &Mutex<Log>,
) -> std::io::Result<()> {
    if !script.greets {
        // Hold the connection open and say nothing, for as long as the
        // client cares to wait.
        let mut sink = [0u8; 64];
        while client.read(&mut sink).await? > 0 {}
        return Ok(());
    }
    let mut session = Session {
        port,
        script,
        log,
        encrypted: false,
        before_upgrade: false,
    };
    match mode {
        TlsMode::Implicit => {
            let mut tls = acceptor.accept(client).await?;
            session.encrypted = true;
            line(&mut tls, "220 localhost ESMTP ready").await?;
            session.run(&mut tls).await?;
        }
        TlsMode::Starttls => {
            session.before_upgrade = true;
            line(&mut client, "220 localhost ESMTP ready").await?;
            if session.run(&mut client).await? == End::Upgrade {
                let mut tls = acceptor.accept(client).await?;
                session.encrypted = true;
                session.before_upgrade = false;
                session.run(&mut tls).await?;
            }
        }
        TlsMode::None => {
            line(&mut client, "220 localhost ESMTP ready").await?;
            session.run(&mut client).await?;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum End {
    Closed,
    /// The client asked for TLS and was told to go ahead.
    Upgrade,
}

struct Session<'a> {
    port: u16,
    script: &'a Script,
    log: &'a Mutex<Log>,
    encrypted: bool,
    /// On a STARTTLS port, still in the clear: only the upgrade is served.
    before_upgrade: bool,
}

impl Session<'_> {
    async fn run<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> std::io::Result<End> {
        let mut signed_in = false;
        let mut from: Option<(String, String)> = None;
        let mut to: Vec<String> = Vec::new();
        loop {
            let Some(command) = self.text_line(stream).await? else {
                return Ok(End::Closed);
            };
            self.log.lock().unwrap().commands.push(command.clone());
            let (verb, rest) = command.split_once(' ').unwrap_or((command.as_str(), ""));
            let verb = verb.to_ascii_uppercase();
            match verb.as_str() {
                "EHLO" => {
                    let mut lines = vec!["localhost at your service".to_owned()];
                    if self.before_upgrade {
                        lines.extend(self.script.before_tls.iter().map(|ext| (*ext).to_owned()));
                        if self.script.starttls != Starttls::Missing {
                            lines.push("STARTTLS".to_owned());
                        }
                    } else {
                        lines.extend(self.script.extensions.iter().map(|ext| (*ext).to_owned()));
                    }
                    let last = lines.len() - 1;
                    let mut reply = String::new();
                    for (index, text) in lines.iter().enumerate() {
                        let separator = if index == last { ' ' } else { '-' };
                        reply.push_str(&format!("250{separator}{text}\r\n"));
                    }
                    stream.write_all(reply.as_bytes()).await?;
                    stream.flush().await?;
                }
                "STARTTLS" if self.before_upgrade => match self.script.starttls {
                    Starttls::Missing => line(stream, "502 5.5.1 Command not implemented").await?,
                    Starttls::Unavailable => {
                        line(stream, "454 4.7.0 TLS not available due to local problem").await?;
                    }
                    Starttls::Offered => {
                        line(stream, "220 2.0.0 Ready to start TLS").await?;
                        return Ok(End::Upgrade);
                    }
                    Starttls::Injecting => {
                        // One write, so the forgery reaches the client in
                        // the same read as the go-ahead.
                        stream
                            .write_all(
                                b"220 2.0.0 Ready to start TLS\r\n\
                                  250-forged.example.com\r\n250-SIZE 10\r\n250 AUTH PLAIN\r\n",
                            )
                            .await?;
                        stream.flush().await?;
                        return Ok(End::Upgrade);
                    }
                },
                _ if self.before_upgrade && verb != "QUIT" && verb != "RSET" => {
                    line(stream, "530 5.7.0 Must issue a STARTTLS command first").await?;
                }
                "AUTH" => {
                    if let Some(mechanism) = self.sign_in(stream, rest).await? {
                        signed_in = true;
                        self.log.lock().unwrap().signed_in.push(mechanism);
                        line(stream, "235 2.7.0 Authentication successful").await?;
                    }
                }
                "MAIL" => {
                    if self.script.require_auth && !signed_in {
                        line(stream, "530 5.7.0 Authentication required").await?;
                        continue;
                    }
                    if self.script.mail_reply.starts_with('2') {
                        let (address, parameters) = path(rest);
                        from = Some((address, parameters));
                        to.clear();
                    }
                    line(stream, self.script.mail_reply).await?;
                }
                "RCPT" => {
                    if from.is_none() {
                        line(stream, "503 5.5.1 Need MAIL command first").await?;
                        continue;
                    }
                    let (address, _) = path(rest);
                    let refusal = self
                        .script
                        .refuse
                        .iter()
                        .find(|(refused, _)| *refused == address);
                    match refusal {
                        Some((_, reply)) => line(stream, reply).await?,
                        None => {
                            to.push(address);
                            line(stream, "250 2.1.5 Ok").await?;
                        }
                    }
                }
                "DATA" => {
                    let Some((sender, parameters)) = from.clone() else {
                        line(stream, "503 5.5.1 Need MAIL command first").await?;
                        continue;
                    };
                    if to.is_empty() {
                        line(stream, "554 5.5.1 No valid recipients").await?;
                        continue;
                    }
                    line(stream, self.script.data_reply).await?;
                    if !self.script.data_reply.starts_with("354") {
                        continue;
                    }
                    let (mut wire, mut data) = (Vec::new(), Vec::new());
                    loop {
                        let Some(raw) = raw_line(stream).await? else {
                            return Ok(End::Closed);
                        };
                        // Only a dot between two CRLFs ends the message.
                        if raw == b".\r\n" {
                            break;
                        }
                        wire.extend_from_slice(&raw);
                        data.extend_from_slice(raw.strip_prefix(b".").unwrap_or(&raw));
                    }
                    self.log.lock().unwrap().wire.push(wire);
                    let Some(reply) = self.script.accepted_reply else {
                        // Took it all, and now says nothing.
                        let mut sink = [0u8; 64];
                        while stream.read(&mut sink).await? > 0 {}
                        return Ok(End::Closed);
                    };
                    if reply.starts_with('2') {
                        self.log.lock().unwrap().delivered.push(Delivered {
                            from: sender,
                            parameters,
                            to: std::mem::take(&mut to),
                            data,
                        });
                    }
                    from = None;
                    line(stream, reply).await?;
                }
                "RSET" => {
                    from = None;
                    to.clear();
                    line(stream, "250 2.0.0 Ok").await?;
                }
                "NOOP" => line(stream, "250 2.0.0 Ok").await?,
                "QUIT" => {
                    line(stream, "221 2.0.0 Bye").await?;
                    return Ok(End::Closed);
                }
                _ => line(stream, "500 5.5.2 Command not recognized").await?,
            }
        }
    }

    /// One line from the client as text, noted if it came in the clear.
    async fn text_line<S: AsyncRead + Unpin>(
        &self,
        stream: &mut S,
    ) -> std::io::Result<Option<String>> {
        let Some(raw) = raw_line(stream).await? else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&raw)
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if !self.encrypted {
            self.log.lock().unwrap().cleartext.push(text.clone());
        }
        Ok(Some(text))
    }

    /// A line sent by the client part way through a sign-in.
    async fn sign_in_line<S: AsyncRead + Unpin>(&self, stream: &mut S) -> std::io::Result<String> {
        let text = self.text_line(stream).await?.unwrap_or_default();
        self.log.lock().unwrap().sign_in_lines.push(text.clone());
        Ok(text)
    }

    /// Runs one `AUTH` command. Returns the mechanism if the client proved
    /// who it is; otherwise the refusal has already been sent.
    async fn sign_in<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        stream: &mut S,
        arguments: &str,
    ) -> std::io::Result<Option<&'static str>> {
        const REFUSAL: &str = "535 5.7.8 Authentication credentials invalid";
        let (mechanism, initial) = arguments.split_once(' ').unwrap_or((arguments, ""));
        let offered = self
            .script
            .extensions
            .iter()
            .filter_map(|ext| ext.strip_prefix("AUTH "))
            .flat_map(str::split_ascii_whitespace)
            .any(|name| name == mechanism);
        if !offered {
            line(stream, "504 5.5.4 Unrecognized authentication type").await?;
            return Ok(None);
        }
        match mechanism {
            "PLAIN" => {
                if decode(initial) == format!("\0{USER}\0{PASSWORD}") {
                    return Ok(Some("PLAIN"));
                }
                line(stream, REFUSAL).await?;
            }
            "LOGIN" => {
                line(stream, "334 VXNlcm5hbWU6").await?;
                let user = decode(&self.sign_in_line(stream).await?);
                line(stream, "334 UGFzc3dvcmQ6").await?;
                let password = decode(&self.sign_in_line(stream).await?);
                if user == USER && password == PASSWORD {
                    return Ok(Some("LOGIN"));
                }
                line(stream, REFUSAL).await?;
            }
            "XOAUTH2" => {
                if decode(initial) == format!("user={USER}\x01auth=Bearer {TOKEN}\x01\x01") {
                    return Ok(Some("XOAUTH2"));
                }
                // The reasons first, as a challenge the client must answer
                // before it is told no.
                let reasons = BASE64.encode(r#"{"status":"401","schemes":"bearer"}"#);
                line(stream, &format!("334 {reasons}")).await?;
                self.sign_in_line(stream).await?;
                line(stream, REFUSAL).await?;
            }
            "OAUTHBEARER" => {
                let expected = format!(
                    "n,a={USER},\x01host=localhost\x01port={}\x01auth=Bearer {TOKEN}\x01\x01",
                    self.port
                );
                if decode(initial) == expected {
                    return Ok(Some("OAUTHBEARER"));
                }
                let reasons = BASE64.encode(r#"{"status":"invalid_token"}"#);
                line(stream, &format!("334 {reasons}")).await?;
                self.sign_in_line(stream).await?;
                line(stream, REFUSAL).await?;
            }
            "SCRAM-SHA-256" => return self.scram(stream, initial).await,
            _ => line(stream, "504 5.5.4 Unrecognized authentication type").await?,
        }
        Ok(None)
    }

    /// The server's side of SCRAM-SHA-256, worked out here from the RFC and
    /// not with the client's code, so that the two check each other.
    async fn scram<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        stream: &mut S,
        initial: &str,
    ) -> std::io::Result<Option<&'static str>> {
        const SALT: &[u8] = b"quill test salt!";
        const ITERATIONS: u32 = 4096;
        const REFUSAL: &str = "535 5.7.8 Authentication credentials invalid";

        let client_first = decode(initial);
        let Some(bare) = client_first.strip_prefix("n,,") else {
            line(stream, "501 5.5.2 No channel binding here").await?;
            return Ok(None);
        };
        let attribute = |message: &str, name: &str| {
            message
                .split(',')
                .find_map(|part| part.strip_prefix(name))
                .unwrap_or_default()
                .to_owned()
        };
        let (user, client_nonce) = (attribute(bare, "n="), attribute(bare, "r="));
        let nonce = format!("{client_nonce}3rfcNHYJY1ZVvWVs7j");
        let server_first = format!("r={nonce},s={},i={ITERATIONS}", BASE64.encode(SALT));
        line(stream, &format!("334 {}", BASE64.encode(&server_first))).await?;

        let client_final = decode(&self.sign_in_line(stream).await?);
        let Some((without_proof, proof)) = client_final.rsplit_once(",p=") else {
            line(stream, "501 5.5.2 Malformed final message").await?;
            return Ok(None);
        };

        let mut salted = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            std::num::NonZeroU32::new(ITERATIONS).unwrap(),
            SALT,
            PASSWORD.as_bytes(),
            &mut salted,
        );
        let sign = |key: &[u8], data: &[u8]| {
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data)
                .as_ref()
                .to_vec()
        };
        let client_key = sign(&salted, b"Client Key");
        let stored_key = digest::digest(&digest::SHA256, &client_key);
        let auth_message = format!("{bare},{server_first},{without_proof}");
        let client_signature = sign(stored_key.as_ref(), auth_message.as_bytes());
        let expected: Vec<u8> = client_key
            .iter()
            .zip(&client_signature)
            .map(|(key, signature)| key ^ signature)
            .collect();

        let proven = user == USER
            && client_nonce.len() >= 16
            && without_proof == format!("c=biws,r={nonce}")
            && BASE64.decode(proof).ok() == Some(expected);
        if !proven {
            line(stream, REFUSAL).await?;
            return Ok(None);
        }

        let server_key = match self.script.scram {
            Scram::Forging => sign(b"not the salted password", b"Server Key"),
            Scram::Honest | Scram::NotSigning => sign(&salted, b"Server Key"),
        };
        if self.script.scram == Scram::NotSigning {
            return Ok(Some("SCRAM-SHA-256"));
        }
        let server_final = format!(
            "v={}",
            BASE64.encode(sign(&server_key, auth_message.as_bytes()))
        );
        line(stream, &format!("334 {}", BASE64.encode(server_final))).await?;
        // An empty line accepts the server's proof; anything else calls
        // the sign-in off.
        if self.sign_in_line(stream).await?.is_empty() {
            return Ok(Some("SCRAM-SHA-256"));
        }
        line(stream, "501 5.7.0 Authentication aborted").await?;
        Ok(None)
    }
}

/// The address between the angle brackets of `FROM:<..>` or `TO:<..>`, and
/// whatever follows it.
fn path(argument: &str) -> (String, String) {
    let Some((_, rest)) = argument.split_once('<') else {
        return (String::new(), String::new());
    };
    let (address, parameters) = rest.split_once('>').unwrap_or((rest, ""));
    (address.to_owned(), parameters.trim().to_owned())
}

fn decode(base64: &str) -> String {
    BASE64
        .decode(base64)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

async fn line<S: AsyncWrite + Unpin>(stream: &mut S, text: &str) -> std::io::Result<()> {
    stream.write_all(format!("{text}\r\n").as_bytes()).await?;
    stream.flush().await
}

/// One line with its line ending, read a byte at a time so that nothing of
/// a TLS handshake that follows is swallowed into a buffer. `None` at the
/// end of the connection.
async fn raw_line<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await? == 0 {
            return Ok(None);
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(Some(line));
        }
    }
}
