//! Attachments on disk: taking a file out of a message so that something
//! outside Quill can show it, and saving one where the user keeps files.
//!
//! This runs on the store worker, never on the UI task. A message's parts
//! are untrusted: the name a sender gave a file is cleaned before it names
//! anything on disk, every file is written into a folder of Quill's own
//! making, and every file is marked as having come from outside, so that
//! macOS asks before running anything in it.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use quill_core::types::LocalMessageId;
use quill_store::Store;

/// Where the store worker may put files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// A private folder for files handed to a viewer. It is the caller's to
    /// make and to remove. `None`: nothing can be viewed.
    pub view_dir: Option<PathBuf>,
    /// Where a saved attachment goes when there is no panel to ask with:
    /// the user's Downloads folder.
    pub downloads: Option<PathBuf>,
}

/// A file attached to a message, as the reader lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedFile {
    /// Which part of the message it is.
    pub part: u32,
    /// Its name, cleaned for showing and for use as a file name. Never
    /// empty.
    pub name: String,
    pub content_type: String,
    pub size: u64,
    /// The sender marked it to be shown in the body.
    pub inline: bool,
}

impl AttachedFile {
    /// Sound or moving pictures: something to play, not to look at.
    pub fn is_playable(&self) -> bool {
        self.content_type.starts_with("audio/") || self.content_type.starts_with("video/")
    }
}

/// A file taken out of a message and written where a viewer can read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    pub path: PathBuf,
    pub name: String,
    pub content_type: String,
}

/// The files of a parsed message, named safely. Two files of one name get
/// different names, so each can be told from the other and written beside
/// it.
pub fn attached(attachments: &[quill_mime::Attachment]) -> Vec<AttachedFile> {
    let mut files: Vec<AttachedFile> = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let wanted = safe_name(
            attachment.name.as_deref().unwrap_or_default(),
            &attachment.content_type,
            attachment.part,
        );
        let mut name = wanted.clone();
        let mut copy = 2;
        while files.iter().any(|file| file.name == name) {
            name = numbered(&wanted, copy);
            copy += 1;
        }
        files.push(AttachedFile {
            part: attachment.part,
            name,
            content_type: attachment.content_type.clone(),
            size: attachment.size as u64,
            inline: attachment.inline,
        });
    }
    files
}

/// `name` with a number before its extension: `photo 2.png`.
fn numbered(name: &str, number: usize) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem} {number}.{extension}"),
        _ => format!("{name} {number}"),
    }
}

/// The longest a file name may be, in bytes: well inside what a file system
/// takes, with room for a number.
const MAX_NAME: usize = 120;

/// A sender's name for a file, made safe to show and to use on disk.
///
/// Only the last path component is kept, so a name cannot climb out of the
/// folder it is written to. Control characters are dropped, and so are the
/// characters that reorder text: a right-to-left override in the middle of
/// a name makes an `.exe` read as a `.pdf`. Leading dots, which would hide
/// the file, go too. A name with nothing left is replaced by one made from
/// the part number and the media type.
pub fn safe_name(name: &str, content_type: &str, part: u32) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !is_reordering(*c) && *c != ':')
        .collect();
    cleaned = cleaned.trim().trim_start_matches('.').trim().to_owned();
    if cleaned.len() > MAX_NAME {
        // Keep the extension: it is what says how the file will open.
        let extension = cleaned
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_owned())
            .filter(|extension| extension.len() <= 12);
        let room = MAX_NAME - extension.as_ref().map_or(0, |e| e.len() + 1);
        let mut end = room.min(cleaned.len());
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
        if let Some(extension) = extension {
            cleaned = format!("{}.{extension}", cleaned.trim_end_matches('.'));
        }
    }
    if cleaned.is_empty() {
        let extension = extension_for(content_type);
        return format!("attachment-{part}.{extension}");
    }
    cleaned
}

/// Characters that change the order text is drawn in, and the ones that
/// take no space at all.
fn is_reordering(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// An extension for a file whose sender gave it no name.
fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" | "image/heif" => "heic",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/calendar" => "ics",
        "message/rfc822" => "eml",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "application/zip" => "zip",
        _ => "bin",
    }
}

