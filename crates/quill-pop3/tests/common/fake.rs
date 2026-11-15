//! A POP3 server in the test process, for what Dovecot cannot be made to
//! do: lack UIDL or TOP, stall, talk nonsense, or hold the mailbox locked.
//!
//! It is a real, if small, POP3 server over a list of messages in memory,
//! and deletes only on `QUIT` as the protocol says. Any command, or the
//! greeting, can be scripted to say something else. It records every line
//! it is sent.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use quill_core::config::TlsMode;
use quill_pop3::{Pop3Backend, Pop3Config, Trust};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::PASSWORD;

/// What the server does in place of its usual answer.
#[derive(Debug, Clone)]
pub enum Say {
    /// Send exactly this, then carry on.
    Text(String),
    /// Send exactly these bytes, then carry on.
    Bytes(Vec<u8>),
    /// Say nothing and keep the connection open.
    Silence,
    /// Close the connection without a word.
    HangUp,
}

struct State {
    /// Unique id and raw message, in mailbox order.
    messages: Vec<(String, String)>,
    /// The lines of the CAPA reply, or `None` for a server without CAPA.
    capa: Option<Vec<String>>,
    /// Keyed by a whole command line or by its first word, in upper case,
    /// or by `GREETING`.
    scripted: HashMap<String, Say>,
    heard: Vec<String>,
    sessions: usize,
}

pub struct Fake {
    pub port: u16,
    state: Arc<Mutex<State>>,
}

impl Fake {
    /// Starts a server holding `messages`: pairs of unique id and raw
    /// message. It offers USER, UIDL and TOP, and no pipelining.
    pub async fn start(messages: &[(&str, &str)]) -> Self {
        let state = Arc::new(Mutex::new(State {
            messages: messages
                .iter()
                .map(|(id, raw)| ((*id).to_owned(), (*raw).to_owned()))
                .collect(),
            capa: Some(vec!["USER".to_owned(), "UIDL".to_owned(), "TOP".to_owned()]),
            scripted: HashMap::new(),
            heard: Vec::new(),
            sessions: 0,
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let _ = serve(client, state).await;
                });
            }
        });
        Self { port, state }
    }

    /// Sets what CAPA lists, or `None` for a server that has no CAPA.
    pub fn capa(&self, lines: Option<&[&str]>) {
        self.state.lock().unwrap().capa =
            lines.map(|lines| lines.iter().map(|line| (*line).to_owned()).collect());
    }

    /// Scripts the answer to a command: `when` is a whole command line, a
    /// command's first word, or `GREETING`.
    pub fn script(&self, when: &str, say: Say) {
        self.state
            .lock()
            .unwrap()
            .scripted
            .insert(when.to_ascii_uppercase(), say);
    }

    pub fn unscript(&self, when: &str) {
        self.state
            .lock()
            .unwrap()
            .scripted
            .remove(&when.to_ascii_uppercase());
    }

    pub fn add(&self, id: &str, raw: &str) {
        self.state
            .lock()
            .unwrap()
            .messages
            .push((id.to_owned(), raw.to_owned()));
    }

    /// Every line any client has sent, in order.
    pub fn heard(&self) -> Vec<String> {
        self.state.lock().unwrap().heard.clone()
    }

    /// The first words of every line any client has sent.
    pub fn verbs(&self) -> Vec<String> {
        self.heard()
            .iter()
            .map(|line| line.split(' ').next().unwrap_or_default().to_owned())
            .collect()
    }

    /// The unique ids in the mailbox now.
    pub fn uidls(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state.messages.iter().map(|(id, _)| id.clone()).collect()
    }

    /// How many connections have been made.
    pub fn sessions(&self) -> usize {
        self.state.lock().unwrap().sessions
    }

    pub fn config(&self, tls: TlsMode) -> Pop3Config {
        Pop3Config {
            host: "127.0.0.1".to_owned(),
            port: self.port,
            tls,
            username: "robin@example.com".to_owned(),
            trust: Trust::default(),
        }
    }

    /// A backend on a plaintext connection that gives up on a silent
    /// server after a fraction of a second.
    pub fn backend(&self) -> Pop3Backend {
        super::backend(self.config(TlsMode::None), PASSWORD)
            .with_patience(std::time::Duration::from_millis(300))
    }
}

/// A multi-line reply: byte-stuffed, and closed with a lone dot.
fn multiline(status: &str, body: &str) -> String {
    let mut reply = format!("{status}\r\n");
    for line in body.split_inclusive('\n') {
        if line.starts_with('.') {
            reply.push('.');
        }
        reply.push_str(line);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        reply.push_str("\r\n");
    }
    reply.push_str(".\r\n");
    reply
}

/// The headers, the blank line, and the first `lines` lines of the body.
fn top(raw: &str, lines: usize) -> String {
    let Some((headers, body)) = raw.split_once("\r\n\r\n") else {
        return raw.to_owned();
    };
    let body: String = body.split_inclusive('\n').take(lines).collect();
    format!("{headers}\r\n\r\n{body}")
}

/// A message by its number, unless it is marked deleted.
fn find<'a>(
    mailbox: &'a [(String, String)],
    deleted: &HashSet<usize>,
    number: Option<&str>,
) -> Option<(usize, &'a (String, String))> {
    let index = number?.parse::<usize>().ok()?.checked_sub(1)?;
    if deleted.contains(&index) {
        return None;
    }
    mailbox.get(index).map(|message| (index, message))
}

