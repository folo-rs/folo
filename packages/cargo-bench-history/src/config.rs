//! Configuration loaded from `.cargo/bench_history.toml`: which project this is,
//! where results are stored, and the command that launches each benchmark engine.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The starter configuration written by `install`.
const DEFAULT_TEMPLATE: &str = "\
# cargo-bench-history configuration.
#
# Stored at .cargo/bench_history.toml. Uncomment and edit as needed.

# [project]
# id = \"my-project\"            # defaults to the workspace directory name

[storage.local]
path = \"./bench-history\"

# Declare the command that launches each engine's benches in this workspace.
# Omit an engine section entirely to leave that engine unused here.

[engines.callgrind]
command = \"just bench-cg\"

[engines.criterion]
command = \"cargo bench\"
";

/// The parsed configuration file.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Config {
    /// Project identity (defaults applied during resolution, not here).
    #[serde(default)]
    pub project: ProjectConfig,
    /// Where result sets are stored.
    pub storage: StorageConfig,
    /// Per-engine launch commands, keyed by engine name (e.g. `callgrind`).
    #[serde(default)]
    pub engines: BTreeMap<String, EngineConfig>,
}

/// Project identity section.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ProjectConfig {
    /// Explicit project id; when absent the workspace directory name is used.
    pub id: Option<String>,
}

/// Where result sets are stored.
///
/// Serialized as an externally-tagged TOML table, e.g. `[storage.local]` with a
/// `path` key, so adding future backends is a new variant with its own table.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageConfig {
    /// Store result sets under a local filesystem path.
    Local {
        /// Root directory for the stored partitions.
        path: PathBuf,
    },
}

/// The command that launches one engine's benches.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EngineConfig {
    /// The shell command to run (e.g. `just bench-cg`).
    pub command: String,
    /// Extra arguments appended to the command (escape hatch; usually empty).
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Parses a configuration from TOML source text.
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if the text is not valid TOML or does not match
/// the configuration schema.
pub fn parse_config(text: &str) -> Result<Config, ConfigError> {
    toml::from_str(text).map_err(|error| ConfigError::Parse(error.to_string()))
}

/// Loads and parses the configuration file at `path`.
///
/// # Errors
///
/// Returns [`ConfigError::Read`] if the file cannot be read (for example, it does
/// not exist), or [`ConfigError::Parse`] if its contents are not a valid
/// configuration.
pub(crate) async fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| ConfigError::Read {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    parse_config(&text)
}

/// The starter configuration text written by `install`.
#[must_use]
pub fn default_template() -> &'static str {
    DEFAULT_TEMPLATE
}

/// An error encountered while loading configuration.
#[non_exhaustive]
#[derive(Debug)]
pub enum ConfigError {
    /// The configuration file could not be read.
    Read {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error, rendered as text.
        message: String,
    },
    /// The configuration text could not be parsed.
    Parse(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message } => {
                write!(
                    f,
                    "failed to read configuration at {}: {message}",
                    path.display()
                )
            }
            Self::Parse(message) => {
                write!(f, "failed to parse configuration: {message}")
            }
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses_with_two_engines() {
        let config = parse_config(default_template()).unwrap();
        assert_eq!(config.engines.len(), 2);
        assert!(config.engines.contains_key("callgrind"));
        assert!(config.engines.contains_key("criterion"));
    }

    #[test]
    fn default_template_uses_local_storage() {
        let config = parse_config(default_template()).unwrap();
        let StorageConfig::Local { path } = config.storage;
        assert_eq!(path, PathBuf::from("./bench-history"));
    }

    #[test]
    fn project_id_defaults_to_none() {
        let config = parse_config(default_template()).unwrap();
        assert_eq!(config.project.id, None);
    }

    #[test]
    fn explicit_project_id_is_parsed() {
        let text = "\
[project]
id = \"folo\"

[storage.local]
path = \"./data\"
";
        let config = parse_config(text).unwrap();
        assert_eq!(config.project.id.as_deref(), Some("folo"));
    }

    #[test]
    fn engine_extra_args_default_to_empty() {
        let text = "\
[storage.local]
path = \"./data\"

[engines.criterion]
command = \"cargo bench\"
";
        let config = parse_config(text).unwrap();
        assert!(
            config
                .engines
                .get("criterion")
                .unwrap()
                .extra_args
                .is_empty()
        );
    }

    #[test]
    fn missing_storage_is_an_error() {
        let error = parse_config("[project]\nid = \"x\"\n").unwrap_err();
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn config_error_display_includes_message() {
        let error = ConfigError::Parse("boom".to_owned());
        assert_eq!(error.to_string(), "failed to parse configuration: boom");
    }

    #[test]
    fn read_error_display_includes_path() {
        let error = ConfigError::Read {
            path: PathBuf::from("/tmp/x.toml"),
            message: "not found".to_owned(),
        };
        let rendered = error.to_string();
        assert!(rendered.contains("x.toml"), "{rendered}");
        assert!(rendered.contains("not found"), "{rendered}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn load_config_reads_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bench_history.toml");
        std::fs::write(&path, default_template()).unwrap();

        let config = load_config(&path).await.unwrap();

        assert_eq!(config.engines.len(), 2);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn load_config_missing_file_is_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");

        let error = load_config(&path).await.unwrap_err();

        assert!(matches!(error, ConfigError::Read { .. }));
    }
}
