//! Writing a message in Vim: the draft's folder, and the Vim that a
//! terminal runs on it.
//!
//! The message goes out as a draft file (`quill_store::draft_file`) in a
//! folder of its own, and comes back from that file when Vim has gone.
//! Vim is not another [`crate::editor::Editor`]: nothing of it lives in
//! this process. What it is told and what it says go through small files
//! in the folder, as `app/Resources/vim/quill.vim` describes.
//!
//! The folder work here runs on the store worker. Starting Vim is the run
//! loop's, and only in a terminal: Quill's own app runs the Vim inside it.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use quill_core::types::LocalAccountId;
use quill_store::draft_file::{self, DraftFile};
use quill_store::{DraftContent, Store};

use crate::compose::written_address;
use crate::files::Files;
use crate::text;

/// The draft file's name in its folder.
pub const DRAFT_FILE: &str = "message.quill";
/// The plugin's name in the folder, where a terminal's Vim is told to
/// find it.
pub const PLUGIN_FILE: &str = "quill.vim";
const VERDICT_FILE: &str = "verdict";
const COMMAND_FILE: &str = "command";
const CONTACTS_FILE: &str = "contacts";
/// What every draft's folder is called, before its number.
const FOLDER_PREFIX: &str = "edit-";

/// The plugin, as it is in the app. A Vim of the user's own is given a
/// copy.
const PLUGIN: &str = include_str!("../../../app/Resources/vim/quill.vim");

/// The most people written out for Vim to complete addresses from.
const MOST_CONTACTS: usize = 300;
/// The largest draft file read back. A message written by hand is a
/// thousandth of this.
const MOST_DRAFT_BYTES: u64 = 8 * 1024 * 1024;
/// The most folders tried for a free name.
const MOST_FOLDERS: u32 = 10_000;

/// What to do with a draft, as the plugin says it and is told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Send,
    Postpone,
    Discard,
}

impl Verdict {
    /// The word in the `verdict` and `command` files.
    pub fn word(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Postpone => "postpone",
            Self::Discard => "discard",
        }
    }

    fn read(text: &str) -> Option<Self> {
        match text.lines().next().map(str::trim) {
            Some("send") => Some(Self::Send),
            Some("postpone") => Some(Self::Postpone),
            Some("discard") => Some(Self::Discard),
            _ => None,
        }
    }
}

/// A draft written out for Vim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandedOut {
    /// The draft's own folder.
    pub dir: PathBuf,
    /// The draft file in it.
    pub path: PathBuf,
    /// The line of the file the body starts on, counting from 1: where an
    /// editor that knows nothing of the file's shape should put the
    /// cursor, past the header fields.
    pub body_line: u32,
}

/// The line the body of a draft file starts on, counting from 1: the one
/// after the first empty line. The first line, if there is no empty one.
pub fn body_line(draft: &str) -> u32 {
    draft
        .lines()
        .position(str::is_empty)
        .and_then(|blank| u32::try_from(blank + 2).ok())
        .unwrap_or(1)
}

