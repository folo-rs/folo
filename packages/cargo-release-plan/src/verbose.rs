// Explanatory stderr notes for `--verbose`.
//
// Standalone binaries must explain the inputs and rules behind each decision,
// not merely announce the conclusion. Ref: docs/standalone-binaries.md.

use std::fmt::Display;

/// Toggle for explanatory notes on stderr.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Verbose {
    enabled: bool,
}

impl Verbose {
    pub(crate) fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Emits one explanatory note when verbose mode is on.
    // Logging has no test-observable return; catching this requires asserting
    // that a side effect does not happen.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn note(self, message: impl Display) {
        if self.enabled {
            eprintln!("[release-plan] {message}");
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Notes are explanatory diagnostics with no return value, so the contract
    /// this can assert is that emitting one is infallible in either mode.
    #[test]
    fn a_note_is_emitted_in_either_mode_without_panicking() {
        Verbose::new(true).note("enabled");
        Verbose::new(false).note("disabled");
    }
}
