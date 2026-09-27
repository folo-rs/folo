use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::panic::catch_unwind;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use command_group::CommandGroup;
use crp_diag::{DiagnosticSink, diagnostic, quote_path};
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

/// Retains pipe contents even when their secondary diagnostic destination fails.
#[derive(Debug)]
struct CapturedStream {
    text: String,
    error: Option<AppError>,
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

/// A one-way stop notification, with no accompanying state published through it.
///
/// Relaxed ordering suffices because observing cancellation does not authorize reading any
/// other memory; adding associated cancellation data would require revisiting that invariant.
static CANCELLED: AtomicBool = AtomicBool::new(false);

fn strip_build_credentials(command: &mut Command, names: impl Iterator<Item = OsString>) {
    for name in TOKEN_VARIABLES {
        command.env_remove(name);
    }
    for name in names {
        if registry_token_variable(&name) {
            command.env_remove(name);
        }
    }
}

fn registry_token_variable(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let name = name.to_ascii_uppercase();
        name.starts_with("CARGO_REGISTRIES_") && name.ends_with("_TOKEN")
    })
}

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
    let operation = quote_path(&program.to_string_lossy()).into_owned();
    if let Some(reason) = interruption(Instant::now() >= deadline, cancellable && cancelled()) {
        return Err(CommandFailed::new(operation, reason.to_owned()).into());
    }
    let message = format!(
        "Running {} {:?} in {}\n",
        operation,
        arguments,
        quote_path(&directory.to_string_lossy())
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
    strip_build_credentials(&mut command, env::vars_os().map(|(name, _)| name));
    for (name, value) in environment {
        command.env(name, value);
    }
    // Cargo launchers set both variables. Source commands must select their own tracked
    // toolchain, while preserving caller build settings and Cargo/rustup homes.
    command.env_remove("RUSTUP_TOOLCHAIN").env_remove("CARGO");
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
                        operation.clone(),
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
            break Err(CommandFailed::new(operation.clone(), reason.to_owned()).into());
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
        (Ok(stdout), Ok(stderr))
            if status.success() && stdout.error.is_none() && stderr.error.is_none() =>
        {
            Ok(stdout.text)
        }
        (stdout, stderr) => {
            let error = CommandFailed::new(operation, format!("exit {status}")).into();
            let (error, stdout) = retain_reader_result(error, "stdout", stdout);
            let (error, stderr) = retain_reader_result(error, "stderr", stderr);
            Err(CapturedOutput::caused_by(stdout, stderr, error).into())
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
    reader: JoinHandle<CapturedStream>,
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
    retain_reader_result(operation, stream, result)
}

fn retain_reader_result(
    operation: AppError,
    stream: &'static str,
    result: Result<CapturedStream, AppError>,
) -> (AppError, String) {
    match result {
        Ok(CapturedStream { text, error }) => match error {
            Some(error) => (retain_cleanup(operation, stream, Err(error)), text),
            None => (operation, text),
        },
        Err(error) => (retain_cleanup(operation, stream, Err(error)), String::new()),
    }
}

pub(crate) fn interruption(deadline_reached: bool, cancelled: bool) -> Option<&'static str> {
    if cancelled {
        Some("batch cancelled")
    } else if deadline_reached {
        Some("operation deadline reached")
    } else {
        None
    }
}

// Pipe reading/streaming is an OS boundary; exercised with the real executable.
#[cfg_attr(test, mutants::skip)]
fn reader(
    pipe: impl Read + Send + 'static,
    stream: Option<Arc<dyn DiagnosticSink>>,
) -> JoinHandle<CapturedStream> {
    thread::spawn(move || read_output(pipe, stream.as_deref()))
}

fn read_output(pipe: impl Read, stream: Option<&dyn DiagnosticSink>) -> CapturedStream {
    let mut output = CapturedStream {
        text: String::new(),
        error: None,
    };
    for line in BufReader::new(pipe).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                output.error = Some(match output.error.take() {
                    Some(primary) => {
                        retain_cleanup(primary, "reading process output", Err(error.into()))
                    }
                    None => error.into(),
                });
                break;
            }
        };
        output.text.push_str(&line);
        output.text.push('\n');
        if output.error.is_none()
            && let Some(sink) = stream
        {
            // Mirroring is secondary to draining the pipe. Retain its first failure and stop
            // mirroring, not reading; otherwise a closed pipe can change the child's own result.
            output.error = match catch_unwind(|| sink.write(&format!("{line}\n"))) {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.into()),
                Err(_) => Some(
                    CommandFailed::new(
                        "stream diagnostics".to_owned(),
                        "diagnostic sink panicked".to_owned(),
                    )
                    .into(),
                ),
            };
        }
    }
    output
}