/// Makes a folder for one edit and writes the draft into it: the draft
/// file, the people to complete addresses from, and with `plugin` a copy
/// of the plugin for a Vim that is not Quill's own.
pub fn hand_out(
    store: &Store,
    files: &Files,
    account: LocalAccountId,
    content: &DraftContent,
    plugin: bool,
) -> Result<HandedOut, String> {
    let Some(view_dir) = &files.view_dir else {
        return Err("There is nowhere to put a draft for Vim.".to_owned());
    };
    let cannot = |err: std::io::Error| format!("The draft cannot be written out for Vim: {err}.");
    match fs::DirBuilder::new().mode(0o700).create(view_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(cannot(err)),
    }
    // A folder nobody else has made: never one that was already there.
    let mut dir = None;
    for number in 1..=MOST_FOLDERS {
        let candidate = view_dir.join(format!("{FOLDER_PREFIX}{number}"));
        match fs::DirBuilder::new().mode(0o700).create(&candidate) {
            Ok(()) => {
                dir = Some(candidate);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(cannot(err)),
        }
    }
    let Some(dir) = dir else {
        return Err("There is no room left for another draft for Vim.".to_owned());
    };

    // Who it goes from is shown at the top, and never read back.
    let from = store
        .accounts()
        .ok()
        .and_then(|accounts| accounts.into_iter().find(|row| row.id == account))
        .map(|row| written_address(&row.name, &row.address))
        .unwrap_or_default();
    // Addresses are what other people's mail said they were: each is made
    // one clean line before it is offered.
    let contacts: String = store
        .top_contacts(MOST_CONTACTS)
        .unwrap_or_default()
        .iter()
        .map(|person| text::sanitize_line(&written_address(&person.name, &person.email)))
        .filter(|line| !line.trim().is_empty())
        .map(|line| line + "\n")
        .collect();

    let path = dir.join(DRAFT_FILE);
    let draft = draft_file::write(content, &from);
    let body_line = body_line(&draft);
    let written = (|| {
        write_own(&path, draft.as_bytes())?;
        write_own(&dir.join(CONTACTS_FILE), contacts.as_bytes())?;
        if plugin {
            write_own(&dir.join(PLUGIN_FILE), PLUGIN.as_bytes())?;
        }
        Ok(())
    })();
    match written {
        Ok(()) => Ok(HandedOut {
            dir,
            path,
            body_line,
        }),
        Err(err) => {
            let _ = fs::remove_dir_all(&dir);
            Err(cannot(err))
        }
    }
}

/// Reads a draft back from its folder once Vim has gone, with what the
/// plugin said to do with it, and removes the folder. `before` is the
/// draft as it was written out. The folder goes whether or not the draft
/// could be read.
pub fn take_back(
    files: &Files,
    dir: &Path,
    before: &DraftContent,
) -> Result<(DraftFile, Option<Verdict>), String> {
    if !is_ours(files, dir) {
        return Err("That is not a draft's folder.".to_owned());
    }
    let read = read_limited(&dir.join(DRAFT_FILE), MOST_DRAFT_BYTES);
    let verdict = read_limited(&dir.join(VERDICT_FILE), 64)
        .ok()
        .and_then(|bytes| Verdict::read(&String::from_utf8_lossy(&bytes)));
    let _ = fs::remove_dir_all(dir);
    let bytes = read.map_err(|why| format!("What was written in Vim cannot be read: {why}."))?;
    // Whatever encoding the file was saved in, what can be read of it is
    // kept: a message is not lost to one stray byte.
    let text = String::from_utf8_lossy(&bytes);
    Ok((draft_file::read(&text, before), verdict))
}

/// Removes a draft's folder unread.
pub fn drop_folder(files: &Files, dir: &Path) {
    if is_ours(files, dir) {
        let _ = fs::remove_dir_all(dir);
    }
}

/// Tells the Vim running on a draft's folder what to do, as if the
/// matching command had been typed there. The plugin looks for the file a
/// few times a second, so it is put in place whole.
pub fn tell(files: &Files, dir: &Path, verdict: Verdict) -> Result<(), String> {
    if !is_ours(files, dir) {
        return Err("That is not a draft's folder.".to_owned());
    }
    let cannot = |err: std::io::Error| format!("Vim could not be told: {err}.");
    let unfinished = dir.join(format!("{COMMAND_FILE}.new"));
    write_own(&unfinished, format!("{}\n", verdict.word()).as_bytes()).map_err(cannot)?;
    fs::rename(&unfinished, dir.join(COMMAND_FILE)).map_err(cannot)
}

/// Whether `dir` is a draft's folder that [`hand_out`] made: nothing else
/// is ever read from, written into or removed on the UI's say-so.
fn is_ours(files: &Files, dir: &Path) -> bool {
    let Some(view_dir) = &files.view_dir else {
        return false;
    };
    dir.parent() == Some(view_dir.as_path())
        && dir
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_prefix(FOLDER_PREFIX))
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        && fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir())
}

