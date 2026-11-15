//! A draft as a text file, for writing in an editor that is not Quill's
//! own: headers, a blank line, then the body, as mutt has it.
//!
//! ```text
//! From: Work <robin@example.com>
//! To: Ana Ruiz <ana.ruiz@example.com>
//! Cc:
//! Bcc:
//! Subject: Q4 roadmap
//! Attach: roadmap.pdf
//!
//! Ana,
//! ```
//!
//! The file is what the user edits, so reading it back is forgiving: field
//! names in any case, fields in any order or missing, a long field
//! continued on indented lines. What it will not do is let the file say
//! more than a draft can: a header Quill does not know is dropped, not
//! sent, and `From` is for reading only. Which account sends is not
//! something an editor buffer decides.

use crate::drafts::{DraftAttachment, DraftContent};

/// A draft read back from its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftFile {
    /// The draft, with what the file says of its recipients, subject and
    /// body. Of the attachments it had, those still named in the file.
    pub content: DraftContent,
    /// Files newly named for attaching, by path, in the order written.
    pub attach: Vec<String>,
}

/// Writes `content` as a draft file. `from` is who it will be sent as, to
/// show at the top.
pub fn write(content: &DraftContent, from: &str) -> String {
    let mut text = String::new();
    let field = |text: &mut String, name: &str, value: &str| {
        text.push_str(name);
        text.push(':');
        let value = one_line(value);
        if !value.is_empty() {
            text.push(' ');
            text.push_str(&value);
        }
        text.push('\n');
    };
    field(&mut text, "From", from);
    field(&mut text, "To", &content.to);
    field(&mut text, "Cc", &content.cc);
    field(&mut text, "Bcc", &content.bcc);
    field(&mut text, "Subject", &content.subject);
    for attachment in &content.attachments {
        field(&mut text, "Attach", &attachment.name);
    }
    text.push('\n');
    text.push_str(&content.body);
    if !content.body.is_empty() && !content.body.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Reads a draft file back. `before` is the draft as it was written out:
/// what the file cannot carry (what it answers, its thread, the bytes of
/// its attachments) is kept from it.
pub fn read(text: &str, before: &DraftContent) -> DraftFile {
    // A file saved with Windows line ends is the same draft.
    let text = text.replace("\r\n", "\n");
    let (head, body) = match text.split_once("\n\n") {
        Some((head, body)) => (head, body),
        // No blank line: if it starts like a header block it is all
        // headers; otherwise the user deleted them and it is all body.
        None if looks_like_headers(&text) => (text.as_str(), ""),
        None => ("", text.as_str()),
    };
    // A file that starts with the blank line has no headers at all.
    let (head, body) = match text.strip_prefix('\n') {
        Some(rest) => ("", rest),
        None => (head, body),
    };

    let mut content = DraftContent {
        to: String::new(),
        cc: String::new(),
        bcc: String::new(),
        subject: String::new(),
        body: body.trim_end_matches('\n').to_owned(),
        attachments: Vec::new(),
        ..before.clone()
    };
    if !content.body.is_empty() {
        content.body.push('\n');
    }
    let mut attach = Vec::new();
    let mut kept: Vec<&DraftAttachment> = Vec::new();
    for (name, value) in fields(head) {
        match name.to_ascii_lowercase().as_str() {
            "to" => join(&mut content.to, &value),
            "cc" => join(&mut content.cc, &value),
            "bcc" => join(&mut content.bcc, &value),
            "subject" => content.subject = value,
            "attach" if !value.is_empty() => {
                // One of the files it already had, by name, each once; or
                // a new one, by where it is.
                let already = before.attachments.iter().find(|file| {
                    file.name == value && !kept.iter().any(|k| std::ptr::eq(*k, *file))
                });
                match already {
                    Some(file) => kept.push(file),
                    None => attach.push(value),
                }
            }
            // `From` is shown, not read. Anything else is not a draft's
            // to set.
            _ => {}
        }
    }
    content.attachments = kept.into_iter().cloned().collect();
    DraftFile { content, attach }
}

/// Whether text with no blank line in it is a block of headers.
fn looks_like_headers(text: &str) -> bool {
    text.lines().next().is_some_and(|line| {
        line.split_once(':').is_some_and(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "from" | "to" | "cc" | "bcc" | "subject" | "attach"
            )
        })
    })
}

/// The fields of a header block: a name, and its value with continuation
/// lines joined.
fn fields(head: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                fields.push((name.to_owned(), one_line(value)));
            }
        }
    }
    fields
}

