// Failure conditions owned by this application's component. Leaves remain implementation details.
// The immutable ohno source chains permit shared observation across unwind boundaries.
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

use crp_diag::Quotable as _;
use semver::Version;
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

/// An expanded-plan output directory could not be created.
#[ohno::error]
#[display("Failed to create output directory '{}'", path.quoted())]
pub(crate) struct CreateOutputDirectoryError {
    path: PathBuf,
}

impl UnwindSafe for CreateOutputDirectoryError {}
impl RefUnwindSafe for CreateOutputDirectoryError {}

/// A release artifact cannot be decoded as the supported JSON document.
#[ohno::error]
#[display("Failed to parse release artifact '{}'", path.quoted())]
pub(crate) struct ParsePlanError {
    path: PathBuf,
}

impl UnwindSafe for ParsePlanError {}
impl RefUnwindSafe for ParsePlanError {}

/// A plan or report uses a schema version this tool does not implement.
#[ohno::error]
#[display(
    "Unsupported release artifact schema_version {version}; regenerate reports with \
     `cargo release-plan report` or prepared evidence with `cargo release-plan prepare`, \
     then regenerate plans with the current tool"
)]
pub(crate) struct UnsupportedPlanSchemaError {
    version: u32,
}

impl UnwindSafe for UnsupportedPlanSchemaError {}
impl RefUnwindSafe for UnsupportedPlanSchemaError {}

#[cfg(test)]
impl UnsupportedPlanSchemaError {
    pub(crate) fn version(&self) -> u32 {
        self.version
    }
}

/// An increment entry is missing `level` and `version`, or supplies both.
#[ohno::error]
#[display(
    "Plan increment '{}' must supply exactly one of `level` or `version`",
    name.quoted()
)]
pub(crate) struct PlanIncrementSpecError {
    name: String,
}

impl UnwindSafe for PlanIncrementSpecError {}
impl RefUnwindSafe for PlanIncrementSpecError {}

#[cfg(test)]
impl PlanIncrementSpecError {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// A plan names a package that is not a tracked workspace version target.
#[ohno::error]
#[display("Plan increment '{}' is not a tracked workspace member", name.quoted())]
pub(crate) struct UnknownPlanTargetError {
    name: String,
}

impl UnwindSafe for UnknownPlanTargetError {}
impl RefUnwindSafe for UnknownPlanTargetError {}

#[cfg(test)]
impl UnknownPlanTargetError {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// An expanded plan no longer names every package it reaches.
///
/// An expanded plan lists every package the decision moves, so presentation and
/// application use the same set. Expanding it again must therefore reproduce
/// exactly that set. Reaching a package it does not name means the workspace's
/// derived group changed after the document was produced, so applying it
/// would edit an unlisted package.
#[ohno::error]
#[display(
    "Expanded plan reaches packages it does not name: {}. The workspace's version groups changed \
     after this document was produced, so expand the proposed plan again and review the wider set",
    unnamed.join(", ")
)]
pub(crate) struct ExpandedPlanDriftError {
    unnamed: Vec<String>,
}

impl UnwindSafe for ExpandedPlanDriftError {}
impl RefUnwindSafe for ExpandedPlanDriftError {}

#[cfg(test)]
impl ExpandedPlanDriftError {
    pub(crate) fn unnamed(&self) -> &[String] {
        &self.unnamed
    }
}

/// An expanded plan carries an increment level instead of an explicit version.
///
/// An expanded plan records the version each package will take, which is what
/// makes reviewing one meaningful. A level is resolved against the manifests as
/// they stand when it is applied, so the same expanded document could apply a
/// different version than the one that was reviewed.
#[ohno::error]
#[display(
    "Expanded plan leaves an increment level unresolved for: {}. An expanded plan records the \
     version each package takes, so expand the proposed plan again",
    unresolved.join(", ")
)]
pub(crate) struct UnresolvedExpandedPlanError {
    unresolved: Vec<String>,
}

impl UnwindSafe for UnresolvedExpandedPlanError {}
impl RefUnwindSafe for UnresolvedExpandedPlanError {}

#[cfg(test)]
impl UnresolvedExpandedPlanError {
    pub(crate) fn unresolved(&self) -> &[String] {
        &self.unresolved
    }
}

/// An increment level is not `major`, `minor`, or `patch`.
#[ohno::error]
#[display(
    "Unknown increment level '{}' for '{}'",
    level.quoted(),
    name.quoted()
)]
pub(crate) struct UnknownIncrementLevelError {
    name: String,
    level: String,
}

impl UnwindSafe for UnknownIncrementLevelError {}
impl RefUnwindSafe for UnknownIncrementLevelError {}

/// History ended before a version change (including creation) was observed.
#[ohno::error]
#[display(
    "Shallow or truncated history: no version change found for package '{}' on the base first-parent line",
    package.quoted()
)]
pub(crate) struct ShallowHistoryError {
    package: String,
}

impl UnwindSafe for ShallowHistoryError {}
impl RefUnwindSafe for ShallowHistoryError {}

#[cfg(test)]
impl ShallowHistoryError {
    pub(crate) fn package(&self) -> &str {
        &self.package
    }
}

/// Two increments demand different explicit versions for the same group.
#[ohno::error]
#[display(
    "Conflicting explicit versions for version group '{}'",
    group.quoted()
)]
pub(crate) struct ConflictingPlanVersionError {
    group: String,
}

impl UnwindSafe for ConflictingPlanVersionError {}
impl RefUnwindSafe for ConflictingPlanVersionError {}

