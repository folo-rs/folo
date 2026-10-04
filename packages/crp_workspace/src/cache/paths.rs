//! Overlap admission for resolved locations, including missing path suffixes.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf, absolute};
use std::{fs, io};

use ohno::AppError;
use tempfile::Builder;

use crate::artifact_path::resolve_path;
use crate::manifest::PathCase;

// Untracked redirects under module-owning directories are source inputs, too.
#[cfg_attr(test, mutants::skip)] // Native listing adapter; traversal is tested with acquired entries.
pub(crate) fn redirected_sources(roots: &BTreeSet<PathBuf>) -> Result<BTreeSet<PathBuf>, AppError> {
    redirected_sources_with(roots, resolve_path, |path| {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        entries
            .map(|entry| {
                let entry = entry?;
                let kind = entry.file_type()?;
                let path = entry.path();
                let directory = if kind.is_symlink() {
                    fs::metadata(&path)?.is_dir()
                } else {
                    kind.is_dir()
                };
                Ok(SourceEntry {
                    path,
                    directory,
                    redirected: kind.is_symlink(),
                })
            })
            .collect()
    })
}

/// Acquired directory facts for discovering redirects without following a cycle indefinitely.
struct SourceEntry {
    path: PathBuf,
    directory: bool,
    redirected: bool,
}

fn redirected_sources_with(
    roots: &BTreeSet<PathBuf>,
    mut resolve: impl FnMut(&Path) -> Result<PathBuf, AppError>,
    mut children: impl FnMut(&Path) -> Result<Vec<SourceEntry>, AppError>,
) -> Result<BTreeSet<PathBuf>, AppError> {
    let mut pending = roots.clone();
    let mut visited = BTreeSet::new();
    let mut redirects = BTreeSet::new();
    while let Some(path) = pending.pop_first() {
        let identity =
            resolve(&path).map_err(|error| SourceInventoryUnavailable::caused_by(&path, error))?;
        if !visited.insert(identity) {
            continue;
        }
        for entry in
            children(&path).map_err(|error| SourceInventoryUnavailable::caused_by(&path, error))?
        {
            if entry.redirected {
                redirects.insert(entry.path.clone());
            }
            if entry.directory {
                pending.insert(entry.path);
            }
        }
    }
    Ok(redirects)
}

/// Incomplete descendant observations must disable optional storage, not admit unchecked paths.
#[ohno::error]
#[display("cannot inventory source directory '{}'", path.display())]
struct SourceInventoryUnavailable {
    path: PathBuf,
}

// Cache removal also removes link entries, not just resolved source/evidence referents.
#[cfg_attr(test, mutants::skip)] // Native identities are injected into protected_paths_with.
pub(crate) fn protected_paths(
    path: &Path,
    redirects: &mut HashMap<PathBuf, bool>,
) -> Result<Vec<PathBuf>, AppError> {
    protected_paths_with(&absolute(path)?, resolve_path, |path| {
        if let Some(value) = redirects.get(path) {
            return Ok(*value);
        }
        let value = redirected(path)?;
        redirects.insert(path.to_owned(), value);
        Ok(value)
    })
}

fn protected_paths_with(
    path: &Path,
    mut resolve: impl FnMut(&Path) -> Result<PathBuf, AppError>,
    mut redirected: impl FnMut(&Path) -> Result<bool, AppError>,
) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = vec![resolve(path)?];
    for ancestor in path.ancestors() {
        if redirected(ancestor)?
            && let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name())
        {
            paths.push(resolve(parent)?.join(name));
        }
    }
    Ok(paths)
}

#[cfg_attr(test, mutants::skip)] // Native symlinks and Windows junctions have boundary coverage.
fn redirected(path: &Path) -> Result<bool, AppError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    // Rust includes Windows name-surrogate reparse points (including junctions), not
    // unrelated reparse tags such as cloud placeholders.
    Ok(metadata.file_type().is_symlink())
}

#[cfg_attr(test, mutants::skip)] // Subject-directory redirection is a native storage boundary.
pub(crate) fn require_direct_subject(path: &Path) -> Result<(), AppError> {
    if redirected(path)? {
        return Err(RedirectedSubject::new(path).into());
    }
    Ok(())
}

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
                    .map_err(|error| CachePathCaseUnavailable::caused_by(cache, protected, error))?
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
pub(super) struct CachePathConflict {
    path: PathBuf,
    protected: PathBuf,
}

