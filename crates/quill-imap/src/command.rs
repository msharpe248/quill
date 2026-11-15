//! Commands written by hand, for the replies async-imap does not pass on.
//!
//! async-imap runs COPY, MOVE and APPEND and reports only that they worked.
//! The reply says more: what the message is called in its new folder
//! (`COPYUID`, `APPENDUID`) and whether a refusal means the folder is
//! missing (`TRYCREATE`). The commands here go out over async-imap's own
//! connection, one at a time like its own, and keep those answers.
//!
//! Writing the command here also settles how a mailbox name is sent: quoted
//! when it can be, and as a literal when it holds bytes a quoted string may
//! not carry.

use std::fmt::Debug;

use async_imap::Session;
use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{RequestId, Response, ResponseCode, Status};
use quill_core::{Error, Result};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// What a connection has to be for async-imap to talk IMAP over it.
pub(crate) trait Wire: AsyncRead + AsyncWrite + Unpin + Debug + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send> Wire for T {}

/// One command, built an argument at a time.
pub(crate) struct Command<'a> {
    /// The command line so far, since the last literal.
    line: String,
    /// Each earlier stretch of the line and the literal that follows it.
    /// The server is asked before every literal, so that a command it is
    /// going to refuse costs one line and not a whole message.
    before: Vec<(String, &'a [u8])>,
}

impl<'a> Command<'a> {
    pub fn new(verb: &str) -> Self {
        Self {
            line: verb.to_owned(),
            before: Vec::new(),
        }
    }

    /// An argument that needs no quoting: a UID, a flag list.
    pub fn arg(mut self, text: &str) -> Self {
        self.line.push(' ');
        self.line.push_str(text);
        self
    }

    /// A mailbox name, exactly as the server listed it.
    pub fn mailbox(self, name: &'a str) -> Result<Self> {
        match encode_mailbox(name)? {
            Mailbox::Quoted(quoted) => Ok(self.arg(&quoted)),
            Mailbox::Literal(bytes) => Ok(self.literal(bytes)),
        }
    }

    /// Bytes sent exactly as they are, whatever they contain.
    pub fn literal(mut self, bytes: &'a [u8]) -> Self {
        self.line.push_str(&format!(" {{{}}}", bytes.len()));
        self.before.push((std::mem::take(&mut self.line), bytes));
        self
    }

    /// Sends the command and reads to the end of its reply. A refusal by the
    /// server is a [`Reply`] too, for the caller to judge; `Err` means the
    /// conversation itself broke down.
    pub async fn run<T: Wire>(self, session: &mut Session<T>) -> Result<Reply> {
        self.run_with(session, |_| {}).await
    }

