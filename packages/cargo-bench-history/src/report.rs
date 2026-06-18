//! A diagnostic-note sink that surfaces the step-by-step detail behind a command
//! when `--verbose` is set, so an otherwise-silent outcome (notably an empty
//! harvest that stores nothing) can be diagnosed.
//!
//! Notes are progress/diagnostic output and are distinct from a command's final
//! summary, which is returned as a [`RunOutcome`](crate::RunOutcome) and printed
//! to standard output. The real adapter writes notes to standard error so they
//! never contaminate machine-readable stdout; tests record them in memory.

/// Receives human-facing diagnostic notes emitted while a command runs.
pub(crate) trait Reporter {
    /// Whether notes are consumed at all.
    ///
    /// Callers may use this to skip building an expensive note (for example, one
    /// formatted per scanned file) when nothing would consume it.
    fn enabled(&self) -> bool;

    /// Records a single diagnostic note.
    fn note(&self, message: &str);
}

/// A [`Reporter`] that writes notes to standard error when verbose mode is on,
/// and discards them otherwise.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StderrReporter {
    verbose: bool,
}

impl StderrReporter {
    /// Creates a reporter that emits notes only when `verbose` is set.
    pub(crate) fn new(verbose: bool) -> Self {
        Self { verbose }
    }
}

impl Reporter for StderrReporter {
    fn enabled(&self) -> bool {
        self.verbose
    }

    fn note(&self, message: &str) {
        if self.verbose {
            eprintln!("[bench-history] {message}");
        }
    }
}

#[cfg(test)]
pub(crate) use test_support::RecordingReporter;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_support {
    use std::cell::RefCell;

    use super::Reporter;

    /// A [`Reporter`] that records every note in memory so tests can assert on the
    /// diagnostic trail.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingReporter {
        notes: RefCell<Vec<String>>,
    }

    impl RecordingReporter {
        /// Creates an empty recording reporter.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Returns a snapshot of the notes recorded so far.
        pub(crate) fn notes(&self) -> Vec<String> {
            self.notes.borrow().clone()
        }

        /// Whether any recorded note contains `needle`.
        pub(crate) fn contains(&self, needle: &str) -> bool {
            self.notes.borrow().iter().any(|note| note.contains(needle))
        }
    }

    impl Reporter for RecordingReporter {
        fn enabled(&self) -> bool {
            true
        }

        fn note(&self, message: &str) {
            self.notes.borrow_mut().push(message.to_owned());
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn stderr_reporter_reports_enabled_state() {
        assert!(StderrReporter::new(true).enabled());
        assert!(!StderrReporter::new(false).enabled());
    }

    #[test]
    fn recording_reporter_captures_notes() {
        let reporter = RecordingReporter::new();
        assert!(reporter.enabled());
        reporter.note("scanning target/criterion");
        reporter.note("excluding stale.json");

        assert_eq!(
            reporter.notes(),
            vec![
                "scanning target/criterion".to_owned(),
                "excluding stale.json".to_owned()
            ]
        );
        assert!(reporter.contains("stale"));
        assert!(!reporter.contains("missing"));
    }
}
