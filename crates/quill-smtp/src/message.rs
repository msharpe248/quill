//! Getting a message ready for the wire.
//!
//! Three things happen to it, and nothing else: `Bcc` headers are removed,
//! line endings become CRLF, and lines that start with a dot get a second
//! one for the `DATA` command.

/// A message as it will be transmitted, before dot-stuffing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// The message with `Bcc` gone and every line ending CRLF. Its length
    /// is the size a server's `SIZE` limit is about.
    pub bytes: Vec<u8>,
    /// Whether any byte has its high bit set. Such a message needs
    /// `8BITMIME`.
    pub eight_bit: bool,
    /// Whether any header byte has its high bit set: raw UTF-8 in a header,
    /// which needs `SMTPUTF8`.
    pub eight_bit_headers: bool,
}

/// Removes `Bcc` and `Resent-Bcc` headers, folded lines and all, and makes
/// every line ending CRLF.
///
/// A lone CR or LF also ends a line. Left alone, a server might read one as
/// a line ending where we did not, and a body line of a single dot would
/// then end the message early with the rest taken as commands.
pub(crate) fn prepare(raw: &[u8]) -> Prepared {
    let mut bytes = Vec::with_capacity(raw.len() + raw.len() / 32 + 2);
    let mut in_headers = true;
    let mut header_len = None;
    // Inside a Bcc header: its continuation lines go with it.
    let mut dropping = false;
    let lines = Lines { rest: raw };
    for line in lines {
        if in_headers {
            match line.first() {
                None => {
                    in_headers = false;
                    header_len = Some(bytes.len());
                }
                Some(b' ' | b'\t') => {
                    if dropping {
                        continue;
                    }
                }
                Some(_) => {
                    dropping = is_bcc(line);
                    if dropping {
                        continue;
                    }
                }
            }
        }
        bytes.extend_from_slice(line);
        bytes.extend_from_slice(b"\r\n");
    }
    let header_len = header_len.unwrap_or(bytes.len());
    Prepared {
        eight_bit: !bytes.is_ascii(),
        eight_bit_headers: !bytes[..header_len].is_ascii(),
        bytes,
    }
}

/// Whether a header line starts a `Bcc` or `Resent-Bcc` field.
fn is_bcc(line: &[u8]) -> bool {
    let Some(colon) = line.iter().position(|byte| *byte == b':') else {
        return false;
    };
    // The old syntax allows spaces between the name and the colon.
    let name = line[..colon].trim_ascii_end();
    name.eq_ignore_ascii_case(b"Bcc") || name.eq_ignore_ascii_case(b"Resent-Bcc")
}

/// The lines of a message, whichever of CRLF, LF or CR ends each.
struct Lines<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let Some(end) = self
            .rest
            .iter()
            .position(|byte| *byte == b'\r' || *byte == b'\n')
        else {
            // A last line with no line ending.
            return Some(std::mem::take(&mut self.rest));
        };
        let line = &self.rest[..end];
        let ending = if self.rest[end..].starts_with(b"\r\n") {
            2
        } else {
            1
        };
        self.rest = &self.rest[end + ending..];
        Some(line)
    }
}

