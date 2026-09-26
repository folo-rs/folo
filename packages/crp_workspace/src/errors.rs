// Failure conditions owned by this application's component. Leaves remain implementation details.
// The immutable ohno source chains permit shared observation across unwind boundaries.
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

use crp_diag::Quotable as _;
/// An OS-level failure prevented starting or communicating with a helper process.
#[ohno::error]
#[display("I/O failure while executing `{program}`")]
pub(crate) struct CommandIoError {
    program: String,
}

impl UnwindSafe for CommandIoError {}
impl RefUnwindSafe for CommandIoError {}

/// A helper process exited unsuccessfully.
#[ohno::error]
#[display("`{program}` exited with status {status}: {stderr}")]
pub(crate) struct CommandFailedError {
    program: String,
    status: String,
    stderr: String,
}

impl UnwindSafe for CommandFailedError {}
impl RefUnwindSafe for CommandFailedError {}

impl CommandFailedError {
    #[must_use]
    pub(crate) fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// Git reported a path that is not valid UTF-8.
///
/// A file name on Unix is an arbitrary byte string, so this is reachable for a
/// tracked file rather than only for a corrupt repository. Decoding such a name
/// lossily would replace the offending bytes, which can collapse two distinct
/// paths into one entry and makes every later `git show` of that path address a
/// different file, so the run stops instead. The name is rendered lossily for
/// the operator's benefit only.
#[ohno::error]
#[display("Git reported a path that is not valid UTF-8: '{}'", path.quoted())]
pub(crate) struct NonUtf8PathError {
    path: String,
}

impl UnwindSafe for NonUtf8PathError {}
impl RefUnwindSafe for NonUtf8PathError {}

/// A blob recorded in history is not valid UTF-8.
///
/// Every blob this tool reads as text is a Cargo manifest, and Cargo requires
/// UTF-8. Replacing invalid bytes could yield a parseable document Git does not
/// store, so the run stops instead.
#[ohno::error]
#[display(
    "Blob '{}:{}' is not valid UTF-8",
    commit.quoted(),
    path.quoted()
)]
pub(crate) struct NonUtf8BlobError {
    commit: String,
    path: String,
}

impl UnwindSafe for NonUtf8BlobError {}
impl RefUnwindSafe for NonUtf8BlobError {}

/// A single path does not fit the platform command-line budget.
#[ohno::error]
#[display(
    "Path '{}' is too long to pass to a 'git' subprocess",
    path.quoted()
)]
pub(crate) struct PathTooLongError {
    path: String,
}

impl UnwindSafe for PathTooLongError {}
impl RefUnwindSafe for PathTooLongError {}

/// A file could not be read.
#[ohno::error]
#[display("Failed to read '{}'", path.quoted())]
pub(crate) struct ReadFileError {
    path: PathBuf,
}

impl UnwindSafe for ReadFileError {}
impl RefUnwindSafe for ReadFileError {}

/// A file could not be written.
#[ohno::error]
#[display("Failed to write '{}'", path.quoted())]
pub(crate) struct WriteFileError {
    path: PathBuf,
}

impl UnwindSafe for WriteFileError {}
impl RefUnwindSafe for WriteFileError {}

/// A TOML document is not valid.
#[ohno::error]
#[display("Failed to parse '{}'", path.quoted())]
pub(crate) struct ParseTomlError {
    path: PathBuf,
}

impl UnwindSafe for ParseTomlError {}
impl RefUnwindSafe for ParseTomlError {}

/// `cargo metadata` JSON is not valid.
#[ohno::error]
#[display("Failed to parse cargo metadata JSON")]
pub(crate) struct ParseMetadataError;

impl UnwindSafe for ParseMetadataError {}
impl RefUnwindSafe for ParseMetadataError {}

/// A version string is not valid `SemVer`.
#[ohno::error]
#[display(
    "Invalid version '{}' for '{}'",
    version.quoted(),
    name.quoted()
)]
pub(crate) struct InvalidVersionError {
    name: String,
    version: String,
}

impl UnwindSafe for InvalidVersionError {}
impl RefUnwindSafe for InvalidVersionError {}

/// The base revision could not be resolved.
#[ohno::error]
#[display("Failed to resolve base revision '{}'", rev.quoted())]
pub(crate) struct UnresolvedBaseError {
    rev: String,
}

impl UnwindSafe for UnresolvedBaseError {}
impl RefUnwindSafe for UnresolvedBaseError {}

/// The current workspace still declares the obsolete manual group key.
#[ohno::error]
#[display(
    "Workspace metadata key 'release-plan.groups' is obsolete; declare version-group membership with exact '=major.minor.patch' workspace dependencies"
)]
pub(crate) struct LegacyVersionGroupsError {}

impl UnwindSafe for LegacyVersionGroupsError {}
impl RefUnwindSafe for LegacyVersionGroupsError {}

/// An in-workspace exact dependency does not use the supported plain triplet.
#[ohno::error]
#[display(
    "Dependency '{}' in {} of '{}' uses unsupported exact requirement '{}'; use one '=major.minor.patch' comparator",
    dependency.quoted(),
    location.quoted(),
    manifest.quoted(),
    requirement.quoted()
)]
pub(crate) struct UnsupportedExactRequirementError {
    manifest: String,
    dependency: String,
    location: String,
    requirement: String,
}

