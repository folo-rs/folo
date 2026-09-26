use std::any::type_name;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fmt, fs, io};

use crp_diag::{DiagnosticSink, diagnostic};
use crp_workspace::git::GitRepo;
use ohno::AppError;
use tempfile::TempDir;
use toml_edit::DocumentMut;

use crate::archive::Staging;
use crate::command::{cancelled, capture, capture_cleanup};
use crate::request::{Artifacts, BuildRequest, ExecutionContext, InvalidPlan, SourceProvider};
use crate::source::{Metadata, executable};

/// Native state belongs to the controller; source worktrees supply only build inputs.
pub struct Native {
    controller: PathBuf,
    workspace: PathBuf,
    output: PathBuf,
    target: PathBuf,
    triple: String,
    source_provider: Box<dyn SourceProvider>,
    diagnostics: Arc<dyn DiagnosticSink>,
    source: Option<TempDir>,
    metadata: Option<Metadata>,
    staging: Option<Staging>,
    deadline: Instant,
    first_in_source: bool,
}

impl fmt::Debug for Native {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("controller", &self.controller)
            .field("workspace", &self.workspace)
            .field("triple", &self.triple)
            .finish_non_exhaustive()
    }
}

/// Retains a directory cleanup failure alongside the original Git cleanup error.
#[ohno::error]
#[display("Source directory cleanup also failed: {directory}")]
struct CleanupFailed {
    directory: io::Error,
}

// Queries and owned cleanup use a separate budget from each native item.
const QUERY_BUDGET: Duration = Duration::from_secs(300);
const ITEM_BUDGET: Duration = Duration::from_hours(1);

impl Native {
    /// Carries the current item budget unchanged into delivery and its postcondition query.
    #[must_use]
    pub fn execution_context(&self) -> ExecutionContext<'_> {
        ExecutionContext {
            directory: &self.controller,
            deadline: self.deadline,
        }
    }

    pub fn artifacts(&self) -> Result<Artifacts<'_>, AppError> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No staged archive".to_owned()))?;
        Ok(Artifacts {
            archive: &staging.archive,
            checksum: &staging.checksum,
        })
    }

    // Native clock acquisition belongs to execution, not deterministic argument construction.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub fn deadline_after(budget: Duration) -> Instant {
        deadline_after(budget)
    }

    // Source/cached-target discovery requires filesystem and subprocess integration coverage.
    #[cfg_attr(test, mutants::skip)]
    pub fn new(
        controller: PathBuf,
        output: PathBuf,
        triple: String,
        source_provider: Box<dyn SourceProvider>,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self, AppError> {
        // Standard Win32 paths are required by PowerShell's script authorization/file APIs.
        let controller = dunce::canonicalize(controller)?;
        fs::create_dir_all(&output)?;
        let output = dunce::canonicalize(output)?;
        let metadata = capture(
            OsStr::new("cargo"),
            &strings(&[
                "metadata",
                "--locked",
                "--offline",
                "--no-deps",
                "--format-version",
                "1",
            ]),
            &controller,
            &[],
            &diagnostics,
            deadline_after(QUERY_BUDGET),
        )?;
        let metadata: Metadata = serde_json::from_str(&metadata)?;
        let target = metadata.target_directory;
        if !target.is_absolute() {
            return Err(
                InvalidPlan::new("Cargo target directory must be absolute".to_owned()).into(),
            );
        }
        let git = GitRepo::discover(&controller)?;
        let workspace = PathBuf::from(git.prefix());
        let controller = dunce::canonicalize(git.root())?;
        Ok(Self {
            controller,
            workspace,
            output,
            target,
            triple,
            source_provider,
            diagnostics,
            source: None,
            metadata: None,
            staging: None,
            deadline: deadline_after(ITEM_BUDGET),
            first_in_source: false,
        })
    }

    // Every build command uses source cwd and the controller cache, without upload credentials.
    #[cfg_attr(test, mutants::skip)]
    fn source_command(&self, program: &str, arguments: &[OsString]) -> Result<String, AppError> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No source worktree is prepared".to_owned()))?
            .path()
            .join("source")
            .join(&self.workspace);
        capture(
            OsStr::new(program),
            arguments,
            &source,
            &[
                ("CARGO_TARGET_DIR", self.target.as_os_str()),
                ("GIT_LFS_SKIP_SMUDGE", OsStr::new("1")),
            ],
            &self.diagnostics,
            self.deadline,
        )
    }
}

