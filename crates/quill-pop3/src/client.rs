//! One POP3 session: the connection, the commands and their replies.
//!
//! Every wait on the server is bounded. A server that says nothing is
//! reported as unreachable, and a server that says too much is cut off:
//! lines and replies have size limits, so nothing a server sends can make
//! the client hold more than it meant to.

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use quill_core::config::TlsMode;
use quill_core::{Error, Result};
use quill_net::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

use crate::config::Pop3Config;
use crate::wire::{self, Capa, Refusal, Reply};

/// The longest status or listing line accepted. RFC 2449 allows 512 bytes.
const MAX_LINE: usize = 8 * 1024;

/// The most a listing (`CAPA`, `UIDL`, `LIST`) may come to. A mailbox of a
/// million messages lists in well under this.
const MAX_LISTING: usize = 64 * 1024 * 1024;

/// The largest message accepted.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// How much of a reply is kept when only a message's headers are wanted.
const MAX_HEADERS: usize = 1024 * 1024;

/// How many commands go out before their replies are read, on a server
/// that allows it. The commands are a few bytes each, so a group of them
/// never waits on the server to make room.
const PIPELINE: usize = 32;

/// What one round trip to the server costs, counted in lines of listing
/// that could have been downloaded in the same time. Rough, and it only
/// has to be: it decides between two ways of asking the same question.
const LINES_PER_ROUND_TRIP: usize = 4096;

/// How much of a multi-line reply to accept.
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// The longest single line.
    line: usize,
    /// How many bytes to keep. The rest is read and dropped.
    keep: usize,
    /// How many bytes to read at all before giving up on the server.
    total: usize,
}

const LISTING: Limits = Limits {
    line: MAX_LINE,
    keep: MAX_LISTING,
    total: MAX_LISTING,
};

const MESSAGE: Limits = Limits {
    line: MAX_MESSAGE,
    keep: MAX_MESSAGE,
    total: MAX_MESSAGE,
};

/// For the headers of a message. With `TOP` the reply is only the headers;
/// with `RETR` it is the whole message, of which the start is enough.
const HEADERS: Limits = Limits {
    line: MAX_MESSAGE,
    keep: MAX_HEADERS,
    total: MAX_MESSAGE,
};

/// A server's answer to a command: the text after `+OK`, or its refusal.
type Answer = std::result::Result<String, Refusal>;

struct Line {
    /// The start of the line: all of it, unless it was longer than what
    /// was asked to be kept.
    bytes: Vec<u8>,
    /// How long the line was on the wire.
    length: usize,
}

impl Line {
    fn whole(&self) -> bool {
        self.bytes.len() == self.length
    }
}

pub(crate) struct Session {
    io: BufReader<Stream>,
    host: String,
    patience: Duration,
    /// What the server said it can do, or `None` if it has no `CAPA`.
    capa: Option<Capa>,
    /// The two sides are no longer in step: an exchange was cut short, or
    /// the server sent something that is not POP3. Nothing more is sent on
    /// such a session, not even `QUIT`.
    broken: bool,
}

/// Waits for `io`, but only so long. Like `quill_net::within`, with a wait
/// that tests can shorten.
async fn within<T>(
    patience: Duration,
    host: &str,
    io: impl Future<Output = std::io::Result<T>>,
) -> Result<T> {
    match timeout(patience, io).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(Error::Connection(format!("{host}: {err}"))),
        Err(_) => Err(Error::Connection(format!(
            "timed out waiting for {host} to answer"
        ))),
    }
}

impl Session {
    /// Connects and reads the greeting, leaving the connection as secure as
    /// the account's TLS mode requires and ready for sign-in.
    pub async fn open(config: &Pop3Config, patience: Duration) -> Result<Self> {
        let tcp = quill_net::tcp(&config.host, config.port).await?;
        let over = |stream| Self {
            io: BufReader::new(stream),
            host: config.host.clone(),
            patience,
            capa: None,
            broken: false,
        };

        match config.tls {
            TlsMode::Implicit => {
                let stream = quill_net::handshake(&config.host, &config.trust, tcp).await?;
                let mut session = over(stream);
                session.greeting().await?;
                Ok(session)
            }
            TlsMode::Starttls => {
                let mut session = over(Stream::Plain(tcp));
                session.greeting().await?;
                // Strict: a server that will not upgrade is a failed
                // connection, never a plaintext one. CAPA is not asked
                // first, since its answer would arrive unprotected and
                // could not be believed either way.
                if session.command("STLS").await?.is_err() {
                    return Err(Error::Insecure(format!(
                        "{} does not offer STLS, and this account requires it",
                        config.host
                    )));
                }
                // Taking the socket back drops the read buffer, and with it
                // anything sent in the clear after the server's +OK.
                // Nothing learned before the upgrade is kept.
                let Stream::Plain(tcp) = session.io.into_inner() else {
                    return Err(Error::Backend("STLS on a TLS stream".to_owned()));
                };
                let stream = quill_net::handshake(&config.host, &config.trust, tcp)
                    .await
                    .map_err(|err| match err {
                        // A refused certificate is its own error: the user
                        // is shown the certificate and decides.
                        Error::Connection(reason) => Error::Insecure(format!(
                            "{} accepted STLS but the upgrade to TLS failed: {reason}",
                            config.host
                        )),
                        other => other,
                    })?;
                Ok(over(stream))
            }
            TlsMode::None => {
                let mut session = over(Stream::Plain(tcp));
                session.greeting().await?;
                Ok(session)
            }
        }
    }

