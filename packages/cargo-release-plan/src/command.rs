// Subprocess helper for `git` and `cargo`.
//
// Classification is specified to shell out rather than link libgit2/gix or a
// Cargo library, so this is the only process port.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use ohno::AppError;

use crate::{CommandFailedError, CommandSpawnError};

/// Runs `program` with `args` in `cwd` and returns UTF-8 stdout on success.
///
/// # Errors
///
/// Returns [`CommandSpawnError`] if the process cannot be created, or
/// [`CommandFailedError`] if it exits unsuccessfully.
pub(crate) fn run_capture(program: &str, args: &[&str], cwd: &Path) -> Result<String, AppError> {
    run_capture_os(program, args, cwd)
}

/// Runs `program` with OS-str arguments (needed for git pathspecs on Windows).
pub(crate) fn run_capture_os(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<String, AppError> {
    let args: Vec<_> = args.into_iter().collect();
    let output = spawn(program, args.iter().map(AsRef::as_ref), cwd)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let status = match output.status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        Err(CommandFailedError::new(
            program,
            status,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )
        .into())
    }
}

/// Like [`run_capture`], but a non-zero exit is returned as `Ok(None)` instead
/// of an error. Spawn failures still error.
pub(crate) fn run_capture_ok(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<Option<String>, AppError> {
    match run_capture_ok_bytes(program, args, cwd)? {
        Some(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        None => Ok(None),
    }
}

/// Like [`run_capture_ok`], keeping stdout as raw bytes so binary files can be
/// compared without UTF-8 replacement.
pub(crate) fn run_capture_ok_bytes(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<Option<Vec<u8>>, AppError> {
    let output = spawn(program, args.iter().map(OsStr::new), cwd)?;
    if output.status.success() {
        Ok(Some(output.stdout))
    } else {
        Ok(None)
    }
}

// Mutations of the spawn arguments cannot be caught without asserting on a real
// child process, which is impractical in unit tests.
#[cfg_attr(test, mutants::skip)]
fn spawn(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<std::process::Output, AppError> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .map_err(|error| CommandSpawnError::caused_by(program, error).into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::CommandSpawnError;

    #[cfg_attr(miri, ignore)] // Process spawn uses host APIs Miri cannot emulate.
    #[test]
    fn run_capture_ok_returns_stdout_on_success() {
        let stdout = run_capture_ok(
            "git",
            &["rev-parse", "--is-inside-work-tree"],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(stdout.as_deref().map(str::trim), Some("true"));
    }

    #[cfg_attr(miri, ignore)] // Process spawn uses host APIs Miri cannot emulate.
    #[test]
    fn run_capture_ok_none_on_nonzero_exit() {
        let stdout = run_capture_ok(
            "git",
            &["rev-parse", "--verify", "cargo-release-plan-no-such-rev"],
            Path::new("."),
        )
        .unwrap();
        assert!(stdout.is_none());
    }

    #[cfg_attr(miri, ignore)] // Process spawn uses host APIs Miri cannot emulate.
    #[test]
    fn spawn_failure_maps_to_command_spawn_error() {
        // A program name that cannot exist on PATH cannot be spawned.
        let error = spawn(
            "cargo-release-plan-no-such-program",
            None::<&OsStr>,
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.find_source::<CommandSpawnError>().is_some());
    }
}
