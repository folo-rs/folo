// Concrete failure conditions of the tool.
//
// Each condition is its own type. They reach the caller through `ohno::AppError`, which
// keeps them in the source chain so callers can identify a failure without the crate
// having to expose a closed taxonomy.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// The input Cargo.toml file could not be read.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to read '{}'", path.display())]
pub struct ReadFileError {
    path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ReadFileError {}
impl RefUnwindSafe for ReadFileError {}

/// The rewritten Cargo.toml file could not be written.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to write '{}'", path.display())]
pub struct WriteFileError {
    path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for WriteFileError {}
impl RefUnwindSafe for WriteFileError {}

/// The input file is not valid TOML.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to parse Cargo.toml")]
pub struct ParseError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ParseError {}
impl RefUnwindSafe for ParseError {}

/// A dependency's `version` field is not a string.
#[doc(hidden)]
#[ohno::error]
#[display("Dependency '{dep}' has a non-string version field of type {actual_type}")]
pub struct UnexpectedVersionTypeError {
    // Crate-visible so that the freeze module's unit tests can assert on the attributed
    // dependency without this becoming public API surface.
    pub(crate) dep: String,
    actual_type: String,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for UnexpectedVersionTypeError {}
impl RefUnwindSafe for UnexpectedVersionTypeError {}

/// A dependency's version requirement is not valid `SemVer`.
#[doc(hidden)]
#[ohno::error]
#[display("Dependency '{dep}' has invalid version requirement '{version}'")]
pub struct InvalidVersionError {
    dep: String,
    version: String,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for InvalidVersionError {}
impl RefUnwindSafe for InvalidVersionError {}

impl InvalidVersionError {
    /// Name of the dependency whose version requirement is invalid.
    #[must_use]
    pub fn dep(&self) -> &str {
        &self.dep
    }

    /// The literal text of the rejected requirement.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::path::Path;
    use std::{error, io};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(
        ReadFileError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        WriteFileError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ParseError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        UnexpectedVersionTypeError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        InvalidVersionError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn read_file_error_display_names_operation_and_path() {
        let error = ReadFileError::caused_by(
            Path::new("some/Cargo.toml"),
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );

        let display = error.to_string();
        assert!(display.contains("read"));
        assert!(display.contains("some/Cargo.toml"));
    }

    #[test]
    fn write_file_error_display_names_operation_and_path() {
        let error = WriteFileError::caused_by(
            Path::new("some/Cargo.toml"),
            io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        );

        let display = error.to_string();
        assert!(display.contains("write"));
        assert!(display.contains("some/Cargo.toml"));
    }

    #[test]
    fn parse_error_carries_the_underlying_failure() {
        let error = ParseError::caused_by(io::Error::new(io::ErrorKind::InvalidData, "bad toml"));

        assert!(error.to_string().contains("bad toml"));
    }

    #[test]
    fn unexpected_version_type_error_display_names_dep_and_type() {
        let error = UnexpectedVersionTypeError::new("serde", "integer");

        let display = error.to_string();
        assert!(display.contains("serde"));
        assert!(display.contains("integer"));
        assert_eq!(error.dep, "serde");
    }

    #[test]
    fn invalid_version_error_display_names_dep_and_version() {
        let source = "not-a-version".parse::<semver::VersionReq>().unwrap_err();
        let error = InvalidVersionError::caused_by("serde", "not-a-version", source);

        let display = error.to_string();
        assert!(display.contains("serde"));
        assert!(display.contains("not-a-version"));
        assert_eq!(error.dep(), "serde");
        assert_eq!(error.version(), "not-a-version");
    }
}
