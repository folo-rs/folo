use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crp_diag::DiagnosticSink;
use crp_native::command::{capture, strings};
use crp_native::{BuildRequest, Native, SourceProvider};
use crp_workspace::testing::{Repository, with_io_slot};
use ohno::AppError;
use tempfile::TempDir;

/// Observes process diagnostics independently of global stderr.
#[derive(Debug, Default)]
struct Recording(Mutex<Vec<String>>);

impl DiagnosticSink for Recording {
    fn write(&self, text: &str) -> io::Result<()> {
        self.0.lock().unwrap().push(text.to_owned());
        Ok(())
    }
}

/// Makes diagnostics unavailable only after native worktree acquisition has completed.
#[derive(Debug)]
struct ClosingSink {
    closed: AtomicBool,
    panic: bool,
}

impl DiagnosticSink for ClosingSink {
    fn write(&self, _text: &str) -> io::Result<()> {
        if self.closed.load(Ordering::Relaxed) {
            assert!(!self.panic, "diagnostic canary");
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

/// Requires the fixture's local commit without permitting network source acquisition.
struct LocalSource;

impl SourceProvider for LocalSource {
    fn fetch(&self, _controller: &Path, _commit: &str, _deadline: Instant) -> Result<(), AppError> {
        panic!("the fixture commit is already present locally")
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "creates and disposes real Git worktrees with Cargo metadata"
)]
fn dropped_preparation_cleans_worktrees_when_diagnostics_are_unavailable() {
    with_io_slot(|| {
        // Real Git/Cargo startup is outside the ordinary synchronization-test timeout.
        testing::with_watchdog_timeout(Duration::from_mins(5), || {
            for panic in [false, true] {
                let repository = Repository::new();
                repository.write(
                    "Cargo.toml",
                    b"[workspace]\n[package]\nname='fixture'\nversion='1.0.0'\nedition='2024'\n",
                );
                repository.write("src/main.rs", b"fn main() {}\n");
                repository.write(
                    "Cargo.lock",
                    b"version = 4\n[[package]]\nname = 'fixture'\nversion = '1.0.0'\n",
                );
                repository.command(&["add", "."]);
                repository.command(&["commit", "--quiet", "-m", "native cleanup fixture"]);
                let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
                let artifacts = TempDir::new().unwrap();
                let sink = Arc::new(ClosingSink {
                    closed: AtomicBool::new(false),
                    panic,
                });
                let diagnostics: Arc<dyn DiagnosticSink> = Arc::<ClosingSink>::clone(&sink);
                let mut native = Native::new(
                    repository.path().to_path_buf(),
                    artifacts.path().to_path_buf(),
                    // Preparation stops at the missing source toolchain, before checking its host.
                    "native".to_owned(),
                    Box::new(LocalSource),
                    diagnostics,
                )
                .unwrap();
                let request = BuildRequest::new(
                    "fixture".to_owned(),
                    "fixture".to_owned(),
                    "1.0.0".to_owned(),
                    "fixture-v1.0.0".to_owned(),
                    source,
                    "fixture-native".to_owned(),
                )
                .unwrap();
                native.prepare(&request).unwrap_err();
                assert_eq!(
                    repository
                        .command(&["worktree", "list", "--porcelain"])
                        .matches("worktree ")
                        .count(),
                    2,
                );
                sink.closed.store(true, Ordering::Relaxed);
                drop(native);
                assert_eq!(
                    repository
                        .command(&["worktree", "list", "--porcelain"])
                        .matches("worktree ")
                        .count(),
                    1,
                );
            }
        });
    });
}

#[test]
#[cfg_attr(miri, ignore = "starts supervised Git processes in an owned directory")]
fn captured_stdout_and_streamed_diagnostics_stay_separate() {
    testing::with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let recording = Arc::new(Recording::default());
        let sink: Arc<dyn DiagnosticSink> = Arc::<Recording>::clone(&recording);
        // The execution deadline is a last-chance bound, never a test failure assertion.
        let deadline = Native::deadline_after(Duration::from_mins(60));
        let output = capture(
            OsStr::new("git"),
            &strings(&["--version"]),
            directory.path(),
            &[],
            &sink,
            deadline,
        )
        .unwrap();
        assert!(output.starts_with("git version "));
        assert!(
            !recording
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains("git version "))
        );
        let error = capture(
            OsStr::new("git"),
            &strings(&["--not-a-git-option"]),
            directory.path(),
            &[],
            &sink,
            deadline,
        )
        .unwrap_err();
        assert!(error.to_string().contains("--not-a-git-option"));
        assert!(recording.0.lock().unwrap().len() > 2);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "attempts a native process in an absent working directory"
)]
fn process_start_failure_preserves_the_foreign_cause() {
    let directory = TempDir::new().unwrap();
    let sink: Arc<dyn DiagnosticSink> = Arc::new(crp_diag::Discard);
    let error = capture(
        OsStr::new("git"),
        &strings(&["--version"]),
        &directory.path().join("absent"),
        &[],
        &sink,
        Native::deadline_after(Duration::from_mins(60)),
    )
    .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
}
