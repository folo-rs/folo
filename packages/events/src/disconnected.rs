use std::error::Error;
use std::fmt::Display;

/// Indicates that a sender-receiver pair has disconnected.
#[derive(Debug)]
#[expect(clippy::exhaustive_structs, reason = "intentionally an empty struct")]
pub struct Disconnected;

impl Error for Disconnected {}

impl Display for Disconnected {
    #[cfg_attr(test, mutants::skip)] // No API contract for error message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sender-receiver pair has disconnected")
    }
}
