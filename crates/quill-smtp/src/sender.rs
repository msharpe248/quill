//! `Sender` for an SMTP submission server.

use std::time::Duration;

use async_trait::async_trait;
use quill_core::config::TlsMode;
use quill_core::{Envelope, Error, RefusedRecipient, Result, Sender};
use quill_net::Stream;

use crate::auth::{self, CredentialProvider};
use crate::capabilities::Capabilities;
use crate::config::{SmtpAuth, SmtpConfig};
use crate::connection::Connection;
use crate::message::{self, Prepared};

/// How many times longer than any other wait the server gets to accept a
/// message once all of it has been sent: with the default patience, ten
/// minutes, as RFC 5321 asks. Giving up early here is the one way to send a
/// message twice, since the server may have taken it.
const ACCEPTANCE_PATIENCE: u32 = 30;

pub struct SmtpSender {
    config: SmtpConfig,
    credential: CredentialProvider,
    patience: Duration,
}

impl SmtpSender {
    pub fn new(config: SmtpConfig, credential: CredentialProvider) -> Self {
        Self {
            config,
            credential,
            patience: quill_net::PATIENCE,
        }
    }

    /// Sets how long to wait for each reply from the server before calling
    /// it unreachable. Opening the connection and the TLS handshake keep
    /// `quill_net`'s own limit.
    pub fn with_patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// Connects and signs in, and sends nothing: a way to find out whether
    /// the account's settings and credentials work before they are saved.
    pub async fn check(&self) -> Result<()> {
        if !is_hello_name(&self.config.hello_name) {
            return Err(Error::Backend(format!(
                "{:?} cannot be used as this client's name in EHLO",
                self.config.hello_name
            )));
        }
        let (mut connection, capabilities) = self.open().await?;
        let outcome = self.sign_in(&mut connection, &capabilities.auth).await;
        if !matches!(outcome, Err(Error::Connection(_))) {
            connection.quit().await;
        }
        outcome
    }

    /// Connects, leaving the connection as secure as the account's TLS mode
    /// requires and the server's extensions known.
    async fn open(&self) -> Result<(Connection, Capabilities)> {
        let config = &self.config;
        let tcp = quill_net::tcp(&config.host, config.port).await?;

        match config.tls {
            TlsMode::Implicit => {
                let stream = quill_net::handshake(&config.host, &config.trust, tcp).await?;
                let mut connection = Connection::new(stream, self.patience);
                greeting(&mut connection).await?;
                let capabilities = self.hello(&mut connection).await?;
                Ok((connection, capabilities))
            }
            TlsMode::Starttls => {
                let mut connection = Connection::new(Stream::Plain(tcp), self.patience);
                greeting(&mut connection).await?;
                // What the server claims here arrives unprotected, so it is
                // not kept, and not consulted either: the upgrade is asked
                // for whether or not it was advertised.
                self.hello(&mut connection).await?;
                // Strict: a server that will not upgrade is a failed
                // connection, never a plaintext one.
                let reply = connection.command("STARTTLS", "STARTTLS").await?;
                if reply.code != 220 {
                    connection.quit().await;
                    return Err(Error::Insecure(format!(
                        "{} would not start TLS ({}), and this account requires it",
                        config.host,
                        reply.text()
                    )));
                }
                // Taking the socket back drops the read buffer, and with it
                // anything sent in the clear after the server's go-ahead.
                let tcp = connection.into_tcp()?;
                let stream = quill_net::handshake(&config.host, &config.trust, tcp).await?;
                let mut connection = Connection::new(stream, self.patience);
                // Ask again: only what is said over TLS counts.
                let capabilities = self.hello(&mut connection).await?;
                Ok((connection, capabilities))
            }
            TlsMode::None => {
                let mut connection = Connection::new(Stream::Plain(tcp), self.patience);
                greeting(&mut connection).await?;
                let capabilities = self.hello(&mut connection).await?;
                Ok((connection, capabilities))
            }
        }
    }

    /// Sends `EHLO` and reads what the server offers.
    async fn hello(&self, connection: &mut Connection) -> Result<Capabilities> {
        let line = format!("EHLO {}", self.config.hello_name);
        let reply = connection.command(&line, "EHLO").await?;
        if reply.code != 250 {
            return Err(reply.into_error("EHLO"));
        }
        Ok(Capabilities::from_ehlo(&reply))
    }

