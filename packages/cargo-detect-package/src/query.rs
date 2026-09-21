use std::io::ErrorKind;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::Path;

use ohno::AppError;

use crate::detection::{DetectedPackage, WorkspaceContext, detect_package};
use crate::errors::{CanonicalizeTargetPathError, TargetPathOutsideWorkspaceError};
use crate::pal::{Filesystem, FilesystemFacade};

/// Queries ownership within an explicitly supplied Cargo workspace.
///
/// This unsupported in-workspace interface performs no command execution or diagnostic output.
/// The caller supplies Cargo's absolute workspace root; relative targets resolve beneath it.
/// A missing target uses its nearest existing ancestor. `None` denotes workspace scope,
/// including a root package or a removed subtree with no remaining package manifest.
///
/// # Errors
///
/// Returns an error if paths cannot be resolved, the target resolves outside the supplied
/// workspace, or a package manifest cannot be interpreted.
#[doc(hidden)]
// This native adapter only selects the filesystem; policy is tested through query_with.
#[cfg_attr(test, mutants::skip)]
pub fn query_package(workspace_root: &Path, target: &Path) -> Result<Option<String>, AppError> {
    query_with(workspace_root, target, &FilesystemFacade::target())
}

/// Reuses manifest ancestry after resolving deleted diff paths without ambient-CWD access.
fn query_with(
    workspace_root: &Path,
    target: &Path,
    fs: &impl Filesystem,
) -> Result<Option<String>, AppError> {
    if !workspace_root.is_absolute() {
        return Err(RelativeWorkspaceRoot::new().into());
    }
    let workspace_root = fs
        .canonicalize(workspace_root)
        .map_err(|error| CanonicalizeTargetPathError::caused_by(workspace_root, error))?;
    let target = workspace_root.join(target);
    let mut ancestor = target.as_path();
    let absolute_target_path = loop {
        match fs.canonicalize(ancestor) {
            Ok(path) => break path,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| CanonicalizeTargetPathError::caused_by(&target, error))?;
            }
            Err(error) => {
                return Err(CanonicalizeTargetPathError::caused_by(&target, error).into());
            }
        }
    };
    if !absolute_target_path.starts_with(&workspace_root) {
        return Err(TargetPathOutsideWorkspaceError::new().into());
    }
    let context = WorkspaceContext {
        absolute_target_path,
        workspace_root,
    };
    Ok(match detect_package(&context, fs)? {
        DetectedPackage::Package(name) => Some(name),
        DetectedPackage::Workspace => None,
    })
}

/// An explicit query root must not acquire meaning from the process working directory.
#[ohno::error]
#[display("Package queries require an absolute Cargo workspace root")]
struct RelativeWorkspaceRoot;

// This immutable diagnostic has no state that can be changed during unwinding.
impl UnwindSafe for RelativeWorkspaceRoot {}
impl RefUnwindSafe for RelativeWorkspaceRoot {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::pal::MockFilesystem;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\workspace")
        } else {
            PathBuf::from("/workspace")
        }
    }

    fn filesystem() -> MockFilesystem {
        let mut fs = MockFilesystem::new();
        fs.expect_canonicalize().returning(|path| {
            if path == root() || path == root().join("member") || path == root().join("Cargo.toml")
            {
                Ok(path.to_owned())
            } else {
                Err(ErrorKind::NotFound.into())
            }
        });
        fs.expect_cargo_toml_exists()
            .returning(|path| path == root() || path == root().join("member"));
        fs.expect_read_cargo_toml()
            .withf(|path| path == root().join("member"))
            .returning(|_| Ok("[package]\nname = 'member'".to_owned()));
        fs.expect_is_file()
            .returning(|path| path == root().join("Cargo.toml"));
        fs
    }

    #[test]
    fn deleted_files_retain_the_nearest_surviving_package() {
        let fs = filesystem();
        assert_eq!(
            query_with(&root(), &PathBuf::from("member").join("removed.rs"), &fs).unwrap(),
            Some("member".to_owned())
        );
    }

    #[test]
    fn removed_packages_and_root_files_select_workspace_scope() {
        let fs = filesystem();
        assert_eq!(
            query_with(&root(), Path::new("Cargo.toml"), &fs).unwrap(),
            None
        );
        assert_eq!(
            query_with(&root(), Path::new("removed-package"), &fs).unwrap(),
            None
        );
    }

    #[test]
    fn absolute_existing_package_does_not_read_current_directory() {
        assert_eq!(
            query_with(&root(), &root().join("member"), &filesystem()).unwrap(),
            Some("member".to_owned())
        );
    }

    #[test]
    fn outside_targets_and_io_errors_are_not_workspace_fallbacks() {
        let mut fs = MockFilesystem::new();
        fs.expect_canonicalize()
            .returning(|path| Ok(path.to_owned()));
        query_with(&root(), root().parent().unwrap(), &fs).unwrap_err();
        let mut fs = MockFilesystem::new();
        fs.expect_canonicalize()
            .returning(|_| Err(ErrorKind::PermissionDenied.into()));
        query_with(&root(), Path::new("member"), &fs).unwrap_err();
    }

    #[test]
    fn unreadable_target_is_not_treated_as_a_deleted_path() {
        let mut fs = MockFilesystem::new();
        fs.expect_canonicalize().returning(|path| {
            if path == root() {
                Ok(root())
            } else {
                Err(ErrorKind::PermissionDenied.into())
            }
        });
        query_with(&root(), Path::new("member"), &fs).unwrap_err();
    }

    #[test]
    fn relative_workspace_roots_do_not_fall_back_to_the_process_directory() {
        let error = query_with(
            Path::new("workspace"),
            Path::new("member"),
            &MockFilesystem::new(),
        )
        .unwrap_err();
        assert!(error.find_source::<RelativeWorkspaceRoot>().is_some());
    }
}
