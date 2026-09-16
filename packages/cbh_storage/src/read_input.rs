use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use cbh_config::Config;
use cbh_diag::{Reporter, ReporterExt};
use same_file::Handle;
use tokio::task::spawn_blocking;

use crate::{
    LocalStorage, ReadStorage, StorageConfigurationError, StorageError, StorageFacade,
    ValidateLocalInputError, resolve_storage,
};

/// Selects a read-only baseline with optional additional local input.
///
/// Baseline selection and caching follow [`resolve_storage`]. The additional
/// input uses ordinary stored-object keys and takes precedence for matching keys.
/// Its path resolves relative to `base`, independently of the repository selected
/// for topology queries.
///
/// # Errors
///
/// Returns a [`StorageError`] if baseline selection fails, the input is not an
/// existing directory, or the input and cache-mirror directories overlap.
// Production adapter construction and filesystem validation are integration-tested;
// the in-process ReadStorage tests cover composition and mutation isolation.
#[cfg_attr(test, mutants::skip)]
pub async fn resolve_read_storage(
    storage_override: Option<StorageFacade>,
    local: Option<&Path>,
    config: &Config,
    base: &Path,
    cache: Option<&Path>,
    local_input: Option<&Path>,
    reporter: &dyn Reporter,
) -> Result<ReadStorage<StorageFacade, LocalStorage>, StorageError> {
    let baseline = resolve_storage(storage_override, local, config, base, cache, reporter)?;
    let input = match local_input {
        Some(path) => {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                base.join(path)
            };
            let mirror = match &baseline {
                StorageFacade::CachedAzure(storage) => Some(storage.cache().root().to_path_buf()),
                StorageFacade::Local(_) | StorageFacade::Azure(_) => None,
            };
            spawn_blocking({
                let path = path.clone();
                move || validate_input_directory(&path, mirror.as_deref())
            })
            .await
            .map_err(|error| ValidateLocalInputError::caused_by(path.clone(), error))??;
            reporter.note_with(|| {
                format!(
                    "local input {} supplements the selected baseline because --local-input \
                     was supplied; matching local objects take precedence, and neither source \
                     is written by this query",
                    path.display()
                )
            });
            Some(LocalStorage::new(path))
        }
        None => None,
    };
    Ok(ReadStorage::new(baseline, input))
}

// This adapter inspects real directory identities, including aliases and mount-specific
// case behavior. Integration tests exercise it without substituting textual path equality.
#[cfg_attr(test, mutants::skip)]
fn validate_input_directory(input: &Path, mirror: Option<&Path>) -> Result<(), StorageError> {
    let metadata = fs::metadata(input)
        .map_err(|error| ValidateLocalInputError::caused_by(input.to_path_buf(), error))?;
    if !metadata.is_dir() {
        return Err(StorageConfigurationError::new(format!(
            "--local-input must identify an existing directory: {}",
            input.display()
        ))
        .into());
    }
    let Some(mirror) = mirror else {
        return Ok(());
    };
    let input = fs::canonicalize(input)
        .map_err(|error| ValidateLocalInputError::caused_by(input.to_path_buf(), error))?;
    let mirror = resolve_mirror_path(mirror)?;
    if contains_directory(&input, &mirror)? || contains_directory(&mirror, &input)? {
        return Err(StorageConfigurationError::new(format!(
            "local input {} and cache mirror {} must be disjoint directories",
            input.display(),
            mirror.display()
        ))
        .into());
    }
    Ok(())
}

// Cache directories are created lazily. Resolve the existing ancestor physically and
// retain only ordinary missing path components, without creating anything during validation.
#[cfg_attr(test, mutants::skip)]
fn resolve_mirror_path(path: &Path) -> Result<PathBuf, StorageError> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in missing.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(
                        ValidateLocalInputError::caused_by(path.to_path_buf(), error).into(),
                    );
                };
                missing.push(name.to_owned());
                ancestor = ancestor
                    .parent()
                    .expect("a path with a final filename has a parent");
            }
            Err(error) => {
                return Err(ValidateLocalInputError::caused_by(path.to_path_buf(), error).into());
            }
        }
    }
}

// Comparing open filesystem identities, not path strings, handles case-insensitive
// mounts and aliases without guessing their behavior from the operating system.
#[cfg_attr(test, mutants::skip)]
fn contains_directory(parent: &Path, child: &Path) -> Result<bool, StorageError> {
    let parent = match Handle::from_path(parent) {
        Ok(parent) => parent,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(ValidateLocalInputError::caused_by(parent.to_path_buf(), error).into());
        }
    };
    for ancestor in child.ancestors() {
        match Handle::from_path(ancestor) {
            Ok(candidate) if candidate == parent => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(
                    ValidateLocalInputError::caused_by(ancestor.to_path_buf(), error).into(),
                );
            }
        }
    }
    Ok(false)
}
