//! Process-level tests that do not require a console.
//!
//! `dure` is a Windows-only binary; on other platforms it is an empty stub with
//! no behavior to assert on.
#![cfg(windows)]

use std::process::Command;

use tempfile::TempDir;

fn dure() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dure"))
}

// Talks to the real operating system: runs the built binary as a child process.
#[cfg_attr(miri, ignore)]
#[test]
fn help_succeeds() {
    let output = dure().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dure"));
}

// Talks to the real operating system: runs the built binary as a child process.
#[cfg_attr(miri, ignore)]
#[test]
fn list_empty_store() {
    let dir = TempDir::new().unwrap();
    let output = dure()
        .args([
            "--store-root",
            dir.path().to_str().expect("temp path is unicode"),
            "list",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ID"));
    assert!(stdout.contains("ATTACHED"));
}

// Talks to the real operating system: runs the built binary as a child process.
#[cfg_attr(miri, ignore)]
#[test]
fn kill_missing_id_fails() {
    let dir = TempDir::new().unwrap();
    let output = dure()
        .args([
            "--store-root",
            dir.path().to_str().expect("temp path is unicode"),
            "kill",
            "--id",
            "1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

// Talks to the real operating system: runs the built binary as a child process.
#[cfg_attr(miri, ignore)]
#[test]
fn kill_without_id_is_a_parse_error() {
    let output = dure().args(["kill"]).output().unwrap();
    assert!(!output.status.success());
}
