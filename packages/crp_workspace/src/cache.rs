//! Disposable, typed observations shared by the application's acquisition subjects.

#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns storage; its child owns filesystem path admission."
)]

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crp_diag::Verbose;
use ohno::AppError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;
use toml_edit::{DocumentMut, Item, Value};

use self::paths::{protected_paths, require_direct_subject, require_disjoint};
use crate::artifact_path::{resolve_path, write_new};
use crate::git::GitRepo;
use crate::inherited::is_workspace_inherit;
use crate::manifest::{
    DEFAULT_README_FILES, PathCase, RESOURCE_KEYS, WorkspaceInherit, parse_document, resource_paths,
};
use crate::metadata::capture_metadata;
use crate::source_inputs::SourceInputs;
use crate::{ParseMetadataError, ReadFileError};

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

// The only shared mutation is an advisory-suppression latch. It guards no user data
// or partially updated observation, so a panic cannot leave inconsistent cache state.
impl UnwindSafe for Cache {}
impl RefUnwindSafe for Cache {}

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
        let mut dependency_manifests = Vec::new();
        let sources = SourceInputs::discover(
            git.root(),
            &metadata.workspace_root,
            &manifests,
            |manifest, dependency| {
                let path = manifest
                    .parent()
                    .expect("a manifest has a parent")
                    .join(dependency);
                // Retain the declaration's spelling so link entries are protected as well.
                dependency_manifests.push(path.join("Cargo.toml"));
                resolve_path(&path)
            },
        );
        let Some(mut sources) = storage_inventory(sources, verbose) else {
            return Ok(Self::default());
        };
        sources
            .files
            .extend(tracked.iter().map(|path| git.root().join(path)));
        let Some(administration) = storage_inventory(git.administrative_paths(), verbose) else {
            return Ok(Self::default());
        };
        sources.files.extend(administration);
        sources.files.extend(dependency_manifests);
        sources.files.insert(manifest.to_owned());
        let reservations = reserve_package_paths(
            &mut sources,
            case,
            |path| {
                fs::read_to_string(path)
                    .map_err(|error| ReadFileError::caused_by(path, error).into())
            },
            resolve_path,
        );
        if storage_inventory(reservations, verbose).is_none() {
            return Ok(Self::default());
        }
        let Some(directory) = resolve_directory(&requested, verbose, resolve_path) else {
            return Ok(Self::default());
        };
        // The inventory shares many ancestors; acquire each link identity once per admission.
        let mut redirects = HashMap::new();
        let paths = sources
            .files
            .iter()
            .chain(&sources.source_directories)
            .map(|path| protected_paths(path, &mut redirects))
            .collect::<Result<Vec<_>, _>>();
        let Some(paths) = storage_inventory(paths, verbose) else {
            return Ok(Self::default());
        };
        for path in paths.iter().flatten() {
            if !storage_admission(require_disjoint(&directory, path), verbose)? {
                return Ok(Self::default());
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
            for protected in protected_paths(path, &mut HashMap::new())? {
                require_disjoint(directory, &protected)?;
            }
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
        let subject = directory.join(T::SUBJECT);
        let path = subject.join(format!("{}.json", checksum(&identity)));
        get_with::<T>(
            &key,
            || {
                require_direct_subject(&subject)?;
                match fs::read(&path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(CacheReadFailed::caused_by(&path, error).into()),
                }
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

fn storage_inventory<T>(sources: Result<T, AppError>, verbose: Verbose<'_>) -> Option<T> {
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

fn storage_admission(result: Result<(), AppError>, verbose: Verbose<'_>) -> Result<bool, AppError> {
    match result {
        Err(error) if error.find_source::<paths::CachePathConflict>().is_some() => Err(error),
        result => Ok(storage_inventory(result, verbose).is_some()),
    }
}

fn reserve_package_paths(
    sources: &mut SourceInputs,
    case: PathCase,
    mut read: impl FnMut(&Path) -> Result<String, AppError>,
    mut resolve: impl FnMut(&Path) -> Result<PathBuf, AppError>,
) -> Result<(), AppError> {
    // Read only source declarations, not package identity or classification policy.
    // These additional reservations belong only to storage admission, not captured evidence.
    let manifests = sources
        .files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| case.is_manifest(name))
        })
        .cloned()
        .collect::<Vec<_>>();
    for manifest in manifests {
        let package = manifest.parent().expect("a manifest has a parent");
        sources.files.insert(package.join("build.rs"));
        for directory in ["src", "examples", "tests", "benches"] {
            sources.source_directories.insert(package.join(directory));
        }
        let document = parse_document(&manifest, &read(&manifest)?)?;
        reserve_resources(sources, &manifest, &document, &mut read)?;
        let mut reserve = |path: Option<&str>| -> Result<(), AppError> {
            if let Some(path) = path {
                let path = package.join(path);
                let parent = path.parent().ok_or_else(|| InvalidSourcePath::new(&path))?;
                // A target's directory also owns its modules. Do not recursively reserve a
                // package ancestor, which would include Cargo's default target/cache location.
                // Keep the original spelling for link-entry protection after this identity check.
                if !resolve(package)?.starts_with(resolve(parent)?) {
                    sources.source_directories.insert(parent.to_owned());
                }
                sources.files.insert(path);
            }
            Ok(())
        };
        reserve(
            document
                .get("package")
                .and_then(Item::as_table_like)
                .and_then(|package| package.get("build"))
                .and_then(Item::as_str),
        )?;
        reserve(
            document
                .get("lib")
                .and_then(Item::as_table_like)
                .and_then(|target| target.get("path"))
                .and_then(Item::as_str),
        )?;
        for kind in ["bin", "example", "test", "bench"] {
            let Some(targets) = document.get(kind) else {
                continue;
            };
            for target in targets.as_array_of_tables().into_iter().flatten() {
                reserve(target.get("path").and_then(Item::as_str))?;
            }
            for target in targets.as_array().into_iter().flatten() {
                reserve(
                    target
                        .as_inline_table()
                        .and_then(|target| target.get("path"))
                        .and_then(Value::as_str),
                )?;
            }
        }
    }
    Ok(())
}

fn reserve_resources(
    sources: &mut SourceInputs,
    manifest: &Path,
    document: &DocumentMut,
    read: &mut impl FnMut(&Path) -> Result<String, AppError>,
) -> Result<(), AppError> {
    let directory = manifest.parent().expect("a manifest has a parent");
    let workspace = WorkspaceInherit::from_root(document);
    // Reserve declarations at their own base, even when only an unselected package inherits
    // them. Resource files do not recursively own their directory like Rust modules do.
    for (table, is_package) in [
        (document.get("package").and_then(Item::as_table_like), true),
        (workspace.package, false),
    ] {
        let Some(table) = table else { continue };
        let (local, _, automatic) = resource_paths(table, &WorkspaceInherit::default());
        sources
            .files
            .extend(local.iter().map(|path| directory.join(path)));
        if automatic && is_package {
            sources
                .files
                .extend(DEFAULT_README_FILES.iter().map(|path| directory.join(path)));
        }
    }
    let Some(package) = document.get("package").and_then(Item::as_table_like) else {
        return Ok(());
    };
    if document.contains_key("workspace")
        || !RESOURCE_KEYS
            .iter()
            .any(|key| package.get(key).is_some_and(is_workspace_inherit))
    {
        return Ok(());
    }
    // Dependencies can inherit from an untracked workspace outside the selected workspace.
    // Follow the explicit root or nearest ancestor rather than using the invocation's root.
    let explicit = package.get("workspace").and_then(Item::as_str);
    let candidates = if let Some(root) = explicit {
        vec![directory.join(root).join("Cargo.toml")]
    } else {
        directory
            .ancestors()
            .skip(1)
            .map(|parent| parent.join("Cargo.toml"))
            .collect()
    };
    for root in candidates {
        let text = match read(&root) {
            Ok(text) => text,
            Err(error)
                if explicit.is_none()
                    && error
                        .find_source::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let document = parse_document(&root, &text)?;
        if document.contains_key("workspace") {
            let (_, inherited, _) =
                resource_paths(package, &WorkspaceInherit::from_root(&document));
            let directory = root.parent().expect("a manifest has a parent");
            sources
                .files
                .extend(inherited.iter().map(|path| directory.join(path)));
            sources.files.insert(root);
            return Ok(());
        }
    }
    Err(ResourceWorkspaceUnavailable::new(manifest).into())
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
    require_direct_subject(parent)?;
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
#[display("source path '{}' does not name a file", path.display())]
struct InvalidSourcePath {
    path: PathBuf,
}

/// Missing ownership prevents admission of an inherited packaging resource.
#[ohno::error]
#[display("cannot locate the workspace for inherited resources in '{}'", manifest.display())]
struct ResourceWorkspaceUnavailable {
    manifest: PathBuf,
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
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Cache: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CacheOptions: UnwindSafe, RefUnwindSafe);

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
                storage_inventory::<SourceInputs>(
                    Err(io::Error::other("missing dependency").into()),
                    verbose
                )
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
    fn admission_keeps_proven_conflicts_fatal_but_disables_incomplete_checks() {
        let recording = Recording::default();
        let verbose = Verbose::new(false, &recording);
        assert!(storage_admission(Ok(()), verbose).unwrap());
        let conflict = paths::CachePathConflict::new(Path::new("source"), Path::new("source"));
        let error = storage_admission(Err(conflict.into()), verbose).unwrap_err();
        assert!(error.find_source::<paths::CachePathConflict>().is_some());
        assert!(recording.messages.lock().unwrap().is_empty());
        assert!(
            !storage_admission(
                Err(io::Error::from(io::ErrorKind::PermissionDenied).into()),
                verbose
            )
            .unwrap()
        );
        assert_eq!(recording.messages.lock().unwrap().len(), 1);
    }

    #[test]
    fn resources_preserve_declaring_bases_and_readme_selection() {
        for (declaration, expected) in [
            (
                "[package]\nversion='uninterpreted'\n",
                vec!["README.md", "README.txt", "README"],
            ),
            ("[package]\nreadme=true\n", vec!["README.md"]),
            ("[package]\nreadme=false\n", vec![]),
            (
                "[package]\nreadme='Docs/guide.md'\nlicense-file='../shared/license'\n",
                vec!["Docs/guide.md", "../shared/license"],
            ),
            (
                "[workspace.package]\nreadme='guide.md'\nlicense-file='license'\n",
                vec!["guide.md", "license"],
            ),
            ("[workspace.package]\nlicense='MIT'\n", vec![]),
        ] {
            let manifest = Path::new("package/Cargo.toml");
            let document = parse_document(manifest, declaration).unwrap();
            let mut sources = SourceInputs::default();
            reserve_resources(&mut sources, manifest, &document, &mut |_| {
                panic!("local resource")
            })
            .unwrap();
            assert_eq!(
                sources.files,
                expected
                    .iter()
                    .map(|path| Path::new("package").join(path))
                    .collect()
            );
            assert!(sources.source_directories.is_empty());
        }
    }

    #[test]
    fn inherited_resources_follow_explicit_or_nearest_workspace_without_identity_parsing() {
        for explicit in [false, true] {
            let manifest = Path::new("external/nested/package/Cargo.toml");
            let text = format!(
                "[package]\nversion='uninterpreted'\nreadme.workspace=true\nlicense-file.workspace=true\n{}",
                if explicit {
                    "workspace='../../owner'\n"
                } else {
                    ""
                }
            );
            let root = if explicit {
                PathBuf::from("external/nested/package/../../owner/Cargo.toml")
            } else {
                PathBuf::from("external/Cargo.toml")
            };
            let document = parse_document(manifest, &text).unwrap();
            let mut sources = SourceInputs::default();
            reserve_resources(&mut sources, manifest, &document, &mut |path| {
                if path == root {
                    Ok("[workspace.package]\nreadme=true\nlicense-file='Legal/license'\n".into())
                } else {
                    assert!(!explicit);
                    assert_eq!(path, Path::new("external/nested/Cargo.toml"));
                    Err(io::Error::from(io::ErrorKind::NotFound).into())
                }
            })
            .unwrap();
            let directory = root.parent().unwrap();
            assert_eq!(
                sources.files,
                [
                    root.clone(),
                    directory.join("README.md"),
                    directory.join("Legal/license")
                ]
                .into()
            );
        }
    }

    #[test]
    fn resource_workspace_failures_cannot_admit_unchecked_storage() {
        let manifest = Path::new("package/Cargo.toml");
        let document = parse_document(manifest, "[package]\nreadme.workspace=true\n").unwrap();
        for text in [None, Some("["), Some("[package]\n")] {
            let error = reserve_resources(
                &mut SourceInputs::default(),
                manifest,
                &document,
                &mut |_| {
                    text.map(str::to_owned)
                        .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied).into())
                },
            )
            .unwrap_err();
            if text.is_none() {
                assert_eq!(
                    error.find_source::<io::Error>().unwrap().kind(),
                    io::ErrorKind::PermissionDenied
                );
            } else if text == Some("[") {
                assert!(error.find_source::<crate::ParseTomlError>().is_some());
            } else {
                assert!(
                    error
                        .find_source::<ResourceWorkspaceUnavailable>()
                        .is_some()
                );
            }
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
            reserve_package_paths(
                &mut sources,
                case,
                |_| Ok(String::new()),
                |path| Ok(path.to_owned()),
            )
            .unwrap();
            let mut expected = original;
            let mut directories = Vec::new();
            for package in ["selected", "dependency", "unselected", "lower"] {
                if package == "lower" && case == PathCase::Sensitive {
                    continue;
                }
                expected.insert(Path::new(package).join("build.rs"));
                for directory in ["src", "examples", "tests", "benches"] {
                    directories.push(Path::new(package).join(directory));
                }
            }
            assert_eq!(sources.files, expected);
            assert_eq!(
                sources.source_directories,
                directories.into_iter().collect()
            );
        }
    }

    #[test]
    fn explicit_source_reservations_preserve_paths_and_ignore_unrelated_fields() {
        let manifest = PathBuf::from("unselected/Cargo.toml");
        let mut sources = SourceInputs {
            files: [manifest.clone()].into(),
            ..SourceInputs::default()
        };
        reserve_package_paths(&mut sources, PathCase::Sensitive, |path| {
            assert_eq!(path, manifest);
            Ok("package = { version = 'not-semver', build = '../scripts/build.rs', metadata = { path = 'not-source' } }\n\
                lib = { path = 'Custom/Library.rs' }\n\
                bin = [{ path = 'custom/main.rs' }, { name = 'automatic' }]\n\
                example = [{ name = 'demo', path = 'custom/demo.rs' }]\n\
                [[test]]\npath = 'custom/test.rs'\n\
                [[bench]]\npath = 'custom/bench.rs'\n".into())
        }, |path| Ok(path.to_owned())).unwrap();
        assert_eq!(
            sources.files,
            [
                "Cargo.toml",
                "build.rs",
                "../scripts/build.rs",
                "Custom/Library.rs",
                "custom/main.rs",
                "custom/demo.rs",
                "custom/test.rs",
                "custom/bench.rs",
                "README.md",
                "README.txt",
                "README",
            ]
            .map(|path| Path::new("unselected").join(path))
            .into()
        );
        assert!(
            sources
                .source_directories
                .contains(Path::new("unselected/Custom"))
        );
        assert!(
            sources
                .source_directories
                .contains(Path::new("unselected/custom"))
        );
    }

    #[test]
    fn explicit_sources_accept_both_target_array_spellings_and_disabled_builds() {
        for declaration in [
            "bin = [{ path = 'bin.rs' }]\nexample = [{ path = 'example.rs' }]\n\
             test = [{ path = 'test.rs' }]\nbench = [{ path = 'bench.rs' }]\n",
            "[[bin]]\npath = 'bin.rs'\n[[example]]\npath = 'example.rs'\n\
             [[test]]\npath = 'test.rs'\n[[bench]]\npath = 'bench.rs'\n",
        ] {
            let mut sources = SourceInputs {
                files: [PathBuf::from("package/Cargo.toml")].into(),
                ..SourceInputs::default()
            };
            reserve_package_paths(
                &mut sources,
                PathCase::Sensitive,
                |_| Ok(format!("package = {{ build = false }}\n{declaration}")),
                |path| Ok(path.to_owned()),
            )
            .unwrap();
            assert_eq!(
                sources.files,
                [
                    "Cargo.toml",
                    "build.rs",
                    "bin.rs",
                    "example.rs",
                    "test.rs",
                    "bench.rs",
                    "README.md",
                    "README.txt",
                    "README",
                ]
                .map(|path| Path::new("package").join(path))
                .into()
            );
            assert!(!sources.source_directories.contains(Path::new("package")));
        }
    }

    #[test]
    fn package_reservations_propagate_unavailable_or_invalid_manifests() {
        for text in [None, Some("[")] {
            let mut sources = SourceInputs {
                files: [PathBuf::from("package/Cargo.toml")].into(),
                ..SourceInputs::default()
            };
            let error = reserve_package_paths(
                &mut sources,
                PathCase::Sensitive,
                |path| {
                    text.map(str::to_owned)
                        .ok_or_else(|| ReadFileError::new(path).into())
                },
                |path| Ok(path.to_owned()),
            )
            .unwrap_err();
            if text.is_none() {
                assert!(error.find_source::<ReadFileError>().is_some());
            } else {
                assert!(error.find_source::<crate::ParseTomlError>().is_some());
            }
        }
    }

    #[test]
    fn explicit_source_roots_use_resolved_identity_without_reserving_ancestors() {
        let mut sources = SourceInputs {
            files: [PathBuf::from("package/Cargo.toml")].into(),
            ..SourceInputs::default()
        };
        reserve_package_paths(
            &mut sources,
            PathCase::Sensitive,
            |_| {
                Ok("package = { build = '../build.rs' }\n\
                    lib = { path = 'self-alias/lib.rs' }\n\
                    bin = [{ path = '../shared/main.rs' }]\n"
                    .into())
            },
            |path| {
                Ok(
                    if path == Path::new("package") || path == Path::new("package/self-alias") {
                        PathBuf::from("root/package")
                    } else if path == Path::new("package/..") {
                        PathBuf::from("root")
                    } else {
                        assert_eq!(path, Path::new("package/../shared"));
                        PathBuf::from("root/shared")
                    },
                )
            },
        )
        .unwrap();
        assert_eq!(
            sources.source_directories,
            ["src", "examples", "tests", "benches", "../shared"]
                .map(|path| Path::new("package").join(path))
                .into()
        );
        assert!(
            sources
                .files
                .contains(Path::new("package/self-alias/lib.rs"))
        );
        reserve_package_paths(
            &mut sources,
            PathCase::Sensitive,
            |_| Ok("lib = { path = 'custom/lib.rs' }".into()),
            |_| Err(ReadFileError::new(Path::new("package")).into()),
        )
        .unwrap_err();
    }

    #[test]
    fn explicit_sources_require_a_file_path_without_panicking() {
        let mut sources = SourceInputs {
            files: [PathBuf::from("package/Cargo.toml")].into(),
            ..SourceInputs::default()
        };
        let error = reserve_package_paths(
            &mut sources,
            PathCase::Sensitive,
            |_| Ok("lib = { path = '/' }".into()),
            |path| Ok(path.to_owned()),
        )
        .unwrap_err();
        assert!(error.find_source::<InvalidSourcePath>().is_some());
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
