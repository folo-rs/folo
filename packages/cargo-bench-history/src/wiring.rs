//! Shared wiring that several commands build their real adapters from: locating
//! the configuration file, resolving the project identity, and constructing the
//! configured storage backend.

use std::path::{Path, PathBuf};

use cbh_config::Config;
use cbh_storage::StorageError;

use crate::command::{CacheSelection, LocalStorageSelection};

/// The environment variable that supplies the local-storage path for a bare
/// `--local` (given with no value): `CARGO_BENCH_HISTORY_STORAGE`.
pub(crate) const STORAGE_ENV_VAR: &str = "CARGO_BENCH_HISTORY_STORAGE";

/// The environment variable that supplies the cache directory for a bare
/// `--cache` (given with no value): `CARGO_BENCH_HISTORY_CACHE`.
pub(crate) const CACHE_ENV_VAR: &str = "CARGO_BENCH_HISTORY_CACHE";

/// The default configuration path, relative to the working directory.
pub(crate) fn default_config_path() -> PathBuf {
    PathBuf::from(".cargo").join("bench_history.toml")
}

/// Joins a relative `path` onto `base`, leaving an absolute `path` unchanged.
///
/// In production `base` is the process working directory, so a relative path
/// resolves exactly as the filesystem would have. Threading the base explicitly
/// (rather than relying on the process current directory) lets tests point each
/// command at its own workspace without a global `chdir`, so the suite need not be
/// forced serial.
pub(crate) fn rebase(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Resolves the configuration path: an explicit `--config` value or the default
/// `.cargo/bench_history.toml`, taken relative to `workspace_dir`.
pub(crate) fn resolve_config_path(workspace_dir: &Path, explicit: Option<&Path>) -> PathBuf {
    let path = explicit.map_or_else(default_config_path, Path::to_path_buf);
    rebase(workspace_dir, path)
}

/// Resolves the repository directory the `analyze`-family commands query: an
/// explicit `--repo` (relative to `workspace_dir`) or `workspace_dir` itself.
pub(crate) fn resolve_repo(workspace_dir: &Path, repo: Option<&Path>) -> PathBuf {
    repo.map_or_else(
        || workspace_dir.to_path_buf(),
        |repo| rebase(workspace_dir, repo.to_path_buf()),
    )
}

/// Reads the local-storage path environment variable, if set.
///
/// This is the single environment read behind `--local`; it lives at the IO edge
/// so the pure [`resolve_local_path`] resolver stays testable and Miri-safe. An
/// unset variable yields `None`; a set-but-empty value yields `Some("")`, leaving
/// the empty/non-empty distinction to the resolver.
///
/// Not mutation-tested: like the `entra_credential` env read in the storage
/// layer it is a bare `std::env::var` read whose behaviour cannot be pinned
/// without mutating the global process environment (which the suite avoids). All
/// the meaningful logic lives in [`resolve_local_path`], which stays fully under
/// mutation; the only coverage of the reader itself is a subprocess-spawning
/// end-to-end test, and forcing the parallel mutation run to reach that slow test
/// just to kill a no-signal IO edge is what times it out.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn storage_env() -> Option<String> {
    std::env::var(STORAGE_ENV_VAR).ok()
}

/// Resolves the `--local` selection into a concrete local-storage path.
///
/// `env` is the value of [`STORAGE_ENV_VAR`], read at the IO edge via
/// [`storage_env`]. Returns `Ok(None)` when `--local` was not given (the caller
/// then uses the configured cloud backend), `Ok(Some(path))` for an explicit
/// `--local=<path>` or a bare `--local` backed by a non-empty environment value,
/// and an error when a bare `--local` has no usable environment value.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if a bare `--local` was given but
/// [`STORAGE_ENV_VAR`] is unset or empty.
pub(crate) fn resolve_local_path(
    selection: Option<&LocalStorageSelection>,
    env: Option<&str>,
) -> Result<Option<PathBuf>, StorageError> {
    match selection {
        None => Ok(None),
        Some(LocalStorageSelection::Path(path)) => Ok(Some(path.clone())),
        Some(LocalStorageSelection::FromEnv) => match env {
            Some(value) if !value.is_empty() => Ok(Some(PathBuf::from(value))),
            _ => Err(StorageError::Config {
                message: format!(
                    "--local was given without a path and {STORAGE_ENV_VAR} is unset or empty"
                ),
            }),
        },
    }
}

/// Reads the cache-directory environment variable, if set.
///
/// The cache-side counterpart of [`storage_env`]: the single environment read
/// behind a bare `--cache`, kept at the IO edge so the pure [`resolve_cache_path`]
/// resolver stays testable and Miri-safe.
///
/// Not mutation-tested, for the same reason as [`storage_env`]: it is a bare
/// `std::env::var` read with no signal beyond the IO edge, and
/// [`resolve_cache_path`] carries all the logic that stays under mutation.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn cache_env() -> Option<String> {
    std::env::var(CACHE_ENV_VAR).ok()
}

