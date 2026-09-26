// Artifact destinations can acquire missing parent directories during generation.
// Resolve existing ancestors before comparing their eventual filesystem locations.

use std::ffi::OsString;
use std::fs::{self, File, Metadata};
use std::io::{Error as IoError, ErrorKind};
use std::path::{Component, Path, PathBuf, absolute};

use ohno::AppError;
use tempfile::NamedTempFile;

use crate::WriteFileError;

/// Promotes a caller-written artifact atomically without replacing an existing destination.
// Filesystem lifetime and promotion are exercised by boundary integration tests.
#[cfg_attr(test, mutants::skip)]
pub fn write_new(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), AppError>,
) -> Result<(), AppError> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file =
        NamedTempFile::new_in(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
    write(file.as_file_mut())?;
    file.persist_noclobber(path)
        .map_err(|error| WriteFileError::caused_by(path, error))?;
    Ok(())
}

pub fn same_path(left: &Path, right: &Path) -> Result<bool, AppError> {
    Ok(resolve_path(left)? == resolve_path(right)?)
}

pub fn resolve_path(path: &Path) -> Result<PathBuf, AppError> {
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
}
