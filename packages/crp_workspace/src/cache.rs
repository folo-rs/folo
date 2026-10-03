//! Disposable, typed observations shared by the application's acquisition subjects.

use std::cell::Cell;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crp_diag::Verbose;
use ohno::AppError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;

use crate::artifact_path::{resolve_path, write_new};
use crate::git::GitRepo;
use crate::manifest::PathCase;
use crate::metadata::capture_metadata;

/// The caller's storage selection, resolved before any prospective workspace is created.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CacheOptions {
    #[default]
    Default,
    Directory(PathBuf),
    Disabled,
}

/// A resolved location, never captured evidence or a source of freshness decisions.
///
/// Subjects own entry keys and revisions. This layer owns complete-entry admission and
/// publication; invocation memory belongs to each subject.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    directory: Option<PathBuf>,
    diagnosed: Rc<Cell<bool>>,
}

impl Cache {
    // Cargo and filesystem identity are native boundaries; admission has pure tests below.
    #[cfg_attr(test, mutants::skip)]
    pub fn resolve(manifest: &Path, options: &CacheOptions) -> Result<Self, AppError> {
        if *options == CacheOptions::Disabled {
            return Ok(Self::default());
        }
        let metadata: CacheMetadata = serde_json::from_slice(&capture_metadata(manifest)?)?;
        let directory = match options {
            CacheOptions::Directory(path) => resolve_path(path)?,
            CacheOptions::Default => resolve_path(
                &metadata
                    .target_directory
                    .join("cargo-release-plan")
                    .join("cache"),
            )?,
            CacheOptions::Disabled => unreachable!("disabled storage returned before acquisition"),
        };
        let git = GitRepo::discover(&metadata.workspace_root)?;
        let case = PathCase::probe(git.root());
        let tracked = git.ls_files("")?;
        for path in &tracked {
            let path = resolve_path(&git.root().join(path))?;
            require_disjoint(&directory, &path)?;
        }
        // Captured inputs recurse through src even when its files are untracked or ignored.
        // Protect all recorded package roots, including path dependencies outside the selected
        // workspace, without excluding any legitimate tracked source from capture.
        for path in tracked.iter().filter(|path| case.is_manifest(path)) {
            let manifest = git.root().join(path);
            if let Some(parent) = manifest.parent() {
                require_outside(&directory, &resolve_path(&parent.join("src"))?)?;
            }
        }
        for package in metadata.packages {
            if let Some(parent) = package.manifest_path.parent() {
                require_outside(&directory, &resolve_path(&parent.join("src"))?)?;
            }
        }
        // An untracked path dependency can also supply captured source.
        for ancestor in directory.ancestors() {
            if let Some(parent) = ancestor.parent()
                && case.same_path(
                    &ancestor.file_name().unwrap_or_default().to_string_lossy(),
                    "src",
                )
                && parent.join("Cargo.toml").try_exists()?
            {
                return Err(CachePathConflict::new(&directory, ancestor).into());
            }
        }
        Ok(Self {
            directory: Some(directory),
            ..Self::default()
        })
    }

    #[must_use]
    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    /// Prevents disposable entries from overwriting or becoming workflow evidence.
    #[cfg_attr(test, mutants::skip)] // Native path resolution; overlap policy is unit-tested.
    pub fn protect(&self, path: &Path) -> Result<(), AppError> {
        if let Some(directory) = &self.directory {
            require_disjoint(directory, &resolve_path(path)?)?;
        }
        Ok(())
    }

    /// Reads one admitted observation or computes it, never caching a failed acquisition.
    #[cfg_attr(test, mutants::skip)] // Native storage adapter; get_with exercises its protocol.
    pub fn get<T: CacheEntry>(
        &self,
        key: &T::Key,
        verbose: Verbose<'_>,
        acquire: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let Some(directory) = &self.directory else {
            return acquire();
        };
        let key = serde_json::to_string(key)?;
        let identity = serde_json::to_vec(&(T::SUBJECT, T::REVISION, &key))?;
        let path = directory
            .join(T::SUBJECT)
            .join(format!("{}.json", checksum(&identity)));
        get_with::<T>(
            &key,
            || match fs::read(&path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(CacheReadFailed::caused_by(&path, error).into()),
            },
            |bytes| publish(directory, &path, bytes),
            |error| {
                // Cache failures are advisory, including a closed diagnostic sink. Share the
                // latch through prospective passes to avoid repeating an unavailable-store error.
                if !self.diagnosed.replace(true) {
                    _ = verbose.sink().write(&format!(
                        "[release-plan] cache entry '{}': {error}; continuing with fresh observations (further cache diagnostics suppressed)\n",
                        path.display()
                    ));
                }
            },
            || {
                verbose.note(|| format!(
                    "acquiring {} because no compatible cache entry matches its immutable inputs",
                    T::SUBJECT
                ));
                acquire()
            },
        )
    }
}

