//! Hooks: the user's own programs, each given one message on its
//! standard input.
//!
//! A hook is named in `hooks.toml` beside the config, by an absolute path
//! and its arguments: an argv, never a line for a shell. It runs only when
//! the user picks it for the message they are on, and only in a terminal:
//! Quill's own app is sandboxed and launches nothing outside its bundle.
//!
//! The program is the user's; the message is not. So the program gets the
//! message as bytes on a pipe and a few of its headers as clean single
//! lines in the environment, and nothing of the message ever becomes part
//! of a command. It has no terminal, a small environment, a time limit,
//! and only the first line of what it prints is shown.

use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quill_store::MessageRow;

use crate::compose::written_address;
use crate::text;

/// The most hooks a file may list.
pub const MAX_HOOKS: usize = 50;
/// The most arguments one hook may have, the program included.
const MAX_ARGUMENTS: usize = 64;

/// What of the message a hook is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookInput {
    /// The message exactly as it arrived, headers and all.
    #[default]
    Raw,
    /// A few headers, a blank line, and the message as plain text.
    Text,
}

/// One program the user has set up to give messages to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// What the picker calls it.
    pub name: String,
    /// The program, by its absolute path, and its arguments.
    pub command: Vec<String>,
    pub input: HookInput,
}

impl Hook {
    /// The program and its arguments as the picker shows them: one clean
    /// line that says what will run.
    pub fn shown(&self) -> String {
        text::sanitize_line(&self.command.join(" "))
    }
}

/// Reads the text of a `hooks.toml`:
///
/// ```toml
/// [[hook]]
/// name = "Save to notes"
/// command = ["/usr/local/bin/save-note", "--from-mail"]
/// input = "text"   # or "raw", the default
/// ```
///
/// On any error no hook is loaded at all: a file that is half understood
/// is not one to run programs from.
pub fn parse(text: &str) -> Result<Vec<Hook>, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        #[serde(default)]
        hook: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry {
        name: String,
        command: Vec<String>,
        #[serde(default)]
        input: HookInput,
    }
    let file: File = toml::from_str(text).map_err(|err| {
        let line = err.span().map_or(1, |span| {
            text.as_bytes()[..span.start.min(text.len())]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count()
                + 1
        });
        format!("line {line}: {}", err.message())
    })?;
    if file.hook.len() > MAX_HOOKS {
        return Err(format!(
            "there are {} hooks, and the most is {MAX_HOOKS}",
            file.hook.len()
        ));
    }
    let mut hooks: Vec<Hook> = Vec::with_capacity(file.hook.len());
    for (index, entry) in file.hook.into_iter().enumerate() {
        let number = index + 1;
        let name = text::sanitize_line(&entry.name).trim().to_owned();
        if name.is_empty() {
            return Err(format!("hook {number} has no name"));
        }
        if hooks.iter().any(|hook| hook.name == name) {
            return Err(format!("two hooks are called \"{name}\""));
        }
        let Some(program) = entry.command.first() else {
            return Err(format!("\"{name}\" has no command"));
        };
        // Found by where it is, never by a search of PATH, and never
        // handed to a shell to make sense of.
        if !program.starts_with('/') {
            return Err(format!(
                "\"{name}\": the program must be an absolute path, such as /usr/local/bin/save-note"
            ));
        }
        if entry.command.len() > MAX_ARGUMENTS {
            return Err(format!(
                "\"{name}\" has more than {MAX_ARGUMENTS} arguments"
            ));
        }
        if entry.command.iter().any(|part| part.contains('\0')) {
            return Err(format!("\"{name}\": an argument has a NUL in it"));
        }
        hooks.push(Hook {
            name,
            command: entry.command,
            input: entry.input,
        });
    }
    Ok(hooks)
}

/// The variables that tell a hook about its message.
pub const SUBJECT_VARIABLE: &str = "QUILL_SUBJECT";
pub const FROM_VARIABLE: &str = "QUILL_FROM";
pub const MESSAGE_ID_VARIABLE: &str = "QUILL_MESSAGE_ID";

/// The longest a header is in a hook's environment, in bytes.
const MOST_VARIABLE_BYTES: usize = 1024;

