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
//!
//! One wait is different. A connection that is idling (`idle`) is waiting to
//! be told of new mail, and hours of silence there are the server working as
//! it should. That connection says so with [`Bounded::expect_silence`] for
//! exactly as long as it idles, and reads wait without a clock meanwhile.
//! It is the same stream with the limit lifted, and not a second kind of
//! stream, so that the limit is back in force the moment the idling ends and
//! covers the command that ends it: that command is how a dead connection is
//! found.

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
    /// Set while the server has nothing to say and is not expected to.
    silence_expected: bool,
}

impl<T> Bounded<T> {
    pub fn new(inner: T, patience: Duration) -> Self {
        Self {
            inner,
            patience,
            reading: None,
            writing: None,
            silence_expected: false,
        }
    }

    /// Says whether the server is expected to keep quiet from here on. While
    /// it is, a read waits for as long as it takes. Writes keep their limit
    /// either way: silence is never a reason for the server not to take what
    /// it is sent.
    ///
    /// A clock already running for a read is stopped, and when the limit
    /// comes back the next wait starts a new one, so the server gets its
    /// full patience counted from then.
    pub fn expect_silence(&mut self, expected: bool) {
        self.silence_expected = expected;
        self.reading = None;
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
        if this.silence_expected {
            return answer;
        }
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

    #[tokio::test]
    async fn expected_silence_is_waited_through_and_the_limit_comes_back_after() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut stream = Bounded::new(ours, SHORT);
        stream.expect_silence(true);
        // Several limits pass with nothing said, and the read is still there
        // to take the byte when it comes.
        let server = tokio::spawn(async move {
            tokio::time::sleep(SHORT * 4).await;
            theirs.write_u8(7).await.unwrap();
            theirs
        });
        assert_eq!(stream.read_u8().await.unwrap(), 7);
        let _theirs = server.await.unwrap();

        // A write nobody takes is still given up on.
        let (ours, _theirs) = tokio::io::duplex(8);
        let mut stream = Bounded::new(ours, SHORT);
        stream.expect_silence(true);
        let err = stream.write_all(&[0; 64]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);

        // With the limit back, silence is counted from that moment.
        stream.expect_silence(false);
        let started = Instant::now();
        let err = stream.read_u8().await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= SHORT);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