/// The body of a `DATA` command for a prepared message: each line that
/// starts with a dot gets a second one, and a line of one dot ends it.
pub(crate) fn dot_stuff(prepared: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prepared.len() + prepared.len() / 64 + 3);
    let mut line_start = true;
    for &byte in prepared {
        if line_start && byte == b'.' {
            out.push(b'.');
        }
        out.push(byte);
        line_start = byte == b'\n';
    }
    // `prepare` ends every line, the last included, so the dot is on a line
    // of its own.
    out.extend_from_slice(b".\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(raw: &str) -> String {
        String::from_utf8(prepare(raw.as_bytes()).bytes).unwrap()
    }

    #[test]
    fn a_message_without_bcc_is_untouched() {
        let raw = "From: Ana Ruiz <ana.ruiz@example.com>\r\nTo: robin@example.com\r\n\
                   Subject: Notes\r\n\r\nFirst line.\r\n\r\nLast line.\r\n";
        let message = prepare(raw.as_bytes());
        assert_eq!(message.bytes, raw.as_bytes());
        assert!(!message.eight_bit && !message.eight_bit_headers);
    }

    #[test]
    fn bcc_is_removed_and_nothing_else() {
        let raw = "From: ana.ruiz@example.com\r\nBcc: Kit Moss <kit@example.com>\r\n\
                   To: robin@example.com\r\n\r\nBody.\r\n";
        assert_eq!(
            prepared(raw),
            "From: ana.ruiz@example.com\r\nTo: robin@example.com\r\n\r\nBody.\r\n"
        );
    }

    #[test]
    fn a_folded_bcc_goes_with_all_its_lines() {
        let raw = "From: ana.ruiz@example.com\r\n\
                   Bcc: Kit Moss <kit@example.com>,\r\n \
                   Sam Okoro <sam@example.com>,\r\n\t\
                   Lee Tran <lee@example.com>\r\n\
                   Subject: Folded\r\n and continued\r\n\r\nBody.\r\n";
        assert_eq!(
            prepared(raw),
            "From: ana.ruiz@example.com\r\nSubject: Folded\r\n and continued\r\n\r\nBody.\r\n"
        );
    }

    #[test]
    fn bcc_is_found_in_any_case_and_every_place() {
        let raw = "BCC: kit@example.com\r\nFrom: ana.ruiz@example.com\r\n\
                   bcc : sam@example.com\r\nResent-Bcc: lee@example.com\r\n\
                   Subject: Hi\r\nBcc:\r\n\r\nBody.\r\n";
        assert_eq!(
            prepared(raw),
            "From: ana.ruiz@example.com\r\nSubject: Hi\r\n\r\nBody.\r\n"
        );
    }

    #[test]
    fn headers_that_only_look_like_bcc_stay() {
        let raw = "X-Bcc: kit@example.com\r\nBcc-Note: none\r\n\
                   Subject: Bcc: in a subject\r\n\r\nBody.\r\n";
        assert_eq!(prepared(raw), raw);
    }

    #[test]
    fn a_bcc_line_in_the_body_stays() {
        let raw = "Subject: Hi\r\n\r\nBcc: this is body text\r\n continued\r\n";
        assert_eq!(prepared(raw), raw);
    }

    #[test]
    fn a_message_that_is_all_headers_still_loses_bcc() {
        assert_eq!(
            prepared("Subject: Hi\r\nBcc: kit@example.com"),
            "Subject: Hi\r\n"
        );
    }

    #[test]
    fn every_line_ending_becomes_crlf() {
        assert_eq!(
            prepared("Subject: Hi\nTo: robin@example.com\n\nOne\nTwo\rThree\r\nFour"),
            "Subject: Hi\r\nTo: robin@example.com\r\n\r\nOne\r\nTwo\r\nThree\r\nFour\r\n"
        );
        // Blank lines survive, however they were written.
        assert_eq!(
            prepared("A: b\n\n\n\nx\r\r\ny"),
            "A: b\r\n\r\n\r\n\r\nx\r\n\r\ny\r\n"
        );
        assert_eq!(prepared(""), "");
    }

    #[test]
    fn eight_bit_content_is_noticed_and_placed() {
        let body = prepare("Subject: Hi\r\n\r\nCaf\u{e9}\r\n".as_bytes());
        assert!(body.eight_bit && !body.eight_bit_headers);
        let header = prepare("Subject: Caf\u{e9}\r\n\r\nPlain\r\n".as_bytes());
        assert!(header.eight_bit && header.eight_bit_headers);
        // A Bcc that is removed does not count.
        let bcc =
            prepare("Bcc: Zo\u{eb} <zoe@example.com>\r\nSubject: Hi\r\n\r\nPlain\r\n".as_bytes());
        assert!(!bcc.eight_bit && !bcc.eight_bit_headers);
    }

    #[test]
    fn lines_that_start_with_a_dot_get_a_second_one() {
        let stuffed = dot_stuff(b"Subject: Hi\r\n\r\n.\r\n..\r\n.hidden\r\nmid.dle\r\nend.\r\n");
        assert_eq!(
            String::from_utf8(stuffed).unwrap(),
            "Subject: Hi\r\n\r\n..\r\n...\r\n..hidden\r\nmid.dle\r\nend.\r\n.\r\n"
        );
    }

    #[test]
    fn a_dot_on_the_very_first_line_is_stuffed_too() {
        assert_eq!(dot_stuff(b".first\r\n"), b"..first\r\n.\r\n");
        assert_eq!(dot_stuff(b""), b".\r\n");
    }

    #[test]
    fn a_lone_dot_cannot_end_the_message_early() {
        // Bare line endings around a dot: the classic way to smuggle a
        // second message past a server.
        let message = prepare(b"Subject: Hi\n\nbefore\n.\nMAIL FROM:<x@example.com>\r.\rafter");
        let stuffed = String::from_utf8(dot_stuff(&message.bytes)).unwrap();
        assert_eq!(
            stuffed,
            "Subject: Hi\r\n\r\nbefore\r\n..\r\nMAIL FROM:<x@example.com>\r\n..\r\nafter\r\n.\r\n"
        );
        // The only line that is exactly one dot is the last.
        let lone_dots = stuffed.split("\r\n").filter(|line| *line == ".").count();
        assert_eq!(lone_dots, 1);
    }
}
