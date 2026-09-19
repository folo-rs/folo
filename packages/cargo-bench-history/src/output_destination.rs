//! Filesystem identity for report preflight. Existing ancestors are resolved before
//! missing suffixes, as in `cargo-release-plan`'s artifact-path checks.
//!
//! Filesystem access is covered by native integration tests. Destination relationships and
//! rejection before any report write are tested with in-memory substitutes under Miri/mutants.

use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf, absolute};
use std::{fs, str};

use same_file::is_same_file;
use tempfile::Builder;

/// A report's existing filesystem anchor and the suffix its writer would create.
///
/// Comparing anchors by file identity catches hard links and directory aliases. Missing
/// names are compared by the filesystem itself, not by an assumed case-folding rule.
struct Destination {
    ancestor: PathBuf,
    suffix: PathBuf,
}

impl Destination {
    #[cfg_attr(test, mutants::skip)]
    fn resolve(path: &Path) -> io::Result<Self> {
        let resolved = resolve_path(path)?;
        let mut ancestor = resolved.as_path();
        loop {
            match fs::metadata(ancestor) {
                Ok(_) => {
                    let suffix = resolved
                        .strip_prefix(ancestor)
                        .expect("ancestor is a path reached only by taking resolved's parent")
                        .to_path_buf();
                    return Ok(Self {
                        ancestor: ancestor.to_path_buf(),
                        suffix,
                    });
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    ancestor = ancestor.parent().ok_or(error)?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn destinations_conflict(left: &Path, right: &Path) -> io::Result<bool> {
    let left = Destination::resolve(left)?;
    let right = Destination::resolve(right)?;
    destinations_conflict_with(
        &left,
        &right,
        |left, right| is_same_file(left, right),
        probe_missing_destinations,
    )
}

fn destinations_conflict_with(
    left: &Destination,
    right: &Destination,
    mut same_file: impl FnMut(&Path, &Path) -> io::Result<bool>,
    probe_missing: impl FnOnce(&Destination, &Destination) -> io::Result<bool>,
) -> io::Result<bool> {
    let left_exists = left.suffix.as_os_str().is_empty();
    let right_exists = right.suffix.as_os_str().is_empty();
    // An existing output can be in the other's ancestry without sharing its nearest anchor.
    if left_exists || right_exists {
        return Ok((left_exists
            && matches_ancestor(&left.ancestor, &right.ancestor, &mut same_file)?)
            || (right_exists
                && matches_ancestor(&right.ancestor, &left.ancestor, &mut same_file)?));
    }
    // A missing component cannot contain an existing directory, so distinct anchors are independent.
    if !same_file(&left.ancestor, &right.ancestor)? {
        return Ok(false);
    }
    if left.suffix.starts_with(&right.suffix) || right.suffix.starts_with(&left.suffix) {
        return Ok(true);
    }
    probe_missing(left, right)
}

fn matches_ancestor(
    path: &Path,
    other: &Path,
    same_file: &mut impl FnMut(&Path, &Path) -> io::Result<bool>,
) -> io::Result<bool> {
    for ancestor in other.ancestors() {
        if same_file(path, ancestor)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg_attr(test, mutants::skip)]
fn probe_missing_destinations(left: &Destination, right: &Destination) -> io::Result<bool> {
    // Probe prospective names in their actual parent, using a common random prefix on the
    // first missing component. Each report is a file; only its parents can be directories.
    // The owned tree follows the writer's rules without creating any output parents.
    // No extra wrapper directory may change the first component's case-sensitivity rules.
    let mut left_components = left.suffix.components();
    let mut right_components = right.suffix.components();
    let left_name = left_components
        .next()
        .expect("empty suffixes returned before probing")
        .as_os_str();
    let right_name = right_components
        .next()
        .expect("empty suffixes returned before probing")
        .as_os_str();
    let mut builder = Builder::new();
    builder
        .prefix(".bench-history-destinations-")
        .suffix(left_name);
    let nested = !left_components.as_path().as_os_str().is_empty();
    let probe_file;
    let probe_directory;
    let probe_root = if nested {
        probe_directory = builder.tempdir_in(&left.ancestor)?;
        probe_directory.path()
    } else {
        probe_file = builder.tempfile_in(&left.ancestor)?;
        probe_file.path()
    };
    let prefix = probe_root
        .file_name()
        .expect("a generated probe has a filename")
        .as_encoded_bytes()
        .strip_suffix(left_name.as_encoded_bytes())
        .expect("the builder appends the supplied suffix");
    let mut name = OsString::from(
        str::from_utf8(prefix).expect("the explicit prefix and generated random portion are ASCII"),
    );
    name.push(right_name);
    let mut probe_left = probe_root.to_path_buf();
    let mut probe_right = right.ancestor.join(name);
    if nested {
        probe_left.push(left_components.as_path());
        fs::create_dir_all(probe_left.parent().expect("a nested report has a parent"))?;
        _ = fs::File::create_new(&probe_left)?;
    }
    if !right_components.as_path().as_os_str().is_empty() {
        probe_right.push(right_components.as_path());
    }
    let mut same_probe_file = |left: &Path, right: &Path| match is_same_file(left, right) {
        Ok(same) => Ok(same),
        // The probe can make a descendant inaccessible by placing a file in its ancestry.
        // Inspect the shorter prefixes instead of treating that failure as independence.
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(false)
        }
        Err(error) => Err(error),
    };
    Ok(
        matches_ancestor(&probe_left, &probe_right, &mut same_probe_file)?
            || matches_ancestor(&probe_right, &probe_left, &mut same_probe_file)?,
    )
}

#[cfg_attr(test, mutants::skip)]
fn resolve_path(path: &Path) -> io::Result<PathBuf> {
    let mut ancestor = absolute(path)?;
    let mut suffix = Vec::<OsString>::new();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(mut resolved) => {
                for component in suffix.into_iter().rev() {
                    match fs::metadata(&resolved) {
                        Ok(metadata) if !metadata.is_dir() => {
                            return Err(ErrorKind::NotADirectory.into());
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                    if component == ".." {
                        _ = resolved.pop();
                    } else if component != "." {
                        resolved.push(component);
                    }
                    // Re-entering an existing directory after a missing/.. pair must resolve
                    // symlinks before processing the next parent component.
                    match fs::canonicalize(&resolved) {
                        Ok(canonical) => resolved = canonical,
                        Err(error) if error.kind() == ErrorKind::NotFound => {
                            reject_unresolved_link(&resolved)?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                reject_unresolved_link(&ancestor)?;
                let Some(component) = ancestor.components().next_back().filter(|component| {
                    matches!(
                        component,
                        Component::Normal(_) | Component::ParentDir | Component::CurDir
                    )
                }) else {
                    return Err(error);
                };
                suffix.push(component.as_os_str().to_os_string());
                if !ancestor.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn reject_unresolved_link(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        // A dangling link is not a missing component that the writer would create.
        // Fail closed when its destination cannot be resolved instead of comparing its name.
        Ok(metadata) if metadata.is_symlink() => Err(ErrorKind::NotFound.into()),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn destination(ancestor: &str, suffix: &str) -> Destination {
        Destination {
            ancestor: PathBuf::from(ancestor),
            suffix: PathBuf::from(suffix),
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "matches the fallible filesystem identity callback"
    )]
    fn same_path(left: &Path, right: &Path) -> io::Result<bool> {
        Ok(left == right)
    }

    fn no_probe(_left: &Destination, _right: &Destination) -> io::Result<bool> {
        panic!("this relationship can be decided without probing missing names")
    }

    #[test]
    fn missing_equal_destinations_and_prefixes_conflict_without_a_probe() {
        let file = destination("root", "report");
        let nested = destination("root", "report/nested/outcome.txt");
        assert!(destinations_conflict_with(&file, &file, same_path, no_probe).unwrap());
        assert!(destinations_conflict_with(&file, &nested, same_path, no_probe).unwrap());
        assert!(destinations_conflict_with(&nested, &file, same_path, no_probe).unwrap());
    }

    #[test]
    fn missing_prefixes_compare_existing_parent_identity() {
        let file = destination("root/alias", "report");
        let nested = destination("root/real", "report/outcome.txt");
        assert!(
            destinations_conflict_with(
                &file,
                &nested,
                |left, right| {
                    assert_eq!(left, Path::new("root/alias"));
                    assert_eq!(right, Path::new("root/real"));
                    Ok(true)
                },
                no_probe,
            )
            .unwrap()
        );
    }

    #[test]
    fn missing_prefixes_under_different_parents_are_independent() {
        let left = destination("root/a", "report");
        let right = destination("root/b", "report/outcome.txt");
        assert!(!destinations_conflict_with(&left, &right, same_path, no_probe).unwrap());
    }

    #[test]
    fn different_missing_spellings_use_the_filesystem_probe_result() {
        let file = destination("root", "report");
        let nested = destination("root", "REPORT/outcome.txt");
        for aliases in [false, true] {
            let actual = destinations_conflict_with(&file, &nested, same_path, |left, right| {
                assert_eq!(left.suffix, Path::new("report"));
                assert_eq!(right.suffix, Path::new("REPORT/outcome.txt"));
                Ok(aliases)
            })
            .unwrap();
            assert_eq!(actual, aliases);
        }
    }

    #[test]
    fn existing_destination_cannot_be_a_missing_reports_parent_in_either_order() {
        let directory = destination("root/directory", "");
        for ancestor in ["root/directory", "root/directory/nested"] {
            let missing = destination(ancestor, "report");
            assert!(destinations_conflict_with(&directory, &missing, same_path, no_probe).unwrap());
            assert!(destinations_conflict_with(&missing, &directory, same_path, no_probe).unwrap());
        }
    }

    #[test]
    fn existing_prefixes_conflict_in_either_order() {
        let directory = destination("root/directory", "");
        let file = destination("root/directory/nested/report", "");
        assert!(destinations_conflict_with(&directory, &file, same_path, no_probe).unwrap());
        assert!(destinations_conflict_with(&file, &directory, same_path, no_probe).unwrap());
    }

    #[test]
    fn existing_hard_links_conflict_by_identity() {
        let left = destination("root/a", "");
        let right = destination("root/b", "");
        assert!(
            destinations_conflict_with(
                &left,
                &right,
                |left, right| Ok(left == Path::new("root/a") && right == Path::new("root/b")),
                no_probe,
            )
            .unwrap()
        );
    }

    #[test]
    fn existing_outputs_and_unrelated_descendants_remain_independent() {
        let file = destination("root/file", "");
        for suffix in ["", "new-report"] {
            let other = destination("root/other", suffix);
            assert!(!destinations_conflict_with(&file, &other, same_path, no_probe).unwrap());
            assert!(!destinations_conflict_with(&other, &file, same_path, no_probe).unwrap());
        }
    }

    #[test]
    fn identity_inspection_errors_propagate_for_existing_and_missing_outputs() {
        for suffix in ["", "report"] {
            let left = destination("root/a", suffix);
            let right = destination("root/b", suffix);
            let error = destinations_conflict_with(
                &left,
                &right,
                |_, _| Err(ErrorKind::PermissionDenied.into()),
                no_probe,
            )
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn missing_name_probe_errors_propagate() {
        let left = destination("root", "report");
        let right = destination("root", "other");
        let error = destinations_conflict_with(&left, &right, same_path, |_, _| {
            Err(ErrorKind::PermissionDenied.into())
        })
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    }
}
