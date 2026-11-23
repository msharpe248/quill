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
    save_into(store, downloads, "The Downloads folder", message, part).map(|(name, _)| name)
}

/// Saves one attachment in `folder`, under its own name or the next free
/// one like it, never over a file that is there. `called` is what the
/// folder is called when it cannot be used. Returns the name it was saved
/// under and where.
pub fn save_into(
    store: &Store,
    folder: &Path,
    called: &str,
    message: LocalMessageId,
    part: u32,
) -> Result<(String, PathBuf), FileProblem> {
    let downloads = folder;
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
    save_new(downloads, called, &file.name, &bytes)
}

/// Writes `bytes` in `folder` as `wanted`, or under the next free name
/// like it, never over a file that is there. Returns the name it was
/// saved under and where.
fn save_new(
    folder: &Path,
    called: &str,
    wanted: &str,
    bytes: &[u8],
) -> Result<(String, PathBuf), FileProblem> {
    fs::create_dir_all(folder)
        .map_err(|err| FileProblem(format!("{called} cannot be used: {err}.")))?;
    let mut name = wanted.to_owned();
    let mut copy = 2;
    loop {
        let path = folder.join(&name);
        match write_new(&path, bytes) {
            Ok(()) => return Ok((name, path)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && copy < 1000 => {
                name = numbered(wanted, copy);
                copy += 1;
            }
            Err(err) => {
                return Err(FileProblem(format!("{wanted} cannot be saved: {err}.")));
            }
        }
    }
}

/// What a message is saved as, in a file of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveAs {
    /// Its text, which the asker has: its fields and what it says, laid
    /// out as lines.
    Text(String),
    /// Its page: its HTML made safe, as one file that needs nothing
    /// beside it, with `fields` (a label and a value each) said above
    /// it. `remote`: whether it may load what it names on the web.
    Page {
        remote: RemoteContent,
        fields: Vec<(String, String)>,
    },
    /// The message itself, as it was sent: every part of it, for any
    /// mail program to open.
    Source,
}

/// Where a message saved as a file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedMessage {
    /// Made ready in Quill's own folder, for the host to ask where it is
    /// to go.
    Ready(ExtractedFile),
    /// In the Downloads folder, under this name.
    InDownloads(String),
}

/// Writes a message out as a file of its own, named for its subject.
/// With `ask` it is made ready where the host can take it from, to put
/// wherever the user says; without, it goes in the Downloads folder,
/// never over a file that is there.
pub fn save_message(
    store: &Store,
    files: &Files,
    message: LocalMessageId,
    subject: &str,
    what: SaveAs,
    ask: bool,
) -> Result<SavedMessage, FileProblem> {
    let (bytes, extension, content_type) = match what {
        SaveAs::Text(text) => (text.into_bytes(), "txt", "text/plain"),
        SaveAs::Page { remote, fields } => (
            page_in_one(store, message, remote, &fields)?.into_bytes(),
            "html",
            "text/html",
        ),
        SaveAs::Source => (
            store.raw(message)?.ok_or_else(|| {
                FileProblem("That message has not been downloaded yet. Open it first.".to_owned())
            })?,
            "eml",
            "message/rfc822",
        ),
    };
    let name = message_file_name(subject, extension);
    if !ask {
        let Some(downloads) = &files.downloads else {
            return Err(FileProblem(
                "There is no Downloads folder to save to.".to_owned(),
            ));
        };
        return save_new(downloads, "The Downloads folder", &name, &bytes)
            .map(|(name, _)| SavedMessage::InDownloads(name));
    }
    let Some(view_dir) = &files.view_dir else {
        return Err(FileProblem(
            "There is nowhere to make the file ready.".to_owned(),
        ));
    };
    let folder = view_dir.join(format!("m{}", message.0));
    let saved = folder.join("saved");
    private_dir(view_dir)?;
    private_dir(&folder)?;
    private_dir(&saved)?;
    let path = saved.join(&name);
    write_private(&path, &bytes)
        .map_err(|err| FileProblem(format!("{name} cannot be written: {err}.")))?;
    Ok(SavedMessage::Ready(ExtractedFile {
        path,
        name,
        content_type: content_type.to_owned(),
    }))
}

