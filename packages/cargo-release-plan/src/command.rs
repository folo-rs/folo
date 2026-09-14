// Subprocess helper for `git` and `cargo`.
//
// Classification is specified to shell out rather than link libgit2/gix or a
// Cargo library, so this is the only subprocess boundary.

use std::ffi::OsStr;
use std::io::{Seek as _, Write as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Output, Stdio};

use ohno::AppError;
use tempfile::tempfile;

use crate::{CommandFailedError, CommandIoError};

/// Hashes captured input bytes without writing an object into the repository.
pub(crate) fn hash_bytes(bytes: &[u8], cwd: &Path) -> Result<String, AppError> {
    run_capture_input("git", &["hash-object", "--stdin"], bytes, cwd)
        .map(|output| output.trim().to_owned())
}

/// Sends captured bytes to a subprocess and decodes stdout lossily.
pub(crate) fn run_capture_input(
    program: &str,
    args: &[&str],
    bytes: &[u8],
    cwd: &Path,
) -> Result<String, AppError> {
    run_capture_input_bytes(program, args, bytes, cwd)
        .map(|output| String::from_utf8_lossy(&output).into_owned())
}

/// Captures raw output after giving a subprocess a finite, already-written input.
///
/// An anonymous file avoids a pipe deadlock when a batch reader fills stdout
/// before consuming all its input. The child reads to EOF while `output` drains
/// stdout and stderr together; no persistent process or writer thread is needed.
pub(crate) fn run_capture_input_bytes(
    program: &str,
    args: &[&str],
    bytes: &[u8],
    cwd: &Path,
) -> Result<Vec<u8>, AppError> {
    let mut input = tempfile().map_err(|error| CommandIoError::caused_by(program, error))?;
    input
        .write_all(bytes)
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    input
        .rewind()
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    let output = capture_command(program, args, cwd)
        .stdin(Stdio::from(input))
        .output()
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    capture_stdout(program, output)
}

/// Runs `program` with `args` in `cwd` and returns UTF-8 stdout on success.
///
/// # Errors
///
/// Returns [`CommandIoError`] if starting or communicating with the process fails, or
/// [`CommandFailedError`] if it exits unsuccessfully.
pub(crate) fn run_capture(program: &str, args: &[&str], cwd: &Path) -> Result<String, AppError> {
    run_capture_os(program, args, cwd)
}