async fn serve(stream: TcpStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    state.lock().unwrap().sessions += 1;
    let scripted = |key: &str| state.lock().unwrap().scripted.get(key).cloned();

    // What this session sees: the mailbox as it was at sign-in, and which
    // of its messages are marked for deletion.
    let mut signed_in = false;
    let mut mailbox: Vec<(String, String)> = Vec::new();
    let mut deleted: HashSet<usize> = HashSet::new();

    let mut pending = Some(
        scripted("GREETING").unwrap_or_else(|| Say::Text("+OK Fake POP3 ready\r\n".to_owned())),
    );
    loop {
        match pending.take() {
            Some(Say::Text(text)) => write.write_all(text.as_bytes()).await?,
            Some(Say::Bytes(bytes)) => write.write_all(&bytes).await?,
            Some(Say::Silence) => {
                // Until the client gives up and closes.
                let mut rest = Vec::new();
                let _ = read.read_to_end(&mut rest).await;
                return Ok(());
            }
            Some(Say::HangUp) => return Ok(()),
            None => {}
        }

        let mut raw = Vec::new();
        if read.read_until(b'\n', &mut raw).await? == 0 {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&raw).trim_end().to_owned();
        state.lock().unwrap().heard.push(line.clone());
        let mut words = line.split(' ');
        let verb = words.next().unwrap_or_default().to_ascii_uppercase();
        let argument = words.next();
        if let Some(say) = scripted(&line.to_ascii_uppercase()).or_else(|| scripted(&verb)) {
            pending = Some(say);
            continue;
        }

        let needs_sign_in = matches!(
            verb.as_str(),
            "STAT" | "LIST" | "UIDL" | "TOP" | "RETR" | "DELE" | "RSET" | "NOOP"
        );
        let reply = if needs_sign_in && !signed_in {
            "-ERR Sign in first\r\n".to_owned()
        } else {
            match verb.as_str() {
                "CAPA" => match state.lock().unwrap().capa.clone() {
                    Some(lines) => multiline("+OK Capability list follows", &lines.join("\r\n")),
                    None => "-ERR Unknown command\r\n".to_owned(),
                },
                "USER" => "+OK\r\n".to_owned(),
                "PASS" | "AUTH" => {
                    let password = if verb == "AUTH" {
                        // SASL PLAIN in two steps: "+ ", then the token.
                        write.write_all(b"+ \r\n").await?;
                        let mut token = String::new();
                        read.read_line(&mut token).await?;
                        let token = token.trim_end().to_owned();
                        state.lock().unwrap().heard.push(token.clone());
                        let decoded = STANDARD.decode(token).unwrap_or_default();
                        String::from_utf8_lossy(&decoded)
                            .rsplit('\0')
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    } else {
                        line.split_once(' ').map_or("", |(_, rest)| rest).to_owned()
                    };
                    if password == PASSWORD {
                        signed_in = true;
                        mailbox = state.lock().unwrap().messages.clone();
                        "+OK Signed in\r\n".to_owned()
                    } else {
                        "-ERR [AUTH] Authentication failed.\r\n".to_owned()
                    }
                }
                "STAT" => {
                    let kept = (0..mailbox.len()).filter(|index| !deleted.contains(index));
                    let (count, bytes) = kept.fold((0, 0), |(count, bytes), index| {
                        (count + 1, bytes + mailbox[index].1.len())
                    });
                    format!("+OK {count} {bytes}\r\n")
                }
                "LIST" | "UIDL" => {
                    let entry = |index: usize, (id, raw): &(String, String)| {
                        if verb == "LIST" {
                            format!("{} {}", index + 1, raw.len())
                        } else {
                            format!("{} {id}", index + 1)
                        }
                    };
                    match argument {
                        Some(number) => match find(&mailbox, &deleted, Some(number)) {
                            Some((index, found)) => format!("+OK {}\r\n", entry(index, found)),
                            None => "-ERR No such message\r\n".to_owned(),
                        },
                        None => {
                            let lines: Vec<String> = mailbox
                                .iter()
                                .enumerate()
                                .filter(|(index, _)| !deleted.contains(index))
                                .map(|(index, found)| entry(index, found))
                                .collect();
                            multiline("+OK", &lines.join("\r\n"))
                        }
                    }
                }
                "TOP" => match find(&mailbox, &deleted, argument) {
                    Some((_, (_, raw))) => {
                        let lines = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                        multiline("+OK", &top(raw, lines))
                    }
                    None => "-ERR No such message\r\n".to_owned(),
                },
                "RETR" => match find(&mailbox, &deleted, argument) {
                    Some((_, (_, raw))) => multiline("+OK", raw),
                    None => "-ERR No such message\r\n".to_owned(),
                },
                "DELE" => match find(&mailbox, &deleted, argument) {
                    Some((index, _)) => {
                        deleted.insert(index);
                        "+OK Marked\r\n".to_owned()
                    }
                    None => "-ERR No such message\r\n".to_owned(),
                },
                "RSET" => {
                    deleted.clear();
                    "+OK\r\n".to_owned()
                }
                "NOOP" => "+OK\r\n".to_owned(),
                "QUIT" => {
                    // The only place anything is removed.
                    {
                        let mut state = state.lock().unwrap();
                        let mut index = 0;
                        state.messages.retain(|_| {
                            index += 1;
                            !deleted.contains(&(index - 1))
                        });
                    }
                    write.write_all(b"+OK Bye\r\n").await?;
                    return Ok(());
                }
                _ => "-ERR Unknown command\r\n".to_owned(),
            }
        };
        write.write_all(reply.as_bytes()).await?;
    }
}
