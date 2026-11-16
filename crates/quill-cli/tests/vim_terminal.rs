//! `quill tui` in a terminal of its own, handing that terminal to an
//! editor and taking it back.
//!
//! macOS's `script` makes the terminal. The editor is a small script
//! standing in for Vim (`QUILL_VIM`): it records how it was started,
//! rewrites the draft file, and leaves. The mailbox is the demo one, and
//! the config and data folders are scratch ones: nothing of the user's is
//! read or written.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SCRIPT: &str = "/usr/bin/script";
const PATIENCE: Duration = Duration::from_secs(30);

/// Quill running in a terminal that this test is the other end of.
struct Session {
    child: Child,
    keys: Arc<Mutex<ChildStdin>>,
    /// Everything the terminal has been sent so far.
    seen: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    /// Starts `quill tui --demo` with `vim` as its Vim, or with `None`
    /// whatever `vim` is on `PATH`. Returns `None` when this Mac has no
    /// `script` to make a terminal with.
    fn start(scratch: &Path, vim: Option<&Path>, config: &str) -> Option<Self> {
        if !Path::new(SCRIPT).is_file() {
            eprintln!("skipped: there is no {SCRIPT} to make a terminal with");
            return None;
        }
        let config_dir = scratch.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), config).unwrap();
        // A home of its own: no vimrc of the user's is read, and nothing
        // an editor keeps between runs is written into theirs.
        let home = scratch.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mut command = Command::new(SCRIPT);
        match vim {
            Some(vim) => command.env("QUILL_VIM", vim),
            None => command.env_remove("QUILL_VIM"),
        };
        let mut child = command
            .args(["-q", "/dev/null", "/bin/sh", "-c"])
            // A size to draw in, then Quill itself: its arguments are
            // passed on as they are, not written into the line.
            .arg("stty rows 40 columns 140 && exec \"$@\"")
            .arg("sh")
            .arg(env!("CARGO_BIN_EXE_quill"))
            .args(["tui", "--demo", "--config-dir"])
            .arg(&config_dir)
            .arg("--data-dir")
            .arg(scratch.join("data"))
            .env("TERM", "xterm-256color")
            .env("HOME", &home)
            .env("STANDIN_LOG", scratch.join("log"))
            .env_remove("VIMINIT")
            .env_remove("EXINIT")
            .env_remove("MYVIMRC")
            .env_remove("QUILL_SHELL_SOCKET")
            .env_remove("QUILL_CONFIG_DIR")
            .env_remove("QUILL_DATA_DIR")
            .env_remove("NO_COLOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let keys = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut output = child.stdout.take().unwrap();
        let (answer, record) = (Arc::clone(&keys), Arc::clone(&seen));
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            while let Ok(read) = output.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let chunk = &buffer[..read];
                record.lock().unwrap().extend_from_slice(chunk);
                // What every terminal says when asked what it is. Quill
                // asks at the start, and again each time it takes the
                // terminal back.
                if chunk.windows(3).any(|window| window == b"\x1b[c") {
                    let mut keys = answer.lock().unwrap();
                    let _ = keys.write_all(b"\x1b[?62;c");
                    let _ = keys.flush();
                }
            }
        });
        Some(Self { child, keys, seen })
    }

    fn press(&self, keys: &[u8]) {
        let mut stdin = self.keys.lock().unwrap();
        stdin.write_all(keys).unwrap();
        stdin.flush().unwrap();
    }

    fn seen(&self) -> Vec<u8> {
        self.seen.lock().unwrap().clone()
    }

    /// How much has been seen so far: a place to look on from.
    fn mark(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// Waits until `needle` has been drawn after `from`.
    fn wait_for(&self, needle: &str, from: usize) {
        let began = Instant::now();
        loop {
            let seen = self.seen();
            if drawn(&seen[from.min(seen.len())..], needle) {
                return;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "never saw {needle:?}; the terminal was sent:\n{}",
                String::from_utf8_lossy(&seen[from.min(seen.len())..])
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Quits from the message list and returns Quill's exit status.
    fn quit(mut self) -> Option<i32> {
        self.press(b"q");
        let began = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            if began.elapsed() > PATIENCE {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("Quill did not quit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where `needle` first is in `haystack`.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Whether `words` were drawn in what the terminal was sent. A screen is
/// drawn as the cells that changed, with moves of the cursor between
/// them, so the words are looked for with the moves and the gaps taken
/// out.
fn drawn(sent: &[u8], words: &str) -> bool {
    let text = String::from_utf8_lossy(sent);
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // An escape sequence: `ESC [`, its parameters, a final letter.
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_whitespace() {
            plain.push(c);
        }
    }
    let words: String = words.chars().filter(|c| !c.is_whitespace()).collect();
    plain.contains(&words)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// What the stand-in prints, straight to the terminal it was given.
const STAND_IN_SAYS: &str = "THE STAND-IN HAS THE TERMINAL";
const LEAVE_SCREEN: &[u8] = b"\x1b[?1049l";
const ENTER_SCREEN: &[u8] = b"\x1b[?1049h";

/// A program to run in Vim's place. It writes how it was started to
/// `$STANDIN_LOG`, puts `draft` in the draft file, says `verdict` (when
/// there is one), and exits with `status`.
fn stand_in(scratch: &Path, draft: &str, verdict: Option<&str>, status: i32) -> PathBuf {
    let path = scratch.join("stand-in-vim");
    let verdict = verdict.map_or_else(String::new, |word| {
        format!("echo {word} > \"$QUILL_DRAFT_DIR/verdict\"\n")
    });
    let body = format!(
        "#!/bin/sh\n\
         if [ -t 0 ] && [ -t 1 ]; then terminal=yes; else terminal=no; fi\n\
         {{\n\
           for argument in \"$@\"; do echo \"arg=$argument\"; done\n\
           echo \"dir=$QUILL_DRAFT_DIR\"\n\
           echo \"pwd=$(pwd -P)\"\n\
           echo \"terminal=$terminal\"\n\
           stty -a\n\
           ls \"$QUILL_DRAFT_DIR\"\n\
         }} > \"$STANDIN_LOG\" 2>&1\n\
         for last; do :; done\n\
         cat > \"$last\" <<'DRAFT'\n{draft}DRAFT\n\
         {verdict}\
         echo '{STAND_IN_SAYS}'\n\
         exit {status}\n"
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

const WRITTEN: &str = "To: ana.ruiz@example.com\n\
                       Subject: From the stand-in\n\
                       \n\
                       Written outside Quill.\n";

#[test]
fn the_terminal_goes_to_the_editor_and_comes_back_whole() {
    let scratch = tempfile::tempdir().unwrap();
    let vim = stand_in(scratch.path(), WRITTEN, None, 0);
    let Some(quill) = Session::start(scratch.path(), Some(&vim), "") else {
        return;
    };
    quill.wait_for("Q4 roadmap", 0);
    quill.press(b"c");
    quill.wait_for("New message", 0);
    let before = quill.mark();
    // ctrl-v: write in Vim.
    quill.press(b"\x16");
    quill.wait_for(STAND_IN_SAYS, before);

    // It was given the terminal as a shell would have it: Quill's screen
    // put away first, and its modes undone.
    let seen = quill.seen();
    let handed_over = position(&seen, LEAVE_SCREEN).expect("the alternate screen was never left");
    let ran = position(&seen, STAND_IN_SAYS.as_bytes()).unwrap();
    assert!(handed_over < ran);

    // Then Quill takes it back, draws everything again, and shows the
    // message as the file left it: open, to send, save or close.
    quill.wait_for("From the stand-in", ran);
    quill.wait_for("Written outside Quill.", ran);
    quill.wait_for("Back from Vim.", ran);
    let after = &quill.seen()[ran..];
    assert_eq!(count(after, ENTER_SCREEN), 1);
    // A whole frame, not a patch: the other panes are drawn again too.
    assert!(drawn(after, "[1] Accounts") && drawn(after, "Q4 roadmap"));

    // How the editor was started, as it saw it.
    let log = std::fs::read_to_string(scratch.path().join("log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    let dir = lines
        .iter()
        .find_map(|line| line.strip_prefix("dir="))
        .unwrap();
    assert_eq!(
        &lines[..7],
        [
            "arg=-N",
            "arg=--cmd",
            "arg=augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
            "arg=-c",
            "arg=execute 'source' fnameescape($QUILL_DRAFT_DIR . '/quill.vim')",
            "arg=--",
            format!("arg={dir}/message.quill").as_str(),
        ],
        "{log}"
    );
    // In the draft's own folder, on the terminal itself, with the
    // terminal back in the state a program expects to find it in.
    let here = lines
        .iter()
        .find_map(|line| line.strip_prefix("pwd="))
        .unwrap();
    assert_eq!(
        Path::new(here),
        std::fs::canonicalize(Path::new(dir).parent().unwrap())
            .unwrap()
            .join(Path::new(dir).file_name().unwrap())
    );
    assert!(lines.contains(&"terminal=yes"), "{log}");
    assert!(log.contains(" icanon") && !log.contains("-icanon"), "{log}");
    assert!(log.contains(" echo") && !log.contains("-echo "), "{log}");
    for file in ["message.quill", "contacts", "quill.vim"] {
        assert!(lines.contains(&file), "{log}");
    }
    // And the folder went when the draft had been read back.
    assert!(!Path::new(dir).exists());

    // The keys are Quill's again: escape closes the message as a draft,
    // and q quits, leaving the terminal as it was found.
    let closing = quill.mark();
    quill.press(b"\x1b");
    quill.wait_for("Saved to Drafts.", closing);
    let seen = Arc::clone(&quill.seen);
    assert_eq!(quill.quit(), Some(0));
    // Entered twice and left twice: once each side of the editor. The
    // last of what it wrote may still be on its way here.
    let began = Instant::now();
    loop {
        let seen = seen.lock().unwrap().clone();
        let (entered, left) = (count(&seen, ENTER_SCREEN), count(&seen, LEAVE_SCREEN));
        if (entered, left) == (2, 2) {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "entered {entered} times and left {left}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn with_vim_as_the_editor_a_new_message_goes_there_and_is_sent_from_there() {
    let scratch = tempfile::tempdir().unwrap();
    let vim = stand_in(scratch.path(), WRITTEN, Some("send"), 0);
    let Some(quill) = Session::start(scratch.path(), Some(&vim), "[compose]\neditor = \"vim\"\n")
    else {
        return;
    };
    quill.wait_for("Q4 roadmap", 0);
    let before = quill.mark();
    // No key but the one that starts a message.
    quill.press(b"c");
    quill.wait_for(STAND_IN_SAYS, before);
    // Sent: it is in the outbox, which the sidebar now lists, and the
    // message is no longer open.
    quill.wait_for("Outbox", before);
    assert_eq!(quill.quit(), Some(0));
}

#[test]
fn an_editor_that_quits_with_an_error_changes_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    // As `:cq` after `:w`: written, and then never mind.
    let vim = stand_in(scratch.path(), WRITTEN, Some("send"), 1);
    let Some(quill) = Session::start(scratch.path(), Some(&vim), "") else {
        return;
    };
    quill.wait_for("Q4 roadmap", 0);
    quill.press(b"c");
    quill.wait_for("New message", 0);
    quill.press(b"robin@example.com");
    let before = quill.mark();
    quill.press(b"\x16");
    quill.wait_for(STAND_IN_SAYS, before);
    quill.wait_for("it is as it was", before);
    // What had been typed is still there, and what the editor wrote is not.
    let seen = quill.seen();
    let ran = position(&seen, STAND_IN_SAYS.as_bytes()).unwrap();
    let after = seen[ran..].to_vec();
    assert!(drawn(&after, "robin@example.com"));
    assert!(!drawn(&after, "From the stand-in"));
    assert!(!drawn(&after, "Outbox"));
    // It was saved as a draft before the editor ever started, so closing
    // it has nothing new to save: the reader is simply back.
    let closing = quill.mark();
    quill.press(b"\x1b");
    quill.wait_for("[3] Reader", closing);
    assert_eq!(quill.quit(), Some(0));
}

#[test]
fn with_no_vim_the_terminal_is_never_given_up() {
    let scratch = tempfile::tempdir().unwrap();
    let missing = scratch.path().join("no-such-editor");
    let Some(quill) = Session::start(scratch.path(), Some(&missing), "") else {
        return;
    };
    quill.wait_for("Q4 roadmap", 0);
    quill.press(b"c");
    quill.wait_for("New message", 0);
    let before = quill.mark();
    quill.press(b"\x16");
    quill.wait_for("Vim could not be started", before);
    // The screen stayed Quill's throughout.
    assert_eq!(count(&quill.seen(), LEAVE_SCREEN), 0);
    // And the message is still there to write: the next key is typed.
    let typing = quill.mark();
    quill.press(b"x");
    quill.wait_for("x", typing);
    quill.press(b"\x1b");
    quill.wait_for("Saved to Drafts.", typing);
    assert_eq!(quill.quit(), Some(0));
}

/// The whole way round with the Vim on this machine: Quill starts it on
/// the terminal, it loads the plugin from the draft's folder, takes keys
/// as a person would type them, and `:QuillSend` sends the message.
#[test]
fn the_real_vim_writes_and_sends_a_message_from_quills_terminal() {
    let on_path = quill_tui::vim::find_program(
        std::ffi::OsStr::new("vim"),
        std::env::var_os("PATH").as_deref(),
    );
    if on_path.is_none() {
        eprintln!("skipped: there is no `vim` on PATH");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let Some(quill) = Session::start(scratch.path(), None, "") else {
        return;
    };
    quill.wait_for("Q4 roadmap", 0);
    quill.press(b"c");
    quill.wait_for("New message", 0);
    let before = quill.mark();
    quill.press(b"\x16");
    // Vim has drawn the draft: its From line is not how Quill shows it.
    quill.wait_for("From: Work <robin@example.com>", before);
    // The plugin starts on the empty To line, ready to type.
    std::thread::sleep(Duration::from_millis(300));
    quill.press(b"ana.ruiz@example.com");
    std::thread::sleep(Duration::from_millis(200));
    quill.press(b"\x1b");
    // A pause, so that the escape is a key by itself.
    std::thread::sleep(Duration::from_millis(400));
    quill.press(b":call setline(5, 'Subject: From a real Vim')\r");
    quill.press(b":call append(line('$'), 'Typed in Vim itself.')\r");
    std::thread::sleep(Duration::from_millis(200));
    quill.press(b":QuillSend\r");
    // Quill has the terminal back, and the message is in the outbox.
    quill.wait_for("Outbox", before);
    let seen = quill.seen();
    let left = position(&seen, LEAVE_SCREEN).unwrap();
    assert!(count(&seen[left..], ENTER_SCREEN) >= 1);
    assert_eq!(quill.quit(), Some(0));
}