/// Entries affecting one target mix an increment level with an explicit version.
#[ohno::error]
#[display(
    "Plan target '{}' mixes increment levels with explicit versions",
    target.quoted()
)]
pub(crate) struct ConflictingPlanIncrementKindError {
    target: String,
}

impl UnwindSafe for ConflictingPlanIncrementKindError {}
impl RefUnwindSafe for ConflictingPlanIncrementKindError {}

/// A group is assigned an explicit version that exact pins cannot represent.
#[ohno::error]
#[display(
    "Version group '{}' cannot use non-plain target version '{}'; use a major.minor.patch version",
    group.quoted(),
    version
)]
pub(crate) struct NonPlainGroupVersionError {
    group: String,
    version: Version,
}

impl UnwindSafe for NonPlainGroupVersionError {}
impl RefUnwindSafe for NonPlainGroupVersionError {}

/// Incrementing a semantic-version component overflows `u64`.
#[ohno::error]
#[display("Incrementing version '{version}' overflows a SemVer component")]
pub(crate) struct VersionOverflowError {
    version: Version,
}

impl UnwindSafe for VersionOverflowError {}
impl RefUnwindSafe for VersionOverflowError {}

#[cfg(test)]
impl VersionOverflowError {
    pub(crate) fn version(&self) -> &Version {
        &self.version
    }
}

/// A declared version is lower than the version already released at the anchor.
#[ohno::error]
#[display(
    "Package '{}' declares version {declared}, which is lower than {anchor_version} \
     released at anchor {anchor_commit}",
    package.quoted()
)]
pub(crate) struct VersionRegressionError {
    package: String,
    declared: Version,
    anchor_version: Version,
    anchor_commit: String,
}

impl UnwindSafe for VersionRegressionError {}
impl RefUnwindSafe for VersionRegressionError {}

#[cfg(test)]
impl VersionRegressionError {
    pub(crate) fn package(&self) -> &str {
        &self.package
    }
}

/// A plan asks for a version lower than one a target already declares.
#[ohno::error]
#[display(
    "Plan sets '{}' to {requested}, which is lower than the declared {declared}",
    target.quoted()
)]
pub(crate) struct PlanVersionRegressionError {
    target: String,
    requested: Version,
    declared: Version,
}

impl UnwindSafe for PlanVersionRegressionError {}
impl RefUnwindSafe for PlanVersionRegressionError {}

#[cfg(test)]
impl PlanVersionRegressionError {
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
}

/// Released content contains a symbolic link.
///
/// Cargo dereferences a link when it builds a package archive, so the released bytes are
/// the target's content, while Git stores the link as a blob holding the target
/// path. Comparing the blobs would call a package unchanged after an edit to the
/// file it points at, and reconstructing the target's historical content is only
/// possible when the link stays inside the repository at both ends. A refusal is
/// preferred over a release verdict that can be silently wrong. Ref:
/// docs/design.md, "Released content".
#[ohno::error]
#[display(
    "Package '{}' releases '{}', which is a symbolic link",
    package.quoted(),
    path.quoted()
)]
pub(crate) struct SymlinkReleasedError {
    package: String,
    path: String,
}

impl UnwindSafe for SymlinkReleasedError {}
impl RefUnwindSafe for SymlinkReleasedError {}

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::path::Path;
    use std::{error, io};

    use static_assertions::assert_impl_all;

    use super::*;
    assert_impl_all!(ReadFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WriteFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CreateOutputDirectoryError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParsePlanError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnsupportedPlanSchemaError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PlanIncrementSpecError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnknownPlanTargetError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ExpandedPlanDriftError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnresolvedExpandedPlanError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(UnknownIncrementLevelError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ShallowHistoryError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ConflictingPlanVersionError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ConflictingPlanIncrementKindError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NonPlainGroupVersionError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(VersionOverflowError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(VersionRegressionError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PlanVersionRegressionError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(SymlinkReleasedError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn shallow_history_error_names_package() {
        let error = ShallowHistoryError::new("nm");
        assert_eq!(error.package(), "nm");
    }

    #[test]
    fn unsupported_plan_schema_error_carries_version() {
        // An arbitrary unsupported schema revision; the test covers round-tripping
        // through the error, not revision-compatibility semantics.
        let error = UnsupportedPlanSchemaError::new(9_u32);
        assert_eq!(error.version(), 9);
    }

    #[test]
    fn plan_increment_spec_error_names_target() {
        let error = PlanIncrementSpecError::new("nm");
        assert_eq!(error.name(), "nm");
    }

    #[test]
    fn unknown_plan_target_error_names_target() {
        let error = UnknownPlanTargetError::new("ghost");
        assert_eq!(error.name(), "ghost");
    }

    #[test]
    fn version_overflow_error_carries_version() {
        // An arbitrary ordinary semantic version; the test covers retained
        // context, not the overflow arithmetic that produces the error.
        let version: Version = "1.2.3".parse().unwrap();
        let error = VersionOverflowError::new(version.clone());
        assert_eq!(error.version(), &version);
    }

    #[test]
    fn version_regression_error_names_package() {
        // Arbitrary ordering-valid versions; the test covers retained context.
        let declared: Version = "0.1.0".parse().unwrap();
        let anchor: Version = "0.2.0".parse().unwrap();
        let error = VersionRegressionError::new("nm", declared, anchor, "abc123");
        assert_eq!(error.package(), "nm");
    }

    /// The same protection applies to the paths the file-access errors name.
    #[test]
    fn a_repository_controlled_path_is_escaped_in_the_message() {
        let error = ReadFileError::caused_by(Path::new("a\nb"), io::Error::other("x"));
        let message = error.to_string();
        let first_line = message.lines().next().unwrap();
        assert!(first_line.contains(r"a\nb"), "{message}");
    }
}