/// What to call a message saved as a file: its subject, made safe to be
/// a file's name, and the extension. A subject is a sender's words, as
/// an attachment's name is: nothing of it is a path, a hidden file, or
/// an extension of its own choosing.
pub fn message_file_name(subject: &str, extension: &str) -> String {
    let cleaned: String = subject
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_whitespace() => ' ',
            c => c,
        })
        .filter(|c| !c.is_control() && !is_reordering(*c))
        .collect();
    // One space where there were several, or a line break.
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut stem = cleaned.trim_start_matches('.').trim().to_owned();
    let room = MAX_NAME - extension.len() - 1;
    if stem.len() > room {
        stem.truncate(stem.floor_char_boundary(room));
    }
    // A name ending in a dot or a space is one some file systems refuse.
    let stem = stem.trim_end_matches(['.', ' ']);
    let stem = if stem.is_empty() { "Message" } else { stem };
    format!("{stem}.{extension}")
}

/// A folder only the user can enter.
/// Whether a message's page may load pictures and the like from the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteContent {
    /// Never, whoever sent it.
    Never,
    /// Only if its sender is one the user has said always may.
    IfSenderAllowed,
    /// Yes: the user asked for it, for this message.
    Allowed,
}

/// A message's HTML made safe and written out as a page of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebPage {
    pub path: PathBuf,
    /// The message's subject.
    pub title: String,
    /// The page may load remote content.
    pub remote: bool,
    /// How many remote things were left out of it. Always 0 when `remote`.
    pub blocked: usize,
    /// Who sent the message, to offer "always from this sender".
    pub sender: Option<String>,
}

/// Writes a message's HTML, sanitized, as a page in a folder of its own,
/// with the pictures the message carries for it beside it. A message with
/// no HTML gets its plain text as a page. Nothing here fetches anything:
/// whether the page may is decided now and written into it. So is whether
/// it is dark: a page for a dark window has the message's own colours
/// made ones for a dark page, and is written again when the window is
/// light.
pub fn web_page(
    store: &Store,
    files: &Files,
    message: LocalMessageId,
    remote: RemoteContent,
    dark: bool,
) -> Result<WebPage, FileProblem> {
    let Some(view_dir) = &files.view_dir else {
        return Err(FileProblem(
            "There is nowhere to put files for viewing.".to_owned(),
        ));
    };
    let page = make_page(store, message, remote, dark, &Pictures::Beside)?;

    // A folder for the page alone: a page can only ever name what is
    // beside it, and nothing else is.
    let folder = view_dir.join(format!("m{}", message.0));
    let page_dir = folder.join(if page.remote { "page-remote" } else { "page" });
    private_dir(view_dir)?;
    private_dir(&folder)?;
    private_dir(&page_dir)?;
    let cannot = |err: std::io::Error| FileProblem(format!("The page cannot be written: {err}."));
    for (name, bytes) in &page.pictures {
        write_private(&page_dir.join(name), bytes).map_err(cannot)?;
    }
    let path = page_dir.join("message.html");
    write_private(&path, page.html.as_bytes()).map_err(cannot)?;
    Ok(WebPage {
        path,
        title: page.title,
        remote: page.remote,
        blocked: page.blocked,
        sender: page.sender,
    })
}

/// A message's page before it is written anywhere.
struct MadePage {
    html: String,
    title: String,
    remote: bool,
    blocked: usize,
    sender: Option<String>,
    /// The pictures the message carries for its page, under the names
    /// the page has for them.
    pictures: Vec<(String, Vec<u8>)>,
}

/// How a page names the pictures its message carries for it.
enum Pictures<'a> {
    /// As files that will lie beside it: `picture-1.png`.
    Beside,
    /// Under names that begin with this mark, which nothing in the
    /// message can have known: they are put inside the page afterwards,
    /// in place of their names. Only the kinds every browser draws.
    Within(&'a str),
}

