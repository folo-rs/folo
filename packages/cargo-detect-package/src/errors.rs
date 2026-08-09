// Private typed errors describing every way the tool can fail.
//
// Each condition reaches the application boundary through `ohno::AppError`.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// The process working directory could not be determined.
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
#[ohno::error]
#[display("Failed to read '{}'", manifest_path.display())]
pub(crate) struct ReadManifestError {
    manifest_path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ReadManifestError {}
impl RefUnwindSafe for ReadManifestError {}

/// A `Cargo.toml` manifest could not be parsed as TOML.
#[ohno::error]
#[display("Failed to parse '{}'", manifest_path.display())]
pub(crate) struct ParseManifestError {
    manifest_path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ParseManifestError {}
impl RefUnwindSafe for ParseManifestError {}

/// A `Cargo.toml` manifest does not declare a package name.
#[ohno::error]
#[display("Could not find package name in {}", manifest_path.display())]
pub(crate) struct PackageNameMissingError {
    manifest_path: PathBuf,
}

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for PackageNameMissingError {}
impl RefUnwindSafe for PackageNameMissingError {}

/// The path is not in any package and that was configured to be an error.
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
#[ohno::error]
#[display("Could not execute command")]
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
    use std::io;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::path::Path;

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

    /// Design (`docs/design.md`, "Diagnostics and error boundary") promises that a
    /// diagnostic identifies the workspace, manifest or path it concerns. The field
    /// assertions elsewhere only prove the value is carried, not that it is rendered.
    #[test]
    fn path_bearing_messages_identify_their_path() {
        let manifest = Path::new("some/dir/Cargo.toml");

        assert!(
            CanonicalizeTargetPathError::caused_by(
                Path::new("some/dir/target.rs"),
                io::Error::new(io::ErrorKind::NotFound, "missing"),
            )
            .message()
            .contains("some/dir/target.rs")
        );

        assert!(
            ReadManifestError::caused_by(manifest, io::Error::new(io::ErrorKind::NotFound, "gone"))
                .message()
                .contains("some/dir/Cargo.toml")
        );

        assert!(
            ParseManifestError::caused_by(
                manifest,
                toml::from_str::<toml::Value>("this is = not [valid toml").unwrap_err(),
            )
            .message()
            .contains("some/dir/Cargo.toml")
        );

        assert!(
            PackageNameMissingError::new(manifest)
                .message()
                .contains("some/dir/Cargo.toml")
        );

        let mismatch =
            WorkspaceMismatchError::new(Path::new("left/root"), Path::new("right/root")).message();
        assert!(mismatch.contains("left/root"));
        assert!(mismatch.contains("right/root"));
    }
}