/// Writes a file only the user can read. It is Quill's own text or the
/// user's, so unlike a file taken out of a message it is not marked as
/// having come from outside.
fn write_own(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// A file's bytes, if it is a plain file of at most `most` bytes.
fn read_limited(path: &Path, most: u64) -> Result<Vec<u8>, String> {
    // Looked at before it is opened: a link is not followed, and a pipe
    // is not waited on.
    let metadata = fs::symlink_metadata(path).map_err(|err| err.to_string())?;
    if !metadata.is_file() {
        return Err("it is not a file".to_owned());
    }
    if metadata.len() > most {
        return Err("it is too large".to_owned());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|err| err.to_string())?
        .take(most + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > most {
        return Err("it is too large".to_owned());
    }
    Ok(bytes)
}

/// The program a terminal runs when none is named.
pub const PROGRAM: &str = "vim";

/// What Vim is told before it reads the user's vimrc: mail quotes other
/// people's text, and a line of it that looks like a modeline must not
/// set options. Said for the buffer as it is read, so that a vimrc which
/// turns modelines on does not undo it, and in a group of its own, so that
/// a vimrc which starts with a bare `autocmd!` does not clear it.
const NO_MODELINES: &str =
    "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END";

/// What Vim is told once the draft is loaded: to read the plugin from the
/// draft's folder. The folder comes from the environment, so no path is
/// ever written into a command for Vim to parse.
const SOURCE_PLUGIN: &str = "execute 'source' fnameescape($QUILL_DRAFT_DIR . '/quill.vim')";

/// The variable that tells the plugin where the draft's folder is.
pub const DIR_VARIABLE: &str = "QUILL_DRAFT_DIR";

/// The editor outside Quill that a message is written in, when it is not
/// written in Quill's own. Most of this file is about Vim, which has
/// Quill's plugin to talk through; the others are given the draft file
/// and nothing more, and the message comes back when they have saved it
/// and gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExternalEditor {
    #[default]
    Vim,
    Nano,
    Pico,
    Mg,
    /// The user's own Vim. Quill's app reaches it through a script of
    /// theirs; in a terminal it is simply their Vim.
    Script,
}

impl ExternalEditor {
    /// What it is called, to the user.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vim | Self::Script => "Vim",
            Self::Nano => "nano",
            Self::Pico => "Pico",
            Self::Mg => "mg",
        }
    }

    /// What closing it from Quill is called, in the bottom bar.
    pub fn close_hint(self) -> &'static str {
        match self {
            Self::Vim | Self::Script => "close Vim",
            Self::Nano => "close nano",
            Self::Pico => "close Pico",
            Self::Mg => "close mg",
        }
    }

    /// Whether it is a Vim, with Quill's plugin loaded: `:QuillSend` and
    /// the rest, and Send and Save reaching it from Quill.
    pub fn is_vim(self) -> bool {
        matches!(self, Self::Vim | Self::Script)
    }

    /// The program a terminal runs, by the name it has on a `PATH`.
    pub fn program(self) -> &'static str {
        match self {
            Self::Vim | Self::Script => PROGRAM,
            Self::Nano => "nano",
            Self::Pico => "pico",
            Self::Mg => "mg",
        }
    }

    /// Which of the editors Quill's app can run this is, if it is one.
    /// On a Mac nano is Pico, so the app has one word for both. mg is not
    /// one: the mg that comes with macOS crashes as it starts inside the
    /// app's sandbox, so the app is never asked for it.
    pub fn in_the_app(self) -> Option<quill_host::bridge::EditorKind> {
        use quill_host::bridge::EditorKind;
        match self {
            Self::Vim => Some(EditorKind::Vim),
            Self::Nano | Self::Pico => Some(EditorKind::Nano),
            Self::Mg => None,
            Self::Script => Some(EditorKind::Script),
        }
    }
}

/// How a terminal's editor is started on a draft: the user's own, with
/// their own settings and environment, in the draft's folder. An argv,
/// never a line for a shell.
pub fn command(
    editor: ExternalEditor,
    program: &Path,
    path: &Path,
    dir: &Path,
    body_line: u32,
) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    match editor {
        ExternalEditor::Vim | ExternalEditor::Script => {
            command
                .arg("-N")
                .arg("--cmd")
                .arg(NO_MODELINES)
                .arg("-c")
                .arg(SOURCE_PLUGIN)
                .arg("--");
        }
        // Save on leaving, without asking: the draft is the one file this
        // run is for. Pico and nano both call that -t. They know nothing
        // of the file's shape, so the cursor is put where the body starts,
        // past the header fields: Vim's plugin does that for Vim.
        ExternalEditor::Nano | ExternalEditor::Pico => {
            command.arg("-t").arg(format!("+{body_line}"));
        }
        // No backup copy of the draft left beside it.
        ExternalEditor::Mg => {
            command.arg("-n").arg(format!("+{body_line}"));
        }
    }
    // The draft's path is absolute, so it never reads as an option.
    command.arg(path).env(DIR_VARIABLE, dir).current_dir(dir);
    command
}

/// Where the program called `name` is: the name itself when it is a path,
/// otherwise the first folder of `search` (a `PATH`) that has it. Looked
/// for before the terminal is given up, so that having no Vim costs
/// nothing but a line saying so.
pub fn find_program(name: &OsStr, search: Option<&OsStr>) -> Option<PathBuf> {
    let runnable = |path: &Path| {
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    };
    if name.is_empty() {
        return None;
    }
    let named = Path::new(name);
    if named.components().count() > 1 {
        return runnable(named).then(|| named.to_owned());
    }
    std::env::split_paths(search?)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(named))
        .find(|candidate| runnable(candidate))
}

