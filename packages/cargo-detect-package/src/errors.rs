// Private conditions converted into the application's `ohno::AppError` boundary.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// The process working directory could not be determined.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to determine the current directory")]
pub(crate) struct CurrentDirectoryError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for CurrentDirectoryError {}
impl RefUnwindSafe for CurrentDirectoryError {}

/// No Cargo workspace was found above the current directory.
#[doc(hidden)]
#[ohno::error]
#[display("Current directory is not within a Cargo workspace")]
pub(crate) struct CurrentDirectoryOutsideWorkspaceError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for CurrentDirectoryOutsideWorkspaceError {}
impl RefUnwindSafe for CurrentDirectoryOutsideWorkspaceError {}

/// Canonicalizing the target path failed.
#[ohno::error]
#[display("Could not canonicalize target path '{}'", path.display())]
pub(crate) struct CanonicalizeTargetPathError {
    path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for CanonicalizeTargetPathError {}
impl RefUnwindSafe for CanonicalizeTargetPathError {}

/// No Cargo workspace was found above the target path.
#[doc(hidden)]
#[ohno::error]
#[display("Target path is not within a Cargo workspace")]
pub(crate) struct TargetPathOutsideWorkspaceError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for TargetPathOutsideWorkspaceError {}
impl RefUnwindSafe for TargetPathOutsideWorkspaceError {}

/// The current directory and the target path are in different workspaces.
#[doc(hidden)]
#[ohno::error]
#[display(
    "Current directory workspace ('{}') differs from target path workspace ('{}')",
    current_workspace.display(),
    target_workspace.display()
)]
pub(crate) struct WorkspaceMismatchError {
    current_workspace: PathBuf,
    target_workspace: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for WorkspaceMismatchError {}
impl RefUnwindSafe for WorkspaceMismatchError {}

/// A `Cargo.toml` manifest could not be read.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to read '{}/Cargo.toml'", directory.display())]
pub(crate) struct ReadManifestError {
    directory: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ReadManifestError {}
impl RefUnwindSafe for ReadManifestError {}

/// A `Cargo.toml` manifest could not be parsed as TOML.
#[doc(hidden)]
#[ohno::error]
#[display("Failed to parse '{}/Cargo.toml'", directory.display())]
pub(crate) struct ParseManifestError {
    directory: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ParseManifestError {}
impl RefUnwindSafe for ParseManifestError {}

/// A `Cargo.toml` manifest does not declare a package name.
#[doc(hidden)]
#[ohno::error]
#[display("Could not find package name in {}/Cargo.toml", directory.display())]
pub(crate) struct PackageNameMissingError {
    directory: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for PackageNameMissingError {}
impl RefUnwindSafe for PackageNameMissingError {}

/// The path is not in any package and that was configured to be an error.
#[doc(hidden)]
#[ohno::error]
#[display("Path is not in any package")]
pub(crate) struct OutsidePackageError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for OutsidePackageError {}
impl RefUnwindSafe for OutsidePackageError {}

/// The subcommand could not be executed.
#[doc(hidden)]
#[ohno::error]
#[display("Error executing command")]
pub(crate) struct CommandExecutionError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for CommandExecutionError {}
impl RefUnwindSafe for CommandExecutionError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use ohno::ErrorExt;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(
        CurrentDirectoryError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        CurrentDirectoryOutsideWorkspaceError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        CanonicalizeTargetPathError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        TargetPathOutsideWorkspaceError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        WorkspaceMismatchError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(ReadManifestError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParseManifestError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        PackageNameMissingError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(OutsidePackageError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CommandExecutionError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);

    /// The tool's `README.md` documents this exact wording as part of its contract.
    #[test]
    fn outside_package_error_message_matches_documented_wording() {
        assert_eq!(
            OutsidePackageError::new().message(),
            "Path is not in any package"
        );
    }
}
