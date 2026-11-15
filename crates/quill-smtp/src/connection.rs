//! One connection to a server: commands out, replies in, and a limit on
//! every wait.

use std::time::Duration;

use quill_core::{Error, Result};
use quill_net::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::reply::{Assembler, Reply};

/// The longest reply line accepted. The protocol's own limit is 512 bytes;
/// servers exceed it in sign-in challenges, but not by this much.
const MAX_LINE: usize = 16 * 1024;

/// How much of a message is written between checks that the server is still
/// taking it.
const CHUNK: usize = 32 * 1024;

/// The longest the goodbye is waited for. The work is done by then.
const GOODBYE: Duration = Duration::from_secs(3);

/// Waits for `future`, but only so long: a server that says nothing is
/// reported as unreachable. `doing` names what was being waited for.
///
/// This is `quill_net::within` with the wait chosen by the caller: sending
/// needs a longer one after the message has gone, and tests a shorter one.
pub(crate) async fn wait<T>(
    patience: Duration,
    future: impl Future<Output = T>,
    doing: &str,
) -> Result<T> {
    timeout(patience, future)
        .await
        .map_err(|_| Error::Connection(format!("timed out waiting for {doing}")))
}

fn dropped(err: std::io::Error) -> Error {
    Error::Connection(format!("the connection dropped: {err}"))
}

pub(crate) struct Connection {
    stream: Stream,
    /// Bytes read from the server and not yet used.
    buffer: Vec<u8>,
    patience: Duration,
}

impl Connection {
    pub fn new(stream: Stream, patience: Duration) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            patience,
        }
    }

    /// Sends one command line and reads the reply to it.
    pub async fn command(&mut self, line: &str, doing: &str) -> Result<Reply> {
        self.send_line(line, doing).await?;
        self.reply(doing).await
    }

    /// Sends one line. The line ending is added here, and a line that
    /// carries one of its own is never sent: it would be two commands.
    pub async fn send_line(&mut self, line: &str, doing: &str) -> Result<()> {
        if line.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
            return Err(Error::Backend(format!(
                "a line break inside a command ({doing})"
            )));
        }
        let mut bytes = Vec::with_capacity(line.len() + 2);
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
        self.write(&bytes, doing).await
    }

    /// Writes `bytes` as they are. The wait is on each piece, not the
    /// whole: a large message on a slow line is fine so long as it keeps
    /// moving.
    pub async fn write(&mut self, bytes: &[u8], doing: &str) -> Result<()> {
        for chunk in bytes.chunks(CHUNK) {
            wait(self.patience, self.stream.write_all(chunk), doing)
                .await?
                .map_err(dropped)?;
        }
        wait(self.patience, self.stream.flush(), doing)
            .await?
            .map_err(dropped)
    }

    /// Reads one whole reply.
    pub async fn reply(&mut self, doing: &str) -> Result<Reply> {
        self.reply_within(self.patience, doing).await
    }

    /// Reads one whole reply, waiting up to `patience` for all of it.
    pub async fn reply_within(&mut self, patience: Duration, doing: &str) -> Result<Reply> {
        wait(patience, self.read_reply(), doing).await?
    }

    async fn read_reply(&mut self) -> Result<Reply> {
        let mut assembler = Assembler::default();
        loop {
            let line = self.read_line().await?;
            if let Some(reply) = assembler.push(&line)? {
                return Ok(reply);
            }
        }
    }

    /// The next line from the server, without its line ending.
    async fn read_line(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            if self.buffer.len() > MAX_LINE {
                return Err(Error::Backend(
                    "the server sent a line too long to be an SMTP reply".to_owned(),
                ));
            }
            let mut chunk = [0u8; 4096];
            let read = self.stream.read(&mut chunk).await.map_err(dropped)?;
            if read == 0 {
                return Err(Error::Connection(
                    "the server closed the connection".to_owned(),
                ));
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// Gives the socket back for the TLS handshake after `STARTTLS`.
    ///
    /// The read buffer is dropped here, and with it anything the server, or
    /// someone in between, sent in the clear after its go-ahead. Nothing
    /// read before the upgrade is kept.
    pub fn into_tcp(self) -> Result<TcpStream> {
        match self.stream {
            Stream::Plain(tcp) => Ok(tcp),
            Stream::Tls(_) => Err(Error::Backend("STARTTLS on a TLS stream".to_owned())),
        }
    }

    /// Says goodbye and closes. Whether the server answers changes nothing,
    /// so this neither waits long nor fails.
    pub async fn quit(mut self) {
        self.patience = self.patience.min(GOODBYE);
        let _ = self.command("QUIT", "the goodbye").await;
        let _ = wait(self.patience, self.stream.shutdown(), "the close").await;
    }
}
