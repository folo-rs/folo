// Artifact destinations can acquire missing parent directories during generation.
// Resolve existing ancestors before comparing their eventual filesystem locations.

use std::ffi::OsString;
use std::fs::{self, File};
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
    // Keep promotion on one filesystem by staging beside the destination. No-clobber
    // promotion separately preserves the caller's write-once artifact contract.
    let mut file =
        NamedTempFile::new_in(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
    write(file.as_file_mut())?;
    file.persist_noclobber(path)
        .map_err(|error| WriteFileError::caused_by(path, error))?;
    Ok(())
}

// Alias identity requires filesystem resolution; resolve_path_with tests the resolution policy.
#[cfg_attr(test, mutants::skip)]
pub fn same_path(left: &Path, right: &Path) -> Result<bool, AppError> {
    same_path_with(left, right, resolve_path)
}

fn same_path_with(
    left: &Path,
    right: &Path,
    mut resolve: impl FnMut(&Path) -> Result<PathBuf, AppError>,
) -> Result<bool, AppError> {
    Ok(resolve(left)? == resolve(right)?)
}

// Acquires the absolute path and native observations for the in-process resolver.
#[cfg_attr(test, mutants::skip)]
pub fn resolve_path(path: &Path) -> Result<PathBuf, AppError> {
    resolve_path_with(
        path,
        absolute(path).map_err(|error| WriteFileError::caused_by(path, error))?,
        |path| fs::canonicalize(path),
        |path| fs::metadata(path).map(|metadata| metadata.is_dir()),
    )
}

// Acquisition is injectable so transient filesystem failures can be exercised without
// permission changes or races. See packages/cargo-release-plan/docs/implementation.md, "Test
// boundaries".
fn resolve_path_with(
    path: &Path,
    mut ancestor: PathBuf,
    mut canonicalize: impl FnMut(&Path) -> Result<PathBuf, IoError>,
    mut is_directory: impl FnMut(&Path) -> Result<bool, IoError>,
) -> Result<PathBuf, AppError> {
    let mut suffix = Vec::<OsString>::new();
    loop {
        match canonicalize(&ancestor) {
            Ok(mut resolved) => {
                // A parent component can return from a missing directory to an existing one.
                // Reacquire identity for existing components to preserve filesystem casing
                // and short-name aliases rather than relying on lexical spelling.
                for component in suffix.into_iter().rev() {
                    match is_directory(&resolved) {
                        Ok(false) => {
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
    fn aliases_compare_resolved_identity_and_propagate_resolution_errors() {
        for same in [false, true] {
            assert_eq!(
                same_path_with(Path::new("first"), Path::new("second"), |path| {
                    Ok(if same {
                        PathBuf::from("actual")
                    } else {
                        path.into()
                    })
                })
                .unwrap(),
                same
            );
        }
        same_path_with(Path::new("first"), Path::new("second"), |_| {
            Err(IoError::other("identity").into())
        })
        .unwrap_err();
    }

    #[test]
    fn missing_suffixes_require_directory_ancestors() {
        let output = Path::new("root").join("missing").join("plan.json");
        for directory in [true, false] {
            let result = resolve_path_with(
                &output,
                output.clone(),
                |path| {
                    if path == Path::new("root") {
                        Ok(PathBuf::from("canonical"))
                    } else {
                        Err(ErrorKind::NotFound.into())
                    }
                },
                |path| {
                    if path == Path::new("canonical") {
                        Ok(directory)
                    } else {
                        Err(ErrorKind::NotFound.into())
                    }
                },
            );
            if directory {
                assert_eq!(
                    result.unwrap(),
                    Path::new("canonical").join("missing").join("plan.json")
                );
            } else {
                assert_eq!(
                    result.unwrap_err().find_source::<IoError>().unwrap().kind(),
                    ErrorKind::NotADirectory
                );
            }
        }
    }

    #[test]
    fn parent_suffix_reacquires_existing_aliases() {
        let output = Path::new("root").join("missing").join("..").join("ALIAS");
        let result = resolve_path_with(
            &output,
            output.clone(),
            |path| {
                if path == Path::new("root") || path == Path::new("canonical") {
                    Ok(PathBuf::from("canonical"))
                } else if path == Path::new("canonical").join("ALIAS") {
                    Ok(Path::new("canonical").join("recorded"))
                } else {
                    Err(ErrorKind::NotFound.into())
                }
            },
            |_| Ok(true),
        )
        .unwrap();
        assert_eq!(result, Path::new("canonical").join("recorded"));
    }

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