impl Native {
    #[must_use]
    pub fn cancelled(&self) -> bool {
        cancelled()
    }
    #[cfg_attr(test, mutants::skip)] // Git/rustup boundary, exercised by pinned-source integration.
    pub fn prepare(&mut self, binary: &BuildRequest) -> Result<(), AppError> {
        self.deadline = deadline_after(ITEM_BUDGET);
        self.first_in_source = true;
        let environment = [("GIT_LFS_SKIP_SMUDGE", OsStr::new("1"))];
        let object = format!("{}^{{commit}}", binary.source_sha);
        // Try the exact object locally before a bounded fetch; never replace it with moving main.
        if capture(
            OsStr::new("git"),
            &strings(&["cat-file", "-e", &object]),
            &self.controller,
            &environment,
            &self.diagnostics,
            self.deadline,
        )
        .inspect_err(|error| {
            diagnostic(
                self.diagnostics.as_ref(),
                &format!("Exact source object is unavailable locally; fetching it: {error}\n"),
            );
        })
        .is_err()
        {
            self.source_provider
                .fetch(&self.controller, &binary.source_sha, self.deadline)?;
        }
        let source = tempfile::Builder::new()
            .prefix("folo-release-source-")
            .tempdir()?;
        let path = source.path().join("source");
        self.source = Some(source);
        capture(
            OsStr::new("git"),
            &[
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                path.as_os_str().to_owned(),
                binary.source_sha.clone().into(),
            ],
            &self.controller,
            &environment,
            &self.diagnostics,
            self.deadline,
        )?;
        let head = self.source_command("git", &strings(&["rev-parse", "HEAD"]))?;
        if head.trim() != binary.source_sha {
            return Err(InvalidPlan::new(
                "Source worktree HEAD differs from the release tag".to_owned(),
            )
            .into());
        }
        self.source_command(
            "git",
            &strings(&["ls-files", "--error-unmatch", "Cargo.lock"]),
        )?;
        let source_workspace = path.join(&self.workspace);
        let source_toolchain = source_workspace
            .ancestors()
            .take_while(|directory| directory.starts_with(&path))
            .map(|directory| directory.join("rust-toolchain.toml"))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| InvalidPlan::new(
                "The source workspace requires a tracked rust-toolchain.toml within its repository".to_owned()
            ))?;
        self.source_command(
            "git",
            &[
                "ls-files".into(),
                "--error-unmatch".into(),
                "--".into(),
                source_toolchain.as_os_str().to_owned(),
            ],
        )?;
        let source_pin: DocumentMut = fs::read_to_string(source_toolchain)?.parse()?;
        let channel = source_pin
            .get("toolchain")
            .and_then(|toolchain| toolchain.get("channel"))
            .and_then(toml_edit::Item::as_str)
            .ok_or_else(|| InvalidPlan::new("Source toolchain requires a channel".to_owned()))?;
        // Rustup reads the tracked source manifest, including its component selection.
        // No controller-repository script is needed by an installed release tool.
        self.source_command(
            "rustup",
            &strings(&["toolchain", "install", "--profile", "minimal"]),
        )?;
        let compiler = self.source_command("rustc", &strings(&["--version", "--verbose"]))?;
        let host = compiler
            .lines()
            .find_map(|line| line.strip_prefix("host: "));
        if host != Some(self.triple.as_str()) {
            return Err(InvalidPlan::new(format!(
                "Native compiler host {host:?} does not match {}",
                self.triple
            ))
            .into());
        }
        self.source_command(
            "rustup",
            &strings(&["target", "add", &self.triple, "--toolchain", channel]),
        )?;
        let metadata = self.source_command(
            "cargo",
            &strings(&["metadata", "--locked", "--no-deps", "--format-version", "1"]),
        )?;
        self.metadata = Some(serde_json::from_str(&metadata)?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Cargo invocation/staging boundary; artifact decisions are pure.
    pub fn build(&mut self, binary: &BuildRequest) -> Result<(), AppError> {
        if !self.first_in_source {
            self.deadline = deadline_after(ITEM_BUDGET);
        }
        self.first_in_source = false;
        self.staging = None;
        let package = self
            .metadata
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("Missing source metadata".to_owned()))?
            .package(binary)?;
        let messages = self.source_command(
            "cargo",
            &strings(&[
                "build",
                "--release",
                "--locked",
                "--target",
                &self.triple,
                "--package",
                &binary.name,
                "--bin",
                &binary.bin,
                "--message-format=json-render-diagnostics",
            ]),
        )?;
        let executable = executable(&messages, &package.id, &binary.bin)?;
        self.staging = Some(Staging::create(
            &self.output,
            binary,
            &self.triple,
            &executable,
        )?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Native archive tools and file hashing use integration fixtures.
    pub fn package(&mut self, _binary: &BuildRequest) -> Result<(), AppError> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No staged executable".to_owned()))?;
        let filename = staging
            .executable
            .file_name()
            .ok_or_else(|| InvalidPlan::new("Missing binary filename".to_owned()))?;
        #[cfg(windows)]
        let (program, arguments) = (
            "7za",
            vec![
                "a".into(),
                "-tzip".into(),
                staging.archive.as_os_str().to_owned(),
                filename.to_owned(),
            ],
        );
        #[cfg(unix)]
        let (program, arguments) = (
            "zip",
            vec![
                "-q".into(),
                staging.archive.as_os_str().to_owned(),
                filename.to_owned(),
            ],
        );
        capture(
            OsStr::new(program),
            &arguments,
            &staging.directory,
            &[],
            &self.diagnostics,
            self.deadline,
        )?;
        staging.checksum()
    }

    #[cfg_attr(test, mutants::skip)] // Owned-worktree cleanup is a Git/filesystem boundary.
    pub fn cleanup(&mut self) -> Result<(), AppError> {
        self.metadata = None;
        self.staging = None;
        if let Some(source) = self.source.take() {
            // Even an interrupted add can register a worktree before returning a failure.
            // Always attempt Git cleanup for this owned path, then remove its temporary parent.
            let result = capture_cleanup(
                &[
                    "worktree".into(),
                    "remove".into(),
                    "--force".into(),
                    source.path().join("source").into_os_string(),
                ],
                &self.controller,
                &self.diagnostics,
                deadline_after(QUERY_BUDGET),
            );
            return finish_cleanup(result, source.close());
        }
        Ok(())
    }
}

fn finish_cleanup(
    operation: Result<String, AppError>,
    directory: Result<(), io::Error>,
) -> Result<(), AppError> {
    match (operation, directory) {
        (Err(operation), Err(directory)) => {
            Err(CleanupFailed::caused_by(directory, operation).into())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(_), Ok(())) => Ok(()),
    }
}

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

// Real deadlines exist only in the native adapter, never in unit-test decisions.
#[cfg_attr(test, mutants::skip)]
fn deadline_after(budget: Duration) -> Instant {
    Instant::now()
        .checked_add(budget)
        .expect("bounded release deadline fits in Instant")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn cleanup_retains_each_failure_and_the_original_source() {
        finish_cleanup(Ok(String::new()), Ok(())).unwrap();
        let operation = || AppError::from(InvalidPlan::new("operation canary".to_owned()));
        let directory = || io::Error::other("directory canary");

        let error = finish_cleanup(Err(operation()), Ok(())).unwrap_err();
        assert!(error.find_source::<InvalidPlan>().is_some());
        let error = finish_cleanup(Ok(String::new()), Err(directory())).unwrap_err();
        assert!(error.find_source::<io::Error>().is_some());

        let error = finish_cleanup(Err(operation()), Err(directory())).unwrap_err();
        assert!(error.find_source::<InvalidPlan>().is_some());
        let combined = error.find_source::<CleanupFailed>().unwrap();
        assert_eq!(combined.directory.kind(), io::ErrorKind::Other);
        // Both independent diagnostics must survive serialization into the batch outcome.
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("operation canary"));
        assert!(diagnostic.contains("directory canary"));
    }
}
