use std::error::Error;
use std::fmt::{self, Display};

/// Indicates that a sender-receiver pair has disconnected.
#[derive(Debug, Eq, PartialEq)]
#[expect(clippy::exhaustive_structs, reason = "intentionally an empty struct")]
pub struct Disconnected;

impl Error for Disconnected {}

impl Display for Disconnected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sender-receiver pair has disconnected")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn disconnected_display_writes_message() {
        let disconnected = Disconnected;
        let display_output = disconnected.to_string();

        // Verify that the display output is not empty.
        assert!(!display_output.is_empty());
    }
}