// Reader failures remain errors, including unexpected reader-thread panics.
fn join_reader(reader: JoinHandle<CapturedStream>) -> Result<CapturedStream, AppError> {
    reader
        .join()
        .map_err(|_panic_payload| {
            CommandFailed::new(
                "read process output".to_owned(),
                "reader thread panicked".to_owned(),
            )
        })
        .map_err(Into::into)
}

/// Constructs literal subprocess arguments without changing the environment or running a command.
pub fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{self, Error, ErrorKind};
    use std::sync::atomic::AtomicUsize;

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
    fn credential_aliases_do_not_remove_build_configuration() {
        let mut command = Command::new("cargo");
        let names = [
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", true),
            ("CARGO_REGISTRIES_PRIVATE_TOKEN", true),
            ("cargo_registries_private_token", true),
            ("CARGO_REGISTRIES_PRIVATE_INDEX", false),
            ("CARGO_HOME", false),
            ("RUSTUP_HOME", false),
        ];
        strip_build_credentials(
            &mut command,
            names.iter().map(|(name, _)| OsString::from(name)),
        );
        let removed = command
            .get_envs()
            .map(|(name, value)| {
                assert!(value.is_none());
                name.to_owned()
            })
            .collect::<Vec<_>>();
        for (name, credential) in names {
            assert_eq!(registry_token_variable(OsStr::new(name)), credential);
            // Command normalizes environment keys on some hosts; use the filter's name policy.
            assert_eq!(
                removed
                    .iter()
                    .any(|removed| removed.to_string_lossy().eq_ignore_ascii_case(name)),
                credential,
            );
        }
        for name in TOKEN_VARIABLES {
            assert!(removed.contains(&OsString::from(name)));
        }
    }

    /// Rejects the mirror while leaving the in-memory pipe readable.
    #[derive(Debug)]
    struct RejectingSink {
        calls: AtomicUsize,
        panic: bool,
    }

    impl DiagnosticSink for RejectingSink {
        fn write(&self, _text: &str) -> io::Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            assert!(!self.panic, "mirror panic canary");
            Err(ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn failed_mirroring_does_not_stop_draining_or_lose_text() {
        for panic in [false, true] {
            let sink = RejectingSink {
                calls: AtomicUsize::new(0),
                panic,
            };
            let result = read_output(&b"first\nsecond\nlast"[..], Some(&sink));
            assert_eq!(result.text, "first\nsecond\nlast\n");
            let error = result.error.unwrap();
            if panic {
                assert!(error.find_source::<CommandFailed>().is_some());
            } else {
                assert_eq!(
                    error.find_source::<Error>().unwrap().kind(),
                    ErrorKind::BrokenPipe
                );
            }
            assert_eq!(sink.calls.load(Ordering::Relaxed), 1);
        }
    }

    /// Ends an otherwise readable in-memory stream with an independent read failure.
    struct BrokenReader;

    impl Read for BrokenReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(ErrorKind::UnexpectedEof.into())
        }
    }

    #[test]
    fn read_failure_retains_earlier_delivery_failure_and_available_text() {
        let sink = RejectingSink {
            calls: AtomicUsize::new(0),
            panic: false,
        };
        let output = read_output(b"retained\n".as_slice().chain(BrokenReader), Some(&sink));
        assert_eq!(output.text, "retained\n");
        let error = output.error.unwrap();
        assert_eq!(
            error.find_source::<Error>().unwrap().kind(),
            ErrorKind::BrokenPipe
        );
        let later = error.find_source::<FinalizationFailed>().unwrap();
        assert_eq!(
            later.cleanup.find_source::<Error>().unwrap().kind(),
            ErrorKind::UnexpectedEof
        );
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
                join_reader(reader(&b"first\nsecond"[..], None))
                    .unwrap()
                    .text,
                "first\nsecond\n"
            );
            let error = join_reader(reader(&b"\xff"[..], None))
                .unwrap()
                .error
                .unwrap();
            assert_eq!(
                error.find_source::<Error>().unwrap().kind(),
                ErrorKind::InvalidData
            );
            let error = join_reader(thread::spawn(|| panic!("fixture reader panic"))).unwrap_err();
            assert!(error.find_source::<CommandFailed>().is_some());
        });
    }
}
