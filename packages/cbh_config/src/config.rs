//! Configuration loaded from `.cargo/bench_history.toml`: which project this is
//! and where its benchmark history is stored.

use std::error::Error;
use std::path::Path;
use std::{fmt, io};

use serde::Deserialize;

/// The starter configuration written by `install`.
const DEFAULT_TEMPLATE: &str = "\
# cargo-bench-history configuration.
#
# Stored at .cargo/bench_history.toml. Uncomment and edit as needed.
#
# Local filesystem storage is NOT configured here, because a local path is
# machine-dependent and this file is shared (version-controlled). Select local
# storage at run time instead, on any command:
#
#   cargo bench-history collect --local=./bench-history
#   cargo bench-history collect --local            # path from CARGO_BENCH_HISTORY_STORAGE

# [project]
# id = \"my-project\"            # defaults to the workspace directory name
# default_branch = \"main\"      # base branch for `analyze`; auto-detected by default

# To store results in Azure Blob Storage, configure the cloud backend here.
# Authentication is always Microsoft Entra ID (OAuth): the endpoint must be
# HTTPS and the identity running the tool is granted data-plane access to the
# container. In CI, GitHub Actions federates into Azure without a stored secret;
# for setup, see
# https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-azure
#
# [storage.azure]
# account = \"mystorageaccount\"
# container = \"bench-history\"
# endpoint = \"https://mystorageaccount.blob.core.windows.net\"  # optional
";

/// The parsed configuration file.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct Config {
    /// Project identity (defaults applied during resolution, not here).
    #[serde(default)]
    pub project: ProjectConfig,
    /// The configured cloud storage backend, if any. Local storage is never
    /// configured here (it is a run-time `--local` selection — §storage), so this
    /// holds only cloud backends, of which exactly one may be configured. `None`
    /// means no cloud backend is configured; a command then requires `--local`.
    #[serde(default)]
    pub storage: Option<CloudStorageConfig>,
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

/// A configured cloud storage backend.
///
/// Serialized as an externally-tagged TOML table, e.g. `[storage.azure]`, so the
/// representation enforces that **exactly one** cloud backend is configured (two
/// tables fail to deserialize) and adding a future backend is a new variant with
/// its own table. Local filesystem storage is deliberately absent: it is selected
/// at run time via `--local`, never carried in this shared file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CloudStorageConfig {
    /// Store result sets in an Azure Blob Storage container.
    Azure(AzureStorageConfig),
}

/// Azure Blob Storage configuration for a `[storage.azure]` table.
///
/// Authentication is always Microsoft Entra ID (OAuth); no secret is carried in
/// this shared file. Unknown fields are rejected (`deny_unknown_fields`), so a
/// leftover legacy auth key (`account_key`, `sas_token`) names itself in a parse
/// error rather than being silently ignored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AzureStorageConfig {
    /// The storage account name (e.g. `devstoreaccount1` for Azurite).
    pub account: String,
    /// The blob container that holds the result-set objects.
    pub container: String,
    /// The blob service endpoint. Defaults to
    /// `https://{account}.blob.core.windows.net`; set it explicitly to reach an
    /// emulator such as Azurite (e.g. `https://127.0.0.1:10000/devstoreaccount1`).
    /// Entra ID requires HTTPS.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Parses a configuration from TOML source text.
///
/// # Errors
///
/// Returns a [`ConfigError`] if the text is not valid TOML or does not match
/// the configuration schema.
pub fn parse_config(text: &str) -> Result<Config, ConfigError> {
    toml::from_str(text)
        .map_err(|error| ConfigError::new(format!("failed to parse configuration: {error}")))
}

/// Loads and parses the configuration file at `path`.
///
/// A configuration file is optional: local storage is selectable at run time via
/// `--local` without one. So when `explicit` is `false` (the default
/// `.cargo/bench_history.toml` location, not a user-supplied `--config`) and the
/// file is **absent**, an empty default [`Config`] is returned rather than an
/// error. An explicitly requested file that is missing is always an error, as is
/// any other read failure or a malformed file.
///
/// # Errors
///
/// Returns a [`ConfigError`] if the file cannot be read (an explicit `--config`
/// that does not exist, or any non-"not found" I/O error), or if its contents
/// are not a valid configuration.
pub async fn load_config(path: &Path, explicit: bool) -> Result<Config, ConfigError> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !explicit => {
            return Ok(Config::default());
        }
        Err(error) => {
            return Err(ConfigError::new(format!(
                "failed to read configuration at {}: {error}",
                path.display()
            )));
        }
    };
    parse_config(&text)
}

/// The starter configuration text written by `install`.
#[must_use]
pub fn default_template() -> &'static str {
    DEFAULT_TEMPLATE
}

