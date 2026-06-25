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

/// Lazy-formatting helpers available on every [`Reporter`], including
/// `&dyn Reporter`.
///
/// These live on a separate trait rather than on [`Reporter`] itself so the base
/// trait stays dyn-compatible: a method taking a generic closure cannot be
/// dispatched through `&dyn Reporter`. A blanket impl makes the helpers available
/// on every reporter regardless.
pub(crate) trait ReporterExt {
    /// Records a diagnostic note whose formatting runs only when notes are
    /// consumed.
    ///
    /// The `build` closure — typically an allocating `format!` — is invoked only
    /// when [`enabled`](Reporter::enabled) is true. On a hot path that emits one
    /// note per scanned object this avoids building (and immediately discarding) a
    /// string for every object when `--verbose` is off, without sprinkling an
    /// `if reporter.enabled()` guard around each call site.
    fn note_with(&self, build: impl FnOnce() -> String);
}

impl<R: Reporter + ?Sized> ReporterExt for R {
    fn note_with(&self, build: impl FnOnce() -> String) {
        if self.enabled() {
            self.note(&build());
        }
    }
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

    /// Writes the note to standard error in verbose mode.
    ///
    /// A pure standard-error side effect with no return value or observable state,
    /// so a mutation of its body cannot be caught by a test; the `verbose` flag it
    /// guards on is covered by `stderr_reporter_reports_enabled_state`.
    #[cfg_attr(test, mutants::skip)]
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

    #[test]
    fn note_with_skips_formatting_when_disabled() {
        use std::cell::Cell;

        // A disabled reporter must never run the formatting closure, so a per-object
        // note costs nothing when nothing will consume it.
        let built = Cell::new(0_u32);
        StderrReporter::new(false).note_with(|| {
            built.set(built.get() + 1);
            "discarded".to_owned()
        });
        assert_eq!(built.get(), 0);

        // An enabled reporter runs the closure and records the resulting note.
        let recording = RecordingReporter::new();
        recording.note_with(|| {
            built.set(built.get() + 1);
            "lazy note".to_owned()
        });
        assert_eq!(built.get(), 1);
        assert!(recording.contains("lazy note"));
    }
}