/// Two `To:` lines are one list of people.
fn join(list: &mut String, more: &str) {
    if more.is_empty() {
        return;
    }
    if !list.is_empty() {
        list.push_str(", ");
    }
    list.push_str(more);
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn draft() -> DraftContent {
        DraftContent {
            to: "Ana Ruiz <ana.ruiz@example.com>".to_owned(),
            cc: String::new(),
            bcc: "kit@example.com".to_owned(),
            subject: "Q4 roadmap".to_owned(),
            body: "Ana,\n\nTwo weeks is fantasy.\n".to_owned(),
            in_reply_to: Some("<q4@example.com>".to_owned()),
            references: vec!["<q4@example.com>".to_owned()],
            attachments: vec![DraftAttachment {
                name: "roadmap.pdf".to_owned(),
                content_type: "application/pdf".to_owned(),
                blob_hash: "abc".to_owned(),
                size: 937,
            }],
            replied_to: Some(7),
        }
    }

    #[test]
    fn a_draft_is_headers_a_blank_line_and_the_body() {
        assert_eq!(
            write(&draft(), "Work <robin@example.com>"),
            "From: Work <robin@example.com>\nTo: Ana Ruiz <ana.ruiz@example.com>\nCc:\n\
             Bcc: kit@example.com\nSubject: Q4 roadmap\nAttach: roadmap.pdf\n\n\
             Ana,\n\nTwo weeks is fantasy.\n"
        );
        // An empty draft still has every field to fill in.
        assert_eq!(
            write(&DraftContent::default(), "robin@example.com"),
            "From: robin@example.com\nTo:\nCc:\nBcc:\nSubject:\n\n"
        );
    }

    #[test]
    fn what_was_written_reads_back_the_same() {
        let before = draft();
        let read = read(&write(&before, "Work <robin@example.com>"), &before);
        assert_eq!(read.content, before);
        assert!(read.attach.is_empty());
    }

    #[test]
    fn edits_are_read_and_what_the_file_cannot_carry_is_kept() {
        let before = draft();
        let file = "from: Someone Else <boss@example.com>\n\
                    TO: Ana Ruiz <ana.ruiz@example.com>,\n   sam@example.com\n\
                    To: lee@example.com\n\
                    Subject:   A better subject  \n\
                    X-Priority: 1\n\
                    Attach: /Users/robin/notes.txt\n\
                    Attach: roadmap.pdf\n\
                    \n\
                    New body.\n\nWith two paragraphs.\n\n\n";
        let read = read(file, &before);
        assert_eq!(
            read.content.to,
            "Ana Ruiz <ana.ruiz@example.com>, sam@example.com, lee@example.com"
        );
        // Fields left out of the file are empty: the user removed them.
        assert_eq!(read.content.cc, "");
        assert_eq!(read.content.bcc, "");
        assert_eq!(read.content.subject, "A better subject");
        assert_eq!(read.content.body, "New body.\n\nWith two paragraphs.\n");
        // The attachment still named is kept, bytes and all; the path is
        // a new file to attach.
        assert_eq!(read.content.attachments, before.attachments);
        assert_eq!(read.attach, ["/Users/robin/notes.txt"]);
        // What it answers is not the file's to change.
        assert_eq!(read.content.in_reply_to, before.in_reply_to);
        assert_eq!(read.content.references, before.references);
        assert_eq!(read.content.replied_to, Some(7));
    }

    #[test]
    fn an_attachment_whose_line_is_deleted_is_gone() {
        let before = draft();
        let read = read("To: ana.ruiz@example.com\nSubject: x\n\nBody\n", &before);
        assert!(read.content.attachments.is_empty());
        assert!(read.attach.is_empty());
        // Named twice, it is still attached once: the second is taken for
        // another file of that name.
        let twice = super::read(
            "Attach: roadmap.pdf\nAttach: roadmap.pdf\n\nBody\n",
            &before,
        );
        assert_eq!(twice.content.attachments.len(), 1);
        assert_eq!(twice.attach, ["roadmap.pdf"]);
    }

    #[test]
    fn a_file_with_its_headers_or_its_blank_line_gone_is_still_read() {
        let before = DraftContent::default();
        // All body.
        let read1 = read("Just a note.\nSecond line.\n", &before);
        assert_eq!(read1.content.body, "Just a note.\nSecond line.\n");
        assert_eq!(read1.content.to, "");
        // All headers.
        let read2 = read("To: ana.ruiz@example.com\nSubject: Empty\n", &before);
        assert_eq!(read2.content.to, "ana.ruiz@example.com");
        assert_eq!(read2.content.body, "");
        // Starts with the blank line.
        let read3 = read("\nTo: not a header, this is the body\n", &before);
        assert_eq!(read3.content.to, "");
        assert_eq!(read3.content.body, "To: not a header, this is the body\n");
        // Windows line ends, and nothing at all.
        let read4 = read("To: a@example.com\r\nSubject: Hi\r\n\r\nBody\r\n", &before);
        assert_eq!(
            (read4.content.subject.as_str(), read4.content.body.as_str()),
            ("Hi", "Body\n")
        );
        assert_eq!(read("", &before).content, DraftContent::default());
    }

    #[test]
    fn a_header_cannot_carry_a_line_of_its_own() {
        let mut sly = draft();
        sly.subject = "Hi\nBcc: spy@example.com".to_owned();
        sly.to = "ana@example.com\r\n\r\nInjected body".to_owned();
        let text = write(&sly, "robin@example.com");
        assert!(
            text.contains("Subject: Hi Bcc: spy@example.com\n"),
            "{text}"
        );
        let read = read(&text, &sly);
        assert_eq!(read.content.bcc, "kit@example.com");
        assert_eq!(read.content.body, sly.body);
    }
}
