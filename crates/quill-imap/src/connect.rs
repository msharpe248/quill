//! Opening a connection in each of the three transport modes.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_imap::Client;
use quill_core::config::TlsMode;
use quill_core::{Error, Result};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::config::ImapConfig;
use crate::tls;

/// How long to wait for the server before calling it unreachable.
const PATIENCE: Duration = Duration::from_secs(20);

/// A connection that is either plaintext or TLS. One type for both lets the
/// rest of the backend ignore which it has.
#[derive(Debug)]
pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// Connects and reads the greeting, leaving the connection as secure as the
/// account's TLS mode requires and ready for sign-in.
pub(crate) async fn connect(config: &ImapConfig) -> Result<Client<Stream>> {
    let address = (config.host.as_str(), config.port);
    let tcp = within(TcpStream::connect(address), "the connection")
        .await?
        .map_err(|err| unreachable_server(config, &err))?;
    let _ = tcp.set_nodelay(true);

    match config.tls {
        TlsMode::Implicit => {
            let stream = handshake(config, tcp).await?;
            let mut client = Client::new(stream);
            greeting(&mut client, config).await?;
            Ok(client)
        }
        TlsMode::Starttls => {
            let mut client = Client::new(Stream::Plain(tcp));
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
            let Stream::Plain(tcp) = client.into_inner() else {
                return Err(Error::Backend("STARTTLS on a TLS stream".to_owned()));
            };
            Ok(Client::new(handshake(config, tcp).await?))
        }
        TlsMode::None => {
            let mut client = Client::new(Stream::Plain(tcp));
            greeting(&mut client, config).await?;
            Ok(client)
        }
    }
}

async fn handshake(config: &ImapConfig, tcp: TcpStream) -> Result<Stream> {
    let (tls_config, presented) = tls::client_config(&config.trust)?;
    let name = ServerName::try_from(config.host.clone())
        .map_err(|_| Error::Backend(format!("\"{}\" is not a host name", config.host)))?;
    let stream = within(
        TlsConnector::from(tls_config).connect(name, tcp),
        "the TLS handshake",
    )
    .await?
    .map_err(|err| {
        // rustls reports a certificate it would not accept as
        // InvalidCertificate, or as General for a pin that did not match.
        let rustls_error = err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>());
        match rustls_error {
            Some(rustls::Error::InvalidCertificate(reason)) => {
                tls::problem(&config.host, format!("{reason:?}"), &presented)
            }
            Some(rustls::Error::General(reason)) if config.trust.pinned_sha256.is_some() => {
                tls::problem(&config.host, reason.clone(), &presented)
            }
            _ => Error::Connection(format!("TLS with {} failed: {err}", config.host)),
        }
    })?;
    Ok(Stream::Tls(Box::new(stream)))
}

async fn greeting(client: &mut Client<Stream>, config: &ImapConfig) -> Result<()> {
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

async fn within<T>(future: impl Future<Output = T>, doing: &str) -> Result<T> {
    timeout(PATIENCE, future)
        .await
        .map_err(|_| Error::Connection(format!("timed out waiting for {doing}")))
}

fn unreachable_server(config: &ImapConfig, err: &io::Error) -> Error {
    Error::Connection(format!("{}:{}: {err}", config.host, config.port))
}
