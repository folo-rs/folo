//! The harness error type: a boxed trait object, since this is a throwaway
//! binary that only needs to surface a readable message and a non-zero exit.

/// A boxed, thread-safe error carrying a human-readable message.
pub(crate) type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Builds an [`Error`] from a message.
pub(crate) fn fail(message: impl Into<String>) -> Error {
    Error::from(message.into())
}
