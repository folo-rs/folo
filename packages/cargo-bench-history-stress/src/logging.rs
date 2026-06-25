//! Progress logging for the stress harness.
//!
//! Two streams of operator-facing text, both on stderr so the final results table
//! (printed to stdout by `main`) stays clean and machine-parseable:
//!
//! * [`Logger::step`] — always-on phase markers describing what the harness is
//!   about to do.
//! * [`Logger::detail`] — emitted only under `--verbose`. Each detail line is
//!   *explanatory*: it states the inputs and the reasoning behind a decision so
//!   the run can be reconstructed from the logs alone, never just the conclusion.

/// Operator-facing progress logger writing to stderr.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Logger {
    /// Whether `--verbose` detail lines are emitted.
    verbose: bool,
}

impl Logger {
    /// Creates a logger; `verbose` enables the explanatory [`detail`](Self::detail)
    /// stream.
    pub(crate) fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Emits an always-on phase marker (`==> ...`) describing the next step.
    //
    // Takes `self` for call-site symmetry with `detail` (both are `logger.x(..)`),
    // even though a phase marker is unconditional and reads no state.
    #[expect(
        clippy::unused_self,
        reason = "kept an instance method so callers use one logger handle uniformly"
    )]
    pub(crate) fn step(self, message: &str) {
        eprintln!("==> {message}");
    }

    /// Emits an explanatory detail line, only under `--verbose`.
    ///
    /// The message should state the inputs and reasoning behind a decision, not
    /// merely announce its outcome.
    pub(crate) fn detail(self, message: &str) {
        if self.verbose {
            eprintln!("    {message}");
        }
    }
}