/// Why a file could not be taken out or saved, in words for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProblem(pub String);

impl From<quill_store::StoreError> for FileProblem {
    fn from(err: quill_store::StoreError) -> Self {
        Self(err.to_string())
    }
}

/// Writes attachments of `message` into the view folder and says where:
/// the one at `part`, or with `all` every one, so that a viewer can step
/// from it to the others. Returns the files and which of them is `part`.
pub fn extract(
    store: &Store,
    files: &Files,
    message: LocalMessageId,
    part: u32,
    all: bool,
) -> Result<(Vec<ExtractedFile>, usize), FileProblem> {
    let Some(view_dir) = &files.view_dir else {
        return Err(FileProblem(
            "There is nowhere to put files for viewing.".to_owned(),
        ));
    };
    let raw = store.raw(message)?.ok_or_else(|| {
        FileProblem("That message has not been downloaded yet. Open it first.".to_owned())
    })?;
    let parsed = quill_mime::parse(&raw, 0)
        .ok_or_else(|| FileProblem("That message cannot be read.".to_owned()))?;
    // One folder per message, so files of the same name from two messages
    // never meet.
    let folder = view_dir.join(format!("m{}", message.0));
    private_dir(view_dir)?;
    private_dir(&folder)?;

    let mut written = Vec::new();
    let mut asked_for = None;
    for file in attached(&parsed.attachments) {
        if file.part == part {
            asked_for = Some(written.len());
        } else if !all {
            continue;
        }
        let path = folder.join(&file.name);
        // Written once: the same part of the same message is the same bytes.
        if !path.exists() {
            let bytes = quill_mime::attachment_bytes(&raw, file.part)
                .ok_or_else(|| FileProblem(format!("{} is not in the message.", file.name)))?;
            write_private(&path, &bytes)
                .map_err(|err| FileProblem(format!("{} cannot be written: {err}.", file.name)))?;
        }
        written.push(ExtractedFile {
            path,
            name: file.name,
            content_type: file.content_type,
        });
    }
    match asked_for {
        Some(index) => Ok((written, index)),
        None => Err(FileProblem("That file is not in the message.".to_owned())),
    }
}

/// Saves one attachment in the user's Downloads folder, under its own name
/// or the next free one like it. Returns the name it was saved under.
pub fn save(
    store: &Store,
    files: &Files,
    message: LocalMessageId,
    part: u32,
) -> Result<String, FileProblem> {
    let Some(downloads) = &files.downloads else {
        return Err(FileProblem(
            "There is no Downloads folder to save to.".to_owned(),
        ));
    };
    let raw = store.raw(message)?.ok_or_else(|| {
        FileProblem("That message has not been downloaded yet. Open it first.".to_owned())
    })?;
    let parsed = quill_mime::parse(&raw, 0)
        .ok_or_else(|| FileProblem("That message cannot be read.".to_owned()))?;
    let file = attached(&parsed.attachments)
        .into_iter()
        .find(|file| file.part == part)
        .ok_or_else(|| FileProblem("That file is not in the message.".to_owned()))?;
    let bytes = quill_mime::attachment_bytes(&raw, part)
        .ok_or_else(|| FileProblem(format!("{} is not in the message.", file.name)))?;
    fs::create_dir_all(downloads)
        .map_err(|err| FileProblem(format!("The Downloads folder cannot be used: {err}.")))?;
    // Never over a file that is already there.
    let mut name = file.name.clone();
    let mut copy = 2;
    loop {
        match write_new(&downloads.join(&name), &bytes) {
            Ok(()) => return Ok(name),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && copy < 1000 => {
                name = numbered(&file.name, copy);
                copy += 1;
            }
            Err(err) => {
                return Err(FileProblem(format!(
                    "{} cannot be saved: {err}.",
                    file.name
                )));
            }
        }
    }
}

