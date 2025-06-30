use std::error::Error;
use std::fmt::Display;
use std::marker::PhantomData;

/// Indicates that a sender-receiver pair has disconnected.
#[derive(Debug)]
pub struct Disconnected {
    _private: PhantomData<()>,
}

impl Disconnected {
    pub(crate) fn new() -> Self {
        Self {
            _private: PhantomData,
        }
    }
}

impl Error for Disconnected {}

impl Display for Disconnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sender-receiver pair has disconnected")
    }
}
