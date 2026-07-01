//! A diagnostic-note sink that surfaces the step-by-step detail behind a command
//! when `--verbose` is set, so an otherwise-silent outcome (notably an empty
//! harvest that stores nothing) can be diagnosed.
//!
//! Notes are progress/diagnostic output and are distinct from a command's final
//! summary, which is returned as a [`RunOutcome`](crate::RunOutcome) and printed
//! to standard output. The real adapter writes notes to standard error so they
//! never contaminate machine-readable stdout; tests record them in memory.
//!
//! Stage timings are a *second*, independent channel ([`Reporter::timing`]): they
//! report the wall-clock cost of each pipeline stage so a mystery slowdown can be
//! localized. They are kept separate from per-object [`note`](Reporter::note)s
//! precisely so a caller can ask for the coarse per-stage breakdown *without* the
//! per-object flood — the stress harness, which analyzes tens of thousands of
//! objects, would otherwise drown in (and be slowed by) one note per object.

use std::time::Duration;

/// Receives human-facing diagnostic notes emitted while a command runs.
pub(crate) trait Reporter {
    /// Whether notes are consumed at all.
    ///
    /// Callers may use this to skip building an expensive note (for example, one
    /// formatted per scanned file) when nothing would consume it.
    fn enabled(&self) -> bool;

    /// Records a single diagnostic note.
    fn note(&self, message: &str);

    /// Records the wall-clock duration of a named pipeline `stage`.
    ///
    /// `stage` should name the stage *and* what it encompasses (so the breakdown
    /// reconstructs where the time went), e.g. `"phase 2/3 fetch + parallel parse +
    /// fold"`. This is a separate channel from [`note`](Reporter::note) so the
    /// per-stage cost can be surfaced without the per-object note flood.
    fn timing(&self, stage: &str, elapsed: Duration);
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

    /// Runs `body` only when notes are consumed.
    ///
    /// The counterpart to [`note_with`](Self::note_with) for a *block* that emits
    /// several notes (or computes intermediate values solely to note them): it
    /// pays nothing when `--verbose` is off, behind a single
    /// [`enabled`](Reporter::enabled) check rather than one per note. Keeping the
    /// guard here — the one place it is tested — means call sites never repeat it.
    fn if_enabled(&self, body: impl FnOnce());
}

impl<R: Reporter + ?Sized> ReporterExt for R {
    fn note_with(&self, build: impl FnOnce() -> String) {
        if self.enabled() {
            self.note(&build());
        }
    }

    fn if_enabled(&self, body: impl FnOnce()) {
        if self.enabled() {
            body();
        }
    }
}

/// A [`Reporter`] that writes notes to standard error when verbose mode is on,
/// and discards them otherwise.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StderrReporter {
    /// Whether per-object diagnostic notes are emitted.
    verbose: bool,
    /// Whether per-stage timing notes are emitted. Independent of `verbose` so a
    /// caller can ask for the timing breakdown alone.
    timing_enabled: bool,
}

impl StderrReporter {
    /// Creates a reporter that emits notes only when `verbose` is set.
    ///
    /// Stage timings follow `verbose` too, so a plain `--verbose` run gets the
    /// per-stage breakdown alongside the per-object detail.
    pub(crate) fn new(verbose: bool) -> Self {
        Self {
            verbose,
            timing_enabled: verbose,
        }
    }

    /// Creates a reporter with independent control over the per-object note stream
    /// (`verbose`) and the per-stage timing stream (`timing`), so a caller can ask
    /// for the timing breakdown without the per-object flood.
    pub(crate) fn with_timing(verbose: bool, timing: bool) -> Self {
        Self {
            verbose,
            timing_enabled: timing,
        }
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

    /// Writes the stage timing to standard error when timing is enabled.
    ///
    /// A pure standard-error side effect, untestable for the same reason as
    /// [`note`](Self::note); the `timing_enabled` flag it guards on is covered by
    /// `stderr_reporter_reports_timing_state`.
    #[cfg_attr(test, mutants::skip)]
    fn timing(&self, stage: &str, elapsed: Duration) {
        if self.timing_enabled {
            eprintln!(
                "[bench-history] timing: {stage} took {}",
                format_elapsed(elapsed)
            );
        }
    }
}

/// Formats a stage duration for a timing note: seconds (with millisecond
/// precision) at or above one second, milliseconds below that, so both a
/// multi-second load and a sub-millisecond step read naturally.
fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64();
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else {
        format!("{:.1} ms", seconds * 1000.0)
    }
}