/// A hook ready to run: the program, and the message it is given.
#[derive(Clone, PartialEq, Eq)]
pub struct HookRun {
    pub name: String,
    pub command: Vec<String>,
    /// What goes to the program's standard input.
    pub input: Vec<u8>,
    /// What the program is told about the message, besides: each a name
    /// and one clean line.
    pub about: Vec<(&'static str, String)>,
}

impl std::fmt::Debug for HookRun {
    // The message itself is not for a log or a failed assertion.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRun")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("input", &format_args!("{} bytes", self.input.len()))
            .field("about", &self.about)
            .finish()
    }
}

impl HookRun {
    /// Puts a hook together with its message. `input` is the message in
    /// the form the hook asked for.
    pub fn new(hook: &Hook, row: &MessageRow, input: Vec<u8>) -> Self {
        let from = row
            .headers
            .from
            .first()
            .map(|from| from.email.as_str())
            .unwrap_or_default();
        let about = [
            (SUBJECT_VARIABLE, row.headers.subject.as_str()),
            (FROM_VARIABLE, from),
            (
                MESSAGE_ID_VARIABLE,
                row.headers.message_id.as_deref().unwrap_or_default(),
            ),
        ]
        .into_iter()
        .map(|(name, value)| (name, one_line(value)))
        .collect();
        Self {
            name: hook.name.clone(),
            command: hook.command.clone(),
            input,
            about,
        }
    }
}

/// A header as an environment variable may hold it: one line, nothing a
/// terminal or a shell's `echo` would act on, and not too long.
fn one_line(value: &str) -> String {
    let mut line = text::sanitize_line(value).trim().to_owned();
    if line.len() > MOST_VARIABLE_BYTES {
        let mut end = MOST_VARIABLE_BYTES;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
    }
    line
}

