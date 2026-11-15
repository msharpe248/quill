//! What Quill's mail protocols share on the wire.
//!
//! IMAP, POP3 and SMTP all reach their servers the same three ways: TLS from
//! the first byte, a plaintext start upgraded with STARTTLS, or plaintext
//! when an account opts in. This crate holds the parts of that which do not
//! depend on the protocol: opening the socket, the TLS handshake, which
//! certificates are trusted, and what is reported when one is not.
//!
//! The STARTTLS exchange itself is the protocol's own. Its rule is the same
//! everywhere: the upgrade is strict, with no fallback to plaintext, and
//! nothing learned before it is kept.

pub mod tls;

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use quill_core::{Error, Result};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// How long to wait for the server before calling it unreachable.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// Which server certificates to accept, beyond those the system trusts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trust {
    /// Extra certificate authorities, DER encoded: a custom CA bundle.
    pub extra_roots: Vec<Vec<u8>>,
    /// The SHA-256 of the one certificate to accept, for a self-signed
    /// server. When set, the certificate must match it exactly and nothing
    /// else about it is checked.
    pub pinned_sha256: Option<[u8; 32]>,
}

/// A connection that is either plaintext or TLS. One type for both lets the
/// rest of the backend ignore which it has.
#[derive(Debug)]
pub enum Stream {
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

/// Opens a TCP connection to `host:port`, or says the server cannot be
/// reached.
pub async fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    let tcp = within(TcpStream::connect((host, port)), "the connection")
        .await?
        .map_err(|err| Error::Connection(format!("{host}:{port}: {err}")))?;
    let _ = tcp.set_nodelay(true);
    Ok(tcp)
}

/// Runs the TLS handshake with `host` over `tcp`, verifying its certificate
/// against the system's trust and `trust`. A certificate that is not
/// accepted comes back as `Error::Certificate`, carrying what the server
/// presented.
pub async fn handshake(host: &str, trust: &Trust, tcp: TcpStream) -> Result<Stream> {
    let (tls_config, presented) = tls::client_config(trust)?;
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| Error::Backend(format!("\"{host}\" is not a host name")))?;
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
                tls::problem(host, format!("{reason:?}"), &presented)
            }
            Some(rustls::Error::General(reason)) if trust.pinned_sha256.is_some() => {
                tls::problem(host, reason.clone(), &presented)
            }
            _ => Error::Connection(format!("TLS with {host} failed: {err}")),
        }
    })?;
    Ok(Stream::Tls(Box::new(stream)))
}

/// Waits for `future`, but only so long: a server that says nothing is
/// reported as unreachable. `doing` names what was being waited for.
pub async fn within<T>(future: impl Future<Output = T>, doing: &str) -> Result<T> {
    timeout(PATIENCE, future)
        .await
        .map_err(|_| Error::Connection(format!("timed out waiting for {doing}")))
}