    async fn greeting(&mut self) -> Result<()> {
        match self.reply().await? {
            Reply::Ok(_) => Ok(()),
            Reply::Refused(refusal) => {
                self.broken = true;
                // A server that greets with -ERR is turning the connection
                // away: too many of them, or it is shutting down.
                Err(refusal.passing().unwrap_or_else(|| {
                    Error::Connection(format!(
                        "{} is not taking connections: {}",
                        self.host,
                        refusal.words()
                    ))
                }))
            }
            Reply::Continue => Err(self.out_of_step("+")),
        }
    }

    /// Signs in. Call only once the connection is as secure as the account
    /// requires: this is where the password goes out.
    pub async fn sign_in(&mut self, username: &str, password: &str) -> Result<()> {
        if [username, password]
            .iter()
            .any(|word| word.contains(['\r', '\n']))
        {
            return Err(Error::Auth(
                "the user name or password has a line break in it, which POP3 cannot send"
                    .to_owned(),
            ));
        }
        // USER and PASS unless the server says it only takes SASL.
        let sasl_only = self
            .capabilities()
            .await?
            .is_some_and(|capa| capa.sasl_plain && !capa.user);
        let refused = if sasl_only {
            self.auth_plain(username, password).await?
        } else {
            self.user_pass(username, password).await?
        };
        if let Some(refusal) = refused {
            return Err(refusal.into_sign_in_error());
        }
        // Asked again, because what was said before sign-in may have come
        // from a proxy in front of the real server, and a server may offer
        // more to someone it knows.
        self.capa = self.capabilities().await?;
        Ok(())
    }

    async fn user_pass(&mut self, username: &str, password: &str) -> Result<Option<Refusal>> {
        if let Err(refusal) = self.command(&format!("USER {username}")).await? {
            return Ok(Some(refusal));
        }
        Ok(self.command(&format!("PASS {password}")).await?.err())
    }

    /// SASL PLAIN (RFC 5034), in two steps so that a long password never
    /// makes a command line longer than a server has to accept.
    async fn auth_plain(&mut self, username: &str, password: &str) -> Result<Option<Refusal>> {
        self.send(&["AUTH PLAIN"]).await?;
        match self.reply().await? {
            Reply::Continue => {}
            Reply::Refused(refusal) => return Ok(Some(refusal)),
            Reply::Ok(_) => return Err(self.out_of_step("+OK before the credentials")),
        }
        let token = STANDARD.encode(format!("\0{username}\0{password}"));
        Ok(self.command(&token).await?.err())
    }

    async fn capabilities(&mut self) -> Result<Option<Capa>> {
        match self.command("CAPA").await? {
            Ok(_) => Ok(Some(Capa::parse(&self.body(LISTING).await?))),
            // A server from before RFC 2449. It may still do everything.
            Err(_) => Ok(None),
        }
    }

    fn pipelining(&self) -> bool {
        self.capa.is_some_and(|capa| capa.pipelining)
    }

    /// How many messages the mailbox holds, and their total size.
    pub async fn stat(&mut self) -> Result<(u64, u64)> {
        let text = self
            .command("STAT")
            .await?
            .map_err(|refusal| refusal.into_error("STAT"))?;
        wire::stat(&text).ok_or_else(|| self.nonsense("STAT", text.as_bytes()))
    }