/// Makes a message's HTML safe as a page. A message with no HTML gets its
/// plain text as one.
fn make_page(
    store: &Store,
    message: LocalMessageId,
    remote: RemoteContent,
    dark: bool,
    naming: &Pictures<'_>,
) -> Result<MadePage, FileProblem> {
    let raw = store.raw(message)?.ok_or_else(|| {
        FileProblem("That message has not been downloaded yet. Open it first.".to_owned())
    })?;
    let parsed = quill_mime::parse(&raw, 0)
        .ok_or_else(|| FileProblem("That message cannot be read.".to_owned()))?;
    let sender = parsed.headers.from.first().map(|from| from.email.clone());
    let remote = match remote {
        RemoteContent::Never => false,
        RemoteContent::Allowed => true,
        RemoteContent::IfSenderAllowed => match &sender {
            Some(address) => store.allows_remote_content(address)?,
            None => false,
        },
    };
    let html = match &parsed.html {
        Some(html) => html.clone(),
        None => format!("<pre>{}</pre>", escape_html(&parsed.body)),
    };

    // Pictures the message carries for its own page (`cid:` addresses)
    // are given names made here, never names from the message.
    let pictures = std::cell::RefCell::new(Vec::<(String, Vec<u8>)>::new());
    let cid_file = |content_id: &str| -> Option<String> {
        let (content_type, bytes) = quill_mime::part_by_content_id(&raw, content_id)?;
        if !content_type.starts_with("image/") || content_type == "image/svg+xml" {
            return None;
        }
        let extension = extension_for(&content_type);
        let mut pictures = pictures.borrow_mut();
        let name = match naming {
            Pictures::Beside => format!("picture-{}.{extension}", pictures.len() + 1),
            Pictures::Within(mark) => {
                embedded_type(extension)?;
                format!("{mark}-{}.{extension}", pictures.len() + 1)
            }
        };
        pictures.push((name.clone(), bytes));
        Some(name)
    };
    let sanitized = quill_mime::sanitize::sanitize(
        &html,
        &quill_mime::sanitize::Options {
            remote,
            cid_file: &cid_file,
            title: &parsed.headers.subject,
            dark,
        },
    );
    Ok(MadePage {
        html: sanitized.html,
        title: parsed.headers.subject,
        remote,
        blocked: sanitized.blocked,
        sender,
        pictures: pictures.into_inner(),
    })
}

