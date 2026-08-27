//! Process-level tests that do not require a console.

use std::process::Command;

use tempfile::TempDir;

fn dure() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dure"))
}

#[test]
fn help_succeeds_on_every_platform() {
    let output = dure().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dure"));
}

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
    if cfg!(windows) {
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("ID"));
        assert!(stdout.contains("ATTACHED"));
    } else {
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Error:"));
    }
}

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

#[test]
fn kill_without_id_is_a_parse_error() {
    let output = dure().args(["kill"]).output().unwrap();
    assert!(!output.status.success());
}