/// A message as text for a hook: the headers a reader shows, a blank
/// line, and the body. Each header is one line whatever the message said.
pub fn as_text(row: &MessageRow, body: &str) -> String {
    let people = |list: &[quill_core::types::Address]| {
        list.iter()
            .map(|person| {
                written_address(person.name.as_deref().unwrap_or_default(), &person.email)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let date = jiff::Timestamp::from_second(row.headers.date)
        .ok()
        .and_then(|at| jiff::fmt::rfc2822::to_string(&at.to_zoned(jiff::tz::TimeZone::UTC)).ok())
        .unwrap_or_default();
    let mut out = String::new();
    for (name, value) in [
        ("From", people(&row.headers.from)),
        ("To", people(&row.headers.to)),
        ("Cc", people(&row.headers.cc)),
        ("Date", date),
        ("Subject", row.headers.subject.clone()),
        (
            "Message-ID",
            row.headers.message_id.clone().unwrap_or_default(),
        ),
    ] {
        let value = text::sanitize_line(&value);
        if value.trim().is_empty() {
            continue;
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value.trim());
        out.push('\n');
    }
    out.push('\n');
    out.push_str(body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// How long a hook may take, and how much of what it prints is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub time: Duration,
    /// The most kept of each of its standard output and error, in bytes.
    /// The rest is read and thrown away, so that a program which prints a
    /// great deal still finishes.
    pub output: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            time: Duration::from_secs(30),
            output: 64 * 1024,
        }
    }
}

/// How a hook ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEnd {
    /// It ran to its end, with this exit status.
    Exited(i32),
    /// Something else killed it.
    Killed,
    /// It took too long and was stopped.
    TimedOut,
    /// Quill was closing and stopped it.
    Stopped,
    /// It could not be started, and why.
    NotStarted(String),
    /// Programs are not run here: this is Quill's own app.
    NotAllowed,
}

/// How a hook ended and what it printed, as far as that was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutcome {
    pub end: HookEnd,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl HookOutcome {
    pub fn not_allowed() -> Self {
        Self {
            end: HookEnd::NotAllowed,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// The first line the program printed that has anything in it: of its
    /// output when it did well, of its complaints first when it did not.
    /// Made safe to draw, and short.
    pub fn first_line(&self) -> String {
        let first = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(|line| text::sanitize_line(line).trim().to_owned())
                .find(|line| !line.is_empty())
        };
        let line = if self.end == HookEnd::Exited(0) {
            first(&self.stdout).or_else(|| first(&self.stderr))
        } else {
            first(&self.stderr).or_else(|| first(&self.stdout))
        };
        text::truncate(&line.unwrap_or_default(), 160)
    }

    /// Whether it did what was asked of it.
    pub fn succeeded(&self) -> bool {
        self.end == HookEnd::Exited(0)
    }

    /// What to tell the user, in one line.
    pub fn said(&self, name: &str) -> String {
        let name = text::sanitize_line(name);
        let line = self.first_line();
        let with_line = |what: String| {
            if line.is_empty() {
                format!("{what}.")
            } else {
                format!("{what}: {line}")
            }
        };
        match &self.end {
            HookEnd::Exited(0) if line.is_empty() => format!("{name} is done."),
            HookEnd::Exited(0) => format!("{name}: {line}"),
            HookEnd::Exited(status) => with_line(format!("{name} failed (exit status {status})")),
            HookEnd::Killed => with_line(format!("{name} was killed")),
            HookEnd::TimedOut => {
                format!("{name} took too long and was stopped.")
            }
            HookEnd::Stopped => format!("{name} was stopped as Quill closed."),
            HookEnd::NotStarted(why) => {
                format!("{name} could not be started: {}", text::sanitize_line(why))
            }
            HookEnd::NotAllowed => {
                "Hooks run only when Quill is in a terminal: the app starts no other programs."
                    .to_owned()
            }
        }
    }
}

/// How often a running hook is looked in on.
const LOOK_EVERY: Duration = Duration::from_millis(20);
/// How long what a finished hook printed is waited for. Something it
/// started and left running may hold the pipe open for ever.
const OUTPUT_GRACE: Duration = Duration::from_millis(250);

/// The variables a hook inherits from Quill, when Quill has them.
const INHERITED: [&str; 3] = ["PATH", "HOME", "LANG"];
/// Where programs are looked for by a hook that inherits no `PATH`.
const DEFAULT_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Runs a hook and waits for it, for at most the time `limits` allow.
/// `stop` ends it early: Quill is closing. This blocks; it is never
/// called on the UI task or the store worker.
pub fn run(hook: &HookRun, limits: Limits, stop: &AtomicBool) -> HookOutcome {
    run_with(hook, limits, stop, |name| std::env::var_os(name))
}

fn run_with(
    hook: &HookRun,
    limits: Limits,
    stop: &AtomicBool,
    inherited: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> HookOutcome {
    let not_started = |why: String| HookOutcome {
        end: HookEnd::NotStarted(why),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let Some((program, arguments)) = hook.command.split_first() else {
        return not_started("it names no program".to_owned());
    };
    if !program.starts_with('/') {
        return not_started("its program is not an absolute path".to_owned());
    }
    let mut command = Command::new(program);
    command
        .args(arguments)
        // Nothing of Quill's own environment but where things are and
        // what language to speak.
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A group of its own: whatever it starts is stopped with it, and
        // a key meant for Quill never interrupts it.
        .process_group(0);
    for name in INHERITED {
        match inherited(name) {
            Some(value) if !value.is_empty() => {
                command.env(name, value);
            }
            _ if name == "PATH" => {
                command.env(name, DEFAULT_PATH);
            }
            _ => {}
        }
    }
    // Somewhere that exists and means something to the user.
    let home = inherited("HOME").filter(|home| std::path::Path::new(home).is_dir());
    command.current_dir(home.unwrap_or_else(|| "/".into()));
    for (name, value) in &hook.about {
        command.env(name, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return not_started(err.to_string()),
    };

    // The message goes in from a thread of its own, so that a program
    // which prints before it has read everything cannot stall both ends.
    if let Some(mut stdin) = child.stdin.take() {
        let input = hook.input.clone();
        std::thread::spawn(move || {
            // A program that does not read what it is given is its own
            // affair. The pipe closes when this ends.
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = collect(child.stdout.take(), limits.output);
    let stderr = collect(child.stderr.take(), limits.output);

    let deadline = Instant::now() + limits.time;
    let end = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break match status.code() {
                    Some(code) => HookEnd::Exited(code),
                    None if status.signal().is_some() => HookEnd::Killed,
                    None => HookEnd::Exited(-1),
                };
            }
            Ok(None) => {}
            Err(err) => {
                stop_group(&mut child);
                break HookEnd::NotStarted(err.to_string());
            }
        }
        if stop.load(Ordering::SeqCst) {
            stop_group(&mut child);
            break HookEnd::Stopped;
        }
        if Instant::now() >= deadline {
            stop_group(&mut child);
            break HookEnd::TimedOut;
        }
        std::thread::sleep(LOOK_EVERY);
    };
    HookOutcome {
        end,
        stdout: stdout.take(OUTPUT_GRACE),
        stderr: stderr.take(OUTPUT_GRACE),
    }
}

/// Kills a hook and everything in its group, and reaps it.
fn stop_group(child: &mut std::process::Child) {
    let group = rustix::process::Pid::from_child(child);
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// What a program has printed so far, read on a thread of its own.
struct Collected {
    kept: Arc<Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Collected {
    /// What was kept, waiting at most `grace` for the program's end of
    /// the pipe to close.
    fn take(self, grace: Duration) -> Vec<u8> {
        let _ = self.done.recv_timeout(grace);
        self.kept
            .lock()
            .map(|mut kept| std::mem::take(&mut *kept))
            .unwrap_or_default()
    }
}

/// Reads a pipe to its end, keeping the first `most` bytes.
fn collect(pipe: Option<impl Read + Send + 'static>, most: usize) -> Collected {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (finished, done) = std::sync::mpsc::channel();
    if let Some(mut pipe) = pipe {
        let kept = Arc::clone(&kept);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if let Ok(mut kept) = kept.lock() {
                            let room = most.saturating_sub(kept.len());
                            kept.extend_from_slice(&buffer[..read.min(room)]);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = finished.send(());
        });
    }
    Collected { kept, done }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use quill_core::types::{
        Address, Flags, Headers, LocalAccountId, LocalFolderId, LocalMessageId,
    };

    use super::*;

    fn row() -> MessageRow {
        MessageRow {
            id: LocalMessageId(7),
            folder: LocalFolderId(1),
            account: LocalAccountId(1),
            headers: Headers {
                message_id: Some("<q4@example.com>".to_owned()),
                subject: "Q4 roadmap\r\nBcc: mallory@example.com \u{1b}[31m".to_owned(),
                from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                to: vec![Address::new("Robin", "robin@example.com")],
                cc: Vec::new(),
                date: 1_794_648_600,
            },
            flags: Flags::default(),
            has_attachments: false,
            thread: 1,
            snoozed_until: None,
        }
    }

    /// Writes an executable `/bin/sh` script and returns where it is.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn hook_for(program: &Path, arguments: &[&str]) -> Hook {
        let mut command = vec![program.to_string_lossy().into_owned()];
        command.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        Hook {
            name: "Test hook".to_owned(),
            command,
            input: HookInput::Raw,
        }
    }

    fn quick() -> Limits {
        Limits {
            time: Duration::from_secs(20),
            output: 64 * 1024,
        }
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_hooks_file_lists_programs_by_absolute_path() {
        let hooks = parse(
            "[[hook]]\nname = \"Save to notes\"\ncommand = [\"/usr/local/bin/save-note\", \"--from-mail\"]\n\n\
             [[hook]]\nname = \"Word count\"\ncommand = [\"/usr/bin/wc\", \"-w\"]\ninput = \"text\"\n",
        )
        .unwrap();
        assert_eq!(
            hooks,
            [
                Hook {
                    name: "Save to notes".to_owned(),
                    command: vec![
                        "/usr/local/bin/save-note".to_owned(),
                        "--from-mail".to_owned()
                    ],
                    input: HookInput::Raw,
                },
                Hook {
                    name: "Word count".to_owned(),
                    command: vec!["/usr/bin/wc".to_owned(), "-w".to_owned()],
                    input: HookInput::Text,
                },
            ]
        );
        assert_eq!(hooks[0].shown(), "/usr/local/bin/save-note --from-mail");
        // No file's worth of hooks is no hooks.
        assert_eq!(parse(""), Ok(Vec::new()));
        assert_eq!(parse("# nothing yet\n"), Ok(Vec::new()));
    }

    #[test]
    fn a_bad_hooks_file_loads_nothing_and_says_why() {
        let one = |entry: &str| parse(&format!("[[hook]]\n{entry}\n")).unwrap_err();
        // Found by a search of PATH, or through a shell: neither.
        for command in [
            "[\"save-note\"]",
            "[\"./save-note\"]",
            "[\"~/bin/save-note\"]",
        ] {
            let problem = one(&format!("name = \"Notes\"\ncommand = {command}"));
            assert!(problem.contains("must be an absolute path"), "{problem}");
        }
        // A shell line is a string, and a command is a list.
        let problem = one("name = \"Notes\"\ncommand = \"/bin/sh -c 'rm -rf ~'\"");
        assert!(problem.starts_with("line 3: "), "{problem}");
        assert!(one("name = \"Notes\"\ncommand = []").contains("has no command"));
        assert!(one("name = \" \"\ncommand = [\"/bin/cat\"]").contains("has no name"));
        assert!(one("command = [\"/bin/cat\"]").contains("name"));
        assert!(
            one("name = \"Notes\"\ncommand = [\"/bin/cat\"]\ninput = \"html\"").contains("line 4")
        );
        assert!(one("name = \"Notes\"\ncommand = [\"/bin/cat\"]\nshell = true").contains("shell"));
        assert!(one("name = \"Notes\"\ncommand = [\"/bin/cat\", \"a\\u0000b\"]").contains("NUL"));
        // One bad hook spoils the file: the good one before it is not run.
        let problem = parse(
            "[[hook]]\nname = \"Good\"\ncommand = [\"/bin/cat\"]\n\
             [[hook]]\nname = \"Good\"\ncommand = [\"/bin/cat\"]\n",
        )
        .unwrap_err();
        assert_eq!(problem, "two hooks are called \"Good\"");
        assert!(parse("[[hooks]]\nname = \"x\"\n").is_err());
        assert!(parse("[[hook]\n").unwrap_err().starts_with("line 1: "));
    }

    #[test]
    fn too_many_hooks_are_refused() {
        let many = |count: usize| -> String {
            (0..count)
                .map(|number| {
                    format!("[[hook]]\nname = \"Hook {number}\"\ncommand = [\"/bin/cat\"]\n")
                })
                .collect()
        };
        assert_eq!(parse(&many(MAX_HOOKS)).unwrap().len(), MAX_HOOKS);
        assert_eq!(
            parse(&many(MAX_HOOKS + 1)).unwrap_err(),
            "there are 51 hooks, and the most is 50"
        );
    }

    #[test]
    fn a_name_is_made_safe_to_show() {
        let hooks =
            parse("[[hook]]\nname = \"Save\\u001b[2J\\nnow\"\ncommand = [\"/bin/cat\"]\n").unwrap();
        assert_eq!(hooks[0].name, "Save now");
    }

    #[test]
    fn what_a_hook_is_told_about_a_message_is_clean_single_lines() {
        let run = HookRun::new(
            &hook_for(Path::new("/bin/cat"), &[]),
            &row(),
            b"raw".to_vec(),
        );
        assert_eq!(
            run.about,
            [
                (
                    SUBJECT_VARIABLE,
                    "Q4 roadmap Bcc: mallory@example.com".to_owned()
                ),
                (FROM_VARIABLE, "ana.ruiz@example.com".to_owned()),
                (MESSAGE_ID_VARIABLE, "<q4@example.com>".to_owned()),
            ]
        );
        // A header of any length is cut to something an environment holds.
        let mut long = row();
        long.headers.subject = "é".repeat(2000);
        let run = HookRun::new(&hook_for(Path::new("/bin/cat"), &[]), &long, Vec::new());
        assert!(run.about[0].1.len() <= 1024 && run.about[0].1.ends_with('é'));
        // The message is not printed with it.
        assert!(!format!("{run:?}").contains("raw"));
    }

    #[test]
    fn a_message_as_text_is_its_headers_then_its_body() {
        let text = as_text(&row(), "Hi Robin,\n\nHere it is.");
        assert_eq!(
            text,
            "From: Ana Ruiz <ana.ruiz@example.com>\n\
             To: Robin <robin@example.com>\n\
             Date: Sat, 14 Nov 2026 09:30:00 +0000\n\
             Subject: Q4 roadmap Bcc: mallory@example.com\n\
             Message-ID: <q4@example.com>\n\
             \n\
             Hi Robin,\n\nHere it is.\n"
        );
    }

    #[test]
    fn the_message_reaches_the_program_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("copy");
        let program = script(dir.path(), "keep", "cat > \"$1\"\necho kept");
        // Every byte there is, and enough of them to fill a pipe many
        // times over.
        let message: Vec<u8> = (0..300_000u32).map(|n| (n % 256) as u8).collect();
        let run = HookRun::new(
            &hook_for(&program, &[copy.to_str().unwrap()]),
            &row(),
            message.clone(),
        );
        let outcome = super::run(&run, quick(), &never());
        assert_eq!(outcome.end, HookEnd::Exited(0), "{outcome:?}");
        assert_eq!(std::fs::read(&copy).unwrap(), message);
        assert_eq!(outcome.said("Keep"), "Keep: kept");
    }

    #[test]
    fn the_program_sees_a_small_environment_and_no_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "look",
            "env | sort\nif [ -t 0 ] || [ -t 1 ] || [ -t 2 ]; then echo HAS-A-TERMINAL; fi\npwd",
        );
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let home = dir.path().to_path_buf();
        let outcome = run_with(&run, quick(), &never(), |name| match name {
            "PATH" => Some("/usr/bin:/bin".into()),
            "HOME" => Some(home.clone().into()),
            "LANG" => Some("en_GB.UTF-8".into()),
            _ => panic!("asked for {name}"),
        });
        assert_eq!(outcome.end, HookEnd::Exited(0), "{outcome:?}");
        let printed = String::from_utf8(outcome.stdout).unwrap();
        let mut names: Vec<&str> = printed
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            // What the shell itself sets as it starts.
            .filter(|name| !matches!(*name, "PWD" | "SHLVL" | "_" | "OLDPWD"))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "HOME",
                "LANG",
                "PATH",
                "QUILL_FROM",
                "QUILL_MESSAGE_ID",
                "QUILL_SUBJECT"
            ],
            "{printed}"
        );
        assert!(printed.contains("QUILL_FROM=ana.ruiz@example.com\n"));
        assert!(printed.contains("QUILL_SUBJECT=Q4 roadmap Bcc: mallory@example.com\n"));
        assert!(!printed.contains("HAS-A-TERMINAL"), "{printed}");
        // It starts in the user's home folder.
        let home = std::fs::canonicalize(&home).unwrap();
        assert_eq!(
            printed.lines().last().map(Path::new),
            Some(home.as_path()),
            "{printed}"
        );
    }

    #[test]
    fn with_no_path_of_quills_own_a_plain_one_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "path", "echo \"$PATH\"\necho \"home=$HOME\"");
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let outcome = run_with(&run, quick(), &never(), |_| None);
        assert_eq!(
            String::from_utf8(outcome.stdout).unwrap(),
            "/usr/bin:/bin:/usr/sbin:/sbin\nhome=\n"
        );
    }

    #[test]
    fn what_it_prints_is_kept_up_to_a_limit_and_the_rest_thrown_away() {
        let dir = tempfile::tempdir().unwrap();
        // A megabyte to each of output and error, then a clean exit.
        let program = script(
            dir.path(),
            "loud",
            "head -c 1000000 /dev/zero | tr '\\0' 'o'\nhead -c 1000000 /dev/zero | tr '\\0' 'e' >&2",
        );
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let limits = Limits {
            output: 64 * 1024,
            ..quick()
        };
        let outcome = super::run(&run, limits, &never());
        // It was not left blocked on a full pipe: it finished.
        assert_eq!(outcome.end, HookEnd::Exited(0), "{:?}", outcome.end);
        assert_eq!(outcome.stdout.len(), 64 * 1024);
        assert_eq!(outcome.stderr.len(), 64 * 1024);
        assert!(outcome.stdout.iter().all(|byte| *byte == b'o'));
        // And what is shown of it is one short line.
        assert_eq!(text::width(&outcome.first_line()), 160);
    }

    #[test]
    fn a_program_that_takes_too_long_is_killed_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("survived");
        // The sleep is a child of the script: killing the script alone
        // would leave it running, and holding the pipes open.
        let program = script(
            dir.path(),
            "slow",
            &format!("echo started\nsleep 5\ntouch '{}'", mark.display()),
        );
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let limits = Limits {
            time: Duration::from_millis(400),
            ..quick()
        };
        let began = Instant::now();
        let outcome = super::run(&run, limits, &never());
        assert_eq!(outcome.end, HookEnd::TimedOut);
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(outcome.stdout, b"started\n");
        assert_eq!(outcome.said("Slow"), "Slow took too long and was stopped.");
        // Nothing of it lived on to finish the job.
        std::thread::sleep(Duration::from_millis(300));
        assert!(!mark.exists());
    }

    #[test]
    fn a_failure_is_reported_with_its_status_and_first_complaint() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "fail",
            "echo to-stdout\necho >&2\nprintf 'no such \\033[31mnotebook\\033[0m\\nsecond line\\n' >&2\nexit 3",
        );
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let outcome = super::run(&run, quick(), &never());
        assert_eq!(outcome.end, HookEnd::Exited(3));
        assert!(!outcome.succeeded());
        // The complaint, not the output; made safe to draw.
        assert_eq!(
            outcome.said("Save to notes"),
            "Save to notes failed (exit status 3): no such notebook"
        );

        // A program that says nothing still has its status reported.
        let quiet = script(dir.path(), "quiet", "exit 1");
        let run = HookRun::new(&hook_for(&quiet, &[]), &row(), Vec::new());
        assert_eq!(
            super::run(&run, quick(), &never()).said("Quiet"),
            "Quiet failed (exit status 1)."
        );
        let fine = script(dir.path(), "fine", "exit 0");
        let run = HookRun::new(&hook_for(&fine, &[]), &row(), Vec::new());
        let outcome = super::run(&run, quick(), &never());
        assert!(outcome.succeeded());
        assert_eq!(outcome.said("Fine"), "Fine is done.");
    }

    #[test]
    fn a_program_that_is_not_there_or_not_named_by_path_is_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nothing-here");
        let run = HookRun::new(&hook_for(&missing, &[]), &row(), Vec::new());
        let outcome = super::run(&run, quick(), &never());
        assert!(matches!(outcome.end, HookEnd::NotStarted(_)), "{outcome:?}");
        assert!(
            outcome
                .said("Gone")
                .starts_with("Gone could not be started: ")
        );

        // Whatever the file said, nothing is ever looked up on PATH here.
        let mut run = HookRun::new(&hook_for(Path::new("/bin/sh"), &[]), &row(), Vec::new());
        run.command = vec!["sh".to_owned()];
        assert_eq!(
            super::run(&run, quick(), &never()).end,
            HookEnd::NotStarted("its program is not an absolute path".to_owned())
        );
        run.command.clear();
        assert!(matches!(
            super::run(&run, quick(), &never()).end,
            HookEnd::NotStarted(_)
        ));
    }

    #[test]
    fn a_program_that_ignores_its_input_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "deaf", "echo did not read");
        let run = HookRun::new(&hook_for(&program, &[]), &row(), vec![b'x'; 1_000_000]);
        let outcome = super::run(&run, quick(), &never());
        assert_eq!(outcome.end, HookEnd::Exited(0));
        assert_eq!(outcome.first_line(), "did not read");
    }

    #[test]
    fn something_left_running_does_not_hold_the_answer_up() {
        let dir = tempfile::tempdir().unwrap();
        // The script ends at once; what it started keeps its pipes.
        let program = script(dir.path(), "leaves", "echo begun\nsleep 3 &\nexit 0");
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let began = Instant::now();
        let outcome = super::run(&run, quick(), &never());
        assert_eq!(outcome.end, HookEnd::Exited(0));
        assert_eq!(outcome.stdout, b"begun\n");
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn closing_quill_stops_a_running_hook() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "slow", "sleep 10");
        let run = HookRun::new(&hook_for(&program, &[]), &row(), Vec::new());
        let stop = Arc::new(AtomicBool::new(false));
        let closing = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            closing.store(true, Ordering::SeqCst);
        });
        let began = Instant::now();
        let outcome = super::run(&run, quick(), &stop);
        assert_eq!(outcome.end, HookEnd::Stopped);
        assert!(began.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_app_runs_nothing_and_says_so() {
        let outcome = HookOutcome::not_allowed();
        assert!(!outcome.succeeded());
        assert!(
            outcome
                .said("Save")
                .contains("only when Quill is in a terminal")
        );
    }
}