/// Runs `program` with OS-str arguments (needed for git pathspecs on Windows).
///
/// Stdout is decoded lossily, so this is for output that is not a file name:
/// path listings go through [`run_capture_os_bytes`] and are decoded strictly,
/// because replacing a byte in a name would silently name a different file.
pub(crate) fn run_capture_os(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<String, AppError> {
    run_capture_os_bytes(program, args, cwd)
        .map(|output| String::from_utf8_lossy(&output).into_owned())
}

/// Like [`run_capture`], mapping a non-zero exit to `Ok(None)`.
///
/// Spawn failures still error.
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

/// Runs `program` and returns raw stdout on success.
pub(crate) fn run_capture_bytes(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<Vec<u8>, AppError> {
    run_capture_os_bytes(program, args.iter().map(OsStr::new), cwd)
}

/// Runs `program` with OS-str arguments and returns raw stdout on success.
pub(crate) fn run_capture_os_bytes(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<Vec<u8>, AppError> {
    capture_stdout(program, spawn(program, args, cwd)?)
}

fn capture_stdout(program: &str, output: Output) -> Result<Vec<u8>, AppError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(CommandFailedError::new(
            program,
            failure_status(output.status),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )
        .into())
    }
}

/// Like [`run_capture_ok`], keeping stdout as raw bytes.
///
/// Binary files can then be compared without UTF-8 replacement.
pub(crate) fn run_capture_ok_bytes(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<Option<Vec<u8>>, AppError> {
    match run_capture_bytes(program, args, cwd) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) => {
            if error.find_source::<CommandFailedError>().is_some() {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

/// Renders either the exit code or the signal-only fallback.
fn failure_status(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_string(), |code| code.to_string())
}

/// Directory passed to `Command::current_dir`.
///
/// `Path::parent()` of a relative file such as `Cargo.toml` is the empty path.
/// `Command::current_dir` rejects that empty path (Windows `ERROR_INVALID_NAME`,
/// Unix `chdir("")`), so use the process current directory instead.
fn subprocess_cwd(cwd: &Path) -> &Path {
    if cwd.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cwd
    }
}

// Mutations of the spawn arguments cannot be caught without asserting on a real
// child process, which is impractical in unit tests.
#[cfg_attr(test, mutants::skip)]
fn spawn(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<Output, AppError> {
    capture_command(program, args, cwd)
        .output()
        .map_err(|error| CommandIoError::caused_by(program, error).into())
}

fn capture_command(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Command {
    // Every child here is captured through pipes and its output is parsed or
    // surfaced verbatim in diagnostics, so it must be free of ANSI escapes. The
    // override belongs on the shared boundary rather than at each call site
    // because Cargo's automatic detection depends on the ambient environment,
    // which would otherwise make captured output differ between a terminal, a
    // CI runner and a test harness.
    //
    // The locale is pinned for the same reason: Git translates its diagnostics,
    // and `git.rs` recognises a path that is absent from a revision by the
    // wording Git uses. Under a translated locale that wording never matches and
    // an ordinary package creation or deletion would surface as an operational
    // error. GNU gettext ignores `LANGUAGE` once the locale is `C`, so this one
    // variable settles it.
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(subprocess_cwd(cwd))
        .env("CARGO_TERM_COLOR", "never")
        .env("LC_ALL", "C");
    command
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;

    use tempfile::TempDir;

    use super::*;
    use crate::CommandIoError;

    #[test]
    fn empty_cwd_uses_process_current_directory() {
        assert_eq!(subprocess_cwd(Path::new("")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new(".")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new("packages")), Path::new("packages"));
    }

    #[test]
    fn capture_configuration_is_shared_by_input_and_no_input_commands() {
        let command = capture_command("git", ["cat-file", "--batch"], Path::new(""));
        assert_eq!(command.get_program(), OsStr::new("git"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("cat-file"), OsStr::new("--batch")]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new(".")));
        assert_eq!(
            command.get_envs().collect::<BTreeMap<_, _>>(),
            BTreeMap::from([
                (OsStr::new("CARGO_TERM_COLOR"), Some(OsStr::new("never"))),
                (OsStr::new("LC_ALL"), Some(OsStr::new("C"))),
            ])
        );
    }

    #[test]
    fn successful_capture_preserves_arbitrary_stdout_bytes() {
        let bytes = b"\0first\n\xfflast\0\n";
        let output = Output {
            status: ExitStatus::default(),
            stdout: bytes.to_vec(),
            stderr: b"non-failing diagnostic".to_vec(),
        };
        assert_eq!(capture_stdout("git", output).unwrap().as_slice(), bytes);
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns Git with captured standard input")]
    fn captured_input_failure_preserves_a_nonzero_exit() {
        // Invalid tree encoding makes Git reject input without requiring a repository.
        let error = run_capture_input(
            "git",
            &["hash-object", "--stdin", "-t", "tree"],
            b"not a tree object",
            Path::new("."),
        )
        .unwrap_err();
        assert!(error.find_source::<CommandFailedError>().is_some());
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
        assert!(error.find_source::<CommandIoError>().is_some());
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
        assert!(error.find_source::<CommandIoError>().is_some());
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
        assert!(error.find_source::<CommandIoError>().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_signal_only_exit_has_a_stable_status() {
        // POSIX wait status for a process terminated by SIGTERM.
        let status = ExitStatus::from_raw(15);
        assert_eq!(failure_status(status), "signal");
    }
}
