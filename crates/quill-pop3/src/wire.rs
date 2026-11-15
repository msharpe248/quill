//! The parts of POP3 that are only text: what a reply line means, how a
//! multi-line reply is unwrapped, and what the listings say.
//!
//! Nothing here touches a socket, so all of it is tested directly.

use quill_core::Error;

/// The first line of a server's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    /// `+OK`, with whatever text followed it.
    Ok(String),
    /// `-ERR`.
    Refused(Refusal),
    /// `+ `: the server wants more, as in the middle of `AUTH`.
    Continue,
}

/// A server's `-ERR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The response code in square brackets (RFC 2449 and RFC 3206),
    /// upper-cased: `IN-USE`, `AUTH`, `SYS/TEMP` and so on.
    pub code: Option<String>,
    /// The server's words, without the code.
    pub text: String,
}

impl Reply {
    /// Reads a status line. `None` for a line that is not one, which means
    /// the other end is not speaking POP3 or the two sides are out of step.
    pub fn parse(line: &[u8]) -> Option<Self> {
        let line = String::from_utf8_lossy(without_line_end(line));
        if let Some(text) = after_keyword(&line, "+OK") {
            return Some(Self::Ok(text.to_owned()));
        }
        if let Some(text) = after_keyword(&line, "-ERR") {
            return Some(Self::Refused(Refusal::parse(text)));
        }
        (line == "+" || line.starts_with("+ ")).then_some(Self::Continue)
    }
}

/// The rest of `line` if it starts with `word` as a whole word. Servers are
/// required to send the indicators in upper case; some do not.
fn after_keyword<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let head = line.get(..word.len())?;
    let rest = line.get(word.len()..)?;
    (head.eq_ignore_ascii_case(word) && (rest.is_empty() || rest.starts_with([' ', '\t'])))
        .then(|| rest.trim())
}

impl Refusal {
    fn parse(text: &str) -> Self {
        let coded = text
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .filter(|(code, _)| !code.is_empty() && !code.contains(' '));
        match coded {
            Some((code, rest)) => Self {
                code: Some(code.to_ascii_uppercase()),
                text: rest.trim().to_owned(),
            },
            None => Self {
                code: None,
                text: text.to_owned(),
            },
        }
    }

    /// What the server said, or that it said nothing.
    pub fn words(&self) -> &str {
        if self.text.is_empty() {
            "the server gave no reason"
        } else {
            &self.text
        }
    }

    /// The error for a refusal that will pass by itself: the mailbox is
    /// held by another session, or the server is having a bad moment.
    /// These are `Error::Connection`, which callers retry.
    pub fn passing(&self) -> Option<Error> {
        let text = self.text.to_ascii_lowercase();
        // Servers older than the response codes say it in words. RFC 1939's
        // own examples are "maildrop already locked" and "unable to lock
        // maildrop".
        let locked_in_words = self.code.is_none()
            && ["already locked", "unable to lock", "in use"]
                .iter()
                .any(|phrase| text.contains(phrase));
        match self.code.as_deref() {
            Some("IN-USE") => Some(self.in_use()),
            _ if locked_in_words => Some(self.in_use()),
            Some("LOGIN-DELAY") => Some(Error::Connection(format!(
                "the server wants a longer wait between sign-ins; try again later ({})",
                self.words()
            ))),
            Some("SYS/TEMP") => Some(Error::Connection(format!(
                "the server has a temporary problem: {}",
                self.words()
            ))),
            _ => None,
        }
    }

    fn in_use(&self) -> Error {
        Error::Connection(format!(
            "the mailbox is in use by another mail program; try again shortly ({})",
            self.words()
        ))
    }

    /// The error for a refused command after sign-in. `doing` says what was
    /// being attempted.
    pub fn into_error(self, doing: &str) -> Error {
        self.passing()
            .unwrap_or_else(|| Error::Backend(format!("{doing}: {}", self.words())))
    }

    /// The error for a refused sign-in. Anything that is not plainly a
    /// passing condition or a server fault is the credentials.
    pub fn into_sign_in_error(self) -> Error {
        if let Some(passing) = self.passing() {
            return passing;
        }
        match self.code.as_deref() {
            Some("SYS/PERM") => Error::Backend(format!("the server failed: {}", self.words())),
            _ => Error::Auth(self.words().to_owned()),
        }
    }
}

