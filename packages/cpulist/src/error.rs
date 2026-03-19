use std::panic::{RefUnwindSafe, UnwindSafe};

/// Errors that can occur when processing cpulist strings.
///
/// The caller provided a supposed cpulist string but it did not match the expected format.
#[ohno::error]
#[display("invalid cpulist syntax: '{invalid_value}' is invalid: {problem}")]
pub struct Error {
    invalid_value: String,
    problem: String,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for Error {}
impl RefUnwindSafe for Error {}

impl Error {
    /// The specific value that was invalid.
    ///
    /// This may either be the entire cpulist
    /// string or a specific part of it, depending on the problem.
    #[must_use]
    pub fn invalid_value(&self) -> &str {
        &self.invalid_value
    }

    /// A human-readable description of the problem.
    #[must_use]
    pub fn problem(&self) -> &str {
        &self.problem
    }
}

/// A specialized `Result` type for cpulist operations, returning the crate's
/// [`Error`] type as the error value.
pub(crate) type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error;
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Error: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn invalid_syntax_is_error() {
        let error = Error::new("abc".to_string(), "not a number".to_string());

        assert_eq!(error.invalid_value(), "abc");
        assert_eq!(error.problem(), "not a number");

        // Verify it is a valid Error that can be used in Result context.
        let result: Result<()> = Err(error);
        assert!(result.is_err());
    }
}
