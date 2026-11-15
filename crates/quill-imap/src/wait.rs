//! A limit on how long the server may keep us waiting.
//!
//! The backend has one connection and holds a lock on it for as long as a
//! command runs. A server that stops answering, without closing the
//! connection, would hold that lock for good and every later call would
//! queue up behind it.
//!
//! The limit is on silence, not on the whole command: the clock starts when
//! a read or a write has to wait and stops as soon as any bytes move. A
//! large message on a slow line takes as long as it takes, as long as it
//! keeps coming. Putting the limit under async-imap, on the stream itself,
//! means no command can be sent around it.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// A stream that gives up when the other end is silent for too long.
#[derive(Debug)]
pub(crate) struct Bounded<T> {
    inner: T,
    patience: Duration,
    /// Running while a read is waiting for the server.
    reading: Option<Pin<Box<Sleep>>>,
    /// Running while a write is waiting for the server to take more.
    writing: Option<Pin<Box<Sleep>>>,
}

impl<T> Bounded<T> {
    pub fn new(inner: T, patience: Duration) -> Self {
        Self {
            inner,
            patience,
            reading: None,
            writing: None,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

/// The same words as `quill_net::within`, so a silent server reads the same
/// wherever it was noticed.
fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for the server")
}

/// Passes on what the stream said, with the clock running only while it
/// says "not yet".
fn bounded<R>(
    answer: Poll<io::Result<R>>,
    clock: &mut Option<Pin<Box<Sleep>>>,
    patience: Duration,
    cx: &mut Context<'_>,
) -> Poll<io::Result<R>> {
    if answer.is_ready() {
        *clock = None;
        return answer;
    }
    let sleep = clock.get_or_insert_with(|| Box::pin(tokio::time::sleep(patience)));
    if sleep.as_mut().poll(cx).is_pending() {
        return Poll::Pending;
    }
    *clock = None;
    Poll::Ready(Err(timed_out()))
}

impl<T: AsyncRead + Unpin> AsyncRead for Bounded<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let answer = Pin::new(&mut this.inner).poll_read(cx, buf);
        bounded(answer, &mut this.reading, this.patience, cx)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Bounded<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let answer = Pin::new(&mut this.inner).poll_write(cx, buf);
        bounded(answer, &mut this.writing, this.patience, cx)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let answer = Pin::new(&mut this.inner).poll_flush(cx);
        bounded(answer, &mut this.writing, this.patience, cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let answer = Pin::new(&mut this.inner).poll_shutdown(cx);
        bounded(answer, &mut this.writing, this.patience, cx)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const SHORT: Duration = Duration::from_millis(60);

    #[tokio::test]
    async fn a_read_nobody_answers_gives_up() {
        let (ours, _theirs) = tokio::io::duplex(64);
        let mut stream = Bounded::new(ours, SHORT);
        let started = Instant::now();
        let err = stream.read_u8().await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "timed out waiting for the server");
        assert!(started.elapsed() >= SHORT);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_write_nobody_takes_gives_up() {
        // A pipe that holds eight bytes, with nobody reading the other end.
        let (ours, _theirs) = tokio::io::duplex(8);
        let mut stream = Bounded::new(ours, SHORT);
        let err = stream.write_all(&[0; 64]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn slow_is_not_silent() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut stream = Bounded::new(ours, SHORT);
        // Ten bytes, each well inside the limit, taking several limits in
        // all.
        let server = tokio::spawn(async move {
            for byte in 0..10u8 {
                tokio::time::sleep(SHORT / 3).await;
                theirs.write_u8(byte).await.unwrap();
            }
            theirs
        });
        let mut got = [0; 10];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(got, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        // The clock starts afresh for the next wait: silence now is counted
        // from now.
        let _theirs = server.await.unwrap();
        let started = Instant::now();
        assert!(stream.read_u8().await.is_err());
        assert!(started.elapsed() >= SHORT);
    }
}
