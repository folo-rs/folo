//! Progress logging for the stress harness.
//!
//! Two streams of operator-facing text, both on stderr so the final results table
//! (printed to stdout by `main`) stays clean and machine-parseable:
//!
//! * [`Logger::step`] — always-on phase markers describing what the harness is
//!   about to do.
//! * [`Logger::detail_with`] — emitted only under `--verbose`, formatted lazily.
//!   Each detail line is *explanatory*: it states the inputs and the reasoning
//!   behind a decision so the run can be reconstructed from the logs alone, never
//!   just the conclusion.

/// Operator-facing progress logger writing to stderr.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Logger {
    /// Whether `--verbose` detail lines are emitted.
    verbose: bool,
}

impl Logger {
    /// Creates a logger; `verbose` enables the explanatory
    /// [`detail_with`](Self::detail_with) stream.
    pub(crate) fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Emits an always-on phase marker (`==> ...`) describing the next step.
    //
    // Takes `self` for call-site symmetry with `detail_with` (both are
    // `logger.x(..)`), even though a phase marker is unconditional and reads no
    // state.
    #[expect(
        clippy::unused_self,
        reason = "kept an instance method so callers use one logger handle uniformly"
    )]
    pub(crate) fn step(self, message: &str) {
        eprintln!("==> {message}");
    }

    /// Whether `--verbose` is enabled. The harness threads this into the analyze
    /// options' `timing` flag so a verbose run surfaces the per-stage breakdown
    /// without the per-object note flood (tens of thousands of objects).
    pub(crate) fn verbose(self) -> bool {
        self.verbose
    }

    /// Emits an explanatory detail line, only under `--verbose`, formatting it
    /// lazily so a non-verbose run pays nothing.
    ///
    /// The message should state the inputs and reasoning behind a decision, not
    /// merely announce its outcome. The `build` closure — typically an allocating
    /// `format!` — runs only when `--verbose` is set, so an unconditional emit is
    /// never reachable at a call site without its guard.
    pub(crate) fn detail_with(self, build: impl FnOnce() -> String) {
        if self.verbose {
            eprintln!("    {}", build());
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::Logger;

    #[test]
    fn verbose_reports_the_constructed_flag() {
        assert!(Logger::new(true).verbose());
        assert!(!Logger::new(false).verbose());
    }
}
