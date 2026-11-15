//! Opening a connection: TCP, then TLS in the way the account asks for.

use std::time::Duration;

use async_imap::Client;
use quill_core::config::TlsMode;
use quill_core::{Error, Result};
use quill_net::{Stream, within};

use crate::config::ImapConfig;
use crate::wait::Bounded;

/// Connects and reads the greeting, leaving the connection as secure as the
/// account's TLS mode requires and ready for sign-in.
///
/// From here on the server is given `patience` to say something whenever it
/// is waited for, and no longer.
pub(crate) async fn connect(
    config: &ImapConfig,
    patience: Duration,
) -> Result<Client<Bounded<Stream>>> {
    let tcp = quill_net::tcp(&config.host, config.port).await?;
    let bounded = |stream: Stream| Bounded::new(stream, patience);

    match config.tls {
        TlsMode::Implicit => {
            let stream = quill_net::handshake(&config.host, &config.trust, tcp).await?;
            let mut client = Client::new(bounded(stream));
            greeting(&mut client, config).await?;
            Ok(client)
        }
        TlsMode::Starttls => {
            let mut client = Client::new(bounded(Stream::Plain(tcp)));
            greeting(&mut client, config).await?;
            // Strict: a server that will not upgrade is a failed connection,
            // never a plaintext one. Whether the server advertised STARTTLS
            // is not consulted, since the advertisement arrives unprotected.
            within(
                client.run_command_and_check_ok("STARTTLS", None),
                "STARTTLS",
            )
            .await?
            .map_err(|err| match err {
                async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_) => {
                    Error::Insecure(format!(
                        "{} does not offer STARTTLS, and this account requires it",
                        config.host
                    ))
                }
                other => Error::Connection(other.to_string()),
            })?;
            // Taking the socket back drops the old connection's read buffer,
            // and with it anything sent in the clear after the server's OK.
            // Nothing learned before the upgrade is kept.
            let Stream::Plain(tcp) = client.into_inner().into_inner() else {
                return Err(Error::Backend("STARTTLS on a TLS stream".to_owned()));
            };
            Ok(Client::new(bounded(
                quill_net::handshake(&config.host, &config.trust, tcp).await?,
            )))
        }
        TlsMode::None => {
            let mut client = Client::new(bounded(Stream::Plain(tcp)));
            greeting(&mut client, config).await?;
            Ok(client)
        }
    }
}

async fn greeting(client: &mut Client<Bounded<Stream>>, config: &ImapConfig) -> Result<()> {
    match within(client.read_response(), "the server's greeting").await? {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(Error::Connection(format!(
            "{} closed the connection before greeting",
            config.host
        ))),
        Err(err) => Err(Error::Connection(format!(
            "no greeting from {}: {err}",
            config.host
        ))),
    }
}
