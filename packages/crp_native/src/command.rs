use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead, BufReader, Read};
use std::panic::catch_unwind;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use command_group::CommandGroup;
use crp_diag::{DiagnosticSink, diagnostic};
use ohno::AppError;

/// Native command failures retain command identity and process diagnostics.
#[ohno::error]
#[display("{operation}: {diagnostic}")]
pub(crate) struct CommandFailed {
    operation: String,
    diagnostic: String,
}

/// Retains an independent cleanup failure without replacing the operation's original cause.
#[ohno::error]
#[display("{stage} also failed: {cleanup}")]
struct FinalizationFailed {
    stage: &'static str,
    cleanup: AppError,
}

/// Keeps drained process output alongside an interrupted operation and its cleanup errors.
#[ohno::error]
#[display("Captured process output:\n{stdout}\n{stderr}")]
struct CapturedOutput {
    stdout: String,
    stderr: String,
}

/// Commands inherit the build environment but never inherit the upload credential.
pub(crate) const TOKEN_VARIABLES: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GIT_TOKEN",
    "INPUT_TOKEN",
    "DEFAULT_GITHUB_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
];

// Observe cancellation promptly without busy-waiting during long native commands. This cadence
// limits routine wakeups, not end-to-end cleanup latency, which also depends on the OS scheduler.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Signal or supervision failure stops later work even when an item failure is recoverable.
static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

// The OS signal adapter only marks cancellation; the ordinary native loop owns child cleanup.
#[cfg_attr(test, mutants::skip)]
pub fn install_cancellation_handler() -> Result<(), AppError> {
    ctrlc::set_handler(|| CANCELLED.store(true, Ordering::Relaxed))?;
    Ok(())
}

/// Spawns only owned children and drains both pipes while enforcing an operation deadline.
// Native process, clock and pipe adapters are covered by executable integration tests.
#[cfg_attr(test, mutants::skip)]
pub fn capture(
    program: &OsStr,
    arguments: &[OsString],
    directory: &Path,
    environment: &[(&str, &OsStr)],
    diagnostics: &Arc<dyn DiagnosticSink>,
    deadline: Instant,
) -> Result<String, AppError> {
    capture_controlled(
        program,
        arguments,
        directory,
        environment,
        diagnostics,
        deadline,
        true,
    )
}

// Cleanup remains available after cancellation, with its own bounded deadline.
#[cfg_attr(test, mutants::skip)]
pub fn capture_cleanup(
    arguments: &[OsString],
    directory: &Path,
    diagnostics: &Arc<dyn DiagnosticSink>,
    deadline: Instant,
) -> Result<String, AppError> {
    capture_controlled(
        OsStr::new("git"),
        arguments,
        directory,
        &[],
        diagnostics,
        deadline,
        false,
    )
}

// Real process/pipe/clock effects are integration boundaries; command-group owns tree mechanics.
#[cfg_attr(test, mutants::skip)]
fn capture_controlled(
    program: &OsStr,
    arguments: &[OsString],
    directory: &Path,
    environment: &[(&str, &OsStr)],
    diagnostics: &Arc<dyn DiagnosticSink>,
    deadline: Instant,
    cancellable: bool,
) -> Result<String, AppError> {
    if let Some(reason) = interruption(Instant::now() >= deadline, cancellable && cancelled()) {
        return Err(CommandFailed::new(program.display().to_string(), reason.to_owned()).into());
    }
    let message = format!(
        "Running {} {:?} in {}\n",
        program.display(),
        arguments,
        directory.display()
    );
    if cancellable {
        diagnostic(diagnostics.as_ref(), &message);
    } else {
        // Cleanup cannot be prevented by an unavailable diagnostic destination.
        _ = catch_unwind(|| diagnostics.write(&message));
    }
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in TOKEN_VARIABLES {
        command.env_remove(name);
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    // Rustup must resolve the source's pin from cwd, not a controller-selected override.
    command.env_remove("RUSTUP_TOOLCHAIN");
    // The ecosystem adapter owns Windows job objects and Unix process groups; no PID discovery
    // or platform-specific tree-killing implementation belongs in release automation.
    let mut child = command.group_spawn()?;
    let stdout = child
        .inner()
        .stdout
        .take()
        .expect("stdout was explicitly piped");
    let stderr = child
        .inner()
        .stderr
        .take()
        .expect("stderr was explicitly piped");
    let stdout = reader(stdout, None);
    // Cleanup still captures stderr for a failing result, but advisory streaming must not
    // stop draining its pipe or prevent resource disposal when diagnostics are unavailable.
    let stderr = reader(stderr, cancellable.then(|| Arc::clone(diagnostics)));
    let mut status = None;
    let result = loop {
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    CANCELLED.store(true, Ordering::Relaxed);
                    break Err(CommandFailed::caused_by(
                        program.display().to_string(),
                        "polling the owned process failed".to_owned(),
                        error,
                    )
                    .into());
                }
            };
        }
        if let Some(status) = status
            && stdout.is_finished()
            && stderr.is_finished()
        {
            break Ok(status);
        }
        if let Some(reason) = interruption(Instant::now() >= deadline, cancellable && cancelled()) {
            break Err(CommandFailed::new(program.display().to_string(), reason.to_owned()).into());
        }
        thread::sleep(POLL_INTERVAL);
    };
    let status = match result {
        Ok(status) => status,
        Err(error) => {
            let killed = child.kill();
            // A rejected termination cannot authorize an unbounded wait for a live process.
            // Still attempt to reap an already exited child, independently of the kill result.
            let waited = if killed.is_ok() {
                child.wait().map(|_| ())
            } else {
                child.try_wait().map(|_| ())
            };
            let terminated = killed.is_ok() && waited.is_ok();
            if !terminated {
                CANCELLED.store(true, Ordering::Relaxed);
            }
            let error = retain_cleanup(
                error,
                "terminating the owned process tree",
                killed.map_err(Into::into),
            );
            let error = retain_cleanup(
                error,
                "reaping the owned process",
                waited.map_err(Into::into),
            );
            let (error, stdout) = finish_reader(error, "stdout", stdout, terminated);
            let (error, stderr) = finish_reader(error, "stderr", stderr, terminated);
            return Err(CapturedOutput::caused_by(stdout, stderr, error).into());
        }
    };
    // Both readers are finalized even if one failed; neither error may discard the other.
    let stdout = join_reader(stdout);
    let stderr = join_reader(stderr);
    match (stdout, stderr) {
        (Ok(stdout), Ok(_)) if status.success() => Ok(stdout),
        (Ok(stdout), Ok(stderr)) => Err(CommandFailed::new(
            program.display().to_string(),
            format!("exit {status}\n{stdout}\n{stderr}"),
        )
        .into()),
        (stdout, stderr) => {
            let mut error = CommandFailed::new(
                program.display().to_string(),
                format!("output capture failed; exit {status}"),
            )
            .into();
            error = retain_cleanup(error, "reading stdout", stdout.map(|_| ()));
            Err(retain_cleanup(error, "reading stderr", stderr.map(|_| ())))
        }
    }
}