    /// As [`Command::run`], showing `each` the data the server sends on the
    /// way: the lines of a LIST, say.
    pub async fn run_with<T: Wire>(
        self,
        session: &mut Session<T>,
        mut each: impl FnMut(&Response<'_>) + Send,
    ) -> Result<Reply> {
        let mut reply = Reply::default();
        let mut stretches = self
            .before
            .into_iter()
            .map(|(line, literal)| (line, Some(literal)))
            .chain(std::iter::once((self.line, None)));

        let Some((first, mut literal)) = stretches.next() else {
            return Err(Error::Backend("an empty command".to_owned()));
        };
        // async-imap adds the tag and the line ending, and remembers the tag.
        let tag = session.run_command(&first).await.map_err(broken)?;
        while let Some(bytes) = literal {
            match read(session, &tag, &mut reply, &mut each).await? {
                Next::GoAhead => {}
                // The server has heard enough. For a refusal that is its
                // right, and the literal is never sent.
                Next::Done if reply.outcome != Outcome::Ok => return Ok(reply),
                Next::Done => {
                    return Err(Error::Backend(
                        "the server answered before the command was complete".to_owned(),
                    ));
                }
            }
            let Some((line, next)) = stretches.next() else {
                return Err(Error::Backend("a command ended in a literal".to_owned()));
            };
            let stream: &mut T = session.as_mut();
            write(stream, bytes).await?;
            write(stream, line.as_bytes()).await?;
            write(stream, b"\r\n").await?;
            stream.flush().await.map_err(lost)?;
            literal = next;
        }
        match read(session, &tag, &mut reply, &mut each).await? {
            Next::Done => Ok(reply),
            Next::GoAhead => Err(Error::Backend(
                "the server asked for more of a finished command".to_owned(),
            )),
        }
    }
}

async fn write<T: Wire>(stream: &mut T, bytes: &[u8]) -> Result<()> {
    stream.write_all(bytes).await.map_err(lost)
}

fn lost(err: std::io::Error) -> Error {
    Error::Connection(err.to_string())
}

fn broken(err: async_imap::error::Error) -> Error {
    use async_imap::error::Error as Imap;
    match err {
        Imap::Io(_) | Imap::ConnectionLost => Error::Connection(err.to_string()),
        other => Error::Backend(other.to_string()),
    }
}

enum Next {
    /// The server is ready for the literal.
    GoAhead,
    /// The command is over and `Reply` holds how it ended.
    Done,
}

/// Reads until the server either asks for the rest of the command or ends
/// it, noting the response codes that pass.
async fn read<T: Wire>(
    session: &mut Session<T>,
    tag: &RequestId,
    reply: &mut Reply,
    each: &mut (impl FnMut(&Response<'_>) + Send),
) -> Result<Next> {
    loop {
        let response = session
            .read_response()
            .await
            .map_err(lost)?
            .ok_or_else(|| Error::Connection("the server closed the connection".to_owned()))?;
        match response.parsed() {
            Response::Continue { .. } => return Ok(Next::GoAhead),
            Response::Done {
                tag: theirs,
                status,
                outcome,
            } if theirs == tag => {
                reply.outcome = match status {
                    Status::Ok => Outcome::Ok,
                    Status::No => Outcome::No,
                    _ => Outcome::Bad,
                };
                reply.note(outcome.code.as_ref());
                reply.text = outcome
                    .information
                    .as_deref()
                    .unwrap_or_default()
                    .to_owned();
                return Ok(Next::Done);
            }
            // MOVE reports COPYUID on a line of its own, ahead of the
            // messages leaving the folder.
            Response::Data {
                status: Status::Ok,
                outcome,
            } => reply.note(outcome.code.as_ref()),
            other => each(other),
        }
    }
}

/// How the server ended a command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    /// The command was understood and not carried out.
    No,
    /// The command was not understood, or anything else that is not OK.
    #[default]
    Bad,
}

/// The end of a command's reply, and the response codes seen on the way.
#[derive(Debug, Default)]
pub(crate) struct Reply {
    pub outcome: Outcome,
    /// The server's own words.
    pub text: String,
    /// `TRYCREATE`: the folder named does not exist.
    pub try_create: bool,
    /// `COPYUID`: where copied or moved messages went.
    pub copied: Option<Copied>,
    /// `APPENDUID`: the UIDVALIDITY and UID of a saved message.
    pub appended: Option<(u32, u32)>,
}

impl Reply {
    fn note(&mut self, code: Option<&ResponseCode<'_>>) {
        match code {
            Some(ResponseCode::TryCreate) => self.try_create = true,
            Some(ResponseCode::CopyUid(validity, from, to)) => {
                self.copied = Some(Copied {
                    validity: *validity,
                    from: from.clone(),
                    to: to.clone(),
                });
            }
            Some(ResponseCode::AppendUid(validity, uids)) => {
                // One message was saved, so anything but one UID is not an
                // answer to trust.
                let mut uids = members(uids);
                if let (Some(uid), None) = (uids.next(), uids.next()) {
                    self.appended = Some((*validity, uid));
                }
            }
            _ => {}
        }
    }

    pub fn is_ok(&self) -> bool {
        self.outcome == Outcome::Ok
    }

    /// The server's words for an error message, or `fallback` when it gave
    /// none.
    pub fn words(&self, fallback: &str) -> String {
        if self.text.is_empty() {
            fallback.to_owned()
        } else {
            self.text.clone()
        }
    }
}

/// A `COPYUID` response code: the destination's UIDVALIDITY, the UIDs that
/// were copied, and the UIDs the copies were given, in matching order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Copied {
    pub validity: u32,
    from: Vec<UidSetMember>,
    to: Vec<UidSetMember>,
}

impl Copied {
    /// The UID the copy of `uid` was given, if the server accounted for it.
    pub fn new_uid(&self, uid: u32) -> Option<u32> {
        let place = members(&self.from).position(|from| from == uid)?;
        members(&self.to).nth(place)
    }
}

/// The UIDs of a set, in the order the set gives them. A range covers the
/// same UIDs whichever end is written first.
fn members(set: &[UidSetMember]) -> impl Iterator<Item = u32> + '_ {
    set.iter().flat_map(|member| match member {
        UidSetMember::Uid(uid) => *uid..=*uid,
        UidSetMember::UidRange(range) => {
            *range.start().min(range.end())..=*range.start().max(range.end())
        }
    })
}