/// What the server said it can do, from `CAPA` (RFC 2449).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Capa {
    /// `TOP`: headers can be had without the body.
    pub top: bool,
    /// `PIPELINING`: several commands may be sent before reading replies.
    pub pipelining: bool,
    /// `USER`: sign-in with `USER` and `PASS`.
    pub user: bool,
    /// `SASL` lists `PLAIN`.
    pub sasl_plain: bool,
}

impl Capa {
    /// Reads the body of a `CAPA` reply, one capability to a line.
    pub fn parse(listing: &[u8]) -> Self {
        let mut capa = Self::default();
        for line in String::from_utf8_lossy(listing).lines() {
            let mut words = line.split_ascii_whitespace();
            let Some(tag) = words.next() else { continue };
            match tag.to_ascii_uppercase().as_str() {
                "TOP" => capa.top = true,
                "PIPELINING" => capa.pipelining = true,
                "USER" => capa.user = true,
                "SASL" => capa.sasl_plain = words.any(|word| word.eq_ignore_ascii_case("PLAIN")),
                _ => {}
            }
        }
        capa
    }
}

fn without_line_end(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// One line of a multi-line reply, as it was before the server
/// byte-stuffed it: a line that starts with a dot is sent with one more in
/// front. `None` is the line with a lone dot that ends the reply.
///
/// The line keeps its line ending, so that joining the lines gives back the
/// message exactly as the server holds it.
pub(crate) fn data_line(line: &[u8]) -> Option<&[u8]> {
    match line.strip_prefix(b".") {
        Some(rest) if without_line_end(rest).is_empty() => None,
        Some(rest) => Some(rest),
        None => Some(line),
    }
}

/// A message number: where a message stands in this session's listing.
/// Counted from one, and good for this session only.
fn message_number(word: &str) -> Option<u32> {
    word.parse().ok().filter(|number| *number > 0)
}

/// A line of `UIDL`: the message's number in this session and its unique
/// id, which is the same in every session.
///
/// The id is everything after the number. RFC 1939 allows only printable
/// ASCII without spaces, but an id that breaks that rule still names its
/// message, so it is kept as sent.
pub(crate) fn uidl_line(line: &[u8]) -> Option<(u32, String)> {
    let line = String::from_utf8_lossy(without_line_end(line));
    let (number, id) = line.trim_start().split_once([' ', '\t'])?;
    let id = id.trim();
    (!id.is_empty()).then_some((message_number(number)?, id.to_owned()))
}

/// A line of `LIST`: the message's number and its size in bytes.
pub(crate) fn list_line(line: &[u8]) -> Option<(u32, u64)> {
    let line = String::from_utf8_lossy(without_line_end(line));
    let mut words = line.split_ascii_whitespace();
    Some((message_number(words.next()?)?, words.next()?.parse().ok()?))
}

/// The text of a `STAT` reply: how many messages, and their total size.
pub(crate) fn stat(text: &str) -> Option<(u64, u64)> {
    let mut words = text.split_ascii_whitespace();
    Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?))
}