/// Resolves the `--cache` selection into a concrete cache directory.
///
/// The cache-side counterpart of [`resolve_local_path`]. `env` is the value of
/// [`CACHE_ENV_VAR`], read at the IO edge via [`cache_env`]. Returns `Ok(None)`
/// when `--cache` was not given (the caller then reads the cloud directly),
/// `Ok(Some(path))` for an explicit `--cache=<path>` or a bare `--cache` backed by
/// a non-empty environment value, and an error when a bare `--cache` has no usable
/// environment value.
///
/// # Errors
///
/// Returns [`StorageError::Config`] if a bare `--cache` was given but
/// [`CACHE_ENV_VAR`] is unset or empty.
pub(crate) fn resolve_cache_path(
    selection: Option<&CacheSelection>,
    env: Option<&str>,
) -> Result<Option<PathBuf>, StorageError> {
    match selection {
        None => Ok(None),
        Some(CacheSelection::Path(path)) => Ok(Some(path.clone())),
        Some(CacheSelection::FromEnv) => match env {
            Some(value) if !value.is_empty() => Ok(Some(PathBuf::from(value))),
            _ => Err(StorageError::Config {
                message: format!(
                    "--cache was given without a path and {CACHE_ENV_VAR} is unset or empty"
                ),
            }),
        },
    }
}

/// Resolves the project identity: explicit config value, else the directory name.
pub(crate) fn resolve_project_id(config: &Config, workspace_dir: &Path) -> String {
    if let Some(id) = &config.project.id {
        return id.clone();
    }
    workspace_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_config::parse_config;

    use super::*;

    fn config_with(extra: &str) -> Config {
        let text = format!("[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n\n{extra}");
        parse_config(&text).unwrap()
    }

    #[test]
    fn resolve_local_path_returns_none_when_not_selected() {
        assert_eq!(resolve_local_path(None, Some("/env/store")).unwrap(), None);
    }

    #[test]
    fn resolve_local_path_uses_explicit_path() {
        let selection = LocalStorageSelection::Path(PathBuf::from("./store"));
        assert_eq!(
            resolve_local_path(Some(&selection), None).unwrap(),
            Some(PathBuf::from("./store"))
        );
    }

    #[test]
    fn resolve_local_path_reads_env_for_bare_local() {
        assert_eq!(
            resolve_local_path(Some(&LocalStorageSelection::FromEnv), Some("/env/store")).unwrap(),
            Some(PathBuf::from("/env/store"))
        );
    }

    #[test]
    fn resolve_local_path_errors_when_env_unset() {
        let error = resolve_local_path(Some(&LocalStorageSelection::FromEnv), None).unwrap_err();
        let StorageError::Config { message } = error else {
            panic!("expected a config error, got {error:?}");
        };
        assert!(message.contains(STORAGE_ENV_VAR), "{message}");
    }

    #[test]
    fn resolve_local_path_treats_empty_env_as_unset() {
        let error =
            resolve_local_path(Some(&LocalStorageSelection::FromEnv), Some("")).unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    fn resolve_cache_path_returns_none_when_not_selected() {
        assert_eq!(resolve_cache_path(None, Some("/env/cache")).unwrap(), None);
    }

    #[test]
    fn resolve_cache_path_uses_explicit_path() {
        let selection = CacheSelection::Path(PathBuf::from("./cache"));
        assert_eq!(
            resolve_cache_path(Some(&selection), None).unwrap(),
            Some(PathBuf::from("./cache"))
        );
    }

    #[test]
    fn resolve_cache_path_reads_env_for_bare_cache() {
        assert_eq!(
            resolve_cache_path(Some(&CacheSelection::FromEnv), Some("/env/cache")).unwrap(),
            Some(PathBuf::from("/env/cache"))
        );
    }

    #[test]
    fn resolve_cache_path_errors_when_env_unset() {
        let error = resolve_cache_path(Some(&CacheSelection::FromEnv), None).unwrap_err();
        let StorageError::Config { message } = error else {
            panic!("expected a config error, got {error:?}");
        };
        assert!(message.contains(CACHE_ENV_VAR), "{message}");
    }

    #[test]
    fn resolve_cache_path_treats_empty_env_as_unset() {
        let error = resolve_cache_path(Some(&CacheSelection::FromEnv), Some("")).unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    fn default_config_path_is_under_dot_cargo() {
        assert_eq!(
            default_config_path(),
            PathBuf::from(".cargo").join("bench_history.toml")
        );
    }

    #[test]
    fn resolve_project_id_prefers_explicit_then_directory_name() {
        let explicit = config_with("[project]\nid = \"explicit\"\n");
        assert_eq!(
            resolve_project_id(&explicit, Path::new("/work/folo")),
            "explicit"
        );

        let implicit = config_with("");
        assert_eq!(
            resolve_project_id(&implicit, Path::new("/work/folo")),
            "folo"
        );
    }

    #[test]
    fn resolve_project_id_falls_back_when_directory_name_is_unavailable() {
        let config = config_with("");
        assert_eq!(resolve_project_id(&config, Path::new("/")), "project");
    }

    #[test]
    fn rebase_joins_relative_paths_and_keeps_absolute_ones() {
        let base = Path::new("/work/folo");
        assert_eq!(
            rebase(base, PathBuf::from("store")),
            PathBuf::from("/work/folo/store")
        );
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\elsewhere\store")
        } else {
            PathBuf::from("/elsewhere/store")
        };
        assert_eq!(rebase(base, absolute.clone()), absolute);
    }

    #[test]
    fn resolve_config_path_uses_the_default_under_the_workspace() {
        let base = Path::new("/work/folo");
        assert_eq!(
            resolve_config_path(base, None),
            base.join(".cargo").join("bench_history.toml")
        );
        assert_eq!(
            resolve_config_path(base, Some(Path::new("custom.toml"))),
            base.join("custom.toml")
        );
    }

    #[test]
    fn resolve_repo_defaults_to_the_workspace_directory() {
        let base = Path::new("/work/folo");
        assert_eq!(resolve_repo(base, None), base.to_path_buf());
        assert_eq!(
            resolve_repo(base, Some(Path::new("nested"))),
            base.join("nested")
        );
    }
}
