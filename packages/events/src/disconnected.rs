use std::error::Error;
use std::fmt::Display;
use std::marker::PhantomData;

/// Represents the kind of value that can be received from an event.
#[derive(Debug)]
pub(crate) enum ValueKind<T> {
    /// The event has been set to a real value.
    Real(T),

    /// The sender has been dropped without ever providing a value.
    Disconnected,
}

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