/// An error encountered while loading configuration.
///
/// Carries a single human-readable message rather than categorizing failures:
/// the construction sites (a file read, a TOML parse, an unresolvable
/// `--local`/`--cache` selection) each bake their context into the message, and
/// nothing downstream branches on a kind.
#[derive(Debug)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    /// Creates a configuration error carrying `message`.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
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
    fn default_template_documents_runtime_local_storage_selection() {
        // The starter config deliberately omits a local path and instead points the
        // reader at the run-time `--local` selection and its environment variable, so
        // the guidance must survive in the emitted template.
        let template = default_template();
        assert!(template.contains("--local"));
        assert!(template.contains("CARGO_BENCH_HISTORY_STORAGE"));
    }

    #[test]
    fn default_template_configures_no_active_storage() {
        // Local storage is a run-time `--local` selection and the cloud block is
        // commented out, so the starter template parses with no configured storage.
        let config = parse_config(default_template()).unwrap();
        assert_eq!(config.storage, None);
    }

    #[test]
    fn config_without_storage_parses_with_none() {
        // Storage is optional: a command can select local storage via `--local`
        // without any `[storage]` section in the file.
        let config = parse_config("[project]\nid = \"x\"\n").unwrap();
        assert_eq!(config.storage, None);
    }

    #[test]
    fn azure_storage_parses_account_container_and_endpoint() {
        let text = "\
[storage.azure]
account = \"devstoreaccount1\"
container = \"bench-history\"
endpoint = \"https://127.0.0.1:10000/devstoreaccount1\"
";
        let config = parse_config(text).unwrap();
        let Some(CloudStorageConfig::Azure(azure)) = config.storage else {
            panic!("expected azure storage, got {:?}", config.storage);
        };
        assert_eq!(azure.account, "devstoreaccount1");
        assert_eq!(azure.container, "bench-history");
        assert_eq!(
            azure.endpoint.as_deref(),
            Some("https://127.0.0.1:10000/devstoreaccount1")
        );
    }

    #[test]
    fn azure_storage_defaults_optional_fields_to_none() {
        let text = "\
[storage.azure]
account = \"prod\"
container = \"history\"
";
        let config = parse_config(text).unwrap();
        let Some(CloudStorageConfig::Azure(azure)) = config.storage else {
            panic!("expected azure storage, got {:?}", config.storage);
        };
        assert_eq!(azure.endpoint, None);
    }

    #[test]
    fn azure_storage_rejects_a_legacy_account_key_field() {
        // The legacy shared-key auth mode is gone: authentication is always Entra
        // ID. A leftover `account_key` is not silently ignored — `deny_unknown_fields`
        // makes it a loud parse error that names the offending key, forcing a clean
        // migration.
        let error = parse_config(
            "[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\naccount_key = \"a2V5\"\n",
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("account_key"),
            "unexpected parse error: {message}"
        );
    }

    #[test]
    fn azure_storage_rejects_a_legacy_sas_token_field() {
        // The legacy verbatim-SAS auth mode is gone too, and rejected the same way.
        let error = parse_config(
            "[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\nsas_token = \"sig=x\"\n",
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("sas_token"),
            "unexpected parse error: {message}"
        );
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
";
        let config = parse_config(text).unwrap();
        assert_eq!(config.project.default_branch.as_deref(), Some("trunk"));
    }

    #[test]
    fn unknown_section_is_ignored() {
        // Unknown top-level tables (e.g. a `[machine]` left over from an older
        // config, or any other section) must not break parsing; storage stays
        // unconfigured.
        let text = "\
[project]
id = \"x\"

[machine]
key = \"ci-pool-a\"
";
        let config = parse_config(text).unwrap();
        assert_eq!(config.storage, None);
    }

    #[test]
    fn leftover_storage_local_table_is_rejected() {
        // `storage` is a known field whose only cloud variant is `azure`, so a
        // leftover `[storage.local]` table is not silently ignored the way a wholly
        // unknown section is: it fails to parse, pointing the user at `--local`.
        let error = parse_config("[storage.local]\npath = \"x\"\n").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("unknown variant `local`"),
            "unexpected parse error: {message}"
        );
    }

    #[test]
    fn config_error_display_is_the_message() {
        let error = ConfigError::new("boom");
        assert_eq!(error.to_string(), "boom");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn load_config_reads_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bench_history.toml");
        std::fs::write(
            &path,
            "[project]\nid = \"folo\"\n\n[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n",
        )
        .unwrap();

        let config = load_config(&path, true).await.unwrap();

        assert!(matches!(config.storage, Some(CloudStorageConfig::Azure(_))));
        assert_eq!(config.project.id.as_deref(), Some("folo"));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn load_config_missing_explicit_file_is_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");

        // An explicitly requested file that does not exist is an error.
        let error = load_config(&path, true).await.unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("failed to read configuration"),
            "{message}"
        );
        assert!(message.contains("absent.toml"), "{message}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot access.
    async fn load_config_missing_default_file_yields_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");

        // The default location being absent is fine: storage can come from `--local`.
        let config = load_config(&path, false).await.unwrap();

        assert_eq!(config, Config::default());
    }
}
