use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write, stderr};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use command_group::CommandGroup;
use ohno::AppError;

/// Native command failures retain command identity and process diagnostics.
#[ohno::error]
#[display("{operation}: {diagnostic}")]
pub(crate) struct CommandFailed {
    operation: String,
    diagnostic: String,
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

// Polling only supervises real child processes; unit decisions do not depend on wall-clock time.
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
    deadline: Instant,
) -> Result<String, AppError> {
    capture_controlled(program, arguments, directory, environment, deadline, true)
}

// Cleanup remains available after cancellation, with its own bounded deadline.
#[cfg_attr(test, mutants::skip)]
pub fn capture_cleanup(
    arguments: &[OsString],
    directory: &Path,
    deadline: Instant,
) -> Result<String, AppError> {
    capture_controlled(
        OsStr::new("git"),
        arguments,
        directory,
        &[],
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
    deadline: Instant,
    cancellable: bool,
) -> Result<String, AppError> {
    if let Some(reason) = interruption(Instant::now() >= deadline, cancellable && cancelled()) {
        return Err(CommandFailed::new(program.display().to_string(), reason.to_owned()).into());
    }
    eprintln!(
        "Running {} {:?} in {}",
        program.display(),
        arguments,
        directory.display()
    );
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
    let stdout = reader(stdout, false);
    let stderr = reader(stderr, true);
    let mut interrupted = None;
    let mut status = None;
    loop {
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    CANCELLED.store(true, Ordering::Relaxed);
                    child.kill()?;
                    child.wait()?;
                    return Err(error.into());
                }
            };
        }
        if status.is_some() && stdout.is_finished() && stderr.is_finished() {
            break;
        }
        interrupted = interruption(Instant::now() >= deadline, cancellable && cancelled());
        if interrupted.is_some() {
            status = Some(
                child
                    .kill()
                    .and_then(|()| child.wait())
                    .inspect_err(|_error| {
                        CANCELLED.store(true, Ordering::Relaxed);
                    })?,
            );
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    let status = status.expect("normal completion or termination supplies the child status");
    let stdout = join_reader(stdout)?;
    let stderr = join_reader(stderr)?;
    if interrupted.is_some() || !status.success() {
        return Err(CommandFailed::new(
            program.display().to_string(),
            format!("exit {status}; interruption={interrupted:?}\n{stdout}\n{stderr}"),
        )
        .into());
    }
    Ok(stdout)
}

fn interruption(deadline_reached: bool, cancelled: bool) -> Option<&'static str> {
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
fn reader(pipe: impl Read + Send + 'static, stream: bool) -> JoinHandle<std::io::Result<String>> {
    thread::spawn(move || {
        let mut output = String::new();
        for line in BufReader::new(pipe).lines() {
            let line = line?;
            if stream {
                writeln!(stderr(), "{line}")?;
            }
            output.push_str(&line);
            output.push('\n');
        }
        Ok(output)
    })
}

// Reader failures remain errors, including unexpected reader-thread panics.
fn join_reader(reader: JoinHandle<std::io::Result<String>>) -> Result<String, AppError> {
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
    fn reader_results_preserve_text_and_propagate_errors() {
        testing::with_watchdog(|| {
            assert_eq!(
                join_reader(reader(&b"first\nsecond"[..], false)).unwrap(),
                "first\nsecond\n"
            );
            let error = join_reader(reader(&b"\xff"[..], false)).unwrap_err();
            assert_eq!(
                error.find_source::<Error>().unwrap().kind(),
                ErrorKind::InvalidData
            );
            let error = join_reader(thread::spawn(|| panic!("fixture reader panic"))).unwrap_err();
            assert!(error.find_source::<CommandFailed>().is_some());
        });
    }
}

/// Constructs literal subprocess arguments without changing the environment or running a command.
pub fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
