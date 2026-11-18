//! The built binary, run as a user would run it. These tests have no
//! terminal, so they cover everything up to the point the UI would start.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Command, Stdio};

fn quill() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quill"))
}

#[test]
fn help_lists_the_tui_subcommand() {
    let output = quill().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("tui"), "{stdout}");
    let output = quill().args(["tui", "--help"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("--demo") && stdout.contains("QUILL_CONFIG_DIR"),
        "{stdout}"
    );
}

#[test]
fn no_subcommand_is_a_usage_error() {
    let output = quill().output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn without_a_terminal_tui_fails_cleanly_and_touches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let output = quill()
        .args(["tui"])
        .env("QUILL_CONFIG_DIR", dir.path().join("config"))
        .env("QUILL_DATA_DIR", dir.path().join("data"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("needs a terminal"), "{stderr}");
    // No escape sequences leaked, and no database was created.
    assert!(output.stdout.is_empty());
    assert!(!dir.path().join("data").exists());
}

#[test]
fn looking_at_mail_that_is_not_there_says_so_and_makes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["list"],
        vec!["search", "anything"],
        vec!["read", "1"],
        vec!["send", "--to", "ana.ruiz@example.com"],
    ] {
        let output = quill()
            .args(&args)
            .env("QUILL_CONFIG_DIR", dir.path().join("config"))
            .env("QUILL_DATA_DIR", dir.path().join("data"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("there is no mail in"), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty());
    }
    assert!(!dir.path().join("data").exists());
}

