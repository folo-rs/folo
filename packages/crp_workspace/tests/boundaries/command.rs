//! External acquisition for command.

use std::ffi::OsStr;
use std::path::Path;

use crp_workspace::command::*;
use ohno::ErrorExt as _;
use tempfile::TempDir;

#[test]
#[cfg_attr(miri, ignore = "spawns Git with captured standard input")]
fn captured_input_failure_preserves_a_nonzero_exit() {
    // Git reads the complete object before validating its tree encoding, so this does
    // not race the child closing stdin before the parent writes its test input.
    let error = run_capture_input(
        "git",
        &["hash-object", "--stdin", "-t", "tree"],
        b"not a tree object",
        Path::new("."),
    )
    .unwrap_err();
    assert!(error.is_nonzero_exit());
}

#[test]
#[cfg_attr(miri, ignore = "attempts a native process spawn")]
fn captured_input_propagates_a_spawn_failure() {
    let error = run_capture_input(
        "cargo-release-plan-no-such-program",
        &[],
        b"input",
        Path::new("."),
    )
    .unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}

#[cfg_attr(miri, ignore = "spawns Git in a temporary directory")]
#[test]
fn run_capture_ok_returns_stdout_on_success() {
    let dir = TempDir::new().unwrap();
    // Quoting arguments does not require a repository, so this exercises successful
    // capture even in a disposable source copy without Git metadata.
    let stdout = run_capture_ok(
        "git",
        &["rev-parse", "--sq-quote", "captured stdout"],
        dir.path(),
    )
    .unwrap();
    assert_eq!(stdout.as_deref(), Some(" 'captured stdout'\n"));
}

#[cfg_attr(miri, ignore = "spawns Git in a temporary directory")]
#[test]
fn run_capture_ok_none_on_nonzero_exit() {
    let dir = TempDir::new().unwrap();
    // Invalid ref syntax fails independently of repository contents or discovery.
    let stdout = run_capture_ok(
        "git",
        &["check-ref-format", "refs/heads/invalid..ref"],
        dir.path(),
    )
    .unwrap();
    assert!(stdout.is_none());
}

#[cfg_attr(miri, ignore)] // Process spawn uses host APIs Miri cannot emulate.
#[test]
fn run_capture_ok_propagates_a_spawn_failure() {
    // A non-zero exit means "no answer", but never starting the program at
    // all is a real error the caller must see.
    let error =
        run_capture_ok("cargo-release-plan-no-such-program", &[], Path::new(".")).unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}

#[cfg_attr(miri, ignore)] // Process spawn uses host APIs Miri cannot emulate.
#[test]
fn spawn_failure_maps_to_command_io_error() {
    // A program name that cannot exist on PATH cannot be spawned.
    let error = spawn(
        "cargo-release-plan-no-such-program",
        None::<&OsStr>,
        Path::new("."),
    )
    .unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}
