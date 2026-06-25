//! The storage backend the dataset is seeded into and analyzed against.
//!
//! Both backends share one on-disk shape: the seeder writes the blob tree to a
//! directory whose paths are the storage keys. For local storage that directory
//! *is* the store. For Azure it is a staging area that `azcopy` bulk-uploads into
//! a freshly created container; the container is deleted afterward unless `--keep`
//! is set. Only the write side is replicated here — measurement reads back through
//! the real `cargo-bench-history` storage code, so the two stay in lockstep.

use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tokio::process::Command;

use crate::error::{Error, fail};
use crate::logging::Logger;

/// A seeded storage backend: where to write the tree, the config that points
/// `analyze` at it, and how to provision/upload/clean it up.
#[derive(Debug)]
pub(crate) struct StorageTarget {
    /// Backend-specific identity.
    kind: Kind,
    /// The directory the seeder writes the blob tree into (store or staging).
    root: PathBuf,
    /// A temporary directory backing `root`, when the harness created one.
    scratch: Option<TempDir>,
}

/// Backend-specific identity and connection details.
#[derive(Debug)]
enum Kind {
    /// Local filesystem storage; `root` is the store itself.
    Local,
    /// Azure Blob storage; `root` is a staging area uploaded to the container.
    Azure {
        /// Storage account name.
        account: String,
        /// Blob container name (created and, unless kept, deleted).
        container: String,
    },
}

impl StorageTarget {
    /// Creates a local-filesystem target.
    ///
    /// With `dir` set the store is that directory (kept across runs); without it
    /// a temporary directory is used and removed on exit unless `--keep`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created.
    pub(crate) fn local(dir: Option<PathBuf>) -> Result<Self, Error> {
        match dir {
            Some(dir) => {
                std::fs::create_dir_all(&dir).map_err(|error| {
                    fail(format!("failed to create {}: {error}", dir.display()))
                })?;
                let root = absolute(&dir)?;
                Ok(Self {
                    kind: Kind::Local,
                    root,
                    scratch: None,
                })
            }
            None => {
                let scratch = TempDir::new()
                    .map_err(|error| fail(format!("failed to create a temp store: {error}")))?;
                let root = scratch.path().to_path_buf();
                Ok(Self {
                    kind: Kind::Local,
                    root,
                    scratch: Some(scratch),
                })
            }
        }
    }

    /// Creates an Azure-blob target staging into a temporary directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the staging directory cannot be created.
    pub(crate) fn azure(account: String, container: String) -> Result<Self, Error> {
        let scratch = TempDir::new()
            .map_err(|error| fail(format!("failed to create an upload staging dir: {error}")))?;
        let root = scratch.path().to_path_buf();
        Ok(Self {
            kind: Kind::Azure { account, container },
            root,
            scratch: Some(scratch),
        })
    }

    /// The directory the seeder writes the blob tree into.
    pub(crate) fn seed_root(&self) -> &Path {
        &self.root
    }

    /// A short human-readable label for this backend, for the report.
    pub(crate) fn label(&self) -> String {
        match &self.kind {
            Kind::Local => "local filesystem".to_owned(),
            Kind::Azure { account, container } => {
                format!("azure ({account}/{container})")
            }
        }
    }

    /// The `.cargo/bench_history.toml` contents pointing `analyze` at this store.
    pub(crate) fn config_toml(&self) -> String {
        let project = crate::scenario::PROJECT;
        match &self.kind {
            Kind::Local => format!(
                "[project]\nid = \"{project}\"\n\n[storage.local]\npath = {}\n",
                toml_path(&self.root)
            ),
            Kind::Azure { account, container } => azure_config_toml(project, account, container),
        }
    }

    /// Provisions the backend before seeding (Azure: create the container).
    ///
    /// # Errors
    ///
    /// Returns an error if provisioning fails.
    pub(crate) async fn provision(&self, logger: Logger) -> Result<(), Error> {
        match &self.kind {
            Kind::Local => {
                logger.detail(&format!("local store directory is {}", self.root.display()));
                Ok(())
            }
            Kind::Azure { account, container } => {
                logger.step(&format!(
                    "creating Azure blob container {container} in account {account}"
                ));
                logger.detail(
                    "a fresh container per run keeps stress data isolated and lets cleanup delete \
                     the whole container in one call",
                );
                az(
                    &[
                        "storage",
                        "container",
                        "create",
                        "--account-name",
                        account,
                        "--name",
                        container,
                        "--auth-mode",
                        "login",
                        "--only-show-errors",
                    ],
                    logger,
                )
                .await
            }
        }
    }