/// Each acquisition subject names its representation and complete input identity.
/// Keys must serialize deterministically.
pub trait CacheEntry: Serialize + DeserializeOwned {
    const SUBJECT: &'static str;
    const REVISION: u32;
    type Key: Serialize;
}

/// Only the Cargo-owned location and package source roots needed for storage admission.
#[derive(Deserialize)]
struct CacheMetadata {
    target_directory: PathBuf,
    workspace_root: PathBuf,
    packages: Vec<CachePackage>,
}

#[derive(Deserialize)]
struct CachePackage {
    manifest_path: PathBuf,
}

/// An integrity-checked complete entry; its identity is checked in addition to its filename.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEntry {
    format: u32,
    subject: String,
    revision: u32,
    key: String,
    payload: String,
    checksum: String,
}

/// Increment when the storage envelope's interpretation changes.
const FORMAT: u32 = 1;

fn encode<T: CacheEntry>(key: &str, value: &T) -> Result<Vec<u8>, AppError> {
    let payload = serde_json::to_string(value)?;
    Ok(serde_json::to_vec(&StoredEntry {
        format: FORMAT,
        subject: T::SUBJECT.to_string(),
        revision: T::REVISION,
        key: key.to_string(),
        checksum: checksum(payload.as_bytes()),
        payload,
    })?)
}

fn decode<T: CacheEntry>(key: &str, bytes: &[u8]) -> Result<Option<T>, AppError> {
    let entry: StoredEntry = serde_json::from_slice(bytes).map_err(CacheCorrupt::caused_by)?;
    if entry.format != FORMAT
        || entry.subject != T::SUBJECT
        || entry.revision != T::REVISION
        || entry.key != key
    {
        return Ok(None);
    }
    if entry.checksum != checksum(entry.payload.as_bytes()) {
        return Err(CacheCorrupt::new().into());
    }
    Ok(Some(
        serde_json::from_str(&entry.payload).map_err(CacheCorrupt::caused_by)?,
    ))
}

fn get_with<T: CacheEntry>(
    key: &str,
    read: impl FnOnce() -> Result<Option<Vec<u8>>, AppError>,
    write: impl FnOnce(&[u8]) -> Result<(), AppError>,
    mut diagnose: impl FnMut(AppError),
    acquire: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    match read().and_then(|bytes| bytes.map(|bytes| decode(key, &bytes)).transpose()) {
        Ok(Some(Some(value))) => return Ok(value),
        Ok(_) => {}
        Err(error) => diagnose(error),
    }
    let value = acquire()?;
    if let Err(error) = encode(key, &value).and_then(|bytes| write(&bytes)) {
        diagnose(error);
    }
    Ok(value)
}

fn checksum(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        })
}

// Staging and rename stay on the destination filesystem. Replacement is intentional:
// concurrent writers of an identical key publish interchangeable complete observations.
#[cfg_attr(test, mutants::skip)]
fn publish(directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .expect("an entry path includes its subject directory");
    fs::create_dir_all(parent).map_err(|error| CacheWriteFailed::caused_by(path, error))?;
    // Hide only tool-owned untracked entries, not arbitrary tracked source. This also keeps
    // default placement unobtrusive in repositories that do not ignore Cargo's target directory.
    let ignore = directory.join(".gitignore");
    if !ignore
        .try_exists()
        .map_err(|error| CacheWriteFailed::caused_by(&ignore, error))?
        && let Err(error) = write_new(&ignore, |file| {
            file.write_all(b"*\n")?;
            Ok(())
        })
        && !error
            .find_source::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists)
    {
        return Err(error);
    }
    atomic_write(path, bytes)
}

#[cfg_attr(test, mutants::skip)] // Filesystem publication is covered by concurrent boundary tests.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .expect("cache entries have a parent directory");
    let mut file =
        NamedTempFile::new_in(parent).map_err(|error| CacheWriteFailed::caused_by(path, error))?;
    file.write_all(bytes)
        .map_err(|error| CacheWriteFailed::caused_by(path, error))?;
    file.persist(path)
        .map_err(|error| CacheWriteFailed::caused_by(path, error))?;
    Ok(())
}