/// How a mailbox name goes on the wire.
#[derive(Debug, PartialEq, Eq)]
enum Mailbox<'a> {
    Quoted(String),
    Literal(&'a [u8]),
}

/// Folder ids are the server's own names, modified UTF-7 and all, so they
/// are sent back untouched. Most can go in quotes, with `\` and `"` escaped.
/// A quoted string cannot hold a line break or a byte outside ASCII, and a
/// name with either goes as a literal, which can hold anything and cannot be
/// mistaken for the start of another command.
fn encode_mailbox(name: &str) -> Result<Mailbox<'_>> {
    if name.is_empty() || name.contains('\0') {
        return Err(Error::NotFound(format!(
            "{name:?} cannot be the name of a folder"
        )));
    }
    if name
        .bytes()
        .all(|byte| byte.is_ascii() && byte != b'\r' && byte != b'\n')
    {
        let mut quoted = String::with_capacity(name.len() + 2);
        quoted.push('"');
        for c in name.chars() {
            if matches!(c, '\\' | '"') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        Ok(Mailbox::Quoted(quoted))
    } else {
        Ok(Mailbox::Literal(name.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use std::ops::RangeInclusive;

    use super::*;

    #[test]
    fn plain_names_are_quoted_and_their_specials_escaped() {
        for (name, wire) in [
            ("INBOX", r#""INBOX""#),
            ("Sent Items", r#""Sent Items""#),
            ("Projects/&ZeVnLIqe-", r#""Projects/&ZeVnLIqe-""#),
            (r#"The "big" one"#, r#""The \"big\" one""#),
            (r"Work\2026", r#""Work\\2026""#),
            ("100% {5}", r#""100% {5}""#),
        ] {
            assert_eq!(
                encode_mailbox(name).unwrap(),
                Mailbox::Quoted(wire.to_owned()),
                "{name}"
            );
        }
    }

    #[test]
    fn names_a_quoted_string_cannot_hold_go_as_literals() {
        for name in ["Entwürfe", "日本語", "Two\r\nA1 LOGOUT", "Line\nbreak"] {
            assert_eq!(
                encode_mailbox(name).unwrap(),
                Mailbox::Literal(name.as_bytes()),
                "{name:?}"
            );
        }
        let command = Command::new("UID COPY")
            .arg("7")
            .mailbox("Entwürfe")
            .unwrap();
        assert_eq!(
            command.before,
            [("UID COPY 7 {9}".to_owned(), "Entwürfe".as_bytes())]
        );
        assert_eq!(command.line, "");
    }

    #[test]
    fn names_no_folder_can_have_are_not_found() {
        for name in ["", "a\0b"] {
            assert!(
                matches!(encode_mailbox(name), Err(Error::NotFound(_))),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_command_is_cut_at_each_literal() {
        let command = Command::new("APPEND")
            .mailbox("Sent")
            .unwrap()
            .arg("(\\Seen)")
            .literal(b"Subject: hello\r\n\r\n");
        assert_eq!(
            command.before,
            [(
                "APPEND \"Sent\" (\\Seen) {18}".to_owned(),
                b"Subject: hello\r\n\r\n".as_slice()
            )]
        );
        assert_eq!(command.line, "");
    }

    #[test]
    fn a_copied_uid_is_found_by_its_place_in_the_set() {
        let copied = Copied {
            validity: 9,
            from: vec![UidSetMember::Uid(4), UidSetMember::UidRange(8..=10)],
            // Written larger end first, which RFC 4315 allows and which
            // means the same UIDs in the same ascending order.
            to: vec![UidSetMember::UidRange(RangeInclusive::new(23, 20))],
        };
        assert_eq!(copied.new_uid(4), Some(20));
        assert_eq!(copied.new_uid(8), Some(21));
        assert_eq!(copied.new_uid(10), Some(23));
        assert_eq!(copied.new_uid(5), None);

        // A server that names fewer copies than messages has not said where
        // the last ones went.
        let short = Copied {
            validity: 9,
            from: vec![UidSetMember::UidRange(1..=2)],
            to: vec![UidSetMember::Uid(7)],
        };
        assert_eq!(short.new_uid(1), Some(7));
        assert_eq!(short.new_uid(2), None);
    }
}