/// A folder only the user can enter.
fn private_dir(path: &Path) -> Result<(), FileProblem> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(FileProblem(format!(
            "A folder for viewing files cannot be made: {err}."
        ))),
    }
}

/// Writes a file only the user can read, replacing any that was there.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    quarantine(path);
    Ok(())
}

/// Writes a file that must not already exist.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    file.write_all(bytes)?;
    quarantine(path);
    Ok(())
}

/// Marks a file as having come from outside, as a browser marks a
/// download, so that macOS checks it and asks before running anything in
/// it. Inside the App Sandbox the system does this by itself and may not
/// let Quill do it too; a failure here is not a failure to write the file.
fn quarantine(path: &Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    // Flags 0081: quarantined, and not yet approved by the user.
    let value = format!("0081;{now:08x};Quill;");
    let _ = rustix::fs::setxattr(
        path,
        "com.apple.quarantine",
        value.as_bytes(),
        rustix::fs::XattrFlags::empty(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_leave_its_folder_or_hide() {
        for (given, wanted) in [
            ("roadmap.pdf", "roadmap.pdf"),
            ("../../etc/passwd", "passwd"),
            ("C:\\Users\\robin\\report.docx", "report.docx"),
            ("/absolute/path/notes.txt", "notes.txt"),
            (".bashrc", "bashrc"),
            ("...", "attachment-7.pdf"),
            ("", "attachment-7.pdf"),
            ("   ", "attachment-7.pdf"),
            ("a/", "attachment-7.pdf"),
            ("Q4 notes ☕.txt", "Q4 notes ☕.txt"),
        ] {
            assert_eq!(safe_name(given, "application/pdf", 7), wanted, "{given:?}");
        }
    }

    #[test]
    fn a_name_cannot_disguise_what_it_is_or_draw_on_the_terminal() {
        // Right-to-left override: this would read "invoiceexe.pdf".
        assert_eq!(
            safe_name("invoice\u{202E}fdp.exe", "application/octet-stream", 1),
            "invoicefdp.exe"
        );
        // An escape sequence and a line break in a name.
        assert_eq!(
            safe_name("a\x1b]0;owned\x07b\r\nc.txt", "text/plain", 1),
            "a]0;ownedbc.txt"
        );
        assert_eq!(
            safe_name("zero\u{200B}width.pdf", "application/pdf", 1),
            "zerowidth.pdf"
        );
        // A colon is a path separator to the Finder.
        assert_eq!(
            safe_name("12:30 notes.txt", "text/plain", 1),
            "1230 notes.txt"
        );
    }

    #[test]
    fn a_very_long_name_is_cut_and_keeps_its_extension() {
        let long = format!("{}.pdf", "é".repeat(300));
        let name = safe_name(&long, "application/pdf", 1);
        assert!(name.len() <= MAX_NAME, "{}", name.len());
        assert!(name.ends_with(".pdf"));
        assert!(name.starts_with('é'));
        // No extension to keep: just cut.
        assert_eq!(safe_name(&"x".repeat(500), "text/plain", 1).len(), MAX_NAME);
    }

    #[test]
    fn unnamed_files_are_named_for_what_they_are() {
        assert_eq!(safe_name("", "image/jpeg", 3), "attachment-3.jpg");
        assert_eq!(safe_name("", "message/rfc822", 4), "attachment-4.eml");
        assert_eq!(
            safe_name("", "application/x-unknown", 5),
            "attachment-5.bin"
        );
    }

    #[test]
    fn files_of_one_name_are_told_apart() {
        let file = |part, name: &str| quill_mime::Attachment {
            part,
            name: Some(name.to_owned()),
            content_type: "image/png".to_owned(),
            size: 10,
            content_id: None,
            inline: false,
        };
        let files = attached(&[
            file(1, "photo.png"),
            file(2, "photo.png"),
            file(3, "sub/photo.png"),
            file(4, "notes"),
            file(5, "notes"),
        ]);
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "photo.png",
                "photo 2.png",
                "photo 3.png",
                "notes",
                "notes 2"
            ]
        );
    }
}
