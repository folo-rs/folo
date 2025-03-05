use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid cpulist syntax: '{invalid_value}' is invalid: {problem}")]
    InvalidSyntax {
        invalid_value: String,
        problem: String,
    },
}

pub type Result<T> = std::result::Result<T, crate::Error>;
