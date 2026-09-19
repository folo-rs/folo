//! Filesystem identity for report preflight. Existing ancestors are resolved before
//! missing suffixes, as in `cargo-release-plan`'s artifact-path checks.
//!
//! This adapter is covered by native integration tests. The decision to reject a collision
//! before writing any report is tested against the in-memory `OutputWriter` under Miri/mutants.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf, absolute};
use std::str;

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
pub(crate) fn same_destination(left: &Path, right: &Path) -> io::Result<bool> {
    let left = Destination::resolve(left)?;
    let right = Destination::resolve(right)?;
    if !is_same_file(&left.ancestor, &right.ancestor)? {
        return Ok(false);
    }
    if left.suffix == right.suffix {
        return Ok(true);
    }
    if left.suffix.as_os_str().is_empty() || right.suffix.as_os_str().is_empty() {
        return Ok(false);
    }

    // Probe prospective names in their actual parent, using a common random prefix on the
    // first missing component. Directory and file names share a namespace; the owned tree
    // follows the writer's directory-creation rules without creating any output parents.
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
    let probe = Builder::new()
        .prefix(".bench-history-destinations-")
        .suffix(left_name)
        .tempdir_in(&left.ancestor)?;
    let prefix = probe
        .path()
        .file_name()
        .expect("a generated directory has a filename")
        .as_encoded_bytes()
        .strip_suffix(left_name.as_encoded_bytes())
        .expect("the builder appends the supplied suffix");
    let mut name = OsString::from(
        str::from_utf8(prefix).expect("the explicit prefix and generated random portion are ASCII"),
    );
    name.push(right_name);
    let probe_left = probe.path().join(left_components.as_path());
    let probe_right = right.ancestor.join(name).join(right_components.as_path());
    fs::create_dir_all(&probe_left)?;
    match is_same_file(probe_left, probe_right) {
        Ok(same) => Ok(same),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
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
