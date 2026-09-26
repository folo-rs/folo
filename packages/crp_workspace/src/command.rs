// Subprocess helper for `git` and `cargo`.
//
// Classification is specified to shell out rather than link libgit2/gix or a
// Cargo library, so this is the only subprocess boundary.

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Output, Stdio};

use ohno::{AppError, OhnoCore};

use crate::{CommandFailedError, CommandIoError};

/// Captured-process failures, with the exit distinction needed by configuration validation.
#[derive(ohno::Error)]
#[no_constructors]
pub struct CommandError {
    #[error]
    core: OhnoCore,
    nonzero_exit: bool,
}

impl CommandError {
    /// Distinguishes command rejection from inability to start or communicate with the process.
    #[must_use]
    pub fn is_nonzero_exit(&self) -> bool {
        self.nonzero_exit
    }
}

impl From<CommandFailedError> for CommandError {
    fn from(error: CommandFailedError) -> Self {
        Self {
            core: OhnoCore::from(error),
            nonzero_exit: true,
        }
    }
}

impl From<CommandIoError> for CommandError {
    fn from(error: CommandIoError) -> Self {
        Self {
            core: OhnoCore::from(error),
            nonzero_exit: false,
        }
    }
}

// The captured error is immutable, including the private condition in its source chain.
impl std::panic::UnwindSafe for CommandError {}
impl std::panic::RefUnwindSafe for CommandError {}

/// Hashes captured input bytes without writing an object into the repository.
pub fn hash_bytes(bytes: &[u8], cwd: &Path) -> Result<String, AppError> {
    run_capture_input("git", &["hash-object", "--stdin"], bytes, cwd)
        .map(|output| output.trim().to_owned())
        .map_err(Into::into)
}

/// Sends captured bytes to a subprocess without involving a shell or staging file.
pub fn run_capture_input(
    program: &str,
    args: &[&str],
    bytes: &[u8],
    cwd: &Path,
) -> Result<String, CommandError> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(subprocess_cwd(cwd))
        .env("CARGO_TERM_COLOR", "never")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    child
        .stdin
        .take()
        .expect("the child was started with a piped standard input")
        .write_all(bytes)
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    let output = child
        .wait_with_output()
        .map_err(|error| CommandIoError::caused_by(program, error))?;
    if !output.status.success() {
        return Err(CommandFailedError::new(
            program,
            failure_status(output.status),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs `program` with `args` in `cwd` and returns UTF-8 stdout on success.
///
/// # Errors
///
/// Returns an error when invocation fails or the command exits unsuccessfully.
pub fn run_capture(program: &str, args: &[&str], cwd: &Path) -> Result<String, CommandError> {
    run_capture_os(program, args, cwd)
}

/// Runs `program` with OS-str arguments (needed for git pathspecs on Windows).
///
/// Stdout is decoded lossily, so this is for output that is not a file name:
/// path listings go through [`run_capture_os_bytes`] and are decoded strictly,
/// because replacing a byte in a name would silently name a different file.
pub fn run_capture_os(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<String, CommandError> {
    let output = spawn(program, args, cwd)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(CommandFailedError::new(
            program,
            failure_status(output.status),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )
        .into())
    }
}

/// Like [`run_capture`], mapping a non-zero exit to `Ok(None)`.
///
/// Spawn failures still error.
pub fn run_capture_ok(
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
pub fn run_capture_bytes(program: &str, args: &[&str], cwd: &Path) -> Result<Vec<u8>, AppError> {
    run_capture_os_bytes(program, args.iter().map(OsStr::new), cwd)
}

/// Runs `program` with OS-str arguments and returns raw stdout on success.
pub fn run_capture_os_bytes(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<Vec<u8>, AppError> {
    let output = spawn(program, args, cwd)?;
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
pub fn run_capture_ok_bytes(
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
pub fn spawn(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<Output, CommandError> {
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
    Command::new(program)
        .args(args)
        .current_dir(subprocess_cwd(cwd))
        .env("CARGO_TERM_COLOR", "never")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| CommandIoError::caused_by(program, error).into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;

    use super::*;

    #[test]
    fn empty_cwd_uses_process_current_directory() {
        assert_eq!(subprocess_cwd(Path::new("")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new(".")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new("packages")), Path::new("packages"));
    }

    #[test]
    fn failure_status_preserves_the_exit_code() {
        // Distinct ordinary failures must not collapse to a generic diagnostic.
        for code in [1, 23] {
            #[cfg(unix)]
            let status = ExitStatus::from_raw(code << 8);
            #[cfg(windows)]
            let status = ExitStatus::from_raw(code);
            assert!(!status.success());
            assert_eq!(failure_status(status), code.to_string());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_signal_only_exit_has_a_stable_status() {
        // POSIX wait status for a process terminated by SIGTERM.
        let status = ExitStatus::from_raw(15);
        assert_eq!(failure_status(status), "signal");
    }
}
