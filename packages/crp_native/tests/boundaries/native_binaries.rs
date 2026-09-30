use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
#[cfg(feature = "private-test-util")]
use std::mem;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, io};

use crp_diag::DiagnosticSink;
#[cfg(feature = "private-test-util")]
use crp_native::__private::write_archive_for_test;
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

/// Accepts the launch record, then refuses the secondary stderr mirror.
#[derive(Debug, Default)]
struct FailedMirror(AtomicUsize);

impl DiagnosticSink for FailedMirror {
    fn write(&self, _text: &str) -> io::Result<()> {
        if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(())
        } else {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compiles and supervises a real process with output pipes"
)]
fn failed_stderr_delivery_retains_complete_native_output() {
    with_io_slot(|| {
        testing::with_watchdog_timeout(Duration::from_mins(5), || {
            let directory = TempDir::new().unwrap();
            let source = directory.path().join("output.rs");
            let executable = directory.path().join(format!("output{EXE_SUFFIX}"));
            fs::write(
                &source,
                r#"fn main() {
                println!("stdout canary");
                eprintln!("first stderr canary");
                eprintln!("last stderr canary");
            }"#,
            )
            .unwrap();
            let discard: Arc<dyn DiagnosticSink> = Arc::new(crp_diag::Discard);
            capture(
                OsStr::new("rustc"),
                &[
                    "--edition=2024".into(),
                    source.as_os_str().to_owned(),
                    "-o".into(),
                    executable.as_os_str().to_owned(),
                ],
                directory.path(),
                &[],
                &discard,
                Native::deadline_after(Duration::from_mins(60)),
            )
            .unwrap();
            let recording = Arc::new(FailedMirror::default());
            let sink: Arc<dyn DiagnosticSink> = Arc::<FailedMirror>::clone(&recording);
            let error = capture(
                executable.as_os_str(),
                &[],
                directory.path(),
                &[],
                &sink,
                Native::deadline_after(Duration::from_mins(60)),
            )
            .unwrap_err();
            let diagnostic = error.to_string();
            for canary in ["stdout canary", "first stderr canary", "last stderr canary"] {
                assert!(diagnostic.contains(canary));
            }
            // The launch is delivered, the first mirrored line fails, and later lines are only drained.
            assert_eq!(recording.0.load(Ordering::Relaxed), 2);
        });
    });
}

/// Requires the fixture's local commit without permitting network source acquisition.
struct LocalSource;

impl SourceProvider for LocalSource {
    fn fetch(&self, _controller: &Path, _commit: &str, _deadline: Instant) -> Result<(), AppError> {
        panic!("the fixture commit is already present locally")
    }
}

#[test]
#[cfg(feature = "private-test-util")]
#[cfg_attr(miri, ignore = "writes and inspects real archive files")]
fn interrupted_archive_never_reaches_the_final_asset_path() {
    let directory = TempDir::new().unwrap();
    let executable = directory.path().join("program");
    let archive = directory.path().join("program.zip");
    fs::write(&executable, b"streaming archive input").unwrap();
    let mut started = false;
    let mut interrupted = false;
    write_archive_for_test(&executable, &archive, || {
        // Allow the initial check, then interrupt after the entry header starts.
        // Buffered file lengths are not a portable signal for that write boundary.
        if mem::replace(&mut started, true) {
            interrupted = true;
            Err(io::Error::other("interrupted archive").into())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(interrupted);
    assert!(!archive.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(miri, ignore = "builds a native executable and writes archives")]
fn packaging_failure_does_not_create_a_checksum_or_replace_existing_outputs() {
    with_io_slot(|| {
        // Compilation is native I/O; the watchdog is not an archive timing assertion.
        testing::with_watchdog_timeout(Duration::from_mins(5), || {
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
            repository.write(".gitignore", b"/target\n");
            repository.write(
                "rust-toolchain.toml",
                include_bytes!("../../../../rust-toolchain.toml"),
            );
            repository.command(&["add", "."]);
            repository.command(&["commit", "--quiet", "-m", "archive fixture"]);
            let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
            let rustc = Command::new("rustc")
                .args(["--version", "--verbose"])
                .current_dir(repository.path())
                .output()
                .unwrap();
            assert!(rustc.status.success());
            let triple = String::from_utf8(rustc.stdout)
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("host: "))
                .unwrap()
                .to_owned();
            let output = TempDir::new().unwrap();
            let mut native = Native::new(
                repository.path().to_owned(),
                output.path().to_owned(),
                triple,
                Box::new(LocalSource),
                Arc::new(crp_diag::Discard),
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
            native.prepare(&request).unwrap();
            native.build(&request).unwrap();
            let artifacts = native.artifacts().unwrap();
            let archive = artifacts.archive.to_owned();
            let checksum = artifacts.checksum.to_owned();
            fs::create_dir_all(&archive).unwrap();
            native.package(&request).unwrap_err();
            assert!(!checksum.exists());
            assert!(archive.is_dir());
            fs::remove_dir(&archive).unwrap();

            fs::write(&checksum, b"existing checksum").unwrap();
            native.package(&request).unwrap_err();
            assert_eq!(fs::read(&checksum).unwrap(), b"existing checksum");
            assert!(archive.is_file());
            native.cleanup().unwrap();
            assert_eq!(
                repository
                    .command(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        });
    });
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
        assert!(
            recording.0.lock().unwrap().iter().any(|line| {
                !line.starts_with("Running ") && line.contains("--not-a-git-option")
            })
        );
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
