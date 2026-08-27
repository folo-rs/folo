//! Subprocess coverage of the `cargo-release-plan` binary entry point.

use std::process::Command;

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn help_exits_success() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn cargo_injected_subcommand_is_stripped() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
        .args(["release-plan", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn unknown_flag_exits_failure() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
        .arg("--definitely-not-a-flag")
        .output()
        .unwrap();
    assert!(!output.status.success());
}
