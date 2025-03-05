use thiserror::Error;

/// Errors that can occur when processing cpulist strings.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid cpulist syntax: '{invalid_value}' is invalid: {problem}")]
    InvalidSyntax {
        invalid_value: String,
        problem: String,
    },
}

/// A specialized `Result` type for cpulist operations, returning the crate's
/// [`Error`][crate::Error] type as the error value.
pub(crate) type Result<T> = std::result::Result<T, crate::Error>;