/// The media type a picture with this extension is put inside a page as:
/// the kinds every browser draws, named here and never by the message.
fn embedded_type(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Where the sanitizer's own frame ends and the message begins: what is
/// said above a saved message goes in after it.
const BODY_OPENS: &str = "</head><body>\n";

/// A message's page as one file that needs nothing beside it: the
/// pictures the message carries are inside it, and `fields` are said
/// above the message, as the reader says them. The page is the one the
/// reading pane shows, from the same sanitizer, and is light or dark as
/// whatever opens it is.
fn page_in_one(
    store: &Store,
    message: LocalMessageId,
    remote: RemoteContent,
    fields: &[(String, String)],
) -> Result<String, FileProblem> {
    use base64::Engine as _;
    use std::hash::{BuildHasher, Hasher};

    // A different mark every time, so that no message can have written
    // the name of a picture into its own words.
    let chance = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let mark = format!("quill-{chance:016x}");
    let page = make_page(store, message, remote, false, &Pictures::Within(&mark))?;
    let mut html = page.html;
    for (name, bytes) in &page.pictures {
        let Some(kind) = name
            .rsplit_once('.')
            .and_then(|(_, extension)| embedded_type(extension))
        else {
            continue;
        };
        let inside = format!(
            "data:{kind};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        html = html.replace(&format!("./{name}"), &inside);
    }
    if let Some(at) = html.find(BODY_OPENS) {
        html.insert_str(at + BODY_OPENS.len(), &fields_block(fields));
    }
    Ok(html)
}

/// The fields of a message as a block of a page, above the message.
/// Every word of them is a sender's: escaped, and nothing but text.
fn fields_block(fields: &[(String, String)]) -> String {
    let mut block = String::from(
        "<div style=\"margin:0 0 1em;padding:0 0 .75em;border-bottom:1px solid #8886;\
         font:13px/1.5 -apple-system,system-ui,sans-serif\">\n",
    );
    for (label, value) in fields {
        block.push_str(&format!(
            "<div><span style=\"opacity:.7\">{}</span> {}</div>\n",
            escape_html(label),
            escape_html(value)
        ));
    }
    block.push_str("</div>\n");
    block
}

/// Writes a message's text as a page of its own, for the web reading pane
/// to show when the message's downloaded copy is not at hand: the same
/// page a message with no HTML gets, from the same sanitizer. It loads
/// nothing and names nothing.
pub fn text_page(
    files: &Files,
    message: LocalMessageId,
    title: &str,
    text: &str,
) -> Result<WebPage, FileProblem> {
    let Some(view_dir) = &files.view_dir else {
        return Err(FileProblem(
            "There is nowhere to put files for viewing.".to_owned(),
        ));
    };
    let folder = view_dir.join(format!("m{}", message.0));
    private_dir(view_dir)?;
    private_dir(&folder)?;
    own_page(
        &folder.join("text"),
        title,
        &format!("<pre>{}</pre>", escape_html(text)),
    )
}

/// Writes words of Quill's own as a page, for the web reading pane to show
/// while it has no message: that none is selected, or that this one is
/// still on its way.
pub fn note_page(files: &Files, text: &str) -> Result<WebPage, FileProblem> {
    let Some(view_dir) = &files.view_dir else {
        return Err(FileProblem(
            "There is nowhere to put files for viewing.".to_owned(),
        ));
    };
    private_dir(view_dir)?;
    own_page(
        &view_dir.join("note"),
        "",
        &format!(
            "<p style=\"text-align:center;margin-top:30vh;opacity:.6\">{}</p>",
            escape_html(text)
        ),
    )
}

/// Writes `html`, which is Quill's own and still goes through the
/// sanitizer every page goes through, as `message.html` in `page_dir`.
fn own_page(page_dir: &Path, title: &str, html: &str) -> Result<WebPage, FileProblem> {
    let no_parts = |_: &str| -> Option<String> { None };
    let sanitized = quill_mime::sanitize::sanitize(
        html,
        &quill_mime::sanitize::Options {
            remote: false,
            cid_file: &no_parts,
            title,
            // Quill's own words name no colours: they are light or dark
            // as the window is, by themselves.
            dark: false,
        },
    );
    private_dir(page_dir)?;
    let path = page_dir.join("message.html");
    write_private(&path, sanitized.html.as_bytes())
        .map_err(|err| FileProblem(format!("The page cannot be written: {err}.")))?;
    Ok(WebPage {
        path,
        title: title.to_owned(),
        remote: false,
        blocked: 0,
        sender: None,
    })
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

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
    fn a_saved_message_is_named_for_its_subject_and_never_by_it() {
        for (subject, wanted) in [
            ("Q4 roadmap", "Q4 roadmap.txt"),
            // Nothing of a path, and no folder to climb to.
            ("Re: plans for Q3/Q4", "Re- plans for Q3-Q4.txt"),
            ("../../etc/passwd", "-..-etc-passwd.txt"),
            ("C:\\temp\\x", "C--temp-x.txt"),
            // Not hidden, and not an extension of the sender's choosing
            // in place of ours.
            (".profile", "profile.txt"),
            ("invoice.exe", "invoice.exe.txt"),
            ("update\u{202E}txt.exe", "updatetxt.exe.txt"),
            // Nothing drawn on a terminal, and one line.
            ("a\x1b]0;owned\x07b\r\nc", "a]0;ownedb c.txt"),
            ("trailing dots...  ", "trailing dots.txt"),
            ("", "Message.txt"),
            (" . ", "Message.txt"),
        ] {
            assert_eq!(message_file_name(subject, "txt"), wanted, "{subject:?}");
        }
        let long = message_file_name(&"é".repeat(400), "html");
        assert!(long.len() <= MAX_NAME, "{}", long.len());
        assert!(long.ends_with(".html") && long.starts_with('é'));
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
