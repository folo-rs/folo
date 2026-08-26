// Private failure conditions of the tool.
//
// Each condition reaches the application boundary through `ohno::AppError`.
//
// The `#[ohno::error]` macro injects an OhnoCore field containing
// `Arc<dyn Error + Send + Sync>`, which is `!UnwindSafe` because `Arc` requires
// `T: RefUnwindSafe` and trait objects are `!RefUnwindSafe`. These error types
// are immutable after construction — no `&self` method mutates internal state —
// so observing them through a shared reference during unwind is harmless.
// Ref: docs/unwind-safety.md.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// A helper process could not be started.
#[ohno::error]
#[display("Failed to spawn `{program}`")]
pub(crate) struct CommandSpawnError {
    program: String,
}

impl UnwindSafe for CommandSpawnError {}
impl RefUnwindSafe for CommandSpawnError {}

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

/// A file could not be read.
#[ohno::error]
#[display("Failed to read '{}'", path.display())]
pub(crate) struct ReadFileError {
    path: PathBuf,
}

impl UnwindSafe for ReadFileError {}
impl RefUnwindSafe for ReadFileError {}

/// A file could not be written.
#[ohno::error]
#[display("Failed to write '{}'", path.display())]
pub(crate) struct WriteFileError {
    path: PathBuf,
}

impl UnwindSafe for WriteFileError {}
impl RefUnwindSafe for WriteFileError {}

/// A TOML document is not valid.
#[ohno::error]
#[display("Failed to parse '{}'", path.display())]
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

/// A plan file is not valid JSON.
#[ohno::error]
#[display("Failed to parse plan '{}'", path.display())]
pub(crate) struct ParsePlanError {
    path: PathBuf,
}

impl UnwindSafe for ParsePlanError {}
impl RefUnwindSafe for ParsePlanError {}

/// The plan uses a schema version this tool does not implement.
#[ohno::error]
#[display("Unsupported plan schema_version {version}")]
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
#[display("Plan increment '{name}' must supply exactly one of `level` or `version`")]
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

/// A plan names a package or group that is not in the workspace.
#[ohno::error]
#[display("Plan increment '{name}' is not a publishable package or version group")]
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

/// An increment level is not `major`, `minor`, or `patch`.
#[ohno::error]
#[display("Unknown increment level '{level}' for '{name}'")]
pub(crate) struct UnknownIncrementLevelError {
    name: String,
    level: String,
}

impl UnwindSafe for UnknownIncrementLevelError {}
impl RefUnwindSafe for UnknownIncrementLevelError {}

/// A version string is not valid `SemVer`.
#[ohno::error]
#[display("Invalid version '{version}' for '{name}'")]
pub(crate) struct InvalidVersionError {
    name: String,
    version: String,
}

impl UnwindSafe for InvalidVersionError {}
impl RefUnwindSafe for InvalidVersionError {}

/// History ended before a version change (including creation) was observed.
#[ohno::error]
#[display(
    "Shallow or truncated history: no version change found for package '{package}' on the base first-parent line"
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

/// The base revision could not be resolved.
#[ohno::error]
#[display("Failed to resolve base revision '{rev}'")]
pub(crate) struct UnresolvedBaseError {
    rev: String,
}

impl UnwindSafe for UnresolvedBaseError {}
impl RefUnwindSafe for UnresolvedBaseError {}

/// Two increments demand different explicit versions for the same group.
#[ohno::error]
#[display("Conflicting explicit versions for version group '{group}'")]
pub(crate) struct ConflictingPlanVersionError {
    group: String,
}

impl UnwindSafe for ConflictingPlanVersionError {}
impl RefUnwindSafe for ConflictingPlanVersionError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::{error, io};

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(
        CommandSpawnError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        CommandFailedError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
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
        ParseTomlError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ParseMetadataError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ParsePlanError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        UnsupportedPlanSchemaError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        PlanIncrementSpecError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        UnknownPlanTargetError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        UnknownIncrementLevelError: Send,
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
    assert_impl_all!(
        ShallowHistoryError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        UnresolvedBaseError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ConflictingPlanVersionError: Send,
        Sync,
        Debug,
        error::Error,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn command_spawn_error_retains_source() {
        let error =
            CommandSpawnError::caused_by("git", io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn shallow_history_error_names_package() {
        let error = ShallowHistoryError::new("nm");
        assert_eq!(error.package(), "nm");
    }

    #[test]
    fn unsupported_plan_schema_error_carries_version() {
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
}
