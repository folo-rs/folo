// Explanatory stderr notes for `--verbose`.
//
// Standalone binaries must explain the inputs and rules behind each decision,
// not merely announce the conclusion. Ref: docs/standalone-binaries.md.

use std::io;
use std::io::Write as _;

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
    ///
    /// The caller supplies a closure rather than a finished message so that an
    /// ordinary run pays nothing for the formatting, path quoting, and listing
    /// that explanatory notes need in order to be reconstructible.
    // Logging has no test-observable return; catching this requires asserting
    // that a side effect does not happen.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn note(self, message: impl FnOnce() -> String) {
        if !self.enabled {
            return;
        }
        // A note is a diagnostic, not a result. A closed or full stderr must not
        // abort a run that would otherwise succeed, which is what the `println`
        // family does on a write failure. Assembling the whole line first also
        // keeps it a single write, so notes cannot interleave mid-line.
        let line = format!("[release-plan] {}\n", message());
        drop(io::stderr().lock().write_all(line.as_bytes()));
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
        Verbose::new(true).note(|| "enabled".to_string());
        Verbose::new(false).note(|| "disabled".to_string());
    }

    /// A disabled toggle must not run the closure, since building an
    /// explanation is the cost the closure exists to avoid.
    #[test]
    fn a_disabled_note_does_not_build_its_message() {
        let mut built = false;
        Verbose::new(false).note(|| {
            built = true;
            String::new()
        });
        assert!(!built);
    }
}
