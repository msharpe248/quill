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