#[cfg(test)]
pub(crate) use test_support::RecordingReporter;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_support {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::Reporter;

    /// A [`Reporter`] that records every note in memory so tests can assert on the
    /// diagnostic trail.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingReporter {
        notes: RefCell<Vec<String>>,
        timings: RefCell<Vec<String>>,
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

        /// Whether a stage timing whose label contains `needle` was recorded.
        pub(crate) fn timed(&self, needle: &str) -> bool {
            self.timings
                .borrow()
                .iter()
                .any(|stage| stage.contains(needle))
        }
    }

    impl Reporter for RecordingReporter {
        fn enabled(&self) -> bool {
            true
        }

        fn note(&self, message: &str) {
            self.notes.borrow_mut().push(message.to_owned());
        }

        fn timing(&self, stage: &str, _elapsed: Duration) {
            // Record only the stage label; the elapsed time is non-deterministic, so
            // tests assert that a stage *was* timed, not how long it took.
            self.timings.borrow_mut().push(stage.to_owned());
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
    fn stderr_reporter_reports_timing_state() {
        // `new` ties timing to verbose, so a plain `--verbose` run also gets timings.
        assert!(StderrReporter::new(true).timing_enabled);
        assert!(!StderrReporter::new(false).timing_enabled);

        // `with_timing` controls the two streams independently, so a caller can ask
        // for the timing breakdown without the per-object note flood.
        let timing_only = StderrReporter::with_timing(false, true);
        assert!(!timing_only.enabled());
        assert!(timing_only.timing_enabled);

        let notes_only = StderrReporter::with_timing(true, false);
        assert!(notes_only.enabled());
        assert!(!notes_only.timing_enabled);
    }

    #[test]
    fn recording_reporter_captures_timings_separately_from_notes() {
        let reporter = RecordingReporter::new();
        reporter.note("a per-object note");
        reporter.timing("select_dataset (full load)", Duration::from_millis(5));

        // Timings live in their own channel, so a per-object note assertion is not
        // disturbed by them and vice versa.
        assert_eq!(reporter.notes(), vec!["a per-object note".to_owned()]);
        assert!(reporter.timed("select_dataset"));
        assert!(!reporter.timed("find_changes"));
    }

    #[test]
    fn format_elapsed_uses_seconds_at_or_above_one_second() {
        assert_eq!(format_elapsed(Duration::from_secs(1)), "1.000 s");
        assert_eq!(format_elapsed(Duration::from_millis(2500)), "2.500 s");
    }

    #[test]
    fn format_elapsed_uses_milliseconds_below_one_second() {
        assert_eq!(format_elapsed(Duration::from_millis(250)), "250.0 ms");
        assert_eq!(format_elapsed(Duration::ZERO), "0.0 ms");
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

    #[test]
    fn if_enabled_runs_the_block_only_when_enabled() {
        use std::cell::Cell;

        // A disabled reporter must never run the block, so a multi-note diagnostic
        // section behind one guard costs nothing when `--verbose` is off.
        let ran = Cell::new(0_u32);
        StderrReporter::new(false).if_enabled(|| ran.set(ran.get() + 1));
        assert_eq!(ran.get(), 0);

        // An enabled reporter runs the block, which can emit several notes.
        let recording = RecordingReporter::new();
        recording.if_enabled(|| {
            ran.set(ran.get() + 1);
            recording.note("first");
            recording.note("second");
        });
        assert_eq!(ran.get(), 1);
        assert_eq!(
            recording.notes(),
            vec!["first".to_owned(), "second".to_owned()]
        );
    }
}