    /// Signs in the way the account asks. Only called once the channel is
    /// as secure as the account's TLS mode requires, and the secret is only
    /// fetched here.
    async fn sign_in(&self, connection: &mut Connection, offered: &[String]) -> Result<()> {
        let mechanism = match self.config.auth {
            SmtpAuth::None => return Ok(()),
            SmtpAuth::Mechanism(mechanism) => Some(mechanism),
            SmtpAuth::Automatic => None,
        };
        let credential = (self.credential)()?;
        let mechanism = match mechanism {
            Some(mechanism) => mechanism,
            None => auth::choose(offered, &credential).ok_or_else(|| {
                let host = &self.config.host;
                Error::Auth(if offered.is_empty() {
                    format!("{host} offers no way to sign in")
                } else {
                    format!(
                        "{host} offers no way to sign in that Quill can use with this \
                         account's credentials (it offers {})",
                        offered.join(", ")
                    )
                })
            })?,
        };
        auth::sign_in(connection, mechanism, &self.config, credential).await
    }

    /// Everything after the connection is open: sign in, name the sender
    /// and the recipients, hand over the message.
    async fn submit(
        &self,
        connection: &mut Connection,
        capabilities: &Capabilities,
        message: &Prepared,
        from: &str,
        recipients: &[&str],
    ) -> Result<()> {
        // What can be known without the server's help is checked first:
        // there is no point signing in to be told no.
        let mail = mail_command(capabilities, message, from, recipients)?;
        self.sign_in(connection, &capabilities.auth).await?;

        let reply = connection.command(&mail, "MAIL FROM").await?;
        if !reply.is_success() {
            return Err(reply.into_error("MAIL FROM"));
        }

        // Every recipient is asked about, so that all the refusals can be
        // shown at once.
        let mut refused = Vec::new();
        for recipient in recipients {
            let reply = connection
                .command(&format!("RCPT TO:<{recipient}>"), "RCPT TO")
                .await?;
            if !reply.is_success() {
                refused.push(RefusedRecipient {
                    address: (*recipient).to_owned(),
                    reason: reply.text(),
                });
            }
        }
        if !refused.is_empty() {
            // To everyone or to no one: call the message off.
            let _ = connection.command("RSET", "RSET").await;
            return Err(Error::Recipients(refused));
        }

        let reply = connection.command("DATA", "DATA").await?;
        if reply.code != 354 {
            return Err(reply.into_error("DATA"));
        }
        connection
            .write(
                &message::dot_stuff(&message.bytes),
                "the message to be taken",
            )
            .await?;
        let reply = connection
            .reply_within(
                self.patience.saturating_mul(ACCEPTANCE_PATIENCE),
                "the server to accept the message",
            )
            .await?;
        if !reply.is_success() {
            return Err(reply.into_error("the message"));
        }
        Ok(())
    }
}

#[async_trait]
impl Sender for SmtpSender {
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()> {
        // A bad envelope or name is refused before anything is connected.
        let recipients = checked(envelope)?;
        if !is_hello_name(&self.config.hello_name) {
            return Err(Error::Backend(format!(
                "{:?} cannot be used as this client's name in EHLO",
                self.config.hello_name
            )));
        }
        let message = message::prepare(raw);

        let (mut connection, capabilities) = self.open().await?;
        let outcome = self
            .submit(
                &mut connection,
                &capabilities,
                &message,
                &envelope.from,
                &recipients,
            )
            .await;
        // A connection that failed is not worth a goodbye, and a server
        // that stopped answering would only keep us waiting for one.
        if !matches!(outcome, Err(Error::Connection(_))) {
            connection.quit().await;
        }
        outcome
    }
}

/// Reads the server's greeting, the first thing on a new connection.
async fn greeting(connection: &mut Connection) -> Result<()> {
    let reply = connection.reply("the server's greeting").await?;
    if reply.code != 220 {
        return Err(reply.into_error("connecting"));
    }
    Ok(())
}

/// Checks the envelope and returns the recipients, each once.
///
/// Addresses go into command lines as they are, so one with a space, an
/// angle bracket or a line break in it could become a command of its own.
/// Those are refused here, before anything is connected.
fn checked(envelope: &Envelope) -> Result<Vec<&str>> {
    if let Err(why) = check_address(&envelope.from) {
        return Err(Error::Refused(format!(
            "{:?} cannot be used as the sender's address: {why}",
            envelope.from
        )));
    }
    let mut recipients: Vec<&str> = Vec::with_capacity(envelope.to.len());
    let mut refused = Vec::new();
    for address in &envelope.to {
        match check_address(address) {
            Ok(()) => {
                if !recipients.contains(&address.as_str()) {
                    recipients.push(address);
                }
            }
            Err(why) => refused.push(RefusedRecipient {
                address: address.clone(),
                reason: format!("not an address mail can be sent to: {why}"),
            }),
        }
    }
    if !refused.is_empty() {
        return Err(Error::Recipients(refused));
    }
    if recipients.is_empty() {
        return Err(Error::Refused("the message has no recipients".to_owned()));
    }
    Ok(recipients)
}

