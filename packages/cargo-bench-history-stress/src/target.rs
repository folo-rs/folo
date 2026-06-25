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
    /// How external tools (`az`, `azcopy`) are executed. Production spawns real
    /// processes; tests substitute a recording double so the Azure orchestration
    /// is exercised without live credentials.
    runner: Runner,
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

/// How a [`StorageTarget`] runs the external tools its Azure path depends on.
#[derive(Debug)]
enum Runner {
    /// Spawns real external processes.
    Process,
    /// A test double that records invocations and returns a canned outcome.
    #[cfg(test)]
    Fake(std::sync::Arc<FakeRunner>),
}

/// Records the external-tool invocations a target attempts and yields a fixed
/// success or failure, so the Azure orchestration can be tested deterministically.
#[cfg(test)]
#[derive(Debug, Default)]
struct FakeRunner {
    /// Each recorded call as `[program, arg, arg, ...]`.
    calls: std::sync::Mutex<Vec<Vec<String>>>,
    /// When set, every call returns an error instead of success.
    fail: bool,
}

#[cfg(test)]
impl FakeRunner {
    /// Records the invocation and returns the configured outcome.
    fn run(&self, program: &str, args: &[&str]) -> Result<(), Error> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls
            .lock()
            .expect("the calls lock is not poisoned")
            .push(call);
        if self.fail {
            Err(fail(format!("fake runner failed: {program}")))
        } else {
            Ok(())
        }
    }
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
                    runner: Runner::Process,
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
                    runner: Runner::Process,
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
            runner: Runner::Process,
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
                self.az(
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
        self.run_tool(
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
                self.az(
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

    /// Runs the Azure CLI, which on Windows is a `.cmd` shim that must go through
    /// `cmd /C` and elsewhere is directly executable. The `logger` is threaded
    /// through so `--verbose` runs surface the actual CLI invocation.
    async fn az(&self, args: &[&str], logger: Logger) -> Result<(), Error> {
        if cfg!(windows) {
            let mut all = vec!["/C", "az"];
            all.extend_from_slice(args);
            self.run_tool("cmd", &all, &[], logger).await
        } else {
            self.run_tool("az", args, &[], logger).await
        }
    }

    /// Runs an external tool with extra environment variables, dispatching to a
    /// real process or, under test, to the recording double.
    async fn run_tool(
        &self,
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        logger: Logger,
    ) -> Result<(), Error> {
        logger.detail(&format!("running {program} {}", args.join(" ")));
        match &self.runner {
            Runner::Process => run_process(program, args, envs).await,
            #[cfg(test)]
            Runner::Fake(fake) => fake.run(program, args),
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

/// Runs an external tool with extra environment variables, failing on a non-zero
/// exit and surfacing its stdout and stderr.
async fn run_process(program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<(), Error> {
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
    use std::sync::Arc;

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

    /// A target whose external tools route to the supplied recording double.
    fn fake_azure_target(fake: &Arc<FakeRunner>, root: PathBuf) -> StorageTarget {
        StorageTarget {
            kind: Kind::Azure {
                account: "acct".to_owned(),
                container: "cont".to_owned(),
            },
            root,
            scratch: None,
            runner: Runner::Fake(Arc::clone(fake)),
        }
    }

    /// The recorded calls, each flattened to a single `program arg arg ...` string.
    fn recorded(fake: &FakeRunner) -> Vec<String> {
        fake.calls
            .lock()
            .expect("the calls lock is not poisoned")
            .iter()
            .map(|call| call.join(" "))
            .collect()
    }

    /// A command that runs `script` through the host's native shell and exits with
    /// its status.
    fn shell_command(script: &str) -> (&'static str, Vec<&str>) {
        if cfg!(windows) {
            ("cmd", vec!["/C", script])
        } else {
            ("sh", vec!["-c", script])
        }
    }

    #[test]
    fn label_names_the_backend() {
        let local = StorageTarget::local(None).expect("a temp local target is created");
        assert_eq!(local.label(), "local filesystem");

        let fake = Arc::new(FakeRunner::default());
        let azure = fake_azure_target(&fake, PathBuf::from("staging"));
        assert_eq!(azure.label(), "azure (acct/cont)");
    }

    #[test]
    fn wildcard_source_targets_directory_contents() {
        let source = wildcard_source(Path::new("seedroot"));
        // The contents are copied via a trailing `*`, not the directory itself.
        assert!(source.contains("seedroot"), "got: {source}");
        assert!(source.ends_with('*'), "got: {source}");
    }

    #[tokio::test]
    async fn provision_creates_the_container() {
        let fake = Arc::new(FakeRunner::default());
        let target = fake_azure_target(&fake, PathBuf::from("staging"));

        target
            .provision(Logger::new(false))
            .await
            .expect("provisioning succeeds with a passing runner");

        let calls = recorded(&fake);
        assert_eq!(calls.len(), 1, "got: {calls:?}");
        let call = calls.first().expect("one call was recorded");
        assert!(call.contains("storage container create"), "got: {call}");
        assert!(call.contains("--account-name acct"), "got: {call}");
        assert!(call.contains("--name cont"), "got: {call}");
    }

    #[tokio::test]
    async fn provision_surfaces_a_runner_failure() {
        let fake = Arc::new(FakeRunner {
            fail: true,
            ..FakeRunner::default()
        });
        let target = fake_azure_target(&fake, PathBuf::from("staging"));

        let result = target.provision(Logger::new(false)).await;

        assert!(result.is_err(), "a failing runner must fail provisioning");
    }

    #[tokio::test]
    async fn upload_copies_the_staged_tree_with_azcopy() {
        let fake = Arc::new(FakeRunner::default());
        let target = fake_azure_target(&fake, PathBuf::from("staging"));

        target
            .upload(Logger::new(false))
            .await
            .expect("uploading succeeds with a passing runner");

        let calls = recorded(&fake);
        assert_eq!(calls.len(), 1, "got: {calls:?}");
        let call = calls.first().expect("one call was recorded");
        assert!(call.starts_with("azcopy copy"), "got: {call}");
        assert!(
            call.contains("https://acct.blob.core.windows.net/cont"),
            "got: {call}"
        );
    }

    #[tokio::test]
    async fn upload_is_a_noop_for_local_storage() {
        let target = StorageTarget::local(None).expect("a temp local target is created");

        target
            .upload(Logger::new(false))
            .await
            .expect("a local upload is a no-op");
    }

    #[tokio::test]
    async fn cleanup_deletes_the_container_when_not_kept() {
        let fake = Arc::new(FakeRunner::default());
        let mut target = fake_azure_target(&fake, PathBuf::from("staging"));

        target
            .cleanup(false, Logger::new(false))
            .await
            .expect("cleanup succeeds with a passing runner");

        let calls = recorded(&fake);
        assert_eq!(calls.len(), 1, "got: {calls:?}");
        let call = calls.first().expect("one call was recorded");
        assert!(call.contains("storage container delete"), "got: {call}");
    }

    #[tokio::test]
    async fn cleanup_keeps_the_container_when_requested() {
        let fake = Arc::new(FakeRunner::default());
        let mut target = fake_azure_target(&fake, PathBuf::from("staging"));

        target
            .cleanup(true, Logger::new(false))
            .await
            .expect("keeping the container succeeds");

        // Keeping the container must not invoke the deletion tool.
        assert!(recorded(&fake).is_empty(), "got: {:?}", recorded(&fake));
    }

    #[tokio::test]
    async fn cleanup_persists_a_kept_local_store() {
        let mut target = StorageTarget::local(None).expect("a temp local target is created");
        let root = target.seed_root().to_path_buf();
        assert!(root.exists(), "the fresh temp store exists");

        target
            .cleanup(true, Logger::new(false))
            .await
            .expect("keeping the local store succeeds");
        drop(target);

        assert!(root.exists(), "a kept local store survives the target drop");
        std::fs::remove_dir_all(&root).expect("the kept store is removable");
    }

    #[tokio::test]
    async fn cleanup_removes_an_unkept_local_store() {
        let mut target = StorageTarget::local(None).expect("a temp local target is created");
        let root = target.seed_root().to_path_buf();
        assert!(root.exists(), "the fresh temp store exists");

        target
            .cleanup(false, Logger::new(false))
            .await
            .expect("local cleanup succeeds");
        drop(target);

        assert!(!root.exists(), "an unkept local store is removed on drop");
    }

    #[tokio::test]
    async fn az_preserves_the_cli_invocation() {
        let fake = Arc::new(FakeRunner::default());
        let target = fake_azure_target(&fake, PathBuf::from("staging"));

        target
            .az(&["account", "show"], Logger::new(false))
            .await
            .expect("the fake runner reports success");

        let calls = recorded(&fake);
        assert_eq!(calls.len(), 1, "got: {calls:?}");
        // The `az account show` invocation survives regardless of the platform's
        // `cmd /C` wrapper.
        let call = calls.first().expect("one call was recorded");
        assert!(call.contains("az account show"), "got: {call}");
    }

    #[tokio::test]
    async fn run_process_accepts_a_zero_exit() {
        let (program, args) = shell_command("exit 0");
        run_process(program, &args, &[])
            .await
            .expect("a zero-exit command succeeds");
    }

    #[tokio::test]
    async fn run_process_rejects_a_nonzero_exit() {
        let (program, args) = shell_command("exit 1");
        let result = run_process(program, &args, &[]).await;
        assert!(result.is_err(), "a non-zero exit must be an error");
    }

    #[tokio::test]
    async fn run_process_rejects_a_missing_program() {
        let result = run_process("definitely-not-a-real-program-zzz", &[], &[]).await;
        assert!(result.is_err(), "a missing program must be an error");
    }
}