/// The header block of a message: everything up to and including the first
/// empty line, or all of it when there is none.
pub(crate) fn header_block(raw: &[u8]) -> &[u8] {
    let mut start = 0;
    while let Some(length) = raw[start..].iter().position(|byte| *byte == b'\n') {
        let line = &raw[start..=start + length];
        start += length + 1;
        if without_line_end(line).is_empty() {
            return &raw[..start];
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(line: &str) -> Refusal {
        match Reply::parse(line.as_bytes()) {
            Some(Reply::Refused(refusal)) => refusal,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn status_lines_are_read_with_their_text() {
        assert_eq!(
            Reply::parse(b"+OK Dovecot ready.\r\n"),
            Some(Reply::Ok("Dovecot ready.".to_owned()))
        );
        assert_eq!(Reply::parse(b"+OK\r\n"), Some(Reply::Ok(String::new())));
        assert_eq!(Reply::parse(b"+OK"), Some(Reply::Ok(String::new())));
        assert_eq!(
            Reply::parse(b"+ok 2 320\n"),
            Some(Reply::Ok("2 320".to_owned()))
        );
        assert_eq!(
            refused("-ERR no such message\r\n"),
            Refusal {
                code: None,
                text: "no such message".to_owned()
            }
        );
        assert_eq!(refused("-ERR\r\n").text, "");
        assert_eq!(Reply::parse(b"+ \r\n"), Some(Reply::Continue));
        assert_eq!(Reply::parse(b"+\r\n"), Some(Reply::Continue));
        assert_eq!(Reply::parse(b"+ dGVzdA==\r\n"), Some(Reply::Continue));
    }

    #[test]
    fn what_is_not_a_status_line_is_not_read_as_one() {
        for garbage in [
            &b""[..],
            b"\r\n",
            b"OK\r\n",
            b"+OKAY then\r\n",
            b"-ERROR\r\n",
            b"* OK IMAP4rev1 ready\r\n",
            b"220 smtp.example.com ESMTP\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
            b"\xff\xfe\x00\x01",
            b"+\xc3\xa9K\r\n",
        ] {
            assert_eq!(Reply::parse(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn response_codes_are_split_from_the_text() {
        let refusal = refused("-ERR [AUTH] Authentication failed.\r\n");
        assert_eq!(refusal.code.as_deref(), Some("AUTH"));
        assert_eq!(refusal.text, "Authentication failed.");
        assert_eq!(
            refused("-ERR [in-use] busy").code.as_deref(),
            Some("IN-USE")
        );
        assert_eq!(
            refused("-ERR [SYS/TEMP] try later").code.as_deref(),
            Some("SYS/TEMP")
        );
        // Brackets that are not a code are part of the text.
        let plain = refused("-ERR [not a code] hello");
        assert_eq!(plain.code, None);
        assert_eq!(plain.text, "[not a code] hello");
        assert_eq!(refused("-ERR [] hello").code, None);
        assert_eq!(refused("-ERR [unclosed").code, None);
    }

    #[test]
    fn a_refused_sign_in_is_the_credentials_unless_the_server_says_otherwise() {
        let auth = |line: &str| refused(line).into_sign_in_error();
        assert!(
            matches!(auth("-ERR [AUTH] Authentication failed."), Error::Auth(text) if text == "Authentication failed.")
        );
        assert!(matches!(auth("-ERR invalid password"), Error::Auth(_)));
        assert!(matches!(auth("-ERR"), Error::Auth(_)));
        // The account is fine; the mailbox is busy or the server is.
        for passing in [
            "-ERR [IN-USE] Do you have another POP session running?",
            "-ERR maildrop already locked",
            "-ERR unable to lock maildrop",
            "-ERR [LOGIN-DELAY] minimum time between mail checks violation",
            "-ERR [SYS/TEMP] Temporary authentication failure.",
        ] {
            assert!(
                matches!(auth(passing), Error::Connection(_)),
                "{passing}: {:?}",
                auth(passing)
            );
        }
        assert!(matches!(
            auth("-ERR [SYS/PERM] no mailbox for this user"),
            Error::Backend(_)
        ));
        // "Locked" about the account is not about the mailbox being busy.
        assert!(matches!(
            auth("-ERR account locked after too many attempts"),
            Error::Auth(_)
        ));
        // A coded refusal is not second-guessed from its words.
        assert!(matches!(
            auth("-ERR [AUTH] password already in use elsewhere"),
            Error::Auth(_)
        ));
    }

    #[test]
    fn a_refused_command_keeps_the_servers_words() {
        match refused("-ERR There's no message 9.").into_error("RETR") {
            Error::Backend(text) => assert_eq!(text, "RETR: There's no message 9."),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            refused("-ERR [IN-USE] locked").into_error("UIDL"),
            Error::Connection(text) if text.contains("in use")
        ));
        assert!(matches!(
            refused("-ERR").into_error("DELE"),
            Error::Backend(text) if text.contains("no reason")
        ));
    }

    #[test]
    fn capabilities_are_read_whatever_their_case_and_arguments() {
        let capa = Capa::parse(
            b"CAPA\r\nTOP\r\nUIDL\r\nRESP-CODES\r\nPIPELINING\r\nAUTH-RESP-CODE\r\nUSER\r\nSASL PLAIN LOGIN\r\n",
        );
        assert_eq!(
            capa,
            Capa {
                top: true,
                pipelining: true,
                user: true,
                sasl_plain: true
            }
        );
        let capa =
            Capa::parse(b"top\r\nsasl CRAM-MD5 plain\r\nEXPIRE 60\r\n\r\nIMPLEMENTATION x\r\n");
        assert!(capa.top && capa.sasl_plain && !capa.user && !capa.pipelining);
        assert_eq!(
            Capa::parse(b"SASL CRAM-MD5\r\nTOPPING\r\n"),
            Capa::default()
        );
        assert_eq!(Capa::parse(b""), Capa::default());
    }

    #[test]
    fn dots_are_unstuffed_and_the_lone_dot_ends_the_reply() {
        assert_eq!(data_line(b"hello\r\n"), Some(&b"hello\r\n"[..]));
        assert_eq!(data_line(b"\r\n"), Some(&b"\r\n"[..]));
        // The terminator, with either line ending or cut short.
        assert_eq!(data_line(b".\r\n"), None);
        assert_eq!(data_line(b".\n"), None);
        assert_eq!(data_line(b"."), None);
        // A stuffed line loses exactly one dot.
        assert_eq!(data_line(b"..\r\n"), Some(&b".\r\n"[..]));
        assert_eq!(data_line(b"...\r\n"), Some(&b"..\r\n"[..]));
        assert_eq!(data_line(b"..hidden\r\n"), Some(&b".hidden\r\n"[..]));
        // A server that did not stuff a line it should have.
        assert_eq!(data_line(b".signature\r\n"), Some(&b"signature\r\n"[..]));
        // Dots elsewhere are left alone.
        assert_eq!(data_line(b"a.b.\r\n"), Some(&b"a.b.\r\n"[..]));
    }

    #[test]
    fn a_stuffed_message_comes_back_byte_for_byte() {
        let message = "Subject: dots\r\n\r\n.\r\n..\r\n.leading\r\nmiddle.dot\r\n...\r\n";
        // What a server sends: every line that starts with a dot gets
        // another, then the lone dot.
        let mut wire = String::new();
        for line in message.split_inclusive('\n') {
            if line.starts_with('.') {
                wire.push('.');
            }
            wire.push_str(line);
        }
        wire.push_str(".\r\n");

        let mut read = Vec::new();
        let mut ended = false;
        for line in wire.as_bytes().split_inclusive(|byte| *byte == b'\n') {
            assert!(!ended, "data after the end");
            match data_line(line) {
                Some(data) => read.extend_from_slice(data),
                None => ended = true,
            }
        }
        assert!(ended);
        assert_eq!(read, message.as_bytes());
    }

    #[test]
    fn uidl_lines_give_the_number_and_the_id() {
        assert_eq!(
            uidl_line(b"1 0000000169185d3a\r\n"),
            Some((1, "0000000169185d3a".to_owned()))
        );
        assert_eq!(
            uidl_line(b"48213 GmailId15a3b0c9d2e4f601\n"),
            Some((48213, "GmailId15a3b0c9d2e4f601".to_owned()))
        );
        // Kept as sent, even where RFC 1939 would not allow it.
        assert_eq!(
            uidl_line(b"2  odd id with spaces \r\n"),
            Some((2, "odd id with spaces".to_owned()))
        );
        for bad in [
            &b""[..],
            b"\r\n",
            b"1\r\n",
            b"1 \r\n",
            b"0 abc\r\n",
            b"-1 abc\r\n",
            b"one abc\r\n",
            b"99999999999 abc\r\n",
        ] {
            assert_eq!(uidl_line(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn list_lines_and_stat_give_numbers() {
        assert_eq!(list_line(b"1 120\r\n"), Some((1, 120)));
        assert_eq!(list_line(b"7 5000000000\r\n"), Some((7, 5_000_000_000)));
        // The single-message form: "+OK 3 340" and whatever else follows.
        assert_eq!(list_line(b"3 340 octets"), Some((3, 340)));
        for bad in [&b""[..], b"1\r\n", b"0 10\r\n", b"a b\r\n", b"1 -5\r\n"] {
            assert_eq!(list_line(bad), None, "{bad:?}");
        }
        assert_eq!(stat("2 320"), Some((2, 320)));
        assert_eq!(stat("0 0"), Some((0, 0)));
        assert_eq!(stat("2 320 octets in maildrop"), Some((2, 320)));
        assert_eq!(stat("2 messages"), None);
        assert_eq!(stat(""), None);
    }

    #[test]
    fn the_header_block_stops_at_the_first_empty_line() {
        assert_eq!(
            header_block(b"A: 1\r\nB: 2\r\n\r\nbody\r\n\r\nmore\r\n"),
            b"A: 1\r\nB: 2\r\n\r\n"
        );
        assert_eq!(header_block(b"A: 1\nB: 2\n\nbody\n"), b"A: 1\nB: 2\n\n");
        assert_eq!(header_block(b"A: 1\r\nB: 2\r\n"), b"A: 1\r\nB: 2\r\n");
        assert_eq!(header_block(b"A: 1"), b"A: 1");
        assert_eq!(header_block(b"\r\nbody"), b"\r\n");
        assert_eq!(header_block(b""), b"");
    }
}
