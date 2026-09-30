// Shared captured Git/Cargo observation commands for workspace and versioning operations.
// Native execution and external compatibility checking have their own process boundaries.

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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
    /// Reports a normal nonzero exit, not signal termination or a process I/O failure.
    ///
    /// The caller determines what that status means for the particular command.
    #[must_use]
    pub fn is_nonzero_exit(&self) -> bool {
        self.nonzero_exit
    }
}

impl From<CommandFailedError> for CommandError {
    fn from(error: CommandFailedError) -> Self {
        let nonzero_exit = error.is_nonzero_exit();
        Self {
            core: OhnoCore::from(error),
            nonzero_exit,
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

/// Fixed orchestration credential inputs removed from compilation environments.
///
/// Compilation adapters additionally remove named Cargo registry tokens and retain
/// responsibility for any credentials explicitly granted to an individual operation.
pub const BUILD_CREDENTIAL_VARIABLES: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GIT_TOKEN",
    "INPUT_TOKEN",
    "DEFAULT_GITHUB_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
];

/// Hashes captured input bytes without writing an object into the repository.
// Git owns object hashing; its invocation and output are checked by command boundary tests.
#[cfg_attr(test, mutants::skip)]
pub fn hash_bytes(bytes: &[u8], cwd: &Path) -> Result<String, AppError> {
    run_capture_input("git", &["hash-object", "--stdin"], bytes, cwd)
        .map(|output| output.trim().to_owned())
        .map_err(Into::into)
}

/// Sends captured bytes to a subprocess without involving a shell or staging file.
// Pipe ownership and child execution require a real subprocess; captured_output tests the verdict.
#[cfg_attr(test, mutants::skip)]
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
    captured_output(program, output).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Runs `program` with `args` in `cwd` and returns UTF-8 stdout on success.
///
/// # Errors
///
/// Returns an error when invocation fails or the command exits unsuccessfully.
// Argument-forwarding adapter; native command boundary tests cover the actual invocation.
#[cfg_attr(test, mutants::skip)]
pub fn run_capture(program: &str, args: &[&str], cwd: &Path) -> Result<String, CommandError> {
    run_capture_os(program, args, cwd)
}

/// Runs `program` with OS-str arguments (needed for git pathspecs on Windows).
///
/// Stdout is decoded lossily, so this is for output that is not a file name:
/// path listings go through [`run_capture_os_bytes`] and are decoded strictly,
/// because replacing a byte in a name would silently name a different file.
// Native acquisition delegates the success/error decision to captured_output.
#[cfg_attr(test, mutants::skip)]
pub fn run_capture_os(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<String, CommandError> {
    captured_output(program, spawn(program, args, cwd)?)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Like [`run_capture`], mapping a non-zero exit to `Ok(None)`.
///
/// Spawn failures still error.
// Native optional-output forwarding; optional_capture owns absence versus operational failure.
#[cfg_attr(test, mutants::skip)]
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
// This adapter only converts argument types before native execution.
#[cfg_attr(test, mutants::skip)]
pub fn run_capture_bytes(program: &str, args: &[&str], cwd: &Path) -> Result<Vec<u8>, AppError> {
    run_capture_os_bytes(program, args.iter().map(OsStr::new), cwd)
}

/// Runs `program` with OS-str arguments and returns raw stdout on success.
// Native acquisition delegates lossless bytes and status interpretation to captured_output.
#[cfg_attr(test, mutants::skip)]
pub fn run_capture_os_bytes(
    program: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    cwd: &Path,
) -> Result<Vec<u8>, AppError> {
    captured_output(program, spawn(program, args, cwd)?).map_err(Into::into)
}

fn captured_output(program: &str, output: Output) -> Result<Vec<u8>, CommandError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(CommandFailedError::new(
            program,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )
        .into())
    }
}

/// Like [`run_capture_ok`], keeping stdout as raw bytes.
///
/// Binary files can then be compared without UTF-8 replacement.
// The subprocess is integration-only; optional_capture remains mutation-tested in process.
#[cfg_attr(test, mutants::skip)]
pub fn run_capture_ok_bytes(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<Option<Vec<u8>>, AppError> {
    optional_capture(run_capture_bytes(program, args, cwd))
}

fn optional_capture(result: Result<Vec<u8>, AppError>) -> Result<Option<Vec<u8>>, AppError> {
    match result {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) => {
            if error
                .find_source::<CommandFailedError>()
                .is_some_and(CommandFailedError::is_nonzero_exit)
            {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
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
    use std::error::Error;
    use std::fmt::Debug;
    use std::io;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::process::ExitStatus;

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(CommandError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn empty_cwd_uses_process_current_directory() {
        assert_eq!(subprocess_cwd(Path::new("")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new(".")), Path::new("."));
        assert_eq!(subprocess_cwd(Path::new("packages")), Path::new("packages"));
    }

    fn status(code: u32) -> ExitStatus {
        #[cfg(unix)]
        return ExitStatus::from_raw(i32::try_from(code).unwrap() << 8);
        #[cfg(windows)]
        return ExitStatus::from_raw(code);
    }

    #[test]
    fn captured_output_preserves_bytes_and_failed_command_diagnostics() {
        let bytes = vec![0xff, 0, b'\n', b'x'];
        let output = Output {
            status: status(0),
            stdout: bytes.clone(),
            stderr: b"unused diagnostic".to_vec(),
        };
        assert_eq!(captured_output("git", output).unwrap(), bytes);
        let error = captured_output(
            "git",
            Output {
                status: status(23),
                stdout: b"partial".to_vec(),
                stderr: b"  rejected input\n".to_vec(),
            },
        )
        .unwrap_err();
        assert!(error.is_nonzero_exit());
        assert_eq!(
            error.find_source::<CommandFailedError>().unwrap().stderr(),
            "rejected input"
        );
    }

    #[test]
    fn optional_capture_distinguishes_empty_output_absence_and_io_failure() {
        assert_eq!(optional_capture(Ok(vec![])).unwrap(), Some(vec![]));
        assert_eq!(optional_capture(Ok(vec![0xff])).unwrap(), Some(vec![0xff]));
        let failure = CommandFailedError::new("git", status(1), "absent");
        assert_eq!(optional_capture(Err(failure.into())).unwrap(), None);
        let error = optional_capture(Err(CommandIoError::caused_by(
            "git",
            io::Error::other("pipe"),
        )
        .into()))
        .unwrap_err();
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn command_failure_reports_only_normal_nonzero_exits() {
        // Distinct ordinary failures must not collapse to a generic diagnostic.
        for code in [1, 23] {
            #[cfg(unix)]
            let status = ExitStatus::from_raw(code << 8);
            #[cfg(windows)]
            let status = ExitStatus::from_raw(code);
            assert!(!status.success());
            let failure = CommandFailedError::new("git", status, "rejected");
            let error = CommandError::from(failure);
            assert!(error.is_nonzero_exit());
        }
        let error = CommandError::from(CommandIoError::caused_by("git", io::Error::other("spawn")));
        assert!(!error.is_nonzero_exit());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn signal_termination_is_an_operational_failure() {
        // POSIX wait status for a process terminated by SIGTERM.
        let status = ExitStatus::from_raw(15);
        let failure = CommandFailedError::new("git", status, "terminated");
        let error = optional_capture(Err(failure.into())).unwrap_err();
        assert!(error.find_source::<CommandFailedError>().is_some());
        let failure = CommandFailedError::new("git", status, "terminated");
        let error = CommandError::from(failure);
        assert!(!error.is_nonzero_exit());
        assert!(error.find_source::<CommandFailedError>().is_some());
    }
}
