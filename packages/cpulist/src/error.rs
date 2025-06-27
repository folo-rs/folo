use thiserror::Error;

/// Errors that can occur when processing cpulist strings.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The caller provided a supposed cpulist string but it did not match the expected format.
    #[error("invalid cpulist syntax: '{invalid_value}' is invalid: {problem}")]
    InvalidSyntax {
        /// The specific value that was invalid. This may either be the entire cpulist string
        /// or a specific part of it, depending on the problem.
        invalid_value: String,

        /// A human-readable description of the problem.
        problem: String,
    },
}

/// A specialized `Result` type for cpulist operations, returning the crate's
/// [`Error`] type as the error value.
pub(crate) type Result<T> = std::result::Result<T, Error>;