fn require_disjoint(cache: &Path, protected: &Path) -> Result<(), AppError> {
    require_outside(cache, protected)?;
    require_outside(protected, cache)
}

fn require_outside(path: &Path, protected: &Path) -> Result<(), AppError> {
    if path.starts_with(protected) {
        return Err(CachePathConflict::new(path, protected).into());
    }
    Ok(())
}

#[ohno::error]
#[display("cache location '{}' overlaps protected source or evidence '{}'", path.display(), protected.display())]
struct CachePathConflict {
    path: PathBuf,
    protected: PathBuf,
}

#[ohno::error]
#[display("cannot read cache entry '{}'", path.display())]
struct CacheReadFailed {
    path: PathBuf,
}

#[ohno::error]
#[display("cannot publish cache entry '{}'", path.display())]
struct CacheWriteFailed {
    path: PathBuf,
}

#[ohno::error]
#[display("incomplete or corrupt cache entry")]
struct CacheCorrupt;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Observation(bool);

    impl CacheEntry for Observation {
        const SUBJECT: &'static str = "test-observation";
        const REVISION: u32 = 1;
        type Key = String;
    }

    #[test]
    fn resolved_storage_and_diagnostics_are_shared_without_enabling_disabled_storage() {
        assert!(Cache::default().directory().is_none());
        let cache = Cache {
            directory: Some("chosen".into()),
            ..Cache::default()
        };
        let clone = cache.clone();
        assert_eq!(clone.directory(), Some(Path::new("chosen")));
        assert!(!clone.diagnosed.replace(true));
        assert!(cache.diagnosed.get());
    }

    #[test]
    fn exact_identity_revision_and_integrity_are_required() {
        let bytes = encode("first", &Observation(false)).unwrap();
        assert_eq!(
            decode::<Observation>("first", &bytes).unwrap(),
            Some(Observation(false))
        );
        assert_eq!(decode::<Observation>("second", &bytes).unwrap(), None);
        for field in ["format", "revision", "subject", "key"] {
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            *value.get_mut(field).unwrap() = if field == "format" || field == "revision" {
                999.into()
            } else {
                "different".into()
            };
            assert_eq!(
                decode::<Observation>("first", &serde_json::to_vec(&value).unwrap()).unwrap(),
                None
            );
        }
        let mut entry: StoredEntry = serde_json::from_slice(&bytes).unwrap();
        entry.payload = "true".to_string();
        assert!(
            decode::<Observation>("first", &serde_json::to_vec(&entry).unwrap())
                .unwrap_err()
                .find_source::<CacheCorrupt>()
                .is_some()
        );
        decode::<Observation>("first", b"{").unwrap_err();
    }

    #[test]
    fn successful_acquisition_is_published_and_reused() {
        let stored = RefCell::new(None);
        let mut acquisitions = 0;
        for _ in 0..2 {
            let value = get_with(
                "key",
                || Ok(stored.borrow().clone()),
                |bytes| {
                    *stored.borrow_mut() = Some(bytes.to_vec());
                    Ok(())
                },
                |_| panic!("valid storage"),
                || {
                    acquisitions += 1;
                    Ok(Observation(false))
                },
            )
            .unwrap();
            assert_eq!(value, Observation(false));
        }
        assert_eq!(acquisitions, 1);
    }

    #[test]
    fn corruption_and_io_failures_are_diagnosed_but_acquisition_errors_propagate() {
        for corrupt in [false, true] {
            let mut diagnostics = 0;
            let value = get_with(
                "key",
                || {
                    if corrupt {
                        Ok(Some(b"{".to_vec()))
                    } else {
                        Err(io::Error::other("read").into())
                    }
                },
                |_| Err(io::Error::other("write").into()),
                |_| {
                    diagnostics += 1;
                },
                || Ok(Observation(true)),
            )
            .unwrap();
            assert_eq!(value, Observation(true));
            assert_eq!(diagnostics, 2);
        }
        let error = get_with::<Observation>(
            "key",
            || Ok(None),
            |_| panic!("failed acquisitions are not published"),
            |_| panic!("no cache failure"),
            || Err(io::Error::other("git").into()),
        )
        .unwrap_err();
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn protected_paths_cannot_contain_or_be_contained_by_cache() {
        for (cache, protected) in [("a", "a"), ("a", "a/b"), ("a/b", "a")] {
            assert!(require_disjoint(Path::new(cache), Path::new(protected)).is_err());
        }
        require_disjoint(Path::new("cache"), Path::new("source")).unwrap();
        require_disjoint(Path::new("cache"), Path::new("cache-other")).unwrap();
    }
}
