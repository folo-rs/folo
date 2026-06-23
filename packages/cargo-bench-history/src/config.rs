//! Configuration loaded from `.cargo/bench_history.toml`: which project this is
//! and where its benchmark history is stored.

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
# default_branch = \"main\"      # base branch for `analyze`; auto-detected by default

[storage.local]
path = \"./bench-history\"

# To store results in Azure Blob Storage instead, replace the [storage.local]
# section above with a [storage.azure] section. The `azure` build feature must
# be enabled. Authentication is, in priority order: a self-signed account SAS
# (set `account_key`), a pre-made SAS token (set `sas_token`), or Microsoft Entra
# ID (set neither; requires an HTTPS endpoint). Entra ID is the recommended mode
# for CI, where GitHub Actions can federate into Azure without a stored secret;
# for setup, see
# https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-azure
#
# [storage.azure]
# account = \"mystorageaccount\"
# container = \"bench-history\"
# endpoint = \"https://mystorageaccount.blob.core.windows.net\"  # optional
# account_key = \"...\"   # optional: base64 account key, self-signs an account SAS
# sas_token = \"...\"     # optional: a pre-made SAS token, used verbatim. Give the
#                        # query string only, without the leading '?', for example
#                        # \"sv=2024-11-04&ss=b&srt=o&sp=r&se=2030-01-01T00:00:00Z&sig=...\"
";

/// The parsed configuration file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Config {
    /// Project identity (defaults applied during resolution, not here).
    #[serde(default)]
    pub project: ProjectConfig,
    /// Where result sets are stored.
    pub storage: StorageConfig,
}

/// Project identity section.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct ProjectConfig {
    /// Explicit project id; when absent the workspace directory name is used.
    pub id: Option<String>,
    /// The repository's default (integration) branch, used by `analyze` as the
    /// base ref when neither `--base` nor a detectable `origin/HEAD` resolves it.
    pub default_branch: Option<String>,
}

/// Where result sets are stored.
///
/// Serialized as an externally-tagged TOML table, e.g. `[storage.local]` with a
/// `path` key, so adding future backends is a new variant with its own table.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageConfig {
    /// Store result sets under a local filesystem path.
    Local {
        /// Root directory for the stored partitions.
        path: PathBuf,
    },
    /// Store result sets in an Azure Blob Storage container.
    ///
    /// Available only when the crate is built with the `azure` feature; the
    /// variant always parses so configuration is portable, but building the
    /// backend without the feature is an error.
    Azure {
        /// The storage account name (e.g. `devstoreaccount1` for Azurite).
        account: String,
        /// The blob container that holds the result-set objects.
        container: String,
        /// The blob service endpoint. Defaults to
        /// `https://{account}.blob.core.windows.net`; set it explicitly to reach
        /// an emulator such as Azurite (e.g.
        /// `http://127.0.0.1:10000/devstoreaccount1`).
        #[serde(default)]
        endpoint: Option<String>,
        /// A base64 storage account key. When set, the backend self-signs an
        /// account SAS to authenticate (the mode used against Azurite).
        #[serde(default)]
        account_key: Option<String>,
        /// A pre-made SAS token (the URL query string only, without a leading
        /// `?`, e.g. `sv=2024-11-04&ss=b&srt=o&sp=r&se=...&sig=...`). When set, it
        /// is used verbatim to authenticate. Mutually exclusive with `account_key`.
        #[serde(default)]
        sas_token: Option<String>,
    },
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
    fn default_template_has_no_engines_section() {
        // Benches run via a single `cargo bench`, with engines detected from the
        // output, so the starter config declares nothing about engines.
        assert!(!default_template().contains("[engines"));
    }

    #[test]
    fn default_template_uses_local_storage() {
        let config = parse_config(default_template()).unwrap();
        let StorageConfig::Local { path } = config.storage else {
            panic!("expected local storage, got {:?}", config.storage);
        };
        assert_eq!(path, PathBuf::from("./bench-history"));
    }

    #[test]
    fn azure_storage_with_account_key_is_parsed() {
        let text = "\
[storage.azure]
account = \"devstoreaccount1\"
container = \"bench-history\"
endpoint = \"http://127.0.0.1:10000/devstoreaccount1\"
account_key = \"a2V5\"
";
        let config = parse_config(text).unwrap();
        let StorageConfig::Azure {
            account,
            container,
            endpoint,
            account_key,
            sas_token,
        } = config.storage
        else {
            panic!("expected azure storage, got {:?}", config.storage);
        };
        assert_eq!(account, "devstoreaccount1");
        assert_eq!(container, "bench-history");
        assert_eq!(
            endpoint.as_deref(),
            Some("http://127.0.0.1:10000/devstoreaccount1")
        );
        assert_eq!(account_key.as_deref(), Some("a2V5"));
        assert_eq!(sas_token, None);
    }

    #[test]
    fn azure_storage_defaults_optional_fields_to_none() {
        let text = "\
[storage.azure]
account = \"prod\"
container = \"history\"
";
        let config = parse_config(text).unwrap();
        let StorageConfig::Azure {
            endpoint,
            account_key,
            sas_token,
            ..
        } = config.storage
        else {
            panic!("expected azure storage, got {:?}", config.storage);
        };
        assert_eq!(endpoint, None);
        assert_eq!(account_key, None);
        assert_eq!(sas_token, None);
    }

    #[test]
    fn azure_storage_with_sas_token_is_parsed() {
        let text = "\
[storage.azure]
account = \"prod\"
container = \"history\"
sas_token = \"sv=2021-08-06&sig=abc\"
";
        let config = parse_config(text).unwrap();
        let StorageConfig::Azure { sas_token, .. } = config.storage else {
            panic!("expected azure storage, got {:?}", config.storage);
        };
        assert_eq!(sas_token.as_deref(), Some("sv=2021-08-06&sig=abc"));
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
    fn project_default_branch_is_parsed_and_defaults_to_none() {
        assert_eq!(
            parse_config(default_template())
                .unwrap()
                .project
                .default_branch,
            None
        );
        let text = "\
[project]
default_branch = \"trunk\"

[storage.local]
path = \"./data\"
";
        let config = parse_config(text).unwrap();
        assert_eq!(config.project.default_branch.as_deref(), Some("trunk"));
    }

    #[test]
    fn missing_storage_is_an_error() {
        let error = parse_config("[project]\nid = \"x\"\n").unwrap_err();
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn legacy_machine_section_is_ignored() {
        // The machine key is now a run-time-only flag (`--machine-key`); a
        // `[machine]` table left over from an older config must not break parsing.
        let text = "\
[storage.local]
path = \"./data\"

[machine]
key = \"ci-pool-a\"
";
        let config = parse_config(text).unwrap();
        assert!(matches!(config.storage, StorageConfig::Local { .. }));
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

        assert!(matches!(config.storage, StorageConfig::Local { .. }));
        assert_eq!(config.project.id, None);
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