/// An unavailable case observation cannot establish overlap or admit storage.
#[ohno::error]
#[display("cannot determine whether cache location '{}' overlaps '{}'", path.display(), protected.display())]
struct CachePathCaseUnavailable {
    path: PathBuf,
    protected: PathBuf,
}

/// Subject storage must not redirect disposable publication into unrelated locations.
#[ohno::error]
#[display("cache subject directory '{}' is redirected", path.display())]
struct RedirectedSubject {
    path: PathBuf,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn redirected_descendants_follow_directory_links_once_and_preserve_file_links() {
        let mut acquisitions = BTreeSet::new();
        let found = redirected_sources_with(
            &["source", "absent"].map(PathBuf::from).into(),
            |path| {
                Ok(match path.to_str().unwrap() {
                    "source/linked" => PathBuf::from("external"),
                    "source/linked/cycle" => PathBuf::from("source"),
                    _ => path.to_owned(),
                })
            },
            |path| {
                assert!(
                    acquisitions.insert(path.to_owned()),
                    "repeated directory acquisition"
                );
                Ok(match path.to_str().unwrap() {
                    "source" => [
                        ("source/ordinary", true, false),
                        ("source/file", false, true),
                        ("source/linked", true, true),
                        ("source/lib.rs", false, false),
                    ]
                    .as_slice(),
                    "source/linked" => [("source/linked/cycle", true, true)].as_slice(),
                    "source/ordinary" | "absent" => &[],
                    _ => panic!("unexpected directory {path:?}"),
                }
                .iter()
                .map(|(path, directory, redirected)| SourceEntry {
                    path: PathBuf::from(path),
                    directory: *directory,
                    redirected: *redirected,
                })
                .collect())
            },
        )
        .unwrap();
        assert_eq!(
            found,
            ["source/file", "source/linked", "source/linked/cycle"]
                .map(PathBuf::from)
                .into()
        );
        assert_eq!(
            acquisitions,
            ["source", "source/ordinary", "source/linked", "absent"]
                .map(PathBuf::from)
                .into()
        );
    }

    #[test]
    fn incomplete_descendant_inventory_preserves_the_failed_source_path() {
        for fail_identity in [false, true] {
            let error = redirected_sources_with(
                &[PathBuf::from("source")].into(),
                |path| {
                    if fail_identity {
                        Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
                    } else {
                        Ok(path.to_owned())
                    }
                },
                |_| Err(io::Error::from(io::ErrorKind::PermissionDenied).into()),
            )
            .unwrap_err();
            assert_eq!(
                error
                    .find_source::<SourceInventoryUnavailable>()
                    .unwrap()
                    .path,
                Path::new("source")
            );
            assert_eq!(
                error.find_source::<io::Error>().unwrap().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn protection_retains_only_link_entries_and_resolved_referents() {
        let path = Path::new("root/linked/entry");
        let paths = protected_paths_with(
            path,
            |input| match input.to_str().unwrap() {
                "root/linked/entry" => Ok("target/value".into()),
                "root/linked" => Ok("target".into()),
                "root" => Ok("root".into()),
                _ => panic!("ordinary ancestors do not need separate protection"),
            },
            |input| Ok(input == path || input == Path::new("root/linked")),
        )
        .unwrap();
        assert_eq!(
            paths,
            [
                PathBuf::from("target/value"),
                PathBuf::from("target/entry"),
                PathBuf::from("root/linked"),
            ]
        );
        let ordinary = protected_paths_with(path, |_| Ok("target".into()), |_| Ok(false)).unwrap();
        assert_eq!(ordinary, [PathBuf::from("target")]);
    }

    #[test]
    fn protection_propagates_failed_identity_acquisition() {
        for fail_resolve in [false, true] {
            let error = protected_paths_with(
                Path::new("root/entry"),
                |_| {
                    if fail_resolve {
                        Err(io::Error::from(io::ErrorKind::NotADirectory).into())
                    } else {
                        Ok("target".into())
                    }
                },
                |_| Err(io::Error::from(io::ErrorKind::NotADirectory).into()),
            )
            .unwrap_err();
            assert_eq!(
                error.find_source::<io::Error>().unwrap().kind(),
                io::ErrorKind::NotADirectory
            );
        }
    }

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
        assert!(error.find_source::<CachePathConflict>().is_none());
        let context = error.find_source::<CachePathCaseUnavailable>().unwrap();
        assert_eq!(context.path, Path::new("a/cache"));
        assert_eq!(context.protected, Path::new("a/CACHE"));
    }
}
