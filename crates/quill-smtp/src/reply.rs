//! Server replies: a three-digit code and one or more lines of text.

use quill_core::{Error, Result};

/// The most lines accepted in one reply. Real replies have a handful; a
/// server that never stops must not fill memory.
const MAX_LINES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub code: u16,
    /// The text of each line, without the code.
    pub lines: Vec<String>,
}

impl Reply {
    /// 2xx: the server did what was asked.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.code)
    }

    /// The reply on one line, code first, as it is shown to the user. The
    /// code stays in because it is what people search for.
    pub fn text(&self) -> String {
        let mut text = self.code.to_string();
        for line in self.lines.iter().filter(|line| !line.is_empty()) {
            text.push(' ');
            text.push_str(line);
        }
        text
    }

    /// What a reply that is not the hoped-for one means. 4xx is "not now",
    /// 5xx is "no"; anything else here is a server not speaking SMTP as
    /// expected. `doing` names the step, for that last case.
    pub fn into_error(self, doing: &str) -> Error {
        match self.code {
            400..500 => Error::Temporary(self.text()),
            500..600 => Error::Refused(self.text()),
            _ => Error::Backend(format!("unexpected reply to {doing}: {}", self.text())),
        }
    }
}

/// Collects the lines of one reply. Every line but the last has a hyphen
/// after the code.
#[derive(Debug, Default)]
pub(crate) struct Assembler {
    code: Option<u16>,
    lines: Vec<String>,
}

impl Assembler {
    /// Takes one line, without its line ending. Returns the reply once its
    /// last line has arrived.
    pub fn push(&mut self, line: &[u8]) -> Result<Option<Reply>> {
        let (code, last, text) = parse_line(line)?;
        match self.code {
            None => self.code = Some(code),
            Some(first) if first != code => {
                return Err(Error::Backend(format!(
                    "the server's reply changed code from {first} to {code} part way"
                )));
            }
            Some(_) => {}
        }
        self.lines.push(text);
        if last {
            self.code = None;
            return Ok(Some(Reply {
                code,
                lines: std::mem::take(&mut self.lines),
            }));
        }
        if self.lines.len() >= MAX_LINES {
            return Err(Error::Backend(
                "the server's reply has too many lines".to_owned(),
            ));
        }
        Ok(None)
    }
}

/// One reply line as (code, whether it is the last, text).
fn parse_line(line: &[u8]) -> Result<(u16, bool, String)> {
    let malformed = || {
        let shown: String = String::from_utf8_lossy(line).chars().take(80).collect();
        Error::Backend(format!(
            "the server sent a line that is not an SMTP reply: {shown:?}"
        ))
    };
    let (digits, rest) = line.split_at_checked(3).ok_or_else(malformed)?;
    if !digits.iter().all(u8::is_ascii_digit) || !(b'2'..=b'5').contains(&digits[0]) {
        return Err(malformed());
    }
    let code = digits
        .iter()
        .fold(0u16, |code, digit| code * 10 + u16::from(digit - b'0'));
    let (last, text) = match rest.split_first() {
        // A bare code with nothing after it is a complete reply.
        None => (true, &[][..]),
        Some((b' ', text)) => (true, text),
        Some((b'-', text)) => (false, text),
        Some(_) => return Err(malformed()),
    };
    Ok((
        code,
        last,
        String::from_utf8_lossy(text).trim_end().to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(lines: &[&str]) -> Result<Option<Reply>> {
        let mut assembler = Assembler::default();
        let mut reply = None;
        for line in lines {
            assert!(reply.is_none(), "lines after the reply ended");
            reply = assembler.push(line.as_bytes())?;
        }
        Ok(reply)
    }

    #[test]
    fn a_single_line_is_a_reply() {
        let reply = assemble(&["250 2.0.0 Ok: queued as 4F2A"])
            .unwrap()
            .unwrap();
        assert_eq!(reply.code, 250);
        assert_eq!(reply.lines, ["2.0.0 Ok: queued as 4F2A"]);
        assert!(reply.is_success());
        assert_eq!(reply.text(), "250 2.0.0 Ok: queued as 4F2A");
    }

    #[test]
    fn a_reply_runs_until_the_line_without_a_hyphen() {
        let mut assembler = Assembler::default();
        assert_eq!(assembler.push(b"250-mail.example.com").unwrap(), None);
        assert_eq!(assembler.push(b"250-SIZE 1000").unwrap(), None);
        let reply = assembler.push(b"250 8BITMIME").unwrap().unwrap();
        assert_eq!(reply.code, 250);
        assert_eq!(reply.lines, ["mail.example.com", "SIZE 1000", "8BITMIME"]);
        // The assembler is ready for the next reply.
        let next = assembler.push(b"220 Go ahead").unwrap().unwrap();
        assert_eq!(next.lines, ["Go ahead"]);
    }

    #[test]
    fn a_bare_code_is_a_reply_with_no_text() {
        let reply = assemble(&["354"]).unwrap().unwrap();
        assert_eq!(
            (reply.code, reply.lines.as_slice()),
            (354, &[String::new()][..])
        );
        assert_eq!(reply.text(), "354");
        // And so is a code with an empty text after the space.
        assert_eq!(assemble(&["250 "]).unwrap().unwrap().text(), "250");
    }

    #[test]
    fn several_lines_read_as_one_sentence() {
        let reply = assemble(&[
            "550-5.7.1 Our system has detected an unusual rate of",
            "550 5.7.1 unsolicited mail.",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            reply.text(),
            "550 5.7.1 Our system has detected an unusual rate of 5.7.1 unsolicited mail."
        );
    }

    #[test]
    fn text_that_is_not_utf8_is_kept_readable() {
        let mut assembler = Assembler::default();
        let reply = assembler.push(b"550 bo\xeete pleine").unwrap().unwrap();
        assert_eq!(reply.lines, ["bo\u{fffd}te pleine"]);
    }

    #[test]
    fn lines_that_are_not_replies_are_refused() {
        for line in [
            "",
            "25",
            "hello",
            "25a ok",
            "250ok",
            "250+ok",
            "150 too low",
            "650 too high",
            "\u{16}\u{3}\u{1}",
        ] {
            assert!(
                matches!(assemble(&[line]), Err(Error::Backend(_))),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_code_that_changes_part_way_is_refused() {
        assert!(matches!(
            assemble(&["250-first", "550 second"]),
            Err(Error::Backend(_))
        ));
    }

    #[test]
    fn a_reply_without_end_is_cut_off() {
        let mut assembler = Assembler::default();
        let mut outcome = Ok(None);
        for _ in 0..=MAX_LINES {
            outcome = assembler.push(b"250-more");
            if outcome.is_err() {
                break;
            }
        }
        assert!(matches!(outcome, Err(Error::Backend(_))));
    }

    #[test]
    fn four_hundreds_are_temporary_and_five_hundreds_final() {
        let reply = |code| Reply {
            code,
            lines: vec!["words".to_owned()],
        };
        assert!(matches!(
            reply(451).into_error("x"),
            Error::Temporary(text) if text == "451 words"
        ));
        assert!(matches!(reply(421).into_error("x"), Error::Temporary(_)));
        assert!(matches!(
            reply(550).into_error("x"),
            Error::Refused(text) if text == "550 words"
        ));
        assert!(matches!(
            reply(354).into_error("EHLO"),
            Error::Backend(text) if text.contains("EHLO") && text.contains("354")
        ));
    }
}
