use std::collections::BTreeMap;
use std::fs::{self, FileType, Metadata, OpenOptions};
use std::io::{ErrorKind, Read as _, Seek as _, SeekFrom, Write as _};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Component, Path, PathBuf};

use ohno::AppError;

use crate::workflow::receipt::Receipt;

/// Downloaded receipts retain their artifact root only at the filesystem boundary.
pub(crate) struct Artifacts {
    pub(crate) receipts: Vec<Receipt>,
    pub(crate) roots: Vec<PathBuf>,
}

// Fixed artifact names keep receipt metadata separate from ordinary store objects.
pub(crate) const RECEIPT_FILE: &str = "receipt.json";
const RESULTS_DIR: &str = "results";

// Match LocalStorage's reserved atomic-write filename prefix (cbh_storage/src/local.rs).
// The independent companion copies its on-disk layout without linking the storage backend.
const TEMP_PREFIX: &str = ".cbh-tmp-";

// These adapters touch the real filesystem. Offline commands have native CLI integration
// coverage; in-memory reconciliation, object merging and projection remain mutation targets.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn read_artifacts(root: &Path) -> Result<Artifacts, AppError> {
    require_kind(root, true)?;
    let mut artifacts = Artifacts {
        receipts: Vec::new(),
        roots: Vec::new(),
    };
    for directory in entries(root)? {
        require_kind(&directory, true)?;
        for entry in entries(&directory)? {
            match entry.file_name().and_then(|value| value.to_str()) {
                Some(RECEIPT_FILE) => require_kind(&entry, false)?,
                Some(RESULTS_DIR) => require_kind(&entry, true)?,
                _ => return Err(InvalidArtifactPath::new(entry).into()),
            }
        }
        let receipt_path = directory.join(RECEIPT_FILE);
        artifacts
            .receipts
            .push(Receipt::parse(&read_file(&receipt_path)?)?);
        artifacts.roots.push(directory);
    }
    Ok(artifacts)
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn read_results(
    roots: impl IntoIterator<Item = PathBuf>,
) -> Result<BTreeMap<PathBuf, Vec<u8>>, AppError> {
    let mut objects = BTreeMap::new();
    for root in roots {
        let root = root.join(RESULTS_DIR);
        if checked_metadata(&root)?.is_none() {
            // Collecting no objects is valid. The analyzer owns the nothing-in-scope verdict.
            continue;
        }
        require_kind(&root, true)?;
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            for path in entries(&directory)? {
                let metadata =
                    checked_metadata(&path)?.ok_or_else(|| InvalidArtifactPath::new(&path))?;
                if metadata.is_dir() {
                    pending.push(path);
                } else if metadata.is_file() {
                    if is_temporary_file(&path) {
                        continue;
                    }
                    let relative = path
                        .strip_prefix(&root)
                        .expect("directory traversal constructs every path below its result root");
                    merge_object(&mut objects, relative, read_file(&path)?)?;
                } else {
                    return Err(InvalidArtifactPath::new(path).into());
                }
            }
        }
    }
    Ok(objects)
}

fn is_temporary_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(TEMP_PREFIX))
}

pub(crate) fn merge_object(
    objects: &mut BTreeMap<PathBuf, Vec<u8>>,
    relative: &Path,
    bytes: Vec<u8>,
) -> Result<(), AppError> {
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            let Component::Normal(name) = component else {
                return true;
            };
            // Store object names are portable path components, not alternate separators,
            // device syntax or Windows alternate data streams.
            name.to_str().is_none_or(|name| name.contains(['\\', ':']))
        })
    {
        return Err(InvalidArtifactPath::new(relative).into());
    }
    if let Some(existing) = objects.get(relative) {
        if *existing != bytes {
            return Err(ConflictingObject::new(relative).into());
        }
    } else {
        objects.insert(relative.to_path_buf(), bytes);
    }
    Ok(())
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
pub(crate) fn fresh_directory(path: &Path) -> Result<PathBuf, AppError> {
    if let Some(metadata) = checked_metadata(path)? {
        if !metadata.is_dir() || !entries(path)?.is_empty() {
            return Err(OccupiedDestination::new(path).into());
        }
    } else {
        fs::create_dir_all(path)
            .map_err(|error| ArtifactIo::caused_by("creating directory", path, error))?;
    }
    fs::canonicalize(path)
        .map_err(|error| ArtifactIo::caused_by("resolving directory", path, error).into())
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

/// Local composition cannot choose arbitrarily between different copies of an object.
#[ohno::error]
#[display("Selected platforms contain conflicting bytes at '{}'", path.display())]
struct ConflictingObject {
    path: PathBuf,
}

impl UnwindSafe for ArtifactIo {}
impl RefUnwindSafe for ArtifactIo {}
impl UnwindSafe for InvalidArtifactPath {}
impl RefUnwindSafe for InvalidArtifactPath {}
impl UnwindSafe for OccupiedDestination {}
impl RefUnwindSafe for OccupiedDestination {}
impl UnwindSafe for ConflictingObject {}
impl RefUnwindSafe for ConflictingObject {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn temporary_file_classification_uses_only_the_reserved_basename_prefix() {
        assert!(is_temporary_file(Path::new(".cbh-tmp-crash")));
        assert!(is_temporary_file(
            &Path::new("nested").join(".cbh-tmp-crash")
        ));
        assert!(!is_temporary_file(Path::new("object.json")));
        assert!(!is_temporary_file(Path::new(".CBH-TMP-other")));
        assert!(!is_temporary_file(
            &Path::new(".cbh-tmp-directory").join("object.json")
        ));
        assert!(!is_temporary_file(Path::new("")));
    }

    #[test]
    fn identical_objects_merge_without_inventing_metadata() {
        let path = Path::new("objects").join("run.json");
        let mut objects = BTreeMap::new();
        merge_object(&mut objects, &path, b"ordinary bytes".to_vec()).unwrap();
        merge_object(&mut objects, &path, b"ordinary bytes".to_vec()).unwrap();
        assert_eq!(
            objects,
            BTreeMap::from([(path.clone(), b"ordinary bytes".to_vec())])
        );
        let error = merge_object(&mut objects, &path, b"conflict".to_vec()).unwrap_err();
        assert!(error.find_source::<ConflictingObject>().is_some());
        assert_eq!(objects.get(&path).unwrap(), b"ordinary bytes");
    }

    #[test]
    fn relative_object_paths_cannot_escape_or_inject_platform_syntax() {
        let mut objects = BTreeMap::new();
        for path in [
            PathBuf::new(),
            Path::new("..").join("outside"),
            Path::new("objects").join("..").join("outside"),
            PathBuf::from("object:stream"),
        ] {
            let error = merge_object(&mut objects, &path, vec![]).unwrap_err();
            assert!(error.find_source::<InvalidArtifactPath>().is_some());
        }
        assert!(objects.is_empty());
    }

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
