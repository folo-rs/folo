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