/// Whether `address` is safe and sensible to put between angle brackets.
/// Not a full parse: the server is the judge of what it will deliver to.
fn check_address(address: &str) -> std::result::Result<(), &'static str> {
    if address
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>')
    {
        return Err("it contains a space, a bracket or a control character");
    }
    match address.rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => Ok(()),
        _ => Err("it is not of the form name@domain"),
    }
}

/// Whether `name` can follow `EHLO`: one printable ASCII word.
fn is_hello_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_graphic())
}

/// The `MAIL FROM` command for this message, or why the server on the other
/// end cannot be given it. Nothing has been sent when this fails.
fn mail_command(
    capabilities: &Capabilities,
    message: &Prepared,
    from: &str,
    recipients: &[&str],
) -> Result<String> {
    let size = message.bytes.len() as u64;
    if let Some(limit) = capabilities.size
        && size > limit
    {
        return Err(Error::TooLarge { size, limit });
    }

    let international = std::iter::once(&from)
        .chain(recipients)
        .find(|address| !address.is_ascii());
    if let Some(address) = international
        && !capabilities.smtputf8
    {
        return Err(Error::Refused(format!(
            "{address} is an internationalised address, and this server cannot \
             carry mail for those (it does not offer SMTPUTF8)"
        )));
    }
    if message.eight_bit_headers && !capabilities.smtputf8 {
        return Err(Error::Refused(
            "the message has unencoded non-ASCII text in its headers, and this \
             server does not take that (it does not offer SMTPUTF8)"
                .to_owned(),
        ));
    }
    // The spec's answer here is to re-encode the message as 7-bit. That
    // means rewriting MIME parts, which would also break any signature
    // over them, so the message is refused instead and left as written.
    if message.eight_bit && !capabilities.eight_bit_mime {
        return Err(Error::Refused(
            "the message has 8-bit content, and this server only takes 7-bit \
             mail (it does not offer 8BITMIME)"
                .to_owned(),
        ));
    }

    let mut command = format!("MAIL FROM:<{from}>");
    if capabilities.size.is_some() {
        command.push_str(&format!(" SIZE={size}"));
    }
    if message.eight_bit {
        command.push_str(" BODY=8BITMIME");
    }
    if international.is_some() || message.eight_bit_headers {
        command.push_str(" SMTPUTF8");
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(from: &str, to: &[&str]) -> Envelope {
        Envelope {
            from: from.to_owned(),
            to: to.iter().map(|address| (*address).to_owned()).collect(),
        }
    }

    fn all() -> Capabilities {
        Capabilities {
            starttls: false,
            auth: Vec::new(),
            size: Some(1000),
            eight_bit_mime: true,
            smtputf8: true,
        }
    }

    const PLAIN: &[u8] = b"Subject: Hi\r\n\r\nHello.\r\n";

    #[test]
    fn ordinary_addresses_pass_and_duplicates_are_sent_once() {
        let envelope = envelope(
            "ana.ruiz@example.com",
            &[
                "robin@example.com",
                "kit+lists@example.com",
                "robin@example.com",
                "\"odd@local\"@example.com",
                "zo\u{eb}@example.com",
            ],
        );
        assert_eq!(
            checked(&envelope).unwrap(),
            [
                "robin@example.com",
                "kit+lists@example.com",
                "\"odd@local\"@example.com",
                "zo\u{eb}@example.com"
            ]
        );
    }

    #[test]
    fn an_address_that_could_carry_a_command_is_refused() {
        for bad in [
            "robin@example.com>\r\nRCPT TO:<kit@example.com",
            "robin@example.com\n",
            "robin@example.com> SIZE=1",
            "robin @example.com",
            "<robin@example.com>",
            "robin@example.com\0",
            "robin\u{2028}@example.com",
        ] {
            match checked(&envelope("ana.ruiz@example.com", &["sam@example.com", bad])) {
                Err(Error::Recipients(refused)) => {
                    assert_eq!(refused.len(), 1);
                    assert_eq!(refused[0].address, bad);
                }
                other => panic!("{bad:?}: {other:?}"),
            }
            assert!(
                matches!(
                    checked(&envelope(bad, &["sam@example.com"])),
                    Err(Error::Refused(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_address_needs_a_name_and_a_domain() {
        for bad in ["", "robin", "@example.com", "robin@"] {
            assert!(check_address(bad).is_err(), "{bad:?}");
        }
        assert!(matches!(
            checked(&envelope("ana.ruiz@example.com", &[])),
            Err(Error::Refused(text)) if text.contains("no recipients")
        ));
    }

    #[test]
    fn a_hello_name_is_one_printable_word() {
        for good in ["[127.0.0.1]", "localhost", "mac.example.com", "[IPv6:::1]"] {
            assert!(is_hello_name(good), "{good}");
        }
        for bad in [
            "",
            "two words",
            "name\r\nMAIL FROM:<x@example.com>",
            "caf\u{e9}",
        ] {
            assert!(!is_hello_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn mail_from_names_the_size_when_the_server_has_a_limit() {
        let message = message::prepare(PLAIN);
        assert_eq!(
            mail_command(
                &all(),
                &message,
                "ana.ruiz@example.com",
                &["robin@example.com"]
            )
            .unwrap(),
            format!("MAIL FROM:<ana.ruiz@example.com> SIZE={}", PLAIN.len())
        );
        assert_eq!(
            mail_command(
                &Capabilities::default(),
                &message,
                "ana.ruiz@example.com",
                &["robin@example.com"]
            )
            .unwrap(),
            "MAIL FROM:<ana.ruiz@example.com>"
        );
    }

    #[test]
    fn a_message_over_the_limit_is_too_large_and_one_at_it_is_not() {
        let message = message::prepare(PLAIN);
        let size = PLAIN.len() as u64;
        let limited = |limit| Capabilities {
            size: Some(limit),
            ..all()
        };
        assert!(
            mail_command(
                &limited(size),
                &message,
                "a@example.com",
                &["b@example.com"]
            )
            .is_ok()
        );
        match mail_command(
            &limited(size - 1),
            &message,
            "a@example.com",
            &["b@example.com"],
        ) {
            Err(Error::TooLarge { size: was, limit }) => {
                assert_eq!((was, limit), (size, size - 1));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn eight_bit_content_uses_8bitmime_or_is_refused() {
        let message = message::prepare("Subject: Hi\r\n\r\nCaf\u{e9}\r\n".as_bytes());
        let command = mail_command(&all(), &message, "a@example.com", &["b@example.com"]).unwrap();
        assert!(command.ends_with(" BODY=8BITMIME"), "{command}");
        let seven_bit = Capabilities {
            eight_bit_mime: false,
            ..all()
        };
        assert!(matches!(
            mail_command(&seven_bit, &message, "a@example.com", &["b@example.com"]),
            Err(Error::Refused(text)) if text.contains("8BITMIME")
        ));
    }

    #[test]
    fn international_addresses_and_headers_use_smtputf8_or_are_refused() {
        let plain = message::prepare(PLAIN);
        let headers = message::prepare("Subject: Caf\u{e9}\r\n\r\nHello.\r\n".as_bytes());
        let no_utf8 = Capabilities {
            smtputf8: false,
            ..all()
        };

        let command = mail_command(&all(), &plain, "a@example.com", &["zo\u{eb}@example.com"]);
        assert!(command.unwrap().ends_with(" SMTPUTF8"));
        let command = mail_command(&all(), &plain, "jos\u{e9}@example.com", &["b@example.com"]);
        assert!(command.unwrap().ends_with(" SMTPUTF8"));
        let command = mail_command(&all(), &headers, "a@example.com", &["b@example.com"]);
        assert!(command.unwrap().ends_with(" BODY=8BITMIME SMTPUTF8"));

        assert!(matches!(
            mail_command(&no_utf8, &plain, "a@example.com", &["b@example.com", "zo\u{eb}@example.com"]),
            Err(Error::Refused(text)) if text.contains("zo\u{eb}@example.com") && text.contains("SMTPUTF8")
        ));
        assert!(matches!(
            mail_command(&no_utf8, &headers, "a@example.com", &["b@example.com"]),
            Err(Error::Refused(text)) if text.contains("headers")
        ));
        // Plain mail to plain addresses asks for neither.
        let command = mail_command(&no_utf8, &plain, "a@example.com", &["b@example.com"]).unwrap();
        assert!(!command.contains("SMTPUTF8") && !command.contains("BODY="));
    }
}
