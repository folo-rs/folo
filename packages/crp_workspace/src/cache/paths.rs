//! Overlap admission for resolved locations, including missing path suffixes.

use std::path::{Path, PathBuf};
use std::{fs, io};

use ohno::AppError;
use tempfile::Builder;

use crate::manifest::PathCase;

// Native case acquisition is isolated from component-wise overlap policy.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn require_disjoint(cache: &Path, protected: &Path) -> Result<(), AppError> {
    require_disjoint_with(cache, protected, creation_case)
}

fn require_disjoint_with(
    cache: &Path,
    protected: &Path,
    mut case: impl FnMut(&Path) -> Result<PathCase, AppError>,
) -> Result<(), AppError> {
    let mut parent = PathBuf::new();
    for (left, right) in cache.components().zip(protected.components()) {
        if left != right {
            let (Some(left), Some(right)) = (left.as_os_str().to_str(), right.as_os_str().to_str())
            else {
                return Ok(());
            };
            if !PathCase::Insensitive.same_path(left, right)
                || !case(&parent)
                    .map_err(|error| CachePathConflict::caused_by(cache, protected, error))?
                    .same_path(left, right)
            {
                return Ok(());
            }
        }
        parent.push(left);
    }
    Err(CachePathConflict::new(cache, protected).into())
}

// Missing directories inherit the containing directory's lookup rules. Probe the nearest
// existing directory, not the checkout or OS. Existing entries avoid writes on read-only
// source trees; a temporary entry provides evidence when the directory is empty.
#[cfg_attr(test, mutants::skip)]
fn creation_case(path: &Path) -> Result<PathCase, AppError> {
    for ancestor in path.ancestors() {
        match fs::metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => {
                if let Some(case) = PathCase::probe_known(ancestor)? {
                    return Ok(case);
                }
                let probe = Builder::new().prefix(".crp-case-").tempfile_in(ancestor)?;
                let name = probe
                    .path()
                    .file_name()
                    .expect("a temporary file has a name");
                let alias = ancestor.join(name.to_string_lossy().to_uppercase());
                return match fs::symlink_metadata(alias) {
                    Ok(_) => Ok(PathCase::Insensitive),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        Ok(PathCase::Sensitive)
                    }
                    Err(error) => Err(error.into()),
                };
            }
            Ok(_) => return Err(io::Error::from(io::ErrorKind::NotADirectory).into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound).into())
}

/// Disposable entries must remain disjoint from source and retained evidence.
#[ohno::error]
#[display("cache location '{}' overlaps protected source or evidence '{}'", path.display(), protected.display())]
struct CachePathConflict {
    path: PathBuf,
    protected: PathBuf,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn prefixes_are_checked_in_both_directions_using_each_parent() {
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            for (left, right, overlap) in [
                ("a", "a", true),
                ("a", "a/b", true),
                ("a/b", "a", true),
                ("a", "ab", false),
                ("a/cache", "a/source", false),
                ("a/cache", "a/CACHE/report", case == PathCase::Insensitive),
                ("a/CACHE/report", "a/cache", case == PathCase::Insensitive),
            ] {
                let result = require_disjoint_with(Path::new(left), Path::new(right), |parent| {
                    assert_eq!(parent, Path::new("a"));
                    Ok(case)
                });
                assert_eq!(result.is_err(), overlap);
                if let Err(error) = result {
                    assert!(error.find_source::<CachePathConflict>().is_some());
                }
            }
        }
        let error = require_disjoint_with(Path::new("a/cache"), Path::new("a/CACHE"), |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
        })
        .unwrap_err();
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