    /// Every message in the mailbox: its number in this session and its
    /// unique id.
    pub async fn uidl(&mut self) -> Result<Vec<(u32, String)>> {
        if let Err(refusal) = self.command("UIDL").await? {
            return Err(refusal.passing().unwrap_or_else(|| {
                Error::Unsupported(format!(
                    "{} does not support UIDL, and without it Quill cannot tell which \
                     messages it already has (the server said: {})",
                    self.host,
                    refusal.words()
                ))
            }));
        }
        let listing = self.body(LISTING).await?;
        listing
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|line| !line.trim_ascii().is_empty())
            .map(|line| wire::uidl_line(line).ok_or_else(|| self.nonsense("UIDL", line)))
            .collect()
    }

    /// The size of each of `numbers`, as the server reports it. `total` is
    /// how many messages the mailbox holds.
    ///
    /// A message the server gives no size for has no entry: a size is for
    /// show, and not worth failing a sync over.
    pub async fn sizes(&mut self, numbers: &[u32], total: usize) -> Result<HashMap<u32, u64>> {
        // One LIST for the whole mailbox, or a LIST for each message:
        // whichever is quicker. The whole listing is a line for every
        // message in the mailbox. Asking one by one is a round trip each,
        // unless the questions can all go out together.
        let one_by_one = if self.pipelining() {
            numbers.len() * 4 <= total
        } else {
            numbers.len() * LINES_PER_ROUND_TRIP <= total
        };
        if one_by_one {
            let commands: Vec<String> = numbers
                .iter()
                .map(|number| format!("LIST {number}"))
                .collect();
            let answers = self.each(&commands, None).await?;
            return Ok(answers
                .iter()
                .filter_map(|answer| wire::list_line(answer.as_ref().ok()?.0.as_bytes()))
                .collect());
        }
        if self.command("LIST").await?.is_err() {
            return Ok(HashMap::new());
        }
        let listing = self.body(LISTING).await?;
        Ok(listing
            .split_inclusive(|byte| *byte == b'\n')
            .filter_map(wire::list_line)
            .collect())
    }

    /// The header block of each of `numbers`, in the same order. `None`
    /// where the server would not give it.
    ///
    /// `TOP` when the server has it, which sends the headers and nothing
    /// else. Otherwise `RETR`, which sends the whole message: slower, but
    /// every server has it. `RETR` is also tried for any one message that
    /// `TOP` was refused for, so a server that claims `TOP` and does not
    /// have it still syncs.
    pub async fn headers(&mut self, numbers: &[u32]) -> Result<Vec<Option<Vec<u8>>>> {
        let with_top = match self.capa {
            Some(capa) if capa.top => numbers.len(),
            Some(_) => 0,
            // No CAPA to go by: try TOP on one message and see.
            None => numbers.len().min(1),
        };
        let mut headers = self.top(&numbers[..with_top]).await?;
        if self.capa.is_none() && headers.first().is_some_and(Option::is_some) {
            headers.extend(self.top(&numbers[with_top..]).await?);
        }
        headers.resize(numbers.len(), None);

        let wanting: Vec<usize> = (0..numbers.len())
            .filter(|index| headers[*index].is_none())
            .collect();
        let commands: Vec<String> = wanting
            .iter()
            .map(|index| format!("RETR {}", numbers[*index]))
            .collect();
        let answers = self.each(&commands, Some(HEADERS)).await?;
        for (index, answer) in wanting.into_iter().zip(answers) {
            headers[index] = answer.ok().map(|(_, mut message)| {
                message.truncate(wire::header_block(&message).len());
                message
            });
        }
        Ok(headers)
    }

    async fn top(&mut self, numbers: &[u32]) -> Result<Vec<Option<Vec<u8>>>> {
        let commands: Vec<String> = numbers
            .iter()
            .map(|number| format!("TOP {number} 0"))
            .collect();
        let answers = self.each(&commands, Some(HEADERS)).await?;
        Ok(answers
            .into_iter()
            .map(|answer| answer.ok().map(|(_, headers)| headers))
            .collect())
    }

    /// The whole of one message, exactly as the server holds it.
    pub async fn retr(&mut self, number: u32) -> Result<Vec<u8>> {
        self.command(&format!("RETR {number}"))
            .await?
            .map_err(|refusal| refusal.into_error("RETR"))?;
        self.body(MESSAGE).await
    }

    /// Marks each of `numbers` for deletion. Nothing is removed until
    /// [`Session::commit`]. Returns the refusals, if the server would not
    /// mark some of them; the others stay marked.
    pub async fn dele(&mut self, numbers: &[u32]) -> Result<Vec<Refusal>> {
        let commands: Vec<String> = numbers
            .iter()
            .map(|number| format!("DELE {number}"))
            .collect();
        let answers = self.each(&commands, None).await?;
        Ok(answers
            .into_iter()
            .filter_map(|answer| answer.err())
            .collect())
    }

    /// Ends the session and has the server carry out the deletions. Only a
    /// `+OK` to `QUIT` means they happened: a session that ends any other
    /// way removes nothing (RFC 1939, section 6).
    pub async fn commit(mut self) -> Result<()> {
        self.command("QUIT")
            .await?
            .map(drop)
            .map_err(|refusal| refusal.into_error("QUIT"))
    }

    /// Ends a session that changed nothing. The work is already done, so
    /// how the server takes its leave does not matter.
    pub async fn goodbye(mut self) {
        if !self.broken {
            let _ = self.command("QUIT").await;
        }
    }

    /// Sends one command and reads the status line that answers it.
    async fn command(&mut self, line: &str) -> Result<Answer> {
        self.send(&[line]).await?;
        match self.reply().await? {
            Reply::Ok(text) => Ok(Ok(text)),
            Reply::Refused(refusal) => Ok(Err(refusal)),
            Reply::Continue => Err(self.out_of_step("+")),
        }
    }

    /// Sends each command and reads its answer, and with `body` the
    /// multi-line reply that follows each `+OK`. On a server with
    /// PIPELINING the commands go out in groups, which saves a round trip
    /// for every one of them.
    async fn each(
        &mut self,
        commands: &[String],
        body: Option<Limits>,
    ) -> Result<Vec<std::result::Result<(String, Vec<u8>), Refusal>>> {
        let group = if self.pipelining() { PIPELINE } else { 1 };
        let mut answers = Vec::with_capacity(commands.len());
        for commands in commands.chunks(group) {
            self.send(commands).await?;
            for _ in commands {
                answers.push(match self.reply().await? {
                    Reply::Ok(text) => {
                        let body = match body {
                            Some(limits) => self.body(limits).await?,
                            None => Vec::new(),
                        };
                        Ok((text, body))
                    }
                    Reply::Refused(refusal) => Err(refusal),
                    Reply::Continue => return Err(self.out_of_step("+")),
                });
            }
        }
        Ok(answers)
    }

    async fn send(&mut self, lines: &[impl AsRef<str>]) -> Result<()> {
        let mut out = Vec::new();
        for line in lines {
            let line = line.as_ref();
            // A line break inside a command would be read as a second
            // command of someone else's choosing.
            if line.contains(['\r', '\n']) {
                return Err(Error::Backend(
                    "a command with a line break in it was not sent".to_owned(),
                ));
            }
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        self.broken = true;
        let io = &mut self.io;
        within(self.patience, &self.host, async move {
            io.write_all(&out).await?;
            io.flush().await
        })
        .await?;
        self.broken = false;
        Ok(())
    }

    /// Reads one status line.
    async fn reply(&mut self) -> Result<Reply> {
        let line = self.line(MAX_LINE, MAX_LINE).await?;
        match Reply::parse(&line.bytes) {
            Some(reply) => Ok(reply),
            None => Err(self.nonsense("a command", &line.bytes)),
        }
    }

    /// Reads the rest of a multi-line reply, up to the line with a lone
    /// dot, and undoes the byte-stuffing.
    async fn body(&mut self, limits: Limits) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut read = 0usize;
        let mut full = false;
        loop {
            let room = limits.keep.saturating_sub(data.len());
            let line = self
                .line(
                    limits.line.min(limits.total.saturating_sub(read)),
                    // Always enough to recognise the closing dot.
                    room.max(4),
                )
                .await?;
            read += line.length;
            let Some(content) = wire::data_line(&line.bytes) else {
                return Ok(data);
            };
            // Once a line has not fitted, nothing after it is kept either:
            // what is returned is a prefix of the reply, never a patchwork.
            full = full || !line.whole() || content.len() > room;
            if !full {
                data.extend_from_slice(content);
            }
        }
    }

    /// Reads one line, line ending included. Fails if it is longer than
    /// `max`; holds on to no more than its first `keep` bytes.
    async fn line(&mut self, max: usize, keep: usize) -> Result<Line> {
        self.broken = true;
        let mut bytes = Vec::new();
        let mut length = 0usize;
        loop {
            let chunk = within(self.patience, &self.host, self.io.fill_buf()).await?;
            if chunk.is_empty() {
                return Err(Error::Connection(format!(
                    "{} closed the connection",
                    self.host
                )));
            }
            let end = chunk.iter().position(|byte| *byte == b'\n');
            let take = end.map_or(chunk.len(), |at| at + 1);
            length += take;
            if length > max {
                return Err(Error::Backend(format!(
                    "{} sent more than this client accepts in one reply",
                    self.host
                )));
            }
            let room = keep.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..take.min(room)]);
            self.io.consume(take);
            if end.is_some() {
                self.broken = false;
                return Ok(Line { bytes, length });
            }
        }
    }

    /// The server answered with something that cannot be an answer.
    fn nonsense(&mut self, to: &str, line: &[u8]) -> Error {
        self.broken = true;
        let shown: String = String::from_utf8_lossy(line)
            .trim_end()
            .chars()
            .take(80)
            .collect();
        Error::Backend(format!(
            "{} did not answer {to} as a POP3 server does: {shown:?}",
            self.host
        ))
    }

    fn out_of_step(&mut self, got: &str) -> Error {
        self.nonsense("a command", got.as_bytes())
    }
}
