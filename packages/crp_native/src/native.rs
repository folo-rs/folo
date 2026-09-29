use std::any::type_name;
use std::ffi::{OsStr, OsString};
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fmt, fs, io};

use crp_diag::{DiagnosticSink, Quotable, diagnostic};
use crp_workspace::git::{GitRepo, TreeEntry, tree_mode};
use ohno::AppError;
use tempfile::TempDir;

use crate::archive::Staging;
use crate::command::{cancelled, capture, capture_cleanup};
use crate::request::InvalidPlan;
use crate::source::{Metadata, executable};
use crate::{Artifacts, BuildRequest, ExecutionContext, SourceProvider};

/// Owns source worktrees and staged artifacts for sequential native execution.
///
/// The controller supplies state and caches; immutable source worktrees supply build inputs.
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
    // Preparation consumes the first item's deadline; subsequent items receive their own.
    // Ref: docs/implementation.md, "Native execution".
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

    #[must_use]
    pub fn cancelled(&self) -> bool {
        cancelled()
    }
    #[cfg_attr(test, mutants::skip)] // Git/rustup boundary, exercised by pinned-source integration.
    pub fn prepare(&mut self, binary: &BuildRequest) -> Result<(), AppError> {
        self.deadline = deadline_after(ITEM_BUDGET);
        self.first_in_source = true;
        // Source acquisition does not automatically hydrate LFS pointers or add an LFS
        // authentication/download phase. Builds use their ordinary declared prerequisites.
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
        self.verify_toolchain(&path, &source_toolchain, &binary.source_sha)?;
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
        let metadata = self.source_command(
            "cargo",
            &strings(&["metadata", "--locked", "--no-deps", "--format-version", "1"]),
        )?;
        self.metadata = Some(serde_json::from_str(&metadata)?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Filesystem and committed-tree observations are native boundaries.
    fn verify_toolchain(
        &self,
        source: &Path,
        toolchain: &Path,
        commit: &str,
    ) -> Result<(), AppError> {
        let relative = toolchain.strip_prefix(source).map_err(|error| {
            InvalidToolchain::caused_by(toolchain.to_owned(), "outside the source worktree", error)
        })?;
        // Git mode is authoritative even when core.symlinks=false materializes a link as text.
        // Literal paths also prevent valid workspace characters from becoming pathspec operators.
        let tree = self.source_command(
            "git",
            &[
                "--literal-pathspecs".into(),
                "-C".into(),
                source.as_os_str().to_owned(),
                "ls-tree".into(),
                "-z".into(),
                commit.into(),
                "--".into(),
                relative.as_os_str().to_owned(),
            ],
        )?;
        let metadata = fs::symlink_metadata(toolchain).map_err(|error| {
            InvalidToolchain::caused_by(toolchain.to_owned(), "cannot inspect source input", error)
        })?;
        let root = dunce::canonicalize(source).map_err(|error| {
            InvalidToolchain::caused_by(
                toolchain.to_owned(),
                "cannot resolve source worktree",
                error,
            )
        })?;
        let resolved = dunce::canonicalize(toolchain).map_err(|error| {
            InvalidToolchain::caused_by(toolchain.to_owned(), "cannot resolve source input", error)
        })?;
        if !regular_toolchain_input(
            &tree,
            metadata.file_type().is_file(),
            resolved.starts_with(root),
        ) {
            return Err(InvalidToolchain::new(
                toolchain.to_owned(),
                "requires a committed regular file contained in the source worktree",
            )
            .into());
        }
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Cargo invocation/staging boundary; artifact decisions are pure.
    pub fn build(&mut self, binary: &BuildRequest) -> Result<(), AppError> {
        // Only later builds reset: preparation and the first build share one item deadline,
        // which publication also carries unchanged through upload and its verification.
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
            self.deadline,
        )?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Archive filesystem effects use integration fixtures.
    pub fn package(&mut self, _binary: &BuildRequest) -> Result<(), AppError> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No staged executable".to_owned()))?;
        staging.package(self.deadline)
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

impl Drop for Native {
    #[cfg_attr(test, mutants::skip)] // Last-resort owned-worktree cleanup is an integration boundary.
    fn drop(&mut self) {
        // Explicit batch finalization retains structured results. This fallback also disposes
        // the worktree if command diagnostics or another operation unwinds unexpectedly.
        if self.source.is_some()
            && let Err(error) = self.cleanup()
        {
            let diagnostics = &self.diagnostics;
            let message = format!("Native source cleanup failed: {error}\n");
            _ = catch_unwind(|| diagnostics.write(&message));
        }
    }
}

/// Retains a directory cleanup failure alongside the original Git cleanup error.
#[ohno::error]
#[display("Source directory cleanup also failed: {directory}")]
struct CleanupFailed {
    directory: io::Error,
}

/// A selected toolchain input must be fixed by the source commit, not a mutable link target.
#[ohno::error]
#[display("Invalid source toolchain {}: {reason}", path.quoted())]
struct InvalidToolchain {
    path: PathBuf,
    reason: &'static str,
}

// Discovery and owned cleanup are short operations, with a conservative last-chance allowance
// for loaded machines. Cleanup receives a fresh allowance even after item cancellation.
const QUERY_BUDGET: Duration = Duration::from_secs(300);
// A conservative per-item hang guard, not a reservation within the workflow's separate ceiling.
const ITEM_BUDGET: Duration = Duration::from_hours(1);

fn regular_toolchain_input(tree: &str, regular: bool, contained: bool) -> bool {
    let mut records = tree.trim_end_matches(['\r', '\n']).split_terminator('\0');
    let entry = records.next().and_then(TreeEntry::parse);
    regular
        && contained
        && entry
            .is_some_and(|entry| entry.mode == tree_mode(false) || entry.mode == tree_mode(true))
        && records.next().is_none()
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

    #[test]
    fn toolchain_tree_requires_one_regular_entry() {
        for executable in [false, true] {
            let record = format!(
                "{} blob {}\trust-toolchain.toml\0\n",
                tree_mode(executable),
                "a".repeat(40)
            );
            for regular in [false, true] {
                for contained in [false, true] {
                    assert_eq!(
                        regular_toolchain_input(&record, regular, contained),
                        regular && contained
                    );
                }
            }
        }
        // Git's symlink and submodule modes must not be accepted as toolchain file contents.
        for mode in ["120000", "160000"] {
            assert!(!regular_toolchain_input(
                &format!("{mode} blob a\trust-toolchain.toml\0"),
                true,
                true
            ));
        }
        for value in ["", "malformed"] {
            assert!(!regular_toolchain_input(value, true, true));
        }
        assert!(!regular_toolchain_input(
            concat!("100644 blob a\tone\0", "100644 blob b\ttwo\0",),
            true,
            true
        ));
    }
}
