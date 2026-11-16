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
    for command in ["list", "search", "read", "send", "sync", "account"] {
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
