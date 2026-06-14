//! Shared wiring that several commands build their real adapters from: locating
//! the configuration file, resolving the project identity, and constructing the
//! configured storage backend.

use std::path::{Path, PathBuf};

use crate::config::{Config, StorageConfig};
use crate::storage::LocalStorage;

/// The default configuration path, relative to the working directory.
pub(crate) fn default_config_path() -> PathBuf {
    PathBuf::from(".cargo").join("bench_history.toml")
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

/// Builds the configured storage backend.
pub(crate) fn build_local_storage(config: &Config) -> LocalStorage {
    match &config.storage {
        StorageConfig::Local { path } => LocalStorage::new(path.clone()),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::config::parse_config;

    use super::*;

    fn config_with(engines: &str) -> Config {
        let text = format!("[storage.local]\npath = \"./data\"\n\n{engines}");
        parse_config(&text).expect("test configuration should parse")
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
        let explicit =
            config_with("[project]\nid = \"explicit\"\n[engines.callgrind]\ncommand = \"x\"\n");
        assert_eq!(
            resolve_project_id(&explicit, Path::new("/work/folo")),
            "explicit"
        );

        let implicit = config_with("[engines.callgrind]\ncommand = \"x\"\n");
        assert_eq!(
            resolve_project_id(&implicit, Path::new("/work/folo")),
            "folo"
        );
    }

    #[test]
    fn resolve_project_id_falls_back_when_directory_name_is_unavailable() {
        let config = config_with("[engines.callgrind]\ncommand = \"x\"\n");
        assert_eq!(resolve_project_id(&config, Path::new("/")), "project");
    }

    #[test]
    fn build_local_storage_uses_the_configured_path() {
        let config = config_with("[engines.callgrind]\ncommand = \"x\"\n");
        let storage = build_local_storage(&config);
        assert!(
            format!("{storage:?}").contains("data"),
            "storage should be rooted at the configured path: {storage:?}"
        );
    }
}
