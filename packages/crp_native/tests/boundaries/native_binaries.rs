use std::ffi::OsStr;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crp_diag::DiagnosticSink;
use crp_native::Native;
use crp_native::command::{capture, strings};
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
