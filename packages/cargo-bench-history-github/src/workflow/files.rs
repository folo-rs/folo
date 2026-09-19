use std::fs::{self, FileType, Metadata, OpenOptions};
use std::io::{ErrorKind, Read as _, Seek as _, SeekFrom, Write as _};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Component, Path, PathBuf};

use ohno::AppError;

use crate::workflow::receipt::Receipt;

// Collection artifacts carry only this receipt; measurement objects stay in configured storage.
const RECEIPT_FILE: &str = "receipt.json";

// These adapters touch the real filesystem. Offline commands have native CLI integration
// coverage; in-memory receipt decoding, reconciliation and projection remain mutation targets.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn read_receipts(root: &Path) -> Result<Vec<Receipt>, AppError> {
    require_kind(root, true)?;
    let mut receipts = Vec::new();
    for directory in entries(root)? {
        require_kind(&directory, true)?;
        for entry in entries(&directory)? {
            match entry.file_name().and_then(|value| value.to_str()) {
                Some(RECEIPT_FILE) => require_kind(&entry, false)?,
                _ => return Err(InvalidArtifactPath::new(entry).into()),
            }
        }
        let receipt_path = directory.join(RECEIPT_FILE);
        receipts.push(Receipt::parse(&read_file(&receipt_path)?)?);
    }
    Ok(receipts)
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, AppError> {
    require_kind(path, false)?;
    fs::read(path).map_err(|error| ArtifactIo::caused_by("reading", path, error).into())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if checked_metadata(path)?.is_some() {
        return Err(OccupiedDestination::new(path).into());
    }
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| ArtifactIo::caused_by("creating parent directory", parent, error))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| ArtifactIo::caused_by("creating file", path, error))?;
    file.write_all(bytes)
        .map_err(|error| ArtifactIo::caused_by("writing", path, error).into())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn append_outputs(path: &Path, value: &str) -> Result<(), AppError> {
    if let Some(metadata) = checked_metadata(path)?
        && !metadata.is_file()
    {
        return Err(InvalidArtifactPath::new(path).into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .map_err(|error| ArtifactIo::caused_by("opening workflow output", path, error))?;
    if file
        .metadata()
        .map_err(|error| ArtifactIo::caused_by("inspecting workflow output", path, error))?
        .len()
        > 0
    {
        // A preceding step may omit its final newline. Keep its value intact while ensuring
        // this command's first output still occupies a separate GitHub output record.
        file.seek(SeekFrom::End(-1))
            .map_err(|error| ArtifactIo::caused_by("reading workflow output", path, error))?;
        let mut last = [0];
        file.read_exact(&mut last)
            .map_err(|error| ArtifactIo::caused_by("reading workflow output", path, error))?;
        if last != *b"\n" {
            file.write_all(b"\n").map_err(|error| {
                ArtifactIo::caused_by("separating workflow output", path, error)
            })?;
        }
    }
    file.write_all(value.as_bytes())
        .map_err(|error| ArtifactIo::caused_by("appending workflow output", path, error).into())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn directory_destination(path: &Path) -> Result<PathBuf, AppError> {
    if let Some(metadata) = checked_metadata(path)? {
        if !metadata.is_dir() || !entries(path)?.is_empty() {
            return Err(OccupiedDestination::new(path).into());
        }
        return canonical_directory(path);
    }
    // Resolve through an existing ancestor without creating a rejected destination.
    // Full path validation above rejects traversal even beyond a missing component.
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while checked_metadata(ancestor)?.is_none() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| InvalidArtifactPath::new(path))?;
        suffix.push(name);
        ancestor = ancestor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
    let mut destination = canonical_directory(ancestor)?;
    for name in suffix.into_iter().rev() {
        destination.push(name);
    }
    Ok(destination)
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn fresh_directory(path: &Path) -> Result<PathBuf, AppError> {
    let destination = directory_destination(path)?;
    fs::create_dir_all(&destination)
        .map_err(|error| ArtifactIo::caused_by("creating directory", &destination, error))?;
    canonical_directory(&destination)
}

pub(crate) fn disjoint(left: &Path, right: &Path) -> Result<(), AppError> {
    if left.starts_with(right) || right.starts_with(left) {
        return Err(OccupiedDestination::new(right).into());
    }
    Ok(())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn canonical_directory(path: &Path) -> Result<PathBuf, AppError> {
    require_kind(path, true)?;
    fs::canonicalize(path)
        .map_err(|error| ArtifactIo::caused_by("resolving directory", path, error).into())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn canonical_file(path: &Path) -> Result<PathBuf, AppError> {
    require_kind(path, false)?;
    fs::canonicalize(path)
        .map_err(|error| ArtifactIo::caused_by("resolving file", path, error).into())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn output_file(path: &Path) -> Result<PathBuf, AppError> {
    if checked_metadata(path)?.is_some() {
        return canonical_file(path);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| InvalidArtifactPath::new(path))?;
    Ok(canonical_directory(parent)?.join(name))
}

#[cfg_attr(test, mutants::skip)]
fn entries(path: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = fs::read_dir(path)
        .map_err(|error| ArtifactIo::caused_by("listing directory", path, error))?
        .map(|entry| {
            entry.map(|entry| entry.path()).map_err(|error| {
                ArtifactIo::caused_by("reading directory entry", path, error).into()
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    paths.sort();
    Ok(paths)
}

#[cfg_attr(test, mutants::skip)]
fn require_kind(path: &Path, directory: bool) -> Result<(), AppError> {
    let metadata = checked_metadata(path)?.ok_or_else(|| InvalidArtifactPath::new(path))?;
    if (directory && metadata.is_dir()) || (!directory && metadata.is_file()) {
        Ok(())
    } else {
        Err(InvalidArtifactPath::new(path).into())
    }
}

#[cfg_attr(test, mutants::skip)]
fn checked_metadata(path: &Path) -> Result<Option<Metadata>, AppError> {
    if path.components().any(|component| match component {
        Component::ParentDir => true,
        Component::Prefix(_) => !path.has_root(),
        Component::Normal(name) => name.to_str().is_some_and(|name| name.contains(':')),
        _ => false,
    }) {
        return Err(InvalidArtifactPath::new(path).into());
    }
    let mut prefix = PathBuf::new();
    let mut last = None;
    for component in path.components() {
        prefix.push(component);
        // A Windows drive or UNC prefix is not independently queryable; inspect it only
        // after the root separator is appended, including canonical verbatim paths.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ArtifactIo::caused_by("inspecting", &prefix, error).into()),
        };
        if is_link(metadata.file_type(), &metadata) {
            return Err(InvalidArtifactPath::new(prefix).into());
        }
        last = Some(metadata);
    }
    Ok(last)
}

#[cfg(windows)]
#[cfg_attr(test, mutants::skip)]
fn is_link(kind: FileType, metadata: &Metadata) -> bool {
    // Junctions and other reparse points also redirect traversal outside the artifact tree.
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    kind.is_symlink() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
#[cfg_attr(test, mutants::skip)]
fn is_link(kind: FileType, _metadata: &Metadata) -> bool {
    kind.is_symlink()
}

/// Filesystem failures retain the attempted operation and original source.
#[ohno::error]
#[display("Failed while {operation} '{}'", path.display())]
struct ArtifactIo {
    operation: String,
    path: PathBuf,
}

/// Artifact traversal accepts only ordinary directories and regular files.
#[ohno::error]
#[display("Unsafe, missing or unexpected artifact path '{}'", path.display())]
pub(crate) struct InvalidArtifactPath {
    path: PathBuf,
}

/// Run-local destinations must never mingle with earlier or unrelated data.
#[ohno::error]
#[display("Output destination must be fresh and separate from other inputs or outputs: '{}'", path.display())]
struct OccupiedDestination {
    path: PathBuf,
}

impl UnwindSafe for ArtifactIo {}
impl RefUnwindSafe for ArtifactIo {}
impl UnwindSafe for InvalidArtifactPath {}
impl RefUnwindSafe for InvalidArtifactPath {}
impl UnwindSafe for OccupiedDestination {}
impl RefUnwindSafe for OccupiedDestination {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn output_roots_cannot_overlap_in_either_direction() {
        let root = Path::new("inputs");
        for other in [root.to_path_buf(), root.join("nested")] {
            assert!(disjoint(root, &other).is_err());
            assert!(disjoint(&other, root).is_err());
        }
        disjoint(root, Path::new("inputs-other")).unwrap();
    }
}