impl UnwindSafe for UnsupportedExactRequirementError {}
impl RefUnwindSafe for UnsupportedExactRequirementError {}

/// An `include` / `exclude` pattern is not a valid gitignore rule.
#[ohno::error]
#[display("Invalid packaging pattern '{}'", pattern.quoted())]
pub(crate) struct InvalidPackagingPatternError {
    pattern: String,
}

impl UnwindSafe for InvalidPackagingPatternError {}
impl RefUnwindSafe for InvalidPackagingPatternError {}

#[cfg(test)]
impl InvalidPackagingPatternError {
    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }
}

/// A `[workspace] members` / `exclude` entry is not a valid path pattern.
#[ohno::error]
#[display("Invalid workspace member pattern '{}'", pattern.quoted())]
pub(crate) struct InvalidMemberPatternError {
    pattern: String,
}

impl UnwindSafe for InvalidMemberPatternError {}
impl RefUnwindSafe for InvalidMemberPatternError {}

#[cfg(test)]
impl InvalidMemberPatternError {
    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }
}

/// A Cargo lockfile does not describe a resolved dependency graph.
///
/// The lockfile is machine-written, so a lockfile that does not parse means the
/// tool is reading something other than what it believes. Guessing a closure
/// from it would silently under-report the dependencies a packaged target ships.
#[ohno::error]
#[display("Failed to read the resolved dependencies in '{}'", path.quoted())]
pub(crate) struct MalformedLockfileError {
    path: String,
}

impl UnwindSafe for MalformedLockfileError {}
impl RefUnwindSafe for MalformedLockfileError {}

/// A package's published dependency closure cannot be reconstructed.
///
/// Classification requires a workspace lockfile at every comparison endpoint
/// where the package has an installable binary target.
#[ohno::error]
#[display(
    "Cannot assess locked dependencies for package '{}' with an installable binary target: {reason}",
    package.quoted()
)]
pub(crate) struct LockfileClosureUnavailableError {
    package: String,
    reason: String,
}

impl UnwindSafe for LockfileClosureUnavailableError {}
impl RefUnwindSafe for LockfileClosureUnavailableError {}

/// A package's private-API declaration is present but is not a boolean.
///
/// Fails closed rather than defaulting: a typo here would otherwise silently
/// decide whether the package is assessed for API compatibility at all.
#[ohno::error]
#[display(
    "Package '{}' declares `[package.metadata.release-plan] private-api` as {}, which must be a boolean",
    package.quoted(),
    value.quoted()
)]
pub(crate) struct MalformedPrivateApiError {
    package: String,
    value: String,
}

impl UnwindSafe for MalformedPrivateApiError {}
impl RefUnwindSafe for MalformedPrivateApiError {}

#[cfg(test)]
impl MalformedPrivateApiError {
    pub(crate) fn package(&self) -> &str {
        &self.package
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
    assert_impl_all!(CommandIoError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CommandFailedError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NonUtf8PathError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NonUtf8BlobError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PathTooLongError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ReadFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WriteFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParseTomlError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParseMetadataError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidVersionError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnresolvedBaseError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(LegacyVersionGroupsError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnsupportedExactRequirementError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidPackagingPatternError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidMemberPatternError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(MalformedLockfileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(LockfileClosureUnavailableError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(MalformedPrivateApiError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn command_io_error_retains_start_write_and_wait_causes() {
        for (kind, cause) in [
            (io::ErrorKind::NotFound, "process creation"),
            (io::ErrorKind::BrokenPipe, "stdin write"),
            (io::ErrorKind::Other, "process wait"),
        ] {
            let error = CommandIoError::caused_by("git", io::Error::new(kind, cause));
            let source = error.find_source::<io::Error>().unwrap();
            assert_eq!(source.kind(), kind);
            assert_eq!(source.to_string(), cause);
        }
    }

    /// A repository controlled value is escaped in the message.
    ///
    /// A manifest can name a package or a pattern with a newline in it, and the message would
    /// otherwise carry that newline into a log where the tail reads as a fresh line of the tool's
    /// own output.
    ///
    /// The condition renders on the first line, followed by its cause and, when
    /// backtraces are enabled, a captured backtrace. Escaping is therefore
    /// asserted on that first line: an unescaped value would push the tail of
    /// the pattern off it.
    #[test]
    fn a_repository_controlled_value_is_escaped_in_the_message() {
        let error = InvalidMemberPatternError::new("a\nb");
        let message = error.to_string();
        let first_line = message.lines().next().unwrap();
        assert!(first_line.contains(r"a\nb"), "{message}");
    }

    /// The same protection applies to the paths the file-access errors name.
    #[test]
    fn a_repository_controlled_path_is_escaped_in_the_message() {
        let error = ReadFileError::caused_by(Path::new("a\nb"), io::Error::other("x"));
        let message = error.to_string();
        let first_line = message.lines().next().unwrap();
        assert!(first_line.contains(r"a\nb"), "{message}");
    }

    /// An ordinary value gains no escaping, so the common message stays plain.
    #[test]
    fn an_ordinary_value_is_left_alone_in_the_message() {
        let error = InvalidMemberPatternError::new("packages/*");
        assert!(error.to_string().contains("'packages/*'"));
    }
}
