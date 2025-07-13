use std::error::Error;
use std::fmt::Display;

/// Indicates that a sender-receiver pair has disconnected.
#[derive(Debug)]
#[expect(clippy::exhaustive_structs, reason = "intentionally an empty struct")]
pub struct Disconnected;

impl Error for Disconnected {}

impl Display for Disconnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sender-receiver pair has disconnected")
    }
}