#[test]
fn the_headless_commands_are_in_the_help() {
    let output = quill().arg("--help").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in [
        "list", "search", "read", "send", "sync", "account", "move", "delete", "restore", "undo",
        "log", "drafts", "outbox", "mcp",
    ] {
        assert!(stdout.contains(command), "{command}: {stdout}");
    }
    let output = quill().args(["list", "--help"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("--json") && stdout.contains("--in"),
        "{stdout}"
    );
}

#[test]
fn a_theme_is_listed_and_imported_from_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let theme = |args: &[&str]| {
        quill()
            .arg("theme")
            .args(args)
            .env("QUILL_CONFIG_DIR", dir.path().join("config"))
            .env("QUILL_DATA_DIR", dir.path().join("data"))
            // A home with no Ghostty of its own in it.
            .env("HOME", dir.path().join("home"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let output = quill().args(["theme", "--help"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("list") && stdout.contains("import"),
        "{stdout}"
    );

    // With nothing of the user's own: the three Quill comes with.
    let output = theme(&["list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let names: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(names, ["dark", "light", "terminal"], "{stdout}");
    // Listing makes nothing.
    assert!(!dir.path().join("config").exists());

    // A theme file written here, in Ghostty's format, by its path.
    let source = dir.path().join("Harbour Fog");
    std::fs::write(
        &source,
        "background = #e8ecef\nforeground = #2c3a47\npalette = 4=#2a5db0\n",
    )
    .unwrap();
    let output = theme(&["import", source.to_str().unwrap()]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("Imported \"Harbour Fog\"")
            && stdout.contains("It is a light theme.")
            && stdout.contains("    theme_light = \"Harbour Fog\""),
        "{stdout}"
    );
    let copy = dir.path().join("config/themes/Harbour Fog");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&source).unwrap()
    );
    // No settings file was written: selecting it is the user's to do.
    assert!(!dir.path().join("config/config.toml").exists());

    let output = theme(&["list", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let themes = listed["themes"].as_array().unwrap();
    assert_eq!(themes.len(), 4);
    assert_eq!(themes[3]["name"], "Harbour Fog");
    assert_eq!(themes[3]["appearance"], "light");

    // What is not a theme is refused, and nothing is copied.
    let notes = dir.path().join("notes");
    std::fs::write(&notes, "Remember the milk.\n").unwrap();
    for what in [notes.to_str().unwrap(), "No Such Theme Anywhere 7f3a"] {
        let output = theme(&["import", what]);
        assert!(!output.status.success(), "{what}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with("quill: "), "{stderr}");
    }
    assert!(!dir.path().join("config/themes/notes").exists());
}

/// A mailbox on disk with the demo's mail in it, and a settings folder
/// beside it.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let settings = dir.path().join("config");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(settings.join("config.toml"), config).unwrap();
        let mut store = quill_store::Store::open(&data.join("quill.db"))
            .unwrap()
            .with_blobs(quill_store::BlobStore::new(data.join("blobs")));
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(quill_tui::demo::seed(&mut store, 1_794_648_600))
            .unwrap();
        Self { dir }
    }

    /// Runs `quill` on this mailbox. Where the mailbox is goes in the
    /// environment, not after the arguments: a test of how arguments are
    /// read must never be one flag away from the mail of whoever runs it.
    fn run(&self, args: &[&str]) -> std::process::Output {
        quill()
            .args(args)
            .env("QUILL_CONFIG_DIR", self.dir.path().join("config"))
            .env("QUILL_DATA_DIR", self.dir.path().join("data"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    /// Runs a command that is to work, with `--json`, and reads what it
    /// printed.
    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut args = args.to_vec();
        args.push("--json");
        let output = self.run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn mail_is_found_changed_and_put_back_from_a_shell() {
    let scratch = Scratch::new("");
    let found = scratch.json(&["search", "from:peter", "dinner"]);
    assert_eq!(found["schema"], 1);
    let id = found["messages"][0]["id"].as_i64().unwrap().to_string();
    assert_eq!(found["messages"][0]["subject"], "Sunday dinner");
    assert_eq!(scratch.json(&["count", "from:peter", "dinner"])["count"], 1);

    // Looked at first: nothing changes.
    let dry = scratch.json(&["delete", "--where", "from:peter dinner", "--dry-run"]);
    assert_eq!(dry["would_change"], 1);
    assert_eq!(scratch.json(&["read", &id])["folder"], "Inbox");

    let deleted = scratch.json(&["delete", &id]);
    assert_eq!(deleted["changed"], 1);
    assert_eq!(scratch.json(&["read", &id])["folder"], "Trash");

    let log = scratch.json(&["log"]);
    assert_eq!(log["entries"][0]["action"], "delete");
    assert_eq!(log["entries"][0]["entry"], deleted["log"]);

    let undone = scratch.json(&["undo"]);
    assert_eq!(undone["entry"], deleted["log"]);
    assert_eq!(scratch.json(&["read", &id])["folder"], "Inbox");

    // For reading: a line that says what was done.
    let said = scratch.run(&["mark", "starred", &id]);
    assert!(said.status.success());
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "Marked 1 message starred."
    );
    let said = scratch.run(&["archive", "--where", "from:peter dinner"]);
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "Archived 1 message."
    );
    let folders = scratch.json(&["folders", "--account", "personal"]);
    assert!(
        folders["folders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|folder| folder["role"] == "archive" && folder["messages"] != 0)
    );
}

#[test]
fn what_the_settings_refuse_exits_with_a_status_of_its_own() {
    // Sending is off as Quill comes.
    let scratch = Scratch::new("");
    let sent = scratch.run(&["send", "--to", "ana.ruiz@example.com", "--subject", "Hello"]);
    assert_eq!(sent.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&sent.stderr).contains("Sending is switched off"),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    // Nothing named is a mistake in the command.
    let nothing = scratch.run(&["delete"]);
    assert_eq!(nothing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&nothing.stderr).contains("Say which messages"));
    // What is not there is a plain failure.
    assert_eq!(scratch.run(&["read", "999999"]).status.code(), Some(1));

    let careful = Scratch::new("[agent]\ndelete = false\norganize = false\n");
    let found = careful.json(&["search", "from:peter", "dinner"]);
    let id = found["messages"][0]["id"].as_i64().unwrap().to_string();
    let refused = careful.run(&["delete", &id, "--json"]);
    assert_eq!(refused.status.code(), Some(3));
    let value: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(value["refused"]["code"], "switched_off");
    assert_eq!(careful.run(&["archive", &id]).status.code(), Some(3));
    assert_eq!(careful.json(&["read", &id])["folder"], "Inbox");
    // The attempt is in the log, with what was looked at.
    let log = careful.json(&["log", "--all"]);
    assert!(
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "delete"
                && entry["summary"].as_str().unwrap().starts_with("refused"))
    );

    // With the commands themselves switched off, nothing is looked at
    // either, and nothing is synced.
    let closed = Scratch::new("[agent]\ncommands = false\n");
    assert_eq!(closed.run(&["list"]).status.code(), Some(3));
    assert_eq!(closed.run(&["sync"]).status.code(), Some(3));
    // A settings file with a mistake in it is no licence.
    let broken = Scratch::new("[agent]\nbulk_limit = 0\n");
    assert_eq!(broken.run(&["list"]).status.code(), Some(1));
}

#[test]
fn a_search_may_start_with_a_dash_and_what_follows_it_is_still_read_as_flags() {
    let scratch = Scratch::new("");
    let subjects = |value: &serde_json::Value| -> Vec<String> {
        value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["subject"].as_str().unwrap().to_owned())
            .collect()
    };
    let all = scratch.json(&["search", "dinner"]);
    assert!(subjects(&all).contains(&"Sunday dinner".to_owned()));
    // Not from Peter: typed plainly when it comes first, and inside
    // quotes with the rest when it does not. Flags are flags wherever
    // they stand.
    for args in [
        vec!["search", "-from:peter", "newer:36500d"],
        vec!["search", "newer:36500d -from:peter"],
        vec!["search", "--limit", "40", "-from:peter", "newer:36500d"],
        vec!["search", "-from:peter newer:36500d", "--limit", "40"],
    ] {
        let found = scratch.json(&args);
        assert!(
            !subjects(&found).contains(&"Sunday dinner".to_owned()),
            "{args:?}"
        );
        assert!(!subjects(&found).is_empty(), "{args:?}");
    }
    // Loose, after other words, it is not guessed at: the command is
    // refused, and says how to write it.
    let loose = scratch.run(&["search", "newer:36500d", "-from:peter", "--json"]);
    assert_eq!(loose.status.code(), Some(2));
    // `-has:attachment` begins like the flag for help, and is a search.
    let without = scratch.json(&["count", "-has:attachment"]);
    let with = scratch.json(&["count", "has:attachment"]);
    assert!(without["count"].as_u64().unwrap() > 0 && with["count"].as_u64().unwrap() > 0);
    // What comes after the words is a flag, not one more word.
    let two = scratch.json(&["search", "-from:peter", "newer:36500d", "--limit", "2"]);
    assert_eq!(two["messages"].as_array().unwrap().len(), 2);
    assert_eq!(two["more"], true);
    // And so for the search a change is given.
    let dry = scratch.json(&["archive", "--where", "-from:peter dinner", "--dry-run"]);
    assert_eq!(dry["changed"], 0);
    assert!(dry["would_change"].is_u64());

    // Where the mail is, said after a search, is where it is looked for:
    // with the environment pointing nowhere, only the flags can have
    // found this mailbox.
    let nowhere = scratch.dir.path().join("nowhere");
    let output = quill()
        .args(["search", "-from:peter", "dinner", "--json", "--data-dir"])
        .arg(scratch.dir.path().join("data"))
        .arg("--config-dir")
        .arg(scratch.dir.path().join("config"))
        .env("QUILL_CONFIG_DIR", &nowhere)
        .env("QUILL_DATA_DIR", &nowhere)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_skill_is_printed_and_put_where_agents_look_for_it() {
    let shown = quill().args(["skill", "show"]).output().unwrap();
    assert!(shown.status.success());
    let text = String::from_utf8(shown.stdout).unwrap();
    assert!(
        text.starts_with("---\nname: quill\ndescription: "),
        "{}",
        &text[..80]
    );
    assert!(text.contains("quill search"));

    // Beside a build in a checkout, the skill that ships is the
    // checkout's own, and that is what is linked to.
    let path = quill().args(["skill", "path"]).output().unwrap();
    assert!(path.status.success());
    let shipped = std::path::PathBuf::from(String::from_utf8(path.stdout).unwrap().trim());
    assert!(shipped.ends_with("skills/quill") && shipped.join("SKILL.md").is_file());

    // With no folder named, the two that agents' programs read: under a
    // home of this test's own.
    let home = tempfile::tempdir().unwrap();
    let installed = quill()
        .args(["skill", "install"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for place in [".claude/skills/quill", ".agents/skills/quill"] {
        let at = home.path().join(place);
        assert_eq!(std::fs::read_link(&at).unwrap(), shipped, "{place}");
        assert_eq!(std::fs::read_to_string(at.join("SKILL.md")).unwrap(), text);
    }
    // A copy, in a folder that is named. And what is somebody else's
    // stops it, with a status that says so.
    let elsewhere = home.path().join("other/skills");
    let copied = quill()
        .args(["skill", "install", "--copy", "--to"])
        .arg(&elsewhere)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(copied.status.success());
    assert!(
        !std::fs::symlink_metadata(elsewhere.join("quill"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    std::fs::write(elsewhere.join("quill/mine.md"), "mine").unwrap();
    let refused = quill()
        .args(["skill", "install", "--to"])
        .arg(&elsewhere)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not Quill's skill"));
    assert!(elsewhere.join("quill/mine.md").is_file());
}

#[test]
fn quill_is_linked_into_a_folder_that_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    // The folder is said twice over, so that no way of reading the
    // arguments could send the link to the folders everyone's programs
    // are in.
    let link = || {
        quill()
            .args(["link", "--to"])
            .arg(&bin)
            .env("QUILL_LINK_DIR", &bin)
            .env("HOME", dir.path())
            .output()
            .unwrap()
    };
    let linked = link();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let at = bin.join("quill");
    assert_eq!(
        std::fs::read_link(&at).unwrap(),
        std::path::Path::new(env!("CARGO_BIN_EXE_quill"))
            .canonicalize()
            .unwrap()
    );
    // The link runs, and is this quill.
    let version = Command::new(&at).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("quill "));
    // Not on the `PATH` of this test, so how to put it there is said.
    assert!(String::from_utf8_lossy(&linked.stdout).contains("~/.zprofile"));
    // Again changes nothing.
    assert!(link().status.success());
    // Somebody's own program of that name stops it.
    std::fs::remove_file(&at).unwrap();
    std::fs::write(&at, "another").unwrap();
    let refused = link();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not a link to Quill"));
}
