//! Disposable, typed observations shared by the application's acquisition subjects.

#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns storage; its child owns filesystem path admission."
)]

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

use self::paths::require_disjoint;
use crate::ParseMetadataError;
use crate::artifact_path::{resolve_path, write_new};
use crate::git::GitRepo;
use crate::manifest::PathCase;
use crate::metadata::capture_metadata;
use crate::source_inputs::SourceInputs;

mod paths;

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
    pub fn resolve(
        manifest: &Path,
        options: &CacheOptions,
        verbose: Verbose<'_>,
    ) -> Result<Self, AppError> {
        if *options == CacheOptions::Disabled {
            return Ok(Self::default());
        }
        let metadata = CacheMetadata::parse(&capture_metadata(manifest)?)?;
        let requested = match options {
            CacheOptions::Directory(path) => path.clone(),
            CacheOptions::Default => metadata
                .target_directory
                .join("cargo-release-plan")
                .join("cache"),
            CacheOptions::Disabled => unreachable!("disabled storage returned before acquisition"),
        };
        let git = GitRepo::discover(&metadata.workspace_root)?;
        let case = PathCase::probe(git.root());
        let tracked = git.ls_files("")?;
        let manifests = metadata
            .packages
            .into_iter()
            .map(|package| package.manifest_path)
            .collect::<Vec<_>>();
        let sources = SourceInputs::discover(
            git.root(),
            &metadata.workspace_root,
            &manifests,
            |manifest, dependency| {
                resolve_path(
                    &manifest
                        .parent()
                        .expect("a manifest has a parent")
                        .join(dependency),
                )
            },
        );
        let Some(mut sources) = storage_inventory(sources, verbose) else {
            return Ok(Self::default());
        };
        sources
            .files
            .extend(tracked.iter().map(|path| git.root().join(path)));
        sources.files.extend(git.administrative_paths()?);
        reserve_package_paths(&mut sources, case);
        let Some(directory) = resolve_directory(&requested, verbose, resolve_path) else {
            return Ok(Self::default());
        };
        for path in sources.files.iter().chain(&sources.source_directories) {
            require_disjoint(&directory, &resolve_path(path)?)?;
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
                    verbose.sink().write_advisory(&format!(
                        "[release-plan] cache entry '{}': {error}; continuing with fresh observations (further cache diagnostics suppressed)\n",
                        path.display()
                    ));
                }
            },
            || {
                if verbose.enabled() {
                    verbose.sink().write_advisory(&format!(
                        "[release-plan] acquiring {} because no compatible cache entry matches its immutable inputs\n",
                        T::SUBJECT
                    ));
                }
                acquire()
            },
        )
    }
}

fn storage_inventory(
    sources: Result<SourceInputs, AppError>,
    verbose: Verbose<'_>,
) -> Option<SourceInputs> {
    match sources {
        Ok(sources) => Some(sources),
        Err(error) => {
            // This inventory is broader than classification's inputs. Failure cannot admit
            // unchecked storage, but must not make unused dependencies required evidence.
            verbose.sink().write_advisory(&format!(
                "[release-plan] cache safety inventory: {error}; continuing with storage disabled\n"
            ));
            None
        }
    }
}

fn reserve_package_paths(sources: &mut SourceInputs, case: PathCase) {
    // Reserve Cargo's autodiscovery locations without interpreting unselected manifests.
    // These additional reservations belong only to storage admission, not captured evidence.
    let packages = sources
        .files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| case.is_manifest(name))
        })
        .filter_map(|manifest| manifest.parent())
        .map(Path::to_owned)
        .collect::<Vec<_>>();
    for package in packages {
        sources.files.insert(package.join("build.rs"));
        sources.source_directories.insert(package.join("src"));
    }
}

/// Each acquisition subject names its representation and complete input identity.
///
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

