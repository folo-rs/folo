// Artifact destinations can acquire missing parent directories during generation.
// Resolve existing ancestors before comparing their eventual filesystem locations.

use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io::{Error as IoError, ErrorKind};
use std::path::{Component, Path, PathBuf, absolute};

use ohno::AppError;

use crate::WriteFileError;

pub(crate) fn same_path(left: &Path, right: &Path) -> Result<bool, AppError> {
    Ok(resolve_path(left)? == resolve_path(right)?)
}

pub(crate) fn resolve_path(path: &Path) -> Result<PathBuf, AppError> {
    resolve_path_with(
        path,
        absolute(path).map_err(|error| WriteFileError::caused_by(path, error))?,
        |path| fs::canonicalize(path),
        |path| fs::metadata(path),
    )
}

// Acquisition is injectable so transient filesystem failures can be exercised without
// permission changes or races. See docs/implementation.md, "Test boundaries".
fn resolve_path_with(
    path: &Path,
    mut ancestor: PathBuf,
    mut canonicalize: impl FnMut(&Path) -> Result<PathBuf, IoError>,
    mut metadata: impl FnMut(&Path) -> Result<Metadata, IoError>,
) -> Result<PathBuf, AppError> {
    let mut suffix = Vec::<OsString>::new();
    loop {
        match canonicalize(&ancestor) {
            Ok(mut resolved) => {
                // A parent component can return from a missing directory to an existing one.
                // Resolve each subsequent component again so a later symlink keeps its meaning.
                for component in suffix.into_iter().rev() {
                    match metadata(&resolved) {
                        Ok(metadata) if !metadata.is_dir() => {
                            return Err(WriteFileError::caused_by(
                                path,
                                IoError::from(ErrorKind::NotADirectory),
                            )
                            .into());
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(WriteFileError::caused_by(path, error).into()),
                    }
                    if component == ".." {
                        _ = resolved.pop();
                    } else if component != "." {
                        resolved.push(component);
                    }
                    match canonicalize(&resolved) {
                        Ok(canonical) => resolved = canonical,
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(WriteFileError::caused_by(path, error).into()),
                    }
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(component) = ancestor.components().next_back().filter(|component| {
                    matches!(
                        component,
                        Component::Normal(_) | Component::ParentDir | Component::CurDir
                    )
                }) else {
                    return Err(WriteFileError::caused_by(path, error).into());
                };
                suffix.push(component.as_os_str().to_os_string());
                if !ancestor.pop() {
                    return Err(WriteFileError::caused_by(path, error).into());
                }
            }
            Err(error) => return Err(WriteFileError::caused_by(path, error).into()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::mem;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;
    #[cfg(unix)]
    use tempfile::tempdir_in;

    use super::*;

    #[test]
    fn an_initial_operational_error_is_not_retried_as_a_missing_suffix() {
        let mut failure = Some(IoError::from(ErrorKind::PermissionDenied));
        let error = resolve_path_with(
            Path::new("plan.json"),
            Path::new("root").join("plan.json"),
            |path| match failure.take() {
                Some(error) => Err(error),
                None => Ok(path.to_path_buf()),
            },
            |_| Err(IoError::from(ErrorKind::NotFound)),
        )
        .unwrap_err();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(
            error.find_source::<IoError>().unwrap().kind(),
            ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn directory_inspection_errors_are_not_missing_directories() {
        let mut missing = true;
        let error = resolve_path_with(
            Path::new("plan.json"),
            Path::new("root").join("plan.json"),
            |path| {
                if mem::take(&mut missing) {
                    Err(IoError::from(ErrorKind::NotFound))
                } else {
                    Ok(path.to_path_buf())
                }
            },
            |_| Err(IoError::from(ErrorKind::PermissionDenied)),
        )
        .unwrap_err();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(
            error.find_source::<IoError>().unwrap().kind(),
            ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn suffix_resolution_errors_are_not_missing_paths() {
        let output = Path::new("root").join("plan.json");
        let mut missing = true;
        let error = resolve_path_with(
            &output,
            output.clone(),
            |path| {
                if mem::take(&mut missing) {
                    Err(IoError::from(ErrorKind::NotFound))
                } else if path == output {
                    Err(IoError::from(ErrorKind::PermissionDenied))
                } else {
                    Ok(path.to_path_buf())
                }
            },
            |_| Err(IoError::from(ErrorKind::NotFound)),
        )
        .unwrap_err();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(
            error.find_source::<IoError>().unwrap().kind(),
            ErrorKind::PermissionDenied
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "resolves filesystem paths with missing ancestors")]
    fn missing_parent_components_cannot_hide_an_input_alias() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("report.json");
        fs::write(&input, "evidence").unwrap();
        let alias = directory
            .path()
            .join("missing")
            .join("..")
            .join("report.json");
        assert!(same_path(&input, &alias).unwrap());
        assert!(!same_path(&input, &directory.path().join("new").join("plan.json")).unwrap());
        assert!(!directory.path().join("missing").exists());
    }

    #[test]
    #[cfg_attr(miri, ignore = "checks filesystem errors for a non-directory ancestor")]
    fn an_unusable_ancestor_is_an_error_not_a_different_path() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("file");
        fs::write(&file, "not a directory").unwrap();
        assert!(
            resolve_path(&file.join("plan.json"))
                .unwrap_err()
                .find_source::<WriteFileError>()
                .is_some()
        );
        _ = resolve_path(
            &directory
                .path()
                .join("new")
                .join("..")
                .join("file")
                .join("plan.json"),
        )
        .unwrap_err();
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(miri, ignore = "checks parent traversal through a regular file")]
    fn parent_traversal_cannot_escape_a_non_directory() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("file");
        fs::write(&file, "not a directory").unwrap();
        let error = resolve_path(
            &directory
                .path()
                .join("missing")
                .join("..")
                .join("file")
                .join("..")
                .join("plan.json"),
        )
        .unwrap_err();
        assert_eq!(
            error.find_source::<IoError>().unwrap().kind(),
            ErrorKind::NotADirectory
        );
        assert_eq!(fs::read_to_string(file).unwrap(), "not a directory");
        assert!(!directory.path().join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(miri, ignore = "creates a directory symlink")]
    fn existing_symlink_ancestors_are_resolved_before_missing_components() {
        // Exercise a noncanonical temporary root regardless of the host's temporary directory.
        let root = tempdir().unwrap();
        let root_alias = root.path().join("root-alias");
        symlink(root.path(), &root_alias).unwrap();
        let directory = tempdir_in(&root_alias).unwrap();
        assert_ne!(
            directory.path(),
            fs::canonicalize(directory.path()).unwrap()
        );
        let real = directory.path().join("nested").join("real");
        fs::create_dir_all(&real).unwrap();
        let alias = directory.path().join("alias");
        symlink(&real, &alias).unwrap();
        let input = real.join("report.json");
        fs::write(&input, "evidence").unwrap();
        assert!(same_path(&input, &alias.join("report.json")).unwrap());
        assert!(same_path(&input, &alias.join("new").join("..").join("report.json")).unwrap());
        assert!(
            same_path(
                &input,
                &directory
                    .path()
                    .join("missing")
                    .join("..")
                    .join("alias")
                    .join("report.json")
            )
            .unwrap()
        );
        let parent_input = real.parent().unwrap().join("parent.json");
        fs::write(&parent_input, "parent evidence").unwrap();
        assert!(
            same_path(
                &parent_input,
                &directory
                    .path()
                    .join("missing")
                    .join("..")
                    .join("alias")
                    .join("..")
                    .join("parent.json")
            )
            .unwrap()
        );
        assert_eq!(
            resolve_path(&alias.join("new").join("plan.json")).unwrap(),
            fs::canonicalize(&real)
                .unwrap()
                .join("new")
                .join("plan.json")
        );
    }
}