/// The program a terminal runs as `editor`: for Vim the one named
/// (`QUILL_VIM`) or `vim`; for the others, the name they go by.
pub fn program_name(editor: ExternalEditor, named: Option<&OsString>) -> &OsStr {
    named
        .filter(|_| editor.is_vim())
        .map(OsString::as_os_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(OsStr::new(editor.program()))
}

#[cfg(test)]
mod tests {
    use quill_core::types::{
        Address, Flags, FolderId, FolderRole, Headers, RemoteFolder, RemoteId, RemoteMessage,
        SyncCursor, SyncDelta,
    };
    use quill_store::DraftAttachment;

    use super::*;

    fn store() -> (Store, LocalAccountId) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "demo")
            .unwrap();
        (store, account)
    }

    fn files(dir: &tempfile::TempDir) -> Files {
        Files {
            view_dir: Some(dir.path().join("view")),
            downloads: None,
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn draft() -> DraftContent {
        DraftContent {
            to: "Ana Ruiz <ana.ruiz@example.com>".to_owned(),
            subject: "Q4 roadmap".to_owned(),
            body: "Ana,\n\nHere it is.\n".to_owned(),
            in_reply_to: Some("<1@example.com>".to_owned()),
            attachments: vec![DraftAttachment {
                name: "roadmap.pdf".to_owned(),
                content_type: "application/pdf".to_owned(),
                blob_hash: "abc".to_owned(),
                size: 3,
            }],
            ..DraftContent::default()
        }
    }

    #[test]
    fn a_draft_goes_out_to_a_private_folder_and_comes_back_the_same() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (store, account) = store();
        let out = hand_out(&store, &files, account, &draft(), true).unwrap();

        // Only the user can enter the folder or read what is in it.
        assert_eq!(out.dir.parent(), files.view_dir.as_deref());
        assert_eq!(mode(&out.dir), 0o700);
        assert_eq!(out.path, out.dir.join(DRAFT_FILE));
        for name in [DRAFT_FILE, CONTACTS_FILE, PLUGIN_FILE] {
            assert_eq!(mode(&out.dir.join(name)), 0o600, "{name}");
        }
        let text = fs::read_to_string(&out.path).unwrap();
        assert!(
            text.starts_with(
                "From: Work <robin@example.com>\nTo: Ana Ruiz <ana.ruiz@example.com>\n"
            ),
            "{text}"
        );
        assert!(text.contains("\nAttach: roadmap.pdf\n\nAna,\n"), "{text}");
        // The plugin is the one the app ships.
        assert_eq!(
            fs::read_to_string(out.dir.join(PLUGIN_FILE)).unwrap(),
            PLUGIN
        );

        // Untouched, it reads back as it went out, with no verdict.
        let (file, verdict) = take_back(&files, &out.dir, &draft()).unwrap();
        assert_eq!(file.content, draft());
        assert!(file.attach.is_empty());
        assert_eq!(verdict, None);
        // And the folder has gone.
        assert!(!out.dir.exists());
    }

    #[test]
    fn what_was_written_and_the_verdict_come_back() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (store, account) = store();
        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        // Quill's own Vim has the plugin already.
        assert!(!out.dir.join(PLUGIN_FILE).exists());
        fs::write(
            &out.path,
            "From: Someone Else <x@example.com>\nTo: sam.lee@example.com\nSubject: Changed\n\
             Attach: /tmp/new.txt\nX-Injected: yes\n\nNew text.\n",
        )
        .unwrap();
        fs::write(out.dir.join(VERDICT_FILE), "send\n").unwrap();

        let (file, verdict) = take_back(&files, &out.dir, &draft()).unwrap();
        assert_eq!(verdict, Some(Verdict::Send));
        assert_eq!(file.content.to, "sam.lee@example.com");
        assert_eq!(file.content.subject, "Changed");
        assert_eq!(file.content.body, "New text.\n");
        // The file it had is no longer named, and a new one is.
        assert!(file.content.attachments.is_empty());
        assert_eq!(file.attach, ["/tmp/new.txt"]);
        // What a file cannot say is kept from the draft.
        assert_eq!(file.content.in_reply_to.as_deref(), Some("<1@example.com>"));
        assert!(!out.dir.exists());
    }

    #[test]
    fn each_edit_has_a_folder_of_its_own() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (store, account) = store();
        let first = hand_out(&store, &files, account, &draft(), false).unwrap();
        let second = hand_out(&store, &files, account, &draft(), false).unwrap();
        assert_ne!(first.dir, second.dir);
        drop_folder(&files, &first.dir);
        assert!(!first.dir.exists() && second.dir.exists());
        // A folder that was already there is never taken over.
        let third = hand_out(&store, &files, account, &draft(), false).unwrap();
        assert!(third.dir != second.dir && third.dir.exists());
    }

    #[test]
    fn a_draft_that_cannot_be_read_says_so_and_its_folder_still_goes() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (store, account) = store();

        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        fs::remove_file(&out.path).unwrap();
        let problem = take_back(&files, &out.dir, &draft()).unwrap_err();
        assert!(problem.starts_with("What was written in Vim cannot be read"));
        assert!(!out.dir.exists());

        // A link in the file's place is not followed.
        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        let elsewhere = scratch.path().join("elsewhere");
        fs::write(&elsewhere, "To: mallory@example.com\n\nNot the draft.\n").unwrap();
        fs::remove_file(&out.path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &out.path).unwrap();
        assert!(take_back(&files, &out.dir, &draft()).is_err());
        assert!(elsewhere.exists());

        // Bytes that are not text lose only themselves.
        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        fs::write(&out.path, b"To: ana.ruiz@example.com\n\nCaf\xe9 at ten.\n").unwrap();
        let (file, _) = take_back(&files, &out.dir, &draft()).unwrap();
        assert_eq!(file.content.body, "Caf\u{fffd} at ten.\n");
    }

    #[test]
    fn only_a_drafts_own_folder_is_ever_touched() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let other = scratch.path().join("precious");
        fs::create_dir(&other).unwrap();
        fs::write(other.join(DRAFT_FILE), "To: x@example.com\n\nHi\n").unwrap();
        // Not in the view folder, not named as a draft's folder is, or
        // reached by a path that climbs out.
        fs::create_dir_all(files.view_dir.as_ref().unwrap().join("m7")).unwrap();
        let inside = files.view_dir.as_ref().unwrap().join("m7");
        let climbing = files
            .view_dir
            .as_ref()
            .unwrap()
            .join("edit-1/../../precious");
        for dir in [&other, &inside, &climbing] {
            assert!(take_back(&files, dir, &draft()).is_err());
            assert!(tell(&files, dir, Verdict::Send).is_err());
            drop_folder(&files, dir);
        }
        assert!(other.join(DRAFT_FILE).exists() && inside.exists());
        // With nowhere to put files, nothing is handed out at all.
        let (store, account) = store();
        assert!(hand_out(&store, &Files::default(), account, &draft(), true).is_err());
    }

    #[test]
    fn vim_is_told_what_to_do_through_the_command_file() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (store, account) = store();
        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        tell(&files, &out.dir, Verdict::Postpone).unwrap();
        assert_eq!(
            fs::read_to_string(out.dir.join(COMMAND_FILE)).unwrap(),
            "postpone\n"
        );
        // Nothing half-written is left beside it.
        assert!(!out.dir.join("command.new").exists());
        tell(&files, &out.dir, Verdict::Send).unwrap();
        assert_eq!(
            fs::read_to_string(out.dir.join(COMMAND_FILE)).unwrap(),
            "send\n"
        );
    }

    #[test]
    fn the_people_written_to_most_are_offered_one_to_a_line() {
        let scratch = tempfile::tempdir().unwrap();
        let files = files(&scratch);
        let (mut store, account) = store();
        let folder = store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId("inbox".to_owned()),
                    name: "Inbox".to_owned(),
                    role: FolderRole::Inbox,
                },
            )
            .unwrap();
        // People are learnt from the mail they send.
        let from = [
            Address::new("Ana Ruiz", "ana.ruiz@example.com"),
            Address::new("Evil\nTo: x@example.com", "eve@example.com"),
            Address {
                name: None,
                email: "sam.lee@example.com".to_owned(),
            },
        ];
        let delta = SyncDelta {
            added: from
                .iter()
                .enumerate()
                .map(|(number, from)| RemoteMessage {
                    id: RemoteId(number.to_string()),
                    headers: Headers {
                        message_id: None,
                        subject: "Hello".to_owned(),
                        from: vec![from.clone()],
                        to: Vec::new(),
                        cc: Vec::new(),
                        date: 100,
                    },
                    flags: Flags::default(),
                    size: 10,
                    has_attachments: false,
                    thread: Vec::new(),
                })
                .collect(),
            changed: Vec::new(),
            removed: Vec::new(),
            reset: false,
            has_more: false,
            next_cursor: SyncCursor("c1".to_owned()),
        };
        store.apply_delta(folder, &delta).unwrap();
        let out = hand_out(&store, &files, account, &draft(), false).unwrap();
        let contacts = fs::read_to_string(out.dir.join(CONTACTS_FILE)).unwrap();
        let lines: Vec<&str> = contacts.lines().collect();
        // Each is one line, whatever a sender called themselves.
        assert_eq!(
            lines,
            [
                "Ana Ruiz <ana.ruiz@example.com>",
                "\"Evil To: x@example.com\" <eve@example.com>",
                "sam.lee@example.com",
            ]
        );
    }

    #[test]
    fn a_terminals_vim_is_started_with_an_argv_and_the_folder_in_its_environment() {
        assert_eq!(body_line("From: a\nTo: b\nSubject: c\n\nHello\n"), 5);
        assert_eq!(body_line("From: a\n\n\nHello\n"), 3);
        assert_eq!(body_line("\nHello\n"), 2);
        assert_eq!(body_line("no empty line at all\n"), 1);
        assert_eq!(body_line(""), 1);
        let command = command(
            ExternalEditor::Vim,
            Path::new("/usr/bin/vim"),
            Path::new("/tmp/view/edit-1/message.quill"),
            Path::new("/tmp/view/edit-1"),
            7,
        );
        assert_eq!(command.get_program(), "/usr/bin/vim");
        let arguments: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(
            arguments,
            [
                "-N",
                "--cmd",
                "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c",
                "execute 'source' fnameescape($QUILL_DRAFT_DIR . '/quill.vim')",
                "--",
                "/tmp/view/edit-1/message.quill",
            ]
        );
        let set: Vec<_> = command.get_envs().collect();
        assert_eq!(
            set,
            [(
                OsStr::new("QUILL_DRAFT_DIR"),
                Some(OsStr::new("/tmp/view/edit-1"))
            )]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(Path::new("/tmp/view/edit-1"))
        );
    }

    #[test]
    fn the_program_is_looked_for_on_the_path_or_taken_as_named() {
        let scratch = tempfile::tempdir().unwrap();
        let bin = scratch.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let program = bin.join("vim");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let text = bin.join("notes");
        fs::write(&text, "not a program").unwrap();
        fs::set_permissions(&text, fs::Permissions::from_mode(0o644)).unwrap();

        let search = std::env::join_paths([scratch.path().join("empty"), bin.clone()]).unwrap();
        assert_eq!(
            find_program(OsStr::new("vim"), Some(&search)),
            Some(program.clone())
        );
        // A file that cannot be run is not a program, and a folder that
        // is not there is passed over.
        assert_eq!(find_program(OsStr::new("notes"), Some(&search)), None);
        assert_eq!(find_program(OsStr::new("emacs"), Some(&search)), None);
        assert_eq!(find_program(OsStr::new("vim"), None), None);
        assert_eq!(find_program(OsStr::new(""), Some(&search)), None);
        // A path is used as it is, wherever PATH points.
        assert_eq!(
            find_program(program.as_os_str(), None),
            Some(program.clone())
        );
        assert_eq!(find_program(text.as_os_str(), None), None);

        assert_eq!(program_name(ExternalEditor::Vim, None), "vim");
        assert_eq!(
            program_name(ExternalEditor::Vim, Some(&OsString::new())),
            "vim"
        );
        assert_eq!(
            program_name(ExternalEditor::Script, Some(&OsString::from("/opt/nvim"))),
            "/opt/nvim"
        );
    }

    #[test]
    fn the_verdict_is_one_known_word_or_nothing() {
        assert_eq!(Verdict::read("send\n"), Some(Verdict::Send));
        assert_eq!(Verdict::read("  postpone  \nmore"), Some(Verdict::Postpone));
        assert_eq!(Verdict::read("discard"), Some(Verdict::Discard));
        for other in ["", "\n", "Send", "send it", "delete"] {
            assert_eq!(Verdict::read(other), None, "{other:?}");
        }
        for verdict in [Verdict::Send, Verdict::Postpone, Verdict::Discard] {
            assert_eq!(Verdict::read(verdict.word()), Some(verdict));
        }
    }
}