fn retain_cleanup(
    operation: AppError,
    stage: &'static str,
    cleanup: Result<(), AppError>,
) -> AppError {
    match cleanup {
        Ok(()) => operation,
        Err(cleanup) => FinalizationFailed::caused_by(stage, cleanup, operation).into(),
    }
}

// Joining requires either terminated writers or a completed reader. Failed tree cleanup must
// report a still-owned reader rather than hiding the original error behind another indefinite wait.
#[cfg_attr(test, mutants::skip)]
fn finish_reader(
    operation: AppError,
    stream: &'static str,
    reader: JoinHandle<io::Result<String>>,
    terminated: bool,
) -> (AppError, String) {
    let result = if terminated || reader.is_finished() {
        join_reader(reader)
    } else {
        Err(CommandFailed::new(
            stream.to_owned(),
            "reader remains active after process-tree cleanup failed".to_owned(),
        )
        .into())
    };
    match result {
        Ok(output) => (operation, output),
        Err(error) => (retain_cleanup(operation, stream, Err(error)), String::new()),
    }
}

pub(crate) fn interruption(deadline_reached: bool, cancelled: bool) -> Option<&'static str> {
    if cancelled {
        Some("batch cancelled")
    } else if deadline_reached {
        Some("item deadline exhausted")
    } else {
        None
    }
}

// Pipe reading/streaming is an OS boundary; exercised with the real executable.
#[cfg_attr(test, mutants::skip)]
fn reader(
    pipe: impl Read + Send + 'static,
    stream: Option<Arc<dyn DiagnosticSink>>,
) -> JoinHandle<io::Result<String>> {
    thread::spawn(move || {
        let mut output = String::new();
        for line in BufReader::new(pipe).lines() {
            let line = line?;
            if let Some(sink) = &stream {
                sink.write(&format!("{line}\n"))?;
            }
            output.push_str(&line);
            output.push('\n');
        }
        Ok(output)
    })
}

// Reader failures remain errors, including unexpected reader-thread panics.
fn join_reader(reader: JoinHandle<io::Result<String>>) -> Result<String, AppError> {
    reader
        .join()
        .map_err(|_panic_payload| {
            CommandFailed::new(
                "read process output".to_owned(),
                "reader thread panicked".to_owned(),
            )
        })?
        .map_err(Into::into)
}

/// Constructs literal subprocess arguments without changing the environment or running a command.
pub fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    #[test]
    fn supervision_only_continues_before_both_stop_conditions() {
        assert!(interruption(false, false).is_none());
        assert!(interruption(true, false).is_some());
        assert!(interruption(false, true).is_some());
        assert_eq!(interruption(true, true), interruption(false, true));
        assert_ne!(interruption(true, false), interruption(false, true));
    }

    #[test]
    fn cleanup_preserves_the_operation_and_every_independent_failure() {
        for kill_fails in [false, true] {
            for wait_fails in [false, true] {
                let operation = CommandFailed::new("poll canary".to_owned(), "failed".to_owned());
                let cleanup = |fails, message| {
                    if fails {
                        Err(Error::other(message).into())
                    } else {
                        Ok(())
                    }
                };
                let error = retain_cleanup(
                    operation.into(),
                    "terminate",
                    cleanup(kill_fails, "kill canary"),
                );
                let error = retain_cleanup(error, "reap", cleanup(wait_fails, "wait canary"));
                assert_eq!(
                    error.find_source::<CommandFailed>().unwrap().operation,
                    "poll canary"
                );
                let diagnostic = error.to_string();
                assert_eq!(diagnostic.contains("kill canary"), kill_fails);
                assert_eq!(diagnostic.contains("wait canary"), wait_fails);
            }
        }
    }

    #[test]
    fn reader_results_preserve_text_and_propagate_errors() {
        testing::with_watchdog(|| {
            assert_eq!(
                join_reader(reader(&b"first\nsecond"[..], None)).unwrap(),
                "first\nsecond\n"
            );
            let error = join_reader(reader(&b"\xff"[..], None)).unwrap_err();
            assert_eq!(
                error.find_source::<Error>().unwrap().kind(),
                ErrorKind::InvalidData
            );
            let error = join_reader(thread::spawn(|| panic!("fixture reader panic"))).unwrap_err();
            assert!(error.find_source::<CommandFailed>().is_some());
        });
    }
}