    /// Uploads the seeded tree (Azure only; a no-op for local storage).
    ///
    /// # Errors
    ///
    /// Returns an error if the upload fails.
    pub(crate) async fn upload(&self, logger: Logger) -> Result<(), Error> {
        let Kind::Azure { account, container } = &self.kind else {
            return Ok(());
        };
        logger.step(&format!(
            "uploading the staged tree to container {container} with azcopy"
        ));
        logger.detail(
            "azcopy authenticates as the current az login session (AZCOPY_AUTO_LOGIN_TYPE=AZCLI), \
             so the same Entra identity is used locally and under workload-identity federation",
        );
        let source = wildcard_source(&self.root);
        let destination = format!("https://{account}.blob.core.windows.net/{container}");
        run_tool(
            "azcopy",
            &[
                "copy",
                &source,
                &destination,
                "--recursive",
                "--overwrite=true",
                "--output-type=text",
            ],
            &[("AZCOPY_AUTO_LOGIN_TYPE", "AZCLI")],
            logger,
        )
        .await
    }

    /// Cleans up after measuring. Azure deletes the container unless `keep`; a
    /// kept local temp store is persisted, otherwise the temp directory is removed.
    ///
    /// # Errors
    ///
    /// Returns an error if container deletion fails.
    pub(crate) async fn cleanup(&mut self, keep: bool, logger: Logger) -> Result<(), Error> {
        match &self.kind {
            Kind::Local => {
                if keep {
                    if let Some(scratch) = self.scratch.take() {
                        let kept = scratch.keep();
                        logger.step(&format!("kept local store at {}", kept.display()));
                    } else {
                        logger.step(&format!("kept local store at {}", self.root.display()));
                    }
                }
                Ok(())
            }
            Kind::Azure { account, container } => {
                if keep {
                    logger.step(&format!(
                        "keeping Azure container {container} (delete it manually when done)"
                    ));
                    return Ok(());
                }
                logger.step(&format!("deleting Azure blob container {container}"));
                az(
                    &[
                        "storage",
                        "container",
                        "delete",
                        "--account-name",
                        account,
                        "--name",
                        container,
                        "--auth-mode",
                        "login",
                        "--only-show-errors",
                    ],
                    logger,
                )
                .await
            }
        }
    }
}

/// Resolves a path to an absolute one without requiring it to exist on all paths.
fn absolute(path: &Path) -> Result<PathBuf, Error> {
    std::path::absolute(path)
        .map_err(|error| fail(format!("failed to resolve {}: {error}", path.display())))
}

/// Formats a path as a TOML basic string with backslashes escaped.
fn toml_path(path: &Path) -> String {
    let escaped = path.display().to_string().replace('\\', "\\\\");
    format!("\"{escaped}\"")
}

/// Quotes a value as a TOML basic string, escaping the backslash and double-quote
/// characters that would otherwise break the literal.
fn toml_basic_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Renders the `[storage.azure]` config block for `analyze`.
///
/// `account` and `container` originate from `--account` / `--container` (or env), so
/// they are escaped as TOML basic strings rather than spliced in raw: a stray `"` or
/// `\` would otherwise produce invalid TOML that `cargo-bench-history` then fails to
/// parse.
fn azure_config_toml(project: &str, account: &str, container: &str) -> String {
    let endpoint = format!("https://{account}.blob.core.windows.net");
    format!(
        "[project]\nid = \"{project}\"\n\n[storage.azure]\naccount = {}\n\
         container = {}\nendpoint = {}\n",
        toml_basic_string(account),
        toml_basic_string(container),
        toml_basic_string(&endpoint),
    )
}

/// The azcopy wildcard source that copies a directory's contents (preserving
/// sub-paths) without nesting them under the staging directory's own name.
fn wildcard_source(root: &Path) -> String {
    root.join("*").display().to_string()
}

/// Runs the Azure CLI, which on Windows is a `.cmd` shim that must go through
/// `cmd /C`, and elsewhere is directly executable. The caller's `logger` is threaded
/// through so `--verbose` runs surface the actual CLI invocation.
async fn az(args: &[&str], logger: Logger) -> Result<(), Error> {
    if cfg!(windows) {
        let mut all = vec!["/C", "az"];
        all.extend_from_slice(args);
        run_tool("cmd", &all, &[], logger).await
    } else {
        run_tool("az", args, &[], logger).await
    }
}

/// Runs an external tool with extra environment variables, failing on non-zero
/// exit and surfacing its stderr.
async fn run_tool(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    logger: Logger,
) -> Result<(), Error> {
    logger.detail(&format!("running {program} {}", args.join(" ")));
    let mut command = Command::new(program);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .await
        .map_err(|error| fail(format!("failed to run {program}: {error}")))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(fail(format!(
            "{program} {} failed:\nstdout: {stdout}\nstderr: {stderr}",
            args.join(" "),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_basic_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_basic_string("plain"), "\"plain\"");
        assert_eq!(toml_basic_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(toml_basic_string("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn azure_config_toml_escapes_account_and_container() {
        // Names carrying a quote and a backslash would break the generated TOML if
        // spliced in raw; the config must escape them instead.
        let config = azure_config_toml("stress", "acc\"t", "cont\\ainer");

        assert!(config.contains("account = \"acc\\\"t\""), "got: {config}");
        assert!(
            config.contains("container = \"cont\\\\ainer\""),
            "got: {config}"
        );
        // The account also forms the endpoint host, which is likewise escaped.
        assert!(
            config.contains("endpoint = \"https://acc\\\"t.blob.core.windows.net\""),
            "got: {config}"
        );
    }
}
