// Private failure conditions of the tool.
//
// Each condition reaches the application boundary through `ohno::AppError`.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// The input Cargo.toml file could not be read.
#[ohno::error]
#[display("Failed to read '{}'", path.display())]
pub(crate) struct ReadFileError {
    path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ReadFileError {}
impl RefUnwindSafe for ReadFileError {}

/// The rewritten Cargo.toml file could not be written.
#[ohno::error]
#[display("Failed to write '{}'", path.display())]
pub(crate) struct WriteFileError {
    path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for WriteFileError {}
impl RefUnwindSafe for WriteFileError {}

/// The input Cargo.toml file is not valid TOML.
#[ohno::error]
#[display("Failed to parse '{}'", path.display())]
pub(crate) struct ParseError {
    pub(crate) path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ParseError {}
impl RefUnwindSafe for ParseError {}

/// A dependency's `version` field is not a string.
#[ohno::error]
#[display("Dependency '{dep}' has a non-string version field of type {actual_type}")]
pub(crate) struct UnexpectedVersionTypeError {
    dep: String,
    actual_type: String,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for UnexpectedVersionTypeError {}
impl RefUnwindSafe for UnexpectedVersionTypeError {}

impl UnexpectedVersionTypeError {
    /// Name of the dependency whose version field is malformed.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn dep(&self) -> &str {
        &self.dep
    }

    /// The TOML type found in the dependency's `version` field.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn actual_type(&self) -> &str {
        &self.actual_type
    }
}

/// A dependency's version requirement is not valid `SemVer`.
#[ohno::error]
#[display("Dependency '{dep}' has invalid version requirement '{version}'")]
pub(crate) struct InvalidVersionError {
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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn dep(&self) -> &str {
        &self.dep
    }

    /// The literal text of the rejected requirement.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn version(&self) -> &str {
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

    use ohno::ErrorExt as _;
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
    fn read_file_error_carries_path_and_source() {
        let error = ReadFileError::caused_by(
            Path::new("some/Cargo.toml"),
            io::Error::new(io::ErrorKind::NotFound, "missing"),
        );

        assert_eq!(error.path, Path::new("some/Cargo.toml"));
        assert!(error.message().contains("some/Cargo.toml"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn write_file_error_carries_path_and_source() {
        let error = WriteFileError::caused_by(
            Path::new("some/Cargo.toml"),
            io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        );

        assert_eq!(error.path, Path::new("some/Cargo.toml"));
        assert!(error.message().contains("some/Cargo.toml"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn parse_error_carries_path_and_source() {
        let source = "this is = not [valid toml"
            .parse::<toml_edit::DocumentMut>()
            .unwrap_err();
        let error = ParseError::caused_by(Path::new("some/Cargo.toml"), source);

        assert_eq!(error.path, Path::new("some/Cargo.toml"));
        assert!(error.message().contains("some/Cargo.toml"));
        assert!(error.find_source::<toml_edit::TomlError>().is_some());
    }

    #[test]
    fn unexpected_version_type_error_carries_dep_and_type() {
        let error = UnexpectedVersionTypeError::new("serde", "integer");

        assert_eq!(error.dep(), "serde");
        assert_eq!(error.actual_type(), "integer");
        assert!(error.message().contains("serde"));
        assert!(error.message().contains("integer"));
    }

    #[test]
    fn invalid_version_error_carries_dep_version_and_source() {
        let source = "not-a-version".parse::<semver::VersionReq>().unwrap_err();
        let error = InvalidVersionError::caused_by("serde", "not-a-version", source);

        assert_eq!(error.dep(), "serde");
        assert_eq!(error.version(), "not-a-version");
        assert!(error.message().contains("serde"));
        assert!(error.message().contains("not-a-version"));
        assert!(error.find_source::<semver::Error>().is_some());
    }
}
