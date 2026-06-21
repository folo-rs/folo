//! Shared wiring that several commands build their real adapters from: locating
//! the configuration file, resolving the project identity, and constructing the
//! configured storage backend.

use std::path::{Path, PathBuf};

use crate::config::Config;

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
    use crate::config::parse_config;

    use super::*;

    fn config_with(extra: &str) -> Config {
        let text = format!("[storage.local]\npath = \"./data\"\n\n{extra}");
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