impl CacheMetadata {
    fn parse(bytes: &[u8]) -> Result<Self, AppError> {
        Ok(serde_json::from_slice(bytes).map_err(ParseMetadataError::caused_by)?)
    }
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
        // Reserve the complete SHA-256 hexadecimal representation.
        .fold(String::with_capacity(64), |mut output, byte| {
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

fn resolve_directory(
    path: &Path,
    verbose: Verbose<'_>,
    resolve: impl FnOnce(&Path) -> Result<PathBuf, AppError>,
) -> Option<PathBuf> {
    match resolve(path) {
        Ok(path) => Some(path),
        Err(error) => {
            // No storage is admitted when its location cannot be resolved.
            verbose.sink().write_advisory(&format!(
                "[release-plan] cache location '{}': {error}; continuing with storage disabled\n",
                path.display()
            ));
            None
        }
    }
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
    use std::sync::Mutex;

    use crp_diag::DiagnosticSink;

    use super::*;

    /// Records advisory delivery, including a rejected write, without a process stream.
    #[derive(Debug, Default)]
    struct Recording {
        messages: Mutex<Vec<String>>,
        closed: bool,
    }

    impl DiagnosticSink for Recording {
        fn write(&self, text: &str) -> io::Result<()> {
            self.messages.lock().unwrap().push(text.to_owned());
            if self.closed {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn metadata_parsing_preserves_acquisition_context_and_json_cause() {
        for bytes in [b"[".as_slice(), b"{}", br#"{"target_directory": false}"#] {
            let error = CacheMetadata::parse(bytes).err().unwrap();
            assert!(error.find_source::<ParseMetadataError>().is_some());
            assert!(error.find_source::<serde_json::Error>().is_some());
        }
        let metadata = CacheMetadata::parse(
            br#"{"target_directory":"target","workspace_root":"root","packages":[{"manifest_path":"root/Cargo.toml"}]}"#,
        )
        .unwrap();
        assert_eq!(metadata.target_directory, Path::new("target"));
        assert_eq!(metadata.workspace_root, Path::new("root"));
        assert_eq!(
            metadata.packages.first().unwrap().manifest_path,
            Path::new("root/Cargo.toml")
        );
    }

    #[test]
    fn checksum_preserves_the_sha256_hexadecimal_representation() {
        assert_eq!(
            checksum(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn incomplete_inventory_disables_storage_and_surfaces_its_cause() {
        for closed in [false, true] {
            let recording = Recording {
                closed,
                ..Recording::default()
            };
            let verbose = Verbose::new(false, &recording);
            let sources = SourceInputs {
                files: [PathBuf::from("workspace/Cargo.toml")].into(),
                ..SourceInputs::default()
            };
            let admitted = storage_inventory(Ok(sources), verbose).unwrap();
            assert!(admitted.files.contains(Path::new("workspace/Cargo.toml")));
            assert!(recording.messages.lock().unwrap().is_empty());
            assert!(
                storage_inventory(Err(io::Error::other("missing dependency").into()), verbose)
                    .is_none()
            );
            let messages = recording.messages.lock().unwrap();
            assert_eq!(messages.len(), 1);
            let message = messages.first().unwrap();
            assert!(message.contains("missing dependency"));
            assert!(message.contains("storage disabled"));
        }
    }

    #[test]
    fn every_known_manifest_reserves_package_autodiscovery_paths() {
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let mut sources = SourceInputs {
                files: [
                    "selected/Cargo.toml",
                    "dependency/Cargo.toml",
                    "unselected/Cargo.toml",
                    "lower/cargo.toml",
                    "not-a-package/Other.toml",
                ]
                .map(PathBuf::from)
                .into(),
                ..SourceInputs::default()
            };
            let original = sources.files.clone();
            reserve_package_paths(&mut sources, case);
            let mut expected = original;
            let mut directories = Vec::new();
            for package in ["selected", "dependency", "unselected", "lower"] {
                if package == "lower" && case == PathCase::Sensitive {
                    continue;
                }
                expected.insert(Path::new(package).join("build.rs"));
                directories.push(Path::new(package).join("src"));
            }
            assert_eq!(sources.files, expected);
            assert_eq!(
                sources.source_directories,
                directories.into_iter().collect()
            );
        }
    }

    #[test]
    fn location_resolution_disables_only_storage_and_diagnoses_without_verbose() {
        for closed in [false, true] {
            let recording = Recording {
                closed,
                ..Recording::default()
            };
            let path = Path::new("cache");
            let verbose = Verbose::new(false, &recording);
            assert_eq!(
                resolve_directory(path, verbose, |input| {
                    assert_eq!(input, path);
                    Ok(PathBuf::from("resolved"))
                }),
                Some(PathBuf::from("resolved"))
            );
            assert!(recording.messages.lock().unwrap().is_empty());
            assert!(
                resolve_directory(path, verbose, |_| {
                    Err(io::Error::from(io::ErrorKind::NotADirectory).into())
                })
                .is_none()
            );
            assert_eq!(recording.messages.lock().unwrap().len(), 1);
        }
    }

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
}
